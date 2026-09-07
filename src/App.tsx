import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  BookOpen,
  Command,
  Copy,
  Mic,
  Minus,
  SlidersHorizontal,
  Square,
  X,
} from "lucide-react";
import { api } from "./api";
import { Logo } from "./components/Logo";
import { SettingsModal } from "./components/SettingsModal";
import { PrivacyNoticeModal } from "./components/PrivacyNoticeModal";
import { Toast, type ToastData } from "./components/Toast";
import { useWorkflowState } from "./hooks/useWorkflowState";
import { Dashboard } from "./pages/Dashboard";
import { Dictionary } from "./pages/Dictionary";
import { Snippets } from "./pages/Snippets";
import type { Page } from "./types";

const navigation = [
  { id: "dashboard" as const, label: "Dictation", icon: Mic },
  { id: "dictionary" as const, label: "Dictionary", icon: BookOpen },
  { id: "snippets" as const, label: "Snippets", icon: Command },
];

function TitleBar() {
  const appWindow = isTauri() ? getCurrentWindow() : null;
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!appWindow) return;

    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refreshMaximized = async () => {
      const next = await appWindow.isMaximized();
      if (!disposed) setMaximized(next);
    };

    void refreshMaximized();
    void appWindow.onResized(refreshMaximized).then((stopListening) => {
      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

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
            aria-label={maximized ? "Restore" : "Maximize"}
            title={maximized ? "Restore" : "Maximize"}
            onClick={() => void appWindow.toggleMaximize()}
          >
            {maximized
              ? <Copy size={13} strokeWidth={1.55} />
              : <Square size={13} strokeWidth={1.55} />}
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
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const [toast, setToast] = useState<ToastData | null>(null);
  const [dashboardVersion, setDashboardVersion] = useState(0);
  const [keybind, setKeybind] = useState("Right Alt");
  const toastTimer = useRef<number | null>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const privacyTriggerRef = useRef<HTMLElement>(null);

  const workflow = useWorkflowState();

  const notify = useCallback((data: ToastData) => {
    if (toastTimer.current !== null) {
      window.clearTimeout(toastTimer.current);
    }
    setToast(data);
    if (data.kind === "success") {
      toastTimer.current = window.setTimeout(() => {
        setToast(null);
        toastTimer.current = null;
      }, 3500);
    } else {
      toastTimer.current = null;
    }
  }, []);

  useEffect(() => {
    const hasSeenNotice = localStorage.getItem("flow_seen_privacy_notice") === "true";
    if (!hasSeenNotice) {
      setPrivacyOpen(true);
    }

    if (!isTauri()) {
      return () => {
        if (toastTimer.current !== null) {
          window.clearTimeout(toastTimer.current);
        }
      };
    }

    void api
      .settings()
      .then((settings) => {
        setKeybind(settings.keybind);
        if (settings.has_seen_privacy_notice) {
          localStorage.setItem("flow_seen_privacy_notice", "true");
          setPrivacyOpen(false);
        }
      })
      .catch((error) => notify({ kind: "error", message: String(error) }));

    const unlisten = listen<{ message: string }>("dictation-complete", (event) => {
      setDashboardVersion((version) => version + 1);
      notify({ kind: "success", message: event.payload.message });
    });
    const unlistenError = listen<{ message: string }>("flow-error", (event) => {
      setDashboardVersion((version) => version + 1);
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

  const handleAcceptPrivacy = () => {
    localStorage.setItem("flow_seen_privacy_notice", "true");
    setPrivacyOpen(false);
  };

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
            <button ref={settingsButtonRef} onClick={() => setSettingsOpen(true)}>
              <SlidersHorizontal size={18} strokeWidth={1.75} />
              <span>Settings</span>
            </button>
            <div className="shortcut-hint">
              <span>{workflow.isRecording ? "Recording…" : workflow.isProcessing ? "Polishing…" : "Start dictating"}</span>
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
          returnFocusRef={settingsButtonRef}
          notify={notify}
          onSaved={(savedKeybind) => {
            setKeybind(savedKeybind);
            setDashboardVersion((v) => v + 1);
          }}
        />
      )}

      <PrivacyNoticeModal
        open={privacyOpen}
        onAccept={handleAcceptPrivacy}
        returnFocusRef={privacyTriggerRef}
      />

      {toast && <Toast data={toast} onClose={() => setToast(null)} />}
    </div>
  );
}
