import { FormEvent, useEffect, useState } from "react";
import { BookOpen, Plus } from "lucide-react";
import { api } from "../api";
import { EditableRow } from "../components/EditableRow";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
import type { DictionaryEntry } from "../types";

export function Dictionary({ notify }: { notify: (data: ToastData) => void }) {
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [value, setValue] = useState("");

  const load = () =>
    api.dictionary().then(setEntries).catch((error) =>
      notify({ kind: "error", message: String(error) }),
    );

  useEffect(() => {
    void load();
  }, []);

  const add = async (event: FormEvent) => {
    event.preventDefault();
    const next = value.trim();
    if (!next) return;
    try {
      const entry = await api.addDictionary(next);
      setEntries((current) => [...current, entry].sort((a, b) => a.value.localeCompare(b.value)));
      setValue("");
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    }
  };

  return (
    <section className="page page--narrow">
      <header className="page-header">
        <div>
          <h1>Dictionary</h1>
          <p>Teach Flow the exact spelling of words and names you use.</p>
        </div>
      </header>

      <form className="add-form" onSubmit={(event) => void add(event)}>
        <input
          aria-label="Word or name"
          placeholder="Add a word or name"
          value={value}
          maxLength={120}
          onChange={(event) => setValue(event.target.value)}
        />
        <button className="primary-button" type="submit" disabled={!value.trim()}>
          <Plus size={17} /> Add
        </button>
      </form>

      <div className="helper-copy">
        Entries guide both transcription and final spelling. Add only the canonical form you want Flow to use.
      </div>

      <section className="section-block flat-list">
        {entries.length === 0 ? (
          <EmptyState
            icon={BookOpen}
            title="No dictionary entries yet"
            description="Add product names, people, places, or specialist terms."
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
    </section>
  );
}
