//! Models a provider reported live for the signed-in account.
//!
//! Each provider crate exposes a `list_models` that asks its backend which
//! models the current credential can use (Codex `/models`, Anthropic
//! `/v1/models`, Antigravity `fetchAvailableModels`, …). The desktop records
//! the result here so `capabilities()` can use the real context window and
//! modalities instead of a compiled-in guess, and so a model released after
//! this build still gets sane limits.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::ModelCapabilities;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    /// Id Sinew uses for the model (the `ModelRef` name).
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Provider-native effort names (`low`, `high`, `max`, …). Empty when the
    /// backend does not say.
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
    /// Id to send on the wire when it differs from `id` (Antigravity serves
    /// some models under a `-tiered` alias).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_id: Option<String>,
}

type Registry = HashMap<String, HashMap<String, DiscoveredModel>>;

fn registry() -> &'static RwLock<Registry> {
    static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Replace what we know about `provider` with a fresh listing.
pub fn record(provider: &str, models: &[DiscoveredModel]) {
    let entries = models
        .iter()
        .map(|model| (model.id.clone(), model.clone()))
        .collect();
    if let Ok(mut registry) = registry().write() {
        registry.insert(provider.to_string(), entries);
    }
}

pub fn lookup(provider: &str, id: &str) -> Option<DiscoveredModel> {
    registry().read().ok()?.get(provider)?.get(id).cloned()
}

/// Override compiled-in limits with what the provider reported, if anything.
pub fn apply(capabilities: &mut ModelCapabilities) {
    let Some(found) = lookup(&capabilities.model.provider, &capabilities.model.name) else {
        return;
    };
    if let Some(window) = found.context_window.filter(|window| *window > 0) {
        capabilities.context_window = window;
        // Keep the same ~10% headroom the static tables use.
        capabilities.preferred_window = window - window / 10;
    }
    // Only ever lower the output budget: the static tables cap it on purpose
    // (DeepSeek reports 384k, Sinew sticks to 64k).
    if let Some(max_output) = found.max_output_tokens.filter(|max| *max > 0) {
        capabilities.max_output_tokens = capabilities.max_output_tokens.min(max_output);
    }
    if let Some(images) = found.supports_images {
        capabilities.supports_images = images;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffortMode, ModelRef};

    fn caps(provider: &str, name: &str) -> ModelCapabilities {
        ModelCapabilities {
            model: ModelRef::new(provider, name),
            context_window: 1_000_000,
            preferred_window: 900_000,
            max_output_tokens: 128_000,
            supports_thinking: true,
            visible_thinking: true,
            supports_tools: true,
            supports_images: false,
            effort_mode: EffortMode::Tier,
        }
    }

    #[test]
    fn discovered_limits_override_static_ones() {
        record(
            "test-provider",
            &[DiscoveredModel {
                id: "model-a".into(),
                context_window: Some(272_000),
                supports_images: Some(true),
                ..Default::default()
            }],
        );
        let mut capabilities = caps("test-provider", "model-a");
        apply(&mut capabilities);
        assert_eq!(capabilities.context_window, 272_000);
        assert_eq!(capabilities.preferred_window, 244_800);
        assert!(capabilities.supports_images);
        assert_eq!(capabilities.max_output_tokens, 128_000);

        let mut untouched = caps("test-provider", "model-b");
        apply(&mut untouched);
        assert_eq!(untouched.context_window, 1_000_000);
    }
}
