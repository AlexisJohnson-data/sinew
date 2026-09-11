use sinew_core::{EffortMode, ModelCapabilities, ModelRef};

pub const PROVIDER_ID: &str = "opencode-go";
pub const MODEL_ID: &str = "gpt-5.6-luna";
// Conservative default window for ids we have not researched. Real per-model
// windows are resolved in `context_window_for`.
pub const MODEL_WINDOW: u32 = 262_144;
pub const MODEL_MAX_OUTPUT: u32 = 65_536;

// OpenCode Go is a flat-rate subscription that fronts ~30 curated open-source
// coding models behind an OpenAI-compatible endpoint
// (https://opencode.ai/zen/go/v1). Its `/models` listing is the minimal OpenAI
// shape (ids only, no capability metadata), so the app fetches the *ids*
// dynamically and this module supplies the capabilities the API omits:
// context window and vision support, resolved per id (with a family fallback so
// a brand-new version in a known line still gets a sensible window without a
// rebuild). Windows are transcribed from vendor docs / Artificial Analysis as
// of 2026-08; correct a value here if a model is resized.
//
// This static list is only the offline fallback for the picker — the live set
// comes from the `/models` endpoint. Keep it roughly in sync with reality but
// it does not gate anything (the client sends whatever id it is given).
pub const MODELS: &[&str] = &[
    "gpt-5.6-luna",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
    "kimi-k2.5",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-flash",
    "deepseek-v4-flash-vision-exp",
    "qwen3.8-max",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-plus",
    "qwen3.5-plus",
    "glm-5.3",
    "glm-5.2",
    "glm-5.1",
    "glm-5",
    "grok-4.5",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "mimo-v2.5",
    "mimo-v2.5-pro",
    "mimo-v2-pro",
    "mimo-v2-omni",
    "hy3",
    "hy3-preview",
    "longcat-2.0",
];

const K_256: u32 = 262_144;
const K_1M: u32 = 1_048_576;

/// Resolve `(context_window, supports_images)` for an OpenCode Go model id.
/// Explicit, researched ids win; anything else falls back to a family guess and
/// finally a conservative 256k default so an unknown/new id still works.
fn context_window_for(id: &str) -> (u32, bool) {
    match id {
        // Multimodal (vision) models.
        "kimi-k3" => (K_1M, true),
        "deepseek-v4-flash-vision-exp" => (K_1M, true),
        "mimo-v2-omni" => (K_256, true),

        // 1M-context, text.
        "minimax-m3"
        | "deepseek-v4-pro"
        | "deepseek-v4-flash"
        | "deepseek-flash" // DeepSeek V4.1 Flash
        | "qwen3.7-max"
        | "qwen3.8-max"
        | "glm-5.2"
        | "glm-5.3"
        | "mimo-v2-pro"
        | "mimo-v2.5"
        | "mimo-v2.5-pro" => (K_1M, false),

        "grok-4.5" => (500_000, false),
        "gpt-5.6-luna" => (400_000, false),

        "minimax-m2.7" | "minimax-m2.5" => (204_800, false),

        // 256k tier.
        "kimi-k2.7-code" | "kimi-k2.6" | "kimi-k2.5" | "glm-5" | "glm-5.1" | "hy3"
        | "hy3-preview" | "qwen3.7-plus" | "qwen3.6-plus" | "qwen3.5-plus" | "longcat-2.0"
        | "ox-alpha-free" | "muse-spark-1.2-contributor" => (K_256, false),

        other => family_fallback(other),
    }
}

/// Best-effort window for an id we have not curated, keyed by version-aware
/// family prefixes. Versions can jump (Kimi K2.x = 256k but K3 = 1M), so newer
/// lines are matched explicitly before the older-family default.
fn family_fallback(id: &str) -> (u32, bool) {
    let vision = id.contains("vision") || id.contains("omni");
    // All DeepSeek lines on the gateway (v4-pro/flash, v4.1 `deepseek-flash`,
    // and future ids) are 1M-context.
    if id.starts_with("deepseek") {
        return (K_1M, vision);
    }
    if id.starts_with("minimax-m3") {
        return (K_1M, false);
    }
    if id.starts_with("minimax-") {
        return (204_800, false);
    }
    if id.starts_with("qwen") && id.contains("-max") {
        return (K_1M, false);
    }
    if id.starts_with("qwen") {
        return (K_256, false);
    }
    // GLM 5.2+ moved to 1M; 5 / 5.1 stayed at 200k.
    if id.starts_with("glm-5.2") || id.starts_with("glm-5.3") || id.starts_with("glm-6") {
        return (K_1M, false);
    }
    if id.starts_with("glm-") {
        return (K_256, false);
    }
    // Kimi K3+ is 1M; K2.x is 256k.
    if id.starts_with("kimi-k3") || id.starts_with("kimi-k4") {
        return (K_1M, false);
    }
    if id.starts_with("kimi-") {
        return (K_256, false);
    }
    if id.starts_with("grok-") {
        return (500_000, false);
    }
    if id.starts_with("gpt-") {
        return (400_000, false);
    }
    if id.starts_with("mimo-") {
        return (K_1M, vision);
    }
    if id.starts_with("hy") {
        return (K_256, false);
    }
    (K_256, false)
}

/// OpenCode Go serves the "US" model families (Grok, GPT/Luna, Muse) through
/// the OpenAI **Responses** API (`/responses`); `/chat/completions` returns a
/// 5xx "Endpoint is unavailable" for them. Everything else (the Chinese open
/// models — DeepSeek, Kimi, GLM, Qwen, MiniMax, MiMo, …) uses
/// `/chat/completions`. Verified 2026-09-07 by probing the gateway.
pub fn uses_responses_api(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    id.starts_with("grok") || id.starts_with("gpt") || id.starts_with("muse")
}

pub fn capabilities(model: &ModelRef) -> ModelCapabilities {
    let (window, supports_images) = context_window_for(&model.name);
    ModelCapabilities {
        model: model.clone(),
        context_window: window,
        // Aim the working window at the model's real ceiling, but keep a sane
        // cap so a 1M model doesn't try to pack a million tokens per turn.
        preferred_window: window.min(250_000),
        max_output_tokens: MODEL_MAX_OUTPUT,
        supports_thinking: true,
        visible_thinking: true,
        supports_tools: true,
        supports_images,
        effort_mode: EffortMode::Flag,
    }
}
