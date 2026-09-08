import { RefObject, useEffect, useRef } from "react";

const focusableSelector = [
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[href]",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

const dialogStack: HTMLElement[] = [];

export function useDialogFocus(
  open: boolean,
  returnFocusRef?: RefObject<HTMLElement | null>,
  onEscape?: () => void,
) {
  const dialogRef = useRef<HTMLElement>(null);
  const onEscapeRef = useRef(onEscape);
  const previousActiveElementRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    onEscapeRef.current = onEscape;
  }, [onEscape]);

  useEffect(() => {
    if (!open) return;

    previousActiveElementRef.current = document.activeElement as HTMLElement | null;
    const dialog = dialogRef.current;

    if (dialog) {
      const prev = dialogStack[dialogStack.length - 1];
      if (prev && prev !== dialog) {
        prev.setAttribute("inert", "");
      }
      dialogStack.push(dialog);
    }

    const rootEl = document.getElementById("root");
    if (rootEl && dialogStack.length === 1 && (!dialog || !rootEl.contains(dialog))) {
      rootEl.setAttribute("inert", "");
    }

    const firstFocusable = dialog?.querySelector<HTMLElement>(focusableSelector);
    (firstFocusable ?? dialog)?.focus();

    const containFocus = (event: KeyboardEvent) => {
      if (dialog && dialogStack[dialogStack.length - 1] !== dialog) return;

      if (event.key === "Escape" && onEscapeRef.current) {
        event.preventDefault();
        onEscapeRef.current();
        return;
      }
      if (event.key !== "Tab" || !dialog) return;
      const focusable = Array.from(
        dialog.querySelectorAll<HTMLElement>(focusableSelector),
      );
      if (focusable.length === 0) {
        event.preventDefault();
        return;
      }

      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    dialog?.addEventListener("keydown", containFocus);

    return () => {
      dialog?.removeEventListener("keydown", containFocus);
      if (dialog) {
        const idx = dialogStack.indexOf(dialog);
        if (idx !== -1) {
          dialogStack.splice(idx, 1);
        }
      }

      const top = dialogStack[dialogStack.length - 1];
      if (top) {
        top.removeAttribute("inert");
        const focusable = top.querySelector<HTMLElement>(focusableSelector);
        (focusable ?? top)?.focus();
      } else if (rootEl) {
        rootEl.removeAttribute("inert");
      }

      // Only restore the original focus when no other dialog remains open;
      // otherwise the focus-to-top-dialog behavior above must be preserved.
      if (dialogStack.length === 0) {
        const targetToFocus = returnFocusRef?.current ?? previousActiveElementRef.current;
        if (targetToFocus && targetToFocus.isConnected && typeof targetToFocus.focus === "function") {
          targetToFocus.focus();
        }
      }
    };
  }, [open, returnFocusRef]);

  return dialogRef;
}
