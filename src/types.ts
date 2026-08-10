export type Page = "dashboard" | "dictionary" | "snippets";

export interface HistoryEntry {
  id: number;
  text: string;
  raw_text: string;
  word_count: number;
  duration_ms: number;
  created_at: number;
}

export interface DictionaryEntry {
  id: number;
  value: string;
  correction: string | null;
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
  text: string;
  stage: "transcription" | "cleanup" | "ready";
  error: string | null;
  created_at: number;
}

export type TranscriptionModel =
  | "groq-whisper-large-v3"
  | "deepgram-nova-3";

export interface SettingsData {
  has_groq_api_key: boolean;
  has_deepgram_api_key: boolean;
  transcription_model: TranscriptionModel;
  microphone_id: string;
  microphone_name: string;
  keybind: string;
  launch_at_startup: boolean;
  history_retention: string;
}

export interface Microphone {
  id: string;
  name: string;
  is_default: boolean;
}
