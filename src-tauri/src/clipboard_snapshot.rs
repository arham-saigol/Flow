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

pub fn is_hglobal_clipboard_format(format: u32) -> bool {
    !matches!(format, 2 | 3 | 9 | 14 | 128 | 130 | 131 | 142 | 512..=1023)
}

/// Runs in the isolated helper process mode (`--clipboard-snapshot`).
/// Reads clipboard data and writes serialized snapshot to standard output pipe.
pub fn run_helper() {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{GetLastError, ERROR_SUCCESS, HGLOBAL};
        use windows::Win32::System::DataExchange::{
            CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber,
            OpenClipboard,
        };
        use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

        let sequence_number = unsafe { GetClipboardSequenceNumber() };

        let opened = unsafe { OpenClipboard(None) };
        if opened.is_err() {
            std::process::exit(1);
        }

        let mut formats = Vec::new();
        let mut total_bytes = 0usize;
        let mut current_format = 0u32;
        let mut success = true;

        unsafe {
            loop {
                windows::Win32::Foundation::SetLastError(ERROR_SUCCESS);
                current_format = EnumClipboardFormats(current_format);
                if current_format == 0 {
                    if GetLastError() != ERROR_SUCCESS {
                        success = false;
                    }
                    break;
                }

                if !is_hglobal_clipboard_format(current_format) {
                    continue;
                }

                let handle_result = GetClipboardData(current_format);
                match handle_result {
                    Ok(handle) if !handle.0.is_null() => {
                        let hglobal = HGLOBAL(handle.0);
                        let size = GlobalSize(hglobal);
                        if size > 0
                            && size <= MAX_CLIPBOARD_ITEM_BYTES
                            && total_bytes + size <= MAX_CLIPBOARD_TOTAL_BYTES
                        {
                            let ptr = GlobalLock(hglobal);
                            if !ptr.is_null() {
                                let slice = std::slice::from_raw_parts(ptr as *const u8, size);
                                formats.push(ClipboardFormatData {
                                    format_id: current_format,
                                    format_name: None,
                                    data: slice.to_vec(),
                                });
                                total_bytes += size;
                                let _ = GlobalUnlock(hglobal);
                            } else {
                                success = false;
                                break;
                            }
                        } else {
                            success = false;
                            break;
                        }
                    }
                    _ => {
                        success = false;
                        break;
                    }
                }
            }
            let _ = CloseClipboard();
        }

        if !success {
            std::process::exit(1);
        }

        // Encode snapshot in bounded binary format:
        // [4 bytes: sequence_number]
        // [4 bytes: format_count]
        // For each format:
        //   [4 bytes: format_id]
        //   [4 bytes: data_len]
        //   [data_len bytes: raw_data]
        let mut payload = Vec::with_capacity(8 + total_bytes + formats.len() * 8);
        payload.extend_from_slice(&sequence_number.to_le_bytes());
        payload.extend_from_slice(&(formats.len() as u32).to_le_bytes());

        for item in formats {
            payload.extend_from_slice(&item.format_id.to_le_bytes());
            payload.extend_from_slice(&(item.data.len() as u32).to_le_bytes());
            payload.extend_from_slice(&item.data);
        }

        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(&(payload.len() as u32).to_le_bytes());
        let _ = stdout.write_all(&payload);
        let _ = stdout.flush();
    }
}

/// Takes a clipboard snapshot using the isolated helper process with a 1-second timeout.
pub fn capture_clipboard_snapshot() -> Option<ClipboardSnapshot> {
    let current_exe = std::env::current_exe().ok()?;
    let mut cmd = std::process::Command::new(current_exe);
    cmd.arg("--clipboard-snapshot")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().ok()?;
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
        if stdout.read_exact(&mut buf).is_err() {
            return;
        }

        if buf.len() < 8 {
            return;
        }

        let sequence_number = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let format_count = u32::from_le_bytes(buf[4..8].try_into().unwrap()) as usize;
        let mut offset = 8;
        let mut formats = Vec::with_capacity(format_count);

        for _ in 0..format_count {
            if offset + 8 > buf.len() {
                return;
            }
            let format_id = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
            let data_len =
                u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap()) as usize;
            offset += 8;

            if offset + data_len > buf.len() {
                return;
            }
            let data = buf[offset..offset + data_len].to_vec();
            offset += data_len;

            formats.push(ClipboardFormatData {
                format_id,
                format_name: None,
                data,
            });
        }

        let _ = tx.send(ClipboardSnapshot {
            sequence_number,
            formats,
        });
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
