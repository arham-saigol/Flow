import { useEffect, useState } from "react";
import { Clipboard, Clock3, History as HistoryIcon, MessageSquareText, Timer, Type } from "lucide-react";
import { api } from "../api";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
import type { DashboardData } from "../types";

const empty: DashboardData = {
  words_this_week: 0,
  dictations_this_week: 0,
  time_dictated_ms: 0,
  estimated_saved_ms: 0,
  history: [],
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

  useEffect(() => {
    setLoading(true);
    api
      .dashboard()
      .then(setData)
      .catch((error) => notify({ kind: "error", message: String(error) }))
      .finally(() => setLoading(false));
  }, [notify, version]);

  const cards = [
    { label: "Words this week", value: number.format(data.words_this_week), icon: Type },
    { label: "Dictations this week", value: number.format(data.dictations_this_week), icon: MessageSquareText },
    { label: "Time dictated", value: formatDuration(data.time_dictated_ms), icon: Clock3 },
    { label: "Estimated time saved", value: formatDuration(data.estimated_saved_ms), icon: Timer },
  ];

  return (
    <section className="page">
      <header className="page-header">
        <div>
          <h1>Dashboard</h1>
          <p>Everything you’ve said, made clearer.</p>
        </div>
      </header>

      <div className="metric-grid">
        {cards.map(({ label, value, icon: Icon }) => (
          <article className={`metric-card ${loading ? "loading" : ""}`} key={label}>
            <div className="metric-card__icon"><Icon size={18} strokeWidth={1.65} /></div>
            <div>
              <span>{label}</span>
              <strong>{value}</strong>
            </div>
          </article>
        ))}
      </div>

      <section className="section-block history-section">
        <div className="section-heading">
          <div>
            <h2>History</h2>
            <p>Click any entry to copy the complete text.</p>
          </div>
          {data.history.length > 0 && (
            <span className="count-pill">{data.history.length}</span>
          )}
        </div>
        {data.history.length === 0 ? (
          <EmptyState
            icon={HistoryIcon}
            title="Your words will land here"
            description={`Press ${keybind} anywhere to start your first dictation.`}
          />
        ) : (
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
                <div className="history-row__copy"><Clipboard size={15} /></div>
                <div className="history-row__body">
                  <p>{entry.text}</p>
                  <div>
                    <span>{relativeTime(entry.created_at)}</span>
                    <i />
                    <span>{entry.word_count} words</span>
                    <i />
                    <span>{formatDuration(entry.duration_ms)}</span>
                  </div>
                </div>
              </button>
            ))}
          </div>
        )}
      </section>
    </section>
  );
}
