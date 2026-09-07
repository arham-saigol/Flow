use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::error::{FlowError, Result};

pub const MAX_WAV_BYTES: usize = 9_600_044; // 5 minutes 16kHz mono 16-bit PCM WAV (44-byte header + 9,600,000 PCM bytes)
pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
pub const BITS_PER_SAMPLE: u16 = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolMetadata {
    pub schema_version: u32,
    pub capture_uuid: String,
    pub created_at: i64,
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm_bits: u16,
    pub partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Imported,
    Cancelled,
    Completed,
    Discarded,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalMarker {
    pub schema_version: u32,
    pub capture_uuid: String,
    pub created_at: i64,
    pub disposition: Disposition,
}

pub struct RecoverySpool {
    #[allow(dead_code)]
    dir: PathBuf,
    capture_uuid: String,
    pcm_part_path: PathBuf,
    meta_path: PathBuf,
    terminal_path: PathBuf,
    file: Option<File>,
    total_bytes: usize,
    last_sync: SystemTime,
}

impl RecoverySpool {
    pub fn new(recovery_dir: &Path, capture_uuid: &str) -> Result<Self> {
        fs::create_dir_all(recovery_dir).map_err(|e| {
            FlowError::Message(format!("Could not create recovery directory: {e}"))
        })?;

        let pcm_part_path = recovery_dir.join(format!("{capture_uuid}.pcm.part"));
        let meta_path = recovery_dir.join(format!("{capture_uuid}.json"));
        let terminal_path = recovery_dir.join(format!("{capture_uuid}.terminal.json"));

        let meta = SpoolMetadata {
            schema_version: 1,
            capture_uuid: capture_uuid.to_string(),
            created_at: now_secs(),
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            pcm_bits: BITS_PER_SAMPLE,
            partial: false,
        };

        // Write metadata file first
        let meta_json = serde_json::to_string_pretty(&meta)
            .map_err(|e| FlowError::Message(format!("Could not serialize spool metadata: {e}")))?;
        write_sync(&meta_path, meta_json.as_bytes())?;

        // Open PCM file
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&pcm_part_path)
            .map_err(|e| FlowError::Message(format!("Could not open spool PCM file: {e}")))?;

        Ok(Self {
            dir: recovery_dir.to_path_buf(),
            capture_uuid: capture_uuid.to_string(),
            pcm_part_path,
            meta_path,
            terminal_path,
            file: Some(file),
            total_bytes: 0,
            last_sync: SystemTime::now(),
        })
    }

    pub fn write_pcm(&mut self, pcm_bytes: &[u8]) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            // Bound PCM bytes to max 5 minutes
            let remaining = (MAX_WAV_BYTES - 44).saturating_sub(self.total_bytes);
            let to_write = pcm_bytes.len().min(remaining);
            if to_write > 0 {
                file.write_all(&pcm_bytes[..to_write])
                    .map_err(|e| FlowError::Message(format!("Spool write error: {e}")))?;
                self.total_bytes += to_write;
            }

            if self.last_sync.elapsed().map(|d| d >= Duration::from_secs(1)).unwrap_or(true) {
                let _ = file.sync_data();
                self.last_sync = SystemTime::now();
            }
        }
        Ok(())
    }

    pub fn finalize_wav(&mut self) -> Result<(Vec<u8>, i64)> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .map_err(|e| FlowError::Message(format!("Could not flush spool: {e}")))?;
            file.sync_all()
                .map_err(|e| FlowError::Message(format!("Could not sync spool: {e}")))?;
        }

        let mut pcm = fs::read(&self.pcm_part_path)
            .map_err(|e| FlowError::Message(format!("Could not read spool PCM: {e}")))?;

        // Ensure 16-bit alignment (truncate trailing orphan single byte)
        if pcm.len() % 2 != 0 {
            pcm.pop();
        }

        let sample_count = pcm.len() / 2;
        let duration_ms = ((sample_count as u64 * 1000) / SAMPLE_RATE as u64) as i64;
        let wav = build_wav_header(&pcm, SAMPLE_RATE, CHANNELS, BITS_PER_SAMPLE);

        Ok((wav, duration_ms))
    }

    pub fn mark_terminal(&self, disposition: Disposition) -> Result<()> {
        let marker = TerminalMarker {
            schema_version: 1,
            capture_uuid: self.capture_uuid.clone(),
            created_at: now_secs(),
            disposition,
        };
        let marker_json = serde_json::to_string_pretty(&marker)
            .map_err(|e| FlowError::Message(format!("Could not serialize terminal marker: {e}")))?;
        write_sync_rename(&self.terminal_path, marker_json.as_bytes())
    }

    pub fn cleanup(&mut self) -> Result<()> {
        self.file.take(); // Close PCM file handle
        let _ = fs::remove_file(&self.pcm_part_path);
        let _ = fs::remove_file(&self.meta_path);
        let _ = fs::remove_file(&self.terminal_path);
        Ok(())
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn build_wav_header(pcm: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let mut wav = Vec::with_capacity(44 + pcm.len());
    let byte_rate = sample_rate * channels as u32 * (bits_per_sample as u32 / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_len = pcm.len() as u32;
    let riff_chunk_size = 36 + data_len;

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_chunk_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size for PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat 1 = PCM
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);

    wav
}

pub fn write_sync(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|e| FlowError::Message(format!("Could not create file {}: {e}", path.display())))?;
    file.write_all(content)
        .map_err(|e| FlowError::Message(format!("Could not write file: {e}")))?;
    file.flush()
        .map_err(|e| FlowError::Message(format!("Could not flush file: {e}")))?;
    file.sync_all()
        .map_err(|e| FlowError::Message(format!("Could not sync file: {e}")))?;
    Ok(())
}

pub fn write_sync_rename(dest_path: &Path, content: &[u8]) -> Result<()> {
    let parent = dest_path.parent().unwrap_or(Path::new("."));
    let temp_name = format!("{}.tmp.{}", dest_path.file_name().unwrap().to_string_lossy(), now_secs());
    let temp_path = parent.join(temp_name);

    write_sync(&temp_path, content)?;

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

        let from_wide: Vec<u16> = temp_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let to_wide: Vec<u16> = dest_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

        unsafe {
            MoveFileExW(
                windows::core::PCWSTR(from_wide.as_ptr()),
                windows::core::PCWSTR(to_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
            .map_err(|e| {
                let _ = fs::remove_file(&temp_path);
                FlowError::Windows(format!("Atomic move failed: {e}"))
            })?;
        }
    }

    #[cfg(not(windows))]
    {
        fs::rename(&temp_path, dest_path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            FlowError::Message(format!("Rename failed: {e}"))
        })?;
    }

    Ok(())
}

/// Discovers recovery spools on startup and imports unmarked spools into the database.
pub fn scan_and_import_spools(
    recovery_dir: &Path,
    database: &crate::database::Database,
) -> Result<()> {
    if !recovery_dir.exists() {
        return Ok(());
    }

    let entries = match fs::read_dir(recovery_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "json" && !path.to_string_lossy().ends_with(".terminal.json") {
                // This is a `<uuid>.json` metadata file
                let capture_uuid = path.file_stem().unwrap().to_string_lossy().to_string();
                let terminal_path = recovery_dir.join(format!("{capture_uuid}.terminal.json"));
                let pcm_part_path = recovery_dir.join(format!("{capture_uuid}.pcm.part"));

                // If a terminal marker exists, check disposition
                if terminal_path.exists() {
                    if let Ok(content) = fs::read_to_string(&terminal_path) {
                        if let Ok(marker) = serde_json::from_str::<TerminalMarker>(&content) {
                            match marker.disposition {
                                Disposition::Cancelled | Disposition::Completed | Disposition::Discarded | Disposition::Expired => {
                                    // Prohibited from import: clean up PCM and metadata
                                    let _ = fs::remove_file(&pcm_part_path);
                                    let _ = fs::remove_file(&path);
                                    let _ = fs::remove_file(&terminal_path);
                                    continue;
                                }
                                Disposition::Imported => {
                                    // Already imported: clean up data files if database has it
                                    let _ = fs::remove_file(&pcm_part_path);
                                    let _ = fs::remove_file(&path);
                                    continue;
                                }
                            }
                        }
                    }
                }

                // Unmarked spool: check if PCM file exists
                if pcm_part_path.exists() {
                    if let Ok(mut pcm) = fs::read(&pcm_part_path) {
                        if pcm.len() % 2 != 0 {
                            pcm.pop();
                        }
                        if pcm.len() >= 3200 { // at least 100ms
                            let sample_count = pcm.len() / 2;
                            let duration_ms = ((sample_count as u64 * 1000) / SAMPLE_RATE as u64) as i64;
                            let wav = build_wav_header(&pcm, SAMPLE_RATE, CHANNELS, BITS_PER_SAMPLE);

                            let _ = database.insert_pending_recording(
                                &capture_uuid,
                                &wav,
                                duration_ms,
                                true,
                                Some("storage_recovered"),
                                "copy_only",
                            );
                        }
                    }
                    // Write imported marker and delete spool files
                    let marker = TerminalMarker {
                        schema_version: 1,
                        capture_uuid: capture_uuid.clone(),
                        created_at: now_secs(),
                        disposition: Disposition::Imported,
                    };
                    if let Ok(marker_json) = serde_json::to_string_pretty(&marker) {
                        let _ = write_sync_rename(&terminal_path, marker_json.as_bytes());
                    }
                    let _ = fs::remove_file(&pcm_part_path);
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    Ok(())
}

use std::time::Duration;
