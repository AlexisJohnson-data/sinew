//! Cost and usage tracking for the desktop app: records each model response
//! (see `sinew_app::usage`), raises the daily-budget alert, and serves
//! Settings → Usage.

use std::{
    path::PathBuf,
    sync::{Mutex as StdMutex, OnceLock},
    time::Duration,
};

use chrono::{Local, TimeZone};
use serde::{Deserialize, Serialize};
use sinew_app::{
    usage::{billing_for, Billing, UsageRecord, UsageSettings, UsageSummary, UsageTotals},
    AgentEvent, AppStore,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{error_to_string, DesktopState};

const BUDGET_EVENT_NAME: &str = "usage-budget-exceeded";
const PRICE_REFRESH_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// Load cached prices now and refresh them in the background, then every
/// few hours (the refresh itself only fetches when they are a day old).
pub(super) fn start_price_refresh(store: AppStore) {
    sinew_app::usage::load_price_book(&store);
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(err) = sinew_app::usage::refresh_price_book(&store, false).await {
                tracing::warn!(%err, "unable to refresh model prices");
            }
            tokio::time::sleep(PRICE_REFRESH_EVERY).await;
        }
    });
}

fn local_midnight_ms() -> i64 {
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .map(|midnight| midnight.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis() - 24 * 60 * 60 * 1000)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BudgetExceeded {
    spent_usd: f64,
    budget_usd: f64,
}

/// Record a `UsageRecorded` event (possibly nested in sub-agent events).
pub(super) fn record_usage_event(
    app: &AppHandle,
    workspace_id: &str,
    conversation_id: &str,
    event: &AgentEvent,
) {
    let (agent, provider, model, usage) = match event {
        AgentEvent::UsageRecorded {
            provider,
            model,
            usage,
        } => (None, provider, model, usage),
        AgentEvent::SubAgentEvent {
            agent_name, event, ..
        } => match event.as_ref() {
            AgentEvent::UsageRecorded {
                provider,
                model,
                usage,
            } => (Some(agent_name.clone()), provider, model, usage),
            // Deeper nesting (a sub-agent's own sub-agents) keeps the
            // innermost name.
            inner @ AgentEvent::SubAgentEvent { .. } => {
                return record_usage_event(app, workspace_id, conversation_id, inner)
            }
            _ => return,
        },
        _ => return,
    };
    let Some(state) = app.try_state::<DesktopState>() else {
        return;
    };
    let store = &state.store;
    let settings = store.load_usage_settings().unwrap_or_default();
    let billing = billing_for(provider, false);
    let record = UsageRecord::new(
        workspace_id,
        conversation_id,
        agent,
        provider,
        model,
        usage,
        billing,
        &settings,
    );
    if let Err(err) = store.record_usage(&record) {
        tracing::warn!(%err, "unable to record usage");
        return;
    }
    if billing == Billing::Metered {
        check_budget(app, store, &settings);
    }
}

/// Notify once per day when metered spend passes the daily budget.
fn check_budget(app: &AppHandle, store: &AppStore, settings: &UsageSettings) {
    static LAST_ALERT_DAY: OnceLock<StdMutex<i64>> = OnceLock::new();
    let Some(budget) = settings.daily_budget_usd.filter(|budget| *budget > 0.0) else {
        return;
    };
    let midnight = local_midnight_ms();
    let Ok(spent) = store.metered_spend_since(midnight) else {
        return;
    };
    if spent < budget {
        return;
    }
    let last = LAST_ALERT_DAY.get_or_init(|| StdMutex::new(0));
    let Ok(mut last) = last.lock() else {
        return;
    };
    if *last == midnight {
        return;
    }
    *last = midnight;
    let _ = app.emit(
        BUDGET_EVENT_NAME,
        BudgetExceeded {
            spent_usd: spent,
            budget_usd: budget,
        },
    );
}

// ---------------------------------------------------------------------------
// Commands.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageSummaryInput {
    /// Days back from now; 1 means "today" (since local midnight).
    days: u32,
    utc_offset_minutes: i32,
}

fn since_ms(days: u32) -> i64 {
    local_midnight_ms() - i64::from(days.saturating_sub(1)) * 24 * 60 * 60 * 1000
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UsageOverview {
    summary: UsageSummary,
    quotas: Vec<sinew_core::quota::QuotaSnapshot>,
    settings: UsageSettings,
    spent_today_usd: f64,
    /// How old the cached prices are; `None` when none could be loaded yet.
    prices_age_ms: Option<i64>,
}

#[tauri::command]
pub(super) async fn usage_overview(
    state: State<'_, DesktopState>,
    input: UsageSummaryInput,
) -> std::result::Result<UsageOverview, String> {
    let store = &state.store;
    Ok(UsageOverview {
        summary: store
            .usage_summary(since_ms(input.days.max(1)), input.utc_offset_minutes)
            .map_err(error_to_string)?,
        quotas: sinew_core::quota::snapshot(),
        settings: store.load_usage_settings().map_err(error_to_string)?,
        spent_today_usd: store
            .metered_spend_since(local_midnight_ms())
            .map_err(error_to_string)?,
        prices_age_ms: sinew_app::usage::price_book_age_ms(),
    })
}

/// Ask Claude and ChatGPT for their plan windows (not more than once a
/// minute), then return every known snapshot. Response headers keep them
/// fresh in between.
#[tauri::command]
pub(super) async fn refresh_plan_limits() -> Vec<sinew_core::quota::QuotaSnapshot> {
    // Held for the whole fetch: a call arriving meanwhile (two windows, or
    // React running the effect twice) waits for it instead of returning the
    // still-empty snapshot.
    static LAST: OnceLock<tokio::sync::Mutex<Option<std::time::Instant>>> = OnceLock::new();
    let mut last = LAST
        .get_or_init(|| tokio::sync::Mutex::new(None))
        .lock()
        .await;
    if last.map_or(true, |at| at.elapsed() >= Duration::from_secs(60)) {
        let (claude, chatgpt, opencode_go) = tokio::join!(
            sinew_anthropic::refresh_plan_limits(),
            sinew_openai::refresh_plan_limits(),
            sinew_opencode_go::refresh_plan_limits()
        );
        if let Err(err) = claude {
            tracing::warn!(%err, "unable to fetch Claude plan limits");
        }
        if let Err(err) = chatgpt {
            tracing::warn!(%err, "unable to fetch ChatGPT plan limits");
        }
        if let Err(err) = opencode_go {
            tracing::warn!(%err, "unable to fetch OpenCode Go plan limits");
        }
        *last = Some(std::time::Instant::now());
    }
    sinew_core::quota::snapshot()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConversationUsageInput {
    conversation_id: String,
}

#[tauri::command]
pub(super) async fn conversation_usage(
    state: State<'_, DesktopState>,
    input: ConversationUsageInput,
) -> std::result::Result<UsageTotals, String> {
    state
        .store
        .conversation_usage(&input.conversation_id)
        .map_err(error_to_string)
}

#[derive(Debug, Deserialize)]
pub(super) struct SaveUsageSettingsInput {
    settings: UsageSettings,
}

#[tauri::command]
pub(super) async fn save_usage_settings(
    state: State<'_, DesktopState>,
    input: SaveUsageSettingsInput,
) -> std::result::Result<UsageSettings, String> {
    let mut settings = input.settings;
    settings.daily_budget_usd = settings
        .daily_budget_usd
        .filter(|budget| budget.is_finite() && *budget > 0.0);
    settings.price_overrides.retain(|key, price| {
        !key.trim().is_empty() && price.input_per_m >= 0.0 && price.output_per_m >= 0.0
    });
    state
        .store
        .save_usage_settings(&settings)
        .map_err(error_to_string)?;
    Ok(settings)
}

#[tauri::command]
pub(super) async fn refresh_model_prices(
    state: State<'_, DesktopState>,
) -> std::result::Result<Option<i64>, String> {
    sinew_app::usage::refresh_price_book(&state.store, true)
        .await
        .map_err(error_to_string)?;
    Ok(sinew_app::usage::price_book_age_ms())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportUsageInput {
    path: String,
    days: u32,
}

#[tauri::command]
pub(super) async fn export_usage_csv(
    state: State<'_, DesktopState>,
    input: ExportUsageInput,
) -> std::result::Result<(), String> {
    let csv = state
        .store
        .usage_csv(since_ms(input.days.max(1)))
        .map_err(error_to_string)?;
    std::fs::write(PathBuf::from(input.path), csv).map_err(error_to_string)
}
