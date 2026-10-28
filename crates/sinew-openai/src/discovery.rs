//! Live model listing for the signed-in OpenAI account.
//!
//! ChatGPT sign-in goes through the same Codex `/models` endpoint the Codex
//! CLI uses, which only returns what the account's plan can run (e.g. a Go
//! plan gets GPT-6 Luna but not GPT-6.1 Sol) together with the real context
//! window. API keys fall back to the public `/v1/models` list, filtered to the
//! GPT generations Sinew drives.

use std::time::Duration;

use serde::Deserialize;
use sinew_core::{AppError, DiscoveredModel, Result};

use crate::auth::{BearerToken, Credential};

const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
/// Usage windows of a ChatGPT plan, as Codex's status line shows them.
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const API_MODELS_URL: &str = "https://api.openai.com/v1/models";
/// The Codex endpoint hides models newer than the advertised client version,
/// so we advertise the latest published `@openai/codex` release.
const CODEX_NPM_URL: &str = "https://registry.npmjs.org/@openai%2Fcodex/latest";
const FALLBACK_CODEX_VERSION: &str = "0.159.0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct CodexModelsResponse {
    #[serde(default)]
    models: Vec<CodexModel>,
}

#[derive(Deserialize)]
struct CodexModel {
    slug: String,
    display_name: Option<String>,
    visibility: Option<String>,
    context_window: Option<u32>,
    #[serde(default)]
    supported_reasoning_levels: Vec<CodexReasoningLevel>,
    default_reasoning_level: Option<String>,
    #[serde(default)]
    input_modalities: Vec<String>,
}

#[derive(Deserialize)]
struct CodexReasoningLevel {
    effort: String,
}

#[derive(Deserialize)]
struct ApiModelsResponse {
    #[serde(default)]
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
}

#[derive(Deserialize)]
struct NpmLatest {
    version: String,
}

/// Models the stored OpenAI credential can use. Empty when OpenAI is not
/// connected.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(Vec::new());
    };
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let bearer = credential.bearer(&http).await?;
    if bearer.is_oauth {
        list_codex_models(&http, &bearer).await
    } else {
        list_api_models(&http, &bearer.token).await
    }
}

async fn list_codex_models(
    http: &reqwest::Client,
    bearer: &BearerToken,
) -> Result<Vec<DiscoveredModel>> {
    let version = latest_codex_version(http).await;
    let mut request = http
        .get(CODEX_MODELS_URL)
        .query(&[("client_version", version.as_str())])
        .header("authorization", format!("Bearer {}", bearer.token))
        .header("originator", "codex_cli_rs")
        .header("user-agent", format!("codex_cli_rs/{version}"));
    if let Some(account_id) = &bearer.account_id {
        request = request.header("chatgpt-account-id", account_id);
    }
    let response = request
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "codex model listing failed with {}",
            response.status()
        )));
    }
    let body: CodexModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid codex models body: {err}")))?;
    Ok(codex_models(body))
}

fn codex_models(body: CodexModelsResponse) -> Vec<DiscoveredModel> {
    body.models
        .into_iter()
        // `hide` covers internal helpers (auto-review, reserve capacity).
        .filter(|model| model.visibility.as_deref() == Some("list"))
        .map(|model| DiscoveredModel {
            label: model.display_name,
            context_window: model.context_window,
            max_output_tokens: None,
            reasoning_levels: model
                .supported_reasoning_levels
                .into_iter()
                .map(|level| level.effort)
                .collect(),
            default_reasoning: model.default_reasoning_level,
            supports_images: (!model.input_modalities.is_empty())
                .then(|| model.input_modalities.iter().any(|kind| kind == "image")),
            wire_id: None,
            id: model.slug,
        })
        .collect()
}

async fn list_api_models(http: &reqwest::Client, api_key: &str) -> Result<Vec<DiscoveredModel>> {
    let response = http
        .get(API_MODELS_URL)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "openai model listing failed with {}",
            response.status()
        )));
    }
    let body: ApiModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid openai models body: {err}")))?;
    Ok(body
        .data
        .into_iter()
        .filter(|model| is_chat_model_id(&model.id))
        .map(|model| DiscoveredModel {
            id: model.id,
            ..Default::default()
        })
        .collect())
}

/// `/v1/models` lists every artefact (audio, realtime, dated snapshots, …);
/// keep the undated GPT-5.6+/GPT-6 chat models Sinew can drive.
fn is_chat_model_id(id: &str) -> bool {
    const EXCLUDED: &[&str] = &[
        "audio",
        "realtime",
        "image",
        "transcribe",
        "tts",
        "search",
        "embedding",
    ];
    let generation = id.starts_with("gpt-6") || id.starts_with("gpt-5.6");
    // Snapshots carry a `-YYYY-MM-DD` suffix.
    let dated = id
        .split('-')
        .any(|part| part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()));
    generation && !dated && !EXCLUDED.iter().any(|word| id.contains(word))
}

async fn latest_codex_version(http: &reqwest::Client) -> String {
    let fetched = async {
        let response = http.get(CODEX_NPM_URL).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let latest: NpmLatest = response.json().await.ok()?;
        let version = latest.version.trim().to_string();
        version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            .then_some(version)
    };
    fetched
        .await
        .unwrap_or_else(|| FALLBACK_CODEX_VERSION.to_string())
}

/// Fetch the ChatGPT plan's usage windows into `sinew_core::quota`.
/// `Ok(false)` when OpenAI is not connected with a ChatGPT login.
pub async fn refresh_plan_limits() -> Result<bool> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(false);
    };
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let bearer = credential.bearer(&http).await?;
    if !bearer.is_oauth {
        return Ok(false);
    }
    let version = latest_codex_version(&http).await;
    let mut request = http
        .get(CODEX_USAGE_URL)
        .header("authorization", format!("Bearer {}", bearer.token))
        .header("originator", "codex_cli_rs")
        .header("user-agent", format!("codex_cli_rs/{version}"));
    if let Some(account_id) = &bearer.account_id {
        request = request.header("chatgpt-account-id", account_id);
    }
    let response = request
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "ChatGPT plan usage request failed with {}",
            response.status()
        )));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid ChatGPT plan usage: {err}")))?;
    sinew_core::quota::record_chatgpt_usage(&body);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_listing_keeps_visible_models_with_their_limits() {
        let body: CodexModelsResponse = serde_json::from_str(
            r#"{"models":[
                {"slug":"gpt-reserve","display_name":"GPT-Reserve","visibility":"hide","context_window":272000},
                {"slug":"gpt-6-luna","display_name":"GPT-6-Luna","visibility":"list","context_window":272000,
                 "supported_reasoning_levels":[{"effort":"low"},{"effort":"max"}],
                 "default_reasoning_level":"medium","input_modalities":["text","image"]}
            ]}"#,
        )
        .unwrap();
        let models = codex_models(body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-6-luna");
        assert_eq!(models[0].context_window, Some(272_000));
        assert_eq!(models[0].reasoning_levels, vec!["low", "max"]);
        assert_eq!(models[0].supports_images, Some(true));
    }

    #[test]
    fn api_listing_keeps_current_chat_models_only() {
        assert!(is_chat_model_id("gpt-6.1-sol"));
        assert!(is_chat_model_id("gpt-6-luna"));
        assert!(is_chat_model_id("gpt-5.6-terra"));
        assert!(!is_chat_model_id("gpt-6-luna-2026-09-22"));
        assert!(!is_chat_model_id("gpt-6-realtime"));
        assert!(!is_chat_model_id("gpt-5.4"));
    }
}
