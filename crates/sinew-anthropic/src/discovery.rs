//! Live model listing for the signed-in Anthropic account (`GET /v1/models`).
//!
//! The Models API reports each model's real input/output limits, so a model
//! released after this build gets correct limits and shows up in the picker
//! without a code change.

use std::time::Duration;

use serde::Deserialize;
use sinew_core::{AppError, DiscoveredModel, Result};

use crate::{auth::Credential, cli_version};

const MODELS_URL: &str = "https://api.anthropic.com/v1/models";
/// Plan usage (5-hour and weekly windows) for a Claude subscription, as the
/// Claude apps and Claude Code's `/usage` show it.
const PLAN_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const API_VERSION: &str = "2023-06-01";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u32>,
    max_tokens: Option<u32>,
}

/// Models the stored Anthropic credential can use. Empty when Anthropic is not
/// connected.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(Vec::new());
    };
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let token = credential.bearer_or_key(&http).await?;
    let mut request = http
        .get(MODELS_URL)
        .query(&[("limit", "100")])
        .header("anthropic-version", API_VERSION)
        .header("user-agent", cli_version::user_agent());
    request = if credential.is_oauth() {
        request
            .header("authorization", format!("Bearer {token}"))
            .header("anthropic-beta", OAUTH_BETA)
            .header("x-app", "cli")
    } else {
        request.header("x-api-key", token)
    };
    let response = request
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "anthropic model listing failed with {}",
            response.status()
        )));
    }
    let body: ModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid anthropic models body: {err}")))?;
    Ok(discovered_models(body))
}

fn discovered_models(body: ModelsResponse) -> Vec<DiscoveredModel> {
    body.data
        .into_iter()
        .filter(|model| model.id.starts_with("claude-"))
        .map(|model| DiscoveredModel {
            label: model
                .display_name
                .map(|name| name.trim_start_matches("Claude ").to_string()),
            context_window: model.max_input_tokens,
            max_output_tokens: model.max_tokens,
            // Every current Claude model takes images.
            supports_images: Some(true),
            id: model.id,
            ..Default::default()
        })
        .collect()
}

/// Fetch the Claude plan's usage windows into `sinew_core::quota`.
/// `Ok(false)` when Anthropic is not connected with a subscription login.
pub async fn refresh_plan_limits() -> Result<bool> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(false);
    };
    if !credential.is_oauth() {
        return Ok(false);
    }
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let token = credential.bearer_or_key(&http).await?;
    let response = http
        .get(PLAN_USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-beta", OAUTH_BETA)
        .header("user-agent", cli_version::user_agent())
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "Claude plan usage request failed with {}",
            response.status()
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid Claude plan usage: {err}")))?;
    sinew_core::quota::record_anthropic_usage(&body);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_api_listing_maps_limits_and_labels() {
        let body: ModelsResponse = serde_json::from_str(
            r#"{"data":[
                {"id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","max_input_tokens":1000000,"max_tokens":128000,"line":"sonnet"},
                {"id":"not-a-claude-model"}
            ],"has_more":false}"#,
        )
        .unwrap();
        let models = discovered_models(body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-sonnet-5-5");
        assert_eq!(models[0].label.as_deref(), Some("Sonnet 5.5"));
        assert_eq!(models[0].context_window, Some(1_000_000));
        assert_eq!(models[0].max_output_tokens, Some(128_000));
    }
}
