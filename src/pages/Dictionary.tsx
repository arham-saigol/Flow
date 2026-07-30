import { FormEvent, useEffect, useRef, useState } from "react";
import { BookOpen } from "lucide-react";
import { api } from "../api";
import { EditableRow } from "../components/EditableRow";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
import { useDialogFocus } from "../hooks/useDialogFocus";
import type { DictionaryEntry } from "../types";

export function Dictionary({ notify }: { notify: (data: ToastData) => void }) {
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [value, setValue] = useState("");
  const [addOpen, setAddOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogFocus(addOpen, addButtonRef);

  const load = () =>
    api.dictionary().then(setEntries).catch((error) =>
      notify({ kind: "error", message: String(error) }),
    );

  useEffect(() => {
    void load();
  }, []);

  useEffect(() => {
    if (!addOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !adding) {
        setAddOpen(false);
        setValue("");
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [addOpen, adding]);

  const closeAddDialog = () => {
    if (adding) return;
    setAddOpen(false);
    setValue("");
  };

  const add = async (event: FormEvent) => {
    event.preventDefault();
    const next = value.trim();
    if (!next) return;
    setAdding(true);
    try {
      const entry = await api.addDictionary(next);
      setEntries((current) => [...current, entry].sort((a, b) => a.value.localeCompare(b.value)));
      setValue("");
      setAddOpen(false);
      notify({ kind: "success", message: "Added to dictionary" });
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    } finally {
      setAdding(false);
    }
  };

  return (
    <section className="page page--narrow">
      <header className="page-header">
        <div>
          <h1>Dictionary</h1>
          <p>Teach Flow the exact spelling of words and names it gets wrong.</p>
        </div>
        <button ref={addButtonRef} className="primary-button" type="button" onClick={() => setAddOpen(true)}>
          Add new
        </button>
      </header>

      <section className="section-block flat-list dictionary-list">
        {entries.length === 0 ? (
          <EmptyState
            icon={BookOpen}
            title="No dictionary entries yet"
            description="Add personal terms, company jargon, client names, or industry-specific lingo."
          />
        ) : (
          entries.map((entry) => (
            <EditableRow
              key={entry.id}
              value={entry.value}
              onError={(error) => notify({ kind: "error", message: String(error) })}
              onSave={async (next) => {
                await api.updateDictionary(entry.id, next);
                setEntries((current) =>
                  current
                    .map((item) => item.id === entry.id ? { ...item, value: next } : item)
                    .sort((a, b) => a.value.localeCompare(b.value)),
                );
              }}
              onDelete={async () => {
                await api.deleteDictionary(entry.id);
                setEntries((current) => current.filter((item) => item.id !== entry.id));
              }}
            />
          ))
        )}
      </section>

      {addOpen && (
        <div
          className="modal-backdrop"
          role="presentation"
          onMouseDown={(event) => event.target === event.currentTarget && closeAddDialog()}
        >
          <section
            ref={dialogRef}
            className="creation-dialog creation-dialog--vocabulary"
            role="dialog"
            aria-modal="true"
            aria-labelledby="add-dictionary-title"
          >
            <header>
              <h2 id="add-dictionary-title">Add to vocabulary</h2>
            </header>

            <form onSubmit={(event) => void add(event)}>
              <div className="creation-dialog__body">
                <input
                  id="dictionary-entry"
                  aria-label="New vocabulary word"
                  autoFocus
                  placeholder="Add a new word"
                  value={value}
                  maxLength={120}
                  disabled={adding}
                  onChange={(event) => setValue(event.target.value)}
                />
              </div>
              <footer>
                <button
                  className="secondary-button"
                  type="button"
                  disabled={adding}
                  onClick={closeAddDialog}
                >
                  Cancel
                </button>
                <button
                  className="primary-button"
                  type="submit"
                  disabled={!value.trim() || adding}
                >
                  {adding ? "Adding…" : "Add word"}
                </button>
              </footer>
            </form>
          </section>
        </div>
      )}
    </section>
  );
}
