import { type RefObject } from "react";
import { ShieldCheck, Lock, Server, HardDrive, CheckCircle2 } from "lucide-react";
import { Dialog } from "./Dialog";

interface PrivacyNoticeModalProps {
  open: boolean;
  onAccept: () => void;
  returnFocusRef: RefObject<HTMLElement>;
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
      maxWidthClass="max-w-xl"
    >
      <div className="space-y-4">
        <div className="flex items-center gap-3 p-3 bg-amber-500/10 border border-amber-500/20 rounded-xl text-amber-900">
          <ShieldCheck className="w-6 h-6 text-amber-700 shrink-0" />
          <p className="text-xs font-medium">
            Please review how Flow processes your speech before dictating.
          </p>
        </div>

        <ul className="space-y-3">
          <li className="flex items-start gap-3">
            <Server className="w-5 h-5 text-stone-500 mt-0.5 shrink-0" />
            <div>
              <span className="font-semibold text-stone-900 block">
                Exclusive Remote Provider (Groq)
              </span>
              <span className="text-xs text-stone-600">
                Dictated audio is sent exclusively to Groq’s OpenAI-compatible endpoint using Whisper Large v3 for speech-to-text.
              </span>
            </div>
          </li>

          <li className="flex items-start gap-3">
            <Lock className="w-5 h-5 text-stone-500 mt-0.5 shrink-0" />
            <div>
              <span className="font-semibold text-stone-900 block">
                Text Polishing & Redaction
              </span>
              <span className="text-xs text-stone-600">
                Raw transcripts are cleaned using Groq’s hosted Qwen 2.5 32B model with strict privacy parameters. No conversational memory is maintained between dictations.
              </span>
            </div>
          </li>

          <li className="flex items-start gap-3">
            <HardDrive className="w-5 h-5 text-stone-500 mt-0.5 shrink-0" />
            <div>
              <span className="font-semibold text-stone-900 block">
                Zero Cloud Storage & Retention
              </span>
              <span className="text-xs text-stone-600">
                Groq does not train on your API submissions or retain dictations beyond ephemeral request execution. History and audio spools are stored strictly locally on your machine according to your retention settings.
              </span>
            </div>
          </li>
        </ul>

        <div className="pt-4 border-t border-stone-200/80 flex justify-end">
          <button
            type="button"
            onClick={onAccept}
            className="flex items-center gap-2 px-5 py-2.5 bg-stone-900 text-stone-50 hover:bg-stone-800 rounded-xl font-medium text-sm transition-colors shadow-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-stone-900"
          >
            <CheckCircle2 className="w-4 h-4" />
            I Understand & Accept
          </button>
        </div>
      </div>
    </Dialog>
  );
}
