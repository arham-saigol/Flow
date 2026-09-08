use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::{
    credentials,
    error::{FlowError, Result},
    models::{
        DictionaryEntry, MessagePayload, OverlayPayload, WorkflowPhase, WorkflowStateSnapshot,
    },
    platform::{self, TargetWindow},
    recovery::{self, Disposition, RecoverySpool},
    text, AppState,
};

pub struct WorkflowCoordinator {
    state: Mutex<WorkflowState>,
    session_counter: AtomicU64,
    revision_counter: AtomicU64,
}

pub struct WorkflowState {
    pub revision: u64,
    pub session_id: Option<u64>,
    pub phase: WorkflowPhase,
    pub active_pending_id: Option<i64>,
    pub destination: Option<TargetWindow>,
    pub cancellation_token: Option<Arc<AtomicBool>>,
    pub capture_uuid: Option<String>,
    pub spool: Option<RecoverySpool>,
    pub message_code: Option<String>,
}

impl Default for WorkflowCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowCoordinator {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(WorkflowState {
                revision: 1,
                session_id: None,
                phase: WorkflowPhase::Idle,
                active_pending_id: None,
                destination: None,
                cancellation_token: None,
                capture_uuid: None,
                spool: None,
                message_code: None,
            }),
            session_counter: AtomicU64::new(1),
            revision_counter: AtomicU64::new(1),
        }
    }

    pub fn snapshot(&self) -> WorkflowStateSnapshot {
        let state = self.state.lock().unwrap();
        self.make_snapshot(&state)
    }

    fn make_snapshot(&self, state: &WorkflowState) -> WorkflowStateSnapshot {
        let can_start = state.phase == WorkflowPhase::Idle;
        let can_stop = state.phase == WorkflowPhase::Recording;
        let can_cancel = matches!(
            state.phase,
            WorkflowPhase::Starting
                | WorkflowPhase::Recording
                | WorkflowPhase::Transcribing
                | WorkflowPhase::Cleaning
                | WorkflowPhase::Delivering
        );

        WorkflowStateSnapshot {
            revision: state.revision,
            session_id: state.session_id,
            phase: state.phase,
            active_pending_id: state.active_pending_id,
            can_start,
            can_stop,
            can_cancel,
            message_code: state.message_code.clone(),
        }
    }

    pub fn next_revision(&self) -> u64 {
        self.revision_counter.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn next_session(&self) -> u64 {
        self.session_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub fn active_pending_id(&self) -> Option<i64> {
        self.state.lock().unwrap().active_pending_id
    }

    pub fn emit_state(&self, app: &AppHandle) {
        let snapshot = self.snapshot();
        let _ = app.emit("workflow-state", &snapshot);
    }
}

pub async fn toggle(app: &AppHandle) {
    let state = app.state::<AppState>();
    let phase = state.workflow.state.lock().unwrap().phase;
    match phase {
        WorkflowPhase::Recording => {
            if let Err(err) = stop_and_process(app).await {
                let _ = app.emit(
                    "flow-error",
                    MessagePayload {
                        message: format!("{err}"),
                    },
                );
            }
        }
        WorkflowPhase::Idle => {
            if let Err(err) = start(app) {
                let _ = app.emit(
                    "flow-error",
                    MessagePayload {
                        message: format!("{err}"),
                    },
                );
            }
        }
        _ => {}
    }
}

pub async fn toggle_from_tray(app: &AppHandle) {
    toggle(app).await;
}

pub fn start(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let (session_id, _capture_uuid, _spool_dir, mic_id) = {
        let mut wf = state.workflow.state.lock().unwrap();
        match wf.phase {
            WorkflowPhase::Starting | WorkflowPhase::Recording => return Ok(()),
            WorkflowPhase::Faulted => {
                return Err(FlowError::Message(
                    "The microphone is not responding. Restart Flow to reconnect it.".into(),
                ));
            }
            WorkflowPhase::Idle => {}
            _ => return Err(FlowError::Busy),
        }

        let settings = state.database.settings(true)?;
        if settings.privacy_notice_version.is_none() {
            return Err(FlowError::Message(
                "Please review and acknowledge the privacy notice before dictating.".into(),
            ));
        }

        if !credentials::has_api_key() {
            return Err(FlowError::MissingApiKey);
        }

        let app_data = app.path().app_data_dir().map_err(|e| {
            FlowError::Message(format!("Could not locate Flow app data directory: {e}"))
        })?;
        let recovery_dir = app_data.join("recovery");

        // Quota check
        state.database.check_recovery_quota(0, 0)?;

        let session_id = state.workflow.next_session();
        let capture_uuid = Uuid::new_v4().to_string();
        let spool = RecoverySpool::new(&recovery_dir, &capture_uuid)?;

        wf.phase = WorkflowPhase::Starting;
        wf.session_id = Some(session_id);
        wf.revision = state.workflow.next_revision();
        wf.capture_uuid = Some(capture_uuid.clone());
        wf.spool = Some(spool);
        wf.cancellation_token = Some(Arc::new(AtomicBool::new(false)));
        wf.message_code = None;

        (
            session_id,
            capture_uuid,
            recovery_dir,
            settings.microphone_id,
        )
    };

    state.workflow.emit_state(app);

    let target = platform::capture_target_with_session(session_id);
    let _ = platform::prepare_overlay(app, target);
    emit_overlay(app, "recording", None);

    let spool_to_start = {
        let mut wf = state.workflow.state.lock().unwrap();
        wf.spool.take()
    };

    let start_res = state
        .recorder
        .start(session_id, app.clone(), &mic_id, target, spool_to_start);
    match start_res {
        Ok(()) => {
            {
                let mut wf = state.workflow.state.lock().unwrap();
                if wf.session_id == Some(session_id) && wf.phase == WorkflowPhase::Starting {
                    wf.phase = WorkflowPhase::Recording;
                    wf.destination = Some(target);
                    wf.revision = state.workflow.next_revision();
                }
            }
            platform::set_recording(true);
            state.workflow.emit_state(app);
            Ok(())
        }
        Err(err) => {
            let abandoned_uuid = {
                let mut wf = state.workflow.state.lock().unwrap();
                if wf.session_id == Some(session_id) {
                    let capture_uuid = wf.capture_uuid.take();
                    wf.phase = WorkflowPhase::Idle;
                    wf.session_id = None;
                    wf.spool = None;
                    wf.cancellation_token = None;
                    wf.revision = state.workflow.next_revision();
                    capture_uuid
                } else {
                    None
                }
            };
            // The queued Start consumed the RecoverySpool, so it is no longer
            // reachable here; mark it cancelled and clean up by capture UUID so
            // abandoned spool files cannot import as recovered dictations.
            if let Some(uuid) = abandoned_uuid {
                discard_abandoned_spool(app, &uuid);
            }
            platform::set_recording(false);
            state.workflow.emit_state(app);
            hide_overlay(app);
            Err(err)
        }
    }
}

pub async fn stop_and_process(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let (session_id, capture_uuid, cancel_token, target) = {
        let mut wf = state.workflow.state.lock().unwrap();
        match wf.phase {
            WorkflowPhase::Stopping
            | WorkflowPhase::Transcribing
            | WorkflowPhase::Cleaning
            | WorkflowPhase::Delivering => return Ok(()),
            WorkflowPhase::Recording => {}
            _ => return Err(FlowError::NotRecording),
        }

        let session_id = wf.session_id.ok_or(FlowError::NotRecording)?;
        let capture_uuid = wf
            .capture_uuid
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let cancel_token = wf
            .cancellation_token
            .clone()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let initial_destination = wf.destination;
        let stop_target = platform::capture_target_with_session(session_id);
        let destination_target = if stop_target.hwnd != 0 && !platform::is_flow_window(stop_target.hwnd) {
            stop_target
        } else if let Some(dest) = initial_destination {
            if dest.hwnd != 0 && !platform::is_flow_window(dest.hwnd) {
                dest
            } else {
                platform::remembered_target().unwrap_or(stop_target)
            }
        } else {
            platform::remembered_target().unwrap_or(stop_target)
        };

        wf.phase = WorkflowPhase::Stopping;
        wf.destination = Some(destination_target);
        wf.revision = state.workflow.next_revision();

        (session_id, capture_uuid, cancel_token, destination_target)
    };

    platform::set_recording(false);
    state.workflow.emit_state(app);

    let stop_res = state.recorder.stop(session_id);
    match stop_res {
        Ok(captured) => {
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                process_captured(
                    &app_clone,
                    session_id,
                    capture_uuid,
                    cancel_token,
                    captured,
                    target,
                )
                .await;
            });
            Ok(())
        }
        Err(err) => {
            {
                let mut wf = state.workflow.state.lock().unwrap();
                if wf.session_id == Some(session_id) {
                    wf.phase = WorkflowPhase::Idle;
                    wf.session_id = None;
                    wf.revision = state.workflow.next_revision();
                }
            }
            state.workflow.emit_state(app);
            hide_overlay(app);
            Err(err)
        }
    }
}

pub(crate) fn process_captured_in_background(
    app: &AppHandle,
    captured: crate::audio::CapturedAudio,
) {
    let state = app.state::<AppState>();
    let (session_id, capture_uuid, cancel_token, destination) = {
        let wf = state.workflow.state.lock().unwrap();
        (
            wf.session_id.unwrap_or(0),
            wf.capture_uuid
                .clone()
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            wf.cancellation_token
                .clone()
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false))),
            wf.destination,
        )
    };
    let app_clone = app.clone();
    let bg_target = platform::capture_target_with_session(session_id);
    let target = if bg_target.hwnd != 0 && !platform::is_flow_window(bg_target.hwnd) {
        bg_target
    } else if let Some(dest) = destination {
        if dest.hwnd != 0 && !platform::is_flow_window(dest.hwnd) {
            dest
        } else {
            platform::remembered_target().unwrap_or(bg_target)
        }
    } else {
        platform::remembered_target().unwrap_or(bg_target)
    };
    tauri::async_runtime::spawn(async move {
        process_captured(
            &app_clone,
            session_id,
            capture_uuid,
            cancel_token,
            captured,
            target,
        )
        .await;
    });
}

async fn process_captured(
    app: &AppHandle,
    session_id: u64,
    capture_uuid: String,
    cancel_token: Arc<AtomicBool>,
    captured: crate::audio::CapturedAudio,
    target: TargetWindow,
) {
    if cancel_token.load(Ordering::Acquire) {
        return;
    }

    let state = app.state::<AppState>();

    // Task 7: Stop limit reached -> save as copy_only pending item for review, do not call Groq or paste
    if captured.limit_reached {
        if let Err(e) = state.database.insert_pending_recording(
            &capture_uuid,
            &captured.wav,
            captured.duration_ms,
            true,
            Some("limit_reached"),
            "copy_only",
        ) {
            // Inform the user before the workflow transitions to Idle and the
            // overlay hides; the capture would otherwise be lost silently.
            report_error(app, e, Some(session_id));
            return;
        }
        {
            let mut wf = state.workflow.state.lock().unwrap();
            if wf.session_id == Some(session_id) {
                wf.phase = WorkflowPhase::Idle;
                wf.active_pending_id = None;
                wf.session_id = None;
                wf.cancellation_token = None;
                wf.revision = state.workflow.next_revision();
            }
        }
        platform::set_recording(false);
        state.workflow.emit_state(app);
        hide_overlay(app);
        return;
    }

    let delivery_mode = if captured.partial {
        "copy_only"
    } else {
        "automatic"
    };

    let review_reason = if captured.partial {
        Some("partial_capture")
    } else {
        None
    };

    let insert_res = state.database.insert_pending_recording(
        &capture_uuid,
        &captured.wav,
        captured.duration_ms,
        captured.partial,
        review_reason,
        delivery_mode,
    );

    let pending_id = match insert_res {
        Ok(id) => id,
        Err(e) => {
            report_error(app, e, Some(session_id));
            return;
        }
    };

    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.active_pending_id = Some(pending_id);
            wf.phase = WorkflowPhase::Transcribing;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    emit_overlay(app, "analysing", Some("Analyzing"));

    if let Err(e) = run_pending(app, session_id, pending_id, Some(target), delivery_mode).await {
        report_error(app, e, Some(session_id));
    }
}

async fn run_pending(
    app: &AppHandle,
    session_id: u64,
    pending_id: i64,
    target: Option<TargetWindow>,
    initial_delivery_mode: &str,
) -> Result<()> {
    let state = app.state::<AppState>();
    let cancel_token = {
        let wf = state.workflow.state.lock().unwrap();
        wf.cancellation_token.clone()
    };

    let check_cancelled = || -> Result<()> {
        if let Some(token) = &cancel_token {
            if token.load(Ordering::Acquire) {
                return Err(FlowError::Message("Processing was cancelled.".into()));
            }
        }
        Ok(())
    };

    check_cancelled()?;

    let pending = state.database.pending_dictation(pending_id)?;
    let settings = state.database.settings(true)?;

    // 1. Transcription stage
    let (transcript, is_suspect) = if let Some(t) = pending.raw_text {
        (t, false)
    } else {
        let api_key = credentials::read_api_key()?;
        let wav = pending
            .wav
            .ok_or_else(|| FlowError::Message("The recoverable recording is incomplete.".into()))?;

        let dict_entries = state.database.dictionary()?;
        let trans_res = state.groq.transcribe(&api_key, wav, &dict_entries).await?;

        check_cancelled()?;

        let review_reason = if trans_res.is_suspect {
            Some("suspect_speech")
        } else {
            None
        };

        // Apply initial deterministic corrections to prepare corrected_text
        let (_, valid_rules) = get_dictionary_rules(&dict_entries);
        let corrected = text::apply_corrections(&trans_res.text, &valid_rules);

        state.database.save_pending_transcript(
            pending_id,
            &trans_res.text,
            &corrected,
            review_reason,
        )?;

        (trans_res.text, trans_res.is_suspect)
    };

    if is_suspect {
        // Suspect speech requires user acceptance. Stop here before cleanup/delivery.
        {
            let mut wf = state.workflow.state.lock().unwrap();
            if wf.session_id == Some(session_id) {
                wf.phase = WorkflowPhase::Idle;
                wf.active_pending_id = None;
                wf.session_id = None;
                wf.revision = state.workflow.next_revision();
            }
        }
        state.workflow.emit_state(app);
        hide_overlay(app);
        let _ = app.emit(
            "flow-warning",
            MessagePayload {
                message: "Dictation contains uncertain speech and was saved for your review."
                    .into(),
            },
        );
        return Ok(());
    }

    check_cancelled()?;

    // 2. Cleanup & Snippet stage
    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Cleaning;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    emit_overlay(app, "thinking", Some("Thinking"));

    let dict_entries = state.database.dictionary()?;
    let (_, valid_rules) = get_dictionary_rules(&dict_entries);
    let corrected = text::apply_corrections(&transcript, &valid_rules);

    let final_text = if let Some(f) = pending.final_text {
        f
    } else {
        // Check snippet trigger
        let snippets = state.database.snippets()?;
        let matched_snippet = snippets.iter().find(|s| {
            if !s.enabled {
                return false;
            }
            if let (Some(s_norm), Some(u_norm)) = (
                text::normalize_snippet_trigger(&s.trigger),
                text::normalize_snippet_trigger(&corrected),
            ) {
                s_norm == u_norm
            } else {
                false
            }
        });

        let text_result = if let Some(snippet) = matched_snippet {
            snippet.content.clone()
        } else {
            let api_key = credentials::read_api_key()?;
            state.groq.clean(&api_key, &transcript, &corrected).await?
        };

        check_cancelled()?;

        let no_content = text_result.is_empty();
        state
            .database
            .save_pending_final(pending_id, &text_result, no_content)?;
        text_result
    };

    if final_text.is_empty() {
        // Pure hesitation filler words
        {
            let mut wf = state.workflow.state.lock().unwrap();
            if wf.session_id == Some(session_id) {
                wf.phase = WorkflowPhase::Idle;
                wf.active_pending_id = None;
                wf.session_id = None;
                wf.revision = state.workflow.next_revision();
            }
        }
        state.workflow.emit_state(app);
        hide_overlay(app);
        return Ok(());
    }

    check_cancelled()?;

    // 3. Delivery stage
    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Delivering;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);

    state
        .database
        .save_pending_to_history(pending_id, &settings.history_retention)?;

    let delivery_outcome = if initial_delivery_mode == "automatic" {
        if let Some(t) = target {
            if platform::is_flow_window(t.hwnd) {
                state
                    .database
                    .update_pending_delivery(pending_id, "not_attempted", None)?;
                "Dictation ready for review"
            } else {
                let outcome = platform::paste_text(t, &final_text);
                match outcome {
                    Ok("pasted") => {
                        state
                            .database
                            .update_pending_delivery(pending_id, "shortcut_sent", None)?;
                        let _ = state.database.delete_pending(pending_id);
                        "Dictation sent to the selected application"
                    }
                    Err(err) => {
                        let msg = format!("Paste delivery failed: {err}");
                        crate::diagnostics::log_event("error", Some(session_id), None, &msg);
                        let warning = format!("Delivery failed: {err}. Dictation saved for review.");
                        state.database.update_pending_delivery(
                            pending_id,
                            "not_attempted",
                            Some(&warning),
                        )?;
                        "Destination was unavailable. Dictation saved for review."
                    }
                    _ => {
                        state.database.update_pending_delivery(
                            pending_id,
                            "not_attempted",
                            Some("Destination was unavailable. Dictation saved for review."),
                        )?;
                        "Destination was unavailable. Dictation saved for review."
                    }
                }
            }
        } else {
            state
                .database
                .update_pending_delivery(pending_id, "not_attempted", None)?;
            "Dictation ready for review"
        }
    } else {
        state
            .database
            .update_pending_delivery(pending_id, "not_attempted", None)?;
        "Dictation ready for review"
    };

    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Idle;
            wf.active_pending_id = None;
            wf.session_id = None;
            wf.cancellation_token = None;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    hide_overlay(app);

    let _ = app.emit(
        "dictation-complete",
        MessagePayload {
            message: delivery_outcome.into(),
        },
    );

    Ok(())
}

pub async fn retry_pending(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let session_id = {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.phase != WorkflowPhase::Idle {
            return Err(FlowError::Busy);
        }
        if wf.active_pending_id == Some(id) {
            return Err(FlowError::Busy);
        }
        let session_id = state.workflow.next_session();
        wf.phase = WorkflowPhase::Transcribing;
        wf.session_id = Some(session_id);
        wf.active_pending_id = Some(id);
        wf.cancellation_token = Some(Arc::new(AtomicBool::new(false)));
        wf.revision = state.workflow.next_revision();
        session_id
    };

    state.workflow.emit_state(app);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_pending(&app_clone, session_id, id, None, "copy_only").await {
            report_error(&app_clone, e, Some(session_id));
        }
    });
    Ok(())
}

pub async fn retry_pending_transcription(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let pending = state.database.pending_dictation(id)?;

    let wav = pending.wav.ok_or_else(|| {
        FlowError::Message("No audio recording is available for retranscription.".into())
    })?;
    if wav.is_empty() {
        return Err(FlowError::Message(
            "No audio recording is available for retranscription.".into(),
        ));
    }
    if pending.history_saved {
        return Err(FlowError::Message(
            "This dictation has already been saved to history.".into(),
        ));
    }

    let session_id = {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.phase != WorkflowPhase::Idle {
            return Err(FlowError::Busy);
        }
        if wf.active_pending_id == Some(id) {
            return Err(FlowError::Busy);
        }
        let session_id = state.workflow.next_session();
        wf.phase = WorkflowPhase::Transcribing;
        wf.session_id = Some(session_id);
        wf.active_pending_id = Some(id);
        wf.cancellation_token = Some(Arc::new(AtomicBool::new(false)));
        wf.revision = state.workflow.next_revision();
        session_id
    };

    state.workflow.emit_state(app);
    emit_overlay(app, "analysing", Some("Transcribing"));

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_retranscribe(&app_clone, session_id, id, wav).await {
            report_error(&app_clone, e, Some(session_id));
        }
    });

    Ok(())
}

async fn run_retranscribe(
    app: &AppHandle,
    session_id: u64,
    pending_id: i64,
    wav: Vec<u8>,
) -> Result<()> {
    let state = app.state::<AppState>();
    let cancel_token = {
        let wf = state.workflow.state.lock().unwrap();
        wf.cancellation_token.clone()
    };

    let check_cancelled = || -> Result<()> {
        if let Some(token) = &cancel_token {
            if token.load(Ordering::Acquire) {
                return Err(FlowError::Message("Processing was cancelled.".into()));
            }
        }
        Ok(())
    };

    check_cancelled()?;

    let api_key = credentials::read_api_key()?;
    let dict_entries = state.database.dictionary()?;
    let trans_res = state.groq.transcribe(&api_key, wav, &dict_entries).await?;

    check_cancelled()?;

    let review_reason = if trans_res.is_suspect {
        Some("suspect_speech")
    } else {
        None
    };

    let (_spellings, valid_rules) = get_dictionary_rules(&dict_entries);
    let corrected = text::apply_corrections(&trans_res.text, &valid_rules);

    state.database.save_pending_transcript(
        pending_id,
        &trans_res.text,
        &corrected,
        review_reason,
    )?;

    if trans_res.is_suspect {
        {
            let mut wf = state.workflow.state.lock().unwrap();
            if wf.session_id == Some(session_id) {
                wf.phase = WorkflowPhase::Idle;
                wf.active_pending_id = None;
                wf.session_id = None;
                wf.revision = state.workflow.next_revision();
            }
        }
        state.workflow.emit_state(app);
        hide_overlay(app);
        let _ = app.emit(
            "flow-warning",
            MessagePayload {
                message: "Dictation contains uncertain speech and was saved for your review."
                    .into(),
            },
        );
        return Ok(());
    }

    check_cancelled()?;

    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Cleaning;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    emit_overlay(app, "thinking", Some("Thinking"));

    let cleaned = state
        .groq
        .clean(&api_key, &trans_res.text, &corrected)
        .await?;
    check_cancelled()?;

    state
        .database
        .save_pending_final(pending_id, &cleaned, cleaned.trim().is_empty())?;

    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Idle;
            wf.active_pending_id = None;
            wf.session_id = None;
            wf.cancellation_token = None;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    hide_overlay(app);

    let _ = app.emit(
        "dictation-complete",
        MessagePayload {
            message: "Retranscription complete. Ready for review.".into(),
        },
    );

    Ok(())
}

pub fn start_mic_test(app: &AppHandle, device_id: &str) -> Result<()> {
    let state = app.state::<AppState>();
    let session_id = {
        let mut wf = state.workflow.state.lock().unwrap();
        match wf.phase {
            WorkflowPhase::Idle => {}
            WorkflowPhase::Faulted => {
                return Err(FlowError::Message(
                    "The microphone is not responding. Restart Flow to reconnect it.".into(),
                ));
            }
            _ => return Err(FlowError::Busy),
        }
        let sid = state.workflow.next_session();
        wf.phase = WorkflowPhase::MicrophoneTest;
        wf.session_id = Some(sid);
        wf.revision = state.workflow.next_revision();
        sid
    };
    state.workflow.emit_state(app);

    let res = state
        .recorder
        .start_mic_test(session_id, app.clone(), device_id);
    if let Err(e) = res {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.session_id == Some(session_id) && wf.phase == WorkflowPhase::MicrophoneTest {
            wf.phase = WorkflowPhase::Idle;
            wf.session_id = None;
            wf.revision = state.workflow.next_revision();
        }
        state.workflow.emit_state(app);
        return Err(e);
    }

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let st = app_clone.state::<AppState>();
        let is_same = {
            let wf = st.workflow.state.lock().unwrap();
            wf.phase == WorkflowPhase::MicrophoneTest && wf.session_id == Some(session_id)
        };
        if is_same {
            let _ = stop_mic_test(&app_clone);
        }
    });

    Ok(())
}

pub fn stop_mic_test(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let session_id = {
        let wf = state.workflow.state.lock().unwrap();
        if wf.phase != WorkflowPhase::MicrophoneTest {
            return Ok(());
        }
        wf.session_id.ok_or(FlowError::NotRecording)?
    };

    let res = state.recorder.stop_mic_test(session_id);
    {
        let mut wf = state.workflow.state.lock().unwrap();
        if wf.phase == WorkflowPhase::MicrophoneTest && wf.session_id == Some(session_id) {
            wf.phase = WorkflowPhase::Idle;
            wf.session_id = None;
            wf.revision = state.workflow.next_revision();
        }
    }
    state.workflow.emit_state(app);
    res
}

/// Marks a no-longer-reachable capture spool as cancelled and removes its
/// files. Used when the queued recorder Start consumed the RecoverySpool and
/// the session then failed or was cancelled: the workflow state no longer owns
/// the spool, so cleanup happens by capture UUID (marker first, data files
/// next, marker removed only after the files are handled).
fn discard_abandoned_spool(app: &AppHandle, capture_uuid: &str) {
    let Ok(app_data) = app.path().app_data_dir() else {
        return;
    };
    let recovery_dir = app_data.join("recovery");
    let marker_path = recovery_dir.join(format!("{capture_uuid}.terminal.json"));
    let pcm_path = recovery_dir.join(format!("{capture_uuid}.pcm.part"));
    let meta_path = recovery_dir.join(format!("{capture_uuid}.json"));

    let marker = recovery::TerminalMarker {
        schema_version: 1,
        capture_uuid: capture_uuid.to_string(),
        created_at: recovery::now_secs(),
        disposition: Disposition::Cancelled,
    };
    if let Ok(marker_json) = serde_json::to_string_pretty(&marker) {
        let _ = recovery::write_sync_rename(&marker_path, marker_json.as_bytes());
    }
    let pcm_gone = std::fs::remove_file(&pcm_path).is_ok() || !pcm_path.exists();
    let meta_gone = std::fs::remove_file(&meta_path).is_ok() || !meta_path.exists();
    if pcm_gone && meta_gone {
        let _ = std::fs::remove_file(&marker_path);
    }
}

pub fn cancel(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let (phase, session_id, spool, capture_uuid, _cancel_token) = {
        let mut wf = state.workflow.state.lock().unwrap();
        let phase = wf.phase;
        let session_id = wf.session_id;
        let spool = wf.spool.take();
        let cancel_token = wf.cancellation_token.clone();
        let mut capture_uuid = None;

        match phase {
            WorkflowPhase::Starting | WorkflowPhase::Recording => {
                capture_uuid = wf.capture_uuid.take();
                wf.phase = WorkflowPhase::Idle;
                wf.session_id = None;
                wf.active_pending_id = None;
                wf.cancellation_token = None;
                wf.revision = state.workflow.next_revision();
            }
            WorkflowPhase::Transcribing | WorkflowPhase::Cleaning | WorkflowPhase::Delivering => {
                if let Some(token) = &cancel_token {
                    token.store(true, Ordering::Release);
                }
                wf.phase = WorkflowPhase::Idle;
                wf.active_pending_id = None;
                wf.session_id = None;
                wf.revision = state.workflow.next_revision();
            }
            _ => return Ok(()),
        }

        (phase, session_id, spool, capture_uuid, cancel_token)
    };

    platform::set_recording(false);
    state.workflow.emit_state(app);
    hide_overlay(app);

    if matches!(phase, WorkflowPhase::Starting | WorkflowPhase::Recording) {
        if let Some(sid) = session_id {
            let _ = state.recorder.cancel(sid);
        }
        if let Some(mut s) = spool {
            let _ = s.mark_terminal(Disposition::Cancelled);
            let _ = s.cleanup();
        } else if let Some(uuid) = capture_uuid {
            // The queued Start consumed the spool, so it is owned by the audio
            // worker; tear it down by capture UUID after recorder.cancel has
            // confirmed the worker can no longer write the files.
            discard_abandoned_spool(app, &uuid);
        }
    }

    Ok(())
}

pub fn discard_pending(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    let pending = state.database.pending_dictation(id)?;

    if let Some(uuid) = &pending.capture_uuid {
        if let Ok(app_data) = app.path().app_data_dir() {
            let recovery_dir = app_data.join("recovery");
            let marker_path = recovery_dir.join(format!("{uuid}.terminal.json"));
            let pcm_path = recovery_dir.join(format!("{uuid}.pcm.part"));
            let meta_path = recovery_dir.join(format!("{uuid}.json"));

            // Before deleting the pending row, durably mark the spool as
            // discarded so a crash cannot resurrect the audio as a new capture.
            // If the marker cannot be persisted, keep the pending row.
            let marker = recovery::TerminalMarker {
                schema_version: 1,
                capture_uuid: uuid.clone(),
                created_at: recovery::now_secs(),
                disposition: Disposition::Discarded,
            };
            let marker_json = serde_json::to_string_pretty(&marker).map_err(|e| {
                FlowError::Message(format!("Could not serialize the discard marker: {e}"))
            })?;
            recovery::write_sync_rename(&marker_path, marker_json.as_bytes())?;

            // Reserve and delete the pending item while holding the workflow
            // claim so retry_pending or run_pending cannot access the same ID
            // concurrently.
            {
                let wf = state.workflow.state.lock().unwrap();
                if wf.active_pending_id == Some(id) {
                    return Err(FlowError::Message(
                        "Cannot discard an active dictation.".into(),
                    ));
                }
                state.database.delete_pending(id)?;
            }

            // Remove the data files, then the terminal marker only after the
            // related spool files are handled; otherwise the marker stays and
            // startup maintenance retries the cleanup instead of importing.
            let pcm_gone = std::fs::remove_file(&pcm_path).is_ok() || !pcm_path.exists();
            let meta_gone = std::fs::remove_file(&meta_path).is_ok() || !meta_path.exists();
            if pcm_gone && meta_gone {
                let _ = std::fs::remove_file(&marker_path);
            }
            return Ok(());
        }
    }

    let wf = state.workflow.state.lock().unwrap();
    if wf.active_pending_id == Some(id) {
        return Err(FlowError::Message(
            "Cannot discard an active dictation.".into(),
        ));
    }
    state.database.delete_pending(id)?;
    Ok(())
}

pub fn accept_pending_transcript(app: &AppHandle, id: i64) -> Result<()> {
    let state = app.state::<AppState>();
    state.database.accept_pending_transcript(id)?;
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        // Surface retry failures (including Busy) to the user instead of
        // silently dropping the accepted transcript.
        if let Err(e) = retry_pending(&app_clone, id).await {
            report_error(&app_clone, e, None);
        }
    });
    Ok(())
}

pub fn report_error(app: &AppHandle, error: FlowError, session_id: Option<u64>) {
    let state = app.state::<AppState>();
    let owns_session = {
        let mut wf = state.workflow.state.lock().unwrap();
        let owns = match (session_id, wf.session_id) {
            (Some(sid), Some(current)) => sid == current,
            _ => false,
        };
        if owns {
            wf.phase = WorkflowPhase::Idle;
            wf.active_pending_id = None;
            wf.session_id = None;
            wf.revision = state.workflow.next_revision();
        }
        owns
    };

    let msg = error.to_string();
    crate::diagnostics::log_event("error", session_id, None, &msg);
    let _ = app.emit("flow-error", MessagePayload { message: msg });

    if !owns_session {
        // A stale task must not overwrite a newer recording session's state.
        return;
    }
    platform::set_recording(false);
    state.workflow.emit_state(app);
    hide_overlay(app);
}

pub fn report_audio_fault(app: &AppHandle, error: FlowError) {
    let state = app.state::<AppState>();
    let session_id = {
        let mut wf = state.workflow.state.lock().unwrap();
        let sid = wf.session_id;
        wf.phase = WorkflowPhase::Faulted;
        wf.active_pending_id = None;
        wf.session_id = None;
        wf.revision = state.workflow.next_revision();
        sid
    };

    platform::set_recording(false);
    state.workflow.emit_state(app);
    hide_overlay(app);

    let msg = error.to_string();
    crate::diagnostics::log_event("fault", session_id, None, &msg);
    let _ = app.emit("flow-error", MessagePayload { message: msg });
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

fn get_dictionary_rules(
    entries: &[DictionaryEntry],
) -> (Vec<String>, Vec<text::ValidatedCorrection>) {
    let spellings = entries
        .iter()
        .filter(|e| e.enabled)
        .map(|e| e.correction.as_deref().unwrap_or(&e.value).to_string())
        .collect();

    let raw_corrections: Vec<(String, String)> = entries
        .iter()
        .filter(|e| e.enabled)
        .filter_map(|e| {
            e.correction
                .as_ref()
                .map(|corr| (e.value.clone(), corr.clone()))
        })
        .collect();

    let (valid, _) = text::filter_conflicting_corrections(&raw_corrections);
    (spellings, valid)
}
