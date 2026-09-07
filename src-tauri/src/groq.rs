use std::time::{Duration, SystemTime, UNIX_EPOCH};
use reqwest::{multipart, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::{FlowError, Result},
    models::{DictionaryEntry, CLEANUP_MODEL, TRANSCRIPTION_MODEL},
};

const API_BASE: &str = "https://api.groq.com/openai/v1";
const SYSTEM_PROMPT: &str = include_str!("../prompts/dictation_cleanup.txt");
const MAX_GUIDANCE_BYTES: usize = 200;
const MAX_TRANSCRIPT_BYTES: usize = 32_000;
const MAX_BODY_BYTES: usize = 1_048_576; // 1 MiB

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

        let models = response.json::<ModelsList>().await.map_err(|_| {
            FlowError::Message("Could not parse Groq models list.".into())
        })?;

        let has_whisper = models.data.iter().any(|m| m.id == TRANSCRIPTION_MODEL);
        let has_qwen = models.data.iter().any(|m| m.id == CLEANUP_MODEL);

        if !has_whisper || !has_qwen {
            return Err(FlowError::Message(format!(
                "Your Groq account does not have access to both required models ({TRANSCRIPTION_MODEL} and {CLEANUP_MODEL})."
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

        let mut attempts = 0;
        loop {
            attempts += 1;
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
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let bytes = resp.bytes().await?;
                        if bytes.len() > MAX_BODY_BYTES {
                            return Err(FlowError::Message("Transcription response exceeded 1 MB limit.".into()));
                        }
                        let verbose = serde_json::from_slice::<VerboseTranscriptionResponse>(&bytes)
                            .map_err(|_| FlowError::Message("Invalid JSON from Whisper transcription.".into()))?;
                        
                        let trimmed = verbose.text.trim().to_string();
                        if trimmed.is_empty() {
                            return Err(FlowError::EmptyRecording);
                        }

                        // Check segments for suspect speech
                        let mut is_suspect = false;
                        for seg in &verbose.segments {
                            let no_speech = seg.no_speech_prob.unwrap_or(0.0);
                            let avg_logprob = seg.avg_logprob.unwrap_or(0.0);
                            let comp_ratio = seg.compression_ratio.unwrap_or(1.0);

                            if (no_speech >= 0.6 && avg_logprob <= -1.0) || comp_ratio > 2.4 {
                                is_suspect = true;
                                break;
                            }
                        }

                        return Ok(TranscriptionResult {
                            text: trimmed,
                            is_suspect,
                        });
                    }

                    if is_retryable(status) && attempts < 3 {
                        let delay = get_retry_delay(&resp, attempts);
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(map_status_error(status));
                }
                Err(e) => {
                    if attempts < 3 && (e.is_connect() || e.is_timeout()) {
                        tokio::time::sleep(Duration::from_millis(1000 * attempts as u64)).await;
                        continue;
                    }
                    return Err(FlowError::Network(e));
                }
            }
        }
    }

    pub async fn clean(
        &self,
        api_key: &str,
        raw_transcript: &str,
        corrected_transcript: &str,
    ) -> Result<String> {
        let bounded_raw = if raw_transcript.len() > MAX_TRANSCRIPT_BYTES {
            return Err(FlowError::Message("Transcript exceeds 32,000 bytes limit.".into()));
        } else {
            raw_transcript
        };

        let bounded_corrected = if corrected_transcript.len() > MAX_TRANSCRIPT_BYTES {
            return Err(FlowError::Message("Corrected transcript exceeds 32,000 bytes limit.".into()));
        } else {
            corrected_transcript
        };

        let user_content = serde_json::to_string(&UserMessagePayload {
            raw_transcript: bounded_raw,
            corrected_transcript: bounded_corrected,
        })
        .map_err(|e| FlowError::Message(format!("Could not serialize cleanup input: {e}")))?;

        let request_body = json!({
            "model": CLEANUP_MODEL,
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
            attempts += 1;
            let send_res = self
                .client
                .post(format!("{}/chat/completions", self.base_url))
                .bearer_auth(api_key.trim())
                .json(&request_body)
                .send()
                .await;

            match send_res {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let bytes = resp.bytes().await?;
                        if bytes.len() > MAX_BODY_BYTES {
                            return Err(FlowError::Message("Cleanup response exceeded 1 MB limit.".into()));
                        }

                        let chat: ChatResponse = serde_json::from_slice(&bytes)
                            .map_err(|_| FlowError::Message("Invalid JSON from cleanup model.".into()))?;

                        if chat.choices.is_empty() {
                            return Err(FlowError::Message("Cleanup model returned no choices.".into()));
                        }

                        let choice = &chat.choices[0];
                        if choice.message.refusal.is_some() {
                            return Err(FlowError::Message("Cleanup request was refused by model.".into()));
                        }

                        if let Some(reason) = &choice.finish_reason {
                            if reason != "stop" {
                                return Err(FlowError::Message(format!(
                                    "Cleanup completion was truncated or abnormal (finish_reason: {reason})."
                                )));
                            }
                        }

                        let content = choice.message.content.as_deref().unwrap_or("").trim();

                        // Check for embedded NUL
                        if content.contains('\0') {
                            return Err(FlowError::Message("Cleanup response contains disallowed NUL byte.".into()));
                        }

                        // Check for leaked reasoning block
                        if content.starts_with("<think>") || content.contains("</think>") {
                            return Err(FlowError::Message("Cleanup response contains leaked reasoning block.".into()));
                        }

                        // Validate empty output
                        if content.is_empty() {
                            if is_filler_only(raw_transcript) && is_filler_only(corrected_transcript) {
                                return Ok(String::new());
                            } else {
                                return Err(FlowError::Message(
                                    "Model returned empty text for substantive dictation.".into(),
                                ));
                            }
                        }

                        return Ok(content.to_string());
                    }

                    if is_retryable(status) && attempts < 3 {
                        let delay = get_retry_delay(&resp, attempts);
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(map_status_error(status));
                }
                Err(e) => {
                    if attempts < 3 && (e.is_connect() || e.is_timeout()) {
                        tokio::time::sleep(Duration::from_millis(1000 * attempts as u64)).await;
                        continue;
                    }
                    return Err(FlowError::Network(e));
                }
            }
        }
    }
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
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|w| !w.is_empty())
        .collect();

    if words.is_empty() {
        return true;
    }

    words.iter().all(|&w| matches!(w, "um" | "uh" | "erm" | "er"))
}

fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::INTERNAL_SERVER_ERROR
        || status == StatusCode::BAD_GATEWAY
        || status == StatusCode::SERVICE_UNAVAILABLE
        || status == StatusCode::GATEWAY_TIMEOUT
}

fn get_retry_delay(resp: &reqwest::Response, attempt: usize) -> Duration {
    if let Some(after_header) = resp.headers().get("Retry-After") {
        if let Ok(after_str) = after_header.to_str() {
            if let Ok(secs) = after_str.parse::<u64>() {
                if secs <= 30 {
                    return Duration::from_secs(secs);
                }
            }
        }
    }
    // Exponential backoff with jitter: 1s, 2s + up to 250ms
    let base_ms = match attempt {
        1 => 1000,
        _ => 2000,
    };
    let jitter = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_millis() % 250) as u64;
    Duration::from_millis(base_ms + jitter)
}

fn map_status_error(status: StatusCode) -> FlowError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => FlowError::MissingApiKey,
        StatusCode::NOT_FOUND => FlowError::Message("Requested model was not found on Groq.".into()),
        StatusCode::PAYLOAD_TOO_LARGE => FlowError::Message("Audio attachment exceeded provider size limit.".into()),
        StatusCode::TOO_MANY_REQUESTS => FlowError::Message("Groq rate limit reached. Please wait a moment and try again.".into()),
        s if s.is_server_error() => FlowError::Message("Groq service is temporarily unavailable.".into()),
        other => FlowError::Message(format!("Groq API error: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictation_cleanup_prompt_snapshot() {
        let loaded = include_str!("../prompts/dictation_cleanup.txt");
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
}
