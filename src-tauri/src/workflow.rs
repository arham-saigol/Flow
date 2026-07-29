use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{AppHandle, Emitter, Manager};
use unicode_categories::UnicodeCategories;

use crate::{
    credentials,
    error::{FlowError, Result},
    models::{MessagePayload, OverlayPayload},
    platform, AppState,
};

static ERROR_GENERATION: AtomicU64 = AtomicU64::new(0);

pub async fn toggle(app: &AppHandle) {
    toggle_with_target(app, platform::remembered_target()).await;
}

pub async fn toggle_from_tray(app: &AppHandle) {
    toggle_with_target(app, platform::remembered_target()).await;
}

async fn toggle_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) {
    let state = app.state::<AppState>();
    if state.recorder.is_recording() {
        stop_and_process_with_target(app, target).await;
    } else if !state.busy.load(Ordering::Acquire) {
        if let Err(error) = start_with_target(app, target) {
            report_error(app, error);
        }
    }
}

pub fn start(app: &AppHandle) -> Result<()> {
    start_with_target(app, None)
}

fn start_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) -> Result<()> {
    let state = app.state::<AppState>();
    if state.busy.swap(true, Ordering::AcqRel) {
        return Err(FlowError::AlreadyRecording);
    }
    ERROR_GENERATION.fetch_add(1, Ordering::AcqRel);
    let result = (|| {
        let has_key = credentials::has_api_key();
        if !has_key {
            return Err(FlowError::MissingApiKey);
        }
        let settings = state.database.settings(true)?;
        let target = target.unwrap_or_else(platform::capture_target);
        platform::prepare_overlay(app, target)?;
        emit_overlay(app, "recording", None);
        state
            .recorder
            .start(app.clone(), &settings.microphone_id, target)?;
        platform::set_recording(true);
        Ok(())
    })();
    if result.is_err() {
        state.busy.store(false, Ordering::Release);
    }
    result
}

pub async fn stop_and_process(app: &AppHandle) {
    stop_and_process_with_target(app, Some(platform::capture_target())).await;
}

async fn stop_and_process_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) {
    let state = app.state::<AppState>();
    if state.processing.swap(true, Ordering::AcqRel) {
        return;
    }
    platform::set_recording(false);
    let recording = match state.recorder.stop() {
        Ok(recording) => recording,
        Err(error) => {
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
            report_error(app, error);
            return;
        }
    };
    // Capture the destination when dictation is stopped. Processing may take
    // several seconds, during which the foreground window can change again.
    let paste_target = target.unwrap_or(recording.target);
    emit_overlay(app, "analysing", Some("Analyzing"));

    let result = async {
        let api_key = credentials::read_api_key()?;
        let settings = state.database.settings(true)?;
        let dictionary = state
            .database
            .dictionary()?
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>();
        let transcript = state
            .groq
            .transcribe(&api_key, recording.wav, &dictionary)
            .await?;

        let normalized = normalize_utterance(&transcript);
        let snippet = state
            .database
            .snippets()?
            .into_iter()
            .find(|snippet| normalize_utterance(&snippet.trigger) == normalized);
        let final_text = if let Some(snippet) = snippet {
            snippet.content
        } else {
            emit_overlay(app, "thinking", Some("Thinking"));
            state.groq.clean(&api_key, &transcript, &dictionary).await?
        };

        let _ = app.emit_to("overlay", "overlay-progress-complete", ());
        tokio_sleep(std::time::Duration::from_millis(150)).await;
        platform::paste_text(paste_target, &final_text)?;
        let history_result = state
            .database
            .insert_history(&final_text, &transcript, recording.duration_ms)
            .and_then(|_| state.database.prune_history(&settings.history_retention));
        Ok::<_, FlowError>(history_result.err())
    }
    .await;

    match result {
        Ok(history_error) => {
            dismiss_overlay(app).await;
            let _ = app.emit(
                "dictation-complete",
                MessagePayload {
                    message: "Dictation pasted".into(),
                },
            );
            if let Some(error) = history_error {
                let _ = app.emit(
                    "flow-warning",
                    MessagePayload {
                        message: format!(
                            "Dictation pasted, but Flow could not update history: {error}"
                        ),
                    },
                );
            }
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
        }
        Err(error) => {
            state.processing.store(false, Ordering::Release);
            report_error(app, error);
        }
    }
}

pub fn cancel(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.recorder.cancel().is_ok() {
        platform::set_recording(false);
        state.busy.store(false, Ordering::Release);
        let _ = app.emit_to("overlay", "overlay-dismiss", ());
        let generation = ERROR_GENERATION.load(Ordering::Acquire);
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio_sleep(std::time::Duration::from_millis(130)).await;
            if ERROR_GENERATION.load(Ordering::Acquire) == generation {
                hide_overlay(&app_clone);
            }
        });
    }
}

pub fn report_error(app: &AppHandle, error: FlowError) {
    platform::set_recording(false);
    let state = app.state::<AppState>();
    if !state.processing.load(Ordering::Acquire) {
        state.busy.store(false, Ordering::Release);
    }
    let message = friendly_error(error);
    if platform::prepare_overlay(app, platform::capture_target()).is_err() {
        crate::show_main(app);
    }
    emit_overlay(app, "error", Some(&message));
    let _ = app.emit(
        "flow-error",
        MessagePayload {
            message: message.clone(),
        },
    );
    let generation = ERROR_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio_sleep(std::time::Duration::from_secs(4)).await;
        if ERROR_GENERATION.load(Ordering::Acquire) == generation {
            dismiss_overlay(&app_clone).await;
        }
    });
}

fn emit_overlay(app: &AppHandle, phase: &str, message: Option<&str>) {
    let _ = app.emit_to(
        "overlay",
        "overlay-state",
        OverlayPayload {
            phase: phase.into(),
            message: message.map(str::to_owned),
        },
    );
}

fn hide_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
    }
}

async fn dismiss_overlay(app: &AppHandle) {
    let _ = app.emit_to("overlay", "overlay-dismiss", ());
    tokio_sleep(std::time::Duration::from_millis(130)).await;
    hide_overlay(app);
}

pub(crate) fn normalize_utterance(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character: char| character.is_punctuation() || character.is_whitespace())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn friendly_error(error: FlowError) -> String {
    match error {
        FlowError::Network(_) => {
            "Flow couldn’t reach Groq. Check your connection and try again.".into()
        }
        other => other.to_string(),
    }
}

async fn tokio_sleep(duration: std::time::Duration) {
    // Tauri's async runtime provides a Tokio context through the default runtime.
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = sender.send(());
    });
    let _ = tauri::async_runtime::spawn_blocking(move || receiver.recv()).await;
}
