export type Page = "dashboard" | "dictionary" | "snippets";

export type WorkflowPhase =
  | "idle"
  | "starting"
  | "recording"
  | "transcribing"
  | "cleaning"
  | "delivering"
  | "faulted";

export interface WorkflowStateSnapshot {
  revision: number;
  session_id: number | null;
  phase: WorkflowPhase;
  active_pending_id: number | null;
  can_start: boolean;
  can_stop: boolean;
  can_cancel: boolean;
  message_code: string | null;
}

export type Keybind =
  | "Right Alt"
  | "Left Alt"
  | "Right Ctrl"
  | "F8"
  | "F9"
  | "F10"
  | "F11"
  | "F12";

export interface HistoryEntry {
  id: number;
  uuid?: string;
  text: string;
  raw_text: string;
  word_count: number;
  duration_ms: number;
  input_tokens?: number | null;
  output_tokens?: number | null;
  delivery_mode?: string;
  created_at: number;
}

export interface DictionaryEntry {
  id: number;
  value: string;
  correction: string | null;
  enabled: boolean;
  conflict_reason: string | null;
  created_at: number;
}

export interface Snippet {
  id: number;
  trigger: string;
  content: string;
  created_at: number;
}

export interface DashboardData {
  total_words_dictated: number;
  average_words_per_minute: number;
  time_dictated_ms: number;
  estimated_saved_ms: number;
  history: HistoryEntry[];
  pending: PendingDictation[];
}

export interface PendingDictation {
  id: number;
  uuid?: string;
  text: string;
  raw_text?: string | null;
  stage: "transcription" | "cleanup" | "ready";
  error: string | null;
  created_at: number;
}

export interface SettingsData {
  has_api_key: boolean;
  microphone_id: string;
  microphone_name: string;
  keybind: string;
  launch_at_startup: boolean;
  history_retention: string;
  has_seen_privacy_notice?: boolean;
}

export interface AppConfig {
  version: string;
  schema_version: number;
  database_version: number;
  groq_api_base: string;
  transcription_model: string;
  cleanup_model: string;
  cleanup_reasoning_effort: string;
  max_recovery_items: number;
  max_recovery_bytes: number;
  recovery_retention_days: number;
  default_history_retention: string;
  backup_file: string | null;
  backup_expires_at: number | null;
}

export interface Microphone {
  id: string;
  name: string;
  is_default: boolean;
}
