import { type ReactNode, type RefObject, useId } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { useDialogFocus } from "../hooks/useDialogFocus";

interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  returnFocusRef?: RefObject<HTMLElement | null>;
  size?: "sm" | "md" | "lg" | "xl";
  maxWidthClass?: string; // backwards compatibility
}

export function Dialog({
  open,
  onClose,
  title,
  children,
  returnFocusRef,
  size = "md",
}: DialogProps) {
  const titleId = useId();
  const dialogRef = useDialogFocus(open, returnFocusRef, onClose);

  if (!open) return null;

  const modalRoot =
    typeof document !== "undefined"
      ? document.getElementById("modal-root") || document.body
      : null;

  if (!modalRoot) return null;

  return createPortal(
    <div
      className="modal-backdrop"
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
        aria-labelledby={titleId}
        tabIndex={-1}
        className={`modal-panel modal-panel--${size}`}
      >
        <header className="modal-header">
          <h2 id={titleId} className="modal-title">
            {title}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close dialog"
            className="modal-close-btn"
          >
            <X size={18} />
          </button>
        </header>
        <div className="modal-body">{children}</div>
      </section>
    </div>,
    modalRoot,
  );
}
