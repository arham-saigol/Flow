import { FormEvent, useEffect, useState } from "react";
import { Check, Command, Pencil, Plus, Trash2, X } from "lucide-react";
import { api } from "../api";
import { EmptyState } from "../components/EmptyState";
import type { ToastData } from "../components/Toast";
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

  return (
    <article className="snippet-row">
      {editing ? (
        <div className="snippet-edit">
          <label>Trigger<input value={trigger} onChange={(e) => setTrigger(e.target.value)} /></label>
          <label>Content<textarea rows={4} value={content} onChange={(e) => setContent(e.target.value)} /></label>
        </div>
      ) : (
        <div className="snippet-copy">
          <span>{snippet.trigger}</span>
          <p>{snippet.content}</p>
        </div>
      )}
      <div className="row-actions">
        {editing ? (
          <>
            <button className="icon-button" aria-label="Save" onClick={() => void save()}><Check size={16} /></button>
            <button className="icon-button" aria-label="Cancel" onClick={() => setEditing(false)}><X size={16} /></button>
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

  useEffect(() => {
    api.snippets().then(setSnippets).catch((error) => notify({ kind: "error", message: String(error) }));
  }, [notify]);

  const add = async (event: FormEvent) => {
    event.preventDefault();
    if (!trigger.trim() || !content.trim()) return;
    try {
      const snippet = await api.addSnippet(trigger.trim(), content);
      setSnippets((current) => [...current, snippet]);
      setTrigger("");
      setContent("");
    } catch (error) {
      notify({ kind: "error", message: String(error) });
    }
  };

  return (
    <section className="page page--narrow">
      <header className="page-header">
        <div>
          <p className="eyebrow">Speak to expand</p>
          <h1>Snippets</h1>
          <p>Say an exact trigger to paste its content instantly.</p>
        </div>
      </header>

      <form className="snippet-form" onSubmit={(event) => void add(event)}>
        <label>
          Trigger
          <input placeholder="e.g. my email address" value={trigger} onChange={(event) => setTrigger(event.target.value)} />
        </label>
        <label>
          Content
          <textarea rows={4} placeholder="The exact text to paste" value={content} onChange={(event) => setContent(event.target.value)} />
        </label>
        <div><button className="primary-button" type="submit" disabled={!trigger.trim() || !content.trim()}><Plus size={17} /> Add snippet</button></div>
      </form>

      <div className="helper-copy">
        Matching ignores capitalization and surrounding punctuation, but the words must otherwise match exactly.
      </div>

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
    </section>
  );
}
