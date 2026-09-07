import { useState, type RefObject } from "react";
import { Copy, Check, Clock, FileText, ArrowRight, AlertTriangle } from "lucide-react";
import { Dialog } from "./Dialog";
import { api } from "../api";
import type { HistoryEntry } from "../types";

interface HistoryDetailProps {
  entry: HistoryEntry | null;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

export function HistoryDetail({
  entry,
  onClose,
  returnFocusRef,
}: HistoryDetailProps) {
  const [copiedRaw, setCopiedRaw] = useState(false);
  const [copiedFinal, setCopiedFinal] = useState(false);
  const [copyError, setCopyError] = useState<string | null>(null);

  if (!entry) return null;

  const handleCopyRaw = async () => {
    try {
      setCopyError(null);
      await api.copyText(entry.raw_text);
      setCopiedRaw(true);
      setTimeout(() => setCopiedRaw(false), 2000);
    } catch (e) {
      setCopyError(`Could not copy raw text: ${e}`);
    }
  };

  const handleCopyFinal = async () => {
    try {
      setCopyError(null);
      await api.copyText(entry.text);
      setCopiedFinal(true);
      setTimeout(() => setCopiedFinal(false), 2000);
    } catch (e) {
      setCopyError(`Could not copy polished text: ${e}`);
    }
  };

  const formattedDate = new Date(entry.created_at * 1000).toLocaleString();
  const durationSec = (entry.duration_ms / 1000).toFixed(1);

  return (
    <Dialog
      open={Boolean(entry)}
      onClose={onClose}
      title="Dictation Details"
      returnFocusRef={returnFocusRef}
      size="lg"
    >
      <div>
        {/* Metadata Badges */}
        <div className="detail-meta-bar">
          <div className="detail-meta-item">
            <Clock size={14} />
            <span>{formattedDate}</span>
          </div>
          <span>•</span>
          <div>
            <span>{entry.word_count} words ({durationSec}s)</span>
          </div>
          {entry.delivery_outcome && (
            <>
              <span>•</span>
              <div className="detail-tag">
                {entry.delivery_outcome === "pasted"
                  ? "Direct Paste"
                  : entry.delivery_outcome === "copied"
                  ? "Copied to Clipboard"
                  : entry.delivery_outcome}
              </div>
            </>
          )}
          {entry.delivery_warning && (
            <div className="detail-tag detail-tag--warning flex items-center gap-1">
              <AlertTriangle size={12} />
              <span>{entry.delivery_warning}</span>
            </div>
          )}
        </div>

        {copyError && (
          <div
            style={{
              padding: "8px 12px",
              marginBottom: "14px",
              borderRadius: "8px",
              background: "#fde8e8",
              color: "#99281e",
              fontSize: "12px",
            }}
          >
            {copyError}
          </div>
        )}

        {/* Polished Text Section */}
        <div className="detail-section">
          <div className="detail-section-header">
            <label className="detail-section-label">
              <FileText size={14} style={{ color: "var(--accent)" }} />
              Polished Final Text
            </label>
            <button
              type="button"
              onClick={handleCopyFinal}
              className="secondary-button compact"
            >
              {copiedFinal ? (
                <>
                  <Check size={13} />
                  Copied
                </>
              ) : (
                <>
                  <Copy size={13} />
                  Copy Final
                </>
              )}
            </button>
          </div>
          <div className="detail-text-box">{entry.text}</div>
        </div>

        {/* Raw Speech-to-Text Section */}
        <div className="detail-section">
          <div className="detail-section-header">
            <label className="detail-section-label">
              <ArrowRight size={14} />
              Raw Whisper Transcript
            </label>
            <button
              type="button"
              onClick={handleCopyRaw}
              className="secondary-button compact"
            >
              {copiedRaw ? (
                <>
                  <Check size={13} />
                  Copied
                </>
              ) : (
                <>
                  <Copy size={13} />
                  Copy Raw
                </>
              )}
            </button>
          </div>
          <div className="detail-text-box detail-text-box--raw">
            {entry.raw_text}
          </div>
        </div>
      </div>
    </Dialog>
  );
}
