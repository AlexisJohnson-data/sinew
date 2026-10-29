//! What a tab does on its own: console messages, uncaught exceptions,
//! network requests, and whether the page has settled after an action.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::log::EventEntryAdded;
use chromiumoxide::cdp::browser_protocol::network::{
    EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    EventConsoleApiCalled, EventExceptionThrown, StackTrace,
};
use chromiumoxide::Page;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::cdp;

const MAX_CONSOLE_ENTRIES: usize = 500;
const MAX_NETWORK_ENTRIES: usize = 500;
/// A request still open after this long is treated as a long poll or a
/// stream and no longer keeps the page "busy".
const LONG_REQUEST: Duration = Duration::from_secs(3);
const MAX_STACK_FRAMES: usize = 6;

#[derive(Debug, Clone)]
pub struct ConsoleEntry {
    pub seq: u64,
    /// `error`, `warning`, `log`, `info`, `debug`, `exception`…
    pub level: String,
    pub text: String,
    /// `url:line:col` frames, innermost first.
    pub stack: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct NetworkEntry {
    pub id: u64,
    pub method: String,
    pub url: String,
    pub resource_type: String,
    pub status: Option<i64>,
    pub status_text: String,
    pub mime_type: String,
    pub failed: Option<String>,
    pub duration_ms: Option<u64>,
    pub size_bytes: Option<u64>,
    pub request_headers: Value,
    pub response_headers: Value,
    pub post_data: Option<String>,
    pub(crate) request_id: String,
    started: Instant,
}

#[derive(Default)]
pub(crate) struct NetworkLog {
    entries: VecDeque<NetworkEntry>,
    next_id: u64,
    /// Open requests and when they started, for the settle check.
    inflight: HashMap<String, Instant>,
    last_activity: Option<Instant>,
}

#[derive(Default)]
pub struct TabLog {
    console: Mutex<VecDeque<ConsoleEntry>>,
    next_console: Mutex<u64>,
    pub(crate) network: Mutex<NetworkLog>,
}

impl TabLog {
    async fn push_console(&self, level: String, text: String, stack: Vec<String>) {
        let seq = {
            let mut next = self.next_console.lock().await;
            *next += 1;
            *next
        };
        let mut console = self.console.lock().await;
        if console.len() >= MAX_CONSOLE_ENTRIES {
            console.pop_front();
        }
        console.push_back(ConsoleEntry {
            seq,
            level,
            text,
            stack,
        });
    }

    /// A message from Sinew itself (an auto-accepted dialog…).
    pub(crate) async fn note(&self, text: String) {
        self.push_console("info".into(), text, Vec::new()).await;
    }

    /// The last console sequence number, to find what an action produced.
    pub async fn console_mark(&self) -> u64 {
        *self.next_console.lock().await
    }

    pub async fn console_since(&self, mark: u64) -> Vec<ConsoleEntry> {
        self.console
            .lock()
            .await
            .iter()
            .filter(|entry| entry.seq > mark)
            .cloned()
            .collect()
    }

    pub async fn console_entries(&self, clear: bool) -> Vec<ConsoleEntry> {
        let mut console = self.console.lock().await;
        let entries = console.iter().cloned().collect();
        if clear {
            console.clear();
        }
        entries
    }

    pub async fn network_entries(&self, clear: bool) -> Vec<NetworkEntry> {
        let mut log = self.network.lock().await;
        let entries = log.entries.iter().cloned().collect();
        if clear {
            log.entries.clear();
        }
        entries
    }

    pub async fn network_entry(&self, id: u64) -> Option<NetworkEntry> {
        let log = self.network.lock().await;
        log.entries.iter().find(|entry| entry.id == id).cloned()
    }

    /// True when no short-lived request is open and none finished in the
    /// last `quiet`.
    async fn network_quiet(&self, quiet: Duration) -> bool {
        let log = self.network.lock().await;
        let busy = log
            .inflight
            .values()
            .any(|started| started.elapsed() < LONG_REQUEST);
        !busy && log.last_activity.map_or(true, |at| at.elapsed() >= quiet)
    }
}

/// Starts recording console, exceptions, log entries and network traffic
/// for `page` into `log`. Events the bundled protocol types cannot decode
/// are dropped by chromiumoxide; the rest keep flowing.
pub(crate) async fn attach(page: &Page, log: Arc<TabLog>) {
    for (method, params) in [
        ("Runtime.enable", serde_json::json!({})),
        ("Network.enable", serde_json::json!({})),
        ("Log.enable", serde_json::json!({})),
        ("Page.enable", serde_json::json!({})),
        ("DOM.enable", serde_json::json!({})),
        ("Accessibility.enable", serde_json::json!({})),
    ] {
        let _ = cdp::call(page, method, params).await;
    }
    // Page-side helpers: last DOM mutation time and Web Vitals, installed in
    // every new document and in the current one.
    let _ = cdp::call(
        page,
        "Page.addScriptToEvaluateOnNewDocument",
        serde_json::json!({ "source": PAGE_HELPERS }),
    )
    .await;
    let _ = cdp::eval(page, PAGE_HELPERS).await;

    if let Ok(mut events) = page.event_listener::<EventConsoleApiCalled>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let text = event
                    .args
                    .iter()
                    .map(|arg| match &arg.value {
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => other.to_string(),
                        None => arg.description.clone().unwrap_or_default(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let level = match event.r#type.as_ref() {
                    "warning" => "warning",
                    "error" | "assert" => "error",
                    "debug" => "debug",
                    "info" => "info",
                    _ => "log",
                };
                log.push_console(level.into(), text, frames(event.stack_trace.as_ref()))
                    .await;
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventExceptionThrown>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let details = &event.exception_details;
                let text = details
                    .exception
                    .as_ref()
                    .and_then(|exception| exception.description.clone())
                    .map(|description| description.lines().next().unwrap_or("").to_string())
                    .unwrap_or_else(|| details.text.clone());
                let mut stack = frames(details.stack_trace.as_ref());
                if stack.is_empty() {
                    if let Some(url) = &details.url {
                        stack.push(format!(
                            "{url}:{}:{}",
                            details.line_number + 1,
                            details.column_number + 1
                        ));
                    }
                }
                log.push_console("exception".into(), text, stack).await;
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventEntryAdded>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let entry = &event.entry;
                // Browser-side messages: failed loads, CSP, deprecations.
                if !matches!(entry.level.as_ref(), "error" | "warning") {
                    continue;
                }
                // Chrome asks every site for a favicon; a missing one is noise.
                if entry.url.as_deref().is_some_and(|url| url.ends_with("/favicon.ico")) {
                    continue;
                }
                let mut stack = frames(entry.stack_trace.as_ref());
                if stack.is_empty() {
                    if let Some(url) = &entry.url {
                        stack.push(match entry.line_number {
                            Some(line) => format!("{url}:{}", line + 1),
                            None => url.clone(),
                        });
                    }
                }
                log.push_console(entry.level.as_ref().to_string(), entry.text.clone(), stack)
                    .await;
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventRequestWillBeSent>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let request_id = event.request_id.inner().clone();
                let mut network = log.network.lock().await;
                let now = Instant::now();
                network.inflight.insert(request_id.clone(), now);
                network.last_activity = Some(now);
                network.next_id += 1;
                let id = network.next_id;
                if network.entries.len() >= MAX_NETWORK_ENTRIES {
                    network.entries.pop_front();
                }
                network.entries.push_back(NetworkEntry {
                    id,
                    method: event.request.method.clone(),
                    url: event.request.url.clone(),
                    resource_type: event
                        .r#type
                        .as_ref()
                        .map(|kind| kind.as_ref().to_string())
                        .unwrap_or_default(),
                    status: None,
                    status_text: String::new(),
                    mime_type: String::new(),
                    failed: None,
                    duration_ms: None,
                    size_bytes: None,
                    request_headers: event.request.headers.inner().clone(),
                    response_headers: Value::Null,
                    post_data: event
                        .request
                        .post_data_entries
                        .as_ref()
                        .and_then(|parts| serde_json::to_value(parts).ok())
                        .map(|value| post_data_text(&value)),
                    request_id,
                    started: now,
                });
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventResponseReceived>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let request_id = event.request_id.inner();
                let mut network = log.network.lock().await;
                if let Some(entry) = network
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|entry| &entry.request_id == request_id)
                {
                    entry.status = Some(event.response.status);
                    entry.status_text = event.response.status_text.clone();
                    entry.mime_type = event.response.mime_type.clone();
                    entry.response_headers = event.response.headers.inner().clone();
                    if entry.resource_type.is_empty() {
                        entry.resource_type = event.r#type.as_ref().to_string();
                    }
                }
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventLoadingFinished>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let request_id = event.request_id.inner();
                let mut network = log.network.lock().await;
                network.inflight.remove(request_id);
                network.last_activity = Some(Instant::now());
                if let Some(entry) = network
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|entry| &entry.request_id == request_id)
                {
                    entry.duration_ms = Some(entry.started.elapsed().as_millis() as u64);
                    entry.size_bytes = Some(event.encoded_data_length.max(0.0) as u64);
                }
            }
        });
    }

    if let Ok(mut events) = page.event_listener::<EventLoadingFailed>().await {
        let log = Arc::clone(&log);
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let request_id = event.request_id.inner();
                let mut network = log.network.lock().await;
                network.inflight.remove(request_id);
                network.last_activity = Some(Instant::now());
                if let Some(entry) = network
                    .entries
                    .iter_mut()
                    .rev()
                    .find(|entry| &entry.request_id == request_id)
                {
                    entry.duration_ms = Some(entry.started.elapsed().as_millis() as u64);
                    entry.failed = Some(if event.canceled == Some(true) {
                        "canceled".to_string()
                    } else {
                        event.error_text.clone()
                    });
                }
            }
        });
    }
}

/// The body of a finished response, as text (base64 bodies are reported
/// by size only).
pub async fn response_body(page: &Page, entry: &NetworkEntry) -> Result<String, String> {
    let body = cdp::call(
        page,
        "Network.getResponseBody",
        serde_json::json!({ "requestId": entry.request_id }),
    )
    .await
    .map_err(|err| err.to_string())?;
    if body["base64Encoded"].as_bool() == Some(true) {
        let size = body["body"].as_str().map_or(0, |data| data.len() * 3 / 4);
        return Err(format!("binary body ({size} bytes)"));
    }
    Ok(body["body"].as_str().unwrap_or_default().to_string())
}

/// Load timings, Web Vitals, long tasks and the slowest resources of the
/// current document, as collected by the page helpers.
pub async fn performance(page: &Page) -> anyhow::Result<Value> {
    cdp::eval(
        page,
        r#"(() => {
  const s = window.__sinew || {};
  const nav = performance.getEntriesByType('navigation')[0];
  const paint = Object.fromEntries(performance.getEntriesByType('paint').map((p) => [p.name, p.startTime]));
  const resources = performance.getEntriesByType('resource');
  const round = (n) => (n == null ? null : Math.round(n));
  return {
    url: location.href,
    ttfb: nav ? round(nav.responseStart) : null,
    fcp: round(paint['first-contentful-paint']),
    domContentLoaded: nav ? round(nav.domContentLoadedEventEnd) : null,
    load: nav ? round(nav.loadEventEnd) : null,
    lcp: s.lcp ? { ms: round(s.lcp.ms), element: s.lcp.element } : null,
    cls: Math.round((s.cls || 0) * 1000) / 1000,
    inp: s.inp ? round(s.inp) : null,
    longTasks: s.longTasks || 0,
    longTaskMs: round(s.longTaskMs || 0),
    requests: resources.length,
    transferKb: Math.round(resources.reduce((sum, r) => sum + (r.transferSize || 0), (nav && nav.transferSize) || 0) / 1024),
    slowest: resources.slice().sort((a, b) => b.duration - a.duration).slice(0, 5)
      .map((r) => ({ url: r.name, ms: round(r.duration), kb: Math.round((r.transferSize || 0) / 1024), type: r.initiatorType })),
  };
})()"#,
    )
    .await
}

fn frames(stack: Option<&StackTrace>) -> Vec<String> {
    stack
        .map(|stack| {
            stack
                .call_frames
                .iter()
                .filter(|frame| !frame.url.is_empty())
                .take(MAX_STACK_FRAMES)
                .map(|frame| {
                    let location = format!(
                        "{}:{}:{}",
                        frame.url,
                        frame.line_number + 1,
                        frame.column_number + 1
                    );
                    if frame.function_name.is_empty() {
                        location
                    } else {
                        format!("{} ({location})", frame.function_name)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn post_data_text(entries: &Value) -> String {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    entries
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part["bytes"].as_str())
                .filter_map(|bytes| BASE64.decode(bytes).ok())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Waits until the page looks done after an action: no short request open
/// and no DOM change for a moment, or `max` elapsed. Returns whether it
/// settled before the deadline.
pub(crate) async fn settle(page: &Page, log: &TabLog, max: Duration) -> bool {
    const QUIET: Duration = Duration::from_millis(350);
    let deadline = Instant::now() + max;
    // Give the action a moment to start requests or mutations.
    tokio::time::sleep(Duration::from_millis(60)).await;
    loop {
        let dom_idle_ms = cdp::eval(
            page,
            "window.__sinew ? performance.now() - window.__sinew.lastMutation : 1e9",
        )
        .await
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(1e9);
        if dom_idle_ms >= QUIET.as_millis() as f64 && log.network_quiet(QUIET).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Installed in every document: tracks the last DOM mutation (for
/// `settle`) and collects Web Vitals (for `browser_performance`).
const PAGE_HELPERS: &str = r#"(() => {
  if (window.__sinew) return;
  const state = window.__sinew = { lastMutation: performance.now(), lcp: null, cls: 0, inp: 0, longTasks: 0, longTaskMs: 0 };
  const watch = () => {
    try {
      new MutationObserver(() => { state.lastMutation = performance.now(); })
        .observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    } catch (_) {}
  };
  if (document.documentElement) watch(); else document.addEventListener('readystatechange', watch, { once: true });
  const observe = (type, onEntry) => {
    try { new PerformanceObserver((list) => list.getEntries().forEach(onEntry)).observe({ type, buffered: true }); } catch (_) {}
  };
  observe('largest-contentful-paint', (entry) => {
    const el = entry.element;
    state.lcp = { ms: entry.startTime, size: entry.size, element: el ? (el.tagName.toLowerCase() + (el.id ? '#' + el.id : '') + (typeof el.className === 'string' && el.className.trim() ? '.' + el.className.trim().split(/\s+/)[0] : '')) : null };
  });
  observe('layout-shift', (entry) => { if (!entry.hadRecentInput) state.cls += entry.value; });
  observe('event', (entry) => { if (entry.interactionId && entry.duration > state.inp) state.inp = entry.duration; });
  observe('longtask', (entry) => { state.longTasks += 1; state.longTaskMs += entry.duration; });
})()"#;
