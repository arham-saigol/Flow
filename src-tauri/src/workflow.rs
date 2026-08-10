use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    LazyLock, Mutex,
};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
use tauri::{AppHandle, Emitter, Manager};
use unicode_categories::UnicodeCategories;

use crate::{
    credentials,
    error::{FlowError, Result},
    models::{DictionaryEntry, MessagePayload, OverlayPayload, TranscriptionModel},
    platform, AppState,
};

static ERROR_GENERATION: AtomicU64 = AtomicU64::new(0);
static ERROR_DISMISS_TASK: LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(None));
static CORRECTION_MATCHER: LazyLock<Mutex<CorrectionMatcherCache>> =
    LazyLock::new(|| Mutex::new(CorrectionMatcherCache::default()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkflowPhase {
    Idle,
    Starting,
    Recording,
    Processing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotkeyOutcome {
    Start,
    Stop,
    Busy(WorkflowPhase),
}

pub struct WorkflowState {
    phase: AtomicU8,
}

impl WorkflowState {
    pub fn new() -> Self {
        Self {
            phase: AtomicU8::new(WorkflowPhase::Idle as u8),
        }
    }

    fn phase(&self) -> WorkflowPhase {
        match self.phase.load(Ordering::Acquire) {
            value if value == WorkflowPhase::Starting as u8 => WorkflowPhase::Starting,
            value if value == WorkflowPhase::Recording as u8 => WorkflowPhase::Recording,
            value if value == WorkflowPhase::Processing as u8 => WorkflowPhase::Processing,
            _ => WorkflowPhase::Idle,
        }
    }

    fn hotkey_press(&self) -> HotkeyOutcome {
        loop {
            let phase = self.phase();
            let (next, outcome) = match phase {
                WorkflowPhase::Idle => (WorkflowPhase::Starting, HotkeyOutcome::Start),
                WorkflowPhase::Recording => (WorkflowPhase::Processing, HotkeyOutcome::Stop),
                WorkflowPhase::Starting | WorkflowPhase::Processing => {
                    return HotkeyOutcome::Busy(phase)
                }
            };
            if self
                .phase
                .compare_exchange(phase as u8, next as u8, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return outcome;
            }
        }
    }

    fn begin_start(&self) -> Result<()> {
        self.phase
            .compare_exchange(
                WorkflowPhase::Idle as u8,
                WorkflowPhase::Starting as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| FlowError::Message("Flow is busy with another dictation.".into()))
    }

    fn begin_stop(&self) -> Result<()> {
        match self.phase.compare_exchange(
            WorkflowPhase::Recording as u8,
            WorkflowPhase::Processing as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(value) if value == WorkflowPhase::Idle as u8 => Err(FlowError::NotRecording),
            Err(_) => Err(FlowError::Message(
                "Flow is busy with another dictation.".into(),
            )),
        }
    }

    fn begin_retry(&self) -> Result<()> {
        self.phase
            .compare_exchange(
                WorkflowPhase::Idle as u8,
                WorkflowPhase::Processing as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| FlowError::Message("Flow is busy with another dictation.".into()))
    }

    fn begin_capture_limit_processing(&self) {
        let _ = self.phase.compare_exchange(
            WorkflowPhase::Recording as u8,
            WorkflowPhase::Processing as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn recording_started(&self) -> bool {
        self.phase
            .compare_exchange(
                WorkflowPhase::Starting as u8,
                WorkflowPhase::Recording as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    fn failed(&self) {
        self.phase
            .store(WorkflowPhase::Idle as u8, Ordering::Release);
    }

    fn finished(&self) {
        self.phase
            .store(WorkflowPhase::Idle as u8, Ordering::Release);
    }
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct CorrectionMatcherCache {
    corrections: Vec<(String, String)>,
    matcher: Option<AhoCorasick>,
}

pub async fn toggle(app: &AppHandle) {
    toggle_with_target(app, platform::remembered_target()).await;
}

async fn toggle_with_target(app: &AppHandle, target: Option<platform::TargetWindow>) {
    let state = app.state::<AppState>();
    let result = match state.workflow.hotkey_press() {
        HotkeyOutcome::Start => start_reserved(app, target),
        HotkeyOutcome::Stop => stop_reserved(app, target).await,
        HotkeyOutcome::Busy(phase) => {
            show_busy_feedback(app, phase);
            return;
        }
    };
    if let Err(error) = result {
        let _ = reported_error(app, error);
    }
}

pub fn start(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    state.workflow.begin_start()?;
    start_reserved(app, None).map_err(|error| reported_error(app, error))
}

fn start_reserved(app: &AppHandle, target: Option<platform::TargetWindow>) -> Result<()> {
    let state = app.state::<AppState>();
    ERROR_GENERATION.fetch_add(1, Ordering::AcqRel);
    cancel_error_dismiss();
    let result = (|| {
        let has_groq_api_key = credentials::has_groq_api_key();
        let has_deepgram_api_key = credentials::has_deepgram_api_key();
        let settings = state
            .database
            .settings(has_groq_api_key, has_deepgram_api_key)?;
        if !has_groq_api_key {
            return Err(FlowError::MissingGroqApiKey);
        }
        if settings.transcription_model == TranscriptionModel::DeepgramNova3
            && !has_deepgram_api_key
        {
            return Err(FlowError::MissingDeepgramApiKey);
        }
        let target = target.unwrap_or_else(platform::capture_target);
        platform::prepare_overlay(app, target)?;
        emit_overlay(app, "starting", Some("Starting"));
        state
            .recorder
            .start(app.clone(), &settings.microphone_id, target)?;
        if !state.workflow.recording_started() {
            let _ = state.recorder.cancel();
            return Err(FlowError::Audio(
                "The microphone stopped while Flow was starting.".into(),
            ));
        }
        platform::set_recording(true);
        emit_overlay(app, "recording", None);
        Ok(())
    })();
    if result.is_err() {
        state.workflow.failed();
    }
    result
}

pub async fn stop_and_process(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    state.workflow.begin_stop()?;
    stop_reserved(app, Some(platform::capture_target()))
        .await
        .map_err(|error| reported_error(app, error))
}

async fn stop_reserved(app: &AppHandle, target: Option<platform::TargetWindow>) -> Result<()> {
    let state = app.state::<AppState>();
    platform::set_recording(false);
    let recording = match state.recorder.stop() {
        Ok(recording) => recording,
        Err(error) => {
            if matches!(error, FlowError::NotRecording)
                && state.capture_limit_processing.load(Ordering::Acquire)
            {
                return Ok(());
            }
            state.workflow.failed();
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
    state.workflow.begin_capture_limit_processing();
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
            state.workflow.failed();
            return Err(error);
        }
    };
    run_pending(app, pending_id, Some(paste_target)).await
}

pub async fn retry_pending(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    state.workflow.begin_retry()?;
    retry_pending_inner(app, id)
        .await
        .map_err(|error| reported_error(app, error))
}

async fn retry_pending_inner(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    ERROR_GENERATION.fetch_add(1, Ordering::AcqRel);
    cancel_error_dismiss();
    let overlay_target = platform::capture_target();
    if let Err(error) = platform::prepare_overlay(app, overlay_target) {
        state.workflow.failed();
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
        let settings = state.database.settings(
            credentials::has_groq_api_key(),
            credentials::has_deepgram_api_key(),
        )?;
        let transcript = if let Some(transcript) = pending.raw_text.clone() {
            transcript
        } else {
            let dictionary = state.database.dictionary()?;
            let (preferred_spellings, _) = dictionary_guidance(&dictionary);
            let wav = pending.wav.ok_or_else(|| {
                FlowError::Message("The recoverable recording is incomplete.".into())
            })?;
            let transcript = match settings.transcription_model {
                TranscriptionModel::GroqWhisperLargeV3 => {
                    let api_key = credentials::read_groq_api_key()?;
                    state
                        .groq
                        .transcribe(&api_key, wav, &preferred_spellings)
                        .await?
                }
                TranscriptionModel::DeepgramNova3 => {
                    let api_key = credentials::read_deepgram_api_key()?;
                    state
                        .deepgram
                        .transcribe(&api_key, wav, &preferred_spellings)
                        .await?
                }
            };
            state
                .database
                .save_pending_transcript(pending_id, &transcript)?;
            transcript
        };

        let final_text = if let Some(final_text) = pending.final_text {
            final_text
        } else {
            let api_key = credentials::read_groq_api_key()?;
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
            let text = final_text.clone();
            tauri::async_runtime::spawn_blocking(move || platform::paste_text(paste_target, &text))
                .await
                .map_err(|error| {
                    FlowError::Message(format!("Could not finish pasting: {error}"))
                })??;
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
            state.workflow.finished();
            Ok(())
        }
        Err(error) => {
            let message = friendly_error_ref(&error);
            let _ = state.database.save_pending_error(pending_id, &message);
            state.workflow.failed();
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
    state.workflow.finished();
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
    if state.workflow.phase() != WorkflowPhase::Processing {
        state.workflow.failed();
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

fn show_busy_feedback(app: &AppHandle, phase: WorkflowPhase) {
    let message = match phase {
        WorkflowPhase::Starting => "Flow is starting",
        WorkflowPhase::Processing => "Flow is busy",
        WorkflowPhase::Idle | WorkflowPhase::Recording => return,
    };
    let target = platform::remembered_target().unwrap_or_else(platform::capture_target);
    let _ = platform::prepare_overlay(app, target);
    let _ = app.emit_to(
        "overlay",
        "overlay-notice",
        MessagePayload {
            message: message.into(),
        },
    );
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
            "Flow couldn’t reach an external service. Check your connection and try again.".into()
        }
        FlowError::Windows(message) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::models::DictionaryEntry;

    use super::{
        dictionary_guidance, normalize_with_corrections, HotkeyOutcome, WorkflowPhase,
        WorkflowState,
    };

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

    #[test]
    fn hotkey_state_machine_handles_every_phase() {
        let state = WorkflowState::new();

        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
        assert_eq!(state.phase(), WorkflowPhase::Starting);
        assert_eq!(
            state.hotkey_press(),
            HotkeyOutcome::Busy(WorkflowPhase::Starting)
        );

        assert!(state.recording_started());
        assert_eq!(state.phase(), WorkflowPhase::Recording);
        assert_eq!(state.hotkey_press(), HotkeyOutcome::Stop);
        assert_eq!(state.phase(), WorkflowPhase::Processing);
        assert_eq!(
            state.hotkey_press(),
            HotkeyOutcome::Busy(WorkflowPhase::Processing)
        );

        state.finished();
        assert_eq!(state.phase(), WorkflowPhase::Idle);
    }

    #[test]
    fn failures_return_the_state_machine_to_idle() {
        let state = WorkflowState::new();
        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
        state.failed();
        assert_eq!(state.phase(), WorkflowPhase::Idle);

        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
        assert!(state.recording_started());
        assert_eq!(state.hotkey_press(), HotkeyOutcome::Stop);
        state.failed();

        assert_eq!(state.phase(), WorkflowPhase::Idle);
        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
    }

    #[test]
    fn interrupted_start_cannot_restore_the_recording_phase() {
        let state = WorkflowState::new();

        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
        state.failed();

        assert!(!state.recording_started());
        assert_eq!(state.phase(), WorkflowPhase::Idle);
    }

    #[test]
    fn rapid_presses_are_acknowledged_before_the_recorder_starts() {
        let state = WorkflowState::new();

        assert_eq!(state.hotkey_press(), HotkeyOutcome::Start);
        assert_eq!(
            state.hotkey_press(),
            HotkeyOutcome::Busy(WorkflowPhase::Starting)
        );
    }
}
