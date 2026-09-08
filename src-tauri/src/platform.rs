use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
        mpsc, Arc, Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Emitter, Manager};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, GlobalFree, SetLastError, ERROR_SUCCESS, HGLOBAL, HINSTANCE,
            HWND, LPARAM, LRESULT, POINT, WPARAM,
        },
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardOwner, GetClipboardSequenceNumber,
                GetOpenClipboardWindow, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            SystemInformation::GetTickCount,
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
                KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU,
                VK_RWIN, VK_SHIFT,
            },
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{
                CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW,
                GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW, SetForegroundWindow,
                SetWindowLongPtrW, SetWindowsHookExW, GWL_EXSTYLE, HHOOK, KBDLLHOOKSTRUCT,
                LLKHF_EXTENDED, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, SMTO_ABORTIFHUNG, SMTO_BLOCK,
                SMTO_ERRORONEXIT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
                WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_RENDERALLFORMATS, WM_RENDERFORMAT,
                WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WS_EX_NOACTIVATE,
                WS_EX_TOOLWINDOW, XBUTTON1,
            },
        },
    },
};

use crate::error::{FlowError, Result};

const CF_UNICODETEXT: u32 = 13;

static APP: OnceLock<AppHandle> = OnceLock::new();
static SELECTED_KEY: AtomicU32 = AtomicU32::new(0xA5); // VK_RMENU
static SELECTED_KEY_DOWN: AtomicBool = AtomicBool::new(false);
static SELECTED_KEY_CHORDED: AtomicBool = AtomicBool::new(false);
static SELECTED_KEY_SCAN: AtomicU32 = AtomicU32::new(0);
static SELECTED_KEY_EXTENDED: AtomicBool = AtomicBool::new(false);
static KEYS_DOWN: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
static MOUSE_BUTTONS_DOWN: AtomicU32 = AtomicU32::new(0);
static LAST_LEFT_CTRL_DOWN: AtomicU32 = AtomicU32::new(0);
static LAST_TARGET: Mutex<Option<TargetWindow>> = Mutex::new(None);
static RECORDING: AtomicBool = AtomicBool::new(false);
static CLIPBOARD_OPERATION: Mutex<()> = Mutex::new(());
static CLIPBOARD_RENDER_STATE: Mutex<Option<ClipboardRenderState>> = Mutex::new(None);
static CLIPBOARD_RENDERED: Condvar = Condvar::new();
static CLIPBOARD_RENDER_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
static CAPTURING_SHORTCUT: AtomicBool = AtomicBool::new(false);
static CAPTURE_START_TICK: AtomicU32 = AtomicU32::new(0);
const SHORTCUT_CAPTURE_TIMEOUT_MS: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetWindow {
    pub hwnd: isize,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub pid: u32,
    pub session_id: u64,
}

unsafe impl Send for TargetWindow {}
unsafe impl Sync for TargetWindow {}

pub fn cache_flow_hwnds(main_hwnd: isize, overlay_hwnd: isize) {
    MAIN_HWND.store(main_hwnd, Ordering::Release);
    OVERLAY_HWND.store(overlay_hwnd, Ordering::Release);
}

pub fn capture_target() -> TargetWindow {
    capture_target_with_session(0)
}

pub fn capture_target_with_session(session_id: u64) -> TargetWindow {
    unsafe {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        if !hwnd.0.is_null() {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
        }
        TargetWindow {
            hwnd: hwnd.0 as isize,
            cursor_x: point.x,
            cursor_y: point.y,
            pid,
            session_id,
        }
    }
}

pub fn remember_target() {
    let target = capture_target();
    if target.hwnd == 0 || is_flow_window(target.hwnd) {
        return;
    }
    if let Ok(mut remembered) = LAST_TARGET.lock() {
        *remembered = Some(target);
    }
}

#[allow(dead_code)]
pub fn remembered_target() -> Option<TargetWindow> {
    LAST_TARGET.lock().ok().and_then(|target| *target)
}

pub fn is_flow_window(hwnd: isize) -> bool {
    let main = MAIN_HWND.load(Ordering::Acquire);
    let overlay = OVERLAY_HWND.load(Ordering::Acquire);
    if (main != 0 && hwnd == main) || (overlay != 0 && hwnd == overlay) {
        return true;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid));
        pid != 0 && pid == std::process::id()
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

pub fn start_shortcut_capture() {
    // Record the capture start tick BEFORE enabling the flag: the hook may
    // observe the flag immediately and needs the deadline to be defined.
    let start_tick: u32 = unsafe { GetTickCount() };
    CAPTURE_START_TICK.store(start_tick, Ordering::Release);
    CAPTURING_SHORTCUT.store(true, Ordering::Release);
}

fn shortcut_capture_expired() -> bool {
    let start = CAPTURE_START_TICK.load(Ordering::Acquire);
    let now: u32 = unsafe { GetTickCount() };
    now.wrapping_sub(start) >= SHORTCUT_CAPTURE_TIMEOUT_MS
}

fn is_shortcut_capture_key(vk: u32) -> bool {
    matches!(vk, 0xA5 | 0xA4 | 0xA3 | 0x77 | 0x78 | 0x79 | 0x7A | 0x7B | 0x1B)
}

pub fn cancel_shortcut_capture() {
    CAPTURING_SHORTCUT.store(false, Ordering::Release);
}

pub fn install_keyboard_hook(app: AppHandle) -> Result<()> {
    APP.set(app)
        .map_err(|_| FlowError::Windows("The keyboard handler was already initialized.".into()))?;
    unsafe {
        ensure_clipboard_render_handler(clipboard_owner()?)?;
    }
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("flow-keyboard-hook".into())
        .spawn(move || unsafe {
            let module = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let keyboard_hook_handle = match SetWindowsHookExW(
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
            let mouse_hook_handle =
                match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), HINSTANCE(module.0), 0) {
                    Ok(hook) => hook,
                    Err(error) => {
                        let _ = ready_sender.send(Err(format!(
                            "Could not install the mouse gesture handler: {error}"
                        )));
                        return;
                    }
                };
            if ready_sender.send(Ok(())).is_err() {
                return;
            }
            message_loop(keyboard_hook_handle, mouse_hook_handle);
        })
        .map_err(|error| {
            FlowError::Windows(format!("Could not start the keyboard handler: {error}"))
        })?;
    ready_receiver
        .recv()
        .map_err(|_| FlowError::Windows("The keyboard handler did not start.".into()))?
        .map_err(FlowError::Windows)
}

unsafe fn message_loop(_keyboard_hook: HHOOK, _mouse_hook: HHOOK) {
    let mut message = MSG::default();
    while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {}
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let message = wparam.0 as u32;
        if let Some((button, is_down)) = mouse_button_state(message, lparam) {
            if is_down {
                MOUSE_BUTTONS_DOWN.fetch_or(button, Ordering::AcqRel);
            } else {
                MOUSE_BUTTONS_DOWN.fetch_and(!button, Ordering::AcqRel);
            }
        }
        if matches!(
            message,
            WM_LBUTTONDOWN
                | WM_RBUTTONDOWN
                | WM_MBUTTONDOWN
                | WM_XBUTTONDOWN
                | WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
        ) && SELECTED_KEY_DOWN.load(Ordering::Acquire)
            && !SELECTED_KEY_CHORDED.swap(true, Ordering::AcqRel)
        {
            let selected_key = SELECTED_KEY.load(Ordering::Acquire);
            if let Err(error) = replay_key_down(
                selected_key as u16,
                SELECTED_KEY_SCAN.load(Ordering::Acquire) as u16,
                SELECTED_KEY_EXTENDED.load(Ordering::Acquire),
            ) {
                if let Some(app) = APP.get() {
                    report_input_error(app.clone(), error);
                }
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

unsafe fn mouse_button_state(message: u32, lparam: LPARAM) -> Option<(u32, bool)> {
    match message {
        WM_LBUTTONDOWN => Some((1, true)),
        WM_LBUTTONUP => Some((1, false)),
        WM_RBUTTONDOWN => Some((2, true)),
        WM_RBUTTONUP => Some((2, false)),
        WM_MBUTTONDOWN => Some((4, true)),
        WM_MBUTTONUP => Some((4, false)),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let button = if (event.mouseData >> 16) as u16 == XBUTTON1 {
                8
            } else {
                16
            };
            Some((button, message == WM_XBUTTONDOWN))
        }
        _ => None,
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }
    let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
    if event.flags.contains(LLKHF_INJECTED) {
        return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
    }
    let vk = event.vkCode;
    let message = wparam.0 as u32;
    let key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let key_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    if vk == 0xA2 && key_down {
        LAST_LEFT_CTRL_DOWN.store(event.time, Ordering::Release);
    }
    if let Some(state) = KEYS_DOWN.get(vk as usize) {
        if key_down {
            state.store(true, Ordering::Release);
        } else if key_up {
            state.store(false, Ordering::Release);
        }
    }

    if CAPTURING_SHORTCUT.load(Ordering::Acquire) {
        if shortcut_capture_expired() {
            // The capture deadline passed: leave capture mode and process this
            // event through the normal path so the keyboard is never left
            // captured without a user-visible way out.
            CAPTURING_SHORTCUT.store(false, Ordering::Release);
            if let Some(app) = APP.get() {
                let _ = app.emit("shortcut-capture-cancelled", ());
            }
        } else {
            if key_down {
                if vk == VK_ESCAPE.0 as u32 {
                    CAPTURING_SHORTCUT.store(false, Ordering::Release);
                    if let Some(app) = APP.get() {
                        let _ = app.emit("shortcut-capture-cancelled", ());
                    }
                    return LRESULT(1);
                }
                let keybind = match vk {
                    0xA5 => Some("Right Alt"),
                    0xA4 => Some("Left Alt"),
                    0xA3 => Some("Right Ctrl"),
                    0x77 => Some("F8"),
                    0x78 => Some("F9"),
                    0x79 => Some("F10"),
                    0x7A => Some("F11"),
                    0x7B => Some("F12"),
                    _ => None,
                };
                if let Some(name) = keybind {
                    CAPTURING_SHORTCUT.store(false, Ordering::Release);
                    if let Some(app) = APP.get() {
                        let _ = app.emit("shortcut-captured", serde_json::json!({ "keybind": name }));
                    }
                    return LRESULT(1);
                }
            }
            // Continue consuming recognized capture keys (Escape and shortcut
            // candidates) in both directions; unrecognized key-up events pass
            // through so ordinary typing is not swallowed while capturing.
            if key_up && !is_shortcut_capture_key(vk) {
                return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
            }
            return LRESULT(1);
        }
    }

    if vk == VK_ESCAPE.0 as u32 && RECORDING.load(Ordering::Acquire) {
        if key_down && SELECTED_KEY_DOWN.load(Ordering::Acquire) {
            SELECTED_KEY_CHORDED.store(true, Ordering::Release);
        }
        if key_down {
            if let Some(app) = APP.get() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::workflow::cancel(&app);
                });
            }
        }
        return LRESULT(1);
    }

    let selected_key = SELECTED_KEY.load(Ordering::Acquire);
    if vk == selected_key && key_down {
        remember_target();
    }
    let mut replayed_chord = false;
    if vk != selected_key && key_down && SELECTED_KEY_DOWN.load(Ordering::Acquire) {
        let was_chorded = SELECTED_KEY_CHORDED.swap(true, Ordering::AcqRel);
        if !was_chorded {
            match replay_chord(
                selected_key as u16,
                SELECTED_KEY_SCAN.load(Ordering::Acquire) as u16,
                SELECTED_KEY_EXTENDED.load(Ordering::Acquire),
                vk as u16,
                event.scanCode as u16,
                event.flags.contains(LLKHF_EXTENDED),
            ) {
                Ok(()) => replayed_chord = true,
                Err(error) => {
                    if let Some(app) = APP.get() {
                        report_input_error(app.clone(), error);
                    }
                }
            }
        }
    }
    if replayed_chord {
        return LRESULT(1);
    }

    if vk == selected_key {
        let mut chorded = SELECTED_KEY_CHORDED.load(Ordering::Acquire);
        if key_down {
            // Repeated key-down messages never toggle. A complete physical press toggles on key-up.
            if !SELECTED_KEY_DOWN.swap(true, Ordering::AcqRel) {
                SELECTED_KEY_SCAN.store(event.scanCode, Ordering::Release);
                SELECTED_KEY_EXTENDED
                    .store(event.flags.contains(LLKHF_EXTENDED), Ordering::Release);
                let synthetic_altgr_ctrl = selected_key == 0xA5
                    && event
                        .time
                        .wrapping_sub(LAST_LEFT_CTRL_DOWN.load(Ordering::Acquire))
                        <= 10;
                chorded = KEYS_DOWN.iter().enumerate().any(|(key, state)| {
                    key != selected_key as usize
                        && !(synthetic_altgr_ctrl && key == 0xA2)
                        && state.load(Ordering::Acquire)
                }) || MOUSE_BUTTONS_DOWN.load(Ordering::Acquire) != 0;
                SELECTED_KEY_CHORDED.store(chorded, Ordering::Release);
            }
        } else if key_up {
            let was_down = SELECTED_KEY_DOWN.swap(false, Ordering::AcqRel);
            chorded = SELECTED_KEY_CHORDED.swap(false, Ordering::AcqRel);
            if was_down && !chorded {
                if let Some(app) = APP.get() {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        crate::workflow::toggle(&app).await;
                    });
                }
            }
        }
        if chorded {
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
    let monitor = overlay
        .monitor_from_point(target.cursor_x.into(), target.cursor_y.into())
        .map_err(|error| {
            FlowError::Windows(format!("Could not identify the target monitor: {error}"))
        })?
        .ok_or_else(|| FlowError::Windows("The target monitor is unavailable.".into()))?;
    let work_area = monitor.work_area();
    let scale = monitor.scale_factor();
    let width = (104.0 * scale).round() as i32;
    let height = (50.0 * scale).round() as i32;
    let bottom_margin = (8.0 * scale).round() as i32;
    // Bias an unavoidable half-pixel to the right instead of leaving the
    // overlay looking one physical pixel left of center.
    let x = work_area.position.x + (work_area.size.width as i32 - width + 1) / 2;
    let y = work_area.position.y + work_area.size.height as i32 - height - bottom_margin;
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

pub fn paste_text(target: TargetWindow, text: &str) -> Result<&'static str> {
    if text.is_empty() {
        return Ok("copied");
    }
    let target_hwnd = HWND(target.hwnd as *mut _);
    unsafe {
        if target.hwnd == 0 || !IsWindow(target_hwnd).as_bool() {
            return Err(FlowError::Windows(
                "The target window is no longer valid.".into(),
            ));
        }
        let mut current_pid = 0u32;
        GetWindowThreadProcessId(target_hwnd, Some(&mut current_pid));
        if target.pid != 0 && current_pid != target.pid {
            return Err(FlowError::Windows("The target process has changed.".into()));
        }
        if GetForegroundWindow().0 != target_hwnd.0 {
            let _ = SetForegroundWindow(target_hwnd);
            thread::sleep(Duration::from_millis(50));
            if GetForegroundWindow().0 != target_hwnd.0 {
                return Err(FlowError::Windows(
                    "The target window did not acquire focus.".into(),
                ));
            }
        }
    }
    paste_via_clipboard(target_hwnd, text)?;
    Ok("pasted")
}

pub fn copy_text(text: &str) -> Result<()> {
    let _operation = CLIPBOARD_OPERATION
        .lock()
        .map_err(|_| FlowError::Windows("The clipboard handler is unavailable.".into()))?;
    unsafe { write_clipboard_text(text, true) }
}

fn clipboard_owner() -> Result<HWND> {
    APP.get()
        .and_then(|app| app.get_webview_window("main"))
        .and_then(|window| window.hwnd().ok())
        .map(|handle| HWND(handle.0 as *mut _))
        .ok_or_else(|| FlowError::Windows("The clipboard owner window is unavailable.".into()))
}

unsafe fn write_clipboard_text(text: &str, include_in_history: bool) -> Result<()> {
    let wide = encode_clipboard_text(text);
    let allocation = allocate_clipboard_wide(&wide)?;
    if let Err(error) =
        open_clipboard_with_retry(clipboard_owner()?, "Could not open the clipboard")
    {
        let _ = GlobalFree(allocation);
        return Err(error);
    }
    if let Err(error) = EmptyClipboard() {
        let _ = GlobalFree(allocation);
        let _ = CloseClipboard();
        return Err(FlowError::Windows(format!(
            "Could not clear the clipboard: {error}"
        )));
    }
    if let Err(error) = set_clipboard_wide(allocation) {
        let _ = CloseClipboard();
        return Err(error);
    }
    if !include_in_history {
        exclude_from_clipboard_services();
    }
    let _ = CloseClipboard();
    Ok(())
}

fn encode_clipboard_text(text: &str) -> Vec<u16> {
    let mut normalized = String::with_capacity(text.len() + 1);
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                normalized.push_str("\r\n");
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
            }
            '\n' => normalized.push_str("\r\n"),
            _ => normalized.push(character),
        }
    }
    normalized
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn write_clipboard_wide(wide: &[u16]) -> Result<()> {
    let allocation = allocate_clipboard_wide(wide)?;
    set_clipboard_wide(allocation)
}

unsafe fn allocate_clipboard_wide(wide: &[u16]) -> Result<HGLOBAL> {
    let allocation = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(wide)).map_err(|error| {
        FlowError::Windows(format!("Could not allocate clipboard memory: {error}"))
    })?;
    let pointer = GlobalLock(allocation).cast::<u16>();
    if pointer.is_null() {
        let _ = GlobalFree(allocation);
        return Err(FlowError::Windows(
            "Could not access clipboard memory.".into(),
        ));
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
    let _ = GlobalUnlock(allocation);
    Ok(allocation)
}

unsafe fn set_clipboard_wide(allocation: HGLOBAL) -> Result<()> {
    if let Err(error) = SetClipboardData(
        CF_UNICODETEXT,
        windows::Win32::Foundation::HANDLE(allocation.0),
    ) {
        let _ = GlobalFree(allocation);
        return Err(FlowError::Windows(format!(
            "Could not write to the clipboard: {error}"
        )));
    }
    Ok(())
}

unsafe fn write_clipboard_dword(format: u32, value: u32) -> Result<()> {
    let allocation = GlobalAlloc(GMEM_MOVEABLE, size_of::<u32>()).map_err(|error| {
        FlowError::Windows(format!("Could not allocate clipboard metadata: {error}"))
    })?;
    let pointer = GlobalLock(allocation).cast::<u32>();
    if pointer.is_null() {
        let _ = GlobalFree(allocation);
        return Err(FlowError::Windows(
            "Could not access clipboard metadata.".into(),
        ));
    }
    pointer.write(value);
    let _ = GlobalUnlock(allocation);
    if let Err(error) = SetClipboardData(format, windows::Win32::Foundation::HANDLE(allocation.0)) {
        let _ = GlobalFree(allocation);
        return Err(FlowError::Windows(format!(
            "Could not write clipboard metadata: {error}"
        )));
    }
    Ok(())
}

fn paste_via_clipboard(target: HWND, text: &str) -> Result<()> {
    let _operation = CLIPBOARD_OPERATION
        .lock()
        .map_err(|_| FlowError::Windows("The clipboard handler is unavailable.".into()))?;
    unsafe {
        if !wait_for_shortcut_modifiers(Duration::from_secs(1)) {
            return Err(FlowError::Windows(
                "Release Ctrl, Shift, Alt, Windows, or V before Flow pastes the dictation.".into(),
            ));
        }

        let Some(original) = prepare_temporary_clipboard(target, text)? else {
            return Err(FlowError::Windows(
                "Flow could not safely preserve the clipboard.".into(),
            ));
        };
        let paste_result = (|| {
            if GetForegroundWindow().0 != target.0 {
                return Err(FlowError::Windows(
                    "The dictation target lost focus before Flow could paste.".into(),
                ));
            }
            if shortcut_modifiers_down() {
                return Err(FlowError::Windows(
                    "A modifier key was pressed before Flow could paste the dictation.".into(),
                ));
            }
            send_armed_paste_shortcut()?;
            wait_for_temporary_clipboard_request(Duration::from_secs(5))?;
            thread::sleep(Duration::from_millis(50));
            Ok(())
        })();

        let restore_result = restore_clipboard(&original);
        if let Ok(mut state) = CLIPBOARD_RENDER_STATE.lock() {
            *state = None;
        }

        match (paste_result, restore_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Err(paste_error), Err(restore_error)) => Err(FlowError::Windows(format!(
                "{paste_error} The previous clipboard also could not be restored: {restore_error}"
            ))),
        }
    }
}

struct ClipboardRenderState {
    wide: Vec<u16>,
    original: Arc<Vec<ClipboardItem>>,
    target_hwnd: isize,
    target_process_ids: Vec<u32>,
    shortcut_sent: bool,
    rendered: Option<std::result::Result<(), String>>,
}

struct ClipboardItem {
    format: u32,
    data: Vec<u8>,
}

unsafe fn prepare_temporary_clipboard(
    target: HWND,
    text: &str,
) -> Result<Option<Arc<Vec<ClipboardItem>>>> {
    let owner = clipboard_owner()?;
    ensure_clipboard_render_handler(owner)?;
    let mut target_process_id = 0;
    if GetWindowThreadProcessId(target, Some(&mut target_process_id)) == 0 || target_process_id == 0
    {
        return Err(FlowError::Windows(
            "Could not identify the dictation target process.".into(),
        ));
    }
    let target_process_ids = target_process_family(target_process_id);
    let Some(rendered_owner) = render_delayed_clipboard_with_timeout(owner) else {
        return Ok(None);
    };

    let snapshot = match crate::clipboard_snapshot::capture_clipboard_snapshot() {
        Some(s) => s,
        None => return Ok(None),
    };

    open_clipboard_with_retry(owner, "Could not preserve the clipboard")?;
    let captured_owner = GetClipboardOwner().map_or(0, |owner| owner.0 as isize);
    if captured_owner != rendered_owner {
        // Ownership changed after the bounded render request.
        let _ = CloseClipboard();
        return Ok(None);
    }

    let current_seq = GetClipboardSequenceNumber();
    if current_seq != snapshot.sequence_number {
        // Clipboard sequence changed between snapshot and lock.
        let _ = CloseClipboard();
        return Ok(None);
    }

    let original = Arc::new(
        snapshot
            .formats
            .into_iter()
            .map(|item| ClipboardItem {
                format: item.format_id,
                data: item.data,
            })
            .collect(),
    );
    let wide = encode_clipboard_text(text);
    match CLIPBOARD_RENDER_STATE.lock() {
        Ok(mut state) => {
            *state = Some(ClipboardRenderState {
                wide,
                original: Arc::clone(&original),
                target_hwnd: target.0 as isize,
                target_process_ids,
                shortcut_sent: false,
                rendered: None,
            });
        }
        Err(_) => {
            let _ = CloseClipboard();
            return Err(FlowError::Windows(
                "The clipboard renderer is unavailable.".into(),
            ));
        }
    }
    let replace_result = (|| {
        EmptyClipboard().map_err(|error| {
            FlowError::Windows(format!("Could not clear the clipboard: {error}"))
        })?;
        set_delayed_clipboard_text()?;
        exclude_from_clipboard_services();
        Ok(())
    })();
    let _ = CloseClipboard();
    if let Err(replace_error) = replace_result {
        if let Ok(mut state) = CLIPBOARD_RENDER_STATE.lock() {
            *state = None;
        }
        return match restore_clipboard(&original) {
            Ok(()) => Err(replace_error),
            Err(restore_error) => Err(FlowError::Windows(format!(
                "{replace_error} The previous clipboard also could not be restored: {restore_error}"
            ))),
        };
    }
    Ok(Some(original))
}

unsafe fn render_delayed_clipboard_with_timeout(flow_owner: HWND) -> Option<isize> {
    const RENDER_TIMEOUT_MS: u32 = 500;

    let Ok(source_owner) = GetClipboardOwner() else {
        return Some(0);
    };
    if source_owner.0 == flow_owner.0 {
        return Some(source_owner.0 as isize);
    }

    // GetClipboardData may otherwise synchronously wait forever for a hung
    // owner to render delayed formats. Ask it to materialize those formats
    // through a bounded message before Flow opens and reads the clipboard.
    let rendered = SendMessageTimeoutW(
        source_owner,
        WM_RENDERALLFORMATS,
        WPARAM(0),
        LPARAM(0),
        SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
        RENDER_TIMEOUT_MS,
        None,
    )
    .0 != 0;
    rendered.then_some(source_owner.0 as isize)
}

unsafe fn target_process_family(target_process_id: u32) -> Vec<u32> {
    let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
        return vec![target_process_id];
    };
    let mut processes = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if Process32FirstW(snapshot, &mut entry).is_ok() {
        loop {
            processes.push((entry.th32ProcessID, entry.th32ParentProcessID));
            if Process32NextW(snapshot, &mut entry).is_err() {
                break;
            }
        }
    }
    let _ = CloseHandle(snapshot);
    descendant_process_ids(target_process_id, &processes)
}

fn descendant_process_ids(target_process_id: u32, processes: &[(u32, u32)]) -> Vec<u32> {
    let mut family = vec![target_process_id];
    loop {
        let previous_len = family.len();
        for &(process_id, parent_process_id) in processes {
            if family.contains(&parent_process_id) && !family.contains(&process_id) {
                family.push(process_id);
            }
        }
        if family.len() == previous_len {
            return family;
        }
    }
}

unsafe fn set_delayed_clipboard_text() -> Result<()> {
    SetLastError(ERROR_SUCCESS);
    let result = SetClipboardData(
        CF_UNICODETEXT,
        windows::Win32::Foundation::HANDLE::default(),
    );
    let error = GetLastError();
    if result.is_err() && error != ERROR_SUCCESS {
        return Err(FlowError::Windows(format!(
            "Could not prepare the clipboard (Windows error {}).",
            error.0
        )));
    }
    Ok(())
}

unsafe fn exclude_from_clipboard_services() {
    // These markers are best-effort so a platform that rejects one still pastes.
    for name in [
        w!("CanIncludeInClipboardHistory"),
        w!("CanUploadToCloudClipboard"),
        w!("ExcludeClipboardContentFromMonitorProcessing"),
    ] {
        let format = RegisterClipboardFormatW(name);
        if format != 0 {
            let _ = write_clipboard_dword(format, 0);
        }
    }
}

unsafe fn restore_clipboard(items: &[ClipboardItem]) -> Result<()> {
    restore_clipboard_for_owner(clipboard_owner()?, items)
}

unsafe fn restore_clipboard_for_owner(flow_owner: HWND, items: &[ClipboardItem]) -> Result<()> {
    open_clipboard_with_retry(
        flow_owner,
        "Could not restore the clipboard; temporary dictation text may remain",
    )?;
    let result = (|| {
        if GetClipboardOwner().map_or(true, |owner| owner.0 != flow_owner.0) {
            // Another application or the user replaced the temporary clipboard
            // while Flow was waiting. Preserve that newer owner's value.
            return Ok(());
        }
        let mut allocations = Vec::with_capacity(items.len());
        for item in items {
            let allocation = match GlobalAlloc(GMEM_MOVEABLE, item.data.len()) {
                Ok(allocation) => allocation,
                Err(error) => {
                    for (_, allocation) in allocations {
                        let _ = GlobalFree(allocation);
                    }
                    return Err(FlowError::Windows(format!(
                        "Could not allocate the restored clipboard: {error}"
                    )));
                }
            };
            let pointer = GlobalLock(allocation).cast::<u8>();
            if pointer.is_null() {
                let _ = GlobalFree(allocation);
                for (_, allocation) in allocations {
                    let _ = GlobalFree(allocation);
                }
                return Err(FlowError::Windows(
                    "Could not access the restored clipboard.".into(),
                ));
            }
            std::ptr::copy_nonoverlapping(item.data.as_ptr(), pointer, item.data.len());
            let _ = GlobalUnlock(allocation);
            allocations.push((item.format, allocation));
        }
        EmptyClipboard().map_err(|error| {
            for (_, allocation) in allocations.iter().copied() {
                let _ = GlobalFree(allocation);
            }
            FlowError::Windows(format!("Could not clear the temporary clipboard: {error}"))
        })?;
        for (index, (format, allocation)) in allocations.iter().copied().enumerate() {
            if let Err(error) =
                SetClipboardData(format, windows::Win32::Foundation::HANDLE(allocation.0))
            {
                let _ = GlobalFree(allocation);
                for (_, allocation) in allocations.iter().skip(index + 1).copied() {
                    let _ = GlobalFree(allocation);
                }
                return Err(FlowError::Windows(format!(
                    "Could not restore the clipboard: {error}"
                )));
            }
        }
        Ok(())
    })();
    let _ = CloseClipboard();
    result
}

unsafe fn open_clipboard_with_retry(owner: HWND, context: &str) -> Result<()> {
    const ATTEMPTS: u32 = 10;

    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        match OpenClipboard(owner) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < ATTEMPTS {
            thread::sleep(Duration::from_millis(5 * u64::from(attempt + 1)));
        }
    }
    Err(FlowError::Windows(format!(
        "{context} after {ATTEMPTS} attempts: {}",
        last_error.expect("at least one clipboard attempt failed")
    )))
}

unsafe fn ensure_clipboard_render_handler(owner: HWND) -> Result<()> {
    if CLIPBOARD_RENDER_HANDLER_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    if !SetWindowSubclass(owner, Some(clipboard_window_proc), 0x464C4F57, 0).as_bool() {
        return Err(FlowError::Windows(
            "Could not initialize the clipboard renderer.".into(),
        ));
    }
    CLIPBOARD_RENDER_HANDLER_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

unsafe extern "system" fn clipboard_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    if message == WM_RENDERALLFORMATS {
        let original = CLIPBOARD_RENDER_STATE
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|state| Arc::clone(&state.original)));
        if let Some(original) = original {
            // The main window is being destroyed while it still owns delayed
            // text. Restore the captured clipboard before this process exits.
            let _ = restore_clipboard_for_owner(hwnd, &original);
            return LRESULT(0);
        }
    }
    if message == WM_RENDERFORMAT && wparam.0 as u32 == CF_UNICODETEXT {
        if let Ok(mut guard) = CLIPBOARD_RENDER_STATE.lock() {
            if let Some(state) = guard.as_mut() {
                let requested_by_target = match GetOpenClipboardWindow() {
                    Ok(requester) => {
                        let mut requester_process_id = 0;
                        state.shortcut_sent
                            && GetWindowThreadProcessId(requester, Some(&mut requester_process_id))
                                != 0
                            && state.target_process_ids.contains(&requester_process_id)
                    }
                    Err(_) => {
                        state.shortcut_sent && GetForegroundWindow().0 as isize == state.target_hwnd
                    }
                };
                if !requested_by_target {
                    // Keep the format delayed when a clipboard monitor asks first;
                    // only the dictation target may materialize the temporary text.
                    return LRESULT(0);
                }
                state.rendered = Some(
                    write_clipboard_wide(&state.wide)
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                );
                CLIPBOARD_RENDERED.notify_all();
            }
        }
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, message, wparam, lparam)
}

unsafe fn send_armed_paste_shortcut() -> Result<()> {
    let mut guard = CLIPBOARD_RENDER_STATE
        .lock()
        .map_err(|_| FlowError::Windows("The clipboard renderer is unavailable.".into()))?;
    let state = guard
        .as_mut()
        .ok_or_else(|| FlowError::Windows("The clipboard renderer is unavailable.".into()))?;
    let result = send_paste_shortcut();
    state.shortcut_sent = result.is_ok();
    result
}

fn wait_for_temporary_clipboard_request(timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let expected_owner = clipboard_owner()?;
    let mut guard = CLIPBOARD_RENDER_STATE
        .lock()
        .map_err(|_| FlowError::Windows("The clipboard renderer is unavailable.".into()))?;
    loop {
        if let Some(result) = guard.as_ref().and_then(|state| state.rendered.as_ref()) {
            return result.clone().map_err(|error| {
                FlowError::Windows(format!("Could not render the paste: {error}"))
            });
        }
        if unsafe { GetClipboardOwner() }.map_or(true, |owner| owner.0 != expected_owner.0) {
            return Err(FlowError::Windows(
                "The clipboard changed before the destination requested the dictation.".into(),
            ));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(FlowError::Windows(
                "The destination did not request the dictation from the clipboard.".into(),
            ));
        }
        let wait = (deadline - now).min(Duration::from_millis(50));
        let (next_guard, _) = CLIPBOARD_RENDERED
            .wait_timeout(guard, wait)
            .map_err(|_| FlowError::Windows("The clipboard renderer is unavailable.".into()))?;
        guard = next_guard;
    }
}

fn wait_for_shortcut_modifiers(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while unsafe { shortcut_modifiers_down() } {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

unsafe fn shortcut_modifiers_down() -> bool {
    [
        VK_CONTROL.0,
        VK_SHIFT.0,
        VK_MENU.0,
        VK_LWIN.0,
        VK_RWIN.0,
        0x56,
    ]
    .into_iter()
    .any(|key| key_is_down(key))
}

unsafe fn key_is_down(key: u16) -> bool {
    GetAsyncKeyState(key as i32) as u16 & 0x8000 != 0
}

unsafe fn send_paste_shortcut() -> Result<()> {
    let inputs = [
        key_input(VK_CONTROL.0, 0, KEYBD_EVENT_FLAGS(0)),
        key_input(0x56, 0, KEYBD_EVENT_FLAGS(0)), // V
        key_input(0x56, 0, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL.0, 0, KEYEVENTF_KEYUP),
    ];
    let inserted = SendInput(&inputs, size_of::<INPUT>() as i32) as usize;
    if inserted == inputs.len() {
        return Ok(());
    }

    let control_up = [key_input(VK_CONTROL.0, 0, KEYEVENTF_KEYUP)];
    let v_and_control_up = [
        key_input(0x56, 0, KEYEVENTF_KEYUP),
        key_input(VK_CONTROL.0, 0, KEYEVENTF_KEYUP),
    ];
    let cleanup: &[INPUT] = match inserted {
        1 | 3 => &control_up,
        2 => &v_and_control_up,
        _ => &[],
    };
    let mut released = 0;
    while released < cleanup.len() {
        let count = SendInput(&cleanup[released..], size_of::<INPUT>() as i32) as usize;
        if count == 0 {
            break;
        }
        released += count;
    }

    let cleanup_status = if released == cleanup.len() {
        "Injected key-downs were released.".to_string()
    } else {
        format!(
            "Windows accepted {released} of {} cleanup events.",
            cleanup.len()
        )
    };
    Err(FlowError::Windows(format!(
        "Windows accepted {inserted} of {} paste shortcut events. {cleanup_status}",
        inputs.len()
    )))
}

unsafe fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    let inserted = SendInput(inputs, size_of::<INPUT>() as i32) as usize;
    if inserted != inputs.len() {
        return Err(FlowError::Windows(format!(
            "Windows accepted {inserted} of {} keyboard input events.",
            inputs.len()
        )));
    }
    Ok(())
}

unsafe fn replay_chord(
    selected_key: u16,
    selected_scan: u16,
    selected_extended: bool,
    chord_key: u16,
    chord_scan: u16,
    chord_extended: bool,
) -> Result<()> {
    send_inputs(&[
        key_input(
            selected_key,
            selected_scan,
            extended_key_flag(selected_extended),
        ),
        key_input(chord_key, chord_scan, extended_key_flag(chord_extended)),
    ])
}

unsafe fn replay_key_down(key: u16, scan: u16, extended: bool) -> Result<()> {
    send_inputs(&[key_input(key, scan, extended_key_flag(extended))])
}

fn extended_key_flag(extended: bool) -> KEYBD_EVENT_FLAGS {
    if extended {
        windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY
    } else {
        KEYBD_EVENT_FLAGS(0)
    }
}

fn report_input_error(app: AppHandle, error: FlowError) {
    tauri::async_runtime::spawn(async move {
        if RECORDING.load(Ordering::Acquire) {
            let _ = crate::workflow::cancel(&app);
        }
        // Cancel already owns the session reset; report without a session id so
        // a stale input failure cannot overwrite a newer recording session.
        crate::workflow::report_error(&app, error, None);
    });
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

#[cfg(test)]
mod tests {
    use super::{descendant_process_ids, encode_clipboard_text};

    #[test]
    fn clipboard_text_uses_cr_lf_line_endings() {
        let wide = encode_clipboard_text("one\ntwo\rthree\r\nfour");

        assert_eq!(wide.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&wide[..wide.len() - 1]).unwrap(),
            "one\r\ntwo\r\nthree\r\nfour"
        );
    }

    #[test]
    fn clipboard_requests_allow_only_the_target_process_family() {
        let processes = [(10, 1), (11, 10), (12, 11), (20, 1), (21, 20)];

        assert_eq!(descendant_process_ids(10, &processes), vec![10, 11, 12]);
    }
}
