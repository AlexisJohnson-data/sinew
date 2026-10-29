//! A Chrome instance per workspace, its tabs, and the per-tab state the
//! tools rely on (references, console and network logs).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::EventJavascriptDialogOpening;
use chromiumoxide::Page;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::cdp;
use crate::observe::{self, TabLog};
use crate::snapshot::RefMap;

/// The viewport every tab is given. Screenshots are taken at this size, so
/// the coordinates a model reads off a screenshot are the ones it clicks.
pub const VIEWPORT: (u32, u32) = (1280, 800);
const NAVIGATION_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest wait for a page to settle after an action.
pub const SETTLE_MAX: Duration = Duration::from_secs(5);

pub struct Tab {
    /// Stable id shown to the agent (`tab_1`, `tab_2`…), never reused.
    pub id: String,
    pub page: Page,
    pub refs: RefMap,
    pub log: Arc<TabLog>,
    target: String,
}

impl Tab {
    pub async fn url(&self) -> String {
        cdp::eval(&self.page, "location.href")
            .await
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    pub async fn title(&self) -> String {
        cdp::eval(&self.page, "document.title")
            .await
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Waits for the page to settle after an action.
    pub async fn settle(&self) -> bool {
        observe::settle(&self.page, &self.log, SETTLE_MAX).await
    }
}

pub struct BrowserSession {
    browser: Browser,
    pub tabs: Vec<Tab>,
    active: usize,
    next_tab: u32,
    viewport: (u32, u32),
    pub headless: bool,
}

impl BrowserSession {
    pub async fn launch(headless: bool) -> Result<Self> {
        let executable = find_browser_executable()
            .context("No Chrome or Edge found. Install Google Chrome or Microsoft Edge.")?;
        let user_data_dir = std::env::temp_dir().join("sinew-browser-profile");
        let mut builder = BrowserConfig::builder()
            .chrome_executable(executable)
            .user_data_dir(&user_data_dir)
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-infobars")
            // Room for the 1280×800 viewport plus Chrome's toolbar.
            .arg("--window-size=1300,960");
        // chromiumoxide injects `--headless` unless told otherwise.
        builder = if headless {
            builder.arg("--headless=new")
        } else {
            builder.with_head()
        };
        let config = builder.build().map_err(|err| anyhow!("{err}"))?;
        let (browser, mut handler) = Browser::launch(config).await?;
        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if let Err(err) = event {
                    // Events newer than the bundled protocol fail to decode;
                    // the connection itself stays up.
                    tracing::debug!("browser handler: {err}");
                }
            }
        });

        let page = browser.new_page("about:blank").await?;
        // Chrome's own start-up tab is noise next to ours.
        if let Ok(pages) = browser.pages().await {
            for other in pages {
                if other.target_id() != page.target_id() {
                    let _ = other.close().await;
                }
            }
        }
        let mut session = Self {
            browser,
            tabs: Vec::new(),
            active: 0,
            next_tab: 0,
            viewport: VIEWPORT,
            headless,
        };
        session.adopt(page).await;
        Ok(session)
    }

    /// Registers a page as a tab: viewport, observers, dialog handling.
    async fn adopt(&mut self, page: Page) -> usize {
        self.next_tab += 1;
        let log = Arc::new(TabLog::default());
        let _ = apply_viewport(&page, self.viewport).await;
        observe::attach(&page, Arc::clone(&log)).await;
        watch_dialogs(&page, Arc::clone(&log)).await;
        self.tabs.push(Tab {
            id: format!("tab_{}", self.next_tab),
            target: page.target_id().as_ref().to_string(),
            page,
            refs: RefMap::default(),
            log,
        });
        self.tabs.len() - 1
    }

    /// Picks up tabs the page opened (`target=_blank`, `window.open`) and
    /// drops closed ones. A tab opened since the last call becomes active.
    /// Returns what changed, for the tool result.
    pub async fn sync_tabs(&mut self) -> Vec<String> {
        let mut changes = Vec::new();
        let Ok(pages) = self.browser.pages().await else {
            return changes;
        };
        let live: Vec<String> = pages
            .iter()
            .map(|page| page.target_id().as_ref().to_string())
            .collect();
        let active_target = self.tabs.get(self.active).map(|tab| tab.target.clone());
        let before = self.tabs.len();
        self.tabs.retain(|tab| live.contains(&tab.target));
        if self.tabs.len() < before {
            changes.push(format!("{} tab(s) closed", before - self.tabs.len()));
        }
        for page in pages {
            let target = page.target_id().as_ref().to_string();
            if self.tabs.iter().any(|tab| tab.target == target) {
                continue;
            }
            let index = self.adopt(page).await;
            self.active = index;
            let tab = &self.tabs[index];
            let _ = tab.settle().await;
            changes.push(format!("new tab {} opened: {} (now active)", tab.id, tab.url().await));
        }
        if !changes.iter().any(|change| change.starts_with("new tab")) {
            self.active = active_target
                .and_then(|target| self.tabs.iter().position(|tab| tab.target == target))
                .unwrap_or(0);
        }
        changes
    }

    pub fn active(&self) -> Result<&Tab> {
        self.tabs
            .get(self.active)
            .ok_or_else(|| anyhow!("the browser has no open tab; call browser_navigate"))
    }

    pub fn active_mut(&mut self) -> Result<&mut Tab> {
        self.tabs
            .get_mut(self.active)
            .ok_or_else(|| anyhow!("the browser has no open tab; call browser_navigate"))
    }

    /// One line per tab, the active one marked.
    pub async fn tab_list(&self) -> String {
        let mut out = String::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            let marker = if index == self.active { " (active)" } else { "" };
            out.push_str(&format!(
                "{}{marker}: {} — {}\n",
                tab.id,
                tab.title().await,
                tab.url().await
            ));
        }
        out
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub async fn new_tab(&mut self, url: Option<&str>) -> Result<String> {
        let page = self.browser.new_page("about:blank").await?;
        let index = self.adopt(page).await;
        self.active = index;
        if let Some(url) = url {
            self.navigate(url).await?;
        }
        Ok(self.tabs[index].id.clone())
    }

    pub async fn switch_tab(&mut self, id: &str) -> Result<()> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| anyhow!("no tab {id}; call browser_tabs with action \"list\""))?;
        self.active = index;
        let _ = self.tabs[index].page.bring_to_front().await;
        Ok(())
    }

    pub async fn close_tab(&mut self, id: &str) -> Result<()> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or_else(|| anyhow!("no tab {id}"))?;
        if self.tabs.len() == 1 {
            bail!("this is the last tab; use browser_close to close the browser");
        }
        let tab = self.tabs.remove(index);
        let _ = tab.page.close().await;
        if self.active >= self.tabs.len() || self.active == index {
            self.active = self.tabs.len().saturating_sub(1);
        } else if self.active > index {
            self.active -= 1;
        }
        let _ = self.tabs[self.active].page.bring_to_front().await;
        Ok(())
    }

    /// Opens `url` in the active tab and waits for it to load and settle.
    pub async fn navigate(&mut self, url: &str) -> Result<()> {
        let tab = self.active()?;
        match tokio::time::timeout(NAVIGATION_TIMEOUT, tab.page.goto(url)).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => bail!("navigation to {url} failed: {err}"),
            Err(_) => bail!("navigation to {url} timed out after {}s", NAVIGATION_TIMEOUT.as_secs()),
        }
        let _ = tab.settle().await;
        Ok(())
    }

    /// `back`, `forward` or `reload` in the active tab.
    pub async fn history(&mut self, action: &str) -> Result<()> {
        let tab = self.active()?;
        match action {
            "reload" => {
                cdp::call(&tab.page, "Page.reload", json!({})).await?;
            }
            "back" | "forward" => {
                let history = cdp::call(&tab.page, "Page.getNavigationHistory", json!({})).await?;
                let current = history["currentIndex"].as_i64().unwrap_or(0);
                let target = if action == "back" { current - 1 } else { current + 1 };
                let entry = history["entries"]
                    .as_array()
                    .and_then(|entries| entries.get(usize::try_from(target).ok()?))
                    .ok_or_else(|| anyhow!("there is no page to go {action} to"))?;
                cdp::call(
                    &tab.page,
                    "Page.navigateToHistoryEntry",
                    json!({ "entryId": entry["id"] }),
                )
                .await?;
            }
            other => bail!("unknown history action {other}"),
        }
        // Give the new document time to start before waiting on it.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let _ = tab.settle().await;
        Ok(())
    }

    /// A PNG of the viewport, of the whole page, or of a region scaled up.
    pub async fn screenshot(&self, full_page: bool, region: Option<[f64; 4]>) -> Result<Vec<u8>> {
        let tab = self.active()?;
        let mut params = json!({ "format": "png" });
        if let Some([x0, y0, x1, y1]) = region {
            let (width, height) = ((x1 - x0).abs(), (y1 - y0).abs());
            if width < 4.0 || height < 4.0 {
                bail!("the zoom region is too small");
            }
            // Fill roughly the viewport width, at most 4×.
            let scale = (f64::from(self.viewport.0) / width).clamp(1.0, 4.0);
            let scroll = cdp::eval(&tab.page, "[scrollX, scrollY]").await.unwrap_or(json!([0, 0]));
            params["clip"] = json!({
                "x": x0.min(x1) + scroll[0].as_f64().unwrap_or(0.0),
                "y": y0.min(y1) + scroll[1].as_f64().unwrap_or(0.0),
                "width": width,
                "height": height,
                "scale": scale,
            });
        } else if full_page {
            let size = cdp::eval(
                &tab.page,
                "[document.documentElement.scrollWidth, document.documentElement.scrollHeight]",
            )
            .await?;
            params["captureBeyondViewport"] = json!(true);
            params["clip"] = json!({
                "x": 0,
                "y": 0,
                "width": size[0].as_f64().unwrap_or(f64::from(self.viewport.0)),
                "height": size[1].as_f64().unwrap_or(f64::from(self.viewport.1)),
                "scale": 1,
            });
        }
        let result = cdp::call(&tab.page, "Page.captureScreenshot", params).await?;
        let bytes = BASE64
            .decode(result["data"].as_str().unwrap_or_default())
            .context("decode screenshot")?;
        Ok(tokio::task::spawn_blocking(move || downscale_if_oversized(bytes)).await?)
    }

    pub fn viewport(&self) -> (u32, u32) {
        self.viewport
    }

    /// Changes the viewport of every tab (responsive checks).
    pub async fn set_viewport(&mut self, width: u32, height: u32) -> Result<()> {
        self.viewport = (width.clamp(240, 3840), height.clamp(240, 2400));
        for tab in &self.tabs {
            apply_viewport(&tab.page, self.viewport).await?;
        }
        Ok(())
    }

    pub async fn eval(&self, js: &str) -> Result<Value> {
        cdp::eval(&self.active()?.page, js).await
    }

    pub async fn pdf(&self) -> Result<Vec<u8>> {
        let result = cdp::call(&self.active()?.page, "Page.printToPDF", json!({})).await?;
        BASE64
            .decode(result["data"].as_str().unwrap_or_default())
            .context("decode PDF")
    }
}

async fn apply_viewport(page: &Page, (width, height): (u32, u32)) -> Result<()> {
    cdp::call(
        page,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": width, "height": height, "deviceScaleFactor": 1, "mobile": false }),
    )
    .await?;
    Ok(())
}

/// `alert`, `confirm`, `prompt` and `beforeunload` block the page until
/// answered; accept them and log what they said.
async fn watch_dialogs(page: &Page, log: Arc<TabLog>) {
    let Ok(mut events) = page.event_listener::<EventJavascriptDialogOpening>().await else {
        return;
    };
    let page = page.clone();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let _ = cdp::call(&page, "Page.handleJavaScriptDialog", json!({ "accept": true })).await;
            log.note(format!(
                "{:?} dialog auto-accepted: {}",
                event.r#type, event.message
            ))
            .await;
        }
    });
}

pub struct BrowserSessions(pub Arc<Mutex<HashMap<String, BrowserSession>>>);

impl BrowserSessions {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }
}

impl Default for BrowserSessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for BrowserSessions {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

fn find_browser_executable() -> Option<std::path::PathBuf> {
    let mut owned_candidates: Vec<String> = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            owned_candidates.push(format!(r"{local}\Google\Chrome\Application\chrome.exe"));
            owned_candidates.push(format!(r"{local}\Microsoft\Edge\Application\msedge.exe"));
        }
    }
    let static_candidates: &[&str] = &[
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\Google\Chrome Beta\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome Beta\Application\chrome.exe",
        r"C:\Program Files\Google\Chrome Dev\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome Dev\Application\chrome.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge Beta\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge Beta\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge Dev\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge Dev\Application\msedge.exe",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
    ];
    let candidates: Vec<&str> = owned_candidates
        .iter()
        .map(|s| s.as_str())
        .chain(static_candidates.iter().copied())
        .collect();
    let candidates = candidates.as_slice();
    for candidate in candidates {
        let path = std::path::Path::new(candidate);
        if path.exists() {
            return Some(path.to_path_buf());
        }
        if !path.is_absolute() {
            if let Ok(found) = which_like(candidate) {
                return Some(found);
            }
        }
    }
    None
}

fn which_like(name: &str) -> Result<std::path::PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = std::process::Command::new(cmd).arg(name).output()?;
    if output.status.success() {
        let path = std::str::from_utf8(&output.stdout)?
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .trim();
        if !path.is_empty() {
            return Ok(std::path::PathBuf::from(path));
        }
    }
    bail!("not found")
}

/// Longest side we allow a screenshot to be. Kept under the 8000 px ceiling
/// that LLM providers (Anthropic, etc.) enforce on uploaded images.
const MAX_SCREENSHOT_DIMENSION: u32 = 7800;

/// Downscale a PNG screenshot so neither side exceeds `MAX_SCREENSHOT_DIMENSION`,
/// preserving aspect ratio. Returns the original bytes unchanged if it already
/// fits or if decode/encode fails (fail-open — a screenshot beats no screenshot).
fn downscale_if_oversized(png: Vec<u8>) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(&png) else {
        return png;
    };
    if img.width() <= MAX_SCREENSHOT_DIMENSION && img.height() <= MAX_SCREENSHOT_DIMENSION {
        return png;
    }
    let resized = img.resize(
        MAX_SCREENSHOT_DIMENSION,
        MAX_SCREENSHOT_DIMENSION,
        image::imageops::FilterType::Triangle,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    match resized.write_to(&mut out, image::ImageFormat::Png) {
        Ok(()) => out.into_inner(),
        Err(_) => png,
    }
}
