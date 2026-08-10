import { type RefObject, useEffect, useState } from "react";
import {
  AudioLines,
  Check,
  Eye,
  EyeOff,
  Keyboard,
  KeyRound,
  LoaderCircle,
  SlidersHorizontal,
} from "lucide-react";
import { api } from "../api";
import { useDialogFocus } from "../hooks/useDialogFocus";
import type { Microphone, SettingsData } from "../types";
import type { ToastData } from "./Toast";

const defaults: SettingsData = {
  has_groq_api_key: false,
  has_deepgram_api_key: false,
  transcription_model: "deepgram-nova-3",
  microphone_id: "",
  microphone_name: "System default",
  keybind: "Right Alt",
  launch_at_startup: false,
  history_retention: "30 days",
};

const supportedHotkeys: Record<string, string> = {
  AltRight: "Right Alt",
  AltLeft: "Left Alt",
  ControlRight: "Right Ctrl",
  F8: "F8",
  F9: "F9",
  F10: "F10",
  F11: "F11",
  F12: "F12",
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
  const [groqApiKey, setGroqApiKey] = useState("");
  const [deepgramApiKey, setDeepgramApiKey] = useState("");
  const [showGroqKey, setShowGroqKey] = useState(false);
  const [showDeepgramKey, setShowDeepgramKey] = useState(false);
  const [testing, setTesting] = useState<"groq" | "deepgram" | null>(null);
  const [groqTested, setGroqTested] = useState(false);
  const [deepgramTested, setDeepgramTested] = useState(false);
  const [saving, setSaving] = useState(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [activeTab, setActiveTab] = useState<"general" | "transcription">("general");
  const [capturingHotkey, setCapturingHotkey] = useState(false);
  const [hotkeyError, setHotkeyError] = useState("");
  const dialogRef = useDialogFocus(true, returnFocusRef, () => {
    if (saving) return;
    if (capturingHotkey) {
      setCapturingHotkey(false);
      setHotkeyError("");
    } else {
      onClose();
    }
  });

  useEffect(() => {
    void api
      .settings()
      .then((next) => {
        setSettings(next);
        setSettingsLoaded(true);
      })
      .catch((error) => notify({ kind: "error", message: String(error) }));
    void api
      .microphones()
      .then(setMicrophones)
      .catch((error) => notify({ kind: "error", message: String(error) }));
  }, [notify]);

  const test = async (provider: "groq" | "deepgram") => {
    const key = provider === "groq" ? groqApiKey : deepgramApiKey;
    if (!key) return;
    setTesting(provider);
    try {
      if (provider === "groq") {
        await api.testGroqApiKey(key);
        setGroqTested(true);
      } else {
        await api.testDeepgramApiKey(key);
        setDeepgramTested(true);
      }
      notify({
        kind: "success",
        message: `${provider === "groq" ? "Groq" : "Deepgram"} API key connected`,
      });
    } catch (error) {
      if (provider === "groq") setGroqTested(false);
      else setDeepgramTested(false);
      notify({ kind: "error", message: String(error) });
    } finally {
      setTesting(null);
    }
  };

  const save = async () => {
    if (!settingsLoaded) return;
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
        groqApiKey || undefined,
        deepgramApiKey || undefined,
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

  const captureHotkey = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!capturingHotkey) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setCapturingHotkey(false);
      setHotkeyError("");
      event.currentTarget.blur();
      return;
    }
    const keybind = supportedHotkeys[event.code];
    if (!keybind) {
      setHotkeyError("Use Left or Right Alt, Right Ctrl, or F8–F12.");
      return;
    }
    setSettings((current) => ({ ...current, keybind }));
    setCapturingHotkey(false);
    setHotkeyError("");
    event.currentTarget.blur();
  };

  return (
    <div
      className="modal-backdrop"
      role="presentation"
      onMouseDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (saving) return;
        if (capturingHotkey) {
          setCapturingHotkey(false);
          setHotkeyError("");
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
              id="settings-general-tab"
              type="button"
              role="tab"
              aria-selected={activeTab === "general"}
              aria-controls="settings-panel"
              className={activeTab === "general" ? "active" : ""}
              onClick={() => setActiveTab("general")}
            >
              <SlidersHorizontal size={16} />
              <span>General</span>
            </button>
            <button
              id="settings-transcription-tab"
              type="button"
              role="tab"
              aria-selected={activeTab === "transcription"}
              aria-controls="settings-panel"
              className={activeTab === "transcription" ? "active" : ""}
              onClick={() => setActiveTab("transcription")}
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
                  <label className="field">
                    <span>Microphone</span>
                    <select value={settings.microphone_id} onChange={(e) => setSettings({ ...settings, microphone_id: e.target.value })}>
                      <option value="">System default</option>
                      {microphones.map((microphone) => <option value={microphone.id} key={microphone.id}>{microphone.name}</option>)}
                    </select>
                  </label>

                  <label className="field">
                    <span>History retention</span>
                    <select value={settings.history_retention} onChange={(e) => setSettings({ ...settings, history_retention: e.target.value })}>
                      {["24 hours", "7 days", "30 days", "Forever"].map((value) => <option key={value}>{value}</option>)}
                    </select>
                  </label>

                  <div className="field field--wide">
                    <span>Dictation shortcut</span>
                    <button
                      className={`hotkey-capture ${capturingHotkey ? "is-capturing" : ""}`}
                      type="button"
                      aria-pressed={capturingHotkey}
                      onClick={() => {
                        setCapturingHotkey(true);
                        setHotkeyError("");
                      }}
                      onKeyDown={captureHotkey}
                      onBlur={() => {
                        setCapturingHotkey(false);
                        setHotkeyError("");
                      }}
                    >
                      <Keyboard size={16} />
                      <span>{capturingHotkey ? "Press a key…" : settings.keybind}</span>
                    </button>
                    <small>
                      {hotkeyError || (capturingHotkey
                        ? "Press Escape or click elsewhere to cancel."
                        : "Click to record a different shortcut.")}
                    </small>
                  </div>

                  <label className="toggle-row field--wide">
                    <div>
                      <span>Launch at startup</span>
                      <p>Keep Flow ready in the system tray.</p>
                    </div>
                    <input type="checkbox" checked={settings.launch_at_startup} onChange={(e) => setSettings({ ...settings, launch_at_startup: e.target.checked })} />
                  </label>
                </div>
              </>
            ) : (
              <>
                <div className="settings-pane__heading">
                  <h3>Transcription</h3>
                  <p>Choose an STT model. Groq continues to polish every transcription.</p>
                </div>

                <div className="settings-transcription">
                  <label className="field">
                    <span>Speech-to-text model</span>
                    <select
                      value={settings.transcription_model}
                      onChange={(event) => setSettings({
                        ...settings,
                        transcription_model: event.target.value as SettingsData["transcription_model"],
                      })}
                    >
                      <option value="deepgram-nova-3">Deepgram Nova-3</option>
                      <option value="groq-whisper-large-v3">Groq Whisper Large V3</option>
                    </select>
                  </label>

                  <label className="field">
                    <span>Groq API key</span>
                    <div className="secret-field">
                      <KeyRound size={16} />
                      <input
                        type={showGroqKey ? "text" : "password"}
                        value={groqApiKey}
                        autoComplete="off"
                        placeholder={settings.has_groq_api_key ? "Saved securely ••••••••" : "gsk_…"}
                        onChange={(event) => {
                          setGroqApiKey(event.target.value);
                          setGroqTested(false);
                        }}
                      />
                      <button type="button" aria-label={showGroqKey ? "Hide Groq API key" : "Show Groq API key"} onClick={() => setShowGroqKey((value) => !value)}>
                        {showGroqKey ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                    </div>
                  </label>
                  <button className="secondary-button compact" disabled={!groqApiKey || testing !== null} onClick={() => void test("groq")}>
                    {testing === "groq" ? <LoaderCircle className="spin" size={15} /> : groqTested ? <Check size={15} /> : null}
                    {testing === "groq" ? "Checking…" : groqTested ? "Connected" : "Test Groq connection"}
                  </button>

                  <label className="field">
                    <span>Deepgram API key</span>
                    <div className="secret-field">
                      <KeyRound size={16} />
                      <input
                        type={showDeepgramKey ? "text" : "password"}
                        value={deepgramApiKey}
                        autoComplete="off"
                        placeholder={settings.has_deepgram_api_key ? "Saved securely ••••••••" : "Deepgram API key"}
                        onChange={(event) => {
                          setDeepgramApiKey(event.target.value);
                          setDeepgramTested(false);
                        }}
                      />
                      <button type="button" aria-label={showDeepgramKey ? "Hide Deepgram API key" : "Show Deepgram API key"} onClick={() => setShowDeepgramKey((value) => !value)}>
                        {showDeepgramKey ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                    </div>
                  </label>
                  <button className="secondary-button compact" disabled={!deepgramApiKey || testing !== null} onClick={() => void test("deepgram")}>
                    {testing === "deepgram" ? <LoaderCircle className="spin" size={15} /> : deepgramTested ? <Check size={15} /> : null}
                    {testing === "deepgram" ? "Checking…" : deepgramTested ? "Connected" : "Test Deepgram connection"}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>

        <footer>
          <button className="secondary-button" disabled={saving} onClick={onClose}>Cancel</button>
          <button className="primary-button" disabled={!settingsLoaded || saving} onClick={() => void save()}>
            {saving && <LoaderCircle className="spin" size={16} />}
            {saving ? "Saving…" : "Save settings"}
          </button>
        </footer>
      </section>
    </div>
  );
}
