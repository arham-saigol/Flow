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
        fs::create_dir_all(recovery_dir)
            .map_err(|e| FlowError::Message(format!("Could not create recovery directory: {e}")))?;

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

            if self
                .last_sync
                .elapsed()
                .map(|d| d >= Duration::from_secs(1))
                .unwrap_or(true)
            {
                let _ = file.sync_data();
                self.last_sync = SystemTime::now();
            }
        }
        Ok(())
    }

    pub fn flush_and_sync(&mut self) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.flush()
                .map_err(|e| FlowError::Message(format!("Could not flush spool: {e}")))?;
            file.sync_all()
                .map_err(|e| FlowError::Message(format!("Could not sync spool: {e}")))?;
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

pub fn build_wav_header(
    pcm: &[u8],
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
) -> Vec<u8> {
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
        .map_err(|e| {
            FlowError::Message(format!("Could not create file {}: {e}", path.display()))
        })?;
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
    let temp_name = format!(
        "{}.tmp.{}",
        dest_path.file_name().unwrap().to_string_lossy(),
        now_secs()
    );
    let temp_path = parent.join(temp_name);

    write_sync(&temp_path, content)?;

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };

        let from_wide: Vec<u16> = temp_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let to_wide: Vec<u16> = dest_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

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

pub fn quarantine_spool(recovery_dir: &Path, capture_uuid: &str) {
    let quarantine_dir = recovery_dir.join("quarantine");
    let _ = fs::create_dir_all(&quarantine_dir);
    for name in &[
        format!("{capture_uuid}.json"),
        format!("{capture_uuid}.pcm.part"),
        format!("{capture_uuid}.terminal.json"),
    ] {
        let src = recovery_dir.join(name);
        if src.exists() {
            let dest = quarantine_dir.join(name);
            let _ = fs::rename(&src, &dest);
        }
    }
}

pub fn calculate_recovery_dir_usage(recovery_dir: &Path) -> (usize, u64) {
    if !recovery_dir.exists() {
        return (0, 0);
    }
    let mut uuids = std::collections::HashSet::new();
    let mut total_bytes = 0u64;

    fn walk(dir: &Path, uuids: &mut std::collections::HashSet<String>, total_bytes: &mut u64) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, uuids, total_bytes);
                } else if p.is_file() {
                    if let Ok(meta) = entry.metadata() {
                        *total_bytes += meta.len();
                    }
                    if let Some(stem) = p.file_stem() {
                        let s = stem.to_string_lossy();
                        let base = s.split('.').next().unwrap_or(&s);
                        if !base.is_empty() {
                            uuids.insert(base.to_string());
                        }
                    }
                }
            }
        }
    }

    walk(recovery_dir, &mut uuids, &mut total_bytes);
    (uuids.len(), total_bytes)
}

/// Discovers recovery spools on startup and imports unmarked spools into the database.
pub fn scan_and_import_spools(
    recovery_dir: &Path,
    database: &crate::database::Database,
) -> Result<()> {
    if !recovery_dir.exists() {
        return Ok(());
    }

    let free_space = crate::database::get_available_disk_space(recovery_dir)?;
    if free_space < crate::database::MIN_VOLUME_FREE_BYTES_IMPORT {
        return Err(FlowError::QuotaExceeded(
            "Not enough free disk space to import recovery spools.".into(),
        ));
    }

    let entries = match fs::read_dir(recovery_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "json" && !path.to_string_lossy().ends_with(".terminal.json") {
                let capture_uuid = match path.file_stem() {
                    Some(s) => s.to_string_lossy().to_string(),
                    None => continue,
                };
                let terminal_path = recovery_dir.join(format!("{capture_uuid}.terminal.json"));
                let pcm_part_path = recovery_dir.join(format!("{capture_uuid}.pcm.part"));

                // Validate metadata file
                let meta_content = match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => {
                        quarantine_spool(recovery_dir, &capture_uuid);
                        continue;
                    }
                };
                let meta: SpoolMetadata = match serde_json::from_str(&meta_content) {
                    Ok(m) => m,
                    Err(_) => {
                        quarantine_spool(recovery_dir, &capture_uuid);
                        continue;
                    }
                };
                if meta.capture_uuid != capture_uuid
                    || meta.sample_rate != SAMPLE_RATE
                    || meta.channels != CHANNELS
                    || meta.pcm_bits != BITS_PER_SAMPLE
                {
                    quarantine_spool(recovery_dir, &capture_uuid);
                    continue;
                }

                // If a terminal marker exists, check disposition
                if terminal_path.exists() {
                    let terminal_content = match fs::read_to_string(&terminal_path) {
                        Ok(c) => c,
                        Err(_) => {
                            quarantine_spool(recovery_dir, &capture_uuid);
                            continue;
                        }
                    };
                    let marker: TerminalMarker = match serde_json::from_str(&terminal_content) {
                        Ok(m) => m,
                        Err(_) => {
                            quarantine_spool(recovery_dir, &capture_uuid);
                            continue;
                        }
                    };
                    if marker.capture_uuid != capture_uuid {
                        quarantine_spool(recovery_dir, &capture_uuid);
                        continue;
                    }

                    match marker.disposition {
                        Disposition::Cancelled
                        | Disposition::Completed
                        | Disposition::Discarded
                        | Disposition::Expired => {
                            let _ = fs::remove_file(&pcm_part_path);
                            let _ = fs::remove_file(&path);
                            let _ = fs::remove_file(&terminal_path);
                            continue;
                        }
                        Disposition::Imported => {
                            if database.has_capture_uuid(&capture_uuid).unwrap_or(false) {
                                let _ = fs::remove_file(&pcm_part_path);
                                let _ = fs::remove_file(&path);
                                // Finish cleanup by removing the terminal marker
                                // once the PCM and metadata files are handled.
                                let _ = fs::remove_file(&terminal_path);
                            } else {
                                quarantine_spool(recovery_dir, &capture_uuid);
                            }
                            continue;
                        }
                    }
                }

                // Unmarked spool: if the capture UUID is already recorded in
                // pending or history, the audio is already durable in the
                // database (the pending row stores the WAV, or the dictation
                // was delivered to history). Importing here would resurrect a
                // completed dictation as a new recoverable recording, so clean
                // the stale spool files instead.
                if database.has_capture_uuid(&capture_uuid).unwrap_or(false) {
                    let _ = fs::remove_file(&pcm_part_path);
                    let _ = fs::remove_file(&path);
                    continue;
                }

                // Unmarked spool: check if PCM file exists
                if pcm_part_path.exists() {
                    let pcm = match fs::read(&pcm_part_path) {
                        Ok(p) => p,
                        Err(_) => {
                            quarantine_spool(recovery_dir, &capture_uuid);
                            continue;
                        }
                    };
                    if pcm.len() < 3200 || pcm.len() > MAX_WAV_BYTES - 44 {
                        quarantine_spool(recovery_dir, &capture_uuid);
                        continue;
                    }

                    let mut aligned_pcm = pcm;
                    if aligned_pcm.len() % 2 != 0 {
                        aligned_pcm.pop();
                    }
                    let sample_count = aligned_pcm.len() / 2;
                    let duration_ms = ((sample_count as u64 * 1000) / SAMPLE_RATE as u64) as i64;
                    let wav =
                        build_wav_header(&aligned_pcm, SAMPLE_RATE, CHANNELS, BITS_PER_SAMPLE);

                    let insert_res = database.insert_pending_recording(
                        &capture_uuid,
                        &wav,
                        duration_ms,
                        true,
                        Some("storage_recovered"),
                        "copy_only",
                    );

                    if insert_res.is_err() {
                        // Preserve PCM and metadata on DB insert failure
                        continue;
                    }

                    // Write imported marker and delete spool files
                    let marker = TerminalMarker {
                        schema_version: 1,
                        capture_uuid: capture_uuid.clone(),
                        created_at: now_secs(),
                        disposition: Disposition::Imported,
                    };
                    if let Ok(marker_json) = serde_json::to_string_pretty(&marker) {
                        if write_sync_rename(&terminal_path, marker_json.as_bytes()).is_ok() {
                            let _ = fs::remove_file(&pcm_part_path);
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flow-test-recovery-{}", name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_probe_24_known_uuid_spool_is_deduped_not_reimported() {
        let dir = temp_dir("p24");
        let db_path = dir.join("test.sqlite3");
        let database = Database::open(&db_path).unwrap();

        let recovery_dir = dir.join("recovery");
        fs::create_dir_all(&recovery_dir).unwrap();

        let uuid = "p24-uuid";
        let mut spool = RecoverySpool::new(&recovery_dir, uuid).unwrap();
        let pcm_bytes = vec![0u8; 4000]; // 2000 samples
        spool.write_pcm(&pcm_bytes).unwrap();
        drop(spool);

        // Pre-insert the same UUID into pending_dictations: the audio is
        // already durable in the database row, so the scan must not import the
        // spool again (a duplicate pending row or a resurrected recording).
        database
            .insert_pending_recording(uuid, b"fake", 100, false, None, "copy_only")
            .unwrap();

        // Run scan_and_import_spools
        let res = scan_and_import_spools(&recovery_dir, &database);
        assert!(res.is_ok());

        // The existing pending row is untouched and no duplicate was created.
        let pending = database.pending_dictations().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].capture_uuid.as_deref(), Some(uuid));
        let record = database.pending_dictation(pending[0].id).unwrap();
        assert_eq!(record.wav.as_deref(), Some(b"fake".as_slice()));

        // The redundant spool files (metadata + PCM) were cleaned up.
        assert!(
            !recovery_dir.join(format!("{uuid}.pcm.part")).exists(),
            "spool PCM known to the database must be cleaned, not reimported"
        );
        assert!(!recovery_dir.join(format!("{uuid}.json")).exists());

        drop(database);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_unmarked_spool_with_known_uuid_is_cleaned_not_reimported() {
        let dir = temp_dir("p26");
        let db_path = dir.join("test.sqlite3");
        let database = Database::open(&db_path).unwrap();

        let recovery_dir = dir.join("recovery");
        fs::create_dir_all(&recovery_dir).unwrap();

        let uuid = "p26-uuid";
        let mut spool = RecoverySpool::new(&recovery_dir, uuid).unwrap();
        let pcm_bytes = vec![0u8; 4000];
        spool.write_pcm(&pcm_bytes).unwrap();
        drop(spool);

        // Simulate a completed dictation: the pending row was delivered to
        // history and then deleted, while its unmarked spool files remained.
        let id = database
            .insert_pending_recording(uuid, b"wav", 100, false, None, "copy_only")
            .unwrap();
        database
            .save_pending_transcript(id, "hello", "hello", None)
            .unwrap();
        database.save_pending_final(id, "hello", false).unwrap();
        database.save_pending_to_history(id, "30 days").unwrap();
        database.delete_pending(id).unwrap();

        let res = scan_and_import_spools(&recovery_dir, &database);
        assert!(res.is_ok());

        // The completed dictation must not reappear as a recoverable recording.
        assert!(
            database.pending_dictations().unwrap().is_empty(),
            "completed dictations must not be reimported as pending"
        );
        // The stale spool files are cleaned up.
        assert!(!recovery_dir.join(format!("{uuid}.pcm.part")).exists());
        assert!(!recovery_dir.join(format!("{uuid}.json")).exists());

        drop(database);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_probe_25_malformed_terminal_marker_quarantines() {
        let dir = temp_dir("p25");
        let db_path = dir.join("test.sqlite3");
        let database = Database::open(&db_path).unwrap();

        let recovery_dir = dir.join("recovery");
        fs::create_dir_all(&recovery_dir).unwrap();

        let uuid = "p25-uuid";
        let mut spool = RecoverySpool::new(&recovery_dir, uuid).unwrap();
        let pcm_bytes = vec![0u8; 4000];
        spool.write_pcm(&pcm_bytes).unwrap();
        drop(spool);

        // Corrupt the terminal marker
        let terminal_path = recovery_dir.join(format!("{uuid}.terminal.json"));
        fs::write(&terminal_path, b"{ not valid json").unwrap();

        // Run scan_and_import_spools
        let res = scan_and_import_spools(&recovery_dir, &database);
        assert!(res.is_ok());

        // Must NOT be imported into DB
        let in_db = database.has_capture_uuid(uuid).unwrap();
        assert!(!in_db, "Malformed marker must not be imported into DB");

        // Must be moved to quarantine folder
        let quarantine_dir = recovery_dir.join("quarantine");
        assert!(
            quarantine_dir
                .join(format!("{uuid}.terminal.json"))
                .exists(),
            "Corrupt marker must be in quarantine"
        );
        assert!(
            quarantine_dir.join(format!("{uuid}.pcm.part")).exists(),
            "PCM must be in quarantine"
        );

        drop(database);
        let _ = fs::remove_dir_all(dir);
    }
}
