import { type RefObject, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  AudioLines,
  Check,
  Download,
  Eye,
  EyeOff,
  Keyboard,
  KeyRound,
  LoaderCircle,
  Mic,
  RefreshCw,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../api";
import { useDialogFocus } from "../hooks/useDialogFocus";
import type { AppConfig, Microphone, SettingsData } from "../types";
import type { ToastData } from "./Toast";
import { PrivacyNoticeModal } from "./PrivacyNoticeModal";

const defaults: SettingsData = {
  has_api_key: false,
  microphone_id: "",
  microphone_name: "System default",
  keybind: "Right Alt",
  launch_at_startup: false,
  history_retention: "30 days",
  privacy_notice_version: null,
};

export function SettingsModal({
  onClose,
  notify,
  onSaved,
  returnFocusRef,
}: {
  onClose: () => void;
  notify: (data: ToastData) => void;
  onSaved: (keybind: string) => void;
  returnFocusRef: RefObject<HTMLElement>;
}) {
  const [settings, setSettings] = useState(defaults);
  const [microphones, setMicrophones] = useState<Microphone[]>([]);
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState(false);
  const [saving, setSaving] = useState(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [activeTab, setActiveTab] = useState<"general" | "transcription">("general");
  const [capturingHotkey, setCapturingHotkey] = useState(false);
  const [hotkeyError, setHotkeyError] = useState("");

  // New states
  const [testingMic, setTestingMic] = useState(false);
  const [micLevel, setMicLevel] = useState(0);
  const [confirmRemoveKey, setConfirmRemoveKey] = useState(false);
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const privacyButtonRef = useRef<HTMLButtonElement>(null);
  const generalTabRef = useRef<HTMLButtonElement>(null);
  const transcriptionTabRef = useRef<HTMLButtonElement>(null);

  const testingMicRef = useRef(testingMic);
  const capturingHotkeyRef = useRef(capturingHotkey);

  // Sync refs after commit (not during render) so the unmount cleanup below
  // observes only values from committed renders.
  useEffect(() => {
    testingMicRef.current = testingMic;
  }, [testingMic]);
  useEffect(() => {
    capturingHotkeyRef.current = capturingHotkey;
  }, [capturingHotkey]);

  const handleTabKeyDown = (e: React.KeyboardEvent, current: "general" | "transcription") => {
    if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      e.preventDefault();
      if (current === "general") {
        setActiveTab("transcription");
        transcriptionTabRef.current?.focus();
      } else {
        setActiveTab("general");
        generalTabRef.current?.focus();
      }
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      e.preventDefault();
      if (current === "general") {
        setActiveTab("transcription");
        transcriptionTabRef.current?.focus();
      } else {
        setActiveTab("general");
        generalTabRef.current?.focus();
      }
    } else if (e.key === "Home") {
      e.preventDefault();
      setActiveTab("general");
      generalTabRef.current?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      setActiveTab("transcription");
      transcriptionTabRef.current?.focus();
    }
  };

  const dialogRef = useDialogFocus(true, returnFocusRef, () => {
    if (saving) return;
    if (capturingHotkey) {
      void api.cancelShortcutCapture();
      setCapturingHotkey(false);
      setHotkeyError("");
    } else {
      onClose();
    }
  });

  const loadMicrophones = () => {
    void api
      .microphones()
      .then(setMicrophones)
      .catch((error) => notify({ kind: "error", message: String(error) }));
  };

  useEffect(() => {
    void api
      .settings()
      .then((next) => {
        setSettings(next);
        setSettingsLoaded(true);
      })
      .catch((error) => notify({ kind: "error", message: String(error) }));

    loadMicrophones();

    void api
      .appConfig()
      .then(setAppConfig)
      .catch(() => {});

    const onFocus = () => loadMicrophones();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [notify]);

  // Shortcut capture listener
  useEffect(() => {
    if (!capturingHotkey) return;

    let disposed = false;
    let unlistenCapture: UnlistenFn | undefined;
    let unlistenCancel: UnlistenFn | undefined;

    void listen<{ keybind: string }>("shortcut-captured", (event) => {
      setSettings((current) => ({ ...current, keybind: event.payload.keybind }));
      setCapturingHotkey(false);
      setHotkeyError("");
    }).then((fn) => {
      if (disposed) fn();
      else unlistenCapture = fn;
    });

    void listen("shortcut-capture-cancelled", () => {
      setCapturingHotkey(false);
      setHotkeyError("");
    }).then((fn) => {
      if (disposed) fn();
      else unlistenCancel = fn;
    });

    return () => {
      disposed = true;
      unlistenCapture?.();
      unlistenCancel?.();
    };
  }, [capturingHotkey]);

  // Microphone test listener
  useEffect(() => {
    if (!testingMic) {
      setMicLevel(0);
      return;
    }

    let disposed = false;
    let unlistenLevel: UnlistenFn | undefined;
    void listen<{ level: number }>("audio-level", (event) => {
      setMicLevel(Math.max(0, Math.min(1, event.payload.level)));
    }).then((fn) => {
      if (disposed) fn();
      else unlistenLevel = fn;
    });

    return () => {
      disposed = true;
      unlistenLevel?.();
    };
  }, [testingMic]);

  // Clean up mic test on unmount
  useEffect(() => {
    return () => {
      if (testingMicRef.current) {
        void api.stopMicrophoneTest();
      }
      if (capturingHotkeyRef.current) {
        void api.cancelShortcutCapture();
      }
    };
  }, []);

  const toggleMicTest = async () => {
    if (testingMic) {
      try {
        await api.stopMicrophoneTest();
      } catch (err) {
        console.error(err);
      }
      setTestingMic(false);
    } else {
      try {
        await api.startMicrophoneTest(settings.microphone_id);
        setTestingMic(true);
      } catch (err) {
        notify({ kind: "error", message: String(err) });
      }
    }
  };

  const startCapturing = async () => {
    setCapturingHotkey(true);
    setHotkeyError("");
    try {
      await api.startShortcutCapture();
    } catch (err) {
      setCapturingHotkey(false);
      notify({ kind: "error", message: String(err) });
    }
  };

  const cancelCapturing = async () => {
    setCapturingHotkey(false);
    setHotkeyError("");
    try {
      await api.cancelShortcutCapture();
    } catch (err) {
      console.error(err);
    }
  };

  const test = async () => {
    if (!apiKey) return;
    setTesting(true);
    try {
      await api.testApiKey(apiKey);
      setTested(true);
      notify({ kind: "success", message: "Groq API key connected" });
    } catch (error) {
      setTested(false);
      notify({ kind: "error", message: String(error) });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    if (!settingsLoaded) return;
    if (testingMic) {
      await api.stopMicrophoneTest().catch(() => {});
      setTestingMic(false);
    }
    setSaving(true);
    try {
      const selected = microphones.find((item) => item.id === settings.microphone_id);
      await api.saveSettings(
        {
          ...settings,
          microphone_name: settings.microphone_id
            ? selected?.name ?? settings.microphone_name
            : "System default",
        },
        apiKey || undefined,
      );
      notify({ kind: "success", message: "Settings saved" });
      onSaved(settings.keybind);
      onClose();
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    } finally {
      setSaving(false);
    }
  };

  const handleRemoveApiKey = async () => {
    try {
      await api.deleteApiKey();
      setSettings((current) => ({ ...current, has_api_key: false }));
      setApiKey("");
      setConfirmRemoveKey(false);
      notify({ kind: "success", message: "API key removed from Windows Credential Manager" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleExportDiagnostics = async () => {
    try {
      const logs = await api.exportDiagnostics();
      await api.copyText(logs);
      notify({ kind: "success", message: "Diagnostic logs copied to clipboard" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleDeleteUpgradeBackup = async () => {
    try {
      await api.deleteUpgradeBackup();
      setAppConfig((c) => c ? { ...c, backup_file: null, backup_expires_at: null } : null);
      notify({ kind: "success", message: "Database upgrade backup deleted" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleClearHistory = async () => {
    if (!window.confirm("Are you sure you want to delete all dictation history? This cannot be undone.")) {
      return;
    }
    try {
      await api.deleteAllHistory(false);
      notify({ kind: "success", message: "Dictation history cleared" });
      onSaved(settings.keybind);
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleResetStats = async () => {
    if (!window.confirm("Reset aggregate statistics counter?")) {
      return;
    }
    try {
      await api.resetStatistics();
      notify({ kind: "success", message: "Statistics reset" });
      onSaved(settings.keybind);
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const modalRoot =
    typeof document !== "undefined"
      ? document.getElementById("modal-root") || document.body
      : null;

  if (!modalRoot) return null;

  return createPortal(
    <>
      <div
        className="modal-backdrop"
        role="presentation"
        onMouseDown={(event) => {
          if (event.target !== event.currentTarget) return;
          if (saving) return;
          if (capturingHotkey) {
            void cancelCapturing();
          } else {
            onClose();
          }
        }}
      >
        <section
          ref={dialogRef}
          className="settings-modal"
          role="dialog"
          aria-modal="true"
          aria-labelledby="settings-title"
          tabIndex={-1}
        >
          <header>
            <h2 id="settings-title">Settings</h2>
          </header>

          <div className="settings-layout">
            <nav className="settings-tabs" aria-label="Settings sections" role="tablist">
              <button
                ref={generalTabRef}
                id="settings-general-tab"
                type="button"
                role="tab"
                tabIndex={activeTab === "general" ? 0 : -1}
                aria-selected={activeTab === "general"}
                aria-controls="settings-panel"
                className={activeTab === "general" ? "active" : ""}
                onClick={() => setActiveTab("general")}
                onKeyDown={(e) => handleTabKeyDown(e, "general")}
              >
                <SlidersHorizontal size={16} />
                <span>General</span>
              </button>
              <button
                ref={transcriptionTabRef}
                id="settings-transcription-tab"
                type="button"
                role="tab"
                tabIndex={activeTab === "transcription" ? 0 : -1}
                aria-selected={activeTab === "transcription"}
                aria-controls="settings-panel"
                className={activeTab === "transcription" ? "active" : ""}
                onClick={() => setActiveTab("transcription")}
                onKeyDown={(e) => handleTabKeyDown(e, "transcription")}
              >
                <AudioLines size={16} />
                <span>Transcription</span>
              </button>
            </nav>

            <div
              id="settings-panel"
              className="settings-pane"
              role="tabpanel"
              aria-labelledby={`settings-${activeTab}-tab`}
            >
              {activeTab === "general" ? (
                <>
                  <div className="settings-pane__heading">
                    <h3>General</h3>
                    <p>Choose how Flow listens, starts, and stores your history.</p>
                  </div>

                  <div className="settings-form-grid">
                    <div className="field">
                      <div className="flex items-center justify-between">
                        <span>Microphone</span>
                        <div className="flex items-center gap-1">
                          <button
                            type="button"
                            onClick={loadMicrophones}
                            title="Refresh microphones"
                            aria-label="Refresh microphone list"
                            className="p-1 text-stone-500 hover:text-stone-800 rounded transition-colors"
                          >
                            <RefreshCw size={13} />
                          </button>
                          <button
                            type="button"
                            onClick={() => void toggleMicTest()}
                            className={`text-xs px-2 py-0.5 rounded border transition-colors ${
                              testingMic
                                ? "bg-amber-100 text-amber-900 border-amber-300 font-semibold"
                                : "bg-white text-stone-600 border-stone-200 hover:bg-stone-50"
                            }`}
                          >
                            {testingMic ? "Stop Test" : "Test Mic"}
                          </button>
                        </div>
                      </div>
                      <select
                        value={settings.microphone_id}
                        onChange={(e) =>
                          setSettings({ ...settings, microphone_id: e.target.value })
                        }
                      >
                        <option value="">System default</option>
                        {settings.microphone_id &&
                          !microphones.some((m) => m.id === settings.microphone_id) && (
                            <option value={settings.microphone_id} disabled>
                              {settings.microphone_name || "Saved microphone"} (Unavailable)
                            </option>
                          )}
                        {microphones.map((microphone) => (
                          <option value={microphone.id} key={microphone.id}>
                            {microphone.name}{!microphone.is_available ? " (Unavailable)" : ""}
                          </option>
                        ))}
                      </select>
                      {testingMic && (
                        <div className="mt-1.5 p-2 bg-stone-100/80 rounded-lg border border-stone-200/80 space-y-1">
                          <div className="flex items-center justify-between text-[11px] text-stone-600">
                            <span className="flex items-center gap-1">
                              <Mic size={11} className="text-emerald-600 animate-pulse" />
                              Input Level
                            </span>
                            <span>{Math.round(micLevel * 100)}%</span>
                          </div>
                          <div className="h-1.5 w-full bg-stone-200 rounded-full overflow-hidden">
                            <div
                              className="h-full bg-emerald-500 transition-all duration-75 rounded-full"
                              style={{ width: `${Math.min(100, micLevel * 100)}%` }}
                            />
                          </div>
                        </div>
                      )}
                    </div>

                    <label className="field">
                      <span>History retention</span>
                      <select
                        value={settings.history_retention}
                        onChange={(e) =>
                          setSettings({ ...settings, history_retention: e.target.value })
                        }
                      >
                        {["24 hours", "7 days", "30 days", "Forever"].map((value) => (
                          <option key={value}>{value}</option>
                        ))}
                      </select>
                    </label>

                    <div className="field field--wide">
                      <span>Dictation shortcut</span>
                      <button
                        className={`hotkey-capture ${capturingHotkey ? "is-capturing" : ""}`}
                        type="button"
                        aria-pressed={capturingHotkey}
                        onClick={() => {
                          if (capturingHotkey) {
                            void cancelCapturing();
                          } else {
                            void startCapturing();
                          }
                        }}
                      >
                        <Keyboard size={16} />
                        <span>{capturingHotkey ? "Press hotkey (e.g. Right Alt, F8)…" : settings.keybind}</span>
                      </button>
                      <small>
                        {hotkeyError ||
                          (capturingHotkey
                            ? "Press Escape to cancel."
                            : "Click to record a shortcut (Right Alt, Left Alt, Right Ctrl, or F8–F12).")}
                      </small>
                    </div>

                    <label className="toggle-row field--wide">
                      <div>
                        <span>Launch at startup</span>
                        <p>Keep Flow ready in the system tray.</p>
                      </div>
                      <input
                        type="checkbox"
                        checked={settings.launch_at_startup}
                        onChange={(e) =>
                          setSettings({ ...settings, launch_at_startup: e.target.checked })
                        }
                      />
                    </label>

                    {/* Data Management & Diagnostics */}
                    <div className="field field--wide pt-2 border-t border-stone-200/80 space-y-2">
                      <span className="font-medium text-stone-800 text-xs uppercase tracking-wider">
                        Storage & Diagnostics
                      </span>
                      <div className="flex flex-wrap gap-2 pt-1">
                        <button
                          type="button"
                          className="secondary-button compact"
                          onClick={() => void handleExportDiagnostics()}
                        >
                          <Download size={13} />
                          Export Logs
                        </button>
                        <button
                          type="button"
                          className="secondary-button compact"
                          onClick={() => void handleClearHistory()}
                        >
                          <Trash2 size={13} />
                          Clear History
                        </button>
                        <button
                          type="button"
                          className="secondary-button compact"
                          onClick={() => void handleResetStats()}
                        >
                          Reset Stats
                        </button>
                        {appConfig?.backup_file && (
                          <button
                            type="button"
                            className="secondary-button compact text-amber-800"
                            onClick={() => void handleDeleteUpgradeBackup()}
                          >
                            Delete Upgrade Backup
                          </button>
                        )}
                        <button
                          ref={privacyButtonRef}
                          type="button"
                          className="secondary-button compact text-stone-600"
                          onClick={() => setPrivacyOpen(true)}
                        >
                          <ShieldCheck size={13} />
                          Privacy Notice
                        </button>
                      </div>
                    </div>
                  </div>
                </>
              ) : (
                <>
                  <div className="settings-pane__heading">
                    <h3>Transcription</h3>
                    <p>Connect the service Flow uses for transcription and writing cleanup.</p>
                  </div>

                  <div className="settings-transcription">
                    <label className="field">
                      <span>Groq API key</span>
                      <div className="secret-field">
                        <KeyRound size={16} />
                        <input
                          type={showKey ? "text" : "password"}
                          value={apiKey}
                          autoComplete="off"
                          placeholder={settings.has_api_key ? "Saved securely ••••••••" : "gsk_…"}
                          onChange={(e) => {
                            setApiKey(e.target.value);
                            setTested(false);
                          }}
                        />
                        <button
                          type="button"
                          aria-label={showKey ? "Hide API key" : "Show API key"}
                          onClick={() => setShowKey((value) => !value)}
                        >
                          {showKey ? <EyeOff size={16} /> : <Eye size={16} />}
                        </button>
                      </div>
                    </label>
                    <div className="flex items-center gap-2">
                      <button
                        className="secondary-button compact"
                        disabled={!apiKey || testing}
                        onClick={() => void test()}
                      >
                        {testing ? (
                          <LoaderCircle className="spin" size={15} />
                        ) : tested ? (
                          <Check size={15} />
                        ) : null}
                        {testing ? "Checking…" : tested ? "Connected" : "Test connection"}
                      </button>

                      {settings.has_api_key && !confirmRemoveKey && (
                        <button
                          type="button"
                          className="secondary-button compact text-red-700 hover:text-red-800 border-red-200"
                          onClick={() => setConfirmRemoveKey(true)}
                        >
                          Remove saved key
                        </button>
                      )}

                      {confirmRemoveKey && (
                        <div className="flex items-center gap-1.5 p-1 bg-red-50 border border-red-200 rounded-lg">
                          <span className="text-xs text-red-800 font-medium px-1">Confirm delete?</span>
                          <button
                            type="button"
                            className="px-2 py-1 bg-red-600 text-white rounded text-xs font-semibold hover:bg-red-700"
                            onClick={() => void handleRemoveApiKey()}
                          >
                            Yes, remove
                          </button>
                          <button
                            type="button"
                            className="px-2 py-1 text-stone-600 rounded text-xs hover:bg-stone-200"
                            onClick={() => setConfirmRemoveKey(false)}
                          >
                            Cancel
                          </button>
                        </div>
                      )}
                    </div>
                  </div>
                </>
              )}
            </div>
          </div>

          <footer>
            <button className="secondary-button" disabled={saving} onClick={onClose}>
              Cancel
            </button>
            <button
              className="primary-button"
              disabled={!settingsLoaded || saving}
              onClick={() => void save()}
            >
              {saving && <LoaderCircle className="spin" size={16} />}
              {saving ? "Saving…" : "Save settings"}
            </button>
          </footer>
        </section>
      </div>

      <PrivacyNoticeModal
        open={privacyOpen}
        onAccept={() => setPrivacyOpen(false)}
        returnFocusRef={privacyButtonRef}
      />
    </>,
    modalRoot,
  );
}
