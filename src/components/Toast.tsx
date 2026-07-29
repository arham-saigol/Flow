import { Check, CircleAlert, X } from "lucide-react";

export interface ToastData {
  kind: "success" | "error";
  message: string;
}

export function Toast({
  data,
  onClose,
}: {
  data: ToastData;
  onClose: () => void;
}) {
  return (
    <div className={`toast toast--${data.kind}`} role="status">
      {data.kind === "success" ? <Check size={16} /> : <CircleAlert size={16} />}
      <span>{data.message}</span>
      <button aria-label="Dismiss" onClick={onClose}>
        <X size={14} />
      </button>
    </div>
  );
}
