//! Token usage and cost tracking.
//!
//! Every model response is recorded in the `usage_events` table with its
//! token counts and an estimated cost. Prices come from OpenRouter's public
//! catalogue (refreshed daily, cached in the store) and can be overridden per
//! model. Subscriptions (Claude, ChatGPT, Gemini, Kimi, OpenCode Go) are not
//! billed per token, so their cost is an *API-equivalent* figure, kept apart
//! from the real spend on metered providers (DeepSeek, OpenRouter, API keys).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{OnceLock, RwLock},
    time::Duration,
};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sinew_core::Usage;

use crate::store::AppStore;

const PRICE_BOOK_KEY: &str = "usage_price_book";
const USAGE_SETTINGS_KEY: &str = "usage_settings";
const PRICE_BOOK_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// Billing.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    /// Covered by a plan; the cost is what the same tokens would cost on the API.
    Subscription,
    /// Paid per token: real money.
    Metered,
}

impl Billing {
    fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Metered => "metered",
        }
    }

    fn parse(value: &str) -> Self {
        if value == "metered" {
            Self::Metered
        } else {
            Self::Subscription
        }
    }
}

/// How a provider is paid for when nothing more specific is known.
/// `api_key` is true when the provider is used with an API key rather than
/// a subscription login.
pub fn billing_for(provider: &str, api_key: bool) -> Billing {
    match provider {
        "deepseek" | "openrouter" => Billing::Metered,
        "openai" | "anthropic" if api_key => Billing::Metered,
        _ => Billing::Subscription,
    }
}

// ---------------------------------------------------------------------------
// Prices.

/// USD per token.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub prompt: f64,
    pub completion: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    /// Higher rates past a prompt size (long-context pricing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<PriceTier>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTier {
    pub min_prompt_tokens: u64,
    pub price: Price,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceBook {
    pub fetched_at_ms: i64,
    /// OpenRouter id (`anthropic/claude-opus-5.5`) → price.
    pub models: BTreeMap<String, Price>,
}

/// A price the user set by hand, in USD per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceOverride {
    pub input_per_m: f64,
    pub output_per_m: f64,
    #[serde(default)]
    pub cache_read_per_m: Option<f64>,
    #[serde(default)]
    pub cache_write_per_m: Option<f64>,
}

impl PriceOverride {
    fn price(&self) -> Price {
        Price {
            prompt: self.input_per_m / 1e6,
            completion: self.output_per_m / 1e6,
            cache_read: self.cache_read_per_m.map(|value| value / 1e6),
            cache_write: self.cache_write_per_m.map(|value| value / 1e6),
            tiers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSettings {
    /// Daily cap on metered spend; past it Sinew shows a notification
    /// (nothing is blocked).
    #[serde(default)]
    pub daily_budget_usd: Option<f64>,
    /// `provider/model` → price.
    #[serde(default)]
    pub price_overrides: BTreeMap<String, PriceOverride>,
}

fn price_book_cell() -> &'static RwLock<Option<PriceBook>> {
    static BOOK: OnceLock<RwLock<Option<PriceBook>>> = OnceLock::new();
    BOOK.get_or_init(Default::default)
}

fn current_book() -> Option<PriceBook> {
    price_book_cell().read().ok().and_then(|book| book.clone())
}

fn parse_rate(value: Option<&Value>) -> Option<f64> {
    let rate = match value? {
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    (rate.is_finite() && rate >= 0.0).then_some(rate)
}

fn parse_price(pricing: &Value) -> Option<Price> {
    Some(Price {
        prompt: parse_rate(pricing.get("prompt"))?,
        completion: parse_rate(pricing.get("completion"))?,
        cache_read: parse_rate(pricing.get("input_cache_read")),
        cache_write: parse_rate(pricing.get("input_cache_write")),
        tiers: Vec::new(),
    })
}

/// Parse OpenRouter's `/models` body into a price book.
pub fn parse_openrouter_catalog(body: &Value, fetched_at_ms: i64) -> PriceBook {
    let mut models = BTreeMap::new();
    for model in body.get("data").and_then(Value::as_array).into_iter().flatten() {
        let Some(id) = model.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(pricing) = model.get("pricing") else {
            continue;
        };
        let Some(mut price) = parse_price(pricing) else {
            continue;
        };
        // OpenRouter marks a model with a negative price when it is routed
        // dynamically; those have no fixed price.
        if price.prompt < 0.0 || price.completion < 0.0 {
            continue;
        }
        for tier in pricing
            .get("overrides")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(min_prompt_tokens) = tier.get("min_prompt_tokens").and_then(Value::as_u64)
            else {
                continue;
            };
            if let Some(tier_price) = parse_price(tier) {
                price.tiers.push(PriceTier {
                    min_prompt_tokens,
                    price: tier_price,
                });
            }
        }
        price.tiers.sort_by_key(|tier| tier.min_prompt_tokens);
        models.insert(id.to_string(), price);
    }
    PriceBook {
        fetched_at_ms,
        models,
    }
}

/// Load the cached price book at startup so costs work offline.
pub fn load_price_book(store: &AppStore) {
    let Ok(Some(book)) = store.load_json_setting::<PriceBook>(PRICE_BOOK_KEY) else {
        return;
    };
    if let Ok(mut cell) = price_book_cell().write() {
        *cell = Some(book);
    }
}

/// Fetch OpenRouter's public catalogue when the cached prices are older than
/// a day (or `force`). No API key is needed.
pub async fn refresh_price_book(store: &AppStore, force: bool) -> Result<()> {
    let now = crate::store::now_ms_pub();
    if !force {
        if let Some(book) = current_book() {
            if now - book.fetched_at_ms < PRICE_BOOK_MAX_AGE_MS {
                return Ok(());
            }
        }
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("sinew")
        .build()
        .context("unable to build HTTP client")?;
    let body: Value = http
        .get(OPENROUTER_MODELS_URL)
        .send()
        .await
        .context("unable to reach OpenRouter for prices")?
        .error_for_status()
        .context("OpenRouter refused the price request")?
        .json()
        .await
        .context("invalid OpenRouter price list")?;
    let book = parse_openrouter_catalog(&body, now);
    if book.models.is_empty() {
        anyhow::bail!("OpenRouter returned no prices");
    }
    store.save_json_setting(PRICE_BOOK_KEY, &book)?;
    if let Ok(mut cell) = price_book_cell().write() {
        *cell = Some(book);
    }
    Ok(())
}

pub fn price_book_age_ms() -> Option<i64> {
    current_book().map(|book| crate::store::now_ms_pub() - book.fetched_at_ms)
}

/// OpenRouter vendor prefix for each Sinew provider.
fn vendors(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &["anthropic"],
        "openai" => &["openai"],
        "google" => &["google"],
        "deepseek" => &["deepseek"],
        "kimi" => &["moonshotai"],
        // OpenCode Go serves several vendors' open models.
        "opencode_go" | "opencode-go" => &["moonshotai", "deepseek", "z-ai", "qwen", "minimax"],
        _ => &[],
    }
}

/// Lowercase, `.` → `-`, so `claude-opus-5-5` matches `claude-opus-5.5`.
fn normalize(id: &str) -> String {
    id.to_ascii_lowercase().replace(['.', '_'], "-")
}

fn tokens(id: &str) -> BTreeSet<String> {
    normalize(id)
        .split('-')
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

/// The OpenRouter entry that prices `provider/model`, if any.
pub fn resolve_price<'a>(
    book: &'a PriceBook,
    provider: &str,
    model: &str,
) -> Option<(&'a str, &'a Price)> {
    if provider == "openrouter" {
        return book
            .models
            .get_key_value(model)
            .map(|(id, price)| (id.as_str(), price));
    }
    let wanted = normalize(model.trim_start_matches("models/"));
    let wanted_tokens = tokens(&wanted);
    let candidates: Vec<(&String, &Price)> = book
        .models
        .iter()
        .filter(|(id, _)| !id.contains(':'))
        .filter(|(id, _)| {
            vendors(provider)
                .iter()
                .any(|vendor| id.starts_with(&format!("{vendor}/")))
        })
        .collect();
    let name = |id: &str| normalize(id.split_once('/').map(|(_, name)| name).unwrap_or(id));

    if let Some((id, price)) = candidates.iter().find(|(id, _)| name(id) == wanted) {
        return Some((id.as_str(), price));
    }
    // Closest superset: every token of ours present, fewest extra tokens;
    // ties go to the lexically greatest id (usually the newest version).
    candidates
        .iter()
        .filter_map(|(id, price)| {
            let theirs = tokens(&name(id));
            wanted_tokens
                .is_subset(&theirs)
                .then(|| (theirs.len() - wanted_tokens.len(), id.as_str(), *price))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(a.1)))
        .map(|(_, id, price)| (id, price))
}

/// Anthropic counts cached tokens apart from `input_tokens`; the other
/// providers include them.
fn uncached_input(provider: &str, usage: &Usage) -> u64 {
    let input = u64::from(usage.input_tokens);
    if provider == "anthropic" {
        input
    } else {
        input.saturating_sub(u64::from(usage.cache_read_tokens))
    }
}

fn prompt_size(provider: &str, usage: &Usage) -> u64 {
    uncached_input(provider, usage)
        + u64::from(usage.cache_read_tokens)
        + u64::from(usage.cache_creation_tokens)
}

pub fn cost_with(price: &Price, provider: &str, usage: &Usage) -> f64 {
    let size = prompt_size(provider, usage);
    let price = price
        .tiers
        .iter()
        .rev()
        .find(|tier| size >= tier.min_prompt_tokens)
        .map(|tier| &tier.price)
        .unwrap_or(price);
    uncached_input(provider, usage) as f64 * price.prompt
        + f64::from(usage.cache_read_tokens) * price.cache_read.unwrap_or(price.prompt)
        + f64::from(usage.cache_creation_tokens) * price.cache_write.unwrap_or(price.prompt)
        + f64::from(usage.output_tokens) * price.completion
}

/// Estimated cost of one response, and which price was used.
pub fn estimate_cost(
    provider: &str,
    model: &str,
    usage: &Usage,
    settings: &UsageSettings,
) -> (Option<f64>, Option<String>) {
    if let Some(price_override) = settings.price_overrides.get(&format!("{provider}/{model}")) {
        return (
            Some(cost_with(&price_override.price(), provider, usage)),
            Some("custom".into()),
        );
    }
    let Some(book) = current_book() else {
        return (None, None);
    };
    match resolve_price(&book, provider, model) {
        Some((id, price)) => (Some(cost_with(price, provider, usage)), Some(id.to_string())),
        None => (None, None),
    }
}

// ---------------------------------------------------------------------------
// Storage.

pub(crate) fn ensure_usage_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        create table if not exists usage_events (
            id integer primary key autoincrement,
            ts_ms integer not null,
            workspace_id text not null,
            conversation_id text not null,
            agent text,
            provider text not null,
            model text not null,
            input_tokens integer not null,
            output_tokens integer not null,
            cache_read_tokens integer not null,
            cache_write_tokens integer not null,
            reasoning_tokens integer not null,
            cost_usd real,
            billing text not null,
            priced_as text
        );
        create index if not exists idx_usage_events_ts on usage_events(ts_ms);
        create index if not exists idx_usage_events_conversation on usage_events(conversation_id);
        "#,
    )
    .context("unable to create usage table")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub ts_ms: i64,
    pub workspace_id: String,
    pub conversation_id: String,
    /// Sub-agent or swarm member name; `None` for the main agent.
    pub agent: Option<String>,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: Option<f64>,
    pub billing: Billing,
    pub priced_as: Option<String>,
}

impl UsageRecord {
    /// A record for one model response, priced with the current book.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace_id: &str,
        conversation_id: &str,
        agent: Option<String>,
        provider: &str,
        model: &str,
        usage: &Usage,
        billing: Billing,
        settings: &UsageSettings,
    ) -> Self {
        let (cost_usd, priced_as) = estimate_cost(provider, model, usage, settings);
        Self {
            ts_ms: crate::store::now_ms_pub(),
            workspace_id: workspace_id.to_string(),
            conversation_id: conversation_id.to_string(),
            agent,
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens: uncached_input(provider, usage),
            output_tokens: u64::from(usage.output_tokens),
            cache_read_tokens: u64::from(usage.cache_read_tokens),
            cache_write_tokens: u64::from(usage.cache_creation_tokens),
            reasoning_tokens: u64::from(usage.reasoning_tokens),
            cost_usd,
            billing,
            priced_as,
        }
    }

    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// Token and cost totals.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub responses: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
    /// Real spend on metered providers.
    pub metered_cost_usd: f64,
    /// What subscription usage would have cost on the API.
    pub subscription_cost_usd: f64,
    /// Responses whose model had no known price.
    pub unpriced_responses: u64,
}

impl UsageTotals {
    fn add(&mut self, record: &UsageRecord) {
        self.responses += 1;
        self.input_tokens += record.input_tokens;
        self.output_tokens += record.output_tokens;
        self.cache_read_tokens += record.cache_read_tokens;
        self.cache_write_tokens += record.cache_write_tokens;
        self.reasoning_tokens += record.reasoning_tokens;
        self.total_tokens += record.total_tokens();
        match (record.cost_usd, record.billing) {
            (Some(cost), Billing::Metered) => self.metered_cost_usd += cost,
            (Some(cost), Billing::Subscription) => self.subscription_cost_usd += cost,
            (None, _) => self.unpriced_responses += 1,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    #[serde(flatten)]
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub since_ms: i64,
    pub totals: UsageTotals,
    pub by_provider: Vec<UsageGroup>,
    pub by_model: Vec<UsageGroup>,
    pub by_project: Vec<UsageGroup>,
    /// Local calendar days, `YYYY-MM-DD`, oldest first, gaps included.
    pub by_day: Vec<UsageGroup>,
}

fn group(map: BTreeMap<String, UsageTotals>) -> Vec<UsageGroup> {
    let mut groups: Vec<UsageGroup> = map
        .into_iter()
        .map(|(key, totals)| UsageGroup { key, totals })
        .collect();
    groups.sort_by(|a, b| {
        let cost = |g: &UsageGroup| g.totals.metered_cost_usd + g.totals.subscription_cost_usd;
        cost(b)
            .partial_cmp(&cost(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.totals.total_tokens.cmp(&a.totals.total_tokens))
    });
    groups
}

/// `YYYY-MM-DD` of a unix-ms instant shifted by the user's UTC offset.
fn day_key(ts_ms: i64, utc_offset_minutes: i32) -> String {
    let days = (ts_ms + i64::from(utc_offset_minutes) * 60_000).div_euclid(DAY_MS);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn project_name(workspace_id: &str) -> String {
    workspace_id
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(workspace_id)
        .to_string()
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    Ok(UsageRecord {
        ts_ms: row.get(0)?,
        workspace_id: row.get(1)?,
        conversation_id: row.get(2)?,
        agent: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        input_tokens: row.get::<_, i64>(6)?.max(0) as u64,
        output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
        cache_read_tokens: row.get::<_, i64>(8)?.max(0) as u64,
        cache_write_tokens: row.get::<_, i64>(9)?.max(0) as u64,
        reasoning_tokens: row.get::<_, i64>(10)?.max(0) as u64,
        cost_usd: row.get(11)?,
        billing: Billing::parse(&row.get::<_, String>(12)?),
        priced_as: row.get(13)?,
    })
}

const SELECT_COLUMNS: &str = "select ts_ms, workspace_id, conversation_id, agent, provider, model, \
     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, \
     cost_usd, billing, priced_as from usage_events";

impl AppStore {
    pub fn load_usage_settings(&self) -> Result<UsageSettings> {
        Ok(self
            .load_json_setting::<UsageSettings>(USAGE_SETTINGS_KEY)?
            .unwrap_or_default())
    }

    pub fn save_usage_settings(&self, settings: &UsageSettings) -> Result<()> {
        self.save_json_setting(USAGE_SETTINGS_KEY, settings)
    }

    pub fn record_usage(&self, record: &UsageRecord) -> Result<()> {
        let conn = self.connection()?;
        conn.execute(
            "insert into usage_events (ts_ms, workspace_id, conversation_id, agent, provider, \
             model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, \
             reasoning_tokens, cost_usd, billing, priced_as) \
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                record.ts_ms,
                record.workspace_id,
                record.conversation_id,
                record.agent,
                record.provider,
                record.model,
                record.input_tokens as i64,
                record.output_tokens as i64,
                record.cache_read_tokens as i64,
                record.cache_write_tokens as i64,
                record.reasoning_tokens as i64,
                record.cost_usd,
                record.billing.as_str(),
                record.priced_as,
            ],
        )
        .context("unable to record usage")?;
        Ok(())
    }

    fn usage_records_since(&self, since_ms: i64) -> Result<Vec<UsageRecord>> {
        let conn = self.connection()?;
        let mut statement = conn
            .prepare(&format!("{SELECT_COLUMNS} where ts_ms >= ?1 order by ts_ms"))
            .context("unable to query usage")?;
        let records = statement
            .query_map(params![since_ms], row_to_record)
            .context("unable to query usage")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("unable to read usage")?;
        Ok(records)
    }

    pub fn usage_summary(&self, since_ms: i64, utc_offset_minutes: i32) -> Result<UsageSummary> {
        let records = self.usage_records_since(since_ms)?;
        let mut summary = UsageSummary {
            since_ms,
            ..Default::default()
        };
        let mut by_provider = BTreeMap::<String, UsageTotals>::new();
        let mut by_model = BTreeMap::<String, UsageTotals>::new();
        let mut by_project = BTreeMap::<String, UsageTotals>::new();
        let mut by_day = BTreeMap::<String, UsageTotals>::new();
        for record in &records {
            summary.totals.add(record);
            by_provider.entry(record.provider.clone()).or_default().add(record);
            by_model
                .entry(format!("{}/{}", record.provider, record.model))
                .or_default()
                .add(record);
            by_project
                .entry(project_name(&record.workspace_id))
                .or_default()
                .add(record);
            by_day
                .entry(day_key(record.ts_ms, utc_offset_minutes))
                .or_default()
                .add(record);
        }
        // Every day of the range, so the chart has no holes.
        let now = crate::store::now_ms_pub();
        let mut day = since_ms.max(now - 366 * DAY_MS);
        while day <= now {
            by_day.entry(day_key(day, utc_offset_minutes)).or_default();
            day += DAY_MS;
        }
        summary.by_provider = group(by_provider);
        summary.by_model = group(by_model);
        summary.by_project = group(by_project);
        summary.by_day = by_day
            .into_iter()
            .map(|(key, totals)| UsageGroup { key, totals })
            .collect();
        Ok(summary)
    }

    pub fn conversation_usage(&self, conversation_id: &str) -> Result<UsageTotals> {
        let conn = self.connection()?;
        let mut statement = conn
            .prepare(&format!("{SELECT_COLUMNS} where conversation_id = ?1"))
            .context("unable to query usage")?;
        let mut totals = UsageTotals::default();
        for record in statement
            .query_map(params![conversation_id], row_to_record)
            .context("unable to query usage")?
        {
            totals.add(&record.context("unable to read usage")?);
        }
        Ok(totals)
    }

    /// Real (metered) spend since an instant, for the daily budget.
    pub fn metered_spend_since(&self, since_ms: i64) -> Result<f64> {
        let conn = self.connection()?;
        let spend: Option<f64> = conn
            .query_row(
                "select sum(cost_usd) from usage_events where ts_ms >= ?1 and billing = 'metered'",
                params![since_ms],
                |row| row.get(0),
            )
            .optional()
            .context("unable to read spend")?
            .flatten();
        Ok(spend.unwrap_or(0.0))
    }

    pub fn usage_csv(&self, since_ms: i64) -> Result<String> {
        let mut csv = String::from(
            "timestamp_ms,project,conversation_id,agent,provider,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,cost_usd,billing,priced_as\n",
        );
        let field = |value: &str| {
            if value.contains([',', '"', '\n']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value.to_string()
            }
        };
        for record in self.usage_records_since(since_ms)? {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                record.ts_ms,
                field(&project_name(&record.workspace_id)),
                field(&record.conversation_id),
                field(record.agent.as_deref().unwrap_or("")),
                field(&record.provider),
                field(&record.model),
                record.input_tokens,
                record.output_tokens,
                record.cache_read_tokens,
                record.cache_write_tokens,
                record.reasoning_tokens,
                record
                    .cost_usd
                    .map(|cost| format!("{cost:.6}"))
                    .unwrap_or_default(),
                record.billing.as_str(),
                field(record.priced_as.as_deref().unwrap_or("")),
            ));
        }
        Ok(csv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn book() -> PriceBook {
        parse_openrouter_catalog(
            &json!({ "data": [
                { "id": "anthropic/claude-opus-5.5", "pricing": {
                    "prompt": "0.000005", "completion": "0.000025",
                    "input_cache_read": "0.0000005", "input_cache_write": "0.00000625",
                    "overrides": [{ "min_prompt_tokens": 200000, "prompt": "0.00001", "completion": "0.0000375" }]
                }},
                { "id": "anthropic/claude-opus-5.5:batch", "pricing": { "prompt": "0.0000025", "completion": "0.0000125" }},
                { "id": "openai/gpt-6.1-sol", "pricing": { "prompt": "0.000002", "completion": "0.000008", "input_cache_read": "0.0000002" }},
                { "id": "deepseek/deepseek-v4-flash", "pricing": { "prompt": "0.0000001", "completion": "0.0000004" }},
                { "id": "deepseek/deepseek-v4.1-flash", "pricing": { "prompt": "0.00000012", "completion": "0.0000005" }},
                { "id": "moonshotai/kimi-k2.7-code", "pricing": { "prompt": "0.0000006", "completion": "0.0000025" }},
                { "id": "openrouter/auto", "pricing": { "prompt": "-1", "completion": "-1" }}
            ]}),
            1,
        )
    }

    #[test]
    fn sinew_ids_find_their_openrouter_price() {
        let book = book();
        assert_eq!(
            resolve_price(&book, "anthropic", "claude-opus-5-5").map(|(id, _)| id),
            Some("anthropic/claude-opus-5.5")
        );
        assert_eq!(
            resolve_price(&book, "openai", "gpt-6.1-sol").map(|(id, _)| id),
            Some("openai/gpt-6.1-sol")
        );
        assert_eq!(
            resolve_price(&book, "kimi", "kimi-k2.7-code-highspeed").map(|(id, _)| id),
            None,
            "a model with extra words must not borrow a cheaper sibling's price"
        );
        assert!(resolve_price(&book, "deepseek", "deepseek-flash").is_some());
        assert!(resolve_price(&book, "anthropic", "claude-unknown-9").is_none());
        assert!(!book.models.contains_key("openrouter/auto"));
    }

    #[test]
    fn costs_count_cache_and_long_context_tiers() {
        let book = book();
        let (_, opus) = resolve_price(&book, "anthropic", "claude-opus-5-5").unwrap();
        // Anthropic: input excludes cache reads.
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 2_000,
            cache_read_tokens: 10_000,
            cache_creation_tokens: 0,
            ..Default::default()
        };
        let cost = cost_with(opus, "anthropic", &usage);
        assert!((cost - (1_000.0 * 5e-6 + 10_000.0 * 5e-7 + 2_000.0 * 25e-6)).abs() < 1e-12);

        let long = Usage {
            input_tokens: 250_000,
            output_tokens: 1_000,
            ..Default::default()
        };
        let cost = cost_with(opus, "anthropic", &long);
        assert!((cost - (250_000.0 * 1e-5 + 1_000.0 * 37.5e-6)).abs() < 1e-9);

        // OpenAI: input includes the cached part.
        let (_, sol) = resolve_price(&book, "openai", "gpt-6.1-sol").unwrap();
        let usage = Usage {
            input_tokens: 10_000,
            cache_read_tokens: 8_000,
            output_tokens: 100,
            ..Default::default()
        };
        let cost = cost_with(sol, "openai", &usage);
        assert!((cost - (2_000.0 * 2e-6 + 8_000.0 * 2e-7 + 100.0 * 8e-6)).abs() < 1e-12);
    }

    #[test]
    fn days_follow_the_local_calendar() {
        // 2026-10-27 23:30 UTC is already the 28th in Paris (UTC+1).
        let ts = 1_793_143_800_000;
        assert_eq!(day_key(ts, 0), "2026-10-27");
        assert_eq!(day_key(ts, 60), "2026-10-28");
        assert_eq!(day_key(0, 0), "1970-01-01");
    }

    #[test]
    fn records_roll_up_by_billing_model_and_day() -> Result<()> {
        let path = std::env::temp_dir().join(format!(
            "sinew-usage-test-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let store = AppStore::open_at(&path)?;
        let settings = UsageSettings::default();
        let usage = Usage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        };
        let mut metered = UsageRecord::new(
            "C:\\code\\app", "c1", None, "deepseek", "deepseek-flash", &usage,
            Billing::Metered, &settings,
        );
        metered.cost_usd = Some(0.5);
        let mut plan = UsageRecord::new(
            "C:\\code\\app", "c1", Some("explorer".into()), "anthropic", "claude-opus-5-5",
            &usage, Billing::Subscription, &settings,
        );
        plan.cost_usd = Some(2.0);
        let mut unpriced = plan.clone();
        unpriced.cost_usd = None;
        unpriced.conversation_id = "c2".into();
        for record in [&metered, &plan, &unpriced] {
            store.record_usage(record)?;
        }

        let summary = store.usage_summary(0, 0)?;
        assert_eq!(summary.totals.responses, 3);
        assert!((summary.totals.metered_cost_usd - 0.5).abs() < 1e-9);
        assert!((summary.totals.subscription_cost_usd - 2.0).abs() < 1e-9);
        assert_eq!(summary.totals.unpriced_responses, 1);
        assert_eq!(summary.by_project[0].key, "app");
        assert_eq!(summary.by_model[0].key, "anthropic/claude-opus-5-5");

        let conversation = store.conversation_usage("c1")?;
        assert_eq!(conversation.responses, 2);
        assert_eq!(conversation.total_tokens, 3_000);
        assert!((store.metered_spend_since(0)? - 0.5).abs() < 1e-9);
        let csv = store.usage_csv(0)?;
        assert_eq!(csv.lines().count(), 4);
        assert!(csv.contains(",explorer,anthropic,claude-opus-5-5,"));
        let _ = std::fs::remove_file(&path);
        Ok(())
    }
}
