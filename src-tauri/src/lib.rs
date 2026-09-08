mod audio;
pub mod clipboard_snapshot;
mod credentials;
mod database;
pub mod diagnostics;
mod error;
mod groq;
mod models;
mod platform;
pub mod recovery;
pub mod text;
pub mod workflow;

use audio::AudioRecorder;
use database::Database;
use error::{FlowError, Result};
use groq::GroqClient;
use models::{
    AppConfig, DashboardData, DictionaryEntry, HistoryEntry, Microphone, SettingsData, Snippet,
    WorkflowStateSnapshot,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

const TRAY_ID: &str = "flow-tray";

pub struct AppState {
    pub workflow: workflow::WorkflowCoordinator,
    pub database: Database,
    pub recorder: AudioRecorder,
    pub groq: GroqClient,
}

/// Per-command authorization decision: the overlay window may invoke only
/// `get_workflow_state` (a read-only snapshot) and receives the
/// `workflow-state` event. Every other command — including start/stop/cancel
/// recording, cancel processing, and microphone listing — accepts only the
/// main-window label.
fn authorize_label(label: &str) -> Result<()> {
    if label != "main" {
        return Err(FlowError::Unauthorized);
    }
    Ok(())
}

fn require_main_window(window: &tauri::WebviewWindow) -> Result<()> {
    authorize_label(window.label())
}

#[tauri::command]
fn get_workflow_state(state: State<'_, AppState>) -> Result<WorkflowStateSnapshot> {
    Ok(state.workflow.snapshot())
}

#[tauri::command]
fn get_app_config(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppConfig> {
    require_main_window(&window)?;
    let mut config = state.database.app_config();
    if let Some((backup_file, backup_expires_at)) = state.database.get_backup_info() {
        config.backup_file = Some(backup_file);
        config.backup_expires_at = Some(backup_expires_at);
    }
    if let Ok(app_data) = app.path().app_data_dir() {
        let rec_dir = app_data.join("recovery");
        config.allocated_recovery_bytes = Some(recovery::calculate_recovery_dir_usage(&rec_dir).1);
    }
    Ok(config)
}

#[tauri::command]
fn get_dashboard(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<DashboardData> {
    require_main_window(&window)?;
    state.database.dashboard()
}

#[tauri::command]
fn get_history_page(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    limit: u32,
    before_created_at: Option<i64>,
    before_id: Option<i64>,
) -> Result<Vec<HistoryEntry>> {
    require_main_window(&window)?;
    state
        .database
        .history_page(limit as usize, before_created_at, before_id)
}

#[tauri::command]
fn delete_history_entry(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: i64,
) -> Result<()> {
    require_main_window(&window)?;
    state.database.delete_history_entry(id)
}

#[tauri::command]
fn delete_all_history(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    delete_pending: bool,
) -> Result<()> {
    require_main_window(&window)?;
    state.database.delete_all_history(delete_pending)
}

#[tauri::command]
fn reset_statistics(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<()> {
    require_main_window(&window)?;
    state.database.reset_statistics()
}

#[tauri::command]
fn delete_upgrade_backup(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<()> {
    require_main_window(&window)?;
    state.database.delete_upgrade_backup()
}

#[tauri::command]
fn delete_api_key(window: tauri::WebviewWindow) -> Result<()> {
    require_main_window(&window)?;
    credentials::delete_api_key()
}

#[tauri::command]
fn list_dictionary(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> Result<Vec<DictionaryEntry>> {
    require_main_window(&window)?;
    state.database.dictionary()
}

#[tauri::command]
fn add_dictionary(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    value: String,
    correction: Option<String>,
) -> Result<DictionaryEntry> {
    require_main_window(&window)?;
    state.database.add_dictionary(&value, correction.as_deref())
}

#[tauri::command]
fn update_dictionary(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: i64,
    value: String,
    correction: Option<String>,
) -> Result<()> {
    require_main_window(&window)?;
    state
        .database
        .update_dictionary(id, &value, correction.as_deref())
}

#[tauri::command]
fn delete_dictionary(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: i64,
) -> Result<()> {
    require_main_window(&window)?;
    state.database.delete_dictionary(id)
}

#[tauri::command]
fn list_snippets(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<Vec<Snippet>> {
    require_main_window(&window)?;
    state.database.snippets()
}

#[tauri::command]
fn add_snippet(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    trigger: String,
    content: String,
) -> Result<Snippet> {
    require_main_window(&window)?;
    state.database.add_snippet(&trigger, &content)
}

#[tauri::command]
fn update_snippet(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    id: i64,
    trigger: String,
    content: String,
) -> Result<()> {
    require_main_window(&window)?;
    state.database.update_snippet(id, &trigger, &content)
}

#[tauri::command]
fn delete_snippet(window: tauri::WebviewWindow, state: State<'_, AppState>, id: i64) -> Result<()> {
    require_main_window(&window)?;
    state.database.delete_snippet(id)
}

#[tauri::command]
fn get_settings(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Result<SettingsData> {
    require_main_window(&window)?;
    state.database.settings(credentials::has_api_key())
}

/// Reverts the autostart flag and tray tooltip to the previous settings.
/// Rollback failures are detected and reported instead of ignored.
fn revert_autostart_and_tooltip(
    autostart: &tauri_plugin_autostart::AutoLaunchManager,
    tray: &TrayIcon,
    previous: &SettingsData,
) -> std::result::Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    if let Err(error) = set_autostart(autostart, previous.launch_at_startup) {
        failures.push(format!("launch-at-startup could not be reverted: {error}"));
    }
    let tooltip = format!("Flow — {} to dictate", previous.keybind);
    if let Err(error) = tray.set_tooltip(Some(&tooltip)) {
        failures.push(format!("the tray tooltip could not be reverted: {error}"));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[tauri::command]
fn save_settings(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    settings: SettingsData,
    api_key: Option<String>,
) -> Result<()> {
    require_main_window(&window)?;

    // Validate through the database validation path (keybind, history
    // retention) before changing autostart, the tray tooltip, or the API key.
    state.database.validate_settings(&settings)?;

    let previous = state.database.settings(credentials::has_api_key())?;
    let autostart = app.autolaunch();
    if previous.launch_at_startup != settings.launch_at_startup {
        set_autostart(&autostart, settings.launch_at_startup)?;
    }
    let tray = app
        .tray_by_id(TRAY_ID)
        .ok_or_else(|| FlowError::Message("The system tray icon is unavailable.".into()))?;
    let tooltip = format!("Flow — {} to dictate", settings.keybind);
    if let Err(error) = tray.set_tooltip(Some(&tooltip)) {
        let revert_result = revert_autostart_and_tooltip(&autostart, &tray, &previous);
        return Err(match revert_result {
            Ok(()) => {
                FlowError::Message(format!("Could not update the system tray: {error}"))
            }
            Err(revert_err) => FlowError::PartialSettingsSave(format!(
                "Could not update the system tray: {error}; {revert_err}."
            )),
        });
    }
    let api_key = api_key.filter(|key| !key.trim().is_empty());
    let previous_api_key = api_key
        .as_ref()
        .and_then(|_| credentials::read_api_key().ok());
    if let Some(api_key) = api_key.as_ref() {
        if let Err(error) = credentials::save_api_key(api_key) {
            let revert_result = revert_autostart_and_tooltip(&autostart, &tray, &previous);
            return Err(match revert_result {
                Ok(()) => error,
                Err(revert_err) => FlowError::PartialSettingsSave(format!(
                    "{error}; {revert_err}."
                )),
            });
        }
    }
    if let Err(error) = state.database.save_settings(&settings) {
        if api_key.is_some() {
            let revert_result = match previous_api_key {
                Some(ref previous_key) => credentials::save_api_key(previous_key),
                None => credentials::delete_api_key(),
            };
            if let Err(revert_err) = revert_result {
                let revert_rest = revert_autostart_and_tooltip(&autostart, &tray, &previous);
                let rest_details = match revert_rest {
                    Ok(()) => String::new(),
                    Err(details) => format!(" {details}."),
                };
                return Err(FlowError::PartialSettingsSave(format!(
                    "Settings failed to save ({error}), and API key could not be reverted ({revert_err}).{rest_details}"
                )));
            }
        }
        let revert_result = revert_autostart_and_tooltip(&autostart, &tray, &previous);
        return Err(match revert_result {
            Ok(()) => error,
            Err(revert_err) => FlowError::PartialSettingsSave(format!(
                "Settings failed to save ({error}), and the previous settings could not be fully restored: {revert_err}."
            )),
        });
    }
    platform::configure_keybind(&settings.keybind);
    Ok(())
}

fn set_autostart(
    autostart: &tauri_plugin_autostart::AutoLaunchManager,
    enabled: bool,
) -> Result<()> {
    if enabled {
        autostart.enable().map_err(|error| {
            FlowError::Message(format!("Could not enable launch at startup: {error}"))
        })
    } else {
        autostart.disable().map_err(|error| {
            FlowError::Message(format!("Could not disable launch at startup: {error}"))
        })
    }
}

#[tauri::command]
fn list_microphones(window: tauri::WebviewWindow) -> Result<Vec<Microphone>> {
    require_main_window(&window)?;
    audio::list_microphones()
}

#[tauri::command]
async fn test_api_key(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    api_key: String,
) -> Result<()> {
    require_main_window(&window)?;
    state.groq.test_key(&api_key).await
}

#[tauri::command]
fn copy_text(window: tauri::WebviewWindow, text: String) -> Result<()> {
    require_main_window(&window)?;
    platform::copy_text(&text)
}

#[tauri::command]
fn start_recording(window: tauri::WebviewWindow, app: AppHandle) -> Result<()> {
    require_main_window(&window)?;
    workflow::start(&app)
}

#[tauri::command]
async fn stop_recording(window: tauri::WebviewWindow, app: AppHandle) -> Result<()> {
    require_main_window(&window)?;
    workflow::stop_and_process(&app).await
}

#[tauri::command]
async fn retry_pending_dictation(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: i64,
) -> Result<()> {
    require_main_window(&window)?;
    workflow::retry_pending(&app, id).await
}

#[tauri::command]
async fn retry_pending_transcription(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: i64,
) -> Result<()> {
    require_main_window(&window)?;
    workflow::retry_pending_transcription(&app, id).await
}

#[tauri::command]
fn delete_pending_dictation(window: tauri::WebviewWindow, app: AppHandle, id: i64) -> Result<()> {
    require_main_window(&window)?;
    workflow::discard_pending(&app, id)
}

#[tauri::command]
fn cancel_recording(window: tauri::WebviewWindow, app: AppHandle) -> Result<()> {
    require_main_window(&window)?;
    workflow::cancel(&app)
}

#[tauri::command]
fn cancel_processing(window: tauri::WebviewWindow, app: AppHandle) -> Result<()> {
    require_main_window(&window)?;
    workflow::cancel(&app)
}

#[tauri::command]
fn start_shortcut_capture(window: tauri::WebviewWindow) -> Result<()> {
    require_main_window(&window)?;
    platform::start_shortcut_capture();
    Ok(())
}

#[tauri::command]
fn cancel_shortcut_capture(window: tauri::WebviewWindow) -> Result<()> {
    require_main_window(&window)?;
    platform::cancel_shortcut_capture();
    Ok(())
}

#[tauri::command]
fn start_microphone_test(
    window: tauri::WebviewWindow,
    app: AppHandle,
    device_id: String,
) -> Result<()> {
    require_main_window(&window)?;
    workflow::start_mic_test(&app, &device_id)
}

#[tauri::command]
fn stop_microphone_test(window: tauri::WebviewWindow, app: AppHandle) -> Result<()> {
    require_main_window(&window)?;
    workflow::stop_mic_test(&app)
}

#[tauri::command]
fn acknowledge_privacy_notice(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    version: i64,
) -> Result<()> {
    require_main_window(&window)?;
    state.database.set_privacy_notice_acknowledged(version)
}

#[tauri::command]
fn accept_pending_transcript(window: tauri::WebviewWindow, app: AppHandle, id: i64) -> Result<()> {
    require_main_window(&window)?;
    workflow::accept_pending_transcript(&app, id)
}

#[tauri::command]
fn export_diagnostics(window: tauri::WebviewWindow) -> Result<String> {
    require_main_window(&window)?;
    Ok(diagnostics::export_diagnostics())
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn create_tray(
    app: &tauri::App,
    keybind: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Open Flow", true, None::<&str>)?;
    let dictate = MenuItem::with_id(app, "dictate", "Start dictating", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Flow", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &dictate, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(format!("Flow — {keybind} to dictate"))
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "dictate" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    workflow::toggle_from_tray(&app).await;
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button_state: MouseButtonState::Down,
                ..
            } => platform::remember_target(),
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                show_main(tray.app_handle());
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Could not locate Flow's data folder: {error}"))?;
            let _ = std::fs::create_dir_all(&data_dir);
            let log_dir = data_dir.join("logs");
            diagnostics::DiagnosticsLogger::init(&log_dir);

            let database = Database::open(&data_dir.join("flow.sqlite3"))?;
            let settings = database.settings(credentials::has_api_key())?;
            platform::configure_keybind(&settings.keybind);
            let groq = GroqClient::new()?;
            let workflow = workflow::WorkflowCoordinator::new();

            // Cache native HWNDs
            let main_win = app.get_webview_window("main");
            let overlay_win = app.get_webview_window("overlay");
            if let (Some(m), Some(o)) = (main_win.as_ref(), overlay_win.as_ref()) {
                if let (Ok(m_hwnd), Ok(o_hwnd)) = (m.hwnd(), o.hwnd()) {
                    platform::cache_flow_hwnds(m_hwnd.0 as isize, o_hwnd.0 as isize);
                }
            }

            // Recovery spools check & import on startup
            let recovery_dir = data_dir.join("recovery");
            let _ = recovery::scan_and_import_spools(&recovery_dir, &database);

            app.manage(AppState {
                workflow,
                database,
                recorder: AudioRecorder::new(),
                groq,
            });

            create_tray(app, &settings.keybind)?;
            platform::install_keyboard_hook(app.handle().clone())?;

            // Background hourly maintenance task (retention & quota cleanup)
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    // Read the active pending ID so active-item exclusion stays
                    // enabled, then run the synchronous database cleanup on a
                    // blocking thread so the async runtime is never blocked.
                    let active_pending_id = app_handle
                        .state::<AppState>()
                        .workflow
                        .active_pending_id();
                    let blocker = app_handle.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let state = blocker.state::<AppState>();
                        state.database.run_maintenance(active_pending_id)
                    })
                    .await;
                }
            });

            if std::env::args().any(|argument| argument == "--minimized") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_workflow_state,
            get_app_config,
            get_dashboard,
            get_history_page,
            delete_history_entry,
            delete_all_history,
            reset_statistics,
            delete_upgrade_backup,
            delete_api_key,
            list_dictionary,
            add_dictionary,
            update_dictionary,
            delete_dictionary,
            list_snippets,
            add_snippet,
            update_snippet,
            delete_snippet,
            get_settings,
            save_settings,
            list_microphones,
            test_api_key,
            copy_text,
            start_recording,
            stop_recording,
            cancel_recording,
            cancel_processing,
            retry_pending_dictation,
            retry_pending_transcription,
            delete_pending_dictation,
            start_shortcut_capture,
            cancel_shortcut_capture,
            start_microphone_test,
            stop_microphone_test,
            accept_pending_transcript,
            acknowledge_privacy_notice,
            export_diagnostics,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Flow");
}

#[cfg(test)]
mod tests {
    use super::{authorize_label, text};

    #[test]
    fn command_allowlist_accepts_only_the_main_window_label() {
        assert!(authorize_label("main").is_ok());
        // The overlay label must not invoke sensitive commands.
        assert!(authorize_label("overlay").is_err());
        assert!(authorize_label("").is_err());
        assert!(authorize_label("other").is_err());
    }

    #[test]
    fn snippet_matching_normalizes_case_spacing_and_edge_punctuation() {
        assert_eq!(
            text::normalize_snippet_trigger("  My   EMAIL address... "),
            Some("my email address".to_string())
        );
        assert_eq!(
            text::normalize_snippet_trigger("。状态؟"),
            Some("状态".to_string())
        );
    }

    #[test]
    fn snippet_matching_keeps_internal_words_exact() {
        assert_ne!(
            text::normalize_snippet_trigger("insert my signature"),
            text::normalize_snippet_trigger("insert signature")
        );
    }
}
