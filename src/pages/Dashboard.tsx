import { useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import {
  AlertCircle,
  ArrowDown,
  Check,
  CheckCircle2,
  Copy,
  Cpu,
  FileText,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { api } from "../api";
import { EmptyState } from "../components/EmptyState";
import { HistoryDetail } from "../components/HistoryDetail";
import type { ToastData } from "../components/Toast";
import type { DashboardData, HistoryEntry } from "../types";

const empty: DashboardData = {
  total_words_dictated: 0,
  average_words_per_minute: 0,
  time_dictated_ms: 0,
  estimated_saved_ms: 0,
  history: [],
  pending: [],
};

const number = new Intl.NumberFormat();

function formatDuration(ms: number) {
  if (ms <= 0) return "0s";
  const totalSeconds = Math.max(1, Math.round(ms / 1000));
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const totalMinutes = Math.round(totalSeconds / 60);
  if (totalMinutes < 60) return `${totalMinutes}m`;
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
}

function relativeTime(timestamp: number) {
  const delta = Math.max(0, Date.now() - timestamp * 1000);
  const minutes = Math.floor(delta / 60000);
  if (minutes < 1) return "Just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function Dashboard({
  version,
  keybind,
  notify,
}: {
  version: number;
  keybind: string;
  notify: (data: ToastData) => void;
}) {
  const [data, setData] = useState(empty);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMoreHistory, setHasMoreHistory] = useState(true);
  const [retryingId, setRetryingId] = useState<number | null>(null);
  const [selectedHistory, setSelectedHistory] = useState<HistoryEntry | null>(null);
  const historyRowRef = useRef<HTMLButtonElement>(null);
  const [, setNow] = useState(Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }

    setLoading(true);
    api
      .dashboard()
      .then((res) => {
        setData(res);
        setHasMoreHistory(res.history.length >= 50);
      })
      .catch((error) => notify({ kind: "error", message: String(error) }))
      .finally(() => setLoading(false));
  }, [notify, version]);

  const loadMoreHistory = async () => {
    if (loadingMore || !hasMoreHistory || data.history.length === 0) return;
    const lastItem = data.history[data.history.length - 1];
    setLoadingMore(true);
    try {
      const more = await api.historyPage(50, lastItem.created_at, lastItem.id);
      if (more.length < 50) {
        setHasMoreHistory(false);
      }
      if (more.length > 0) {
        setData((curr) => ({
          ...curr,
          history: [...curr.history, ...more],
        }));
      }
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    } finally {
      setLoadingMore(false);
    }
  };

  const handleDeleteHistoryEntry = async (e: React.MouseEvent, id: number) => {
    e.stopPropagation();
    try {
      await api.deleteHistoryEntry(id);
      setData((curr) => ({
        ...curr,
        history: curr.history.filter((h) => h.id !== id),
      }));
      notify({ kind: "success", message: "Dictation removed from history" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleCopyText = async (e: React.MouseEvent, text: string) => {
    e.stopPropagation();
    try {
      await api.copyText(text);
      notify({ kind: "success", message: "Copied to clipboard" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleAcceptPending = async (id: number) => {
    try {
      await api.acceptPendingTranscript(id);
      notify({ kind: "success", message: "Transcript accepted and delivered" });
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const cards = [
    { label: "Total words dictated", value: number.format(data.total_words_dictated) },
    { label: "Words per minute", value: number.format(data.average_words_per_minute) },
    { label: "Time dictated this week", value: formatDuration(data.time_dictated_ms) },
    { label: "Estimated time saved this week", value: formatDuration(data.estimated_saved_ms) },
  ];

  return (
    <>
      <section className="page">
        <header className="page-header">
          <div>
            <h1>Dictation</h1>
            <p>Everything you’ve said, made clearer.</p>
          </div>
        </header>

        <div className="metric-grid">
          {cards.map(({ label, value }) => (
            <article className={`metric-card ${loading ? "loading" : ""}`} key={label}>
              <strong>{value}</strong>
              <span>{label}</span>
            </article>
          ))}
        </div>

        <section className="section-block history-section" aria-label="Dictation history">
          {data.pending.length > 0 && (
            <div className="pending-list" aria-label="Recoverable dictations">
              {data.pending.map((entry) => {
                const actionsDisabled = retryingId !== null;
                const isReady = entry.stage === "ready";
                return (
                  <article className="pending-row" key={entry.id}>
                    <div>
                      <div className="flex items-center gap-2">
                        <AlertCircle className="w-4 h-4 text-amber-600 shrink-0" />
                        <strong>
                          {isReady
                            ? "Dictation ready to paste"
                            : `Dictation paused at ${entry.stage}`}
                        </strong>
                        <span className="text-[11px] px-1.5 py-0.5 rounded bg-amber-100 text-amber-800 font-medium uppercase">
                          {entry.stage}
                        </span>
                      </div>
                      <p className="mt-1 text-sm text-stone-800 font-medium">
                        {entry.text || "Recording audio preserved. Ready to transcribe."}
                      </p>
                      {entry.raw_text && entry.raw_text !== entry.text && (
                        <p className="text-xs text-stone-500 font-mono mt-0.5">
                          Raw: {entry.raw_text}
                        </p>
                      )}
                      {entry.error && (
                        <small className="text-red-700 block mt-1 font-sans">
                          {entry.error}
                        </small>
                      )}
                    </div>
                    <div className="pending-row__actions flex items-center gap-1.5 flex-wrap">
                      {isReady && (
                        <button
                          type="button"
                          className="secondary-button compact font-semibold bg-emerald-50 text-emerald-800 border-emerald-300 hover:bg-emerald-100"
                          disabled={actionsDisabled}
                          onClick={() => void handleAcceptPending(entry.id)}
                        >
                          <CheckCircle2 size={13} />
                          Deliver
                        </button>
                      )}
                      {entry.text && (
                        <button
                          type="button"
                          className="secondary-button compact"
                          disabled={actionsDisabled}
                          onClick={(e) => void handleCopyText(e, entry.text)}
                        >
                          <Copy size={13} />
                          Copy
                        </button>
                      )}
                      {entry.raw_text && entry.raw_text !== entry.text && (
                        <button
                          type="button"
                          className="secondary-button compact text-stone-600"
                          disabled={actionsDisabled}
                          onClick={(e) => void handleCopyText(e, entry.raw_text!)}
                        >
                          Copy Raw
                        </button>
                      )}
                      <button
                        type="button"
                        className="secondary-button compact"
                        disabled={actionsDisabled}
                        onClick={() => {
                          setRetryingId(entry.id);
                          void api
                            .retryPendingDictation(entry.id)
                            .catch((error) => notify({ kind: "error", message: String(error) }))
                            .finally(() => setRetryingId(null));
                        }}
                      >
                        <RotateCcw size={13} />
                        Retry
                      </button>
                      <button
                        type="button"
                        className="secondary-button compact text-stone-500 hover:text-red-700"
                        disabled={actionsDisabled}
                        onClick={() =>
                          void api
                            .deletePendingDictation(entry.id)
                            .then(() =>
                              setData((current) => ({
                                ...current,
                                pending: current.pending.filter((item) => item.id !== entry.id),
                              })),
                            )
                            .catch((error) => notify({ kind: "error", message: String(error) }))
                        }
                      >
                        Discard
                      </button>
                    </div>
                  </article>
                );
              })}
            </div>
          )}

          {data.history.length === 0 && data.pending.length === 0 ? (
            <EmptyState
              title="Your words will land here"
              description={`Press ${keybind} anywhere to start your first dictation.`}
            />
          ) : data.history.length > 0 ? (
            <div className="history-list">
              {data.history.map((entry) => (
                <button
                  ref={historyRowRef}
                  className="history-row group relative text-left w-full"
                  key={entry.id}
                  onClick={() => setSelectedHistory(entry)}
                >
                  <div className="flex items-center justify-between w-full mb-1">
                    <time
                      className="text-xs text-stone-500"
                      dateTime={new Date(entry.created_at * 1000).toISOString()}
                    >
                      {relativeTime(entry.created_at)}
                    </time>
                    <div className="flex items-center gap-2">
                      {(entry.input_tokens || entry.output_tokens) && (
                        <span
                          className="flex items-center gap-1 text-[11px] text-stone-400"
                          title="Tokens consumed"
                        >
                          <Cpu size={11} />
                          {entry.input_tokens ?? 0}/{entry.output_tokens ?? 0}
                        </span>
                      )}
                      {entry.delivery_mode === "copy" && (
                        <span className="text-[10px] px-1.5 py-0.5 rounded bg-stone-200/70 text-stone-600 font-medium">
                          Copied
                        </span>
                      )}
                      <button
                        type="button"
                        className="opacity-0 group-hover:opacity-100 p-1 text-stone-400 hover:text-stone-700 rounded transition-opacity"
                        title="Copy text"
                        onClick={(e) => void handleCopyText(e, entry.text)}
                      >
                        <Copy size={13} />
                      </button>
                      <button
                        type="button"
                        className="opacity-0 group-hover:opacity-100 p-1 text-stone-400 hover:text-red-600 rounded transition-opacity"
                        title="Delete entry"
                        onClick={(e) => void handleDeleteHistoryEntry(e, entry.id)}
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  </div>
                  <p className="line-clamp-2 text-stone-800" title={entry.text}>
                    {entry.text}
                  </p>
                </button>
              ))}

              {hasMoreHistory && (
                <div className="pt-3 pb-1 flex justify-center">
                  <button
                    type="button"
                    disabled={loadingMore}
                    onClick={() => void loadMoreHistory()}
                    className="secondary-button compact text-xs flex items-center gap-1.5"
                  >
                    <ArrowDown size={13} />
                    {loadingMore ? "Loading older history…" : "Load older dictations"}
                  </button>
                </div>
              )}
            </div>
          ) : null}
        </section>
      </section>

      <HistoryDetail
        entry={selectedHistory}
        onClose={() => setSelectedHistory(null)}
        returnFocusRef={historyRowRef}
      />
    </>
  );
}
