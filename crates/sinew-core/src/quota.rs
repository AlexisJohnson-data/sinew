//! Subscription quota windows reported by providers.
//!
//! Claude (OAuth) and ChatGPT (Codex) report how much of their rolling usage
//! windows a plan has consumed, in response headers and, for Codex, in a
//! stream event. Clients feed what they see here; Settings → Usage reads the
//! latest snapshot per provider. Everything is best effort: a provider that
//! reports nothing simply has no gauge.

use std::{
    collections::BTreeMap,
    sync::{OnceLock, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// `5h`, `7d`, `7d_opus`, `primary`, `secondary`…
    pub label: String,
    /// 0–100.
    pub used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub provider: String,
    pub windows: Vec<QuotaWindow>,
    /// Provider-reported state, e.g. `allowed` / `rejected`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Plan name when the provider says (`go`, `plus`, `pro`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub updated_at_ms: i64,
}

fn registry() -> &'static RwLock<BTreeMap<String, QuotaSnapshot>> {
    static REGISTRY: OnceLock<RwLock<BTreeMap<String, QuotaSnapshot>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

pub fn record(provider: &str, windows: Vec<QuotaWindow>, status: Option<String>) {
    record_with_plan(provider, windows, status, None);
}

/// Like [`record`]; a missing `plan` keeps the one already known.
pub fn record_with_plan(
    provider: &str,
    windows: Vec<QuotaWindow>,
    status: Option<String>,
    plan: Option<String>,
) {
    if windows.is_empty() && status.is_none() {
        return;
    }
    if let Ok(mut registry) = registry().write() {
        let plan = plan.or_else(|| {
            registry
                .get(provider)
                .and_then(|snapshot| snapshot.plan.clone())
        });
        registry.insert(
            provider.to_string(),
            QuotaSnapshot {
                provider: provider.to_string(),
                windows,
                status,
                plan,
                updated_at_ms: now_ms(),
            },
        );
    }
}

pub fn snapshot() -> Vec<QuotaSnapshot> {
    registry()
        .read()
        .map(|registry| registry.values().cloned().collect())
        .unwrap_or_default()
}

/// Utilization as a percentage. Claude reports a 0–1 fraction; some headers
/// carry a percentage already.
fn percent(raw: &str) -> Option<f64> {
    let value: f64 = raw.trim().parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(if value <= 1.0 { value * 100.0 } else { value.min(100.0) })
}

/// A reset given either as unix seconds/milliseconds or an RFC 3339 date.
fn reset_ms(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(number) = raw.parse::<f64>() {
        let number = number as i64;
        return Some(if number > 10_000_000_000 { number } else { number * 1000 });
    }
    None
}

/// `anthropic-ratelimit-unified-<window>-utilization` / `-reset` and
/// `anthropic-ratelimit-unified-status`.
pub fn record_anthropic_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) {
    const PREFIX: &str = "anthropic-ratelimit-unified-";
    let mut windows: BTreeMap<String, QuotaWindow> = BTreeMap::new();
    let mut resets: BTreeMap<String, i64> = BTreeMap::new();
    let mut status = None;
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        let Some(rest) = name.strip_prefix(PREFIX) else {
            continue;
        };
        if rest == "status" {
            status = Some(value.trim().to_string());
        } else if let Some(label) = rest.strip_suffix("-utilization") {
            if let Some(used_percent) = percent(value) {
                windows.insert(
                    label.to_string(),
                    QuotaWindow {
                        label: label.to_string(),
                        used_percent,
                        resets_at_ms: None,
                        window_minutes: None,
                    },
                );
            }
        } else if let Some(label) = rest.strip_suffix("-reset") {
            if let Some(at) = reset_ms(value) {
                resets.insert(label.to_string(), at);
            }
        }
    }
    for (label, at) in resets {
        if let Some(window) = windows.get_mut(&label) {
            window.resets_at_ms = Some(at);
        }
    }
    record("anthropic", windows.into_values().collect(), status);
}

/// `x-codex-<primary|secondary>-used-percent`, `-window-minutes`,
/// `-reset-after-seconds` / `-reset-at`.
pub fn record_codex_headers<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) {
    const PREFIX: &str = "x-codex-";
    let mut windows: BTreeMap<String, QuotaWindow> = BTreeMap::new();
    let mut extras: Vec<(String, String, String)> = Vec::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        let Some(rest) = name.strip_prefix(PREFIX) else {
            continue;
        };
        let Some((label, field)) = rest.split_once('-') else {
            continue;
        };
        if field == "used-percent" {
            if let Ok(used) = value.trim().parse::<f64>() {
                windows.insert(
                    label.to_string(),
                    QuotaWindow {
                        label: label.to_string(),
                        used_percent: used.clamp(0.0, 100.0),
                        resets_at_ms: None,
                        window_minutes: None,
                    },
                );
            }
        } else {
            extras.push((label.to_string(), field.to_string(), value.trim().to_string()));
        }
    }
    for (label, field, value) in extras {
        let Some(window) = windows.get_mut(&label) else {
            continue;
        };
        match field.as_str() {
            "window-minutes" => window.window_minutes = value.parse().ok(),
            "reset-after-seconds" => {
                window.resets_at_ms = value
                    .parse::<f64>()
                    .ok()
                    .map(|seconds| now_ms() + (seconds * 1000.0) as i64)
            }
            "reset-at" => window.resets_at_ms = reset_ms(&value),
            _ => {}
        }
    }
    record("openai", windows.into_values().collect(), None);
}

/// Codex may also stream `{"type": "codex.rate_limits", "rate_limits":
/// {"primary": {...}, "secondary": {...}}}` (or the windows at the top level).
pub fn observe_codex_event(event: &Value) {
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return;
    };
    if !kind.contains("rate_limits") {
        return;
    }
    let body = event.get("rate_limits").unwrap_or(event);
    let mut windows = Vec::new();
    for label in ["primary", "secondary"] {
        let Some(window) = body.get(label) else {
            continue;
        };
        let Some(used_percent) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let resets_at_ms = window
            .get("resets_at")
            .and_then(Value::as_f64)
            .map(|at| reset_ms(&at.to_string()).unwrap_or(0))
            .filter(|at| *at > 0)
            .or_else(|| {
                window
                    .get("reset_after_seconds")
                    .or_else(|| window.get("resets_in_seconds"))
                    .and_then(Value::as_f64)
                    .map(|seconds| now_ms() + (seconds * 1000.0) as i64)
            });
        windows.push(QuotaWindow {
            label: label.to_string(),
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at_ms,
            window_minutes: window
                .get("window_minutes")
                .and_then(Value::as_u64)
                .map(|minutes| minutes as u32),
        });
    }
    record("openai", windows, None);
}

/// Unix milliseconds of an RFC 3339 instant such as
/// `2026-10-28T18:00:00.572905+00:00` or `…Z`.
fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let (clock, offset_minutes) = if let Some(clock) = rest.strip_suffix('Z') {
        (clock, 0)
    } else {
        let split = rest.rfind(['+', '-'])?;
        let (clock, offset) = rest.split_at(split);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = offset[1..].split_once(':').unwrap_or((&offset[1..], "0"));
        let minutes = hours.parse::<i64>().ok()? * 60 + minutes.parse::<i64>().ok()?;
        (clock, sign * minutes)
    };
    let mut clock_parts = clock.split(':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let seconds: f64 = clock_parts.next().unwrap_or("0").parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 24 + hour) * 60 + minute - offset_minutes) * 60_000
        + (seconds * 1000.0) as i64;
    Some(ms)
}

/// Body of Claude's plan-usage endpoint. Recent bodies list every window of
/// the plan in `limits` (`session`, `weekly_all`, `weekly_opus`…, with a
/// 0–100 `percent`); older ones only have `five_hour` / `seven_day*` objects
/// with a 0–100 `utilization`. Both carry an RFC 3339 `resets_at`.
pub fn record_anthropic_usage(body: &Value) {
    let (windows, status) = anthropic_usage_windows(body);
    record("anthropic", windows, status);
}

fn anthropic_usage_windows(body: &Value) -> (Vec<QuotaWindow>, Option<String>) {
    let resets = |window: &Value| {
        window
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_ms)
    };
    let mut windows = Vec::new();
    for limit in body.get("limits").and_then(Value::as_array).into_iter().flatten() {
        let Some(percent) = limit.get("percent").and_then(Value::as_f64) else {
            continue;
        };
        let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("");
        let group = limit.get("group").and_then(Value::as_str).unwrap_or(kind);
        let (label, window_minutes) = match group {
            "session" => ("5h".to_string(), Some(300)),
            "weekly" => {
                let scope = limit
                    .get("scope")
                    .and_then(Value::as_str)
                    .or_else(|| kind.strip_prefix("weekly_"))
                    .filter(|scope| !scope.is_empty() && *scope != "all");
                (
                    scope.map_or_else(|| "7d".to_string(), |scope| format!("7d_{scope}")),
                    Some(10_080),
                )
            }
            _ if kind.is_empty() => continue,
            _ => (kind.to_string(), None),
        };
        windows.push(QuotaWindow {
            label,
            used_percent: percent.clamp(0.0, 100.0),
            resets_at_ms: resets(limit),
            window_minutes,
        });
    }

    let mut status = None;
    let Some(fields) = body.as_object() else {
        return (windows, status);
    };
    let from_limits = !windows.is_empty();
    for (key, window) in fields {
        let (label, window_minutes) = if key == "five_hour" {
            ("5h".to_string(), 300)
        } else if let Some(rest) = key.strip_prefix("seven_day") {
            (format!("7d{rest}"), 10_080)
        } else {
            continue;
        };
        if let Some(reason) = window.get("locked_reason").and_then(Value::as_str) {
            status = Some(reason.to_string());
        }
        if from_limits {
            continue;
        }
        let Some(used_percent) = window.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(QuotaWindow {
            label,
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at_ms: resets(window),
            window_minutes: Some(window_minutes),
        });
    }
    // Session first, then the weekly windows.
    windows.sort_by_key(|window| (window.window_minutes.unwrap_or(u32::MAX), window.label.clone()));
    (windows, status)
}

/// Body of ChatGPT's usage endpoint: `rate_limit.primary_window` /
/// `secondary_window` (5 hours and a week on Plus/Pro/Team, a single
/// 30-day window on Go), plus optional per-feature limits
/// (`code_review_rate_limit`, `additional_rate_limits`). Windows carry
/// `used_percent`, `limit_window_seconds` and `reset_at` (unix seconds).
pub fn record_chatgpt_usage(body: &Value) {
    if let Some((windows, status, plan)) = chatgpt_usage_windows(body) {
        record_with_plan("openai", windows, status, plan);
    }
}

fn chatgpt_rate_limit_windows(rate_limit: &Value, scope: Option<&str>) -> Vec<QuotaWindow> {
    let mut windows = Vec::new();
    for (key, base) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        let Some(window) = rate_limit.get(key).filter(|window| window.is_object()) else {
            continue;
        };
        let Some(used_percent) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let resets_at_ms = window
            .get("reset_at")
            .and_then(Value::as_f64)
            .map(|at| at as i64 * 1000)
            .or_else(|| {
                window
                    .get("reset_after_seconds")
                    .and_then(Value::as_f64)
                    .map(|seconds| now_ms() + (seconds * 1000.0) as i64)
            });
        windows.push(QuotaWindow {
            label: match scope {
                Some(scope) => format!("{base}:{scope}"),
                None => base.to_string(),
            },
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at_ms,
            window_minutes: window
                .get("limit_window_seconds")
                .and_then(Value::as_u64)
                .map(|seconds| (seconds / 60) as u32),
        });
    }
    windows
}

fn chatgpt_usage_windows(
    body: &Value,
) -> Option<(Vec<QuotaWindow>, Option<String>, Option<String>)> {
    let rate_limit = body.get("rate_limit")?;
    let mut windows = chatgpt_rate_limit_windows(rate_limit, None);
    if let Some(review) = body.get("code_review_rate_limit").filter(|v| v.is_object()) {
        windows.extend(chatgpt_rate_limit_windows(review, Some("code review")));
    }
    for extra in body
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = ["limit_name", "metered_feature", "name"]
            .iter()
            .find_map(|key| extra.get(key).and_then(Value::as_str))
            .unwrap_or("extra");
        let limits = extra.get("rate_limit").unwrap_or(extra);
        windows.extend(chatgpt_rate_limit_windows(limits, Some(name)));
    }
    let status = rate_limit
        .get("limit_reached")
        .and_then(Value::as_bool)
        .filter(|reached| *reached)
        .map(|_| "limit reached".to_string());
    let plan = body
        .get("plan_type")
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);
    Some((windows, status, plan))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_plans_become_windows() {
        // Pro, as the endpoint answers today: `limits` plus legacy fields.
        let (pro, status) = anthropic_usage_windows(&serde_json::json!({
            "five_hour": { "utilization": 10, "resets_at": "2026-10-28T18:00:00.572905+00:00", "locked_reason": null },
            "seven_day": { "utilization": 22, "resets_at": "2026-10-28T18:00:00Z" },
            "seven_day_opus": null,
            "seven_day_breakdown": { "rows": [] },
            "limits": [
                { "kind": "session", "group": "session", "percent": 10, "resets_at": "2026-10-28T18:00:00.572905+00:00", "scope": null },
                { "kind": "weekly_all", "group": "weekly", "percent": 22, "resets_at": "2026-10-28T18:00:00Z", "scope": null }
            ]
        }));
        assert_eq!(status, None);
        let labels: Vec<_> = pro.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d"]);
        assert_eq!(pro[0].used_percent, 10.0);
        assert_eq!(pro[0].resets_at_ms, Some(1_793_210_400_572));
        assert_eq!(pro[1].window_minutes, Some(10_080));

        // Max: an extra model-specific weekly window.
        let (max, _) = anthropic_usage_windows(&serde_json::json!({
            "limits": [
                { "kind": "session", "group": "session", "percent": 40 },
                { "kind": "weekly_all", "group": "weekly", "percent": 30 },
                { "kind": "weekly_opus", "group": "weekly", "percent": 75, "scope": "opus" }
            ]
        }));
        let labels: Vec<_> = max.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d", "7d_opus"]);

        // Older bodies without `limits`, and a locked window.
        let (legacy, status) = anthropic_usage_windows(&serde_json::json!({
            "five_hour": { "utilization": 100, "locked_reason": "rate_limited" },
            "seven_day": { "utilization": 50 },
            "seven_day_sonnet": { "utilization": 12 }
        }));
        let labels: Vec<_> = legacy.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d", "7d_sonnet"]);
        assert_eq!(status.as_deref(), Some("rate_limited"));

        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("1970-01-01T01:00:00+01:00"), Some(0));
    }

    #[test]
    fn chatgpt_plans_become_windows() {
        // Go: one 30-day window.
        let (go, status, plan) = chatgpt_usage_windows(&serde_json::json!({
            "plan_type": "go",
            "rate_limit": {
                "limit_reached": false,
                "primary_window": { "used_percent": 5, "limit_window_seconds": 2592000, "reset_at": 1795590020 },
                "secondary_window": null
            },
            "code_review_rate_limit": null,
            "additional_rate_limits": null
        }))
        .unwrap();
        assert_eq!((status, plan.as_deref()), (None, Some("go")));
        assert_eq!(go.len(), 1);
        assert_eq!(go[0].window_minutes, Some(43_200));
        assert_eq!(go[0].resets_at_ms, Some(1_795_590_020_000));

        // Plus / Pro / Team: 5 hours + a week, maybe per-feature limits.
        let (plus, status, plan) = chatgpt_usage_windows(&serde_json::json!({
            "plan_type": "plus",
            "rate_limit": {
                "limit_reached": true,
                "primary_window": { "used_percent": 100, "limit_window_seconds": 18000, "reset_after_seconds": 600 },
                "secondary_window": { "used_percent": 41, "limit_window_seconds": 604800, "reset_at": 1795590020 }
            },
            "code_review_rate_limit": {
                "primary_window": { "used_percent": 3, "limit_window_seconds": 604800 }
            },
            "additional_rate_limits": [
                { "limit_name": "GPT-6.1 Sol", "rate_limit": {
                    "primary_window": { "used_percent": 60, "limit_window_seconds": 18000 }
                }}
            ]
        }))
        .unwrap();
        assert_eq!(status.as_deref(), Some("limit reached"));
        assert_eq!(plan.as_deref(), Some("plus"));
        let labels: Vec<_> = plus.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            ["primary", "secondary", "primary:code review", "primary:GPT-6.1 Sol"]
        );
        assert_eq!(plus[0].window_minutes, Some(300));
        assert_eq!(plus[1].window_minutes, Some(10_080));
        assert!(plus[0].resets_at_ms.is_some());
    }

    #[test]
    fn anthropic_headers_become_windows() {
        record_anthropic_headers([
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.62"),
            ("anthropic-ratelimit-unified-5h-reset", "1800000000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.1"),
            ("content-type", "text/event-stream"),
        ]);
        let snapshot = snapshot()
            .into_iter()
            .find(|snapshot| snapshot.provider == "anthropic")
            .unwrap();
        assert_eq!(snapshot.status.as_deref(), Some("allowed"));
        let five = snapshot.windows.iter().find(|w| w.label == "5h").unwrap();
        assert!((five.used_percent - 62.0).abs() < 1e-9);
        assert_eq!(five.resets_at_ms, Some(1_800_000_000_000));
        assert!(snapshot.windows.iter().any(|w| w.label == "7d"));
    }

    #[test]
    fn codex_headers_and_events_become_windows() {
        record_codex_headers([
            ("x-codex-primary-used-percent", "40"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-secondary-used-percent", "12.5"),
        ]);
        let openai = snapshot()
            .into_iter()
            .find(|snapshot| snapshot.provider == "openai")
            .unwrap();
        let primary = openai.windows.iter().find(|w| w.label == "primary").unwrap();
        assert_eq!(primary.window_minutes, Some(300));
        assert_eq!(openai.windows.len(), 2);

        observe_codex_event(&serde_json::json!({
            "type": "codex.rate_limits",
            "rate_limits": { "primary": { "used_percent": 55.0, "window_minutes": 300, "reset_after_seconds": 60 } }
        }));
        let openai = snapshot()
            .into_iter()
            .find(|snapshot| snapshot.provider == "openai")
            .unwrap();
        assert!((openai.windows[0].used_percent - 55.0).abs() < 1e-9);
        assert!(openai.windows[0].resets_at_ms.is_some());
    }
}
