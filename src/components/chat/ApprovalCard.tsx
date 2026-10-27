import { useState } from "react";
import type { ApprovalDecision } from "../../types";
import type { PendingApproval } from "./stream";

type Props = {
  approval: PendingApproval;
  disabled?: boolean;
  onAnswer: (
    approval: PendingApproval,
    decision: ApprovalDecision,
    message?: string,
  ) => Promise<void> | void;
};

const TOOL_LABELS: Record<string, string> = {
  bash: "Run command",
  bash_input: "Send input to the shell",
  edit_file: "Edit files",
  write_file: "Write file",
};

/// Permission prompt for a tool call the rules did not let through on their
/// own (see `sinew_app::permissions`). The agent waits until one button is
/// pressed; Stop cancels the turn and denies it.
export function ApprovalCard({ approval, disabled, onAnswer }: Props) {
  const [busy, setBusy] = useState(false);
  const [denyOpen, setDenyOpen] = useState(false);
  const [message, setMessage] = useState("");

  const answer = async (decision: ApprovalDecision, note?: string) => {
    if (busy || disabled) return;
    setBusy(true);
    try {
      await onAnswer(approval, decision, note);
    } finally {
      setBusy(false);
    }
  };

  const title = approval.toolName.startsWith("mcp__")
    ? "Use MCP tool"
    : TOOL_LABELS[approval.toolName] ?? approval.toolName;

  return (
    <div className="approval-card" role="group" aria-label="Permission request">
      <div className="approval-card__head">
        <span className="approval-card__badge">Approval needed</span>
        <span className="approval-card__title">{title}</span>
      </div>
      <pre className="approval-card__subject">{approval.subject}</pre>
      <div className="approval-card__reason">Why: {approval.reason}.</div>
      {denyOpen ? (
        <form
          className="approval-card__deny"
          onSubmit={(event) => {
            event.preventDefault();
            void answer("deny", message);
          }}
        >
          <input
            className="approval-card__input"
            autoFocus
            placeholder="Tell the agent what to do instead (optional)"
            value={message}
            onChange={(event) => setMessage(event.target.value)}
          />
          <button type="submit" className="approval-card__btn" disabled={busy || disabled}>
            Deny
          </button>
          <button
            type="button"
            className="approval-card__btn approval-card__btn--ghost"
            onClick={() => setDenyOpen(false)}
          >
            Back
          </button>
        </form>
      ) : (
        <div className="approval-card__actions">
          <button
            type="button"
            className="approval-card__btn approval-card__btn--primary"
            disabled={busy || disabled}
            onClick={() => void answer("allow_once")}
          >
            Allow once
          </button>
          <button
            type="button"
            className="approval-card__btn"
            disabled={busy || disabled}
            title={`Saves "${approval.suggestedRule}" to .sinew/permissions.json`}
            onClick={() => void answer("allow_always")}
          >
            Always allow in this project
          </button>
          <button
            type="button"
            className="approval-card__btn approval-card__btn--ghost"
            disabled={busy || disabled}
            onClick={() => void answer("deny")}
          >
            Deny
          </button>
          <button
            type="button"
            className="approval-card__btn approval-card__btn--ghost"
            disabled={busy || disabled}
            onClick={() => setDenyOpen(true)}
          >
            Deny with a message…
          </button>
        </div>
      )}
    </div>
  );
}
