import { useState, type RefObject } from "react";
import { Copy, Check, Clock, FileText, Cpu, ArrowRight } from "lucide-react";
import { Dialog } from "./Dialog";
import { api } from "../api";
import type { HistoryEntry } from "../types";

interface HistoryDetailProps {
  entry: HistoryEntry | null;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLElement>;
}

export function HistoryDetail({
  entry,
  onClose,
  returnFocusRef,
}: HistoryDetailProps) {
  const [copiedRaw, setCopiedRaw] = useState(false);
  const [copiedFinal, setCopiedFinal] = useState(false);

  if (!entry) return null;

  const handleCopyRaw = async () => {
    try {
      await api.copyText(entry.raw_text);
      setCopiedRaw(true);
      setTimeout(() => setCopiedRaw(false), 2000);
    } catch (e) {
      console.error(e);
    }
  };

  const handleCopyFinal = async () => {
    try {
      await api.copyText(entry.text);
      setCopiedFinal(true);
      setTimeout(() => setCopiedFinal(false), 2000);
    } catch (e) {
      console.error(e);
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
      maxWidthClass="max-w-2xl"
    >
      <div className="space-y-5">
        {/* Metadata Badges */}
        <div className="flex flex-wrap items-center gap-3 text-xs text-stone-600 bg-stone-100/60 p-3 rounded-xl border border-stone-200/60">
          <div className="flex items-center gap-1.5">
            <Clock className="w-3.5 h-3.5 text-stone-400" />
            <span>{formattedDate}</span>
          </div>
          <span>•</span>
          <div>
            <span>{entry.word_count} words ({durationSec}s)</span>
          </div>
          {entry.delivery_mode && (
            <>
              <span>•</span>
              <div className="px-2 py-0.5 rounded bg-stone-200/60 text-stone-700 font-medium">
                {entry.delivery_mode === "paste" ? "Direct Paste" : "Copied to Clipboard"}
              </div>
            </>
          )}
          {(entry.input_tokens || entry.output_tokens) && (
            <>
              <span>•</span>
              <div className="flex items-center gap-1">
                <Cpu className="w-3.5 h-3.5 text-stone-400" />
                <span>
                  {entry.input_tokens ?? 0} in / {entry.output_tokens ?? 0} out tokens
                </span>
              </div>
            </>
          )}
        </div>

        {/* Polished Text Section */}
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <label className="text-xs font-semibold uppercase tracking-wider text-stone-700 flex items-center gap-1.5">
              <FileText className="w-3.5 h-3.5 text-emerald-600" />
              Polished Final Text
            </label>
            <button
              type="button"
              onClick={handleCopyFinal}
              className="flex items-center gap-1.5 text-xs font-medium text-stone-600 hover:text-stone-900 px-2.5 py-1 rounded-lg border border-stone-200 bg-white hover:bg-stone-50 transition-colors shadow-2xs"
            >
              {copiedFinal ? (
                <>
                  <Check className="w-3.5 h-3.5 text-emerald-600" />
                  Copied
                </>
              ) : (
                <>
                  <Copy className="w-3.5 h-3.5" />
                  Copy Final
                </>
              )}
            </button>
          </div>
          <div className="p-3.5 bg-white border border-stone-200/90 rounded-xl text-stone-900 leading-relaxed font-sans text-sm select-text">
            {entry.text}
          </div>
        </div>

        {/* Raw Speech-to-Text Section */}
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <label className="text-xs font-semibold uppercase tracking-wider text-stone-600 flex items-center gap-1.5">
              <ArrowRight className="w-3.5 h-3.5 text-amber-600" />
              Raw Whisper Transcript
            </label>
            <button
              type="button"
              onClick={handleCopyRaw}
              className="flex items-center gap-1.5 text-xs font-medium text-stone-600 hover:text-stone-900 px-2.5 py-1 rounded-lg border border-stone-200 bg-white hover:bg-stone-50 transition-colors shadow-2xs"
            >
              {copiedRaw ? (
                <>
                  <Check className="w-3.5 h-3.5 text-emerald-600" />
                  Copied
                </>
              ) : (
                <>
                  <Copy className="w-3.5 h-3.5" />
                  Copy Raw
                </>
              )}
            </button>
          </div>
          <div className="p-3 bg-stone-100/70 border border-stone-200/70 rounded-xl text-stone-600 font-mono text-xs leading-relaxed select-text">
            {entry.raw_text}
          </div>
        </div>
      </div>
    </Dialog>
  );
}
