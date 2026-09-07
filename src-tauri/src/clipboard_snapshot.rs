use std::{
    io::{Read, Write},
    time::Duration,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardSnapshot {
    pub sequence_number: u32,
    pub formats: Vec<ClipboardFormatData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardFormatData {
    pub format_id: u32,
    pub format_name: Option<String>,
    pub data: Vec<u8>,
}

pub const MAX_CLIPBOARD_ITEM_BYTES: usize = 8 * 1024 * 1024; // 8 MiB
pub const MAX_CLIPBOARD_TOTAL_BYTES: usize = 32 * 1024 * 1024; // 32 MiB

/// Runs in the isolated helper process mode (`--clipboard-snapshot`).
/// Reads clipboard data and writes serialized snapshot to standard output pipe.
pub fn run_helper() {
    #[cfg(windows)]
    {
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardFormatNameW,
            GetClipboardSequenceNumber, OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
        use windows::Win32::Foundation::HGLOBAL;

        let sequence_number = unsafe { GetClipboardSequenceNumber() };

        let opened = unsafe { OpenClipboard(None) };
        if opened.is_err() {
            std::process::exit(1);
        }

        let mut formats = Vec::new();
        let mut total_bytes = 0usize;
        let mut current_format = 0u32;

        unsafe {
            loop {
                current_format = EnumClipboardFormats(current_format);
                if current_format == 0 {
                    break;
                }

                // Query format name if custom
                let mut name_buf = [0u16; 256];
                let name_len = GetClipboardFormatNameW(current_format, &mut name_buf);
                let format_name = if name_len > 0 {
                    Some(String::from_utf16_lossy(&name_buf[..name_len as usize]))
                } else {
                    None
                };

                let handle_result = GetClipboardData(current_format);
                if let Ok(handle) = handle_result {
                    if !handle.0.is_null() {
                        let hglobal = HGLOBAL(handle.0);
                        let size = GlobalSize(hglobal);
                        if size > 0 && size <= MAX_CLIPBOARD_ITEM_BYTES {
                            if total_bytes + size <= MAX_CLIPBOARD_TOTAL_BYTES {
                                let ptr = GlobalLock(hglobal);
                                if !ptr.is_null() {
                                    let slice = std::slice::from_raw_parts(ptr as *const u8, size);
                                    formats.push(ClipboardFormatData {
                                        format_id: current_format,
                                        format_name,
                                        data: slice.to_vec(),
                                    });
                                    total_bytes += size;
                                    let _ = GlobalUnlock(hglobal);
                                }
                            }
                        }
                    }
                }
            }
            let _ = CloseClipboard();
        }

        let snapshot = ClipboardSnapshot {
            sequence_number,
            formats,
        };

        if let Ok(json) = serde_json::to_vec(&snapshot) {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(&(json.len() as u32).to_le_bytes());
            let _ = stdout.write_all(&json);
            let _ = stdout.flush();
        }
    }
}

/// Takes a clipboard snapshot using the isolated helper process with a 1-second timeout.
pub fn capture_clipboard_snapshot() -> Option<ClipboardSnapshot> {
    let current_exe = std::env::current_exe().ok()?;
    let mut child = std::process::Command::new(current_exe)
        .arg("--clipboard-snapshot")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;

    // Read with deadline in separate thread
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut len_buf = [0u8; 4];
        if stdout.read_exact(&mut len_buf).is_err() {
            return;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_CLIPBOARD_TOTAL_BYTES + 65536 {
            return;
        }
        let mut buf = vec![0u8; len];
        if stdout.read_exact(&mut buf).is_ok() {
            if let Ok(snapshot) = serde_json::from_slice::<ClipboardSnapshot>(&buf) {
                let _ = tx.send(snapshot);
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(1)) {
        Ok(snapshot) => {
            let _ = child.wait();
            Some(snapshot)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}
