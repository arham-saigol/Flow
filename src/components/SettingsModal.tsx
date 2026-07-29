import { useEffect, useState } from "react";
import { Check, Eye, EyeOff, KeyRound, LoaderCircle, X } from "lucide-react";
import { api } from "../api";
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

export function SettingsModal({
  onClose,
  notify,
  onSaved,
}: {
  onClose: () => void;
  notify: (data: ToastData) => void;
  onSaved: (keybind: string) => void;
}) {
  const [settings, setSettings] = useState(defaults);
  const [microphones, setMicrophones] = useState<Microphone[]>([]);
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [tested, setTested] = useState(false);
  const [saving, setSaving] = useState(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);

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
        { ...settings, microphone_name: selected?.name ?? settings.microphone_name },
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

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <section className="settings-modal" role="dialog" aria-modal="true" aria-labelledby="settings-title">
        <header>
          <div>
            <p className="eyebrow">Preferences</p>
            <h2 id="settings-title">Settings</h2>
          </div>
          <button className="icon-button" aria-label="Close" onClick={onClose}><X size={18} /></button>
        </header>

        <div className="settings-body">
          <div className="settings-group">
            <div className="settings-group__heading">
              <h3>Groq</h3>
              <p>Used for transcription and writing cleanup.</p>
            </div>
            <label className="field">
              <span>API key</span>
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

          <div className="settings-group settings-grid">
            <label className="field">
              <span>Microphone</span>
              <select value={settings.microphone_id} onChange={(e) => setSettings({ ...settings, microphone_id: e.target.value })}>
                <option value="">System default</option>
                {microphones.map((microphone) => <option value={microphone.id} key={microphone.id}>{microphone.name}</option>)}
              </select>
            </label>
            <label className="field">
              <span>Keybind</span>
              <select value={settings.keybind} onChange={(e) => setSettings({ ...settings, keybind: e.target.value })}>
                {["Right Alt", "Left Alt", "Right Ctrl", "F8", "F9", "F10", "F11", "F12"].map((key) => <option key={key}>{key}</option>)}
              </select>
            </label>
            <label className="field">
              <span>History retention</span>
              <select value={settings.history_retention} onChange={(e) => setSettings({ ...settings, history_retention: e.target.value })}>
                {["24 hours", "7 days", "30 days", "Forever"].map((value) => <option key={value}>{value}</option>)}
              </select>
            </label>
          </div>

          <div className="settings-group toggle-list">
            <label className="toggle-row">
              <div><span>Launch at startup</span><p>Keep Flow ready in the system tray.</p></div>
              <input type="checkbox" checked={settings.launch_at_startup} onChange={(e) => setSettings({ ...settings, launch_at_startup: e.target.checked })} />
            </label>
          </div>
        </div>

        <footer>
          <button className="secondary-button" onClick={onClose}>Cancel</button>
          <button className="primary-button" disabled={!settingsLoaded || saving} onClick={() => void save()}>
            {saving && <LoaderCircle className="spin" size={16} />}
            {saving ? "Saving…" : "Save settings"}
          </button>
        </footer>
      </section>
    </div>
  );
}
