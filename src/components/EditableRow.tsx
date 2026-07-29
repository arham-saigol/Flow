import { Check, Pencil, Trash2, X } from "lucide-react";
import { useState } from "react";

export function EditableRow({
  value,
  onSave,
  onDelete,
}: {
  value: string;
  onSave: (value: string) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);

  const save = async () => {
    const next = draft.trim();
    if (!next) return;
    await onSave(next);
    setEditing(false);
  };

  return (
    <div className="flat-row">
      {editing ? (
        <input
          autoFocus
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void save();
            if (event.key === "Escape") {
              setDraft(value);
              setEditing(false);
            }
          }}
        />
      ) : (
        <span>{value}</span>
      )}
      <div className="row-actions">
        {editing ? (
          <>
            <button className="icon-button" aria-label="Save" onClick={() => void save()}>
              <Check size={16} />
            </button>
            <button
              className="icon-button"
              aria-label="Cancel"
              onClick={() => {
                setDraft(value);
                setEditing(false);
              }}
            >
              <X size={16} />
            </button>
          </>
        ) : (
          <>
            <button className="icon-button" aria-label="Edit" onClick={() => setEditing(true)}>
              <Pencil size={15} />
            </button>
            <button className="icon-button icon-button--danger" aria-label="Remove" onClick={() => void onDelete()}>
              <Trash2 size={15} />
            </button>
          </>
        )}
      </div>
    </div>
  );
}
