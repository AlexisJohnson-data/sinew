import { useEffect, useState } from "react";
import { api } from "../../lib/ipc";
import { formatTokens, formatUsd } from "../../lib/usage";
import type { AgentEvent, UsageTotals } from "../../types";

type Props = {
  conversationId: string;
  subscribeEvents: (
    handler: (conversationId: string, event: AgentEvent, sequence?: number) => void,
  ) => () => void;
};

/// A usage event for this response, possibly wrapped in sub-agent events.
function isUsageEvent(event: AgentEvent): boolean {
  if (event.type === "usage_recorded") return true;
  if (event.type === "sub_agent_event") return isUsageEvent(event.event);
  return false;
}

/// "142k · ~$1.84" next to the context meter: what this conversation has
/// used so far, with the breakdown on hover.
export function ConversationCost({ conversationId, subscribeEvents }: Props) {
  const [totals, setTotals] = useState<UsageTotals | null>(null);

  useEffect(() => {
    if (!conversationId) return undefined;
    let cancelled = false;
    let timer: number | undefined;
    const load = () => {
      api
        .conversationUsage(conversationId)
        .then((next) => !cancelled && setTotals(next))
        .catch(() => undefined);
    };
    setTotals(null);
    load();
    const unsubscribe = subscribeEvents((eventConversationId, event) => {
      if (eventConversationId !== conversationId || !isUsageEvent(event)) return;
      // The row is written as the event goes out; give it a beat.
      window.clearTimeout(timer);
      timer = window.setTimeout(load, 300);
    });
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      unsubscribe();
    };
  }, [conversationId, subscribeEvents]);

  if (!totals || totals.responses === 0) return null;
  const cost = totals.meteredCostUsd + totals.subscriptionCostUsd;
  const priced = totals.unpricedResponses < totals.responses;
  const lines = [
    `${totals.responses} responses`,
    `Input ${formatTokens(totals.inputTokens)} · output ${formatTokens(totals.outputTokens)}`,
    `Cache read ${formatTokens(totals.cacheReadTokens)} · written ${formatTokens(totals.cacheWriteTokens)}`,
    totals.meteredCostUsd > 0 ? `Spent (pay-per-token): ${formatUsd(totals.meteredCostUsd)}` : null,
    totals.subscriptionCostUsd > 0
      ? `API equivalent of subscription use: ${formatUsd(totals.subscriptionCostUsd)}`
      : null,
    totals.unpricedResponses > 0 ? `${totals.unpricedResponses} responses without a known price` : null,
  ].filter(Boolean);

  return (
    <span className="conversation-cost" tabIndex={0}>
      <span className="conversation-cost__value">
        {formatTokens(totals.totalTokens)}
        {priced && <> · ~{formatUsd(cost)}</>}
      </span>
      <span className="conversation-cost__tip" role="tooltip">
        {lines.map((line) => (
          <span key={line}>{line}</span>
        ))}
      </span>
    </span>
  );
}
