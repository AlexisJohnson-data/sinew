//! Live model listing for the stored DeepSeek API key (`GET /models`).
//!
//! DeepSeek's listing carries the real window, output cap, modalities and
//! effort levels, so renames like `deepseek-v4-flash` → `deepseek-flash` are
//! picked up without a code change.

use std::time::Duration;

use serde::Deserialize;
use sinew_core::{AppError, DiscoveredModel, Result};

use crate::auth::load_default_api_key;

const MODELS_URL: &str = "https://api.deepseek.com/v1/models";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    name: Option<String>,
    context_window: Option<u32>,
    max_output_tokens: Option<u32>,
    effort: Option<ApiEffort>,
}

#[derive(Deserialize)]
struct ApiEffort {
    #[serde(default)]
    supported_levels: Vec<String>,
    default_level: Option<String>,
}

/// Models the stored DeepSeek key can use. Empty when DeepSeek is not
/// connected.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    let Some(api_key) = load_default_api_key()? else {
        return Ok(Vec::new());
    };
    let response = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?
        .get(MODELS_URL)
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "deepseek model listing failed with {}",
            response.status()
        )));
    }
    let body: ModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid deepseek models body: {err}")))?;
    Ok(discovered_models(body))
}

fn discovered_models(body: ModelsResponse) -> Vec<DiscoveredModel> {
    body.data
        .into_iter()
        .map(|model| {
            let (reasoning_levels, default_reasoning) = model
                .effort
                .map(|effort| (effort.supported_levels, effort.default_level))
                .unwrap_or_default();
            DiscoveredModel {
                label: model.name,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                reasoning_levels,
                default_reasoning,
                // Not propagated yet: `/chat/completions` rejected `image_url`
                // parts on V4 and the V4.1 vision path is unverified, so the
                // static text-only setting stays authoritative.
                supports_images: None,
                wire_id: None,
                id: model.id,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_maps_limits_modalities_and_effort() {
        // Trimmed copy of the live response (2026-10-27).
        let body: ModelsResponse = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"deepseek-flash","name":"DeepSeek-V4.1-Flash","context_window":1048576,"max_output_tokens":393216,
                 "input_modalities":["text","image"],"effort":{"supported_levels":["low","high","max"],"default_level":"high"}},
                {"id":"deepseek-v4-pro","name":"DeepSeek-V4-Pro","context_window":1048576,"input_modalities":["text"]}
            ]}"#,
        )
        .unwrap();
        let models = discovered_models(body);
        assert_eq!(models[0].id, "deepseek-flash");
        assert_eq!(models[0].reasoning_levels, vec!["low", "high", "max"]);
        assert_eq!(models[0].supports_images, None);
        assert_eq!(models[1].context_window, Some(1_048_576));
    }
}
