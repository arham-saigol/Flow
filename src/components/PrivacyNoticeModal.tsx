import { type RefObject } from "react";
import { ShieldCheck, Lock, Server, HardDrive, CheckCircle2 } from "lucide-react";
import { Dialog } from "./Dialog";

interface PrivacyNoticeModalProps {
  open: boolean;
  onAccept: () => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

export function PrivacyNoticeModal({
  open,
  onAccept,
  returnFocusRef,
}: PrivacyNoticeModalProps) {
  return (
    <Dialog
      open={open}
      onClose={() => {}}
      title="Privacy & Data Processing Notice"
      returnFocusRef={returnFocusRef}
      size="lg"
    >
      <div className="privacy-notice">
        <div className="privacy-banner">
          <ShieldCheck size={20} />
          <span>
            Please review how Flow processes your speech before dictating.
          </span>
        </div>

        <ul className="privacy-list">
          <li className="privacy-item">
            <Server className="privacy-item__icon" />
            <div>
              <span className="privacy-item__title">
                Remote Provider (Groq Cloud API)
              </span>
              <span className="privacy-item__desc">
                Dictated audio is sent to Groq’s endpoint using <code>whisper-large-v3</code> for speech-to-text. Raw transcripts, vocabulary hints, and text corrections are cleaned using <code>qwen/qwen3.8-27b</code> (Preview) with conservative editing instructions. Review Groq’s terms at{" "}
                <a
                  href="https://groq.com/privacy-policy/"
                  target="_blank"
                  rel="noreferrer"
                >
                  groq.com/privacy-policy
                </a>.
              </span>
            </div>
          </li>

          <li className="privacy-item">
            <Lock className="privacy-item__icon" />
            <div>
              <span className="privacy-item__title">
                Zero Cloud Sync & No Conversational Memory
              </span>
              <span className="privacy-item__desc">
                Flow contains no telemetry, analytics, background sync, or automatic model fallback. Each dictation is processed as an isolated request.
              </span>
            </div>
          </li>

          <li className="privacy-item">
            <HardDrive className="privacy-item__icon" />
            <div>
              <span className="privacy-item__title">
                Local Storage & Retention Limits
              </span>
              <span className="privacy-item__desc">
                Dictation history, temporary audio spools, and settings are stored locally on your machine in unencrypted SQLite and files. Recoverable pending recordings expire after 7 days (capped at 100 items / 256 MB). Pre-upgrade database backups expire after 7 days. Be aware that pasted dictations may be captured by Windows Clipboard History or Cloud Clipboard if enabled in Windows Settings.
              </span>
            </div>
          </li>
        </ul>

        <div className="modal-footer" style={{ padding: "16px 0 0", borderTop: "1px solid var(--border-soft)" }}>
          <button
            type="button"
            onClick={onAccept}
            className="primary-button"
            style={{ width: "100%" }}
          >
            <CheckCircle2 size={16} />
            I Understand & Accept
          </button>
        </div>
      </div>
    </Dialog>
  );
}
