use std::{
    mem::size_of,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Manager};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, GlobalFree, SetLastError, ERROR_SUCCESS, HGLOBAL, HINSTANCE, HWND,
            LPARAM, LRESULT, POINT, WPARAM,
        },
        System::{
            DataExchange::{
                CloseClipboard, CountClipboardFormats, EmptyClipboard, EnumClipboardFormats,
                GetClipboardData, GetClipboardSequenceNumber, GetOpenClipboardWindow,
                OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
                KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VK_CONTROL, VK_ESCAPE,
                VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
            },
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{
                CallNextHookEx, GetCursorPos, GetForegroundWindow, GetMessageW,
                GetWindowThreadProcessId, IsWindow, SetForegroundWindow, SetWindowLongPtrW,
                SetWindowsHookExW, GWL_EXSTYLE, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
                LLKHF_INJECTED, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN,
                WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
                WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_RENDERFORMAT,
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

pub fn paste_text(target: TargetWindow, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let target_hwnd = HWND(target.hwnd as *mut _);
    unsafe {
        if target.hwnd == 0 || !IsWindow(target_hwnd).as_bool() {
            return Err(FlowError::Windows(
                "The application selected when dictation ended is no longer open.".into(),
            ));
        }
        if !SetForegroundWindow(target_hwnd).as_bool() {
            return Err(FlowError::Windows(
                "Flow could not focus the application selected when dictation ended.".into(),
            ));
        }
        thread::sleep(Duration::from_millis(24));
        if GetForegroundWindow().0 != target_hwnd.0 {
            return Err(FlowError::Windows(
                "The application selected when dictation ended did not gain focus.".into(),
            ));
        }
    }
    paste_via_clipboard(target_hwnd, text)
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
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
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

        let Some((original, mut temporary_sequence)) = prepare_temporary_clipboard(target, text)?
        else {
            if GetForegroundWindow().0 != target.0 {
                return Err(FlowError::Windows(
                    "The dictation target lost focus before Flow could type.".into(),
                ));
            }
            return send_unicode(text);
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
            temporary_sequence =
                wait_for_temporary_clipboard_request(temporary_sequence, Duration::from_secs(5))?;
            // WM_RENDERFORMAT confirms that the target requested the text. Give
            // GetClipboardData a short grace period to copy the rendered handle.
            thread::sleep(Duration::from_millis(50));
            Ok(())
        })();

        let restore_result = restore_clipboard(&original, Some(temporary_sequence));
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
    target_hwnd: isize,
    target_process_id: u32,
    paste_armed: bool,
    rendered: Option<std::result::Result<(u32, bool), String>>,
}

struct ClipboardItem {
    format: u32,
    data: Vec<u8>,
}

unsafe fn prepare_temporary_clipboard(
    target: HWND,
    text: &str,
) -> Result<Option<(Vec<ClipboardItem>, u32)>> {
    let owner = clipboard_owner()?;
    ensure_clipboard_render_handler(owner)?;
    let mut target_process_id = 0;
    if GetWindowThreadProcessId(target, Some(&mut target_process_id)) == 0 || target_process_id == 0
    {
        return Err(FlowError::Windows(
            "Could not identify the dictation target process.".into(),
        ));
    }
    open_clipboard_with_retry(owner, "Could not preserve the clipboard")?;
    let original = match capture_open_clipboard() {
        Ok(Some(original)) => original,
        Ok(None) => {
            let _ = CloseClipboard();
            return Ok(None);
        }
        Err(error) => {
            let _ = CloseClipboard();
            return Err(error);
        }
    };
    let wide = text.encode_utf16().chain(std::iter::once(0)).collect();
    match CLIPBOARD_RENDER_STATE.lock() {
        Ok(mut state) => {
            *state = Some(ClipboardRenderState {
                wide,
                target_hwnd: target.0 as isize,
                target_process_id,
                paste_armed: false,
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
    let temporary_sequence = GetClipboardSequenceNumber();
    let _ = CloseClipboard();
    if let Err(replace_error) = replace_result {
        if let Ok(mut state) = CLIPBOARD_RENDER_STATE.lock() {
            *state = None;
        }
        return match restore_clipboard(&original, Some(temporary_sequence)) {
            Ok(()) => Err(replace_error),
            Err(restore_error) => Err(FlowError::Windows(format!(
                "{replace_error} The previous clipboard also could not be restored: {restore_error}"
            ))),
        };
    }
    Ok(Some((original, temporary_sequence)))
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

unsafe fn capture_open_clipboard() -> Result<Option<Vec<ClipboardItem>>> {
    const MAX_ITEM_BYTES: usize = 8 * 1024 * 1024;
    const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;

    let mut items = Vec::new();
    let mut total_bytes: usize = 0;
    let mut previous_format = 0;
    loop {
        SetLastError(ERROR_SUCCESS);
        let format = EnumClipboardFormats(previous_format);
        if format == 0 {
            if GetLastError() != ERROR_SUCCESS {
                return Ok(None);
            }
            break;
        }
        if !is_hglobal_clipboard_format(format) {
            return Ok(None);
        }
        let handle = match GetClipboardData(format) {
            Ok(handle) => handle,
            Err(_) => return Ok(None),
        };
        let allocation = HGLOBAL(handle.0);
        let size = GlobalSize(allocation);
        if size == 0 {
            return Ok(None);
        }
        let Some(next_total) = total_bytes.checked_add(size) else {
            return Ok(None);
        };
        if size > MAX_ITEM_BYTES || next_total > MAX_TOTAL_BYTES {
            return Ok(None);
        }
        let pointer = GlobalLock(allocation).cast::<u8>();
        if pointer.is_null() {
            return Ok(None);
        }
        items.push(ClipboardItem {
            format,
            data: std::slice::from_raw_parts(pointer, size).to_vec(),
        });
        total_bytes = next_total;
        let _ = GlobalUnlock(allocation);
        previous_format = format;
    }
    if CountClipboardFormats() > 0 && items.is_empty() {
        return Ok(None);
    }
    Ok(Some(items))
}

fn is_hglobal_clipboard_format(format: u32) -> bool {
    // These ranges use owner-managed or GDI handles rather than HGLOBAL memory.
    // Common semantic equivalents such as CF_DIB are still captured.
    !matches!(format, 2 | 3 | 9 | 14 | 128 | 130 | 131 | 142 | 512..=1023)
}

unsafe fn restore_clipboard(items: &[ClipboardItem], expected_sequence: Option<u32>) -> Result<()> {
    open_clipboard_with_retry(
        clipboard_owner()?,
        "Could not restore the clipboard; temporary dictation text may remain",
    )?;
    let result = (|| {
        if expected_sequence.is_some_and(|sequence| GetClipboardSequenceNumber() != sequence) {
            // Another application or the user replaced the temporary clipboard
            // while Flow was waiting to acquire it. Preserve that newer value.
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
    if message == WM_RENDERFORMAT && wparam.0 as u32 == CF_UNICODETEXT {
        if let Ok(mut guard) = CLIPBOARD_RENDER_STATE.lock() {
            if let Some(state) = guard.as_mut() {
                let (should_render, target_identified) = match GetOpenClipboardWindow() {
                    Ok(requester) => {
                        let mut requester_process_id = 0;
                        let requested_by_target = state.paste_armed
                            && GetWindowThreadProcessId(requester, Some(&mut requester_process_id))
                                != 0
                            && requester_process_id == state.target_process_id;
                        (requested_by_target, requested_by_target)
                    }
                    Err(_) => (
                        state.paste_armed && GetForegroundWindow().0 as isize == state.target_hwnd,
                        false,
                    ),
                };
                if !should_render {
                    // Keep the format delayed when a clipboard monitor asks first;
                    // only the dictation target may materialize the temporary text.
                    return LRESULT(0);
                }
                state.rendered = Some(
                    write_clipboard_wide(&state.wide)
                        .map(|_| (GetClipboardSequenceNumber(), target_identified))
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
    state.paste_armed = true;
    send_paste_shortcut()
}

fn wait_for_temporary_clipboard_request(mut sequence: u32, timeout: Duration) -> Result<u32> {
    let deadline = Instant::now() + timeout;
    let mut ownerless_render = None;
    let mut guard = CLIPBOARD_RENDER_STATE
        .lock()
        .map_err(|_| FlowError::Windows("The clipboard renderer is unavailable.".into()))?;
    loop {
        if let Some(result) = guard.as_ref().and_then(|state| state.rendered.as_ref()) {
            match result {
                Ok((rendered_sequence, true)) => return Ok(*rendered_sequence),
                Ok((rendered_sequence, false)) => {
                    // OpenClipboard(NULL) provides no requester identity. Keep the
                    // text available for the full timeout so a clipboard monitor
                    // cannot make Flow restore it before the target consumes it.
                    sequence = *rendered_sequence;
                    ownerless_render = Some(*rendered_sequence);
                }
                Err(error) => {
                    return Err(FlowError::Windows(format!(
                        "Could not render the paste: {error}"
                    )));
                }
            }
        }
        if unsafe { GetClipboardSequenceNumber() } != sequence {
            return Err(FlowError::Windows(
                "The clipboard changed before the destination requested the dictation.".into(),
            ));
        }
        let now = Instant::now();
        if now >= deadline {
            if let Some(rendered_sequence) = ownerless_render {
                return Ok(rendered_sequence);
            }
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

unsafe fn send_unicode(text: &str) -> Result<()> {
    const INPUTS_PER_CHUNK: usize = 128;

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

    for chunk in inputs.chunks(INPUTS_PER_CHUNK) {
        let mut sent = 0;
        while sent < chunk.len() {
            let inserted = SendInput(&chunk[sent..], size_of::<INPUT>() as i32) as usize;
            if inserted == 0 {
                return Err(FlowError::Windows(format!(
                    "Windows accepted {sent} of {} keyboard input events in the current chunk.",
                    chunk.len()
                )));
            }
            sent += inserted;
        }
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
