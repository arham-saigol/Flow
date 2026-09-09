use reqwest::{multipart, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{
    error::{FlowError, Result},
    models::{DictionaryEntry, CLEANUP_FALLBACK_MODEL, CLEANUP_MODEL, TRANSCRIPTION_MODEL},
};
use unicode_categories::UnicodeCategories;

const API_BASE: &str = "https://api.groq.com/openai/v1";
const SYSTEM_PROMPT: &str = include_str!("../prompts/dictation_cleanup.txt");
const MAX_GUIDANCE_BYTES: usize = 200;
const MAX_TRANSCRIPT_BYTES: usize = 32_000;
const MAX_BODY_BYTES: usize = 1_048_576; // 1 MiB

async fn read_bounded_body(mut resp: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(FlowError::Network)? {
        if bytes.len() + chunk.len() > max_bytes {
            return Err(FlowError::Message("Response exceeded 1 MB limit.".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Debug, Deserialize)]
pub struct VerboseTranscriptionResponse {
    pub text: String,
    #[serde(default)]
    pub segments: Vec<TranscriptionSegment>,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptionSegment {
    #[allow(dead_code)]
    pub text: Option<String>,
    pub no_speech_prob: Option<f64>,
    pub avg_logprob: Option<f64>,
    pub compression_ratio: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
    #[serde(default)]
    refusal: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct UserMessagePayload<'a> {
    raw_transcript: &'a str,
    corrected_transcript: &'a str,
}

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub is_suspect: bool,
}

#[derive(Clone)]
pub struct GroqClient {
    client: Client,
    base_url: String,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        Self::with_base_url(API_BASE)
    }

    pub fn with_base_url(base_url: &str) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(90))
            .tcp_nodelay(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    pub async fn test_key(&self, api_key: &str) -> Result<()> {
        let trimmed_key = api_key.trim();
        if trimmed_key.is_empty() {
            return Err(FlowError::MissingApiKey);
        }

        let response = self
            .client
            .get(format!("{}/models", self.base_url))
            .bearer_auth(trimmed_key)
            .timeout(Duration::from_secs(10))
            .send()
            .await?;

        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(FlowError::Message("Invalid Groq API key.".into()));
        }
        if !status.is_success() {
            return Err(FlowError::Message(format!(
                "Groq API returned an error: status {status}"
            )));
        }

        #[derive(Deserialize)]
        struct ModelsList {
            data: Vec<ModelItem>,
        }
        #[derive(Deserialize)]
        struct ModelItem {
            id: String,
        }

        let models = response
            .json::<ModelsList>()
            .await
            .map_err(|_| FlowError::Message("Could not parse Groq models list.".into()))?;

        let has_whisper = models.data.iter().any(|m| m.id == TRANSCRIPTION_MODEL);
        let has_qwen = models.data.iter().any(|m| m.id == CLEANUP_MODEL);
        // The cleanup stage falls back to this model when the primary
        // cleanup model is rate limited or the service errors, so the key
        // check must cover it too.
        let has_fallback = models.data.iter().any(|m| m.id == CLEANUP_FALLBACK_MODEL);

        if !has_whisper || !has_qwen || !has_fallback {
            return Err(FlowError::Message(format!(
                "Your Groq account does not have access to all required models ({TRANSCRIPTION_MODEL}, {CLEANUP_MODEL}, and {CLEANUP_FALLBACK_MODEL})."
            )));
        }

        Ok(())
    }

    pub async fn transcribe(
        &self,
        api_key: &str,
        wav: Vec<u8>,
        dictionary_entries: &[DictionaryEntry],
    ) -> Result<TranscriptionResult> {
        let prompt = build_whisper_guidance(dictionary_entries);
        let stage_deadline = tokio::time::Instant::now() + Duration::from_secs(90);

        let mut attempts = 0;
        loop {
            if tokio::time::Instant::now() >= stage_deadline {
                return Err(FlowError::Message(
                    "Transcription exceeded 90-second stage deadline.".into(),
                ));
            }
            attempts += 1;
            // Bound each attempt by the remaining stage budget (see
            // clean_with_model) so the stage deadline cannot be exceeded.
            let remaining = stage_deadline.saturating_duration_since(tokio::time::Instant::now());
            let file_part = multipart::Part::bytes(wav.clone())
                .file_name("dictation.wav")
                .mime_str("audio/wav")
                .map_err(|e| FlowError::Message(format!("Could not prepare recording: {e}")))?;

            let mut form = multipart::Form::new()
                .part("file", file_part)
                .text("model", TRANSCRIPTION_MODEL)
                .text("temperature", "0")
                .text("response_format", "verbose_json");

            if !prompt.is_empty() {
                form = form.text("prompt", prompt.clone());
            }

            let send_res = self
                .client
                .post(format!("{}/audio/transcriptions", self.base_url))
                .bearer_auth(api_key.trim())
                .multipart(form)
                .timeout(remaining)
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let bytes = read_bounded_body(resp, MAX_BODY_BYTES).await?;
                        let verbose =
                            serde_json::from_slice::<VerboseTranscriptionResponse>(&bytes)
                                .map_err(|_| {
                                    FlowError::Message(
                                        "Invalid JSON from Whisper transcription.".into(),
                                    )
                                })?;

                        let trimmed = verbose.text.trim().to_string();
                        if trimmed.is_empty() {
                            return Err(FlowError::EmptyRecording);
                        }

                        // Check segments for suspect speech
                        // If segments are missing or nonfinite, must flag as suspect
                        let mut is_suspect = verbose.segments.is_empty();
                        for seg in &verbose.segments {
                            let no_speech = match seg.no_speech_prob {
                                Some(p) if p.is_finite() => p,
                                _ => {
                                    is_suspect = true;
                                    break;
                                }
                            };
                            let avg_logprob = match seg.avg_logprob {
                                Some(p) if p.is_finite() => p,
                                _ => {
                                    is_suspect = true;
                                    break;
                                }
                            };
                            let comp_ratio = seg.compression_ratio.unwrap_or(1.0);

                            if (no_speech >= 0.6 && avg_logprob <= -1.0) || comp_ratio > 2.4 {
                                is_suspect = true;
                                break;
                            }
                        }

                        // Preserve original Whisper text (trimmed view was used for validation)
                        return Ok(TranscriptionResult {
                            text: verbose.text,
                            is_suspect,
                        });
                    }

                    if is_retryable(status) && attempts < 3 {
                        if let Some(delay) = get_retry_delay(&resp, attempts) {
                            if tokio::time::Instant::now() + delay < stage_deadline {
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
                    }

                    let detail = read_error_detail(resp).await;
                    return Err(map_status_error(status, detail.as_deref()));
                }
                Err(e) => {
                    if attempts < 3 && e.is_connect() {
                        let delay = Duration::from_millis(1000 * attempts as u64);
                        if tokio::time::Instant::now() + delay < stage_deadline {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(FlowError::Network(e));
                }
            }
        }
    }

    /// Cleans a transcript with the primary cleanup model, falling back to
    /// CLEANUP_FALLBACK_MODEL when the primary hits a rate limit or the Groq
    /// service returns server errors.
    pub async fn clean(
        &self,
        api_key: &str,
        raw_transcript: &str,
        corrected_transcript: &str,
    ) -> Result<String> {
        let bounded_raw = if raw_transcript.len() > MAX_TRANSCRIPT_BYTES {
            return Err(FlowError::Message(
                "Transcript exceeds 32,000 bytes limit.".into(),
            ));
        } else {
            raw_transcript
        };

        let bounded_corrected = if corrected_transcript.len() > MAX_TRANSCRIPT_BYTES {
            return Err(FlowError::Message(
                "Corrected transcript exceeds 32,000 bytes limit.".into(),
            ));
        } else {
            corrected_transcript
        };

        let user_content = serde_json::to_string(&UserMessagePayload {
            raw_transcript: bounded_raw,
            corrected_transcript: bounded_corrected,
        })
        .map_err(|e| FlowError::Message(format!("Could not serialize cleanup input: {e}")))?;
        let filler_only = is_filler_only(raw_transcript) && is_filler_only(corrected_transcript);

        let stage_deadline = tokio::time::Instant::now() + Duration::from_secs(90);

        match self
            .clean_with_model(
                api_key,
                CLEANUP_MODEL,
                &user_content,
                filler_only,
                stage_deadline,
            )
            .await
        {
            Ok(text) => Ok(text),
            Err(CleanAttemptError::Fatal(error)) => Err(error),
            Err(CleanAttemptError::Provider(status, detail)) => {
                let primary_error = map_status_error(status, detail.as_deref());
                // No time left for the fallback: report the primary failure.
                // A request needs meaningful time; starting one with less
                // than a second of stage budget would run past the
                // 90-second stage deadline even with the per-attempt timeout.
                if tokio::time::Instant::now() + Duration::from_secs(1) >= stage_deadline {
                    return Err(primary_error);
                }
                match self
                    .clean_with_model(
                        api_key,
                        CLEANUP_FALLBACK_MODEL,
                        &user_content,
                        filler_only,
                        stage_deadline,
                    )
                    .await
                {
                    Ok(text) => Ok(text),
                    Err(CleanAttemptError::Fatal(error)) => Err(error),
                    Err(CleanAttemptError::Provider(fallback_status, fallback_detail)) => {
                        Err(FlowError::Message(format!(
                            "{primary_error} The fallback model also failed: {}",
                            map_status_error(fallback_status, fallback_detail.as_deref())
                        )))
                    }
                }
            }
        }
    }

    async fn clean_with_model(
        &self,
        api_key: &str,
        model: &str,
        user_content: &str,
        filler_only: bool,
        stage_deadline: tokio::time::Instant,
    ) -> std::result::Result<String, CleanAttemptError> {
        let request_body = json!({
            "model": model,
            "temperature": 0.1,
            "reasoning_effort": "low",
            "reasoning_format": "hidden",
            "max_completion_tokens": 16384,
            "stream": false,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user_content }
            ]
        });

        let mut attempts = 0;
        loop {
            if tokio::time::Instant::now() >= stage_deadline {
                return Err(CleanAttemptError::Fatal(FlowError::Message(
                    "Cleanup exceeded 90-second stage deadline.".into(),
                )));
            }
            attempts += 1;
            // Bound each attempt by the remaining stage budget so a request
            // started near the deadline cannot run the client's full
            // 90-second timeout past the stage deadline.
            let remaining = stage_deadline.saturating_duration_since(tokio::time::Instant::now());
            let send_res = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(api_key.trim())
                .timeout(remaining)
                .json(&request_body)
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let bytes = read_bounded_body(resp, MAX_BODY_BYTES)
                            .await
                            .map_err(CleanAttemptError::Fatal)?;
                        let chat: ChatResponse = serde_json::from_slice(&bytes).map_err(|_| {
                            CleanAttemptError::Fatal(FlowError::Message(
                                "Invalid JSON from cleanup model.".into(),
                            ))
                        })?;

                        if chat.choices.len() != 1 {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup model returned unexpected number of choices.".into(),
                            )));
                        }

                        let choice = &chat.choices[0];
                        if choice.message.refusal.is_some() || choice.message.tool_calls.is_some() {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup request was refused or contained tool calls.".into(),
                            )));
                        }

                        if choice.finish_reason.as_deref() != Some("stop") {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(format!(
                                "Cleanup completion was truncated or abnormal (finish_reason: {:?}).",
                                choice.finish_reason
                            ))));
                        }

                        let Some(ref content) = choice.message.content else {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup model returned null content.".into(),
                            )));
                        };

                        if content.len() > MAX_TRANSCRIPT_BYTES {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup response exceeded 32,000 bytes limit.".into(),
                            )));
                        }

                        // Check for disallowed control characters: C0 (except \t, \n, \r) and \x7f
                        if content.chars().any(|c| {
                            (c < ' ' && c != '\t' && c != '\n' && c != '\r') || c == '\x7f'
                        }) {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup response contains disallowed control characters.".into(),
                            )));
                        }

                        // Check for leaked reasoning block
                        if content.starts_with("<think>") || content.contains("</think>") {
                            return Err(CleanAttemptError::Fatal(FlowError::Message(
                                "Cleanup response contains leaked reasoning block.".into(),
                            )));
                        }

                        // Validate empty output
                        let trimmed = content.trim();
                        if trimmed.is_empty() {
                            if filler_only {
                                return Ok(String::new());
                            } else {
                                return Err(CleanAttemptError::Fatal(FlowError::Message(
                                    "Model returned empty text for substantive dictation.".into(),
                                )));
                            }
                        }

                        return Ok(trimmed.to_string());
                    }

                    if is_retryable(status) && attempts < 3 {
                        if let Some(delay) = get_retry_delay(&resp, attempts) {
                            if tokio::time::Instant::now() + delay < stage_deadline {
                                tokio::time::sleep(delay).await;
                                continue;
                            }
                        }
                    }

                    let detail = read_error_detail(resp).await;
                    return Err(clean_attempt_failure(status, detail));
                }
                Err(e) => {
                    if attempts < 3 && e.is_connect() {
                        let delay = Duration::from_millis(1000 * attempts as u64);
                        if tokio::time::Instant::now() + delay < stage_deadline {
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                    return Err(CleanAttemptError::Fatal(FlowError::Network(e)));
                }
            }
        }
    }
}

/// Why a cleanup attempt on a single model stopped. Provider failures (rate
/// limits, server errors) are retried with the fallback model; everything
/// else is surfaced directly.
enum CleanAttemptError {
    Provider(StatusCode, Option<String>),
    Fatal(FlowError),
}

fn clean_attempt_failure(status: StatusCode, detail: Option<String>) -> CleanAttemptError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        CleanAttemptError::Provider(status, detail)
    } else {
        CleanAttemptError::Fatal(map_status_error(status, detail.as_deref()))
    }
}

fn parse_error_detail(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        #[serde(default)]
        error: Option<ErrorDetail>,
    }
    #[derive(Deserialize)]
    struct ErrorDetail {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        request_id: Option<String>,
    }

    let parsed = serde_json::from_str::<ErrorBody>(body).ok()?;
    let detail = parsed.error?;
    // PLAN F17: provider response prose must never reach error strings. Only
    // allowlisted metadata (safe code, request ID) is extracted; the message
    // field and any raw body are discarded because they are provider-
    // controlled text that could echo request or transcript content into the
    // diagnostic log and the UI.
    let mut parts = Vec::new();
    if let Some(code) = detail.code {
        parts.push(code);
    }
    if let Some(request_id) = detail.request_id {
        parts.push(request_id);
    }
    let joined = parts.join(": ");
    if joined.is_empty() {
        None
    } else {
        Some(joined.chars().take(300).collect())
    }
}

/// Reads a bounded slice of an error response body and extracts only
/// allowlisted metadata (safe code, request ID). Provider message text and
/// raw body content are deliberately discarded so neither the UI nor the
/// diagnostic log can capture response or transcript content.
async fn read_error_detail(mut resp: reqwest::Response) -> Option<String> {
    const MAX_ERROR_BODY_BYTES: usize = 2048;

    let mut bytes = Vec::new();
    while bytes.len() < MAX_ERROR_BODY_BYTES {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(MAX_ERROR_BODY_BYTES - bytes.len());
                bytes.extend_from_slice(&chunk[..take]);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    if bytes.is_empty() {
        return None;
    }
    let body = String::from_utf8_lossy(&bytes);
    parse_error_detail(&body)
}

pub fn build_whisper_guidance(entries: &[DictionaryEntry]) -> String {
    // Sort entries: enabled only, newest created_at first, id descending as tie-breaker
    let mut sorted: Vec<&DictionaryEntry> = entries.iter().filter(|e| e.enabled).collect();
    sorted.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });

    let mut selected: Vec<&str> = Vec::new();
    let mut total_bytes = 0;

    for entry in sorted {
        let spelling = entry.correction.as_deref().unwrap_or(&entry.value).trim();
        if spelling.is_empty() {
            continue;
        }
        let needed = if selected.is_empty() {
            spelling.len()
        } else {
            spelling.len() + 2 // for ", "
        };

        if total_bytes + needed <= MAX_GUIDANCE_BYTES {
            selected.push(spelling);
            total_bytes += needed;
        }
    }

    selected.join(", ")
}

fn is_filler_only(s: &str) -> bool {
    let lower = s.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || c.is_punctuation() || c.is_ascii_punctuation())
        .filter(|w| !w.is_empty())
        .collect();

    if words.is_empty() {
        return true;
    }

    words
        .iter()
        .all(|&w| matches!(w, "um" | "uh" | "erm" | "er"))
}

fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
}

fn get_retry_delay(resp: &reqwest::Response, attempt: usize) -> Option<Duration> {
    if let Some(after_header) = resp.headers().get("Retry-After") {
        if let Ok(after_str) = after_header.to_str() {
            if let Ok(secs) = after_str.parse::<u64>() {
                if secs <= 30 {
                    return Some(Duration::from_secs(secs));
                } else {
                    return None;
                }
            }
        }
    }
    // Exponential backoff with jitter: 1s, 2s + up to 250ms
    let base_ms = match attempt {
        1 => 1000,
        _ => 2000,
    };
    let jitter = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis()
        % 250) as u64;
    Some(Duration::from_millis(base_ms + jitter))
}

fn map_status_error(status: StatusCode, detail: Option<&str>) -> FlowError {
    let base = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            // Same invalid-key error and wording as test_key so transcription
            // and cleanup failures read identically to the key check.
            FlowError::Message("Invalid Groq API key.".into())
        }
        StatusCode::NOT_FOUND => {
            FlowError::Message("Requested model was not found on Groq.".into())
        }
        StatusCode::PAYLOAD_TOO_LARGE => {
            FlowError::Message("Audio attachment exceeded provider size limit.".into())
        }
        StatusCode::TOO_MANY_REQUESTS => FlowError::Message(
            "Groq rate limit reached. Please wait a moment and try again.".into(),
        ),
        s if s.is_server_error() => {
            FlowError::Message("Groq service is temporarily unavailable.".into())
        }
        other => FlowError::Message(format!("Groq API error: {other}")),
    };
    match detail {
        Some(detail) if !detail.trim().is_empty() => {
            FlowError::Message(format!("{base} ({detail})"))
        }
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictation_cleanup_prompt_snapshot() {
        // Normalize line endings so the snapshot holds on CRLF checkouts too.
        let loaded = include_str!("../prompts/dictation_cleanup.txt").replace("\r\n", "\n");
        assert!(loaded.starts_with("You clean speech-to-text dictation."));
        assert!(loaded.contains("raw_transcript is the original transcription."));
        assert!(loaded.contains("corrected_transcript contains the same dictation"));
        assert!(loaded.ends_with("If it is already clean, return it unchanged.\n"));
    }

    #[test]
    fn test_filler_only() {
        assert!(is_filler_only("um uh erm..."));
        assert!(is_filler_only("   UH,   ER  "));
        assert!(!is_filler_only("um urgent"));
        assert!(!is_filler_only("hello"));
    }

    #[test]
    fn test_whisper_guidance_budget() {
        let entries = vec![
            DictionaryEntry {
                id: 1,
                value: "Flow".into(),
                correction: None,
                enabled: true,
                conflict_reason: None,
                created_at: 10,
            },
            DictionaryEntry {
                id: 2,
                value: "btw".into(),
                correction: Some("by the way".into()),
                enabled: true,
                conflict_reason: None,
                created_at: 20,
            },
        ];
        let guidance = build_whisper_guidance(&entries);
        assert_eq!(guidance, "by the way, Flow");
        assert!(guidance.len() <= MAX_GUIDANCE_BYTES);
    }

    #[test]
    fn test_parse_error_detail_extracts_only_allowlisted_fields() {
        let body = r#"{"error":{"message":"Rate limit reached for model `qwen/qwen3.8-27b` on tokens per day (TPD): Limit 200000, Used 199000, Requested 16384.","type":"requests","code":"rate_limit_exceeded","request_id":"req_01abc"}}"#;
        let detail = parse_error_detail(body).unwrap();
        assert_eq!(detail, "rate_limit_exceeded: req_01abc");
        // Provider message prose must never be echoed into error strings.
        assert!(!detail.contains("Rate limit reached"));
    }

    #[test]
    fn test_parse_error_detail_requires_allowlisted_fields() {
        // Without a safe code or request ID there is no detail: raw body or
        // message text must never fall back into the error string.
        let body = r#"{"error":{"message":"provider message that may echo transcript content"}}"#;
        assert!(parse_error_detail(body).is_none());
    }

    #[test]
    fn test_parse_error_detail_ignores_non_json_and_empty() {
        assert!(parse_error_detail("not json").is_none());
        assert!(parse_error_detail("{\"error\":{}}").is_none());
        assert!(parse_error_detail("").is_none());
    }

    #[test]
    fn test_map_status_error_appends_provider_detail() {
        let base = map_status_error(StatusCode::TOO_MANY_REQUESTS, None).to_string();
        assert_eq!(
            base,
            "Groq rate limit reached. Please wait a moment and try again."
        );
        let with_detail = map_status_error(
            StatusCode::TOO_MANY_REQUESTS,
            Some(
                "rate_limit_exceeded: Rate limit reached for model `qwen/qwen3.8-27b` on tokens per day (TPD).",
            ),
        )
        .to_string();
        assert!(with_detail
            .starts_with("Groq rate limit reached. Please wait a moment and try again. ("));
        assert!(with_detail.contains("rate_limit_exceeded"));
    }

    #[test]
    fn test_clean_attempt_failure_classifies_provider_errors() {
        assert!(matches!(
            clean_attempt_failure(StatusCode::TOO_MANY_REQUESTS, None),
            CleanAttemptError::Provider(_, _)
        ));
        assert!(matches!(
            clean_attempt_failure(StatusCode::BAD_GATEWAY, None),
            CleanAttemptError::Provider(_, _)
        ));
        assert!(matches!(
            clean_attempt_failure(StatusCode::UNAUTHORIZED, None),
            CleanAttemptError::Fatal(FlowError::Message(_))
        ));
    }
}
