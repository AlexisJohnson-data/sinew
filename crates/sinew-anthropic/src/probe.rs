//! Account details read from a short-lived Claude Code process: the models
//! the signed-in account can use (`initialize`) and the plan's usage windows
//! (`get_usage`). No prompt is sent, so no model request is made.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sinew_core::{DiscoveredModel, Result};
use tokio::sync::Mutex;

use crate::cli;
use crate::process::ClaudeProcess;

/// Probes are a process start each, so results are reused for a while.
const PROBE_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
struct Probe {
    models: Vec<DiscoveredModel>,
    usage_recorded: bool,
}

fn cache() -> &'static Mutex<Option<(Instant, Probe)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Probe)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

async fn probe() -> Result<Probe> {
    let mut cached = cache().lock().await;
    if let Some((at, probe)) = cached.as_ref() {
        if at.elapsed() < PROBE_TTL {
            return Ok(probe.clone());
        }
    }
    if !cli::status().await.logged_in {
        *cached = None;
        return Ok(Probe::default());
    }
    let cwd = directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    let process = ClaudeProcess::spawn(
        &[
            "--strict-mcp-config".into(),
            "--no-session-persistence".into(),
        ],
        &cwd,
    )?;
    let sender = process.sender();
    let init = process.initialize(json!({})).await;
    let usage = sender
        .request(
            json!({ "subtype": "get_usage", "skip_behaviors": true }),
            REQUEST_TIMEOUT,
        )
        .await;
    process.shutdown(Duration::from_secs(2)).await;

    let probe = Probe {
        models: init.as_ref().map(discovered_models).unwrap_or_default(),
        usage_recorded: usage.as_ref().is_ok_and(record_usage),
    };
    *cached = Some((Instant::now(), probe.clone()));
    init?;
    Ok(probe)
}

/// Models the signed-in Claude Code account can use. Empty when Claude Code
/// is missing or signed out.
pub async fn list_models() -> Result<Vec<DiscoveredModel>> {
    Ok(probe().await?.models)
}

/// Fetches the Claude plan's usage windows into `sinew_core::quota`.
/// `Ok(false)` when there is no subscription sign-in.
pub async fn refresh_plan_limits() -> Result<bool> {
    Ok(probe().await?.usage_recorded)
}

/// Records a `get_usage` answer. `false` when the account has no plan limits
/// (API key sign-in).
pub fn record_usage(response: &Value) -> bool {
    let Some(limits) = response
        .get("rate_limits")
        .filter(|limits| limits.is_object())
    else {
        return false;
    };
    sinew_core::quota::record_anthropic_usage(limits);
    true
}

fn discovered_models(init: &Value) -> Vec<DiscoveredModel> {
    let mut models: Vec<DiscoveredModel> = Vec::new();
    for model in init["models"].as_array().into_iter().flatten() {
        let id = model["resolvedModel"]
            .as_str()
            .or_else(|| model["value"].as_str())
            .unwrap_or_default()
            // `claude-opus-5-5[1m]` names the 1M-context variant.
            .split('[')
            .next()
            .unwrap_or_default()
            .to_string();
        if !id.starts_with("claude-") || models.iter().any(|known| known.id == id) {
            continue;
        }
        let reasoning_levels = model["supportedEffortLevels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        models.push(DiscoveredModel {
            label: model["displayName"]
                .as_str()
                .map(|name| name.trim_start_matches("Claude ").to_string()),
            supports_images: Some(true),
            reasoning_levels,
            id,
            ..Default::default()
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_models_from_initialize() {
        let init = json!({
            "models": [
                { "value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5", "supportedEffortLevels": ["low", "high", "max"] },
                { "value": "opus[1m]", "resolvedModel": "claude-opus-5-5[1m]", "displayName": "Opus 5.5 (1M)" },
                { "value": "sonnet", "resolvedModel": "claude-sonnet-5-5", "displayName": "Claude Sonnet 5.5" },
                { "value": "custom", "displayName": "Something else" }
            ]
        });
        let models = discovered_models(&init);
        let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, ["claude-opus-5-5", "claude-sonnet-5-5"]);
        assert_eq!(models[0].reasoning_levels, ["low", "high", "max"]);
        assert_eq!(models[1].label.as_deref(), Some("Sonnet 5.5"));
    }

    #[test]
    fn records_plan_windows_only_with_a_subscription() {
        assert!(!record_usage(&json!({ "rate_limits_available": false, "rate_limits": null })));
        assert!(record_usage(&json!({
            "rate_limits": {
                "five_hour": { "utilization": 12.0, "resets_at": "2026-10-31T18:00:00Z" },
                "seven_day": { "utilization": 40.0, "resets_at": "2026-11-03T08:00:00Z" }
            }
        })));
    }
}
