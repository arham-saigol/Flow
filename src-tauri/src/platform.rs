use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
};

use tauri::{AppHandle, Manager};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{GlobalFree, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
        },
        UI::{
            Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
                KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_ESCAPE, VK_RETURN,
            },
            WindowsAndMessaging::{
                CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW, IsWindow,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowsHookExW, GWL_EXSTYLE, HHOOK,
                KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT,
                WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
                WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, XBUTTON1,
            },
        },
    },
};

use crate::error::{FlowError, Result};

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

pub fn remember_target() {
    let target = capture_target();
    if target.hwnd == 0 || is_flow_window(target.hwnd) {
        return;
    }
    if let Ok(mut remembered) = LAST_TARGET.lock() {
        *remembered = Some(target);
    }
}

pub fn remembered_target() -> Option<TargetWindow> {
    LAST_TARGET.lock().ok().and_then(|target| *target)
}

fn is_flow_window(hwnd: isize) -> bool {
    APP.get().is_some_and(|app| {
        ["main", "overlay"].iter().any(|label| {
            app.get_webview_window(label)
                .and_then(|window| window.hwnd().ok())
                .is_some_and(|handle| handle.0 as isize == hwnd)
        })
    })
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

    if vk == VK_ESCAPE.0 as u32 && RECORDING.load(Ordering::Acquire) {
        if key_down && SELECTED_KEY_DOWN.load(Ordering::Acquire) {
            SELECTED_KEY_CHORDED.store(true, Ordering::Release);
        }
        if key_down {
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
    let x = work_area.position.x + (work_area.size.width as i32 - width) / 2;
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
        if !SetForegroundWindow(target_hwnd).as_bool() {
            return Err(FlowError::Windows(
                "Flow could not return focus to the application where dictation started.".into(),
            ));
        }
        thread::sleep(Duration::from_millis(24));
        if GetForegroundWindow().0 != target_hwnd.0 {
            return Err(FlowError::Windows(
                "The application where dictation started did not regain focus.".into(),
            ));
        }

        send_unicode(text)?;
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

unsafe fn write_clipboard_text(text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    OpenClipboard(clipboard_owner()?)
        .map_err(|error| FlowError::Windows(format!("Could not open the clipboard: {error}")))?;
    let allocation = match GlobalAlloc(GMEM_MOVEABLE, wide.len() * size_of::<u16>()) {
        Ok(allocation) => allocation,
        Err(error) => {
            let _ = CloseClipboard();
            return Err(FlowError::Windows(format!(
                "Could not allocate clipboard memory: {error}"
            )));
        }
    };
    let pointer = GlobalLock(allocation).cast::<u16>();
    if pointer.is_null() {
        let _ = GlobalFree(allocation);
        let _ = CloseClipboard();
        return Err(FlowError::Windows(
            "Could not access clipboard memory.".into(),
        ));
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
    let _ = GlobalUnlock(allocation);
    if let Err(error) = EmptyClipboard() {
        let _ = GlobalFree(allocation);
        let _ = CloseClipboard();
        return Err(FlowError::Windows(format!(
            "Could not clear the clipboard: {error}"
        )));
    }
    if let Err(error) = SetClipboardData(13, windows::Win32::Foundation::HANDLE(allocation.0)) {
        let _ = GlobalFree(allocation);
        let _ = CloseClipboard();
        return Err(FlowError::Windows(format!(
            "Could not write to the clipboard: {error}"
        )));
    }
    let _ = CloseClipboard();
    Ok(())
}

unsafe fn send_unicode(text: &str) -> Result<()> {
    let mut inputs = Vec::with_capacity(text.encode_utf16().count() * 2);
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                characters.next();
            }
            inputs.push(key_input(VK_RETURN.0, 0, KEYBD_EVENT_FLAGS(0)));
            inputs.push(key_input(VK_RETURN.0, 0, KEYEVENTF_KEYUP));
        } else if character == '\n' {
            inputs.push(key_input(VK_RETURN.0, 0, KEYBD_EVENT_FLAGS(0)));
            inputs.push(key_input(VK_RETURN.0, 0, KEYEVENTF_KEYUP));
        } else {
            let mut encoded = [0_u16; 2];
            for unit in character.encode_utf16(&mut encoded) {
                inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE));
                inputs.push(key_input(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
    }
    for chunk in inputs.chunks(64) {
        send_inputs(chunk)?;
    }
    Ok(())
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
            crate::workflow::cancel(&app);
        }
        crate::workflow::report_error(&app, error);
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
