use reqwest::{header, Client, StatusCode};
use serde::Deserialize;

use crate::error::{FlowError, Result};

const API_BASE: &str = "https://api.deepgram.com/v1";
const MAX_KEYTERMS: usize = 50;
const MAX_KEYTERM_CHARS: usize = 2_000;

#[derive(Deserialize)]
struct TranscriptionResponse {
    results: Results,
}

#[derive(Deserialize)]
struct Results {
    channels: Vec<Channel>,
}

#[derive(Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Deserialize)]
struct Alternative {
    transcript: String,
}

#[derive(Clone)]
pub struct DeepgramClient {
    client: Client,
}

impl DeepgramClient {
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
            .get(format!("{API_BASE}/auth/token"))
            .header(header::AUTHORIZATION, authorization(api_key))
            .send()
            .await?;
        response_error(response).await.map(|_| ())
    }

    pub async fn transcribe(
        &self,
        api_key: &str,
        wav: Vec<u8>,
        preferred_spellings: &[String],
    ) -> Result<String> {
        let keyterms = transcription_keyterms(preferred_spellings);
        let mut response = self
            .send_transcription(api_key, wav.clone(), &keyterms)
            .await?;
        if response.status() == StatusCode::BAD_REQUEST && !keyterms.is_empty() {
            response = self.send_transcription(api_key, wav, &[]).await?;
        }
        let response = response_error(response).await?;
        let transcript = response
            .json::<TranscriptionResponse>()
            .await?
            .results
            .channels
            .into_iter()
            .next()
            .and_then(|channel| channel.alternatives.into_iter().next())
            .map(|alternative| alternative.transcript.trim().to_string())
            .unwrap_or_default();
        if transcript.is_empty() {
            return Err(FlowError::Message("No speech was detected.".into()));
        }
        Ok(transcript)
    }

    async fn send_transcription(
        &self,
        api_key: &str,
        wav: Vec<u8>,
        keyterms: &[&str],
    ) -> Result<reqwest::Response> {
        Ok(self
            .client
            .post(format!("{API_BASE}/listen"))
            .header(header::AUTHORIZATION, authorization(api_key))
            .header(header::CONTENT_TYPE, "audio/wav")
            .query(&[
                ("model", "nova-3"),
                ("smart_format", "true"),
                ("detect_language", "true"),
            ])
            .query(
                &keyterms
                    .iter()
                    .map(|keyterm| ("keyterm", *keyterm))
                    .collect::<Vec<_>>(),
            )
            .body(wav)
            .send()
            .await?)
    }
}

fn authorization(api_key: &str) -> String {
    format!("Token {}", api_key.trim())
}

fn transcription_keyterms(preferred_spellings: &[String]) -> Vec<&str> {
    let mut selected = Vec::new();
    let mut used_chars = 0;
    for spelling in preferred_spellings.iter().take(MAX_KEYTERMS) {
        let chars = spelling.chars().count();
        if used_chars + chars > MAX_KEYTERM_CHARS {
            break;
        }
        used_chars += chars;
        selected.push(spelling.as_str());
    }
    selected
}

async fn response_error(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value.get("err_msg")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| match status {
            StatusCode::UNAUTHORIZED => "The Deepgram API key was rejected.".into(),
            StatusCode::TOO_MANY_REQUESTS => {
                "Deepgram is rate limiting requests. Please try again shortly.".into()
            }
            _ => format!("Deepgram request failed ({status})."),
        });
    Err(FlowError::Message(detail))
}

#[cfg(test)]
mod tests {
    use super::{transcription_keyterms, MAX_KEYTERMS, MAX_KEYTERM_CHARS};

    #[test]
    fn transcription_keyterms_preserve_dictionary_spellings() {
        let spellings = vec!["Flow".into(), "Deepgram Nova".into()];
        assert_eq!(
            transcription_keyterms(&spellings),
            ["Flow", "Deepgram Nova"]
        );
    }

    #[test]
    fn transcription_keyterms_are_bounded() {
        let spellings = (0..100)
            .map(|index| format!("{index}-{}", "x".repeat(100)))
            .collect::<Vec<_>>();
        let keyterms = transcription_keyterms(&spellings);
        assert!(keyterms.len() <= MAX_KEYTERMS);
        assert!(
            keyterms
                .iter()
                .map(|term| term.chars().count())
                .sum::<usize>()
                <= MAX_KEYTERM_CHARS
        );
    }
}
