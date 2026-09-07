export type Page = "dashboard" | "dictionary" | "snippets";

export type WorkflowPhase =
  | "idle"
  | "starting"
  | "recording"
  | "stopping"
  | "transcribing"
  | "cleaning"
  | "delivering"
  | "microphone_test"
  | "faulted"
  | "shutting_down";

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
  capture_uuid: string | null;
  text: string;
  raw_text: string;
  word_count: number;
  duration_ms: number;
  delivery_outcome: string;
  delivery_warning: string | null;
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
  enabled: boolean;
  conflict_reason: string | null;
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
  capture_uuid: string | null;
  text: string;
  raw_text: string | null;
  corrected_text: string | null;
  stage: string;
  partial: boolean;
  review_reason: string | null;
  no_content: boolean;
  delivery_mode: string;
  delivery_outcome: string;
  delivery_warning: string | null;
  error: string | null;
  error_code: string | null;
  retry_after: number | null;
  history_saved: boolean;
  history_id: number | null;
  created_at: number;
}

export interface SettingsData {
  has_api_key: boolean;
  microphone_id: string;
  microphone_name: string;
  keybind: string;
  launch_at_startup: boolean;
  history_retention: string;
  privacy_notice_version: number | null;
}

export interface AppConfig {
  max_dictionary_source_chars: number;
  max_dictionary_correction_chars: number;
  max_dictionary_entries: number;
  max_snippet_trigger_chars: number;
  max_snippet_content_chars: number;
  max_snippets: number;
  transcription_model: string;
  cleanup_model: string;
  recovery_retention_days: number;
  max_recovery_items: number;
  max_recovery_bytes: number;
  supported_keybinds: string[];
  supported_retentions: string[];
  backup_file: string | null;
  backup_expires_at: number | null;
  allocated_recovery_bytes?: number;
}

export interface Microphone {
  id: string;
  name: string;
  is_default: boolean;
  is_available: boolean;
}
