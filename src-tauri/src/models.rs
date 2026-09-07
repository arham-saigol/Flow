use serde::{Deserialize, Serialize};

pub const MAX_DICTIONARY_SOURCE_CHARS: usize = 100;
pub const MAX_DICTIONARY_CORRECTION_CHARS: usize = 200;
pub const MAX_DICTIONARY_ENTRIES: usize = 1_000;
pub const MAX_SNIPPET_TRIGGER_CHARS: usize = 120;
pub const MAX_SNIPPET_CONTENT_CHARS: usize = 4_000;
pub const MAX_SNIPPETS: usize = 1_000;
pub const TRANSCRIPTION_MODEL: &str = "whisper-large-v3";
pub const CLEANUP_MODEL: &str = "qwen/qwen3.8-27b";
pub const RECOVERY_RETENTION_DAYS: u32 = 7;
pub const MAX_RECOVERY_ITEMS: usize = 100;
pub const MAX_RECOVERY_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Keybind {
    #[serde(rename = "Right Alt")]
    RightAlt,
    #[serde(rename = "Left Alt")]
    LeftAlt,
    #[serde(rename = "Right Ctrl")]
    RightCtrl,
    #[serde(rename = "F8")]
    F8,
    #[serde(rename = "F9")]
    F9,
    #[serde(rename = "F10")]
    F10,
    #[serde(rename = "F11")]
    F11,
    #[serde(rename = "F12")]
    F12,
}

impl Default for Keybind {
    fn default() -> Self {
        Self::RightAlt
    }
}

impl std::fmt::Display for Keybind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RightAlt => write!(f, "Right Alt"),
            Self::LeftAlt => write!(f, "Left Alt"),
            Self::RightCtrl => write!(f, "Right Ctrl"),
            Self::F8 => write!(f, "F8"),
            Self::F9 => write!(f, "F9"),
            Self::F10 => write!(f, "F10"),
            Self::F11 => write!(f, "F11"),
            Self::F12 => write!(f, "F12"),
        }
    }
}

impl std::str::FromStr for Keybind {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Right Alt" => Ok(Self::RightAlt),
            "Left Alt" => Ok(Self::LeftAlt),
            "Right Ctrl" => Ok(Self::RightCtrl),
            "F8" => Ok(Self::F8),
            "F9" => Ok(Self::F9),
            "F10" => Ok(Self::F10),
            "F11" => Ok(Self::F11),
            "F12" => Ok(Self::F12),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryRetention {
    #[serde(rename = "24 hours")]
    TwentyFourHours,
    #[serde(rename = "7 days")]
    SevenDays,
    #[serde(rename = "30 days")]
    ThirtyDays,
    #[serde(rename = "Forever")]
    Forever,
}

impl Default for HistoryRetention {
    fn default() -> Self {
        Self::ThirtyDays
    }
}

impl std::fmt::Display for HistoryRetention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TwentyFourHours => write!(f, "24 hours"),
            Self::SevenDays => write!(f, "7 days"),
            Self::ThirtyDays => write!(f, "30 days"),
            Self::Forever => write!(f, "Forever"),
        }
    }
}

impl std::str::FromStr for HistoryRetention {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "24 hours" => Ok(Self::TwentyFourHours),
            "7 days" => Ok(Self::SevenDays),
            "30 days" => Ok(Self::ThirtyDays),
            "Forever" => Ok(Self::Forever),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub capture_uuid: Option<String>,
    pub text: String,
    pub raw_text: String,
    pub word_count: i64,
    pub duration_ms: i64,
    pub delivery_outcome: String,
    pub delivery_warning: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDictation {
    pub id: i64,
    pub capture_uuid: Option<String>,
    pub text: String,
    pub raw_text: Option<String>,
    pub corrected_text: Option<String>,
    pub stage: String,
    pub partial: bool,
    pub review_reason: Option<String>,
    pub no_content: bool,
    pub delivery_mode: String,
    pub delivery_outcome: String,
    pub delivery_warning: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub retry_after: Option<i64>,
    pub history_saved: bool,
    pub history_id: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: i64,
    pub value: String,
    pub correction: Option<String>,
    pub enabled: bool,
    pub conflict_reason: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: i64,
    pub trigger: String,
    pub content: String,
    pub enabled: bool,
    pub conflict_reason: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub privacy_notice_version: Option<i64>,
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
            privacy_notice_version: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowPhase {
    Idle,
    Starting,
    Recording,
    Stopping,
    Transcribing,
    Cleaning,
    Delivering,
    MicrophoneTest,
    Faulted,
    ShuttingDown,
}

impl std::fmt::Display for WorkflowPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Starting => write!(f, "starting"),
            Self::Recording => write!(f, "recording"),
            Self::Stopping => write!(f, "stopping"),
            Self::Transcribing => write!(f, "transcribing"),
            Self::Cleaning => write!(f, "cleaning"),
            Self::Delivering => write!(f, "delivering"),
            Self::MicrophoneTest => write!(f, "microphone_test"),
            Self::Faulted => write!(f, "faulted"),
            Self::ShuttingDown => write!(f, "shutting_down"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStateSnapshot {
    pub revision: u64,
    pub session_id: Option<u64>,
    pub phase: WorkflowPhase,
    pub active_pending_id: Option<i64>,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_cancel: bool,
    pub message_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub max_dictionary_source_chars: usize,
    pub max_dictionary_correction_chars: usize,
    pub max_dictionary_entries: usize,
    pub max_snippet_trigger_chars: usize,
    pub max_snippet_content_chars: usize,
    pub max_snippets: usize,
    pub transcription_model: String,
    pub cleanup_model: String,
    pub recovery_retention_days: u32,
    pub max_recovery_items: usize,
    pub max_recovery_bytes: u64,
    pub supported_keybinds: Vec<String>,
    pub supported_retentions: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            max_dictionary_source_chars: MAX_DICTIONARY_SOURCE_CHARS,
            max_dictionary_correction_chars: MAX_DICTIONARY_CORRECTION_CHARS,
            max_dictionary_entries: MAX_DICTIONARY_ENTRIES,
            max_snippet_trigger_chars: MAX_SNIPPET_TRIGGER_CHARS,
            max_snippet_content_chars: MAX_SNIPPET_CONTENT_CHARS,
            max_snippets: MAX_SNIPPETS,
            transcription_model: TRANSCRIPTION_MODEL.into(),
            cleanup_model: CLEANUP_MODEL.into(),
            recovery_retention_days: RECOVERY_RETENTION_DAYS,
            max_recovery_items: MAX_RECOVERY_ITEMS,
            max_recovery_bytes: MAX_RECOVERY_BYTES,
            supported_keybinds: vec![
                "Right Alt".into(),
                "Left Alt".into(),
                "Right Ctrl".into(),
                "F8".into(),
                "F9".into(),
                "F10".into(),
                "F11".into(),
                "F12".into(),
            ],
            supported_retentions: vec![
                "24 hours".into(),
                "7 days".into(),
                "30 days".into(),
                "Forever".into(),
            ],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OverlayPayload {
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WaveformPayload {
    pub level: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MessagePayload {
    pub message: String,
}
