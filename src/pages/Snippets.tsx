import { FormEvent, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Command, Pencil, Trash2 } from "lucide-react";
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
  const [saving, setSaving] = useState(false);
  const [trigger, setTrigger] = useState(snippet.trigger);
  const [content, setContent] = useState(snippet.content);
  const editButtonRef = useRef<HTMLButtonElement>(null);
  const dialogRef = useDialogFocus(editing, editButtonRef);

  const close = () => {
    if (saving) return;
    setTrigger(snippet.trigger);
    setContent(snippet.content);
    setEditing(false);
  };

  useEffect(() => {
    if (!editing) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [editing, saving, snippet]);

  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!trigger.trim() || !content.trim()) return;
    setSaving(true);
    try {
      const next = { ...snippet, trigger: trigger.trim(), content };
      await api.updateSnippet(snippet.id, next.trigger, next.content);
      onChange(next);
      setEditing(false);
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <article className="snippet-row">
      <div className="snippet-copy">{snippet.trigger}</div>
      <div className="row-actions">
        <button
          ref={editButtonRef}
          className="icon-button"
          aria-label={`Edit ${snippet.trigger}`}
          onClick={() => setEditing(true)}
        >
          <Pencil size={15} />
        </button>
        <button className="icon-button icon-button--danger" aria-label={`Remove ${snippet.trigger}`} onClick={onDelete}>
          <Trash2 size={15} />
        </button>
      </div>

      {editing &&
        createPortal(
          <div
            className="modal-backdrop"
            role="presentation"
            onMouseDown={(event) => event.target === event.currentTarget && close()}
          >
            <section
              ref={dialogRef}
              className="creation-dialog creation-dialog--snippet"
              role="dialog"
              aria-modal="true"
              aria-labelledby={`edit-snippet-${snippet.id}`}
            >
              <header>
                <h2 id={`edit-snippet-${snippet.id}`}>Edit snippet</h2>
              </header>
              <form onSubmit={(event) => void save(event)}>
                <div className="creation-dialog__body snippet-dialog__body">
                  <input
                    aria-label="Snippet trigger"
                    autoFocus
                    value={trigger}
                    maxLength={120}
                    disabled={saving}
                    onChange={(event) => setTrigger(event.target.value)}
                  />
                  <div className="snippet-expansion">
                    <textarea
                      aria-label="Snippet expansion"
                      value={content}
                      maxLength={4000}
                      disabled={saving}
                      onChange={(event) => setContent(event.target.value)}
                    />
                    <span>{content.length}/4000</span>
                  </div>
                </div>
                <footer>
                  <button className="secondary-button" type="button" disabled={saving} onClick={close}>
                    Cancel
                  </button>
                  <button
                    className="primary-button"
                    type="submit"
                    disabled={!trigger.trim() || !content.trim() || saving}
                  >
                    {saving ? "Saving…" : "Save changes"}
                  </button>
                </footer>
              </form>
            </section>
          </div>,
          document.getElementById("modal-root") || document.body,
        )}
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

      {addOpen &&
        createPortal(
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
          </div>,
          document.getElementById("modal-root") || document.body,
        )}
    </section>
  );
}
