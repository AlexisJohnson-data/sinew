//! The page as the agent reads it: Chrome's accessibility tree, compacted,
//! with a reference (`ref_12`) on every element worth acting on.
//!
//! A reference names one DOM node (by backend node id). The same node keeps
//! its reference across reads of the same document. References are never
//! reused in a tab: those of a previous document are reported as stale.

use std::collections::HashMap;

use anyhow::{anyhow, bail, Result};
use chromiumoxide::Page;
use serde_json::Value;

use crate::cdp;

pub const DEFAULT_MAX_CHARS: usize = 30_000;
pub const HARD_MAX_CHARS: usize = 50_000;
const MAX_NAME_CHARS: usize = 100;
const MAX_TEXT_CHARS: usize = 160;
const MAX_FIND_RESULTS: usize = 20;
const MAX_FRAMES: usize = 12;

/// Roles an agent acts on. They always get a reference.
const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "option",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "treeitem",
    "gridcell",
    "columnheader",
    "rowheader",
    "scrollbar",
    "textarea",
    "Iframe",
    "iframe",
];

/// Structure shown in the full view, for orientation.
const STRUCTURE: &[&str] = &[
    "heading",
    "main",
    "navigation",
    "banner",
    "contentinfo",
    "complementary",
    "region",
    "form",
    "search",
    "dialog",
    "alertdialog",
    "alert",
    "status",
    "tablist",
    "tabpanel",
    "menu",
    "menubar",
    "toolbar",
    "tree",
    "grid",
    "table",
    "list",
    "listitem",
    "article",
    "figure",
    "image",
    "img",
    "progressbar",
    "separator",
];

/// Structure shown only when it has a name.
const UNNAMED_NOISE: &[&str] = &[
    "image",
    "img",
    "figure",
    "separator",
    "list",
    "listitem",
    "article",
    "region",
    "progressbar",
];

/// Flattened away: they only wrap other nodes.
const TRANSPARENT: &[&str] = &[
    "none",
    "generic",
    "presentation",
    "InlineTextBox",
    "LineBreak",
    "RootWebArea",
    "WebArea",
    "paragraph",
    "group",
    "Section",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "row",
    "rowgroup",
    "cell",
    "ListMarker",
    "DescriptionList",
    "term",
    "definition",
    "strong",
    "emphasis",
    "code",
    "label",
    "legend",
    "abbr",
    "time",
    "mark",
    "superscript",
    "subscript",
    "blockquote",
    "Pre",
    "Canvas",
    "Video",
    "Audio",
    "Ruby",
    "Abbr",
    "sectionheader",
    "sectionfooter",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Interactive elements and dialogs only.
    Interactive,
    /// Interactive elements, structure and text.
    All,
}

/// Refs issued for a tab's current document.
#[derive(Debug, Default)]
pub struct RefMap {
    by_ref: HashMap<String, i64>,
    by_node: HashMap<i64, String>,
    next: u32,
    /// `performance.timeOrigin` of the document the refs belong to.
    document: Option<f64>,
}

impl RefMap {
    fn ref_for(&mut self, backend_node_id: i64) -> String {
        if let Some(existing) = self.by_node.get(&backend_node_id) {
            return existing.clone();
        }
        self.next += 1;
        let reference = format!("ref_{}", self.next);
        self.by_ref.insert(reference.clone(), backend_node_id);
        self.by_node.insert(backend_node_id, reference.clone());
        reference
    }

    /// A new document: earlier references become stale. Numbering goes
    /// on, so an old reference can never name a new element.
    fn reset(&mut self, document: Option<f64>) {
        self.by_ref.clear();
        self.by_node.clear();
        self.document = document;
    }

    /// The node behind `reference`, checked to still be in the page.
    pub async fn resolve(&self, page: &Page, reference: &str) -> Result<i64> {
        let reference = reference.trim();
        let stale = || {
            anyhow!(
                "{reference} is stale or not found. The page changed since it was read; call browser_read_page or browser_find for fresh references."
            )
        };
        let Some(&backend_node_id) = self.by_ref.get(reference) else {
            return Err(stale());
        };
        if document_key(page).await != self.document {
            return Err(stale());
        }
        let connected = cdp::call_on_node(
            page,
            backend_node_id,
            "function() { return this.isConnected; }",
            &[],
        )
        .await
        .map_err(|_| stale())?;
        if connected != Value::Bool(true) {
            return Err(stale());
        }
        Ok(backend_node_id)
    }
}

async fn document_key(page: &Page) -> Option<f64> {
    cdp::eval(page, "performance.timeOrigin")
        .await
        .ok()
        .and_then(|value| value.as_f64())
}

struct AxNode {
    role: String,
    name: String,
    value: String,
    backend: Option<i64>,
    ignored: bool,
    children: Vec<String>,
    props: Vec<(String, Value)>,
}

/// One frame's tree, keyed by AX node id.
struct FrameTree {
    nodes: HashMap<String, AxNode>,
    root: Option<String>,
}

fn ax_string(value: &Value) -> String {
    match &value["value"] {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn parse_tree(response: &Value) -> FrameTree {
    let mut nodes = HashMap::new();
    let mut root = None;
    for node in response["nodes"].as_array().into_iter().flatten() {
        let Some(id) = node["nodeId"].as_str() else {
            continue;
        };
        if root.is_none() && node.get("parentId").map_or(true, Value::is_null) {
            root = Some(id.to_string());
        }
        let props = node["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|prop| {
                Some((
                    prop["name"].as_str()?.to_string(),
                    prop["value"]["value"].clone(),
                ))
            })
            .collect();
        nodes.insert(
            id.to_string(),
            AxNode {
                role: ax_string(&node["role"]),
                name: ax_string(&node["name"]),
                value: ax_string(&node["value"]),
                backend: node["backendDOMNodeId"].as_i64(),
                ignored: node["ignored"].as_bool().unwrap_or(false),
                children: node["childIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|child| child.as_str().map(str::to_string))
                    .collect(),
                props,
            },
        );
    }
    FrameTree { nodes, root }
}

/// A printed element, used by `find`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub reference: String,
    pub role: String,
    pub name: String,
    pub value: String,
    /// Nearest heading or landmark names above it.
    pub context: Vec<String>,
}

pub struct Snapshot {
    pub text: String,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Every frame of the page: the main tree, plus child-frame trees keyed
/// by the backend node id of their `<iframe>` (None when unreadable).
async fn load_frames(page: &Page) -> Result<(FrameTree, HashMap<i64, Option<FrameTree>>)> {
    let main = parse_tree(&cdp::call(page, "Accessibility.getFullAXTree", serde_json::json!({})).await?);
    let mut frames = HashMap::new();
    let frame_tree = cdp::call(page, "Page.getFrameTree", serde_json::json!({}))
        .await
        .unwrap_or(Value::Null);
    let mut pending: Vec<&Value> = frame_tree["frameTree"]["childFrames"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    while let Some(child) = pending.pop() {
        if frames.len() >= MAX_FRAMES {
            break;
        }
        let Some(frame_id) = child["frame"]["id"].as_str() else {
            continue;
        };
        let owner = cdp::call(
            page,
            "DOM.getFrameOwner",
            serde_json::json!({ "frameId": frame_id }),
        )
        .await
        .ok()
        .and_then(|owner| owner["backendNodeId"].as_i64());
        let tree = cdp::call(
            page,
            "Accessibility.getFullAXTree",
            serde_json::json!({ "frameId": frame_id }),
        )
        .await
        .ok()
        .map(|response| parse_tree(&response));
        if let Some(owner) = owner {
            frames.insert(owner, tree);
        }
        pending.extend(child["childFrames"].as_array().into_iter().flatten());
    }
    Ok((main, frames))
}

struct Renderer<'a> {
    refs: &'a mut RefMap,
    frames: &'a HashMap<i64, Option<FrameTree>>,
    filter: Filter,
    max_chars: usize,
    out: String,
    entries: Vec<Entry>,
    truncated: bool,
    /// Name of the nearest shown element above: text it already says is
    /// not printed again.
    enclosing_names: Vec<String>,
}

impl Renderer<'_> {
    fn render(&mut self, tree: &FrameTree, id: &str, depth: usize, context: &mut Vec<String>) {
        if self.truncated {
            return;
        }
        let Some(node) = tree.nodes.get(id) else {
            return;
        };
        let role = node.role.as_str();
        let interactive = INTERACTIVE.contains(&role) || is_focusable_widget(node);
        // Decorative or merely grouping nodes say nothing without a name.
        let structural = STRUCTURE.contains(&role)
            && (!UNNAMED_NOISE.contains(&role) || !node.name.trim().is_empty());
        let is_text = role == "StaticText";
        let shown = !node.ignored
            && !TRANSPARENT.contains(&role)
            && match self.filter {
                Filter::Interactive => interactive || matches!(role, "dialog" | "alertdialog"),
                Filter::All => {
                    interactive
                        || structural
                        || (is_text && !node.name.trim().is_empty() && !self.said_above(&node.name))
                }
            };

        let mut child_depth = depth;
        let mut pushed_context = false;
        if shown {
            let name = clip(&collapse(&node.name), if is_text { MAX_TEXT_CHARS } else { MAX_NAME_CHARS });
            let mut line = format!("{}{}", "  ".repeat(depth), display_role(role));
            if !name.is_empty() {
                line.push_str(&format!(" \"{}\"", name.replace('"', "'")));
            }
            line.push_str(&props_suffix(node));
            if !is_text {
                if let Some(backend) = node.backend {
                    let reference = self.refs.ref_for(backend);
                    line.push_str(&format!(" [{reference}]"));
                    self.entries.push(Entry {
                        reference,
                        role: role.to_string(),
                        name: collapse(&node.name),
                        value: collapse(&node.value),
                        context: context.clone(),
                    });
                }
            }
            if self.out.len() + line.len() + 1 > self.max_chars {
                self.truncated = true;
                return;
            }
            self.out.push_str(&line);
            self.out.push('\n');
            child_depth = depth + 1;
            if (role == "heading" || STRUCTURE.contains(&role)) && !name.is_empty() {
                context.push(name);
                pushed_context = true;
            }
        }

        // An <iframe> continues with its frame's own tree.
        if matches!(role, "Iframe" | "iframe") {
            if let Some(backend) = node.backend {
                match self.frames.get(&backend) {
                    Some(Some(frame)) => {
                        if let Some(root) = frame.root.clone() {
                            self.render(frame, &root, child_depth, context);
                        }
                    }
                    Some(None) => {
                        let line = format!(
                            "{}(cross-origin frame: not readable, use coordinates from a screenshot)\n",
                            "  ".repeat(child_depth)
                        );
                        if self.out.len() + line.len() <= self.max_chars {
                            self.out.push_str(&line);
                        }
                    }
                    None => {}
                }
            }
        }

        let names_element = shown && !is_text && !node.name.trim().is_empty();
        if names_element {
            self.enclosing_names.push(collapse(&node.name).to_lowercase());
        }
        for child in &node.children {
            self.render(tree, child, child_depth, context);
        }
        if names_element {
            self.enclosing_names.pop();
        }
        if pushed_context {
            context.pop();
        }
    }

    /// Whether the nearest named element above already says `text`.
    fn said_above(&self, text: &str) -> bool {
        let text = collapse(text).to_lowercase();
        self.enclosing_names
            .last()
            .is_some_and(|name| name.contains(&text))
    }
}

fn is_focusable_widget(node: &AxNode) -> bool {
    // Custom widgets (div with tabindex and a click handler) show up as
    // generic but focusable; give them a reference when they have a name.
    node.role == "generic"
        && !node.name.trim().is_empty()
        && node
            .props
            .iter()
            .any(|(name, value)| name == "focusable" && value == &Value::Bool(true))
}

fn display_role(role: &str) -> &str {
    match role {
        "StaticText" => "text",
        "Iframe" => "iframe",
        "img" => "image",
        other => other,
    }
}

fn props_suffix(node: &AxNode) -> String {
    let mut out = String::new();
    for (name, value) in &node.props {
        let flag = match (name.as_str(), value) {
            ("disabled", Value::Bool(true)) => "disabled".to_string(),
            ("checked", Value::String(state)) if state == "true" => "checked".to_string(),
            ("checked", Value::String(state)) if state == "mixed" => "checked=mixed".to_string(),
            ("checked", Value::Bool(true)) => "checked".to_string(),
            ("pressed", Value::String(state)) if state == "true" => "pressed".to_string(),
            ("selected", Value::Bool(true)) => "selected".to_string(),
            ("expanded", Value::Bool(state)) => format!("expanded={state}"),
            ("focused", Value::Bool(true)) => "focused".to_string(),
            ("required", Value::Bool(true)) => "required".to_string(),
            ("invalid", Value::String(state)) if state != "false" => "invalid".to_string(),
            ("level", Value::Number(level)) => format!("level={level}"),
            _ => continue,
        };
        out.push_str(&format!(" [{flag}]"));
    }
    let value = collapse(&node.value);
    if !value.is_empty() && node.role != "StaticText" && value != collapse(&node.name) {
        out.push_str(&format!(" value=\"{}\"", clip(&value, MAX_NAME_CHARS).replace('"', "'")));
    }
    out
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Reads the page (or the subtree under `scope`) into text with references.
pub async fn read(
    page: &Page,
    refs: &mut RefMap,
    filter: Filter,
    scope: Option<&str>,
    max_chars: usize,
) -> Result<Snapshot> {
    let document = document_key(page).await;
    if document != refs.document {
        refs.reset(document);
    }
    let scope_backend = match scope {
        Some(reference) => Some(refs.resolve(page, reference).await?),
        None => None,
    };
    let (main, frames) = load_frames(page).await?;
    let mut renderer = Renderer {
        refs,
        frames: &frames,
        filter,
        // Callers clamp what they show; `find` reads further to search.
        max_chars: max_chars.max(1_000),
        out: String::new(),
        entries: Vec::new(),
        truncated: false,
        enclosing_names: Vec::new(),
    };
    let mut context = Vec::new();
    match scope_backend {
        Some(backend) => {
            let start = std::iter::once(&main)
                .chain(frames.values().flatten())
                .find_map(|tree| {
                    tree.nodes
                        .iter()
                        .find(|(_, node)| node.backend == Some(backend))
                        .map(|(id, _)| (tree, id.clone()))
                });
            let Some((tree, id)) = start else {
                bail!("{} is not in the accessibility tree (hidden or decorative)", scope.unwrap_or(""));
            };
            renderer.render(tree, &id, 0, &mut context);
        }
        None => {
            if let Some(root) = main.root.clone() {
                renderer.render(&main, &root, 0, &mut context);
            }
        }
    }
    Ok(Snapshot {
        text: merge_text_runs(&renderer.out),
        entries: renderer.entries,
        truncated: renderer.truncated,
    })
}

/// Joins consecutive `text "…"` lines at the same depth (a paragraph
/// split by inline elements, a code block split into tokens) into one.
fn merge_text_runs(out: &str) -> String {
    const MAX_RUN: usize = 400;
    let mut merged: Vec<String> = Vec::new();
    for line in out.lines() {
        let indent = line.len() - line.trim_start().len();
        let text = line
            .trim_start()
            .strip_prefix("text \"")
            .and_then(|rest| rest.strip_suffix('"'));
        if let (Some(text), Some(previous)) = (text, merged.last_mut()) {
            let previous_indent = previous.len() - previous.trim_start().len();
            if previous_indent == indent && previous.trim_start().starts_with("text \"") {
                if previous.len() < MAX_RUN + indent + 8 {
                    previous.pop();
                    let joiner = if text.starts_with(|c: char| ",.;:!?)".contains(c)) { "" } else { " " };
                    previous.push_str(joiner);
                    previous.push_str(text);
                    previous.push('"');
                } else if !previous.ends_with("…\"") {
                    previous.pop();
                    previous.push_str(" …\"");
                }
                continue;
            }
        }
        merged.push(line.to_string());
    }
    let mut text = merged.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "for", "with", "and", "or", "le", "la", "les", "un",
    "une", "des", "du", "de", "et", "ou", "sur", "dans", "pour", "avec", "this", "that", "its",
];

/// Role words a query may use for a role.
fn role_words(role: &str) -> &'static [&'static str] {
    match role {
        "button" => &["button", "btn", "bouton"],
        "link" => &["link", "lien", "anchor"],
        "textbox" | "searchbox" | "textarea" => {
            &["input", "field", "textbox", "box", "text", "champ", "search", "textarea"]
        }
        "combobox" | "listbox" => &["select", "dropdown", "combobox", "menu", "list", "liste"],
        "checkbox" => &["checkbox", "check", "case"],
        "radio" => &["radio", "option"],
        "switch" => &["switch", "toggle"],
        "tab" => &["tab", "onglet"],
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => &["menu", "item"],
        "heading" => &["heading", "title", "titre"],
        "image" | "img" => &["image", "img", "picture", "icon"],
        "slider" => &["slider", "range"],
        "option" => &["option", "item"],
        "dialog" | "alertdialog" => &["dialog", "modal", "popup"],
        _ => &[],
    }
}

/// Elements matching a natural-language query ("login button", "email
/// field"), best first.
pub async fn find(page: &Page, refs: &mut RefMap, query: &str) -> Result<Vec<Entry>> {
    let snapshot = read(page, refs, Filter::All, None, HARD_MAX_CHARS * 4).await?;
    let tokens: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty() && !STOP_WORDS.contains(token))
        .map(str::to_string)
        .collect();
    if tokens.is_empty() {
        bail!("empty query");
    }
    let mut scored: Vec<(f32, Entry)> = snapshot
        .entries
        .into_iter()
        .filter_map(|entry| {
            let text = format!("{} {}", entry.name, entry.value).to_lowercase();
            let context = entry.context.join(" ").to_lowercase();
            let words = role_words(&entry.role);
            let mut score = 0.0;
            let mut text_hits = 0;
            for token in &tokens {
                if text.contains(token.as_str()) {
                    score += 2.0;
                    text_hits += 1;
                } else if words.contains(&token.as_str()) || entry.role == *token {
                    score += 1.0;
                } else if context.contains(token.as_str()) {
                    score += 0.5;
                }
            }
            // A role word alone ("button") matches every button: only keep
            // it when that's all the query asked for.
            let role_only = tokens
                .iter()
                .all(|token| words.contains(&token.as_str()) || entry.role == *token);
            (score > 0.0 && (text_hits > 0 || role_only)).then(|| {
                let exact = text.trim() == tokens.join(" ");
                (score + if exact { 1.0 } else { 0.0 }, entry)
            })
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored
        .into_iter()
        .take(MAX_FIND_RESULTS)
        .map(|(_, entry)| entry)
        .collect())
}

/// The readable text of the page: `<main>` or `<article>` when present.
pub async fn text(page: &Page, max_chars: usize) -> Result<String> {
    let value = cdp::eval(
        page,
        r#"(() => {
  const root = document.querySelector('main, [role=main], article') || document.body;
  if (!root) return '';
  return (root.innerText || '').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();
})()"#,
    )
    .await?;
    let text = value.as_str().unwrap_or_default();
    if text.chars().count() > max_chars {
        let clipped: String = text.chars().take(max_chars).collect();
        return Ok(format!("{clipped}\n… (truncated at {max_chars} characters)"));
    }
    Ok(text.to_string())
}
