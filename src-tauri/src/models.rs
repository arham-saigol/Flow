use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub text: String,
    pub raw_text: String,
    pub word_count: i64,
    pub duration_ms: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingDictation {
    pub id: i64,
    pub text: String,
    pub stage: String,
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DictionaryEntry {
    pub id: i64,
    pub value: String,
    pub correction: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snippet {
    pub id: i64,
    pub trigger: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardData {
    pub total_words_dictated: i64,
    pub average_words_per_minute: i64,
    pub time_dictated_ms: i64,
    pub estimated_saved_ms: i64,
    pub history: Vec<HistoryEntry>,
    pub pending: Vec<PendingDictation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsData {
    pub has_api_key: bool,
    pub microphone_id: String,
    pub microphone_name: String,
    pub keybind: String,
    pub launch_at_startup: bool,
    pub history_retention: String,
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            has_api_key: false,
            microphone_id: String::new(),
            microphone_name: "System default".into(),
            keybind: "Right Alt".into(),
            launch_at_startup: false,
            history_retention: "30 days".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Serialize)]
pub struct OverlayPayload {
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct WaveformPayload {
    pub level: f32,
}

#[derive(Clone, Serialize)]
pub struct MessagePayload {
    pub message: String,
}
