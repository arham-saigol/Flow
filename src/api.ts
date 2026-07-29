import { invoke } from "@tauri-apps/api/core";
import type {
  DashboardData,
  DictionaryEntry,
  Microphone,
  SettingsData,
  Snippet,
} from "./types";

export const api = {
  dashboard: () => invoke<DashboardData>("get_dashboard"),
  dictionary: () => invoke<DictionaryEntry[]>("list_dictionary"),
  addDictionary: (value: string) =>
    invoke<DictionaryEntry>("add_dictionary", { value }),
  updateDictionary: (id: number, value: string) =>
    invoke<void>("update_dictionary", { id, value }),
  deleteDictionary: (id: number) =>
    invoke<void>("delete_dictionary", { id }),
  snippets: () => invoke<Snippet[]>("list_snippets"),
  addSnippet: (trigger: string, content: string) =>
    invoke<Snippet>("add_snippet", { trigger, content }),
  updateSnippet: (id: number, trigger: string, content: string) =>
    invoke<void>("update_snippet", { id, trigger, content }),
  deleteSnippet: (id: number) => invoke<void>("delete_snippet", { id }),
  settings: () => invoke<SettingsData>("get_settings"),
  saveSettings: (settings: SettingsData, apiKey?: string) =>
    invoke<void>("save_settings", { settings, apiKey: apiKey || null }),
  microphones: () => invoke<Microphone[]>("list_microphones"),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  testApiKey: (apiKey: string) => invoke<void>("test_api_key", { apiKey }),
  startRecording: () => invoke<void>("start_recording"),
  stopRecording: () => invoke<void>("stop_recording"),
  cancelRecording: () => invoke<void>("cancel_recording"),
};
