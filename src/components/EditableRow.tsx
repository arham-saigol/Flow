import { FormEvent, useRef, useState } from "react";
import { Pencil, Trash2 } from "lucide-react";
import { useDialogFocus } from "../hooks/useDialogFocus";
import type { DictionaryEntry } from "../types";

export function EditableRow({
  entry,
  onSave,
  onDelete,
  onError,
}: {
  entry: DictionaryEntry;
  onSave: (value: string, correction: string | null) => Promise<void>;
  onDelete: () => Promise<void>;
  onError: (error: unknown) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [draft, setDraft] = useState(entry.value);
  const [draftCorrection, setDraftCorrection] = useState(entry.correction ?? "");
  const [correctingMisspelling, setCorrectingMisspelling] = useState(entry.correction !== null);
  const editButtonRef = useRef<HTMLButtonElement>(null);

  const close = () => {
    if (saving) return;
    setDraft(entry.value);
    setDraftCorrection(entry.correction ?? "");
    setCorrectingMisspelling(entry.correction !== null);
    setEditing(false);
  };
  const dialogRef = useDialogFocus(editing, editButtonRef, close);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    const next = draft.trim();
    const nextCorrection = correctingMisspelling ? draftCorrection.trim() : null;
    if (
      !next ||
      (correctingMisspelling &&
        (!nextCorrection || next === nextCorrection))
    ) return;
    setSaving(true);
    try {
      await onSave(next, nextCorrection);
      setDraft(next);
      setDraftCorrection(nextCorrection ?? "");
      setCorrectingMisspelling(nextCorrection !== null);
      setEditing(false);
    } catch (error) {
      onError(error);
    } finally {
      setSaving(false);
    }
  };
  const displayValue = entry.correction
    ? `${entry.value} → ${entry.correction}`
    : entry.value;
  const canSave =
    Boolean(draft.trim()) &&
    (!correctingMisspelling ||
      (Boolean(draftCorrection.trim()) &&
        draft.trim() !== draftCorrection.trim()));

  return (
    <div className="flat-row">
      <span className="dictionary-entry-value">
        {entry.value}
        {entry.correction && (
          <>
            <span className="dictionary-entry-arrow" aria-hidden="true">→</span>
            <span>{entry.correction}</span>
          </>
        )}
      </span>
      <div className="row-actions">
        <button
          ref={editButtonRef}
          className="icon-button"
          type="button"
          aria-label={`Edit ${displayValue}`}
          onClick={() => setEditing(true)}
        >
          <Pencil size={15} />
        </button>
        <button
          className="icon-button icon-button--danger"
          type="button"
          aria-label={`Remove ${displayValue}`}
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
                <label className="dictionary-correction-toggle toggle-row">
                  <div>
                    <span>Correct a misspelling</span>
                  </div>
                  <input
                    type="checkbox"
                    role="switch"
                    checked={correctingMisspelling}
                    disabled={saving}
                    onChange={(event) => setCorrectingMisspelling(event.target.checked)}
                  />
                </label>
                {correctingMisspelling ? (
                  <div className="dictionary-correction-fields">
                    <input
                      aria-label="Misspelling"
                      autoFocus
                      placeholder="Misspelling"
                      value={draft}
                      maxLength={120}
                      disabled={saving}
                      onChange={(event) => setDraft(event.target.value)}
                    />
                    <span aria-hidden="true">→</span>
                    <input
                      aria-label="Correct spelling"
                      placeholder="Correct spelling"
                      value={draftCorrection}
                      maxLength={120}
                      disabled={saving}
                      onChange={(event) => setDraftCorrection(event.target.value)}
                    />
                  </div>
                ) : (
                  <input
                    aria-label="Vocabulary word"
                    autoFocus
                    value={draft}
                    maxLength={120}
                    disabled={saving}
                    onChange={(event) => setDraft(event.target.value)}
                  />
                )}
              </div>
              <footer>
                <button className="secondary-button" type="button" disabled={saving} onClick={close}>
                  Cancel
                </button>
                <button className="primary-button" type="submit" disabled={!canSave || saving}>
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
