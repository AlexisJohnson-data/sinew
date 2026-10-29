//! The agent's browser tools, modelled on Claude's browser toolset: read
//! the page as an accessibility tree with references, act on a reference
//! (or a screenshot coordinate as a fallback) with real mouse and keyboard
//! input, then read what changed. Plus the debugging views a coding agent
//! needs: console, network and performance.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use sinew_browser::input::{self, Button, Target};
use sinew_browser::snapshot::{self, Filter};
use sinew_browser::{
    gif_to_base64, observe, BrowserSession, BrowserSessions, ConsoleEntry, RecordingHandle,
    VIEWPORT,
};
use sinew_core::ToolDescriptor;
use tokio::sync::Mutex;

use crate::tool_names;
use crate::tool_run::{ToolRunImage, ToolRunResult};

const NAVIGATE_SNAPSHOT_CHARS: usize = 8_000;
const EVAL_OUTPUT_LIMIT: usize = 16 * 1024;
const TEXT_DEFAULT_CHARS: usize = 20_000;
const CONSOLE_DEFAULT_LIMIT: usize = 50;
const NETWORK_DEFAULT_LIMIT: usize = 60;
const BODY_LIMIT: usize = 4_000;
const WAIT_DEFAULT_SECS: f64 = 10.0;
const WAIT_MAX_SECS: f64 = 30.0;
/// Header names whose values never reach the model.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-xsrf-token",
];

pub struct BrowserTools {
    sessions: BrowserSessions,
    workspace_id: String,
    recording: Arc<Mutex<Option<RecordingHandle>>>,
}

impl BrowserTools {
    pub fn new(workspace_id: String, sessions: BrowserSessions) -> Self {
        Self {
            sessions,
            workspace_id,
            recording: Arc::new(Mutex::new(None)),
        }
    }

    /// The tools offered to the model. `scripts` adds `browser_eval` and
    /// `browser_upload`, which a hostile page could talk the agent into
    /// misusing; they are off unless enabled in Settings.
    pub fn all_descriptors(&self, scripts: bool) -> Vec<ToolDescriptor> {
        let target = |what: &str| {
            json!({
                "ref": { "type": "string", "description": format!("Reference of the element {what}, e.g. \"ref_12\", from browser_read_page or browser_find. Preferred.") },
                "coordinate": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, y] in viewport pixels, read off a browser_screenshot. Only when the element has no reference (canvas, video, cross-origin frame)." }
            })
        };
        let with = |base: Value, extra: Value| {
            let mut properties = base.as_object().cloned().unwrap_or_default();
            if let Some(extra) = extra.as_object() {
                properties.extend(extra.clone());
            }
            Value::Object(properties)
        };
        let object = |properties: Value, required: &[&str]| {
            json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
        };

        let mut tools = vec![
            descriptor(
                tool_names::BROWSER_NAVIGATE,
                "Open a URL in the browser (launching Chrome if needed), or go \"back\", \"forward\" or \"reload\". Waits for the page to load and settle, then returns its interactive elements with references. Only http(s) URLs; a bare host like localhost:5173 gets http://.",
                object(
                    json!({
                        "url": { "type": "string", "description": "URL, or \"back\", \"forward\", \"reload\"" },
                        "headless": { "type": "boolean", "description": "Launch without a visible window (only when the browser is not open yet). Default false." }
                    }),
                    &["url"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_READ_PAGE,
                "Read the page as an accessibility tree. Every element you can act on carries a reference like [ref_12]; pass it to browser_click, browser_type, browser_form_input… References stay valid until the page changes; re-read after a navigation or a big update. Use filter \"interactive\" for just the controls, or `ref` to read one part of the page.",
                object(
                    json!({
                        "filter": { "type": "string", "enum": ["all", "interactive"], "description": "\"all\" (default): controls, headings, landmarks and text. \"interactive\": controls only." },
                        "ref": { "type": "string", "description": "Only read the subtree under this reference." },
                        "max_chars": { "type": "integer", "description": "Output limit, default 30000, max 50000." }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_FIND,
                "Find elements by what they say or are, e.g. \"sign in button\", \"email field\", \"Pricing link\". Returns up to 20 matches with references, best first.",
                object(json!({ "query": { "type": "string" } }), &["query"]),
            ),
            descriptor(
                tool_names::BROWSER_GET_TEXT,
                "The readable text of the page (its <main> or <article> when there is one). For reading content, cheaper than browser_read_page.",
                object(json!({ "max_chars": { "type": "integer", "description": "Default 20000." } }), &[]),
            ),
            descriptor(
                tool_names::BROWSER_SCREENSHOT,
                &format!("Capture the viewport ({}×{} px) as an image. Coordinates in it are the ones browser_click takes. `zoom` captures a region [x0, y0, x1, y1] enlarged; `full_page` the whole scrollable page. Prefer browser_read_page to understand the page; use screenshots to check layout and visuals.", VIEWPORT.0, VIEWPORT.1),
                object(
                    json!({
                        "zoom": { "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4, "description": "[x0, y0, x1, y1] in viewport pixels" },
                        "full_page": { "type": "boolean" }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_CLICK,
                "Click an element with the real mouse: it is scrolled into view and checked to be visible, enabled and not covered first. Waits for the page to settle and reports what changed (navigation, new tab, console errors).",
                object(
                    with(
                        target("to click"),
                        json!({
                            "button": { "type": "string", "enum": ["left", "right", "middle"] },
                            "clicks": { "type": "integer", "enum": [1, 2, 3], "description": "2 for a double click, 3 for a triple click." },
                            "modifiers": { "type": "array", "items": { "type": "string", "enum": ["ctrl", "shift", "alt", "meta"] } }
                        }),
                    ),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_TYPE,
                "Type text. With `ref`, the element is clicked to focus it first; without, the text goes to the focused element. `clear` empties the field first, `submit` presses Enter after.",
                object(
                    json!({
                        "text": { "type": "string" },
                        "ref": { "type": "string" },
                        "clear": { "type": "boolean" },
                        "submit": { "type": "boolean" }
                    }),
                    &["text"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_KEY,
                "Press keys: one key (\"Enter\", \"Escape\", \"ArrowDown\"), a chord (\"ctrl+a\", \"shift+Tab\"), or a space-separated sequence (\"Tab Tab Enter\").",
                object(json!({ "keys": { "type": "string" } }), &["keys"]),
            ),
            descriptor(
                tool_names::BROWSER_FORM_INPUT,
                "Set a form control directly: a <select> (option value or label), a checkbox, radio or switch (true / false), or an input like date, range or color. Fires the events frameworks listen to.",
                object(
                    json!({
                        "ref": { "type": "string" },
                        "value": { "description": "String, boolean, number, or an array of option values for a multi-select." }
                    }),
                    &["ref", "value"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_HOVER,
                "Move the mouse over an element (menus, tooltips).",
                object(target("to hover"), &[]),
            ),
            descriptor(
                tool_names::BROWSER_SCROLL,
                "Scroll with the mouse wheel over an element or point (default: the middle of the page), or with only `ref` and no direction, scroll that element into view.",
                object(
                    with(
                        target("to scroll over or into view"),
                        json!({
                            "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                            "amount": { "type": "integer", "description": "Wheel ticks, default 3 (about 300 px)." }
                        }),
                    ),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_DRAG,
                "Drag with the mouse from one element or point to another (sliders, sortable lists, canvas). HTML5 drag-and-drop may not react to synthetic mouse moves.",
                object(
                    json!({
                        "from_ref": { "type": "string" },
                        "from_coordinate": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 },
                        "to_ref": { "type": "string" },
                        "to_coordinate": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_WAIT,
                "Wait for text to appear on the page, for a referenced element to go away, or for a number of seconds (max 30). Actions already wait for the page to settle; use this for slow work like a build or an upload.",
                object(
                    json!({
                        "text": { "type": "string", "description": "Wait until the page shows this text." },
                        "ref_gone": { "type": "string", "description": "Wait until this element is removed or hidden." },
                        "seconds": { "type": "number", "description": "How long to wait at most (or exactly, alone). Default 10." }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_TABS,
                "List, open, switch to or close tabs. Tabs the page opens itself (target=_blank, pop-ups) are picked up and reported by the action that opened them.",
                object(
                    json!({
                        "action": { "type": "string", "enum": ["list", "new", "switch", "close"] },
                        "tab_id": { "type": "string", "description": "For switch and close, e.g. \"tab_2\"." },
                        "url": { "type": "string", "description": "For new: the URL to open." }
                    }),
                    &["action"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_CONSOLE,
                "Console messages and uncaught exceptions of the active tab, with stack traces. Frames served from the local dev server are shown as project files (line numbers are those of the served file).",
                object(
                    json!({
                        "level": { "type": "string", "enum": ["all", "warning", "error"], "description": "\"error\": errors and exceptions; \"warning\": also warnings. Default \"all\"." },
                        "limit": { "type": "integer", "description": "Most recent N, default 50." },
                        "clear": { "type": "boolean", "description": "Empty the log after reading." }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_NETWORK,
                "Network requests of the active tab. Without `id`: a list (filter by URL text, or failed_only for errors and 4xx/5xx). With `id`: that request's status, headers (secrets masked), request body and response body.",
                object(
                    json!({
                        "id": { "type": "integer" },
                        "filter": { "type": "string" },
                        "failed_only": { "type": "boolean" },
                        "limit": { "type": "integer", "description": "Default 60." },
                        "clear": { "type": "boolean" }
                    }),
                    &[],
                ),
            ),
            descriptor(
                tool_names::BROWSER_PERFORMANCE,
                "Load timings (TTFB, FCP, load), Web Vitals (LCP with its element, CLS, INP), long tasks, transfer size and the slowest requests. `reload` measures a fresh load.",
                object(json!({ "reload": { "type": "boolean" } }), &[]),
            ),
            descriptor(
                tool_names::BROWSER_RESIZE,
                "Set the viewport size, e.g. 390×844 to check a phone layout. Screenshots and coordinates follow it.",
                object(
                    json!({ "width": { "type": "integer" }, "height": { "type": "integer" } }),
                    &["width", "height"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_RECORD,
                "Record the active tab as an animated GIF: \"start\", do the actions, then \"stop\" to get it.",
                object(
                    json!({
                        "action": { "type": "string", "enum": ["start", "stop"] },
                        "fps": { "type": "integer", "description": "1-10, default 4." },
                        "max_duration_s": { "type": "integer", "description": "Default 30, max 120." }
                    }),
                    &["action"],
                ),
            ),
            descriptor(
                tool_names::BROWSER_CLOSE,
                "Close the browser and all its tabs.",
                object(json!({}), &[]),
            ),
        ];
        if scripts {
            tools.push(descriptor(
                tool_names::BROWSER_EVAL,
                "Run JavaScript in the active tab and return the (JSON) result. Promises are awaited.",
                object(json!({ "expression": { "type": "string" } }), &["expression"]),
            ));
            tools.push(descriptor(
                tool_names::BROWSER_UPLOAD,
                "Put files from the project into an <input type=file>.",
                object(
                    json!({
                        "ref": { "type": "string" },
                        "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths inside the project." }
                    }),
                    &["ref", "paths"],
                ),
            ));
        }
        tools
    }

    pub async fn run(&self, name: &str, input: Value, scripts: bool) -> ToolRunResult {
        if !scripts && (name == tool_names::BROWSER_EVAL || name == tool_names::BROWSER_UPLOAD) {
            return ToolRunResult::err(
                format!("{name} is disabled. The user can enable it in Settings → Tools → Browser."),
                Vec::new(),
            );
        }
        let result = match name {
            n if n == tool_names::BROWSER_NAVIGATE => self.run_navigate(input).await,
            n if n == tool_names::BROWSER_READ_PAGE => self.run_read_page(input).await,
            n if n == tool_names::BROWSER_FIND => self.run_find(input).await,
            n if n == tool_names::BROWSER_GET_TEXT => self.run_get_text(input).await,
            n if n == tool_names::BROWSER_SCREENSHOT => return self.run_screenshot(input).await,
            n if n == tool_names::BROWSER_CLICK => self.run_click(input).await,
            n if n == tool_names::BROWSER_TYPE => self.run_type(input).await,
            n if n == tool_names::BROWSER_KEY => self.run_key(input).await,
            n if n == tool_names::BROWSER_FORM_INPUT => self.run_form_input(input).await,
            n if n == tool_names::BROWSER_HOVER => self.run_hover(input).await,
            n if n == tool_names::BROWSER_SCROLL => self.run_scroll(input).await,
            n if n == tool_names::BROWSER_DRAG => self.run_drag(input).await,
            n if n == tool_names::BROWSER_WAIT => self.run_wait(input).await,
            n if n == tool_names::BROWSER_TABS => self.run_tabs(input).await,
            n if n == tool_names::BROWSER_CONSOLE => self.run_console(input).await,
            n if n == tool_names::BROWSER_NETWORK => self.run_network(input).await,
            n if n == tool_names::BROWSER_PERFORMANCE => self.run_performance(input).await,
            n if n == tool_names::BROWSER_RESIZE => self.run_resize(input).await,
            n if n == tool_names::BROWSER_RECORD => return self.run_record(input).await,
            n if n == tool_names::BROWSER_CLOSE => self.run_close().await,
            n if n == tool_names::BROWSER_EVAL => self.run_eval(input).await,
            n if n == tool_names::BROWSER_UPLOAD => self.run_upload(input).await,
            _ => Err(format!(
                "unknown browser tool: {name}. The browser tools were renamed; see the tool list (browser_navigate, browser_read_page, browser_click…)."
            )),
        };
        match result {
            Ok(text) => ToolRunResult::ok(text, Vec::new()),
            Err(text) => ToolRunResult::err(text, Vec::new()),
        }
    }

    // ── Navigation and reading ──────────────────────────────────────────

    async fn run_navigate(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            url: String,
            #[serde(default)]
            headless: bool,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        if !sessions.contains_key(&self.workspace_id) {
            let session = BrowserSession::launch(input.headless)
                .await
                .map_err(|err| err.to_string())?;
            sessions.insert(self.workspace_id.clone(), session);
        }
        let session = sessions
            .get_mut(&self.workspace_id)
            .ok_or("browser session vanished")?;
        let before = Before::capture(session).await;
        let action = input.url.trim();
        match action {
            "back" | "forward" | "reload" => session.history(action).await,
            _ => {
                let url = normalize_url(action)?;
                session.navigate(&url).await
            }
        }
        .map_err(|err| err.to_string())?;
        let mut report = after(session, &before, &self.workspace_id, Report::Navigation).await;
        let tab = session.active_mut().map_err(|err| err.to_string())?;
        match snapshot::read(&tab.page, &mut tab.refs, Filter::Interactive, None, NAVIGATE_SNAPSHOT_CHARS).await {
            Ok(snap) => {
                report.push_str("\nInteractive elements:\n");
                report.push_str(if snap.text.trim().is_empty() { "(none)\n" } else { &snap.text });
                if snap.truncated {
                    report.push_str("… more: call browser_read_page or browser_find.\n");
                }
            }
            Err(err) => report.push_str(&format!("\n(Could not read the page: {err})")),
        }
        Ok(report)
    }

    async fn run_read_page(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            filter: Option<String>,
            #[serde(rename = "ref")]
            reference: Option<String>,
            max_chars: Option<usize>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let tab = session.active_mut().map_err(|err| err.to_string())?;
        let filter = match input.filter.as_deref() {
            Some("interactive") => Filter::Interactive,
            _ => Filter::All,
        };
        let max_chars = input
            .max_chars
            .unwrap_or(snapshot::DEFAULT_MAX_CHARS)
            .clamp(1_000, snapshot::HARD_MAX_CHARS);
        let snap = snapshot::read(&tab.page, &mut tab.refs, filter, input.reference.as_deref(), max_chars)
            .await
            .map_err(|err| err.to_string())?;
        let mut out = format!("Page: {} — {}\n", tab.title().await, tab.url().await);
        out.push_str(if snap.text.trim().is_empty() { "(nothing readable)\n" } else { &snap.text });
        if snap.truncated {
            out.push_str(&format!(
                "… truncated at {max_chars} characters. Read part of the page with `ref`, use filter \"interactive\", or browser_find.\n"
            ));
        }
        Ok(out)
    }

    async fn run_find(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            query: String,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let tab = session.active_mut().map_err(|err| err.to_string())?;
        let matches = snapshot::find(&tab.page, &mut tab.refs, &input.query)
            .await
            .map_err(|err| err.to_string())?;
        if matches.is_empty() {
            return Ok(format!(
                "No element matches \"{}\". Try other words, browser_read_page, or a screenshot.",
                input.query
            ));
        }
        let mut out = format!("{} match(es) for \"{}\":\n", matches.len(), input.query);
        for entry in matches {
            out.push_str(&format!("{} \"{}\" [{}]", entry.role, entry.name, entry.reference));
            if !entry.value.is_empty() && entry.value != entry.name {
                out.push_str(&format!(" value=\"{}\"", entry.value));
            }
            if let Some(section) = entry.context.last() {
                out.push_str(&format!(" — in \"{section}\""));
            }
            out.push('\n');
        }
        Ok(out)
    }

    async fn run_get_text(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            max_chars: Option<usize>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let tab = session.active().map_err(|err| err.to_string())?;
        let text = snapshot::text(&tab.page, input.max_chars.unwrap_or(TEXT_DEFAULT_CHARS).clamp(500, 100_000))
            .await
            .map_err(|err| err.to_string())?;
        Ok(format!("Page: {} — {}\n\n{text}", tab.title().await, tab.url().await))
    }

    async fn run_screenshot(&self, input: Value) -> ToolRunResult {
        #[derive(Deserialize, Default)]
        struct Input {
            zoom: Option<[f64; 4]>,
            #[serde(default)]
            full_page: bool,
        }
        let input: Input = match parse(input) {
            Ok(input) => input,
            Err(err) => return ToolRunResult::err(err, Vec::new()),
        };
        let mut sessions = self.sessions.0.lock().await;
        let session = match session_mut(&mut sessions, &self.workspace_id) {
            Ok(session) => session,
            Err(err) => return ToolRunResult::err(err, Vec::new()),
        };
        session.sync_tabs().await;
        match session.screenshot(input.full_page, input.zoom).await {
            Ok(bytes) => {
                let (width, height) = png_dimensions(&bytes);
                let path = save_capture(&self.workspace_id, &bytes, "png");
                let kind = if input.zoom.is_some() {
                    "Zoomed region (coordinates stay those of the full viewport)"
                } else if input.full_page {
                    "Full page"
                } else {
                    "Viewport"
                };
                let mut text = format!("{kind}: {width}×{height} px");
                if let Some(path) = &path {
                    text.push_str(&format!(" — saved to {path}"));
                }
                ToolRunResult::ok_with_images(
                    text,
                    vec![ToolRunImage {
                        media_type: "image/png".into(),
                        data: BASE64.encode(&bytes),
                        path,
                    }],
                    Vec::new(),
                )
            }
            Err(err) => ToolRunResult::err(err.to_string(), Vec::new()),
        }
    }

    // ── Actions ─────────────────────────────────────────────────────────

    async fn run_click(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            #[serde(rename = "ref")]
            reference: Option<String>,
            coordinate: Option<[f64; 2]>,
            button: Option<String>,
            clicks: Option<u8>,
            #[serde(default)]
            modifiers: Vec<String>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let target = resolve_target(session, input.reference.as_deref(), input.coordinate).await?;
        let before = Before::capture(session).await;
        let button = match input.button.as_deref() {
            Some("right") => Button::Right,
            Some("middle") => Button::Middle,
            _ => Button::Left,
        };
        let page = &session.active().map_err(|err| err.to_string())?.page;
        let (x, y) = input::point_of(page, target, true).await.map_err(|err| err.to_string())?;
        input::click(page, x, y, button, input.clicks.unwrap_or(1), input::modifier_bits(&input.modifiers))
            .await
            .map_err(|err| err.to_string())?;
        let what = describe_target(input.reference.as_deref(), (x, y));
        Ok(format!("Clicked {what}.\n{}", after(session, &before, &self.workspace_id, Report::Changes).await))
    }

    async fn run_type(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            text: String,
            #[serde(rename = "ref")]
            reference: Option<String>,
            #[serde(default)]
            clear: bool,
            #[serde(default)]
            submit: bool,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let node = match &input.reference {
            Some(reference) => Some(resolve_ref(session, reference).await?),
            None => None,
        };
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        input::type_into(page, node, &input.text, input.clear)
            .await
            .map_err(|err| err.to_string())?;
        if input.submit {
            input::keys(page, "Enter").await.map_err(|err| err.to_string())?;
        }
        let into = input
            .reference
            .as_deref()
            .map(|reference| format!(" into {reference}"))
            .unwrap_or_default();
        Ok(format!(
            "Typed {} character(s){into}{}.\n{}",
            input.text.chars().count(),
            if input.submit { " and pressed Enter" } else { "" },
            after(session, &before, &self.workspace_id, Report::Edits).await
        ))
    }

    async fn run_key(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            keys: String,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        input::keys(page, &input.keys).await.map_err(|err| err.to_string())?;
        Ok(format!(
            "Pressed {}.\n{}",
            input.keys,
            after(session, &before, &self.workspace_id, Report::Edits).await
        ))
    }

    async fn run_form_input(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            #[serde(rename = "ref")]
            reference: String,
            value: Value,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let node = resolve_ref(session, &input.reference).await?;
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        let outcome = input::set_value(page, node, &input.value)
            .await
            .map_err(|err| err.to_string())?;
        Ok(format!(
            "{}: {outcome}.\n{}",
            input.reference,
            after(session, &before, &self.workspace_id, Report::Edits).await
        ))
    }

    async fn run_hover(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            #[serde(rename = "ref")]
            reference: Option<String>,
            coordinate: Option<[f64; 2]>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let target = resolve_target(session, input.reference.as_deref(), input.coordinate).await?;
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        let (x, y) = input::point_of(page, target, false).await.map_err(|err| err.to_string())?;
        input::hover(page, x, y).await.map_err(|err| err.to_string())?;
        Ok(format!(
            "Hovering {}.\n{}",
            describe_target(input.reference.as_deref(), (x, y)),
            after(session, &before, &self.workspace_id, Report::Changes).await
        ))
    }

    async fn run_scroll(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            #[serde(rename = "ref")]
            reference: Option<String>,
            coordinate: Option<[f64; 2]>,
            direction: Option<String>,
            amount: Option<u32>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let (width, height) = session.viewport();
        let target = match (&input.reference, input.coordinate) {
            (None, None) => Target::Point(f64::from(width) / 2.0, f64::from(height) / 2.0),
            (reference, coordinate) => resolve_target(session, reference.as_deref(), coordinate).await?,
        };
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        let Some(direction) = input.direction.as_deref() else {
            let Target::Node(_) = target else {
                return Err("give a direction, or a ref to scroll into view".into());
            };
            input::point_of(page, target, false).await.map_err(|err| err.to_string())?;
            return Ok(format!(
                "Scrolled {} into view.\n{}",
                input.reference.unwrap_or_default(),
                after(session, &before, &self.workspace_id, Report::Changes).await
            ));
        };
        let (x, y) = input::point_of(page, target, false).await.map_err(|err| err.to_string())?;
        let ticks = f64::from(input.amount.unwrap_or(3).clamp(1, 30)) * 100.0;
        let (dx, dy) = match direction {
            "up" => (0.0, -ticks),
            "left" => (-ticks, 0.0),
            "right" => (ticks, 0.0),
            _ => (0.0, ticks),
        };
        input::wheel(page, x, y, dx, dy).await.map_err(|err| err.to_string())?;
        let position = session
            .eval("[Math.round(scrollX), Math.round(scrollY), document.documentElement.scrollHeight - innerHeight]")
            .await
            .unwrap_or(Value::Null);
        Ok(format!(
            "Scrolled {direction}. Page scroll: x={} y={} (max y {}).\n{}",
            position[0], position[1], position[2],
            after(session, &before, &self.workspace_id, Report::Changes).await
        ))
    }

    async fn run_drag(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            from_ref: Option<String>,
            from_coordinate: Option<[f64; 2]>,
            to_ref: Option<String>,
            to_coordinate: Option<[f64; 2]>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session.sync_tabs().await;
        let from = resolve_target(session, input.from_ref.as_deref(), input.from_coordinate).await?;
        let to = resolve_target(session, input.to_ref.as_deref(), input.to_coordinate).await?;
        let before = Before::capture(session).await;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        let from = input::point_of(page, from, true).await.map_err(|err| err.to_string())?;
        let to = input::point_of(page, to, false).await.map_err(|err| err.to_string())?;
        input::drag(page, from, to).await.map_err(|err| err.to_string())?;
        Ok(format!(
            "Dragged from ({:.0}, {:.0}) to ({:.0}, {:.0}).\n{}",
            from.0, from.1, to.0, to.1,
            after(session, &before, &self.workspace_id, Report::Changes).await
        ))
    }

    async fn run_wait(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            text: Option<String>,
            ref_gone: Option<String>,
            seconds: Option<f64>,
        }
        let input: Input = parse(input)?;
        let limit = input.seconds.unwrap_or(WAIT_DEFAULT_SECS).clamp(0.0, WAIT_MAX_SECS);
        if input.text.is_none() && input.ref_gone.is_none() {
            tokio::time::sleep(std::time::Duration::from_secs_f64(limit)).await;
            return Ok(format!("Waited {limit}s."));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(limit);
        loop {
            {
                let mut sessions = self.sessions.0.lock().await;
                let session = session_mut(&mut sessions, &self.workspace_id)?;
                let tab = session.active().map_err(|err| err.to_string())?;
                if let Some(text) = &input.text {
                    let found = sinew_browser_eval(
                        session,
                        &format!("(document.body && document.body.innerText || '').includes({})", json!(text)),
                    )
                    .await;
                    if found == Value::Bool(true) {
                        return Ok(format!("\"{text}\" is on the page."));
                    }
                }
                if let Some(reference) = &input.ref_gone {
                    let gone = match tab.refs.resolve(&tab.page, reference).await {
                        Err(_) => true,
                        Ok(node) => input::element_point(&tab.page, node, false).await.is_err(),
                    };
                    if gone {
                        return Ok(format!("{reference} is gone."));
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("Still waiting after {limit}s; the condition did not happen."));
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    async fn run_tabs(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            action: String,
            tab_id: Option<String>,
            url: Option<String>,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        let changes = session.sync_tabs().await;
        let need_id = || input.tab_id.clone().ok_or("tab_id is required");
        match input.action.as_str() {
            "list" => {}
            "new" => {
                let url = input.url.as_deref().map(normalize_url).transpose()?;
                session.new_tab(url.as_deref()).await.map_err(|err| err.to_string())?;
            }
            "switch" => session.switch_tab(&need_id()?).await.map_err(|err| err.to_string())?,
            "close" => session.close_tab(&need_id()?).await.map_err(|err| err.to_string())?,
            other => return Err(format!("unknown action {other}")),
        }
        let mut out = String::new();
        for change in changes {
            out.push_str(&format!("{change}\n"));
        }
        out.push_str(&session.tab_list().await);
        Ok(out)
    }

    // ── Debugging ───────────────────────────────────────────────────────

    async fn run_console(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            level: Option<String>,
            limit: Option<usize>,
            #[serde(default)]
            clear: bool,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        let tab = session.active().map_err(|err| err.to_string())?;
        let entries = tab.log.console_entries(input.clear).await;
        let keep = |entry: &ConsoleEntry| match input.level.as_deref() {
            Some("error") => matches!(entry.level.as_str(), "error" | "exception"),
            Some("warning") => matches!(entry.level.as_str(), "error" | "exception" | "warning"),
            _ => true,
        };
        let matching: Vec<&ConsoleEntry> = entries.iter().filter(|entry| keep(entry)).collect();
        if matching.is_empty() {
            return Ok("No console messages.".into());
        }
        let limit = input.limit.unwrap_or(CONSOLE_DEFAULT_LIMIT).clamp(1, 500);
        let skipped = matching.len().saturating_sub(limit);
        let mut out = String::new();
        if skipped > 0 {
            out.push_str(&format!("({skipped} older message(s) not shown)\n"));
        }
        for entry in matching.into_iter().skip(skipped) {
            out.push_str(&format_console(entry, &self.workspace_id));
        }
        Ok(out)
    }

    async fn run_network(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            id: Option<u64>,
            filter: Option<String>,
            #[serde(default)]
            failed_only: bool,
            limit: Option<usize>,
            #[serde(default)]
            clear: bool,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        let tab = session.active().map_err(|err| err.to_string())?;
        if let Some(id) = input.id {
            let entry = tab
                .log
                .network_entry(id)
                .await
                .ok_or_else(|| format!("no request #{id} (the log keeps the last 500)"))?;
            let mut out = format!(
                "#{} {} {}\nStatus: {}\nType: {} · {}\n",
                entry.id,
                entry.method,
                entry.url,
                status_text(&entry),
                entry.resource_type,
                entry.mime_type,
            );
            if let Some(ms) = entry.duration_ms {
                out.push_str(&format!("Time: {ms} ms"));
                if let Some(size) = entry.size_bytes {
                    out.push_str(&format!(" · {} KB", size / 1024));
                }
                out.push('\n');
            }
            out.push_str(&format!("\nRequest headers:\n{}", headers(&entry.request_headers)));
            if let Some(body) = entry.post_data.as_deref().filter(|body| !body.is_empty()) {
                out.push_str(&format!("\nRequest body:\n{}\n", clip(body, BODY_LIMIT)));
            }
            out.push_str(&format!("\nResponse headers:\n{}", headers(&entry.response_headers)));
            if entry.status.is_some() && entry.failed.is_none() {
                match observe::response_body(&tab.page, &entry).await {
                    Ok(body) => out.push_str(&format!("\nResponse body:\n{}\n", clip(&body, BODY_LIMIT))),
                    Err(reason) => out.push_str(&format!("\nResponse body: {reason}\n")),
                }
            }
            return Ok(out);
        }
        let entries = tab.log.network_entries(input.clear).await;
        let filter = input.filter.as_deref().map(str::to_lowercase);
        let matching: Vec<_> = entries
            .iter()
            .filter(|entry| {
                filter
                    .as_deref()
                    .map_or(true, |filter| entry.url.to_lowercase().contains(filter))
            })
            .filter(|entry| {
                !input.failed_only
                    || entry.failed.is_some()
                    || entry.status.is_some_and(|status| status >= 400)
            })
            .collect();
        if matching.is_empty() {
            return Ok("No matching requests.".into());
        }
        let limit = input.limit.unwrap_or(NETWORK_DEFAULT_LIMIT).clamp(1, 500);
        let skipped = matching.len().saturating_sub(limit);
        let mut out = String::new();
        if skipped > 0 {
            out.push_str(&format!("({skipped} older request(s) not shown)\n"));
        }
        for entry in matching.into_iter().skip(skipped) {
            let time = entry
                .duration_ms
                .map(|ms| format!(" {ms}ms"))
                .unwrap_or_else(|| " (pending)".into());
            out.push_str(&format!(
                "#{} {} {} {} ({}{time})\n",
                entry.id,
                entry.method,
                status_text(entry),
                clip(&entry.url, 160),
                entry.resource_type.to_lowercase(),
            ));
        }
        out.push_str("Details: browser_network with `id`.\n");
        Ok(out)
    }

    async fn run_performance(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize, Default)]
        struct Input {
            #[serde(default)]
            reload: bool,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        if input.reload {
            session.history("reload").await.map_err(|err| err.to_string())?;
            // LCP and late layout shifts land after the load settles.
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        }
        let tab = session.active().map_err(|err| err.to_string())?;
        let metrics = observe::performance(&tab.page).await.map_err(|err| err.to_string())?;
        let ms = |key: &str| {
            metrics[key]
                .as_i64()
                .map(|value| format!("{value} ms"))
                .unwrap_or_else(|| "n/a".into())
        };
        let mut out = format!("Performance of {}\n", metrics["url"].as_str().unwrap_or(""));
        out.push_str(&format!(
            "TTFB {} · FCP {} · DOMContentLoaded {} · load {}\n",
            ms("ttfb"),
            ms("fcp"),
            ms("domContentLoaded"),
            ms("load")
        ));
        let lcp = &metrics["lcp"];
        out.push_str(&format!(
            "LCP {}{} (good ≤ 2500 ms)\n",
            lcp["ms"].as_i64().map(|value| format!("{value} ms")).unwrap_or_else(|| "n/a".into()),
            lcp["element"].as_str().map(|el| format!(" on {el}")).unwrap_or_default()
        ));
        out.push_str(&format!("CLS {} (good ≤ 0.1)\n", metrics["cls"]));
        out.push_str(&format!(
            "INP {} (good ≤ 200 ms; only after interactions)\n",
            ms("inp")
        ));
        out.push_str(&format!(
            "Long tasks: {} ({} ms blocking) · {} requests · {} KB transferred\n",
            metrics["longTasks"], metrics["longTaskMs"], metrics["requests"], metrics["transferKb"]
        ));
        if let Some(slowest) = metrics["slowest"].as_array().filter(|list| !list.is_empty()) {
            out.push_str("Slowest requests:\n");
            for resource in slowest {
                out.push_str(&format!(
                    "  {} ms · {} KB · {} {}\n",
                    resource["ms"],
                    resource["kb"],
                    resource["type"].as_str().unwrap_or(""),
                    clip(resource["url"].as_str().unwrap_or(""), 140)
                ));
            }
        }
        out.push_str("For a full audit (accessibility, SEO, best practices), run `npx lighthouse <url>` in the shell.\n");
        Ok(out)
    }

    async fn run_resize(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            width: u32,
            height: u32,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        session
            .set_viewport(input.width, input.height)
            .await
            .map_err(|err| err.to_string())?;
        let (width, height) = session.viewport();
        let _ = session.active().map_err(|err| err.to_string())?.settle().await;
        Ok(format!("Viewport is now {width}×{height}."))
    }

    async fn run_record(&self, input: Value) -> ToolRunResult {
        #[derive(Deserialize)]
        struct Input {
            action: String,
            fps: Option<u8>,
            max_duration_s: Option<u32>,
        }
        let input: Input = match parse(input) {
            Ok(input) => input,
            Err(err) => return ToolRunResult::err(err, Vec::new()),
        };
        if input.action == "start" {
            let page = {
                let mut sessions = self.sessions.0.lock().await;
                match session_mut(&mut sessions, &self.workspace_id)
                    .and_then(|session| session.active().map(|tab| tab.page.clone()).map_err(|err| err.to_string()))
                {
                    Ok(page) => page,
                    Err(err) => return ToolRunResult::err(err, Vec::new()),
                }
            };
            let fps = input.fps.unwrap_or(4).clamp(1, 10);
            let max = input.max_duration_s.unwrap_or(30).clamp(1, 120);
            *self.recording.lock().await = Some(RecordingHandle::start(page, fps, max));
            return ToolRunResult::ok(format!("Recording at {fps} fps (max {max}s). Call again with \"stop\"."), Vec::new());
        }
        let Some(handle) = self.recording.lock().await.take() else {
            return ToolRunResult::err("No recording in progress.", Vec::new());
        };
        match handle.stop("gif").await {
            Ok((bytes, media_type, frames, duration_ms)) if frames > 0 => ToolRunResult::ok_with_images(
                format!("{frames} frames · {duration_ms} ms · {} KB", bytes.len() / 1024),
                vec![ToolRunImage {
                    media_type: media_type.into(),
                    data: gif_to_base64(&bytes),
                    path: save_capture(&self.workspace_id, &bytes, "gif"),
                }],
                Vec::new(),
            ),
            Ok(_) => ToolRunResult::err("The recording captured no frames.", Vec::new()),
            Err(err) => ToolRunResult::err(err.to_string(), Vec::new()),
        }
    }

    async fn run_close(&self) -> Result<String, String> {
        *self.recording.lock().await = None;
        let removed = self.sessions.0.lock().await.remove(&self.workspace_id);
        Ok(if removed.is_some() {
            "Browser closed.".into()
        } else {
            "The browser was not open.".into()
        })
    }

    async fn run_eval(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            expression: String,
        }
        let input: Input = parse(input)?;
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        let value = session.eval(&input.expression).await.map_err(|err| err.to_string())?;
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        Ok(clip(&text, EVAL_OUTPUT_LIMIT))
    }

    async fn run_upload(&self, input: Value) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Input {
            #[serde(rename = "ref")]
            reference: String,
            paths: Vec<String>,
        }
        let input: Input = parse(input)?;
        let root = std::fs::canonicalize(&self.workspace_id).map_err(|err| err.to_string())?;
        let mut files = Vec::new();
        for path in &input.paths {
            let full = std::fs::canonicalize(root.join(path))
                .map_err(|_| format!("{path} does not exist in the project"))?;
            if !full.starts_with(&root) || !full.is_file() {
                return Err(format!("{path} is not a file inside the project"));
            }
            files.push(full.to_string_lossy().trim_start_matches(r"\\?\").to_string());
        }
        let mut sessions = self.sessions.0.lock().await;
        let session = session_mut(&mut sessions, &self.workspace_id)?;
        let node = resolve_ref(session, &input.reference).await?;
        let page = &session.active().map_err(|err| err.to_string())?.page;
        input::upload(page, node, &files).await.map_err(|err| err.to_string())?;
        Ok(format!("{} file(s) set on {}.", files.len(), input.reference))
    }
}

// ── Before / after an action ────────────────────────────────────────────

struct Before {
    url: String,
    document: Value,
    console_mark: u64,
    clock: f64,
    tabs: usize,
}

impl Before {
    async fn capture(session: &BrowserSession) -> Self {
        let Ok(tab) = session.active() else {
            return Self { url: String::new(), document: Value::Null, console_mark: 0, clock: 0.0, tabs: 0 };
        };
        let state = sinew_browser_eval(session, "[location.href, performance.timeOrigin, performance.now()]").await;
        Self {
            url: state[0].as_str().unwrap_or_default().to_string(),
            document: state[1].clone(),
            console_mark: tab.log.console_mark().await,
            clock: state[2].as_f64().unwrap_or(0.0),
            tabs: session.tab_count(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Report {
    /// Navigation already waited for the page: report where it landed.
    Navigation,
    /// Clicks, scrolls…: settle, then say whether anything happened.
    Changes,
    /// Typing and form input: settle, but an unchanged page is expected.
    Edits,
}

/// What an action did: settles the page, then reports navigation, new
/// tabs, whether the page changed, and new console errors.
async fn after(session: &mut BrowserSession, before: &Before, workspace: &str, report: Report) -> String {
    if report != Report::Navigation {
        if let Ok(tab) = session.active() {
            let _ = tab.settle().await;
        }
    }
    let changes = session.sync_tabs().await;
    let mut out = String::new();
    for change in &changes {
        out.push_str(&format!("{change}\n"));
    }
    let Ok(tab) = session.active() else {
        out.push_str("No tab is open.\n");
        return out;
    };
    let state = sinew_browser_eval(
        session,
        "[location.href, document.title, performance.timeOrigin, window.__sinew ? window.__sinew.lastMutation : 0]",
    )
    .await;
    let url = state[0].as_str().unwrap_or_default();
    let title = state[1].as_str().unwrap_or_default();
    let new_document = state[2] != before.document;
    let opened_tab = changes.iter().any(|change| change.starts_with("new tab"));
    if opened_tab {
        // Reported above.
    } else if url != before.url {
        out.push_str(&format!("Navigated to {url} — \"{title}\".\n"));
    } else if new_document {
        out.push_str(&format!("The page reloaded: \"{title}\".\n"));
    } else if state[3].as_f64().unwrap_or(0.0) > before.clock {
        out.push_str("The page updated; references to removed elements are now stale.\n");
    } else if report == Report::Changes {
        out.push_str("No visible change on the page.\n");
    }
    if session.tab_count() > 1 && before.tabs != session.tab_count() || session.tab_count() > 1 && opened_tab {
        out.push_str(&format!("Tabs:\n{}", session.tab_list().await));
    }
    let messages: Vec<ConsoleEntry> = tab
        .log
        .console_since(before.console_mark)
        .await
        .into_iter()
        .filter(|entry| matches!(entry.level.as_str(), "error" | "exception") || entry.text.contains("dialog auto-accepted"))
        .collect();
    if !messages.is_empty() {
        out.push_str(&format!("Console ({} new):\n", messages.len()));
        for entry in messages.iter().take(5) {
            out.push_str(&format_console(entry, workspace));
        }
    }
    out
}

async fn sinew_browser_eval(session: &BrowserSession, expression: &str) -> Value {
    session.eval(expression).await.unwrap_or(Value::Null)
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn parse<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, String> {
    // Some models send `{}` as null for tools without arguments.
    let input = if input.is_null() { json!({}) } else { input };
    serde_json::from_value(input).map_err(|err| format!("invalid arguments: {err}"))
}

fn session_mut<'a>(
    sessions: &'a mut std::collections::HashMap<String, BrowserSession>,
    workspace: &str,
) -> Result<&'a mut BrowserSession, String> {
    sessions
        .get_mut(workspace)
        .ok_or_else(|| "The browser is not open. Call browser_navigate first.".to_string())
}

async fn resolve_ref(session: &BrowserSession, reference: &str) -> Result<i64, String> {
    let tab = session.active().map_err(|err| err.to_string())?;
    tab.refs.resolve(&tab.page, reference).await.map_err(|err| err.to_string())
}

async fn resolve_target(
    session: &BrowserSession,
    reference: Option<&str>,
    coordinate: Option<[f64; 2]>,
) -> Result<Target, String> {
    match (reference, coordinate) {
        (Some(reference), _) => Ok(Target::Node(resolve_ref(session, reference).await?)),
        (None, Some([x, y])) => {
            let (width, height) = session.viewport();
            if x < 0.0 || y < 0.0 || x > f64::from(width) || y > f64::from(height) {
                return Err(format!(
                    "({x}, {y}) is outside the {width}×{height} viewport; scroll first or use a ref"
                ));
            }
            Ok(Target::Point(x, y))
        }
        (None, None) => Err("give a `ref` (preferred) or a `coordinate`".into()),
    }
}

fn describe_target(reference: Option<&str>, (x, y): (f64, f64)) -> String {
    match reference {
        Some(reference) => format!("{reference} at ({x:.0}, {y:.0})"),
        None => format!("({x:.0}, {y:.0})"),
    }
}

/// Only web pages; a bare host gets a scheme (http for local addresses).
fn normalize_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty URL".into());
    }
    if raw == "about:blank" {
        return Ok(raw.into());
    }
    let lower = raw.to_lowercase();
    if let Some((scheme, _)) = lower.split_once("://") {
        if scheme == "http" || scheme == "https" {
            return Ok(raw.into());
        }
        return Err(format!("{scheme}: URLs are not allowed; only http and https"));
    }
    if let Some((scheme, _)) = lower.split_once(':') {
        if !scheme.contains('.') && !scheme.chars().all(|c| c.is_ascii_digit()) && !is_local_host(scheme) {
            return Err(format!("{scheme}: URLs are not allowed; only http and https"));
        }
    }
    let host = lower.split(['/', ':']).next().unwrap_or("");
    let scheme = if is_local_host(host) { "http" } else { "https" };
    Ok(format!("{scheme}://{raw}"))
}

fn is_local_host(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".test")
        || host.starts_with("127.")
        || host == "0.0.0.0"
        || host.starts_with("192.168.")
        || host.starts_with("10.")
        || host == "[::1]"
}

fn format_console(entry: &ConsoleEntry, workspace: &str) -> String {
    let mut out = format!("[{}] {}\n", entry.level, clip(&entry.text, 2_000));
    for frame in &entry.stack {
        out.push_str(&format!("    at {}\n", map_frame(frame, workspace)));
    }
    out
}

/// `http://localhost:5173/src/App.tsx?t=1:12:5` → `src/App.tsx:12:5` when
/// that file exists in the project (Vite, webpack-internal, /@fs paths).
fn map_frame(frame: &str, workspace: &str) -> String {
    let (prefix, location) = match frame.rfind(" (") {
        Some(at) if frame.ends_with(')') => (&frame[..at + 2], &frame[at + 2..frame.len() - 1]),
        _ => ("", frame),
    };
    let mapped = map_location(location, Path::new(workspace)).unwrap_or_else(|| location.to_string());
    if prefix.is_empty() {
        mapped
    } else {
        format!("{prefix}{mapped})")
    }
}

fn map_location(location: &str, workspace: &Path) -> Option<String> {
    // Split off `:line:col` from the end.
    let mut parts = location.rsplitn(3, ':');
    let col = parts.next()?;
    let line = parts.next()?;
    let url = parts.next()?;
    if !col.chars().all(|c| c.is_ascii_digit()) || !line.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let path = if let Some(rest) = url.strip_prefix("webpack-internal:///") {
        rest.trim_start_matches("./").to_string()
    } else {
        let after_scheme = url.split_once("://")?.1;
        let (host, path) = after_scheme.split_once('/')?;
        let host = host.split(':').next().unwrap_or(host);
        if !is_local_host(host) && host != "[::1]" {
            return None;
        }
        path.to_string()
    };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let candidate: PathBuf = match path.strip_prefix("@fs/") {
        Some(absolute) => PathBuf::from(format!("/{absolute}")),
        None => workspace.join(path),
    };
    if !candidate.is_file() {
        return None;
    }
    let shown = candidate
        .strip_prefix(workspace)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| candidate.to_string_lossy().into_owned());
    Some(format!("{shown}:{line}:{col}"))
}

fn status_text(entry: &sinew_browser::NetworkEntry) -> String {
    match (&entry.failed, entry.status) {
        (Some(reason), _) => format!("FAILED ({reason})"),
        (None, Some(status)) => {
            if entry.status_text.is_empty() {
                status.to_string()
            } else {
                format!("{status} {}", entry.status_text)
            }
        }
        (None, None) => "pending".into(),
    }
}

fn headers(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return "  (none)\n".into();
    };
    let mut out = String::new();
    for (name, value) in map {
        let lower = name.to_lowercase();
        let secret = SECRET_HEADERS.contains(&lower.as_str()) || lower.contains("token") || lower.contains("secret");
        let shown = if secret {
            "[hidden]".to_string()
        } else {
            clip(value.as_str().unwrap_or(&value.to_string()), 300)
        };
        out.push_str(&format!("  {name}: {shown}\n"));
    }
    if out.is_empty() {
        "  (none)\n".into()
    } else {
        out
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let clipped: String = text.chars().take(max).collect();
    format!("{clipped}… ({} characters total)", text.chars().count())
}

fn png_dimensions(bytes: &[u8]) -> (u32, u32) {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return (0, 0);
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap_or([0; 4]));
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap_or([0; 4]));
    (width, height)
}

/// Saves a capture under `.sinew/screens` in the project.
fn save_capture(workspace_id: &str, bytes: &[u8], extension: &str) -> Option<String> {
    let dir = Path::new(workspace_id).join(".sinew").join("screens");
    std::fs::create_dir_all(&dir).ok()?;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!("{millis}.{extension}"));
    std::fs::write(&path, bytes).ok()?;
    path.to_str().map(|path| path.trim_start_matches(r"\\?\").to_string())
}

fn descriptor(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.into(),
        description: description.into(),
        input_schema,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_limited_to_the_web() {
        assert_eq!(normalize_url("localhost:5173").unwrap(), "http://localhost:5173");
        assert_eq!(normalize_url("example.com/a").unwrap(), "https://example.com/a");
        assert_eq!(normalize_url("https://x.dev").unwrap(), "https://x.dev");
        assert_eq!(normalize_url("127.0.0.1:3000").unwrap(), "http://127.0.0.1:3000");
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("data:text/html,hi").is_err());
    }

    #[test]
    fn local_frames_map_to_project_files() {
        let dir = std::env::temp_dir().join(format!("sinew-map-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/App.tsx"), "x").unwrap();
        let workspace = dir.to_string_lossy().to_string();
        assert_eq!(
            map_frame("render (http://localhost:5173/src/App.tsx?t=17:12:5)", &workspace),
            "render (src/App.tsx:12:5)"
        );
        assert_eq!(
            map_frame("webpack-internal:///./src/App.tsx:3:1", &workspace),
            "src/App.tsx:3:1"
        );
        assert_eq!(
            map_frame("https://cdn.example.com/lib.js:1:2", &workspace),
            "https://cdn.example.com/lib.js:1:2"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Serves `FIXTURE` at / and a 500 at /api/fail, then drives a real
    /// headless Chrome through the tools. Needs Chrome or Edge installed:
    /// `cargo test -p sinew-app --lib browser -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn drives_a_real_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        const FIXTURE: &str = r#"<!doctype html><html><head><title>Fixture</title></head><body>
<main>
  <h1>Sinew fixture</h1>
  <div><label>Email <input id="email" type="email"></label></div>
  <div><select id="plan" aria-label="Plan"><option value="free">Free</option><option value="pro">Pro</option></select></div>
  <div><label><input id="terms" type="checkbox"> Accept terms</label></div>
  <button id="go" onclick="document.getElementById('out').textContent = 'Hello ' + document.getElementById('email').value + ' on ' + document.getElementById('plan').value + (document.getElementById('terms').checked ? ' (terms)' : ''); fetch('/api/fail'); console.error('boom from fixture');">Send</button>
  <p id="out">nothing yet</p>
  <div><a href="/" target="_blank">Open a copy</a></div>
  <div style="margin-top:40px"><button id="covered">Hidden behind</button></div>
  <div style="position:fixed;left:0;top:0;width:100vw;height:100vh;pointer-events:none"></div>
  <div id="overlay" style="position:absolute;left:0;width:200px;height:40px;background:#fff"></div>
</main>
<script>
  const covered = document.getElementById('covered');
  const overlay = document.getElementById('overlay');
  const r = covered.getBoundingClientRect();
  overlay.style.top = (r.top + scrollY - 10) + 'px';
  overlay.style.left = (r.left - 10) + 'px';
</script>
</body></html>"#;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let (status, body, kind) = if request.starts_with("GET /api/fail") {
                        ("500 Internal Server Error", "{\"error\":\"nope\"}", "application/json")
                    } else if request.starts_with("GET / ") {
                        ("200 OK", FIXTURE, "text/html")
                    } else {
                        ("404 Not Found", "", "text/plain")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });

        let workspace = std::env::temp_dir().join(format!("sinew-browser-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let tools = BrowserTools::new(workspace.to_string_lossy().into_owned(), BrowserSessions::new());
        let run = |name: &'static str, input: Value| {
            let tools = &tools;
            async move {
                let result = tools.run(name, input, false).await;
                println!("── {name} ──\n{}\n", result.content);
                result
            }
        };

        let opened = run(tool_names::BROWSER_NAVIGATE, json!({ "url": format!("127.0.0.1:{port}"), "headless": true })).await;
        assert!(!opened.is_error, "{}", opened.content);
        assert!(opened.content.contains("button \"Send\" [ref_"), "{}", opened.content);

        let page = run(tool_names::BROWSER_READ_PAGE, json!({})).await;
        assert!(page.content.contains("heading \"Sinew fixture\""), "{}", page.content);

        let reference = |text: &str, label: &str| -> String {
            let line = text.lines().find(|line| line.contains(label)).unwrap_or_else(|| panic!("no {label} in {text}"));
            let start = line.rfind("[ref_").unwrap() + 1;
            let end = start + line[start..].find(']').unwrap();
            line[start..end].to_string()
        };
        let found = run(tool_names::BROWSER_FIND, json!({ "query": "email field" })).await;
        let email = reference(&found.content, "Email");
        let send = reference(&page.content, "\"Send\"");
        let plan = reference(&page.content, "\"Plan\"");
        let terms = reference(&page.content, "Accept terms");
        let covered = reference(&page.content, "Hidden behind");

        let typed = run(tool_names::BROWSER_TYPE, json!({ "ref": email, "text": "ada@example.com" })).await;
        assert!(!typed.is_error, "{}", typed.content);
        let selected = run(tool_names::BROWSER_FORM_INPUT, json!({ "ref": plan, "value": "Pro" })).await;
        assert!(selected.content.contains("selected Pro"), "{}", selected.content);
        let checked = run(tool_names::BROWSER_FORM_INPUT, json!({ "ref": terms, "value": true })).await;
        assert!(checked.content.contains("checked"), "{}", checked.content);

        let clicked = run(tool_names::BROWSER_CLICK, json!({ "ref": send })).await;
        assert!(!clicked.is_error, "{}", clicked.content);
        assert!(clicked.content.contains("The page updated"), "{}", clicked.content);
        assert!(clicked.content.contains("boom from fixture"), "{}", clicked.content);
        let text = run(tool_names::BROWSER_GET_TEXT, json!({})).await;
        assert!(text.content.contains("Hello ada@example.com on pro (terms)"), "{}", text.content);

        let blocked = run(tool_names::BROWSER_CLICK, json!({ "ref": covered })).await;
        assert!(blocked.is_error && blocked.content.contains("covered by div#overlay"), "{}", blocked.content);

        let network = run(tool_names::BROWSER_NETWORK, json!({ "failed_only": true })).await;
        assert!(network.content.contains("500"), "{}", network.content);
        let console = run(tool_names::BROWSER_CONSOLE, json!({ "level": "error" })).await;
        assert!(console.content.contains("boom from fixture"), "{}", console.content);
        let performance = run(tool_names::BROWSER_PERFORMANCE, json!({})).await;
        assert!(performance.content.contains("TTFB"), "{}", performance.content);

        let keys = run(tool_names::BROWSER_KEY, json!({ "keys": "ctrl+a Escape" })).await;
        assert!(!keys.is_error, "{}", keys.content);
        let shot = run(tool_names::BROWSER_SCREENSHOT, json!({})).await;
        assert!(shot.content.contains("1280×800"), "{}", shot.content);

        let link = reference(&page.content, "Open a copy");
        let popup = run(tool_names::BROWSER_CLICK, json!({ "ref": link })).await;
        assert!(popup.content.contains("new tab tab_2"), "{}", popup.content);
        let tabs = run(tool_names::BROWSER_TABS, json!({ "action": "switch", "tab_id": "tab_1" })).await;
        assert!(tabs.content.contains("tab_1 (active)"), "{}", tabs.content);

        let reloaded = run(tool_names::BROWSER_NAVIGATE, json!({ "url": "reload" })).await;
        assert!(!reloaded.is_error, "{}", reloaded.content);
        let stale = run(tool_names::BROWSER_CLICK, json!({ "ref": send })).await;
        assert!(stale.is_error && stale.content.contains("stale"), "{}", stale.content);

        let disabled = run(tool_names::BROWSER_EVAL, json!({ "expression": "1" })).await;
        assert!(disabled.is_error, "{}", disabled.content);

        run(tool_names::BROWSER_CLOSE, json!({})).await;
        let _ = std::fs::remove_dir_all(workspace);
    }

    /// Reads a real site: `SINEW_BROWSER_URL=https://… cargo test -p
    /// sinew-app --lib browser::tests::reads_a_live_site -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn reads_a_live_site() {
        let Ok(url) = std::env::var("SINEW_BROWSER_URL") else {
            return;
        };
        let query = std::env::var("SINEW_BROWSER_FIND").unwrap_or_else(|_| "search".into());
        let workspace = std::env::temp_dir().join(format!("sinew-browser-live-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let tools = BrowserTools::new(workspace.to_string_lossy().into_owned(), BrowserSessions::new());
        for (name, input) in [
            (tool_names::BROWSER_NAVIGATE, json!({ "url": url, "headless": true })),
            (tool_names::BROWSER_READ_PAGE, json!({})),
            (tool_names::BROWSER_FIND, json!({ "query": query })),
            (tool_names::BROWSER_CONSOLE, json!({ "level": "warning" })),
            (tool_names::BROWSER_PERFORMANCE, json!({})),
        ] {
            let started = std::time::Instant::now();
            let result = tools.run(name, input, false).await;
            let preview: String = result.content.chars().take(2_500).collect();
            println!(
                "── {name} ({} ms, {} chars, error={}) ──\n{preview}\n",
                started.elapsed().as_millis(),
                result.content.len(),
                result.is_error
            );
        }
        tools.run(tool_names::BROWSER_CLOSE, json!({}), false).await;
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn secret_headers_are_hidden() {
        let shown = headers(&json!({ "Authorization": "Bearer abc", "Accept": "*/*", "X-Session-Token": "t" }));
        assert!(shown.contains("Authorization: [hidden]"));
        assert!(shown.contains("X-Session-Token: [hidden]"));
        assert!(shown.contains("Accept: */*"));
    }
}
