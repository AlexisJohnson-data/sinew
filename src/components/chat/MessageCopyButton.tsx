import { useState } from "react";
import { Icon } from "@iconify/react";

/**
 * Small "Copy" affordance shown at the bottom of an assistant message
 * (the same way Claude.ai / Claude Code surface it). Always visible
 * but muted — clicking copies the raw markdown text and shows a
 * transient "Copied" tick.
 *
 * The icon is a stacked-square (`solar:copy-linear`) so the button is
 * recognisable without a label.
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
    <div className="msg__actions">
      <button
        type="button"
        className="msg__action"
        onClick={copy}
        title={copied ? "Copied" : "Copy message"}
        aria-label="Copy message"
        data-copied={copied ? "true" : "false"}
      >
        <Icon
          icon={copied ? "solar:check-read-linear" : "solar:copy-linear"}
          width={14}
          height={14}
          aria-hidden="true"
        />
      </button>
    </div>
  );
}
