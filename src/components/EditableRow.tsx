import { FormEvent, useEffect, useRef, useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { useDialogFocus } from "../hooks/useDialogFocus";

export function EditableRow({
  value,
  onSave,
  onDelete,
  onError,
}: {
  value: string;
  onSave: (value: string) => Promise<void>;
  onDelete: () => Promise<void>;
  onError: (error: unknown) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [draft, setDraft] = useState(value);
  const editButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogFocus(editing, editButtonRef);

  const close = () => {
    if (saving) return;
    setDraft(value);
    setEditing(false);
  };

  useEffect(() => {
    if (!editing) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [editing, saving, value]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    const next = draft.trim();
    if (!next) return;
    setSaving(true);
    try {
      await onSave(next);
      setDraft(next);
      setEditing(false);
    } catch (error) {
      onError(error);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flat-row">
      <span>{value}</span>
      <div className="row-actions">
        <button
          ref={editButtonRef}
          className="icon-button"
          aria-label={`Edit ${value}`}
          onClick={() => setEditing(true)}
        >
          <Pencil size={15} />
        </button>
        <button
          className="icon-button icon-button--danger"
          aria-label={`Remove ${value}`}
          onClick={() => void onDelete().catch(onError)}
        >
          <Trash2 size={15} />
        </button>
      </div>

      {editing && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(event) => event.target === event.currentTarget && close()}
        >
          <section
            ref={dialogRef}
            className="creation-dialog creation-dialog--vocabulary"
            role="dialog"
            aria-modal="true"
            aria-labelledby="edit-dictionary-title"
          >
            <header>
              <h2 id="edit-dictionary-title">Edit vocabulary</h2>
            </header>
            <form onSubmit={(event) => void save(event)}>
              <div className="creation-dialog__body">
                <input
                  aria-label="Vocabulary word"
                  autoFocus
                  value={draft}
                  maxLength={120}
                  disabled={saving}
                  onChange={(event) => setDraft(event.target.value)}
                />
              </div>
              <footer>
                <button className="secondary-button" type="button" disabled={saving} onClick={close}>
                  Cancel
                </button>
                <button className="primary-button" type="submit" disabled={!draft.trim() || saving}>
                  {saving ? "Saving…" : "Save changes"}
                </button>
              </footer>
            </form>
          </section>
        </div>
      )}
    </div>
  );
}
