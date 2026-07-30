import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  BookOpen,
  Command,
  Gauge,
  Minus,
  Settings,
  Square,
  X,
} from "lucide-react";
import { api } from "./api";
import { Logo } from "./components/Logo";
import { SettingsModal } from "./components/SettingsModal";
import { Toast, type ToastData } from "./components/Toast";
import { Dashboard } from "./pages/Dashboard";
import { Dictionary } from "./pages/Dictionary";
import { Snippets } from "./pages/Snippets";
import type { Page } from "./types";

const navigation = [
  { id: "dashboard" as const, label: "Dashboard", icon: Gauge },
  { id: "dictionary" as const, label: "Dictionary", icon: BookOpen },
  { id: "snippets" as const, label: "Snippets", icon: Command },
];

function TitleBar() {
  const appWindow = isTauri() ? getCurrentWindow() : null;

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar__drag-area" data-tauri-drag-region />
      {appWindow && (
        <div className="window-controls" aria-label="Window controls">
          <button
            type="button"
            aria-label="Minimize"
            title="Minimize"
            onClick={() => void appWindow.minimize()}
          >
            <Minus size={17} strokeWidth={1.6} />
          </button>
          <button
            type="button"
            aria-label="Maximize"
            title="Maximize"
            onClick={() => void appWindow.toggleMaximize()}
          >
            <Square size={13} strokeWidth={1.55} />
          </button>
          <button
            type="button"
            className="window-controls__close"
            aria-label="Close"
            title="Close"
            onClick={() => void appWindow.close()}
          >
            <X size={18} strokeWidth={1.55} />
          </button>
        </div>
      )}
    </header>
  );
}

export default function App() {
  const [page, setPage] = useState<Page>("dashboard");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [toast, setToast] = useState<ToastData | null>(null);
  const [dashboardVersion, setDashboardVersion] = useState(0);
  const [keybind, setKeybind] = useState("Right Alt");
  const toastTimer = useRef<number | null>(null);

  const notify = useCallback((data: ToastData) => {
    if (toastTimer.current !== null) {
      window.clearTimeout(toastTimer.current);
    }
    setToast(data);
    toastTimer.current = window.setTimeout(() => {
      setToast(null);
      toastTimer.current = null;
    }, 3500);
  }, []);

  useEffect(() => {
    if (!isTauri()) {
      return () => {
        if (toastTimer.current !== null) {
          window.clearTimeout(toastTimer.current);
        }
      };
    }

    void api
      .settings()
      .then((settings) => setKeybind(settings.keybind))
      .catch((error) => notify({ kind: "error", message: String(error) }));
    const unlisten = listen<{ message: string }>("dictation-complete", () => {
      setDashboardVersion((version) => version + 1);
      notify({ kind: "success", message: "Dictation pasted" });
    });
    const unlistenError = listen<{ message: string }>("flow-error", (event) => {
      notify({ kind: "error", message: event.payload.message });
    });
    const unlistenWarning = listen<{ message: string }>("flow-warning", (event) => {
      notify({ kind: "error", message: event.payload.message });
    });
    return () => {
      void unlisten.then((fn) => fn());
      void unlistenError.then((fn) => fn());
      void unlistenWarning.then((fn) => fn());
      if (toastTimer.current !== null) {
        window.clearTimeout(toastTimer.current);
      }
    };
  }, [notify]);

  return (
    <div className="app-shell">
      <TitleBar />
      <div className="app-frame">
        <aside className="sidebar">
          <div className="sidebar__brand">
            <Logo />
          </div>
          <nav aria-label="Primary">
            {navigation.map(({ id, label, icon: Icon }) => (
              <button
                key={id}
                className={page === id ? "active" : ""}
                onClick={() => setPage(id)}
              >
                <Icon size={18} strokeWidth={1.75} />
                <span>{label}</span>
              </button>
            ))}
          </nav>
          <div className="sidebar__bottom">
            <button onClick={() => setSettingsOpen(true)}>
              <Settings size={18} strokeWidth={1.75} />
              <span>Settings</span>
            </button>
            <div className="shortcut-hint">
              <span>Start dictating</span>
              <kbd>{keybind}</kbd>
            </div>
          </div>
        </aside>

        <main className="main-content">
          {page === "dashboard" && (
            <Dashboard version={dashboardVersion} keybind={keybind} notify={notify} />
          )}
          {page === "dictionary" && <Dictionary notify={notify} />}
          {page === "snippets" && <Snippets notify={notify} />}
        </main>
      </div>

      {settingsOpen && (
        <SettingsModal
          onClose={() => setSettingsOpen(false)}
          notify={notify}
          onSaved={(savedKeybind) => {
            setKeybind(savedKeybind);
            void api.dashboard().then(() => setDashboardVersion((v) => v + 1));
          }}
        />
      )}
      {toast && <Toast data={toast} onClose={() => setToast(null)} />}
    </div>
  );
}
