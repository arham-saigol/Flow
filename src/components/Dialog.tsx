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
  /** Hides the close button and disables backdrop-click and Escape dismissal. */
  blocking?: boolean;
}

export function Dialog({
  open,
  onClose,
  title,
  children,
  returnFocusRef,
  size = "md",
  blocking = false,
}: DialogProps) {
  const titleId = useId();
  const dialogRef = useDialogFocus(
    open,
    returnFocusRef,
    blocking ? undefined : onClose,
  );

  if (!open) return null;

  const modalRoot =
    typeof document !== "undefined"
      ? document.getElementById("modal-root") || document.body
      : null;

  if (!modalRoot) return null;

  return createPortal(
    <div
      className="modal-backdrop"
      // onMouseDown instead of onClick so a drag that starts inside the panel
      // (e.g. selecting text) does not dismiss the dialog when it ends outside.
      onMouseDown={(e) => {
        if (!blocking && e.target === e.currentTarget) {
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
          {!blocking && (
            <button
              type="button"
              onClick={onClose}
              aria-label="Close dialog"
              className="modal-close-btn"
            >
              <X size={18} />
            </button>
          )}
        </header>
        <div className="modal-body">{children}</div>
      </section>
    </div>,
    modalRoot,
  );
}
