use std::sync::{
    atomic::{AtomicU64, Ordering},
    LazyLock, Mutex,
};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use tauri::{AppHandle, Emitter, Manager};
use unicode_categories::UnicodeCategories;

use crate::{
    credentials,
    error::{FlowError, Result},
    models::{DictionaryEntry, MessagePayload, OverlayPayload},
    platform, AppState,
};

static ERROR_GENERATION: AtomicU64 = AtomicU64::new(0);
static ERROR_DISMISS_TASK: LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(None));
static CORRECTION_MATCHER: LazyLock<Mutex<CorrectionMatcherCache>> =
    LazyLock::new(|| Mutex::new(CorrectionMatcherCache::default()));

#[derive(Default)]
struct CorrectionMatcherCache {
    corrections: Vec<(String, String)>,
    matcher: Option<AhoCorasick>,
}

pub async fn toggle(app: &AppHandle) {
    toggle_with_target(app, platform::remembered_target()).await;
}

pub async fn toggle_from_tray(app: &AppHandle) {
    toggle_with_target(app, platform::remembered_target()).await;
}

async fn toggle_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) {
    let state = app.state::<AppState>();
    if state.recorder.is_recording() {
        if let Err(error) = stop_and_process_with_target(app, target).await {
            let _ = reported_error(app, error);
        }
    } else if !state.busy.load(Ordering::Acquire) {
        if let Err(error) = start_with_target(app, target) {
            let _ = reported_error(app, error);
        }
    }
}

pub fn start(app: &AppHandle) -> Result<()> {
    start_with_target(app, None).map_err(|error| reported_error(app, error))
}

fn start_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) -> Result<()> {
    let state = app.state::<AppState>();
    if state.busy.swap(true, Ordering::AcqRel) {
        return Err(FlowError::AlreadyRecording);
    }
    ERROR_GENERATION.fetch_add(1, Ordering::AcqRel);
    cancel_error_dismiss();
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

pub async fn stop_and_process(app: &AppHandle) -> Result<()> {
    stop_and_process_with_target(app, Some(platform::capture_target()))
        .await
        .map_err(|error| reported_error(app, error))
}

async fn stop_and_process_with_target(
    app: &AppHandle,
    target: Option<platform::TargetWindow>,
) -> Result<()> {
    let state = app.state::<AppState>();
    if !state.recorder.is_recording() {
        return if state.capture_limit_processing.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(FlowError::NotRecording)
        };
    }
    if state.processing.swap(true, Ordering::AcqRel) {
        return Err(FlowError::Message(
            "Flow is already processing a dictation.".into(),
        ));
    }
    platform::set_recording(false);
    let recording = match state.recorder.stop() {
        Ok(recording) => recording,
        Err(error) => {
            if matches!(error, FlowError::NotRecording)
                && state.capture_limit_processing.load(Ordering::Acquire)
            {
                return Ok(());
            }
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
            return Err(error);
        }
    };
    process_captured_inner(app, recording, target).await
}

pub(crate) fn process_captured_in_background(
    app: &AppHandle,
    recording: crate::audio::CapturedAudio,
) {
    let state = app.state::<AppState>();
    state
        .capture_limit_processing
        .store(true, Ordering::Release);
    state.processing.store(true, Ordering::Release);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = process_captured_inner(&app, recording, None).await;
        app.state::<AppState>()
            .capture_limit_processing
            .store(false, Ordering::Release);
        if let Err(error) = result {
            report_error(&app, error);
        }
    });
}

async fn process_captured_inner(
    app: &AppHandle,
    recording: crate::audio::CapturedAudio,
    target: Option<platform::TargetWindow>,
) -> Result<()> {
    let state = app.state::<AppState>();
    // Capture the destination when dictation is stopped. Processing may take
    // several seconds, during which the foreground window can change again.
    let paste_target = target.unwrap_or(recording.target);
    let pending_id = match state
        .database
        .insert_pending_recording(&recording.wav, recording.duration_ms)
    {
        Ok(id) => id,
        Err(error) => {
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
            return Err(error);
        }
    };
    run_pending(app, pending_id, Some(paste_target)).await
}

pub async fn retry_pending(app: &AppHandle, id: i64) -> Result<()> {
    retry_pending_inner(app, id)
        .await
        .map_err(|error| reported_error(app, error))
}

async fn retry_pending_inner(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    if state.busy.swap(true, Ordering::AcqRel) {
        return Err(FlowError::Message(
            "Flow is busy with another dictation.".into(),
        ));
    }
    if state.processing.swap(true, Ordering::AcqRel) {
        state.busy.store(false, Ordering::Release);
        return Err(FlowError::Message(
            "Flow is already processing a dictation.".into(),
        ));
    }
    ERROR_GENERATION.fetch_add(1, Ordering::AcqRel);
    cancel_error_dismiss();
    let overlay_target = platform::capture_target();
    if let Err(error) = platform::prepare_overlay(app, overlay_target) {
        state.processing.store(false, Ordering::Release);
        state.busy.store(false, Ordering::Release);
        return Err(error);
    }
    run_pending(app, id, None).await
}

async fn run_pending(
    app: &AppHandle,
    pending_id: i64,
    paste_target: Option<platform::TargetWindow>,
) -> Result<()> {
    let state = app.state::<AppState>();
    emit_overlay(app, "analysing", Some("Analyzing"));
    let result = async {
        let pending = state.database.pending_dictation(pending_id)?;
        let settings = state.database.settings(true)?;
        let transcript = if let Some(transcript) = pending.raw_text.clone() {
            transcript
        } else {
            let api_key = credentials::read_api_key()?;
            let dictionary = state.database.dictionary()?;
            let (preferred_spellings, _) = dictionary_guidance(&dictionary);
            let wav = pending.wav.ok_or_else(|| {
                FlowError::Message("The recoverable recording is incomplete.".into())
            })?;
            let transcript = state
                .groq
                .transcribe(&api_key, wav, &preferred_spellings)
                .await?;
            state
                .database
                .save_pending_transcript(pending_id, &transcript)?;
            transcript
        };

        let final_text = if let Some(final_text) = pending.final_text {
            final_text
        } else {
            let api_key = credentials::read_api_key()?;
            let dictionary = state.database.dictionary()?;
            let (_, corrections) = dictionary_guidance(&dictionary);
            let normalized = normalize_with_corrections(&transcript, &corrections);
            let snippet = state
                .database
                .snippets()?
                .into_iter()
                .find(|snippet| normalize_utterance(&snippet.trigger) == normalized);
            let final_text = if let Some(snippet) = snippet {
                snippet.content
            } else {
                emit_overlay(app, "thinking", Some("Thinking"));
                state
                    .groq
                    .clean(&api_key, &transcript, &corrections)
                    .await?
            };
            state.database.save_pending_final(pending_id, &final_text)?;
            final_text
        };

        state
            .database
            .save_pending_to_history(pending.id, &settings.history_retention)?;
        let completion_message = if let Some(paste_target) = paste_target {
            platform::paste_text(paste_target, &final_text)?;
            "Dictation pasted"
        } else {
            platform::copy_text(&final_text)?;
            "Recovered dictation copied"
        };
        state.database.delete_pending(pending.id)?;
        let _ = app.emit_to("overlay", "overlay-progress-complete", ());
        Ok::<_, FlowError>(completion_message)
    }
    .await;

    match result {
        Ok(completion_message) => {
            dismiss_overlay(app, None).await;
            let _ = app.emit(
                "dictation-complete",
                MessagePayload {
                    message: completion_message.into(),
                },
            );
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
            Ok(())
        }
        Err(error) => {
            let message = friendly_error_ref(&error);
            let _ = state.database.save_pending_error(pending_id, &message);
            state.processing.store(false, Ordering::Release);
            state.busy.store(false, Ordering::Release);
            Err(error)
        }
    }
}

pub fn cancel(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    state
        .recorder
        .cancel()
        .map_err(|error| reported_error(app, error))?;
    platform::set_recording(false);
    state.busy.store(false, Ordering::Release);
    let _ = app.emit_to("overlay", "overlay-dismiss", ());
    let generation = ERROR_GENERATION.load(Ordering::Acquire);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(130)).await;
        if ERROR_GENERATION.load(Ordering::Acquire) == generation {
            hide_overlay(&app_clone);
        }
    });
    Ok(())
}

pub fn report_error(app: &AppHandle, error: FlowError) {
    let _ = reported_error(app, error);
}

fn reported_error(app: &AppHandle, error: FlowError) -> FlowError {
    platform::set_recording(false);
    let state = app.state::<AppState>();
    if !state.processing.load(Ordering::Acquire) {
        state.busy.store(false, Ordering::Release);
    }
    let message = friendly_error(error);
    eprintln!("Flow error: {message}");
    let _ = platform::prepare_overlay(app, platform::capture_target());
    emit_overlay(app, "error", Some(&message));
    let _ = app.emit(
        "flow-error",
        MessagePayload {
            message: message.clone(),
        },
    );
    let generation = ERROR_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let app_clone = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        if ERROR_GENERATION.load(Ordering::Acquire) == generation {
            dismiss_overlay(&app_clone, Some(generation)).await;
        }
    });
    if let Ok(mut previous) = ERROR_DISMISS_TASK.lock() {
        if let Some(previous) = previous.replace(task) {
            previous.abort();
        }
    }
    crate::show_main(app);
    FlowError::Message(message)
}

fn cancel_error_dismiss() {
    if let Ok(mut task) = ERROR_DISMISS_TASK.lock() {
        if let Some(task) = task.take() {
            task.abort();
        }
    }
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

async fn dismiss_overlay(app: &AppHandle, expected_generation: Option<u64>) {
    let _ = app.emit_to("overlay", "overlay-dismiss", ());
    tokio::time::sleep(std::time::Duration::from_millis(130)).await;
    if expected_generation
        .map(|generation| ERROR_GENERATION.load(Ordering::Acquire) == generation)
        .unwrap_or(true)
    {
        hide_overlay(app);
    }
}

fn dictionary_guidance(entries: &[DictionaryEntry]) -> (Vec<String>, Vec<(String, String)>) {
    let preferred_spellings = entries
        .iter()
        .map(|entry| entry.correction.as_ref().unwrap_or(&entry.value).to_owned())
        .collect();
    let corrections = entries
        .iter()
        .filter_map(|entry| {
            entry.correction.as_ref().map(|correction| {
                (
                    normalize_correction_source(&entry.value),
                    correction.clone(),
                )
            })
        })
        .collect();
    (preferred_spellings, corrections)
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

pub(crate) fn normalize_correction_source(value: &str) -> String {
    value
        .trim_start_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '"' | '\'' | '(' | '[' | '{' | '“' | '‘' | '¿' | '¡'
                )
        })
        .trim_end_matches(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | ','
                        | '!'
                        | '?'
                        | ';'
                        | ':'
                        | '"'
                        | '\''
                        | ')'
                        | ']'
                        | '}'
                        | '…'
                        | '”'
                        | '’'
                )
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_with_corrections(value: &str, corrections: &[(String, String)]) -> String {
    let normalized = normalize_correction_source(value);
    let Ok(mut cache) = CORRECTION_MATCHER.lock() else {
        return normalize_utterance(&normalized);
    };
    let next = corrections
        .iter()
        .map(|(incorrect, correct)| {
            (
                normalize_correction_source(incorrect),
                normalize_correction_replacement(correct),
            )
        })
        .filter(|(incorrect, correct)| !incorrect.is_empty() && !correct.is_empty())
        .collect::<Vec<_>>();
    if cache.corrections != next {
        cache.matcher = if next.is_empty() {
            None
        } else {
            AhoCorasickBuilder::new()
                .build(next.iter().map(|(incorrect, _)| incorrect))
                .ok()
        };
        cache.corrections = next;
    }
    let Some(matcher) = cache.matcher.as_ref() else {
        return normalize_utterance(&normalized);
    };
    let mut corrected = String::with_capacity(normalized.len());
    let mut index = 0;
    let mut matches = matcher
        .find_overlapping_iter(&normalized)
        .filter(|matching| {
            normalized[..matching.start()]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric())
                && normalized[matching.end()..]
                    .chars()
                    .next()
                    .is_none_or(|character| !character.is_alphanumeric())
        })
        .collect::<Vec<_>>();
    matches.sort_unstable_by(|left, right| {
        left.start()
            .cmp(&right.start())
            .then_with(|| right.len().cmp(&left.len()))
    });
    for matching in matches {
        if matching.start() < index {
            continue;
        }
        corrected.push_str(&normalized[index..matching.start()]);
        corrected.push_str(&cache.corrections[matching.pattern()].1);
        index = matching.end();
    }
    corrected.push_str(&normalized[index..]);
    normalize_utterance(&corrected)
}

fn normalize_correction_replacement(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn friendly_error(error: FlowError) -> String {
    friendly_error_ref(&error)
}

fn friendly_error_ref(error: &FlowError) -> String {
    match error {
        FlowError::Network(_) => {
            "Flow couldn’t reach Groq. Check your connection and try again.".into()
        }
        FlowError::Windows(message) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::models::DictionaryEntry;

    use super::{dictionary_guidance, normalize_with_corrections};

    #[test]
    fn regular_dictionary_words_never_reach_cleanup_guidance() {
        let entries = vec![
            DictionaryEntry {
                id: 1,
                value: "Flow".into(),
                correction: None,
                created_at: 1,
            },
            DictionaryEntry {
                id: 2,
                value: " btw. ".into(),
                correction: Some("by the way".into()),
                created_at: 2,
            },
            DictionaryEntry {
                id: 3,
                value: ".NET".into(),
                correction: Some("dotnet".into()),
                created_at: 3,
            },
        ];

        let (preferred_spellings, corrections) = dictionary_guidance(&entries);
        assert_eq!(preferred_spellings, ["Flow", "by the way", "dotnet"]);
        assert_eq!(
            corrections,
            [
                ("btw".into(), "by the way".into()),
                (".net".into(), "dotnet".into())
            ]
        );
    }

    #[test]
    fn dictionary_corrections_apply_before_snippet_matching() {
        let corrections = vec![
            ("four word".into(), "Forward".into()),
            ("see sharp".into(), "C#".into()),
            ("C++".into(), "C Plus Plus".into()),
        ];
        assert_eq!(
            normalize_with_corrections("Please, four word now.", &corrections),
            "please, forward now"
        );
        assert_eq!(
            normalize_with_corrections("four words", &corrections),
            "four words"
        );
        assert_eq!(
            normalize_with_corrections("see sharp project", &corrections),
            "c# project"
        );
        assert_eq!(
            normalize_with_corrections("C++ project", &corrections),
            "c plus plus project"
        );
        assert_eq!(
            normalize_with_corrections("c project", &corrections),
            "c project"
        );
    }

    #[test]
    fn correction_matching_keeps_valid_shorter_overlaps() {
        let corrections = vec![("c".into(), "see".into()), ("c sharp".into(), "C#".into())];
        assert_eq!(
            normalize_with_corrections("c sharper", &corrections),
            "see sharper"
        );
    }
}
