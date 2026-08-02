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
  has_api_key: false,
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
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState(false);
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
                      <button type="button" aria-label={showKey ? "Hide API key" : "Show API key"} onClick={() => setShowKey((value) => !value)}>
                        {showKey ? <EyeOff size={16} /> : <Eye size={16} />}
                      </button>
                    </div>
                  </label>
                  <button className="secondary-button compact" disabled={!apiKey || testing} onClick={() => void test()}>
                    {testing ? <LoaderCircle className="spin" size={15} /> : tested ? <Check size={15} /> : null}
                    {testing ? "Checking…" : tested ? "Connected" : "Test connection"}
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
