import { useEffect, useRef, useState, type RefObject } from "react";
import { isTauri } from "@tauri-apps/api/core";
import {
  AlertCircle,
  AlertTriangle,
  ArrowDown,
  Check,
  Copy,
  RotateCcw,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { api } from "../api";
import { Dialog } from "../components/Dialog";
import { EmptyState } from "../components/EmptyState";
import { HistoryDetail } from "../components/HistoryDetail";
import { useWorkflowState } from "../hooks/useWorkflowState";
import type { ToastData } from "../components/Toast";
import type { DashboardData, HistoryEntry, PendingDictation } from "../types";

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

function SuspectReviewModal({
  entry,
  onClose,
  onAccept,
  onRetranscribe,
  returnFocusRef,
}: {
  entry: PendingDictation | null;
  onClose: () => void;
  onAccept: (id: number) => void;
  onRetranscribe: (id: number) => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}) {
  if (!entry) return null;
  return (
    <Dialog
      open={Boolean(entry)}
      onClose={onClose}
      title="Review Suspect Dictation"
      returnFocusRef={returnFocusRef}
      size="lg"
    >
      <div>
        <div className="privacy-banner">
          <AlertCircle size={20} />
          <span>
            The speech model flagged this transcription as low confidence or missing confidence metadata. Please review the raw transcript before proceeding.
          </span>
        </div>
        <div className="detail-section">
          <label className="detail-section-label">Raw Dictation Text</label>
          <div className="detail-text-box detail-text-box--raw">
            {entry.raw_text || entry.text}
          </div>
        </div>
        <div className="modal-footer" style={{ padding: "16px 0 0", borderTop: "1px solid var(--border-soft)" }}>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="secondary-button"
            onClick={() => {
              onClose();
              onRetranscribe(entry.id);
            }}
          >
            <RotateCcw size={14} />
            Retranscribe
          </button>
          <button
            type="button"
            className="primary-button"
            onClick={() => {
              onClose();
              onAccept(entry.id);
            }}
          >
            <Check size={14} />
            Accept & Polish
          </button>
        </div>
      </div>
    </Dialog>
  );
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
  const [reviewingPending, setReviewingPending] = useState<PendingDictation | null>(null);
  const lastClickedTriggerRef = useRef<HTMLElement | null>(null);
  const loadGenerationRef = useRef(0);
  const [, setNow] = useState(Date.now());

  const workflow = useWorkflowState();

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const refreshDashboard = () => {
    if (!isTauri()) {
      setLoading(false);
      return;
    }
    const currentGen = ++loadGenerationRef.current;
    setLoading(true);
    api
      .dashboard()
      .then((res) => {
        if (currentGen === loadGenerationRef.current) {
          setData(res);
          setHasMoreHistory(res.history.length >= 50);
        }
      })
      .catch((error) => {
        if (currentGen === loadGenerationRef.current) {
          notify({ kind: "error", message: String(error) });
        }
      })
      .finally(() => {
        if (currentGen === loadGenerationRef.current) {
          setLoading(false);
        }
      });
  };

  useEffect(() => {
    refreshDashboard();
  }, [notify, version]);

  // Refresh when workflow returns to idle
  useEffect(() => {
    if (workflow.phase === "idle") {
      refreshDashboard();
    }
  }, [workflow.phase]);

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
      notify({ kind: "success", message: "Transcript accepted and processing" });
      refreshDashboard();
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    }
  };

  const handleRetranscribePending = async (id: number) => {
    try {
      setRetryingId(id);
      await api.retryPendingTranscription(id);
      notify({ kind: "success", message: "Retranscription started" });
      refreshDashboard();
    } catch (err) {
      notify({ kind: "error", message: String(err) });
    } finally {
      setRetryingId(null);
    }
  };

  const handleDiscardPending = async (id: number) => {
    if (!window.confirm("Discard this dictation permanently?")) return;
    try {
      await api.deletePendingDictation(id);
      setData((curr) => ({
        ...curr,
        pending: curr.pending.filter((p) => p.id !== id),
      }));
      notify({ kind: "success", message: "Dictation discarded" });
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

  const isRecording = workflow.phase === "recording";
  const isBusy = workflow.phase !== "idle" && workflow.phase !== "faulted";

  return (
    <>
      <section className="page">
        <header className="page-header">
          <div>
            <h1>Dictation</h1>
            <p>Everything you’ve said, made clearer.</p>
          </div>
          <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
            {isRecording && (
              <button
                type="button"
                className="primary-button"
                onClick={() => void api.stopRecording().catch((e) => notify({ kind: "error", message: String(e) }))}
              >
                <Square size={14} />
                Stop Recording
              </button>
            )}
            {isBusy && (
              <button
                type="button"
                className="secondary-button"
                onClick={() => void api.cancelRecording().catch((e) => notify({ kind: "error", message: String(e) }))}
              >
                <X size={14} />
                Cancel
              </button>
            )}
          </div>
        </header>

        {workflow.phase === "faulted" && (
          <div
            className="privacy-banner"
            style={{
              background: "#fde8e8",
              borderColor: "#f8b4b4",
              color: "#99281e",
              marginBottom: "20px",
            }}
          >
            <AlertTriangle size={20} />
            <span>
              The microphone stream encountered an unrecoverable error. Please restart Flow to reconnect audio.
            </span>
          </div>
        )}

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
                const isRowActive =
                  retryingId === entry.id || workflow.active_pending_id === entry.id;
                const actionsDisabled = isRowActive || (workflow.active_pending_id !== null && workflow.active_pending_id !== entry.id);
                const isReady = entry.stage === "ready";
                const isSuspect = entry.review_reason === "suspect_speech";
                const isPartial = entry.partial || entry.review_reason === "partial_capture";

                return (
                  <article className="pending-row" key={entry.id}>
                    <div>
                      <div className="pending-row-header">
                        {isSuspect ? (
                          <AlertTriangle size={15} style={{ color: "#b25e1d" }} />
                        ) : (
                          <AlertCircle size={15} style={{ color: "var(--muted)" }} />
                        )}
                        <strong>
                          {isReady
                            ? "Dictation ready"
                            : isSuspect
                            ? "Review required: suspect transcript"
                            : entry.no_content
                            ? "No speech detected"
                            : `Dictation paused at ${entry.stage}`}
                        </strong>
                        <span
                          className={`pending-badge ${
                            isReady
                              ? "pending-badge--ready"
                              : isSuspect
                              ? "pending-badge--review"
                              : "pending-badge--paused"
                          }`}
                        >
                          {entry.stage}
                        </span>
                        {isPartial && (
                          <span className="pending-badge pending-badge--review">
                            partial
                          </span>
                        )}
                      </div>
                      <p className="pending-row-text">
                        {entry.text || entry.raw_text || "Audio preserved in recovery spool. Ready to process."}
                      </p>
                      {entry.raw_text && entry.raw_text !== entry.text && (
                        <p className="pending-row-raw">
                          Raw: {entry.raw_text}
                        </p>
                      )}
                      {entry.error && (
                        <small className="pending-row-error">
                          {entry.error}
                        </small>
                      )}
                    </div>
                    <div className="pending-row__actions">
                      {isSuspect && (
                        <button
                          type="button"
                          className="secondary-button compact"
                          disabled={actionsDisabled}
                          onClick={(e) => {
                            lastClickedTriggerRef.current = e.currentTarget;
                            setReviewingPending(entry);
                          }}
                        >
                          Review & Accept
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
                          {isReady ? "Copy Final" : "Copy"}
                        </button>
                      )}
                      {entry.raw_text && (entry.raw_text !== entry.text || !entry.text) && (
                        <button
                          type="button"
                          className="secondary-button compact"
                          disabled={actionsDisabled}
                          onClick={(e) => void handleCopyText(e, entry.raw_text!)}
                        >
                          Copy Raw
                        </button>
                      )}
                      {!isReady && !entry.no_content && (
                        <button
                          type="button"
                          className="secondary-button compact"
                          disabled={actionsDisabled}
                          onClick={() => {
                            if (isPartial && !window.confirm("This dictation is partial or was interrupted. Retry processing?")) {
                              return;
                            }
                            setRetryingId(entry.id);
                            void api
                              .retryPendingDictation(entry.id)
                              .then(refreshDashboard)
                              .catch((error) => notify({ kind: "error", message: String(error) }))
                              .finally(() => setRetryingId(null));
                          }}
                        >
                          <RotateCcw size={13} />
                          Retry
                        </button>
                      )}
                      <button
                        type="button"
                        className="secondary-button compact"
                        style={{ color: "#99281e" }}
                        disabled={actionsDisabled}
                        onClick={() => void handleDiscardPending(entry.id)}
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
                <div className="history-item" key={entry.id}>
                  <button
                    type="button"
                    className="history-item__trigger"
                    onClick={(e) => {
                      lastClickedTriggerRef.current = e.currentTarget;
                      setSelectedHistory(entry);
                    }}
                  >
                    <time dateTime={new Date(entry.created_at * 1000).toISOString()}>
                      {relativeTime(entry.created_at)}
                    </time>
                    <p title={entry.text}>{entry.text}</p>
                  </button>
                  <div className="history-item__actions">
                    <button
                      type="button"
                      className="icon-button"
                      title="Copy text"
                      onClick={(e) => void handleCopyText(e, entry.text)}
                    >
                      <Copy size={13} />
                    </button>
                    <button
                      type="button"
                      className="icon-button"
                      title="Delete entry"
                      onClick={(e) => void handleDeleteHistoryEntry(e, entry.id)}
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
              ))}

              {hasMoreHistory && (
                <div style={{ padding: "14px 0 6px", display: "flex", justifyContent: "center" }}>
                  <button
                    type="button"
                    disabled={loadingMore}
                    onClick={() => void loadMoreHistory()}
                    className="secondary-button compact"
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
        returnFocusRef={lastClickedTriggerRef}
      />

      <SuspectReviewModal
        entry={reviewingPending}
        onClose={() => setReviewingPending(null)}
        onAccept={(id) => void handleAcceptPending(id)}
        onRetranscribe={(id) => void handleRetranscribePending(id)}
        returnFocusRef={lastClickedTriggerRef}
      />
    </>
  );
}
