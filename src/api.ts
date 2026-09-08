import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  AppConfig,
  DashboardData,
  DictionaryEntry,
  HistoryEntry,
  Microphone,
  SettingsData,
  Snippet,
  WorkflowStateSnapshot,
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
  workflowState: () => invokeCommand<WorkflowStateSnapshot>("get_workflow_state"),
  appConfig: () => invokeCommand<AppConfig>("get_app_config"),
  dashboard: () => invokeCommand<DashboardData>("get_dashboard"),
  historyPage: (limit: number, beforeCreatedAt?: number | null, beforeId?: number | null) =>
    invokeCommand<HistoryEntry[]>("get_history_page", {
      limit,
      beforeCreatedAt: beforeCreatedAt ?? null,
      beforeId: beforeId ?? null,
    }),
  deleteHistoryEntry: (id: number) =>
    invokeCommand<void>("delete_history_entry", { id }),
  deleteAllHistory: (deletePending: boolean) =>
    invokeCommand<void>("delete_all_history", { deletePending }),
  resetStatistics: () => invokeCommand<void>("reset_statistics"),
  deleteUpgradeBackup: () => invokeCommand<void>("delete_upgrade_backup"),
  deleteApiKey: () => invokeCommand<void>("delete_api_key"),
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
  saveSettings: (settings: SettingsData, apiKey?: string) =>
    invokeCommand<void>("save_settings", { settings, apiKey: apiKey || null }),
  microphones: () => invokeCommand<Microphone[]>("list_microphones"),
  copyText: (text: string) => invokeCommand<void>("copy_text", { text }),
  testApiKey: (apiKey: string) => invokeCommand<void>("test_api_key", { apiKey }),
  startRecording: () => invokeCommand<void>("start_recording"),
  stopRecording: () => invokeCommand<void>("stop_recording"),
  cancelRecording: () => invokeCommand<void>("cancel_recording"),
  cancelProcessing: () => invokeCommand<void>("cancel_processing"),
  retryPendingDictation: (id: number) =>
    invokeCommand<void>("retry_pending_dictation", { id }),
  retryPendingTranscription: (id: number) =>
    invokeCommand<void>("retry_pending_transcription", { id }),
  deletePendingDictation: (id: number) =>
    invokeCommand<void>("delete_pending_dictation", { id }),
  startShortcutCapture: () => invokeCommand<void>("start_shortcut_capture"),
  cancelShortcutCapture: () => invokeCommand<void>("cancel_shortcut_capture"),
  startMicrophoneTest: (deviceId: string) =>
    invokeCommand<void>("start_microphone_test", { deviceId }),
  stopMicrophoneTest: () => invokeCommand<void>("stop_microphone_test"),
  acceptPendingTranscript: (id: number) =>
    invokeCommand<void>("accept_pending_transcript", { id }),
  acknowledgePrivacyNotice: (version: number) =>
    invokeCommand<void>("acknowledge_privacy_notice", { version }),
  exportDiagnostics: () => invokeCommand<string>("export_diagnostics"),
};
