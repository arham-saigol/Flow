mod audio;
mod credentials;
mod database;
mod error;
mod groq;
mod models;
mod platform;
pub mod workflow;

use std::sync::atomic::AtomicBool;

use audio::AudioRecorder;
use database::Database;
use error::{FlowError, Result};
use groq::GroqClient;
use models::{DashboardData, DictionaryEntry, Microphone, SettingsData, Snippet};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

const TRAY_ID: &str = "flow-tray";

pub struct AppState {
    pub database: Database,
    pub recorder: AudioRecorder,
    pub groq: GroqClient,
    pub workflow: workflow::WorkflowState,
    pub capture_limit_processing: AtomicBool,
}

#[tauri::command]
fn get_dashboard(state: State<'_, AppState>) -> Result<DashboardData> {
    state.database.dashboard()
}

#[tauri::command]
fn list_dictionary(state: State<'_, AppState>) -> Result<Vec<DictionaryEntry>> {
    state.database.dictionary()
}

#[tauri::command]
fn add_dictionary(
    state: State<'_, AppState>,
    value: String,
    correction: Option<String>,
) -> Result<DictionaryEntry> {
    state.database.add_dictionary(&value, correction.as_deref())
}

#[tauri::command]
fn update_dictionary(
    state: State<'_, AppState>,
    id: i64,
    value: String,
    correction: Option<String>,
) -> Result<()> {
    state
        .database
        .update_dictionary(id, &value, correction.as_deref())
}

#[tauri::command]
fn delete_dictionary(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.database.delete_dictionary(id)
}

#[tauri::command]
fn list_snippets(state: State<'_, AppState>) -> Result<Vec<Snippet>> {
    state.database.snippets()
}

#[tauri::command]
fn add_snippet(state: State<'_, AppState>, trigger: String, content: String) -> Result<Snippet> {
    state.database.add_snippet(&trigger, &content)
}

#[tauri::command]
fn update_snippet(
    state: State<'_, AppState>,
    id: i64,
    trigger: String,
    content: String,
) -> Result<()> {
    state.database.update_snippet(id, &trigger, &content)
}

#[tauri::command]
fn delete_snippet(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.database.delete_snippet(id)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<SettingsData> {
    state.database.settings(credentials::has_api_key())
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: SettingsData,
    api_key: Option<String>,
) -> Result<()> {
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
        let _ = set_autostart(&autostart, previous.launch_at_startup);
        return Err(FlowError::Message(format!(
            "Could not update the system tray: {error}"
        )));
    }
    let api_key = api_key.filter(|key| !key.trim().is_empty());
    let previous_api_key = api_key
        .as_ref()
        .and_then(|_| credentials::read_api_key().ok());
    if let Some(api_key) = api_key.as_ref() {
        if let Err(error) = credentials::save_api_key(api_key) {
            let _ = set_autostart(&autostart, previous.launch_at_startup);
            let _ = tray.set_tooltip(Some(format!("Flow — {} to dictate", previous.keybind)));
            return Err(error);
        }
    }
    if let Err(error) = state.database.save_settings(&settings) {
        if api_key.is_some() {
            match previous_api_key {
                Some(previous_key) => {
                    let _ = credentials::save_api_key(&previous_key);
                }
                None => {
                    let _ = credentials::delete_api_key();
                }
            }
        }
        let _ = set_autostart(&autostart, previous.launch_at_startup);
        let _ = tray.set_tooltip(Some(format!("Flow — {} to dictate", previous.keybind)));
        return Err(error);
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
fn list_microphones() -> Result<Vec<Microphone>> {
    audio::list_microphones()
}

#[tauri::command]
async fn test_api_key(state: State<'_, AppState>, api_key: String) -> Result<()> {
    state.groq.test_key(&api_key).await
}

#[tauri::command]
fn copy_text(text: String) -> Result<()> {
    platform::copy_text(&text)
}

#[tauri::command]
fn start_recording(app: AppHandle) -> Result<()> {
    workflow::start(&app)
}

#[tauri::command]
async fn stop_recording(app: AppHandle) -> Result<()> {
    workflow::stop_and_process(&app).await
}

#[tauri::command]
async fn retry_pending_dictation(app: AppHandle, id: i64) -> Result<()> {
    workflow::retry_pending(&app, id).await
}

#[tauri::command]
fn delete_pending_dictation(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.database.delete_pending(id)
}

#[tauri::command]
fn cancel_recording(app: AppHandle) -> Result<()> {
    workflow::cancel(&app)
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
                    workflow::toggle(&app).await;
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
            let database = Database::open(&data_dir.join("flow.sqlite3"))?;
            let settings = database.settings(credentials::has_api_key())?;
            platform::configure_keybind(&settings.keybind);
            let groq = GroqClient::new()?;
            app.manage(AppState {
                database,
                recorder: AudioRecorder::new(),
                groq,
                workflow: workflow::WorkflowState::new(),
                capture_limit_processing: AtomicBool::new(false),
            });

            create_tray(app, &settings.keybind)?;
            platform::install_keyboard_hook(app.handle().clone())?;
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
            get_dashboard,
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
            retry_pending_dictation,
            delete_pending_dictation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Flow");
}

#[cfg(test)]
mod tests {
    use super::workflow;

    #[test]
    fn snippet_matching_normalizes_case_spacing_and_edge_punctuation() {
        assert_eq!(
            workflow::normalize_utterance("  My   EMAIL address... "),
            "my email address"
        );
        assert_eq!(workflow::normalize_utterance("。状态؟"), "状态");
    }

    #[test]
    fn snippet_matching_keeps_internal_words_exact() {
        assert_ne!(
            workflow::normalize_utterance("insert my signature"),
            workflow::normalize_utterance("insert signature")
        );
    }
}
