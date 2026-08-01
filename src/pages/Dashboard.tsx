import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { api } from "../api";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
import type { DashboardData } from "../types";

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
  const [retryingId, setRetryingId] = useState<number | null>(null);
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
      .then(setData)
      .catch((error) => notify({ kind: "error", message: String(error) }))
      .finally(() => setLoading(false));
  }, [notify, version]);

  const cards = [
    { label: "Total words dictated", value: number.format(data.total_words_dictated) },
    { label: "Words per minute", value: number.format(data.average_words_per_minute) },
    { label: "Time dictated this week", value: formatDuration(data.time_dictated_ms) },
    { label: "Estimated time saved this week", value: formatDuration(data.estimated_saved_ms) },
  ];

  return (
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
              return (
                <article className="pending-row" key={entry.id}>
                  <div>
                    <strong>Dictation needs attention</strong>
                    <p>{entry.text || "The recording is saved and ready to retry."}</p>
                    {entry.error && <small>{entry.error}</small>}
                  </div>
                  <div className="pending-row__actions">
                    {entry.text && (
                      <button
                        className="secondary-button compact"
                        disabled={actionsDisabled}
                        onClick={() => void api.copyText(entry.text)
                          .then(() => notify({ kind: "success", message: "Recovered text copied" }))
                          .catch((error) => notify({ kind: "error", message: String(error) }))}
                      >
                        Copy
                      </button>
                    )}
                    <button
                      className="secondary-button compact"
                      disabled={actionsDisabled}
                      onClick={() => {
                        setRetryingId(entry.id);
                        void api.retryPendingDictation(entry.id)
                          .catch((error) => notify({ kind: "error", message: String(error) }))
                          .finally(() => setRetryingId(null));
                      }}
                    >
                      Retry
                    </button>
                    <button
                      className="secondary-button compact"
                      disabled={actionsDisabled}
                      onClick={() => void api.deletePendingDictation(entry.id)
                        .then(() => setData((current) => ({
                          ...current,
                          pending: current.pending.filter((item) => item.id !== entry.id),
                        })))
                        .catch((error) => notify({ kind: "error", message: String(error) }))}
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
                className="history-row"
                key={entry.id}
                onClick={() => {
                  void api
                    .copyText(entry.text)
                    .then(() => notify({ kind: "success", message: "Copied to clipboard" }))
                    .catch((error) => notify({ kind: "error", message: String(error) }));
                }}
              >
                <time dateTime={new Date(entry.created_at * 1000).toISOString()}>
                  {relativeTime(entry.created_at)}
                </time>
                <p title={entry.text}>{entry.text}</p>
              </button>
            ))}
          </div>
        ) : null}
      </section>
    </section>
  );
}
