use reqwest::{multipart, Client, StatusCode};
use serde::Deserialize;
use serde_json::json;

use crate::error::{FlowError, Result};

const API_BASE: &str = "https://api.groq.com/openai/v1";
const CLEANUP_PROMPT: &str = r#"You are the final writing pass for a voice dictation tool. Transform the raw transcript into polished, ready-to-send writing.

Correct spelling, grammar, punctuation, capitalization, names, and formatting. Remove filler words, repetition, false starts, and fluff. Rephrase only when it helps clarity. Preserve the speaker's exact intent, tone, facts, and level of certainty. Never add, infer, or invent information. Do not answer the speaker or comment on the text.

Return only the final text with no quotation marks, preamble, labels, markdown fences, or explanation."#;

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[derive(Clone)]
pub struct GroqClient {
    client: Client,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(90))
            .tcp_nodelay(true)
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self { client })
    }

    pub async fn test_key(&self, api_key: &str) -> Result<()> {
        let response = self
            .client
            .get(format!("{API_BASE}/models"))
            .bearer_auth(api_key.trim())
            .send()
            .await?;
        response_error(response).await.map(|_| ())
    }

    pub async fn transcribe(
        &self,
        api_key: &str,
        wav: Vec<u8>,
        dictionary: &[String],
    ) -> Result<String> {
        let prompt = if dictionary.is_empty() {
            String::new()
        } else {
            format!("Preferred spellings: {}", dictionary.join(", "))
        };
        let file = multipart::Part::bytes(wav)
            .file_name("dictation.wav")
            .mime_str("audio/wav")
            .map_err(|error| {
                FlowError::Message(format!("Could not prepare the recording: {error}"))
            })?;
        let form = multipart::Form::new()
            .part("file", file)
            .text("model", "whisper-large-v3")
            .text("response_format", "json")
            .text("temperature", "0")
            .text("language", "en")
            .text("prompt", prompt);
        let response = self
            .client
            .post(format!("{API_BASE}/audio/transcriptions"))
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await?;
        let response = response_error(response).await?;
        let transcript = response
            .json::<TranscriptionResponse>()
            .await?
            .text
            .trim()
            .to_string();
        if transcript.is_empty() {
            return Err(FlowError::Message("No speech was detected.".into()));
        }
        Ok(transcript)
    }

    pub async fn clean(&self, api_key: &str, transcript: &str) -> Result<String> {
        let response = self
            .client
            .post(format!("{API_BASE}/chat/completions"))
            .bearer_auth(api_key)
            .json(&json!({
                "model": "qwen/qwen3.6-27b",
                "temperature": 0.1,
                "top_p": 0.9,
                "reasoning_effort": "none",
                "messages": [
                    { "role": "system", "content": CLEANUP_PROMPT },
                    { "role": "user", "content": format!("Raw transcript:\n{transcript}") }
                ]
            }))
            .send()
            .await?;
        let response = response_error(response).await?;
        let output = response
            .json::<ChatResponse>()
            .await?
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| FlowError::Message("Groq returned an empty response.".into()))?;
        Ok(output)
    }
}

async fn response_error(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value.pointer("/error/message")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| match status {
            StatusCode::UNAUTHORIZED => "The Groq API key was rejected.".into(),
            StatusCode::TOO_MANY_REQUESTS => {
                "Groq is rate limiting requests. Please try again shortly.".into()
            }
            _ => format!("Groq request failed ({status})."),
        });
    Err(FlowError::Message(detail))
}
