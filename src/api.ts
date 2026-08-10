import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  DashboardData,
  DictionaryEntry,
  Microphone,
  SettingsData,
  Snippet,
} from "./types";

function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  if (!isTauri()) {
    return Promise.reject(
      new Error("This feature is only available in the Flow desktop app."),
    );
  }
  return invoke<T>(command, args);
}

export const api = {
  dashboard: () => invokeCommand<DashboardData>("get_dashboard"),
  dictionary: () => invokeCommand<DictionaryEntry[]>("list_dictionary"),
  addDictionary: (value: string, correction: string | null) =>
    invokeCommand<DictionaryEntry>("add_dictionary", { value, correction }),
  updateDictionary: (id: number, value: string, correction: string | null) =>
    invokeCommand<void>("update_dictionary", { id, value, correction }),
  deleteDictionary: (id: number) =>
    invokeCommand<void>("delete_dictionary", { id }),
  snippets: () => invokeCommand<Snippet[]>("list_snippets"),
  addSnippet: (trigger: string, content: string) =>
    invokeCommand<Snippet>("add_snippet", { trigger, content }),
  updateSnippet: (id: number, trigger: string, content: string) =>
    invokeCommand<void>("update_snippet", { id, trigger, content }),
  deleteSnippet: (id: number) => invokeCommand<void>("delete_snippet", { id }),
  settings: () => invokeCommand<SettingsData>("get_settings"),
  saveSettings: (
    settings: SettingsData,
    groqApiKey?: string,
    deepgramApiKey?: string,
  ) =>
    invokeCommand<void>("save_settings", {
      settings,
      groqApiKey: groqApiKey || null,
      deepgramApiKey: deepgramApiKey || null,
    }),
  microphones: () => invokeCommand<Microphone[]>("list_microphones"),
  copyText: (text: string) => invokeCommand<void>("copy_text", { text }),
  testGroqApiKey: (apiKey: string) =>
    invokeCommand<void>("test_groq_api_key", { apiKey }),
  testDeepgramApiKey: (apiKey: string) =>
    invokeCommand<void>("test_deepgram_api_key", { apiKey }),
  retryPendingDictation: (id: number) =>
    invokeCommand<void>("retry_pending_dictation", { id }),
  deletePendingDictation: (id: number) =>
    invokeCommand<void>("delete_pending_dictation", { id }),
};
