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
  created_at: number;
}

export interface Snippet {
  id: number;
  trigger: string;
  content: string;
  created_at: number;
}

export interface DashboardData {
  words_this_week: number;
  dictations_this_week: number;
  time_dictated_ms: number;
  estimated_saved_ms: number;
  history: HistoryEntry[];
}

export interface SettingsData {
  has_api_key: boolean;
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
