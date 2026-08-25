import type {
  AgentMode,
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
    value: "anthropic:claude-fable-5",
    provider: "anthropic",
    label: "Fable 5",
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
    value: "deepseek:deepseek-v4-flash",
    provider: "deepseek",
    label: "DeepSeek V4 Flash",
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

export function modelsWithOpenRouter(
  openRouterModels: readonly OpenRouterModel[] = [],
  opencodeGoModels: readonly string[] = [],
): ModelEntry[] {
  const useDynamicGo = opencodeGoModels.length > 0;
  return [
    ...MODELS.filter((model) => !(useDynamicGo && model.provider === "opencode-go")),
    ...(useDynamicGo ? opencodeGoModelEntries(opencodeGoModels) : []),
    ...openRouterModelEntries(openRouterModels),
  ];
}

export function availableModelsForProviders(
  configuredProviders: readonly string[],
  openRouterModels: readonly OpenRouterModel[] = [],
  opencodeGoModels: readonly string[] = [],
): ModelEntry[] {
  const configured = new Set(configuredProviders);
  // When we have a live OpenCode Go model list, it replaces the static entries
  // (fresh ids + correct windows) instead of stacking duplicates.
  const useDynamicGo = opencodeGoModels.length > 0;
  return [
    ...MODELS.filter(
      (model) =>
        configured.has(model.provider) &&
        !(useDynamicGo && model.provider === "opencode-go"),
    ),
    ...(configured.has("opencode-go") && useDynamicGo
      ? opencodeGoModelEntries(opencodeGoModels)
      : []),
    ...(configured.has("openrouter") ? openRouterModelEntries(openRouterModels) : []),
  ];
}

// Per-family reasoning levels for OpenCode Go, from vendor docs + Artificial
// Analysis (2026-08). We only expose levels a model actually accepts — an
// unsupported / redundant `reasoning_effort` is misleading (and DeepSeek, for
// one, aliases medium/xhigh to high, so we don't offer them there). Unknown ids
// get a safe standard set.
const GO_DEEPSEEK: readonly ThinkingLevel[] = ["off", "low", "high", "max"];
const GO_KIMI: readonly ThinkingLevel[] = ["off", "low", "high", "max"];
const GO_GLM: readonly ThinkingLevel[] = ["off", "high", "max"];
const GO_QWEN_MAX: readonly ThinkingLevel[] = ["low", "medium", "xhigh"];
const GO_GROK: readonly ThinkingLevel[] = ["high"]; // cannot be disabled
const GO_GPT: readonly ThinkingLevel[] = ["off", "low", "medium", "high", "xhigh", "max"];
const GO_ONOFF: readonly ThinkingLevel[] = ["off", "high"]; // minimax / mimo / hunyuan
const GO_DEFAULT: readonly ThinkingLevel[] = ["off", "low", "medium", "high"];

function opencodeGoThinking(id: string): {
  thinking: readonly ThinkingLevel[];
  defaultThinking: ThinkingLevel;
} {
  if (id.startsWith("deepseek")) return { thinking: GO_DEEPSEEK, defaultThinking: "high" };
  if (id.startsWith("kimi")) return { thinking: GO_KIMI, defaultThinking: "high" };
  if (id.startsWith("glm")) return { thinking: GO_GLM, defaultThinking: "high" };
  if (id.startsWith("qwen") && id.includes("-max"))
    return { thinking: GO_QWEN_MAX, defaultThinking: "xhigh" };
  if (id.startsWith("grok")) return { thinking: GO_GROK, defaultThinking: "high" };
  if (id.startsWith("gpt")) return { thinking: GO_GPT, defaultThinking: "high" };
  if (id.startsWith("minimax") || id.startsWith("mimo") || id.startsWith("hy"))
    return { thinking: GO_ONOFF, defaultThinking: "high" };
  // qwen (non-max plus tiers), longcat, and anything unknown.
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
  return modelName === "gpt-5.6" || modelName.startsWith("gpt-5.6-");
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
