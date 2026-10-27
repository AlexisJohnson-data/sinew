//! Live model listing for the signed-in Kimi Code account
//! (OpenAI-compatible `GET /models` on the coding endpoint).

use std::time::Duration;

use serde::Deserialize;
use sinew_core::{AppError, DiscoveredModel, Result};

use crate::auth::{common_headers, Credential};

const MODELS_URL: &str = "https://api.kimi.com/coding/v1/models";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    id: String,
    #[serde(alias = "name")]
    display_name: Option<String>,
    #[serde(alias = "context_window")]
    context_length: Option<u32>,
    supports_image_in: Option<bool>,
}

/// Models the stored Kimi credential can use. Empty when Kimi is not connected.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    let Some(credential) = Credential::load_default()? else {
        return Ok(Vec::new());
    };
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|err| AppError::Network(err.to_string()))?;
    let token = credential.bearer(&http).await?;
    let mut request = http
        .get(MODELS_URL)
        .bearer_auth(token)
        .header("accept", "application/json");
    for (key, value) in common_headers()? {
        request = request.header(key, value);
    }
    let response = request
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Provider(format!(
            "kimi model listing failed with {}",
            response.status()
        )));
    }
    let body: ModelsResponse = response
        .json()
        .await
        .map_err(|err| AppError::Decode(format!("invalid kimi models body: {err}")))?;
    Ok(discovered_models(body))
}

fn discovered_models(body: ModelsResponse) -> Vec<DiscoveredModel> {
    body.data
        .into_iter()
        .filter(|model| !model.id.contains("embedding"))
        .map(|model| DiscoveredModel {
            label: model.display_name,
            context_window: model.context_length,
            supports_images: model.supports_image_in,
            id: model.id,
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_maps_openai_style_models() {
        let body: ModelsResponse = serde_json::from_str(
            r#"{"object":"list","data":[
                {"id":"kimi-k3.1-pro","object":"model","context_length":1000000,"supports_image_in":true},
                {"id":"kimi-for-coding","display_name":"Kimi For Coding"}
            ]}"#,
        )
        .unwrap();
        let models = discovered_models(body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].context_window, Some(1_000_000));
        assert_eq!(models[0].supports_images, Some(true));
        assert_eq!(models[1].label.as_deref(), Some("Kimi For Coding"));
    }
}
