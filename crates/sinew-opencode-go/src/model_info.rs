use sinew_core::{EffortMode, ModelCapabilities, ModelRef};

pub const PROVIDER_ID: &str = "opencode-go";
pub const MODEL_ID: &str = "gpt-5.6-luna";
pub const MODEL_WINDOW: u32 = 262_144;
pub const MODEL_MAX_OUTPUT: u32 = 65_536;

// OpenCode Go is a flat-rate subscription ($5 first month, then $10/mo) that
// fronts ~19 curated open-source coding models behind an OpenAI-compatible
// endpoint (https://opencode.ai/zen/go/v1). Its `/models` listing is the
// minimal OpenAI shape (ids only, no capability metadata), so we carry a
// curated id list with uniform, conservative capabilities instead of trying to
// infer per-model windows. The ids are the API model ids — the `<model-id>`
// half of OpenCode's documented `opencode-go/<model-id>` form.
//
// These ids are transcribed from the OpenCode Go docs. If the gateway rejects
// one, correct the string here (and in `src/lib/models.ts`); a wrong id is the
// only failure mode and it is isolated to that single model.
pub const MODELS: &[&str] = &[
    "gpt-5.6-luna",
    "kimi-k3",
    "kimi-k2.7-code",
    "kimi-k2.6",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "qwen3.8-max",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-plus",
    "glm-5.3",
    "glm-5.2",
    "glm-5.1",
    "grok-4.5",
    "minimax-m3",
    "minimax-m2.7",
    "mimo-v2.5",
    "mimo-v2.5-pro",
    "hy3",
];

pub fn capabilities(model: &ModelRef) -> ModelCapabilities {
    ModelCapabilities {
        model: model.clone(),
        context_window: MODEL_WINDOW,
        preferred_window: 250_000,
        max_output_tokens: MODEL_MAX_OUTPUT,
        supports_thinking: true,
        visible_thinking: true,
        supports_tools: true,
        // Conservative: treat every Go model as text-only. The client downgrades
        // images in history to a text placeholder, so multimodal history never
        // gets a 400. Refine per-model later if a Go model gains vision.
        supports_images: false,
        effort_mode: EffortMode::Flag,
    }
}
