import { useEffect, useRef, useState } from "react";
import { Icon } from "@iconify/react";
import { api } from "../../lib/ipc";
import {
  quotaLevel,
  quotaProvider,
  quotaTitle,
  resetLabel,
  windowLabel,
} from "../../lib/usage";
import type { AgentEvent, QuotaSnapshot } from "../../types";

const REFRESH_MS = 5 * 60 * 1000;

type Props = {
  /// Provider of the model selected in the composer.
  provider: string | undefined;
  workspacePath: string;
  subscribeEvents: (
    handler: (conversationId: string, event: AgentEvent, sequence?: number) => void,
  ) => () => void;
};

/// A small pill next to the composer showing how close the selected
/// provider's plan is to its limit; click for every window. It is a
/// pill with a number, not a ring, so it can't be mistaken for the
/// context meter.
export function PlanUsageRing({ provider, workspacePath, subscribeEvents }: Props) {
  const [snapshots, setSnapshots] = useState<QuotaSnapshot[]>([]);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const target = quotaProvider(provider);

  useEffect(() => {
    if (!target) return undefined;
    let cancelled = false;
    const refresh = () => {
      api
        .refreshPlanLimits()
        .then((next) => !cancelled && setSnapshots(next))
        .catch(() => undefined);
    };
    refresh();
    const timer = window.setInterval(refresh, REFRESH_MS);
    // A finished turn has used some of the plan; the backend throttles.
    const unsubscribe = subscribeEvents((_conversationId, event) => {
      if (event.type === "turn_finished") refresh();
    });
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      unsubscribe();
    };
  }, [target, subscribeEvents]);

  useEffect(() => {
    if (!open) return undefined;
    const close = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", escape);
    };
  }, [open]);

  const snapshot = snapshots.find((item) => item.provider === target);
  if (!target || !snapshot || snapshot.windows.length === 0) return null;

  // The ring shows whichever window is closest to its limit.
  const tightest = snapshot.windows.reduce((a, b) => (b.usedPercent > a.usedPercent ? b : a));
  const percent = Math.round(tightest.usedPercent);
  const level = quotaLevel(tightest.usedPercent);

  const openSettings = () => {
    setOpen(false);
    void api
      .openSecondaryWindow({ view: "settings", workspacePath, section: "usage" })
      .catch(() => undefined);
  };

  return (
    <div className="plan-ring" ref={rootRef}>
      <button
        type="button"
        className="plan-ring__button"
        data-level={level}
        onClick={() => setOpen((value) => !value)}
        aria-label={`${quotaTitle(snapshot)}: ${windowLabel(tightest)} ${percent}% used`}
        aria-expanded={open}
        title={`${windowLabel(tightest)} · ${percent}%`}
      >
        <Icon icon="solar:speedometer-middle-linear" width={13} height={13} aria-hidden="true" />
        <span>{percent}%</span>
      </button>
      {open && (
        <div className="plan-ring__popover" role="dialog" aria-label="Plan limits">
          <div className="plan-ring__head">
            <span>Plan limits · {quotaTitle(snapshot)}</span>
            <button type="button" className="plan-ring__all" onClick={openSettings}>
              See all
              <Icon icon="solar:arrow-right-linear" width={13} height={13} />
            </button>
          </div>
          {snapshot.status && snapshot.status !== "allowed" && (
            <div className="plan-ring__status">{snapshot.status}</div>
          )}
          {snapshot.windows.map((window) => (
            <div key={window.label} className="plan-ring__row">
              <div className="plan-ring__row-text">
                <span className="plan-ring__label">{windowLabel(window)}</span>
                <span className="plan-ring__meta">
                  {resetLabel(window.resetsAtMs) ?? ""}
                  <span className="plan-ring__percent">{Math.round(window.usedPercent)}%</span>
                </span>
              </div>
              <span className="usage__bar">
                <span
                  className="usage__bar-fill"
                  data-level={quotaLevel(window.usedPercent)}
                  style={{ width: `${Math.min(100, window.usedPercent)}%` }}
                />
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
