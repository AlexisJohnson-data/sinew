import { useEffect, useRef, useState } from "react";
import { Icon } from "@iconify/react";
import { api } from "../lib/ipc";

type Props = {
  open: boolean;
  onClose: () => void;
};

const PLACEHOLDER = `Rules for every project, for example:

- Answer in French; keep code, identifiers and commit messages in English.
- Never push or publish without asking me first.
- Prefer small, focused commits.`;

/// Editor for ~/.sinew/AGENTS.md, the user's instructions loaded in every
/// project (before the project's own AGENTS.md / CLAUDE.md).
export function InstructionsDialog({ open, onClose }: Props) {
  const [path, setPath] = useState("");
  const [content, setContent] = useState("");
  const [saved, setSaved] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .readGlobalInstructions()
      .then((file) => {
        if (cancelled) return;
        setPath(file.path);
        setContent(file.content);
        setSaved(file.content);
        requestAnimationFrame(() => textareaRef.current?.focus());
      })
      .catch((err) => !cancelled && setError(String(err)))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [open]);

  const dirty = content !== saved;

  const close = () => {
    if (busy) return;
    if (dirty && !window.confirm("Discard your unsaved changes?")) return;
    onClose();
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const file = await api.saveGlobalInstructions(content);
      setSaved(file.content);
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  if (!open) return null;

  return (
    <div
      className="migrate__backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Your instructions"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") close();
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
          event.preventDefault();
          if (!busy && !loading) void save();
        }
      }}
    >
      <div className="migrate instructions-dialog">
        <header className="migrate__head">
          <div className="migrate__head-text">
            <h2>Your instructions</h2>
            <p>
              Loaded in every project, before the project's own <code>AGENTS.md</code> (which
              wins when they disagree). Changes apply from the next message.
            </p>
          </div>
          <button
            type="button"
            className="migrate__close"
            title="Close"
            onClick={close}
            disabled={busy}
          >
            <Icon icon="solar:close-square-linear" width={14} height={14} />
          </button>
        </header>

        <textarea
          ref={textareaRef}
          className="instructions-dialog__editor"
          value={content}
          placeholder={loading ? "Loading…" : PLACEHOLDER}
          disabled={loading}
          spellCheck={false}
          onChange={(event) => setContent(event.target.value)}
        />

        {error && <p className="migrate__hint migrate__hint--warn">{error}</p>}

        <div className="migrate__actions">
          <span className="instructions-dialog__path" title={path}>
            {path}
          </span>
          <button type="button" className="migrate__btn" onClick={close} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className="migrate__btn migrate__btn--primary"
            onClick={() => void save()}
            disabled={busy || loading || !dirty}
            title="Save (Ctrl+S)"
          >
            {busy ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
