import { useState } from "react";

/**
 * Small "Copy" button overlaid on assistant messages. Mirrors the
 * code-block copy UX (transient "Copied!" feedback, silent failure on
 * unfocused-document clipboard errors) but for the whole message text.
 *
 * Renders a positioned button — the parent must set
 * `position: relative` so the absolute placement lands correctly.
 */
export function MessageCopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    const trimmed = text.trim();
    if (!trimmed) return;
    navigator.clipboard
      .writeText(trimmed)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1400);
      })
      .catch(() => {
        // Clipboard API can fail when the document is not focused;
        // ignore silently rather than showing a confusing error.
      });
  };

  return (
    <button
      type="button"
      className="msg__copy"
      onClick={copy}
      title={copied ? "Copied!" : "Copy message"}
      aria-label="Copy message"
      data-copied={copied ? "true" : "false"}
    >
      {copied ? "Copied!" : "Copy"}
    </button>
  );
}
