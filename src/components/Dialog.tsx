import { type ReactNode, type RefObject } from "react";
import { X } from "lucide-react";
import { useDialogFocus } from "../hooks/useDialogFocus";

interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  returnFocusRef: RefObject<HTMLElement>;
  maxWidthClass?: string;
}

export function Dialog({
  open,
  onClose,
  title,
  children,
  returnFocusRef,
  maxWidthClass = "max-w-lg",
}: DialogProps) {
  const dialogRef = useDialogFocus(open, returnFocusRef, onClose);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4 animate-fade-in"
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          onClose();
        }
      }}
      role="presentation"
    >
      <section
        ref={dialogRef as RefObject<HTMLElement>}
        role="dialog"
        aria-modal="true"
        aria-labelledby="dialog-title"
        tabIndex={-1}
        className={`w-full ${maxWidthClass} bg-stone-50 border border-stone-300/80 rounded-2xl shadow-2xl overflow-hidden flex flex-col focus:outline-none`}
      >
        <header className="flex items-center justify-between px-6 py-4 border-b border-stone-200/80 bg-stone-100/50">
          <h2 id="dialog-title" className="text-lg font-semibold text-stone-900">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close dialog"
            className="rounded-lg p-1.5 text-stone-500 hover:text-stone-800 hover:bg-stone-200/60 transition-colors focus-visible:outline-2 focus-visible:outline-stone-800"
          >
            <X className="w-5 h-5" />
          </button>
        </header>
        <div className="p-6 overflow-y-auto max-h-[80vh] text-stone-700 text-sm">
          {children}
        </div>
      </section>
    </div>
  );
}
