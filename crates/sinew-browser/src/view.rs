//! The live view behind Sinew's browser panel: the active tab streamed as
//! JPEG frames (a CDP screencast), and the user's mouse and keyboard sent
//! back as real input events. The agent drives the same tabs with its tools,
//! so the user watches it work and can step in (log in, pick an element).

use std::time::Duration;

use anyhow::{anyhow, Result};
use chromiumoxide::cdp::browser_protocol::page::EventScreencastFrame;
use chromiumoxide::Page;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::cdp;
use crate::session::{BrowserSession, BrowserSessions, VIEWPORT};

/// How often the view checks whether the agent switched tabs.
const TAB_POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewFrame {
    pub tab: String,
    /// Base64 JPEG.
    pub data: String,
    /// The page's CSS size: input coordinates are in this space.
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// User input, in page CSS pixels for the mouse.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ViewInput {
    #[serde(rename_all = "camelCase")]
    Mouse {
        /// `mousePressed`, `mouseReleased`, `mouseMoved` or `mouseWheel`.
        action: String,
        x: f64,
        y: f64,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        click_count: Option<u32>,
        #[serde(default)]
        delta_x: Option<f64>,
        #[serde(default)]
        delta_y: Option<f64>,
        #[serde(default)]
        modifiers: u32,
    },
    #[serde(rename_all = "camelCase")]
    Key {
        /// `keyDown` or `keyUp`.
        action: String,
        key: String,
        code: String,
        #[serde(default)]
        key_code: Option<u32>,
        /// The character typed, for printable keys.
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        modifiers: u32,
    },
}

/// Launches a windowless browser for `workspace` if none is running.
pub async fn ensure_session(sessions: &BrowserSessions, workspace: &str) -> Result<()> {
    let mut map = sessions.0.lock().await;
    if !map.contains_key(workspace) {
        let session = BrowserSession::launch(true).await?;
        map.insert(workspace.to_string(), session);
    }
    Ok(())
}

async fn active_page(sessions: &BrowserSessions, workspace: &str) -> Result<(String, Page)> {
    let mut map = sessions.0.lock().await;
    let session = map
        .get_mut(workspace)
        .ok_or_else(|| anyhow!("the browser is not running"))?;
    session.sync_tabs().await;
    let tab = session.active()?;
    Ok((tab.id.clone(), tab.page.clone()))
}

/// The active tab's id, without waiting: `None` while a tool or a
/// navigation holds the browser (a page can take 30 s to load, and frames
/// must keep flowing meanwhile), `Some(None)` once the browser is gone.
async fn active_tab_if_free(sessions: &BrowserSessions, workspace: &str) -> Option<Option<String>> {
    let mut map = sessions.0.try_lock().ok()?;
    let Some(session) = map.get_mut(workspace) else {
        return Some(None);
    };
    session.sync_tabs().await;
    Some(session.active().ok().map(|tab| tab.id.clone()))
}

/// Streams the active tab to `on_frame` until `stop` turns true, following
/// the agent from tab to tab. Ends when the browser closes.
pub async fn stream(
    sessions: BrowserSessions,
    workspace: String,
    mut stop: watch::Receiver<bool>,
    on_frame: impl Fn(ViewFrame) + Send + 'static,
) {
    while !*stop.borrow() {
        let (tab, page) = match active_page(&sessions, &workspace).await {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!("browser view: no active tab: {err}");
                return;
            }
        };
        let mut frames = match page.event_listener::<EventScreencastFrame>().await {
            Ok(frames) => frames,
            Err(err) => {
                tracing::warn!("browser view: cannot listen to frames: {err}");
                return;
            }
        };
        let started = cdp::call(
            &page,
            "Page.startScreencast",
            json!({
                "format": "jpeg",
                "quality": 70,
                "maxWidth": VIEWPORT.0,
                "maxHeight": VIEWPORT.1,
                "everyNthFrame": 1,
            }),
        )
        .await;
        if let Err(err) = started {
            tracing::warn!("browser view: screencast failed: {err}");
            return;
        }
        tracing::debug!("browser view: streaming {tab}");
        let mut poll = tokio::time::interval(TAB_POLL);
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                frame = frames.next() => {
                    let Some(frame) = frame else {
                        tracing::warn!("browser view: frame stream ended");
                        break;
                    };
                    // Chrome sends the next frame only once this one is acked.
                    let _ = cdp::call(
                        &page,
                        "Page.screencastFrameAck",
                        json!({ "sessionId": frame.session_id }),
                    )
                    .await;
                    let data: &str = frame.data.as_ref();
                    on_frame(ViewFrame {
                        tab: tab.clone(),
                        data: data.to_string(),
                        width: frame.metadata.device_width,
                        height: frame.metadata.device_height,
                    });
                }
                _ = poll.tick() => {
                    match active_tab_if_free(&sessions, &workspace).await {
                        Some(Some(current)) if current != tab => break,
                        Some(None) => return,
                        _ => {}
                    }
                }
            }
        }
        let _ = cdp::call(&page, "Page.stopScreencast", json!({})).await;
    }
}

/// Sends the user's input to the active tab.
pub async fn send_input(sessions: &BrowserSessions, workspace: &str, input: ViewInput) -> Result<()> {
    let (_, page) = active_page(sessions, workspace).await?;
    match input {
        ViewInput::Mouse {
            action,
            x,
            y,
            button,
            click_count,
            delta_x,
            delta_y,
            modifiers,
        } => {
            let mut params = json!({
                "type": action,
                "x": x,
                "y": y,
                "modifiers": modifiers,
                "button": button.unwrap_or_else(|| "none".into()),
                "clickCount": click_count.unwrap_or(0),
            });
            if action == "mouseWheel" {
                params["deltaX"] = json!(delta_x.unwrap_or(0.0));
                params["deltaY"] = json!(delta_y.unwrap_or(0.0));
            }
            cdp::call(&page, "Input.dispatchMouseEvent", params).await?;
        }
        ViewInput::Key {
            action,
            key,
            code,
            key_code,
            text,
            modifiers,
        } => {
            // A printable key goes in as `keyDown` with its text, which the
            // page receives as keydown + keypress + input.
            let mut params = json!({
                "type": action,
                "key": key,
                "code": code,
                "modifiers": modifiers,
            });
            if let Some(key_code) = key_code {
                params["windowsVirtualKeyCode"] = json!(key_code);
                params["nativeVirtualKeyCode"] = json!(key_code);
            }
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                params["text"] = json!(text);
                params["unmodifiedText"] = json!(text);
            }
            cdp::call(&page, "Input.dispatchKeyEvent", params).await?;
        }
    }
    Ok(())
}

/// The open tabs, for the panel's header.
pub async fn tabs(sessions: &BrowserSessions, workspace: &str) -> Result<Vec<ViewTab>> {
    let mut map = sessions.0.lock().await;
    let Some(session) = map.get_mut(workspace) else {
        return Ok(Vec::new());
    };
    session.sync_tabs().await;
    let active = session.active().map(|tab| tab.id.clone()).unwrap_or_default();
    let mut out = Vec::new();
    for tab in &session.tabs {
        out.push(ViewTab {
            id: tab.id.clone(),
            title: tab.title().await,
            url: tab.url().await,
            active: tab.id == active,
        });
    }
    Ok(out)
}

/// Opens a URL, or goes `back` / `forward` / `reload`, in the active tab.
pub async fn navigate(sessions: &BrowserSessions, workspace: &str, target: &str) -> Result<()> {
    let mut map = sessions.0.lock().await;
    let session = map
        .get_mut(workspace)
        .ok_or_else(|| anyhow!("the browser is not running"))?;
    match target {
        "back" | "forward" | "reload" => session.history(target).await,
        url => session.navigate(&normalize_url(url)).await,
    }
}

pub async fn switch_tab(sessions: &BrowserSessions, workspace: &str, tab: &str) -> Result<()> {
    let mut map = sessions.0.lock().await;
    let session = map
        .get_mut(workspace)
        .ok_or_else(|| anyhow!("the browser is not running"))?;
    session.switch_tab(tab).await
}

/// `localhost:5173` → `http://localhost:5173`, `example.com` → `https://…`.
fn normalize_url(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains("://") || raw.starts_with("about:") || raw.starts_with("data:") {
        return raw.to_string();
    }
    let local = raw.starts_with("localhost") || raw.starts_with("127.") || raw.starts_with("[::1]");
    format!("{}://{raw}", if local { "http" } else { "https" })
}

/// What the user pointed at, described for the agent.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedElement {
    /// `button "Sign in"` style summary.
    pub label: String,
    /// A CSS selector that finds it again.
    pub selector: String,
    pub text: String,
    pub url: String,
    /// Box in page CSS pixels: x, y, width, height.
    pub rect: [f64; 4],
}

const PICK_JS: &str = r#"(function (x, y) {
  const el = document.elementFromPoint(x, y);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  const role = el.getAttribute('role') || el.tagName.toLowerCase();
  const name = (el.getAttribute('aria-label') || el.getAttribute('alt') || el.getAttribute('title') ||
    el.getAttribute('placeholder') || (el.innerText || el.value || '')).trim().replace(/\s+/g, ' ').slice(0, 120);
  const part = (node) => {
    if (node.id) return '#' + CSS.escape(node.id);
    let s = node.tagName.toLowerCase();
    const classes = [...node.classList].filter((c) => !/^(css-|sc-|_)/.test(c)).slice(0, 2);
    if (classes.length) s += '.' + classes.map((c) => CSS.escape(c)).join('.');
    const parent = node.parentElement;
    if (parent) {
      const same = [...parent.children].filter((c) => c.tagName === node.tagName);
      if (same.length > 1) s += ':nth-of-type(' + (same.indexOf(node) + 1) + ')';
    }
    return s;
  };
  const parts = [];
  for (let node = el; node && node.nodeType === 1 && parts.length < 5; node = node.parentElement) {
    parts.unshift(part(node));
    if (node.id) break;
  }
  return {
    label: name ? role + ' "' + name + '"' : role,
    selector: parts.join(' > '),
    text: (el.innerText || '').trim().slice(0, 500),
    url: location.href,
    rect: [r.x, r.y, r.width, r.height],
  };
})"#;

/// The element under (x, y) in page CSS pixels.
pub async fn element_at(
    sessions: &BrowserSessions,
    workspace: &str,
    x: f64,
    y: f64,
) -> Result<Option<PickedElement>> {
    let (_, page) = active_page(sessions, workspace).await?;
    let value = cdp::eval(&page, &format!("{PICK_JS}({x}, {y})")).await?;
    if value.is_null() {
        return Ok(None);
    }
    let field = |name: &str| value[name].as_str().unwrap_or_default().to_string();
    let rect = value["rect"].as_array().cloned().unwrap_or_default();
    let number = |index: usize| rect.get(index).and_then(Value::as_f64).unwrap_or(0.0);
    Ok(Some(PickedElement {
        label: field("label"),
        selector: field("selector"),
        text: field("text"),
        url: field("url"),
        rect: [number(0), number(1), number(2), number(3)],
    }))
}

/// A PNG of the page region around `rect` (CSS pixels), with some margin.
pub async fn capture_region(
    sessions: &BrowserSessions,
    workspace: &str,
    rect: [f64; 4],
) -> Result<Vec<u8>> {
    const MARGIN: f64 = 16.0;
    let map = sessions.0.lock().await;
    let session = map
        .get(workspace)
        .ok_or_else(|| anyhow!("the browser is not running"))?;
    let (width, height) = session.viewport();
    let x0 = (rect[0] - MARGIN).max(0.0);
    let y0 = (rect[1] - MARGIN).max(0.0);
    let x1 = (rect[0] + rect[2] + MARGIN).min(width as f64);
    let y1 = (rect[1] + rect[3] + MARGIN).min(height as f64);
    if x1 <= x0 || y1 <= y0 {
        return session.screenshot(false, None).await;
    }
    session.screenshot(false, Some([x0, y0, x1, y1])).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_typed_addresses() {
        assert_eq!(normalize_url("localhost:5173"), "http://localhost:5173");
        assert_eq!(normalize_url("127.0.0.1:3000/x"), "http://127.0.0.1:3000/x");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("http://a.dev"), "http://a.dev");
    }

    #[test]
    fn reads_input_from_the_panel() {
        let click: ViewInput = serde_json::from_value(json!({
            "kind": "mouse", "action": "mousePressed", "x": 10.5, "y": 20.0, "button": "left", "clickCount": 1
        }))
        .unwrap();
        assert!(matches!(click, ViewInput::Mouse { click_count: Some(1), .. }));
        let key: ViewInput = serde_json::from_value(json!({
            "kind": "key", "action": "keyDown", "key": "a", "code": "KeyA", "text": "a"
        }))
        .unwrap();
        assert!(matches!(key, ViewInput::Key { text: Some(_), .. }));
    }

    /// Streams a real page, clicks a button through the view and picks it.
    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn streams_and_takes_input() {
        let sessions = BrowserSessions::new();
        let workspace = "view-test";
        ensure_session(&sessions, workspace).await.unwrap();
        navigate(
            &sessions,
            workspace,
            "data:text/html,<button id=go style='position:absolute;left:40px;top:40px;width:120px;height:40px' onclick=\"this.textContent='clicked'\">Go</button>",
        )
        .await
        .unwrap();
        let (stop_tx, stop_rx) = watch::channel(false);
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let streaming = tokio::spawn(stream(sessions.clone(), workspace.into(), stop_rx, move |frame| {
            let _ = frame_tx.send(frame);
        }));
        let first = tokio::time::timeout(Duration::from_secs(10), frame_rx.recv())
            .await
            .expect("a frame arrives")
            .unwrap();
        assert!(first.width > 0.0 && !first.data.is_empty());

        for action in ["mousePressed", "mouseReleased"] {
            send_input(
                &sessions,
                workspace,
                serde_json::from_value(json!({
                    "kind": "mouse", "action": action, "x": 100, "y": 60, "button": "left", "clickCount": 1
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        }
        let picked = element_at(&sessions, workspace, 100.0, 60.0).await.unwrap().unwrap();
        assert_eq!(picked.label, "button \"clicked\"");
        assert_eq!(picked.selector, "#go");
        let png = capture_region(&sessions, workspace, picked.rect).await.unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));

        stop_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), streaming).await.unwrap().unwrap();
    }
}
