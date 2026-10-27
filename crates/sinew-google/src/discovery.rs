//! Live model listing for the signed-in Antigravity account
//! (`v1internal:fetchAvailableModels`).
//!
//! Antigravity only serves a subset of the public Gemini line-up per account,
//! sometimes under an alias (`gemini-3.8-flash-tiered`). The listing gives the
//! exact wire id, so new Gemini models route correctly without a code change.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sinew_core::{AppError, DiscoveredModel, Result};

use crate::auth::{load_default_user_data, Credential};
use crate::client::{antigravity_user_agent, PROD_BASE_URL};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Thinking tiers Antigravity bakes into some wire ids (`-low`, `-high`, …).
/// Those are routed by `antigravity_model_and_thinking`, not pinned here.
const THINKING_SUFFIXES: &[&str] = &["-minimal", "-low", "-medium", "-high"];
/// Non-chat artefacts the listing also returns.
const EXCLUDED: &[&str] = &["image", "tts", "live", "embedding", "transcribe", "agent"];

#[derive(Deserialize)]
struct AvailableModelsResponse {
    #[serde(default)]
    models: BTreeMap<String, AvailableModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableModel {
    max_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
    supports_images: Option<bool>,
    #[serde(default)]
    is_internal: bool,
}

/// Gemini models the stored Google credential can use. Empty when Google is
/// not connected.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(Vec::new());
    };
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let token = credential.bearer(&http).await?;
    let project = load_default_user_data()?.map(|data| data.project_id);
    let response = http
        .post(format!("{PROD_BASE_URL}:fetchAvailableModels"))
        .bearer_auth(token)
        .header("user-agent", antigravity_user_agent())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&json!({ "project": project }))
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "antigravity model listing failed with {}",
            response.status()
        )));
    }
    let body: AvailableModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid fetchAvailableModels body: {err}")))?;
    Ok(discovered_models(body))
}

fn discovered_models(body: AvailableModelsResponse) -> Vec<DiscoveredModel> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for (wire_id, model) in body.models {
        // Gemini 2.x is legacy-only on Antigravity; Claude/GPT-OSS entries are
        // served by their own providers in Sinew.
        if model.is_internal
            || !wire_id.starts_with("gemini-")
            || wire_id.starts_with("gemini-2")
            || EXCLUDED.iter().any(|word| wire_id.contains(word))
        {
            continue;
        }
        let (id, pinned_wire_id) = canonical_id(&wire_id);
        if !seen.insert(id.clone()) {
            continue;
        }
        models.push(DiscoveredModel {
            id,
            label: None,
            context_window: model.max_tokens,
            max_output_tokens: model.max_output_tokens,
            reasoning_levels: Vec::new(),
            default_reasoning: None,
            supports_images: model.supports_images,
            wire_id: pinned_wire_id,
        });
    }
    models
}

/// Map an Antigravity wire id to the public model id Sinew stores, keeping the
/// wire id only when it is a routing alias rather than a thinking tier.
fn canonical_id(wire_id: &str) -> (String, Option<String>) {
    if let Some(base) = wire_id.strip_suffix("-tiered") {
        return (base.to_string(), Some(wire_id.to_string()));
    }
    for suffix in THINKING_SUFFIXES {
        if let Some(base) = wire_id.strip_suffix(suffix) {
            return (base.to_string(), None);
        }
    }
    (wire_id.to_string(), Some(wire_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_keeps_gemini_chat_models_with_their_wire_ids() {
        let body: AvailableModelsResponse = serde_json::from_str(
            r#"{"models":{
                "chat_20706":{"isInternal":true,"maxTokens":16384},
                "claude-sonnet-4-6":{"maxTokens":250000},
                "gemini-2.5-pro":{"maxTokens":1048576},
                "gemini-3.1-flash-image":{"maxTokens":65536},
                "gemini-3.5-flash-lite":{"maxTokens":1048576,"maxOutputTokens":65535,"supportsImages":true},
                "gemini-3.8-flash-tiered":{"maxTokens":1048576,"maxOutputTokens":65536,"supportsImages":true},
                "gemini-4-argon-high":{"maxTokens":1048576}
            }}"#,
        )
        .unwrap();
        let models = discovered_models(body);
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "gemini-3.5-flash-lite",
                "gemini-3.8-flash",
                "gemini-4-argon"
            ]
        );
        assert_eq!(models[0].wire_id.as_deref(), Some("gemini-3.5-flash-lite"));
        assert_eq!(
            models[1].wire_id.as_deref(),
            Some("gemini-3.8-flash-tiered")
        );
        assert_eq!(models[2].wire_id, None);
        assert_eq!(models[1].max_output_tokens, Some(65_536));
    }
}
