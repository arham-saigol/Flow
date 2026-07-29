use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, OnceLock,
    },
    thread,
    time::Duration,
};

use tauri::{AppHandle, Manager};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
                OpenClipboard, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        },
        UI::{
            Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
                KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_CONTROL, VK_ESCAPE,
            },
            WindowsAndMessaging::{
                CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW, IsWindow,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowsHookExW, GWL_EXSTYLE, HHOOK,
                KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
                WM_SYSKEYUP, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            },
        },
    },
};

use crate::error::{FlowError, Result};

static APP: OnceLock<AppHandle> = OnceLock::new();
static SELECTED_KEY: AtomicU32 = AtomicU32::new(0xA5); // VK_RMENU
static SELECTED_KEY_DOWN: AtomicBool = AtomicBool::new(false);
static SELECTED_KEY_CHORDED: AtomicBool = AtomicBool::new(false);
static RECORDING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy)]
pub struct TargetWindow {
    hwnd: isize,
    pub cursor_x: i32,
    pub cursor_y: i32,
}

unsafe impl Send for TargetWindow {}
unsafe impl Sync for TargetWindow {}

pub fn capture_target() -> TargetWindow {
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        TargetWindow {
            hwnd: GetForegroundWindow().0 as isize,
            cursor_x: point.x,
            cursor_y: point.y,
        }
    }
}

pub fn configure_keybind(keybind: &str) {
    let key = match keybind {
        "Left Alt" => 0xA4,
        "Right Ctrl" => 0xA3,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        _ => 0xA5,
    };
    SELECTED_KEY.store(key, Ordering::Release);
    SELECTED_KEY_DOWN.store(false, Ordering::Release);
    SELECTED_KEY_CHORDED.store(false, Ordering::Release);
}

pub fn set_recording(recording: bool) {
    RECORDING.store(recording, Ordering::Release);
}

pub fn install_keyboard_hook(app: AppHandle) -> Result<()> {
    APP.set(app)
        .map_err(|_| FlowError::Windows("The keyboard handler was already initialized.".into()))?;
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("flow-keyboard-hook".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let hook = match SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                HINSTANCE(module.0),
                0,
            ) {
                Ok(hook) => hook,
                Err(error) => {
                    let _ = ready_sender.send(Err(format!(
                        "Could not install the keyboard handler: {error}"
                    )));
                    return;
                }
            };
            if ready_sender.send(Ok(())).is_err() {
                return;
            }
            message_loop(hook);
        })
        .map_err(|error| {
            FlowError::Windows(format!("Could not start the keyboard handler: {error}"))
        })?;
    ready_receiver
        .recv()
        .map_err(|_| FlowError::Windows("The keyboard handler did not start.".into()))?
        .map_err(FlowError::Windows)
}

unsafe fn message_loop(_hook: HHOOK) {
    let mut message = MSG::default();
    while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {}
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }
    let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    let vk = event.vkCode;
    let message = wparam.0 as u32;

    if vk == VK_ESCAPE.0 as u32 && RECORDING.load(Ordering::Acquire) {
        if message == WM_KEYDOWN {
            if let Some(app) = APP.get() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    crate::workflow::cancel(&app);
                });
            }
        }
        return LRESULT(1);
    }

    let selected_key = SELECTED_KEY.load(Ordering::Acquire);
    if vk != selected_key
        && (message == WM_KEYDOWN || message == WM_SYSKEYDOWN)
        && SELECTED_KEY_DOWN.load(Ordering::Acquire)
    {
        SELECTED_KEY_CHORDED.store(true, Ordering::Release);
    }

    if vk == selected_key {
        if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
            // Repeated key-down messages never toggle. A complete physical press toggles on key-up.
            if !SELECTED_KEY_DOWN.swap(true, Ordering::AcqRel) {
                SELECTED_KEY_CHORDED.store(false, Ordering::Release);
            }
        } else if (message == WM_KEYUP || message == WM_SYSKEYUP)
            && SELECTED_KEY_DOWN.swap(false, Ordering::AcqRel)
            && !SELECTED_KEY_CHORDED.swap(false, Ordering::AcqRel)
        {
            if let Some(app) = APP.get() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    crate::workflow::toggle(&app).await;
                });
            }
        }
        if matches!(selected_key, 0xA3..=0xA5) {
            return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
        }
        return LRESULT(1);
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

pub fn prepare_overlay(app: &AppHandle, target: TargetWindow) -> Result<()> {
    let overlay = app
        .get_webview_window("overlay")
        .ok_or_else(|| FlowError::Windows("The dictation bar is unavailable.".into()))?;
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: target.cursor_x,
                y: target.cursor_y,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut info);
    }
    let width = 190;
    let height = 88;
    let x = info.rcWork.left + (info.rcWork.right - info.rcWork.left - width) / 2;
    let y = info.rcWork.bottom - height - 28;
    overlay
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|error| {
            FlowError::Windows(format!("Could not position the dictation bar: {error}"))
        })?;
    if let Ok(raw) = overlay.hwnd() {
        unsafe {
            let hwnd = HWND(raw.0 as *mut _);
            let style =
                windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                style | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize,
            );
        }
    }
    overlay
        .show()
        .map_err(|error| FlowError::Windows(format!("Could not show the dictation bar: {error}")))
}

pub fn paste_text(target: TargetWindow, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    unsafe {
        let target_hwnd = HWND(target.hwnd as *mut _);
        if target.hwnd == 0 || !IsWindow(target_hwnd).as_bool() {
            return Err(FlowError::Windows(
                "The application you started dictating in is no longer open.".into(),
            ));
        }
        let _ = SetForegroundWindow(target_hwnd);
        thread::sleep(Duration::from_millis(24));

        match snapshot_text_clipboard() {
            ClipboardSnapshot::Safe(previous) => {
                if write_clipboard_text(text).is_ok() {
                    send_paste();
                    // Give the target message queue time to read before restoring ownership.
                    thread::sleep(Duration::from_millis(110));
                    match previous {
                        Some(value) => write_clipboard_text(&value)?,
                        None => clear_clipboard()?,
                    }
                } else {
                    send_unicode(text);
                }
            }
            ClipboardSnapshot::PreserveUntouched => send_unicode(text),
        }
        let _ =
            windows::Win32::UI::WindowsAndMessaging::SetCursorPos(target.cursor_x, target.cursor_y);
    }
    Ok(())
}

pub fn copy_text(text: &str) -> Result<()> {
    unsafe { write_clipboard_text(text) }
}

fn clipboard_owner() -> Result<HWND> {
    APP.get()
        .and_then(|app| app.get_webview_window("main"))
        .and_then(|window| window.hwnd().ok())
        .map(|handle| HWND(handle.0 as *mut _))
        .ok_or_else(|| FlowError::Windows("The clipboard owner window is unavailable.".into()))
}

enum ClipboardSnapshot {
    Safe(Option<String>),
    PreserveUntouched,
}

unsafe fn snapshot_text_clipboard() -> ClipboardSnapshot {
    if OpenClipboard(HWND::default()).is_err() {
        return ClipboardSnapshot::PreserveUntouched;
    }
    let mut format = 0_u32;
    let mut has_unicode = false;
    let mut has_other = false;
    loop {
        format = EnumClipboardFormats(format);
        if format == 0 {
            break;
        }
        if format == 13 {
            has_unicode = true;
        } else {
            has_other = true;
        }
    }
    if has_other {
        let _ = CloseClipboard();
        return ClipboardSnapshot::PreserveUntouched;
    }
    let value = if has_unicode {
        GetClipboardData(13).ok().and_then(|handle| {
            let pointer = GlobalLock(HGLOBAL(handle.0));
            if pointer.is_null() {
                return None;
            }
            let wide = pointer.cast::<u16>();
            let mut length = 0;
            while *wide.add(length) != 0 {
                length += 1;
            }
            let value = String::from_utf16(std::slice::from_raw_parts(wide, length)).ok();
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            value
        })
    } else {
        None
    };
    let _ = CloseClipboard();
    ClipboardSnapshot::Safe(value)
}

unsafe fn write_clipboard_text(text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    OpenClipboard(clipboard_owner()?)
        .map_err(|error| FlowError::Windows(format!("Could not open the clipboard: {error}")))?;
    if let Err(error) = EmptyClipboard() {
        let _ = CloseClipboard();
        return Err(FlowError::Windows(format!(
            "Could not clear the clipboard: {error}"
        )));
    }
    let allocation =
        GlobalAlloc(GMEM_MOVEABLE, wide.len() * size_of::<u16>()).map_err(|error| {
            FlowError::Windows(format!("Could not allocate clipboard memory: {error}"))
        })?;
    let pointer = GlobalLock(allocation).cast::<u16>();
    if pointer.is_null() {
        let _ = CloseClipboard();
        return Err(FlowError::Windows(
            "Could not access clipboard memory.".into(),
        ));
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
    let _ = GlobalUnlock(allocation);
    if let Err(error) = SetClipboardData(13, windows::Win32::Foundation::HANDLE(allocation.0)) {
        let _ = CloseClipboard();
        return Err(FlowError::Windows(format!(
            "Could not write to the clipboard: {error}"
        )));
    }
    let _ = CloseClipboard();
    Ok(())
}

unsafe fn clear_clipboard() -> Result<()> {
    OpenClipboard(clipboard_owner()?)
        .map_err(|error| FlowError::Windows(format!("Could not restore the clipboard: {error}")))?;
    let result = EmptyClipboard()
        .map_err(|error| FlowError::Windows(format!("Could not restore the clipboard: {error}")));
    let _ = CloseClipboard();
    result
}

unsafe fn send_paste() {
    let inputs = [
        key_input(VK_CONTROL.0, 0, KEYBD_EVENT_FLAGS(0)),
        key_input(b'V' as u16, 0, KEYBD_EVENT_FLAGS(0)),
        key_input(b'V' as u16, 0, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL.0, 0, KEYEVENTF_KEYUP),
    ];
    let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
}

unsafe fn send_unicode(text: &str) {
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    for unit in text.encode_utf16() {
        inputs.push(key_input(0, unit, KEYEVENTF_UNICODE));
        inputs.push(key_input(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    for chunk in inputs.chunks(64) {
        let _ = SendInput(chunk, size_of::<INPUT>() as i32);
    }
}

fn key_input(key: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(key),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
