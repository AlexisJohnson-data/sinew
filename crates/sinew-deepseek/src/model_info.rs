use sinew_core::{EffortMode, ModelCapabilities, ModelRef};

pub const PROVIDER_ID: &str = "deepseek";
pub const MODEL_ID: &str = "deepseek-v4-flash";
pub const MODEL_WINDOW: u32 = 1_000_000;
pub const MODEL_MAX_OUTPUT: u32 = 65_536;

struct DeepSeekModelInfo {
    id: &'static str,
    context_window: u32,
    preferred_window: u32,
    max_output_tokens: u32,
}

// DeepSeek V4 exposes a 1M-token context and up to 384k output tokens; we cap
// the output budget at a saner 64k, in line with the other providers. Both
// hosted V4 models share the same window. The call name stays `deepseek-v4-*`
// even as DeepSeek ships dated checkpoints (e.g. -0731) behind it.
const MODELS: &[DeepSeekModelInfo] = &[
    DeepSeekModelInfo {
        id: "deepseek-v4-flash",
        context_window: MODEL_WINDOW,
        preferred_window: 950_000,
        max_output_tokens: MODEL_MAX_OUTPUT,
    },
    DeepSeekModelInfo {
        id: "deepseek-v4-pro",
        context_window: MODEL_WINDOW,
        preferred_window: 950_000,
        max_output_tokens: MODEL_MAX_OUTPUT,
    },
];

fn model_info(model_id: &str) -> &'static DeepSeekModelInfo {
    MODELS
        .iter()
        .find(|info| info.id == model_id)
        .unwrap_or(&MODELS[0])
}

pub fn capabilities(model: &ModelRef) -> ModelCapabilities {
    let info = model_info(&model.name);
    ModelCapabilities {
        model: model.clone(),
        context_window: info.context_window,
        preferred_window: info.preferred_window,
        max_output_tokens: info.max_output_tokens,
        supports_thinking: true,
        visible_thinking: true,
        supports_tools: true,
        // DeepSeek V4 (api.deepseek.com) is text-only: the `/chat/completions`
        // endpoint rejects `image_url` content parts ("unknown variant
        // 'image_url', expected 'text'"). Images in history are downgraded to a
        // text placeholder by the client instead of being sent.
        supports_images: false,
        effort_mode: EffortMode::Flag,
    }
}
