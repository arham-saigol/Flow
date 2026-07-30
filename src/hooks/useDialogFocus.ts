import { RefObject, useEffect, useRef } from "react";

const focusableSelector = [
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[href]",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export function useDialogFocus(
  open: boolean,
  returnFocusRef: RefObject<HTMLElement>,
) {
  const dialogRef = useRef<HTMLElement>(null);

  useEffect(() => {
    if (!open) return;
    const dialog = dialogRef.current;

    const containFocus = (event: KeyboardEvent) => {
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
      returnFocusRef.current?.focus();
    };
  }, [open, returnFocusRef]);

  return dialogRef;
}
