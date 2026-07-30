import { FormEvent, useEffect, useRef, useState } from "react";
import { Check, Command, Pencil, Trash2, X } from "lucide-react";
import { api } from "../api";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
import { useDialogFocus } from "../hooks/useDialogFocus";
import type { Snippet } from "../types";

function SnippetRow({
  snippet,
  onChange,
  onDelete,
  notify,
}: {
  snippet: Snippet;
  onChange: (snippet: Snippet) => void;
  onDelete: () => void;
  notify: (data: ToastData) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [trigger, setTrigger] = useState(snippet.trigger);
  const [content, setContent] = useState(snippet.content);

  const save = async () => {
    if (!trigger.trim() || !content.trim()) return;
    try {
      await api.updateSnippet(snippet.id, trigger.trim(), content);
      onChange({ ...snippet, trigger: trigger.trim(), content });
      setEditing(false);
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    }
  };

  const cancel = () => {
    setTrigger(snippet.trigger);
    setContent(snippet.content);
    setEditing(false);
  };

  return (
    <article className="snippet-row">
      {editing ? (
        <div className="snippet-edit">
          <label>Trigger<input value={trigger} onChange={(e) => setTrigger(e.target.value)} /></label>
          <label>Content<textarea rows={4} value={content} onChange={(e) => setContent(e.target.value)} /></label>
        </div>
      ) : (
        <div className="snippet-copy">{snippet.trigger}</div>
      )}
      <div className="row-actions">
        {editing ? (
          <>
            <button className="icon-button" aria-label="Save" onClick={() => void save()}><Check size={16} /></button>
            <button className="icon-button" aria-label="Cancel" onClick={cancel}><X size={16} /></button>
          </>
        ) : (
          <>
            <button className="icon-button" aria-label="Edit" onClick={() => setEditing(true)}><Pencil size={15} /></button>
            <button className="icon-button icon-button--danger" aria-label="Remove" onClick={onDelete}><Trash2 size={15} /></button>
          </>
        )}
      </div>
    </article>
  );
}

export function Snippets({ notify }: { notify: (data: ToastData) => void }) {
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [trigger, setTrigger] = useState("");
  const [content, setContent] = useState("");
  const [addOpen, setAddOpen] = useState(false);
  const [adding, setAdding] = useState(false);
  const addButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogFocus(addOpen, addButtonRef);

  useEffect(() => {
    api.snippets().then(setSnippets).catch((error) => notify({ kind: "error", message: String(error) }));
  }, [notify]);

  useEffect(() => {
    if (!addOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !adding) {
        setAddOpen(false);
        setTrigger("");
        setContent("");
      }
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [addOpen, adding]);

  const closeAddDialog = () => {
    if (adding) return;
    setAddOpen(false);
    setTrigger("");
    setContent("");
  };

  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!trigger.trim() || !content.trim()) return;
    setAdding(true);
    try {
      const snippet = await api.addSnippet(trigger.trim(), content);
      setSnippets((current) => [...current, snippet]);
      setTrigger("");
      setContent("");
      setAddOpen(false);
      notify({ kind: "success", message: "Snippet added" });
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
          <h1>Snippets</h1>
          <p>Say an exact trigger to paste its content instantly.</p>
        </div>
        <button ref={addButtonRef} className="primary-button" type="button" onClick={() => setAddOpen(true)}>
          Add new
        </button>
      </header>

      <section className="section-block snippets-list">
        {snippets.length === 0 ? (
          <EmptyState icon={Command} title="No snippets yet" description="Create a trigger for text you type often." />
        ) : (
          snippets.map((snippet) => (
            <SnippetRow
              key={snippet.id}
              snippet={snippet}
              notify={notify}
              onChange={(next) => setSnippets((current) => current.map((item) => item.id === next.id ? next : item))}
              onDelete={() => {
                void api
                  .deleteSnippet(snippet.id)
                  .then(() => {
                    setSnippets((current) => current.filter((item) => item.id !== snippet.id));
                  })
                  .catch((error) => notify({ kind: "error", message: String(error) }));
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
            className="creation-dialog creation-dialog--snippet"
            role="dialog"
            aria-modal="true"
            aria-labelledby="add-snippet-title"
          >
            <header>
              <h2 id="add-snippet-title">Add snippet</h2>
            </header>

            <form onSubmit={(event) => void add(event)}>
              <div className="creation-dialog__body snippet-dialog__body">
                <input
                  aria-label="Snippet trigger"
                  autoFocus
                  placeholder="Snippet"
                  value={trigger}
                  maxLength={120}
                  disabled={adding}
                  onChange={(event) => setTrigger(event.target.value)}
                />
                <div className="snippet-expansion">
                  <textarea
                    aria-label="Snippet expansion"
                    placeholder="Expansion"
                    value={content}
                    maxLength={4000}
                    disabled={adding}
                    onChange={(event) => setContent(event.target.value)}
                  />
                  <span>{content.length}/4000</span>
                </div>
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
                  disabled={!trigger.trim() || !content.trim() || adding}
                >
                  {adding ? "Adding…" : "Add snippet"}
                </button>
              </footer>
            </form>
          </section>
        </div>
      )}
    </section>
  );
}
