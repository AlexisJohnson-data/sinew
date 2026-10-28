import { useCallback, useEffect, useMemo, useState } from "react";
import { Icon } from "@iconify/react";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../lib/ipc";
import {
  formatTokens,
  formatUsd,
  providerLabel,
  quotaLevel,
  quotaTitle,
  resetLabel,
  utcOffsetMinutes,
  windowLabel,
} from "../lib/usage";
import type {
  PriceOverride,
  QuotaSnapshot,
  UsageGroup,
  UsageOverview,
  UsageSettings,
} from "../types";

const RANGES = [
  { days: 1, label: "Today" },
  { days: 7, label: "7 days" },
  { days: 30, label: "30 days" },
] as const;

const PLAN_LIMITS_REFRESH_MS = 5 * 60 * 1000;

function ageLabel(ms: number | null): string {
  if (ms === null) return "prices not loaded yet";
  const hours = Math.floor(ms / 3_600_000);
  if (hours < 1) return "prices updated less than an hour ago";
  if (hours < 48) return `prices updated ${hours} h ago`;
  return `prices updated ${Math.floor(hours / 24)} days ago`;
}

/// Settings → Usage: spend, API-equivalent cost, tokens, plan quotas.
export function UsageSection() {
  const [days, setDays] = useState<number>(7);
  const [overview, setOverview] = useState<UsageOverview | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [quotas, setQuotas] = useState<QuotaSnapshot[] | null>(null);

  // Live plan windows: asked for when the section opens, then every few
  // minutes while it stays open.
  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      api
        .refreshPlanLimits()
        .then((next) => !cancelled && setQuotas(next))
        .catch(() => undefined);
    };
    refresh();
    const timer = window.setInterval(refresh, PLAN_LIMITS_REFRESH_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const load = useCallback(async () => {
    try {
      setOverview(await api.usageOverview(days, utcOffsetMinutes()));
    } catch (error) {
      setStatus(String(error));
    }
  }, [days]);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async (work: () => Promise<unknown>, done?: string) => {
    setBusy(true);
    setStatus(null);
    try {
      await work();
      await load();
      if (done) setStatus(done);
    } catch (error) {
      setStatus(String(error));
    } finally {
      setBusy(false);
    }
  };

  const exportCsv = () =>
    run(async () => {
      const today = new Date().toISOString().slice(0, 10);
      const path = await save({
        defaultPath: `sinew-usage-${today}.csv`,
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!path) return;
      await api.exportUsageCsv(path, days);
    }, "Exported");

  const saveSettings = (settings: UsageSettings) =>
    run(() => api.saveUsageSettings(settings), "Saved");

  const totals = overview?.summary.totals;
  const subscriptionGroups = (overview?.summary.byProvider ?? [])
    .filter((group) => group.subscriptionCostUsd > 0)
    .sort((a, b) => b.subscriptionCostUsd - a.subscriptionCostUsd);

  return (
    <>
      <header className="settings-pane__header">
        <div className="settings-pane__header-text">
          <h1 className="settings-pane__title">Usage</h1>
          <p className="settings-pane__subtitle">
            {overview ? ageLabel(overview.pricesAgeMs) : "Loading…"} · from OpenRouter
          </p>
        </div>
        <div className="settings-pane__actions">
          {status && <span className="settings-pane__status">{status}</span>}
          <div className="settings-pane__tool-provider-switch usage__range" role="group" aria-label="Range">
            {RANGES.map((range) => (
              <button
                key={range.days}
                type="button"
                data-active={days === range.days ? "true" : "false"}
                onClick={() => setDays(range.days)}
              >
                {range.label}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="settings-pane__btn"
            disabled={busy}
            onClick={() => void run(() => api.refreshModelPrices(), "Prices updated")}
            title="Fetch the latest prices from OpenRouter"
          >
            <Icon icon="solar:refresh-linear" width={13} height={13} />
            <span>Prices</span>
          </button>
          <button type="button" className="settings-pane__btn" disabled={busy} onClick={() => void exportCsv()}>
            <Icon icon="solar:download-minimalistic-linear" width={13} height={13} />
            <span>CSV</span>
          </button>
        </div>
      </header>
      <div className="settings-pane__body settings-pane__body--tools">
        <div className="usage__cards">
          <UsageCard
            label="Spent"
            value={totals ? formatUsd(totals.meteredCostUsd) : "…"}
            hint="Pay-per-token providers (DeepSeek, OpenRouter): real money."
          />
          {/* One card per plan: Claude and OpenCode Go are separate
              subscriptions, so their API equivalents are never summed. */}
          {subscriptionGroups.length > 0 ? (
            subscriptionGroups.map((group) => (
              <UsageCard
                key={group.key}
                label={`${providerLabel(group.key)} · API equivalent`}
                value={formatUsd(group.subscriptionCostUsd)}
                hint="What this plan's use would cost on the API. Not billed."
              />
            ))
          ) : (
            <UsageCard
              label="API equivalent"
              value={totals ? formatUsd(0) : "…"}
              hint="What your subscription use would cost on the API. Not billed."
            />
          )}
          <UsageCard
            label="Tokens"
            value={totals ? formatTokens(totals.totalTokens) : "…"}
            hint={
              totals
                ? `${totals.responses} responses · cache read ${formatTokens(totals.cacheReadTokens)}`
                : ""
            }
          />
          <UsageCard
            label="Today"
            value={overview ? formatUsd(overview.spentTodayUsd) : "…"}
            hint={
              overview?.settings.dailyBudgetUsd
                ? `of ${formatUsd(overview.settings.dailyBudgetUsd)} daily budget`
                : "No daily budget set"
            }
            alert={
              !!overview?.settings.dailyBudgetUsd &&
              overview.spentTodayUsd >= overview.settings.dailyBudgetUsd
            }
          />
        </div>
        {totals && totals.unpricedResponses > 0 && (
          <p className="settings-pane__field-hint">
            {totals.unpricedResponses} responses used a model without a known price; their tokens
            count, their cost does not. Set a price below if you know it.
          </p>
        )}

        {(quotas || overview) && <QuotaList quotas={quotas ?? overview?.quotas ?? []} loading={quotas === null} />}
        {overview && totals && totals.responses > 0 && overview.summary.byDay.length > 1 && (
          <DayChart days={overview.summary.byDay} />
        )}
        {overview && (
          <>
            <UsageTable title="By provider" groups={overview.summary.byProvider} label={providerLabel} />
            <UsageTable title="By model" groups={overview.summary.byModel} />
            <UsageTable title="By project" groups={overview.summary.byProject} />
            <BudgetEditor settings={overview.settings} busy={busy} onSave={saveSettings} />
            <PriceOverrides settings={overview.settings} busy={busy} onSave={saveSettings} />
          </>
        )}
      </div>
    </>
  );
}

function UsageCard({
  label,
  value,
  hint,
  alert,
}: {
  label: string;
  value: string;
  hint: string;
  alert?: boolean;
}) {
  return (
    <div className="usage__card" data-alert={alert ? "true" : undefined}>
      <span className="usage__card-label">{label}</span>
      <span className="usage__card-value">{value}</span>
      <span className="usage__card-hint">{hint}</span>
    </div>
  );
}

function QuotaList({ quotas, loading }: { quotas: QuotaSnapshot[]; loading: boolean }) {
  return (
    <section className="settings-pane__tool-group">
      <div className="settings-pane__tool-group-head">
        <h2>Plan limits</h2>
      </div>
      {quotas.length === 0 ? (
        <p className="settings-pane__field-hint">
          {loading
            ? "Asking Claude and ChatGPT for your plan limits…"
            : "No plan limits: connect Claude or ChatGPT with your subscription, or an OpenCode Go key, in Providers."}
        </p>
      ) : (
        quotas.map((quota) => (
          <div key={quota.provider} className="usage__quota">
            <span className="usage__quota-provider">
              {quotaTitle(quota)}
              {quota.status && quota.status !== "allowed" ? ` · ${quota.status}` : ""}
            </span>
            {quota.windows.map((window) => (
              <div key={window.label} className="usage__quota-row">
                <span className="usage__quota-label">{windowLabel(window)}</span>
                <span className="usage__bar">
                  <span
                    className="usage__bar-fill"
                    data-level={quotaLevel(window.usedPercent)}
                    style={{ width: `${Math.min(100, window.usedPercent)}%` }}
                  />
                </span>
                <span className="usage__quota-value">
                  {resetLabel(window.resetsAtMs) ? `${resetLabel(window.resetsAtMs)} · ` : ""}
                  {Math.round(window.usedPercent)}%
                </span>
              </div>
            ))}
          </div>
        ))
      )}
    </section>
  );
}

function DayChart({ days }: { days: UsageGroup[] }) {
  const useCost = days.some((day) => day.meteredCostUsd + day.subscriptionCostUsd > 0);
  const value = (day: UsageGroup) =>
    useCost ? day.meteredCostUsd + day.subscriptionCostUsd : day.totalTokens;
  const max = Math.max(...days.map(value), 1e-9);
  return (
    <section className="settings-pane__tool-group">
      <div className="settings-pane__tool-group-head">
        <h2>{useCost ? "Cost per day" : "Tokens per day"}</h2>
      </div>
      <div className="usage__chart" role="img" aria-label="Usage per day">
        {days.map((day) => {
          const total = value(day);
          const metered = useCost ? day.meteredCostUsd : 0;
          const title = `${day.key}: ${formatTokens(day.totalTokens)} tokens · spent ${formatUsd(
            day.meteredCostUsd,
          )} · API equivalent ${formatUsd(day.subscriptionCostUsd)}`;
          return (
            <span key={day.key} className="usage__chart-col" title={title}>
              <span className="usage__chart-bar" style={{ height: `${(total / max) * 100}%` }}>
                {metered > 0 && (
                  <span
                    className="usage__chart-metered"
                    style={{ height: `${(metered / Math.max(total, 1e-9)) * 100}%` }}
                  />
                )}
              </span>
              <span className="usage__chart-day">{day.key.slice(8)}</span>
            </span>
          );
        })}
      </div>
      {useCost && (
        <p className="settings-pane__field-hint">
          <span className="usage__legend usage__legend--metered" /> spent ·{" "}
          <span className="usage__legend" /> API equivalent of subscriptions
        </p>
      )}
    </section>
  );
}

function UsageTable({
  title,
  groups,
  label = (key) => key,
}: {
  title: string;
  groups: UsageGroup[];
  label?: (key: string) => string;
}) {
  const [expanded, setExpanded] = useState(false);
  if (groups.length === 0) return null;
  const visible = expanded ? groups : groups.slice(0, 8);
  return (
    <section className="settings-pane__tool-group">
      <div className="settings-pane__tool-group-head">
        <h2>{title}</h2>
      </div>
      <table className="usage__table">
        <thead>
          <tr>
            <th />
            <th>Tokens</th>
            <th>Spent</th>
            <th>API equiv.</th>
          </tr>
        </thead>
        <tbody>
          {visible.map((group) => (
            <tr key={group.key}>
              <td title={group.key}>{label(group.key)}</td>
              <td>{formatTokens(group.totalTokens)}</td>
              <td>{group.meteredCostUsd > 0 ? formatUsd(group.meteredCostUsd) : "—"}</td>
              <td>
                {group.subscriptionCostUsd > 0
                  ? formatUsd(group.subscriptionCostUsd)
                  : group.unpricedResponses > 0 && group.meteredCostUsd === 0
                    ? "no price"
                    : "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {groups.length > 8 && (
        <button type="button" className="settings-pane__btn" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "Show less" : `Show all ${groups.length}`}
        </button>
      )}
    </section>
  );
}

function BudgetEditor({
  settings,
  busy,
  onSave,
}: {
  settings: UsageSettings;
  busy: boolean;
  onSave: (settings: UsageSettings) => Promise<void>;
}) {
  const [value, setValue] = useState(settings.dailyBudgetUsd ? String(settings.dailyBudgetUsd) : "");
  useEffect(() => {
    setValue(settings.dailyBudgetUsd ? String(settings.dailyBudgetUsd) : "");
  }, [settings.dailyBudgetUsd]);
  const parsed = Number.parseFloat(value.replace(",", "."));
  return (
    <section className="settings-pane__tool-group">
      <div className="settings-pane__tool-group-head">
        <h2>Daily budget</h2>
      </div>
      <form
        className="usage__inline-form"
        onSubmit={(event) => {
          event.preventDefault();
          void onSave({
            ...settings,
            dailyBudgetUsd: Number.isFinite(parsed) && parsed > 0 ? parsed : null,
          });
        }}
      >
        <span>$</span>
        <input
          className="settings-pane__tool-config-desc usage__number"
          inputMode="decimal"
          placeholder="none"
          value={value}
          onChange={(event) => setValue(event.target.value)}
        />
        <span>per day</span>
        <button type="submit" className="settings-pane__btn" disabled={busy}>
          Save
        </button>
      </form>
      <p className="settings-pane__field-hint">
        Counts pay-per-token providers only. Past it you get a notification; nothing is blocked.
      </p>
    </section>
  );
}

function PriceOverrides({
  settings,
  busy,
  onSave,
}: {
  settings: UsageSettings;
  busy: boolean;
  onSave: (settings: UsageSettings) => Promise<void>;
}) {
  const [model, setModel] = useState("");
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const entries = useMemo(
    () => Object.entries(settings.priceOverrides ?? {}),
    [settings.priceOverrides],
  );
  const number = (text: string) => Number.parseFloat(text.replace(",", "."));
  const write = (overrides: Record<string, PriceOverride>) =>
    onSave({ ...settings, priceOverrides: overrides });

  return (
    <section className="settings-pane__tool-group">
      <div className="settings-pane__tool-group-head">
        <h2>Custom prices</h2>
      </div>
      {entries.length > 0 && (
        <table className="usage__table">
          <thead>
            <tr>
              <th />
              <th>Input / M</th>
              <th>Output / M</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {entries.map(([key, price]) => (
              <tr key={key}>
                <td>{key}</td>
                <td>{formatUsd(price.inputPerM)}</td>
                <td>{formatUsd(price.outputPerM)}</td>
                <td>
                  <button
                    type="button"
                    className="settings-pane__icon-btn"
                    title="Remove"
                    disabled={busy}
                    onClick={() => {
                      const next = { ...settings.priceOverrides };
                      delete next[key];
                      void write(next);
                    }}
                  >
                    <Icon icon="solar:trash-bin-minimalistic-linear" width={13} height={13} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <form
        className="usage__inline-form"
        onSubmit={(event) => {
          event.preventDefault();
          const key = model.trim();
          if (!key.includes("/") || !Number.isFinite(number(input)) || !Number.isFinite(number(output))) {
            return;
          }
          void write({
            ...settings.priceOverrides,
            [key]: { inputPerM: number(input), outputPerM: number(output) },
          }).then(() => {
            setModel("");
            setInput("");
            setOutput("");
          });
        }}
      >
        <input
          className="settings-pane__tool-config-desc"
          placeholder="provider/model, e.g. kimi/kimi-k2.7-code-highspeed"
          value={model}
          onChange={(event) => setModel(event.target.value)}
        />
        <input
          className="settings-pane__tool-config-desc usage__number"
          inputMode="decimal"
          placeholder="$ in / M"
          value={input}
          onChange={(event) => setInput(event.target.value)}
        />
        <input
          className="settings-pane__tool-config-desc usage__number"
          inputMode="decimal"
          placeholder="$ out / M"
          value={output}
          onChange={(event) => setOutput(event.target.value)}
        />
        <button type="submit" className="settings-pane__btn" disabled={busy}>
          Add
        </button>
      </form>
      <p className="settings-pane__field-hint">
        Replaces the OpenRouter price for that model, in USD per million tokens. Applies to new
        responses; past ones keep the cost they were recorded with.
      </p>
    </section>
  );
}
