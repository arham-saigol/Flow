use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlowError {
    #[error("{0}")]
    Message(String),
    #[error("Item not found.")]
    NotFound,
    #[error("Flow is busy with another dictation.")]
    Busy,
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Audio error: {0}")]
    Audio(String),
    #[error("Windows error: {0}")]
    Windows(String),
    #[error("No Groq API key is saved. Open Settings to add one.")]
    MissingApiKey,
    #[error("Flow is already recording.")]
    AlreadyRecording,
    #[error("There is no active recording.")]
    NotRecording,
    #[error("The recording was too short. Please try again.")]
    EmptyRecording,
    #[error("Storage quota or disk space limit exceeded: {0}")]
    QuotaExceeded(String),
    #[error("Settings partially saved: {0}")]
    PartialSettingsSave(String),
    #[error("Unauthorized window caller.")]
    Unauthorized,
}

impl serde::Serialize for FlowError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, FlowError>;
