import { useMemo, useSyncExternalStore } from "react";
import type {
  AgentMode,
  DiscoveredModel,
  DiscoveredModels,
  ModeModelSettings,
  ModelRef,
  OpenRouterModel,
  ThinkingLevel,
} from "../types";

export type ModelId = string;
export type ProviderId =
  | "anthropic"
  | "openai"
  | "google"
  | "kimi"
  | "deepseek"
  | "opencode-go"
  | "openrouter";
export type ModeModelSelection = { model: ModelId; thinking: ThinkingLevel };
export type ModeModelSelections = Record<AgentMode, ModeModelSelection>;

export type ModelEntry = {
  value: ModelId;
  provider: ProviderId;
  label: string;
  thinking: readonly ThinkingLevel[];
  defaultThinking: ThinkingLevel;
  supportsFast?: boolean;
};

export const PROVIDERS: {
  value: ProviderId;
  label: string;
  icon: string;
}[] = [
  {
    value: "anthropic",
    label: "Anthropic",
    icon: "simple-icons:anthropic",
  },
  {
    value: "openai",
    label: "OpenAI",
    icon: "simple-icons:openai",
  },
  {
    value: "google",
    label: "Google",
    icon: "simple-icons:google",
  },
  {
    value: "kimi",
    label: "Kimi",
    icon: "local:kimi",
  },
  {
    value: "deepseek",
    label: "DeepSeek",
    icon: "simple-icons:deepseek",
  },
  {
    value: "opencode-go",
    label: "OpenCode Go",
    icon: "solar:code-square-bold",
  },
  {
    value: "openrouter",
    label: "OpenRouter",
    icon: "simple-icons:openrouter",
  },
];

export const THINKING_LEVELS: { value: ThinkingLevel; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "XHigh" },
  { value: "max", label: "Max" },
];

export const MODELS: ModelEntry[] = [
  {
    value: "anthropic:claude-fable-5-1",
    provider: "anthropic",
    label: "Fable 5.1",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "high",
  },
  {
    value: "anthropic:claude-sonnet-5-5",
    provider: "anthropic",
    label: "Sonnet 5.5",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "high",
  },
  {
    value: "anthropic:claude-haiku-5-5",
    provider: "anthropic",
    label: "Haiku 5.5",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-fable-5",
    provider: "anthropic",
    label: "Fable 5",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-opus-5-5",
    provider: "anthropic",
    label: "Opus 5.5",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-opus-5",
    provider: "anthropic",
    label: "Opus 5",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-opus-4-8",
    provider: "anthropic",
    label: "Opus 4.8",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-opus-4-7",
    provider: "anthropic",
    label: "Opus 4.7",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-opus-4-6",
    provider: "anthropic",
    label: "Opus 4.6",
    thinking: ["off", "low", "medium", "high", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-sonnet-5",
    provider: "anthropic",
    label: "Sonnet 5",
    thinking: ["off", "low", "medium", "high", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-sonnet-4-6",
    provider: "anthropic",
    label: "Sonnet 4.6",
    thinking: ["off", "low", "medium", "high", "max"],
    defaultThinking: "medium",
  },
  {
    value: "anthropic:claude-haiku-4-5",
    provider: "anthropic",
    label: "Haiku 4.5",
    thinking: ["off", "low", "medium", "high"],
    defaultThinking: "medium",
  },
  {
    value: "openai:gpt-6-astra",
    provider: "openai",
    label: "GPT-6 Astra",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-6.1-sol",
    provider: "openai",
    label: "GPT-6.1 Sol",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-6.1-terra",
    provider: "openai",
    label: "GPT-6.1 Terra",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-6-sol",
    provider: "openai",
    label: "GPT-6 Sol",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-6-luna",
    provider: "openai",
    label: "GPT-6 Luna",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.6-sol",
    provider: "openai",
    label: "GPT-5.6 Sol",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.6-terra",
    provider: "openai",
    label: "GPT-5.6 Terra",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.6-luna",
    provider: "openai",
    label: "GPT-5.6 Luna",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.5",
    provider: "openai",
    label: "GPT-5.5",
    thinking: ["off", "low", "medium", "high", "xhigh"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.4",
    provider: "openai",
    label: "GPT-5.4",
    thinking: ["off", "low", "medium", "high", "xhigh"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.4-mini",
    provider: "openai",
    label: "GPT-5.4 Mini",
    thinking: ["off", "low", "medium", "high", "xhigh"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.3-codex",
    provider: "openai",
    label: "GPT-5.3 Codex",
    thinking: ["off", "low", "medium", "high", "xhigh"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.3-codex-spark",
    provider: "openai",
    label: "GPT-5.3 Codex Spark",
    thinking: ["low", "medium", "high", "xhigh"],
    defaultThinking: "low",
    supportsFast: true,
  },
  {
    value: "openai:gpt-5.2",
    provider: "openai",
    label: "GPT-5.2",
    thinking: ["off", "low", "medium", "high", "xhigh"],
    defaultThinking: "medium",
    supportsFast: true,
  },
  {
    value: "google:gemini-4-argon",
    provider: "google",
    label: "Gemini 4 Argon",
    thinking: ["low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3.5-flash-lite",
    provider: "google",
    label: "Gemini 3.5 Flash-Lite",
    thinking: ["minimal", "low", "medium", "high"],
    defaultThinking: "medium",
  },
  {
    value: "google:gemini-3.8-flash",
    provider: "google",
    label: "Gemini 3.8 Flash",
    thinking: ["low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3.7-flash",
    provider: "google",
    label: "Gemini 3.7 Flash",
    thinking: ["low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3.1-pro",
    provider: "google",
    label: "Gemini 3.1 Pro",
    thinking: ["low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3.6-flash",
    provider: "google",
    label: "Gemini 3.6 Flash",
    thinking: ["minimal", "low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3-flash",
    provider: "google",
    label: "Gemini 3 Flash",
    thinking: ["minimal", "low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "google:gemini-3.5-flash",
    provider: "google",
    label: "Gemini 3.5 Flash",
    thinking: ["minimal", "low", "medium", "high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-k3.1-pro",
    provider: "kimi",
    label: "Kimi K3.1 Pro",
    thinking: ["off", "high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-k3.1-pro-highspeed",
    provider: "kimi",
    label: "Kimi K3.1 Pro Highspeed",
    thinking: ["off", "high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-k3",
    provider: "kimi",
    label: "Kimi K3",
    thinking: ["off", "high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-k2.7-code-highspeed",
    provider: "kimi",
    label: "Kimi K2.7 Code Highspeed",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-k2.7-code",
    provider: "kimi",
    label: "Kimi K2.7 Code",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "kimi:kimi-for-coding",
    provider: "kimi",
    label: "Kimi K2.6",
    thinking: ["off", "high"],
    defaultThinking: "high",
  },
  {
    value: "deepseek:deepseek-flash",
    provider: "deepseek",
    label: "DeepSeek V4.1 Flash",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "deepseek:deepseek-v4-flash",
    provider: "deepseek",
    label: "DeepSeek V4 Flash (legacy)",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "deepseek:deepseek-v4-pro",
    provider: "deepseek",
    label: "DeepSeek V4 Pro",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:gpt-5.6-luna",
    provider: "opencode-go",
    label: "GPT-5.6 Luna",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:kimi-k3",
    provider: "opencode-go",
    label: "Kimi K3",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:kimi-k2.7-code",
    provider: "opencode-go",
    label: "Kimi K2.7 Code",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:kimi-k2.6",
    provider: "opencode-go",
    label: "Kimi K2.6",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:deepseek-v4-pro",
    provider: "opencode-go",
    label: "DeepSeek V4 Pro",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:deepseek-v4-flash",
    provider: "opencode-go",
    label: "DeepSeek V4 Flash",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    // OpenCode Go's `deepseek-flash` id maps to DeepSeek V4.1 Flash (per their
    // Go docs); the bare id would otherwise title-case to "Deepseek Flash".
    value: "opencode-go:deepseek-flash",
    provider: "opencode-go",
    label: "DeepSeek V4.1 Flash",
    thinking: ["off", "low", "medium", "high", "xhigh", "max"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:qwen3.8-max",
    provider: "opencode-go",
    label: "Qwen3.8 Max",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:qwen3.7-max",
    provider: "opencode-go",
    label: "Qwen3.7 Max",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:qwen3.7-plus",
    provider: "opencode-go",
    label: "Qwen3.7 Plus",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:qwen3.6-plus",
    provider: "opencode-go",
    label: "Qwen3.6 Plus",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:glm-5.3",
    provider: "opencode-go",
    label: "GLM-5.3",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:glm-5.2",
    provider: "opencode-go",
    label: "GLM-5.2",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:glm-5.1",
    provider: "opencode-go",
    label: "GLM-5.1",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:grok-4.5",
    provider: "opencode-go",
    label: "Grok 4.5",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:minimax-m3",
    provider: "opencode-go",
    label: "MiniMax M3",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:minimax-m2.7",
    provider: "opencode-go",
    label: "MiniMax M2.7",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:mimo-v2.5",
    provider: "opencode-go",
    label: "MiMo-V2.5",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:mimo-v2.5-pro",
    provider: "opencode-go",
    label: "MiMo-V2.5-Pro",
    thinking: ["high"],
    defaultThinking: "high",
  },
  {
    value: "opencode-go:hy3",
    provider: "opencode-go",
    label: "Hy3",
    thinking: ["high"],
    defaultThinking: "high",
  },
];

const OPENROUTER_THINKING: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh"];
const OPENROUTER_NO_THINKING: readonly ThinkingLevel[] = ["off"];

export function sanitizeOpenRouterName(name: string | null | undefined): string {
  const raw = (name ?? "").trim();
  if (!raw) return "";
  // OpenRouter prefixes most names with the underlying provider, e.g. "OpenAI: GPT-4o".
  // The provider icon already conveys that information in Sinew, so drop the prefix.
  const colon = raw.indexOf(":");
  if (colon <= 0) return raw;
  const tail = raw.slice(colon + 1).trim();
  return tail || raw;
}

/// Per provider: `order` lists models most powerful first, and each of `lines`
/// lists one product line newest first. A model is an older version only when
/// the account can use a newer one of its line, so a plan without the top
/// models keeps the best it has. OpenCode Go works it out from its labels (see
/// `earlierModels`) and OpenRouter is left as listed. A model missing from
/// here (released after this build) counts as current and comes first.
const MODEL_RANKING: Partial<
  Record<ProviderId, { order: readonly string[]; lines: readonly (readonly string[])[] }>
> = {
  anthropic: {
    order: [
      "claude-fable-5-1",
      "claude-opus-5-5",
      "claude-sonnet-5-5",
      "claude-haiku-5-5",
      "claude-fable-5",
      "claude-opus-5",
      "claude-opus-4-8",
      "claude-opus-4-7",
      "claude-opus-4-6",
      "claude-sonnet-5",
      "claude-sonnet-4-6",
      "claude-haiku-4-5",
    ],
    lines: [
      ["claude-fable-5-1", "claude-fable-5"],
      ["claude-opus-5-5", "claude-opus-5", "claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6"],
      ["claude-sonnet-5-5", "claude-sonnet-5", "claude-sonnet-4-6"],
      ["claude-haiku-5-5", "claude-haiku-4-5"],
    ],
  },
  openai: {
    order: [
      "gpt-6-astra",
      "gpt-6.1-sol",
      "gpt-6.1-terra",
      "gpt-6-sol",
      "gpt-6-luna",
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "gpt-5.6-luna",
      "gpt-5.5",
      "gpt-5.4",
      "gpt-5.3-codex",
      "gpt-5.2",
      "gpt-5.4-mini",
      "gpt-5.3-codex-spark",
    ],
    lines: [
      ["gpt-6.1-sol", "gpt-6-sol", "gpt-5.6-sol", "gpt-5.5", "gpt-5.4", "gpt-5.3-codex", "gpt-5.2"],
      ["gpt-6.1-terra", "gpt-5.6-terra"],
      ["gpt-6-luna", "gpt-5.6-luna", "gpt-5.4-mini", "gpt-5.3-codex-spark"],
    ],
  },
  google: {
    order: [
      "gemini-4-argon",
      "gemini-3.1-pro",
      "gemini-3.8-flash",
      "gemini-3.7-flash",
      "gemini-3.6-flash",
      "gemini-3.5-flash",
      "gemini-3-flash",
      "gemini-3.5-flash-lite",
    ],
    lines: [
      ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-3.6-flash", "gemini-3.5-flash", "gemini-3-flash"],
    ],
  },
  kimi: {
    order: [
      "kimi-k3.1-pro",
      "kimi-k3.1-pro-highspeed",
      "kimi-k3",
      "kimi-k2.7-code",
      "kimi-k2.7-code-highspeed",
      "kimi-for-coding",
    ],
    lines: [
      ["kimi-k3.1-pro", "kimi-k3", "kimi-k2.7-code", "kimi-for-coding"],
      ["kimi-k3.1-pro-highspeed", "kimi-k2.7-code-highspeed"],
    ],
  },
  deepseek: {
    order: ["deepseek-v4-pro", "deepseek-flash", "deepseek-v4-flash"],
    lines: [["deepseek-flash", "deepseek-v4-flash"]],
  },
};

function modelName(model: ModelEntry): string {
  return model.value.slice(model.provider.length + 1);
}

function modelRank(model: ModelEntry): number {
  const ranking = MODEL_RANKING[model.provider];
  if (!ranking) return -1;
  return ranking.order.indexOf(modelName(model));
}

/// OpenCode Go's vendors, in the order the picker groups them. Others follow
/// alphabetically.
const GO_FAMILIES: readonly (readonly [prefix: string, label: string])[] = [
  ["gpt", "GPT"],
  ["kimi", "Kimi"],
  ["deepseek", "DeepSeek"],
  ["qwen", "Qwen"],
  ["glm", "GLM"],
  ["grok", "Grok"],
  ["minimax", "MiniMax"],
  ["mimo", "MiMo"],
  ["hy", "Hy"],
];
/// Strongest first, for models of one family and version.
const GO_VARIANTS = ["max", "pro", "plus", "sol", "terra", "", "code", "flash", "luna", "lite", "mini"];

type GoModelParts = { family: string; version: number[]; variant: string };

/// `Qwen3.7 Max` as family `qwen`, version 3.7, variant `max`. It reads the
/// label because some ids hide the version (`deepseek-flash` is V4.1 Flash).
function goModelParts(model: ModelEntry): GoModelParts {
  const family = modelName(model).match(/^[a-z]+/i)?.[0].toLowerCase() ?? modelName(model);
  const label = model.label.toLowerCase();
  const rest = label.startsWith(family) ? label.slice(family.length) : label;
  const version = rest.match(/[a-z]*(\d+(?:\.\d+)*)/);
  const variant = rest
    .replace(version?.[0] ?? "", "")
    .replace(/\(.*?\)/g, "")
    .replace(/[^a-z0-9]/g, "");
  return {
    family,
    version: (version?.[1] ?? "0").split(".").map(Number),
    variant,
  };
}

function compareVersions(left: number[], right: number[]): number {
  for (let i = 0; i < Math.max(left.length, right.length); i++) {
    const diff = (left[i] ?? 0) - (right[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

function goFamilyIndex(family: string): number {
  const index = GO_FAMILIES.findIndex(([prefix]) => prefix === family);
  return index < 0 ? GO_FAMILIES.length : index;
}

/// By family, then newest version, then strongest variant.
function compareGoModels(left: ModelEntry, right: ModelEntry): number {
  const a = goModelParts(left);
  const b = goModelParts(right);
  const variantIndex = (variant: string) => {
    const index = GO_VARIANTS.indexOf(variant);
    return index < 0 ? GO_VARIANTS.length : index;
  };
  return (
    goFamilyIndex(a.family) - goFamilyIndex(b.family) ||
    a.family.localeCompare(b.family) ||
    compareVersions(b.version, a.version) ||
    variantIndex(a.variant) - variantIndex(b.variant) ||
    a.variant.localeCompare(b.variant)
  );
}

/// The vendor heading of an OpenCode Go model in the picker.
export function goFamilyLabel(model: ModelEntry): string {
  const { family } = goModelParts(model);
  return (
    GO_FAMILIES.find(([prefix]) => prefix === family)?.[1] ??
    family.charAt(0).toUpperCase() + family.slice(1)
  );
}

/// Most powerful first within each provider; providers keep their order.
function rankedModels(models: readonly ModelEntry[]): ModelEntry[] {
  const providers = [...new Set(models.map((model) => model.provider))];
  return providers.flatMap((provider) =>
    models
      .filter((model) => model.provider === provider)
      .sort(
        provider === "opencode-go"
          ? compareGoModels
          : (left, right) => modelRank(left) - modelRank(right),
      ),
  );
}

/// The older versions in `models`, which pickers hide: those with a newer one
/// of their line in the same list. OpenCode Go's list changes with the
/// gateway, so its lines are the family and variant (Qwen 3.7 Max next to
/// Qwen 3.8 Max, but not Qwen 3.7 Plus).
export function earlierModels(models: readonly ModelEntry[]): ModelEntry[] {
  const go = models.filter((model) => model.provider === "opencode-go").map(goModelParts);
  const listed = new Set(models.map((model) => model.value));
  return models.filter((model) => {
    if (model.provider !== "opencode-go") {
      const name = modelName(model);
      const line = MODEL_RANKING[model.provider]?.lines.find((line) => line.includes(name));
      return (line ?? [])
        .slice(0, line?.indexOf(name) ?? 0)
        .some((newer) => listed.has(`${model.provider}:${newer}`));
    }
    const parts = goModelParts(model);
    return go.some(
      (other) =>
        other.family === parts.family &&
        other.variant === parts.variant &&
        compareVersions(other.version, parts.version) > 0,
    );
  });
}

const ENABLED_EARLIER_KEY = "sinew.enabledEarlierModels";
const EARLIER_CHANGED_EVENT = "sinew:earlier-models-changed";

function readEnabledEarlierRaw(): string {
  try {
    return localStorage.getItem(ENABLED_EARLIER_KEY) ?? "[]";
  } catch {
    return "[]";
  }
}

function parseEnabledEarlier(raw: string): ModelId[] {
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? parsed.filter((id): id is string => typeof id === "string")
      : [];
  } catch {
    return [];
  }
}

export function setEarlierModelEnabled(id: ModelId, enabled: boolean) {
  const others = parseEnabledEarlier(readEnabledEarlierRaw()).filter((value) => value !== id);
  try {
    localStorage.setItem(
      ENABLED_EARLIER_KEY,
      JSON.stringify(enabled ? [...others, id] : others),
    );
  } catch {
    // Without storage the switch simply does not stick.
  }
  window.dispatchEvent(new CustomEvent(EARLIER_CHANGED_EVENT));
}

function subscribeEarlierModels(onChange: () => void) {
  window.addEventListener(EARLIER_CHANGED_EVENT, onChange);
  window.addEventListener("storage", onChange);
  return () => {
    window.removeEventListener(EARLIER_CHANGED_EVENT, onChange);
    window.removeEventListener("storage", onChange);
  };
}

/// The earlier versions the user turned on in Settings, kept in sync.
export function useEnabledEarlierModels(): readonly ModelId[] {
  const raw = useSyncExternalStore(subscribeEarlierModels, readEnabledEarlierRaw);
  return useMemo(() => parseEnabledEarlier(raw), [raw]);
}

/// What a model picker lists: no earlier versions, except the ones enabled
/// and the one in use (a selection must stay visible).
export function modelsForPicker(
  models: readonly ModelEntry[],
  enabledEarlier: readonly ModelId[],
  selected: ModelId | null | undefined,
): ModelEntry[] {
  const earlier = new Set(earlierModels(models).map((model) => model.value));
  return models.filter(
    (model) =>
      !earlier.has(model.value) ||
      enabledEarlier.includes(model.value) ||
      model.value === selected,
  );
}

export function modelsWithOpenRouter(
  openRouterModels: readonly OpenRouterModel[] = [],
  opencodeGoModels: readonly string[] = [],
  discoveredModels: DiscoveredModels = {},
): ModelEntry[] {
  const useDynamicGo = opencodeGoModels.length > 0;
  return [
    ...catalogWithDiscovered(discoveredModels).filter(
      (model) => !(useDynamicGo && model.provider === "opencode-go"),
    ),
    ...(useDynamicGo ? opencodeGoModelEntries(opencodeGoModels) : []),
    ...openRouterModelEntries(openRouterModels),
  ];
}

export function availableModelsForProviders(
  configuredProviders: readonly string[],
  openRouterModels: readonly OpenRouterModel[] = [],
  opencodeGoModels: readonly string[] = [],
  discoveredModels: DiscoveredModels = {},
): ModelEntry[] {
  const configured = new Set(configuredProviders);
  // When we have a live OpenCode Go model list, it replaces the static entries
  // (fresh ids + correct windows) instead of stacking duplicates.
  const useDynamicGo = opencodeGoModels.length > 0;
  return rankedModels([
    ...catalogWithDiscovered(discoveredModels).filter(
      (model) =>
        configured.has(model.provider) &&
        !(useDynamicGo && model.provider === "opencode-go"),
    ),
    ...(configured.has("opencode-go") && useDynamicGo
      ? opencodeGoModelEntries(opencodeGoModels)
      : []),
    ...(configured.has("openrouter") ? openRouterModelEntries(openRouterModels) : []),
  ]);
}

const THINKING_ORDER: readonly ThinkingLevel[] = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
];

/// Thinking levels for a model we only know from a live listing. Providers
/// that report their effort names (Codex, DeepSeek) are mapped onto Sinew's
/// levels (`ultra` has no equivalent and is dropped); otherwise each provider
/// gets the set its static entries use.
function discoveredThinking(
  provider: ProviderId,
  model: DiscoveredModel,
): { thinking: readonly ThinkingLevel[]; defaultThinking: ThinkingLevel } {
  if (provider === "kimi") return { thinking: ["off", "high"], defaultThinking: "high" };
  // DeepSeek and Kimi are on/off on the wire (EffortMode::Flag).
  if (provider === "deepseek") return { thinking: ["high"], defaultThinking: "high" };
  if (provider === "google") {
    return model.id.includes("flash")
      ? { thinking: ["minimal", "low", "medium", "high"], defaultThinking: "high" }
      : { thinking: ["low", "medium", "high"], defaultThinking: "high" };
  }
  const reported = new Set<string>(model.reasoningLevels);
  const levels: ThinkingLevel[] =
    reported.size > 0
      ? THINKING_ORDER.filter((level) => level === "off" || reported.has(level))
      : ["off", "low", "medium", "high", "xhigh", "max"];
  const reportedDefault = levels.find((level) => level === model.defaultReasoning);
  return {
    thinking: levels,
    defaultThinking: reportedDefault ?? (levels.includes("medium") ? "medium" : levels[levels.length - 1]),
  };
}

/// Readable label for a listed model the static catalog does not know:
/// "GPT-6.1-Sol" → "GPT-6.1 Sol", "gemini-4-argon" → "Gemini 4 Argon".
function discoveredLabel(model: DiscoveredModel): string {
  const raw = model.label?.trim();
  if (raw) return raw.replace(/-(?=[A-Za-z])/g, " ");
  return model.id
    .split("-")
    .map((part) => (/^\d/.test(part) ? part : part.charAt(0).toUpperCase() + part.slice(1)))
    .join(" ")
    .replace(/^Gpt /, "GPT-");
}

/// The static catalog with every provider that answered a live listing
/// replaced by what the account can actually use: known models keep their
/// curated label/levels, new ones are derived from the listing. Providers
/// without a listing (offline, not connected) keep the static entries.
function catalogWithDiscovered(discoveredModels: DiscoveredModels): ModelEntry[] {
  const replaced = new Set(
    Object.entries(discoveredModels)
      .filter(([, models]) => models.length > 0)
      .map(([provider]) => provider),
  );
  if (replaced.size === 0) return MODELS;
  const entries: ModelEntry[] = [];
  for (const provider of PROVIDERS.map((p) => p.value)) {
    const statics = MODELS.filter((model) => model.provider === provider);
    if (!replaced.has(provider)) {
      entries.push(...statics);
      continue;
    }
    const listed = discoveredModels[provider];
    const listedIds = new Set(listed.map((model) => modelId(provider, model.id)));
    const unknown = listed.filter(
      (model) => !statics.some((entry) => entry.value === modelId(provider, model.id)),
    );
    // New models first (they are usually the newest), then the known ones in
    // their curated order.
    for (const model of unknown) {
      entries.push({
        value: modelId(provider, model.id),
        provider,
        label: discoveredLabel(model),
        ...discoveredThinking(provider, model),
        supportsFast: provider === "openai" ? true : undefined,
      });
    }
    entries.push(...statics.filter((entry) => listedIds.has(entry.value)));
  }
  return entries;
}

// Per-family reasoning levels for OpenCode Go. Determined empirically by
// probing the gateway (2026-09-08): each candidate `reasoning_effort` /
// `reasoning.effort` was sent per model and we kept the ones it accepts (200)
// vs rejects (400). `minimal` is dropped — the backend clamps it to `low` for
// this provider, so it's never distinct. A couple of families ignore effort
// entirely (a bogus value still returns 200), so they only get on/off.
const GO_FULL: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh", "max"];
const GO_NO_MAX: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh"];
const GO_ONOFF: readonly ThinkingLevel[] = ["off", "high"]; // minimax / longcat: effort ignored
const GO_GROK: readonly ThinkingLevel[] = ["low", "medium", "high", "xhigh", "max"]; // always reasons
const GO_GPT: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh", "max"];
const GO_MUSE: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh"]; // no max
const GO_DEFAULT: readonly ThinkingLevel[] = ["off", "low", "medium", "high"];

function opencodeGoThinking(id: string): {
  thinking: readonly ThinkingLevel[];
  defaultThinking: ThinkingLevel;
} {
  // /responses families first (Grok / GPT-Luna / Muse).
  if (id.startsWith("grok")) return { thinking: GO_GROK, defaultThinking: "high" };
  if (id.startsWith("gpt")) return { thinking: GO_GPT, defaultThinking: "high" };
  if (id.startsWith("muse")) return { thinking: GO_MUSE, defaultThinking: "high" };
  // Effort has no graded effect on these — only on/off.
  if (id.startsWith("minimax") || id.startsWith("longcat"))
    return { thinking: GO_ONOFF, defaultThinking: "high" };
  // Graded reasoning families (all accept low→max).
  if (id.startsWith("deepseek")) return { thinking: GO_FULL, defaultThinking: "high" };
  if (id.startsWith("kimi")) return { thinking: GO_FULL, defaultThinking: "high" };
  if (id.startsWith("glm")) return { thinking: GO_FULL, defaultThinking: "high" };
  if (id.startsWith("qwen") && id.includes("-max"))
    return { thinking: GO_FULL, defaultThinking: "xhigh" };
  if (id.startsWith("qwen")) return { thinking: GO_NO_MAX, defaultThinking: "high" }; // plus tiers: no max
  if (id.startsWith("mimo")) return { thinking: GO_FULL, defaultThinking: "high" };
  if (id.startsWith("hy")) return { thinking: GO_FULL, defaultThinking: "high" }; // hunyuan
  // Anything unknown/new gets a safe standard set.
  return { thinking: GO_DEFAULT, defaultThinking: "medium" };
}

/// Human label for an OpenCode Go id: reuse a curated static label when the id
/// is one we know, else title-case the id (e.g. "deepseek-v4-pro" -> "Deepseek
/// V4 Pro").
function opencodeGoLabel(id: string): string {
  const known = MODELS.find((m) => m.value === modelId("opencode-go", id));
  if (known) return known.label;
  return id
    .split("-")
    .map((seg) =>
      /\d/.test(seg) ? seg.toUpperCase() : seg.charAt(0).toUpperCase() + seg.slice(1),
    )
    .join(" ");
}

function opencodeGoModelEntries(ids: readonly string[]): ModelEntry[] {
  return ids.map((id) => {
    const { thinking, defaultThinking } = opencodeGoThinking(id);
    return {
      value: modelId("opencode-go", id),
      provider: "opencode-go",
      label: opencodeGoLabel(id),
      thinking,
      defaultThinking,
    };
  });
}

function openRouterModelEntries(
  openRouterModels: readonly OpenRouterModel[],
): ModelEntry[] {
  return openRouterModels.map((model) => ({
    value: modelId("openrouter", model.id),
    provider: "openrouter",
    label: sanitizeOpenRouterName(model.name) || model.id,
    thinking: model.supportsThinking ? OPENROUTER_THINKING : OPENROUTER_NO_THINKING,
    defaultThinking: model.supportsThinking ? "medium" : "off",
  }));
}

export function modelIdFromRef(model: ModelRef | null | undefined): ModelId {
  if (model?.provider && model.name) {
    return modelId(model.provider, normalizedModelName(model.provider, model.name));
  }
  return MODELS[0].value;
}

/// Human-readable label for a `ModelRef` — e.g. `"Opus 4.7"` for a
/// registered Anthropic model, or `"Anthropic claude-foo"` as a fallback
/// when the exact id isn't in our catalog (custom OpenRouter slugs, future
/// models, etc.). Returns `null` when no model is provided so callers can
/// render their own placeholder.
export function labelForModelRef(model: ModelRef | null | undefined): string | null {
  if (!model?.provider || !model?.name) return null;
  const id = modelIdFromRef(model);
  const entry = MODELS.find((m) => m.value === id);
  if (entry) return entry.label;
  const providerLabel =
    PROVIDERS.find((p) => p.value === model.provider)?.label ?? model.provider;
  return `${providerLabel} ${model.name}`;
}

export function modelRefFromId(model: ModelId): ModelRef {
  const separator = model.indexOf(":");
  if (separator < 0) return { provider: "anthropic", name: model };
  const provider = model.slice(0, separator);
  const name = model.slice(separator + 1);
  return { provider, name };
}

export function thinkingFromRef(
  model: ModelRef | null | undefined,
): ThinkingLevel {
  if (model?.provider === "google") {
    if (model.name.endsWith("-low")) return "low";
    if (model.name.endsWith("-medium")) return "medium";
    if (model.name.endsWith("-high")) return "high";
    if (model.effort === "low" || model.effort === "medium" || model.effort === "high") {
      return model.effort;
    }
    if (model.effort === "none") {
      // Pro variants don't support `minimal`; clamp to low so we never send
      // an invalid thinking level for those models.
      return model.name.includes("-pro") ? "low" : "minimal";
    }
    return "high";
  }
  if (model?.provider === "kimi") {
    if (model.name === "kimi-k2.7-code") return "high";
    if (model.effort === "none") return "off";
    return "high";
  }
  if (model?.provider === "openrouter") {
    if (model.effort === "none") return "off";
    if (
      model.effort === "low" ||
      model.effort === "medium" ||
      model.effort === "high" ||
      model.effort === "xhigh"
    ) {
      return model.effort;
    }
    if (model.effort === "max") {
      return "xhigh";
    }
    return "medium";
  }
  if (
    model?.provider === "openai" &&
    model.name === "gpt-5.3-codex-spark" &&
    model.effort === "none"
  ) {
    return "low";
  }
  if (model?.effort === "none") return "off";
  if (model?.effort === "xhigh") return "xhigh";
  if (
    model?.provider === "openai" &&
    model.effort === "max" &&
    !supportsOpenAiMaxEffort(model.name)
  ) {
    return "xhigh";
  }
  if (
    model?.effort === "low" ||
    model?.effort === "medium" ||
    model?.effort === "high" ||
    model?.effort === "max"
  ) {
    return model.effort;
  }
  return "medium";
}

export function modelRefWithThinking(
  model: ModelRef,
  thinking: ThinkingLevel,
): ModelRef {
  if (model.provider === "google") {
    const name = normalizedGoogleModelName(model.name);
    if (thinking === "off") return { ...model, name, effort: "low" };
    if (thinking === "minimal") {
      // Pro variants reject `minimal` server-side; clamp to low.
      return name.includes("-pro")
        ? { ...model, name, effort: "low" }
        : { ...model, name, effort: "none" };
    }
    if (thinking === "xhigh" || thinking === "max") return { ...model, name, effort: "high" };
    return { ...model, name, effort: thinking };
  }
  if (model.provider === "kimi" && model.name === "kimi-k2.7-code") {
    return { ...model, effort: "high" };
  }
  if (
    model.provider === "openai" &&
    model.name === "gpt-5.3-codex-spark" &&
    thinking === "off"
  ) {
    return { ...model, effort: "low" };
  }
  if (thinking === "off") return { ...model, effort: "none" };
  if (model.provider === "kimi") return { ...model, effort: "high" };
  if (model.provider === "openrouter" && thinking === "max") {
    return { ...model, effort: "xhigh" };
  }
  // `minimal` is Gemini-only on the backend. The Google branch above already
  // handled it; for any other provider that ever surfaces it, clamp to low.
  if (thinking === "minimal") return { ...model, effort: "low" };
  return { ...model, effort: thinking };
}

export function selectionFromRef(
  model: ModelRef | null | undefined,
): ModeModelSelection {
  return {
    model: modelIdFromRef(model),
    thinking: thinkingFromRef(model),
  };
}

function modelId(provider: string, name: string): ModelId {
  return `${provider}:${name}`;
}

function supportsOpenAiMaxEffort(modelName: string): boolean {
  // Mirrors `supports_max_reasoning_effort` in the OpenAI crate: GPT-5.6 and
  // the whole GPT-6 generation (6-*, 6.1-*) accept `max`.
  return (
    modelName === "gpt-5.6" ||
    modelName.startsWith("gpt-5.6-") ||
    modelName === "gpt-6" ||
    modelName.startsWith("gpt-6-") ||
    modelName.startsWith("gpt-6.")
  );
}

function normalizedModelName(provider: string, name: string): string {
  if (provider === "google") return normalizedGoogleModelName(name);
  return name;
}

function normalizedGoogleModelName(name: string): string {
  if (name === "gemini-3.1-pro-preview") return "gemini-3.1-pro";
  if (name === "gemini-3-flash-preview") return "gemini-3-flash";
  if (name === "gemini-3.1-pro-low" || name === "gemini-3.1-pro-high") {
    return "gemini-3.1-pro";
  }
  if (
    name === "gemini-3.5-flash-low" ||
    name === "gemini-3.5-flash-medium" ||
    name === "gemini-3.5-flash-high"
  ) {
    return "gemini-3.5-flash";
  }
  return name;
}

export function selectionsFromSettings(
  settings: ModeModelSettings | null | undefined,
  fallback: ModelRef,
): ModeModelSelections {
  return {
    act: selectionFromRef(settings?.act ?? fallback),
    plan: selectionFromRef(settings?.plan ?? fallback),
    goal: selectionFromRef(settings?.goal ?? settings?.act ?? fallback),
  };
}
