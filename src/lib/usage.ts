/// Display helpers for usage, cost and plan-limit figures.

import type { QuotaSnapshot, QuotaWindow } from "../types";

export function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(1)}B`;
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(tokens >= 10_000_000 ? 0 : 1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(tokens >= 10_000 ? 0 : 1)}k`;
  return String(tokens);
}

export function formatUsd(value: number): string {
  if (value === 0) return "$0";
  if (value < 0.01) return "<$0.01";
  if (value < 100) return `$${value.toFixed(2)}`;
  return `$${Math.round(value).toLocaleString("en-US")}`;
}

/// Minutes to add to UTC to get local time (the opposite sign of
/// Date#getTimezoneOffset).
export function utcOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

const PROVIDER_LABELS: Record<string, string> = {
  anthropic: "Claude",
  openai: "ChatGPT",
  google: "Gemini",
  kimi: "Kimi",
  deepseek: "DeepSeek",
  openrouter: "OpenRouter",
  "opencode-go": "OpenCode Go",
};

export function providerLabel(provider: string): string {
  return PROVIDER_LABELS[provider] ?? provider;
}

/// "ChatGPT · Plus".
export function quotaTitle(snapshot: QuotaSnapshot): string {
  const plan = snapshot.plan
    ? ` · ${snapshot.plan.charAt(0).toUpperCase()}${snapshot.plan.slice(1)}`
    : "";
  return providerLabel(snapshot.provider) + plan;
}

function spanLabel(minutes: number): string {
  if (Math.abs(minutes - 300) <= 5) return "5-hour limit";
  if (Math.abs(minutes - 10_080) <= 60) return "Weekly";
  if (minutes >= 40_000 && minutes <= 46_000) return "30-day limit";
  if (minutes >= 1440) return `${Math.round(minutes / 1440)}-day limit`;
  return `${Math.round(minutes / 60)}-hour limit`;
}

/// What a window limits beyond the whole plan: `7d_opus` → "opus",
/// `primary:code review` → "code review".
function windowScope(label: string): string | null {
  const colon = label.indexOf(":");
  if (colon >= 0) return label.slice(colon + 1);
  if (label.startsWith("7d_")) return label.slice(3).replace(/_/g, " ");
  return null;
}

export function windowLabel(window: QuotaWindow): string {
  const scope = windowScope(window.label);
  const span = window.windowMinutes
    ? spanLabel(window.windowMinutes)
    : window.label === "5h"
      ? "5-hour limit"
      : window.label.startsWith("7d")
        ? "Weekly"
        : window.label.split(":")[0];
  return scope ? `${span} · ${scope}` : span;
}

/// "Resets at 13:00" today, "Resets Thu 19:00" this week, a date beyond.
export function resetLabel(at?: number): string | null {
  if (!at) return null;
  const when = new Date(at);
  const now = new Date();
  if (at <= now.getTime()) return "Resets now";
  const time = when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (when.toDateString() === now.toDateString()) return `Resets at ${time}`;
  if (at - now.getTime() < 6 * 24 * 60 * 60 * 1000) {
    return `Resets ${when.toLocaleDateString([], { weekday: "short" })} ${time}`;
  }
  return `Resets ${when.toLocaleDateString([], { day: "numeric", month: "short" })}`;
}

export function quotaLevel(percent: number): "ok" | "warn" | "danger" {
  return percent >= 90 ? "danger" : percent >= 70 ? "warn" : "ok";
}

/// Sinew provider id → the provider whose plan limits apply.
export function quotaProvider(provider: string | undefined): string | null {
  if (provider === "anthropic") return "anthropic";
  if (provider === "openai") return "openai";
  if (provider === "opencode-go") return "opencode-go";
  return null;
}
