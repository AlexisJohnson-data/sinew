//! Real input: mouse and keyboard events dispatched by Chrome as if a
//! person used them, aimed at an element's on-screen position.

use anyhow::{anyhow, bail, Result};
use chromiumoxide::Page;
use serde_json::{json, Value};

use crate::cdp;

/// Where an action lands: a reference from the page snapshot, already
/// resolved to its node, or a viewport point in CSS pixels.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Node(i64),
    Point(f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
        }
    }
}

/// CDP modifier bits from names like `ctrl`, `shift`.
pub fn modifier_bits(names: &[String]) -> i64 {
    names.iter().fold(0, |bits, name| {
        bits | match name.to_lowercase().as_str() {
            "alt" | "option" => 1,
            "ctrl" | "control" => 2,
            "meta" | "cmd" | "command" | "win" | "super" => 4,
            "shift" => 8,
            _ => 0,
        }
    })
}

/// Scrolls the node into view and returns its center, after checking it
/// is visible, enabled and not covered by something else.
pub async fn element_point(page: &Page, backend_node_id: i64, require_enabled: bool) -> Result<(f64, f64)> {
    let _ = cdp::call(
        page,
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await;
    let quads = cdp::call(
        page,
        "DOM.getContentQuads",
        json!({ "backendNodeId": backend_node_id }),
    )
    .await
    .map_err(|_| anyhow!("the element is not rendered (display: none or detached)"))?;
    let (x, y) = quads["quads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(quad_center)
        .next()
        .ok_or_else(|| anyhow!("the element has no visible area (zero size or fully clipped)"))?;

    if require_enabled {
        let disabled = cdp::call_on_node(
            page,
            backend_node_id,
            "function() { return !!(this.disabled || (this.getAttribute && this.getAttribute('aria-disabled') === 'true') || (this.closest && this.closest('fieldset[disabled]'))); }",
            &[],
        )
        .await
        .unwrap_or(Value::Bool(false));
        if disabled == Value::Bool(true) {
            bail!("the element is disabled");
        }
    }

    // Is the element (or something inside it) what's actually at its
    // center? A modal, cookie banner or sticky header may be on top. Asked
    // in the element's own document, so frames and shadow roots work.
    if let Ok(Value::String(cover)) = cdp::call_on_node(page, backend_node_id, COVER_CHECK, &[]).await {
        bail!("the element is covered by {cover} at ({x:.0}, {y:.0}); close or scroll past it first, or click that element instead");
    }
    Ok((x, y))
}

fn quad_center(quad: &Value) -> Option<(f64, f64)> {
    let points: Vec<f64> = quad.as_array()?.iter().filter_map(Value::as_f64).collect();
    if points.len() != 8 {
        return None;
    }
    let xs = [points[0], points[2], points[4], points[6]];
    let ys = [points[1], points[3], points[5], points[7]];
    let width = xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min);
    let height = ys.iter().cloned().fold(f64::MIN, f64::max) - ys.iter().cloned().fold(f64::MAX, f64::min);
    if width < 1.0 || height < 1.0 {
        return None;
    }
    Some((xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0))
}

/// Returns a description of what covers `this` at its center, or null.
const COVER_CHECK: &str = r#"function() {
  const rect = this.getBoundingClientRect();
  const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
  const root = this.getRootNode && this.getRootNode().elementFromPoint ? this.getRootNode() : this.ownerDocument;
  const hit = root.elementFromPoint(x, y);
  if (!hit || hit === this || this.contains(hit) || hit.contains(this)) return null;
  if (hit.control === this || (this.labels && Array.from(this.labels).includes(hit))) return null;
  const cls = typeof hit.className === 'string' && hit.className.trim() ? '.' + hit.className.trim().split(/\s+/).slice(0, 2).join('.') : '';
  return hit.tagName.toLowerCase() + (hit.id ? '#' + hit.id : '') + cls;
}"#;

pub async fn point_of(page: &Page, target: Target, require_enabled: bool) -> Result<(f64, f64)> {
    match target {
        Target::Node(node) => element_point(page, node, require_enabled).await,
        Target::Point(x, y) => Ok((x, y)),
    }
}

async fn mouse(page: &Page, kind: &str, x: f64, y: f64, extra: Value) -> Result<()> {
    let mut params = json!({ "type": kind, "x": x, "y": y });
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    cdp::call(page, "Input.dispatchMouseEvent", params).await?;
    Ok(())
}

pub async fn click(page: &Page, x: f64, y: f64, button: Button, clicks: u8, modifiers: i64) -> Result<()> {
    mouse(page, "mouseMoved", x, y, json!({ "modifiers": modifiers })).await?;
    for count in 1..=clicks.clamp(1, 3) {
        let extra = json!({ "button": button.name(), "clickCount": count, "modifiers": modifiers, "buttons": 1 });
        mouse(page, "mousePressed", x, y, extra.clone()).await?;
        mouse(page, "mouseReleased", x, y, extra).await?;
    }
    Ok(())
}

pub async fn hover(page: &Page, x: f64, y: f64) -> Result<()> {
    mouse(page, "mouseMoved", x, y, json!({})).await
}

pub async fn drag(page: &Page, from: (f64, f64), to: (f64, f64)) -> Result<()> {
    mouse(page, "mouseMoved", from.0, from.1, json!({})).await?;
    mouse(page, "mousePressed", from.0, from.1, json!({ "button": "left", "clickCount": 1, "buttons": 1 })).await?;
    const STEPS: u32 = 12;
    for step in 1..=STEPS {
        let t = step as f64 / STEPS as f64;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        mouse(page, "mouseMoved", x, y, json!({ "button": "left", "buttons": 1 })).await?;
        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
    }
    mouse(page, "mouseReleased", to.0, to.1, json!({ "button": "left", "clickCount": 1, "buttons": 0 })).await
}

pub async fn wheel(page: &Page, x: f64, y: f64, delta_x: f64, delta_y: f64) -> Result<()> {
    mouse(page, "mouseWheel", x, y, json!({ "deltaX": delta_x, "deltaY": delta_y })).await
}

/// Focuses the node, clears it when asked, and inserts `text` the way an
/// IME would (one `input` event, fine for React and friends).
pub async fn type_into(page: &Page, target: Option<i64>, text: &str, clear: bool) -> Result<()> {
    if let Some(node) = target {
        let (x, y) = element_point(page, node, true).await?;
        // A click focuses like a person would and places the caret.
        click(page, x, y, Button::Left, 1, 0).await?;
        let _ = cdp::call(page, "DOM.focus", json!({ "backendNodeId": node })).await;
        if clear {
            cdp::call_on_node(
                page,
                node,
                "function() { if (typeof this.select === 'function') { this.select(); } else if (this.isContentEditable) { const range = document.createRange(); range.selectNodeContents(this); const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range); } }",
                &[],
            )
            .await?;
            press(page, "Backspace", 0).await?;
        }
    } else if clear {
        press_chord(page, "ctrl+a").await?;
        press(page, "Backspace", 0).await?;
    }
    if !text.is_empty() {
        cdp::call(page, "Input.insertText", json!({ "text": text })).await?;
    }
    Ok(())
}

/// Presses keys: `Enter`, `ctrl+a`, or a space-separated sequence like
/// `Tab Tab Enter`.
pub async fn keys(page: &Page, sequence: &str) -> Result<()> {
    for chord in sequence.split_whitespace() {
        press_chord(page, chord).await?;
    }
    Ok(())
}

async fn press_chord(page: &Page, chord: &str) -> Result<()> {
    let parts: Vec<String> = chord.split('+').map(str::to_string).collect();
    let (key, modifiers) = match parts.split_last() {
        Some((key, mods)) if !key.is_empty() => (key.clone(), modifier_bits(mods)),
        // "ctrl++" → the plus key.
        _ => ("+".to_string(), modifier_bits(&parts[..parts.len().saturating_sub(2)])),
    };
    press(page, &key, modifiers).await
}

pub async fn press(page: &Page, key: &str, modifiers: i64) -> Result<()> {
    let info = key_info(key).ok_or_else(|| anyhow!("unknown key `{key}`"))?;
    // Text is only produced without ctrl/meta (ctrl+a selects, it doesn't type).
    let text = if modifiers & (2 | 4) == 0 {
        info.text.map(|text| {
            if modifiers & 8 != 0 {
                text.to_uppercase()
            } else {
                text
            }
        })
    } else {
        None
    };
    let mut down = json!({
        "type": if text.is_some() { "keyDown" } else { "rawKeyDown" },
        "key": info.key,
        "code": info.code,
        "windowsVirtualKeyCode": info.key_code,
        "nativeVirtualKeyCode": info.key_code,
        "modifiers": modifiers,
    });
    if let Some(text) = &text {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    // Let editing shortcuts act on text fields.
    if modifiers & (2 | 4) != 0 {
        if let Some(command) = editing_command(&info.key.to_lowercase()) {
            down["commands"] = json!([command]);
        }
    }
    cdp::call(page, "Input.dispatchKeyEvent", down).await?;
    cdp::call(
        page,
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": info.key,
            "code": info.code,
            "windowsVirtualKeyCode": info.key_code,
            "nativeVirtualKeyCode": info.key_code,
            "modifiers": modifiers,
        }),
    )
    .await?;
    Ok(())
}

fn editing_command(key: &str) -> Option<&'static str> {
    match key {
        "a" => Some("selectAll"),
        "c" => Some("copy"),
        "x" => Some("cut"),
        "v" => Some("paste"),
        "z" => Some("undo"),
        "y" => Some("redo"),
        _ => None,
    }
}

struct KeyInfo {
    key: String,
    code: String,
    key_code: i64,
    text: Option<String>,
}

fn key_info(name: &str) -> Option<KeyInfo> {
    let named = |key: &str, code: &str, key_code: i64, text: Option<&str>| {
        Some(KeyInfo {
            key: key.into(),
            code: code.into(),
            key_code,
            text: text.map(str::to_string),
        })
    };
    let lower = name.to_lowercase();
    match lower.as_str() {
        "enter" | "return" => return named("Enter", "Enter", 13, Some("\r")),
        "tab" => return named("Tab", "Tab", 9, None),
        "escape" | "esc" => return named("Escape", "Escape", 27, None),
        "backspace" => return named("Backspace", "Backspace", 8, None),
        "delete" | "del" => return named("Delete", "Delete", 46, None),
        "space" | "spacebar" => return named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => return named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => return named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => return named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => return named("ArrowRight", "ArrowRight", 39, None),
        "home" => return named("Home", "Home", 36, None),
        "end" => return named("End", "End", 35, None),
        "pageup" => return named("PageUp", "PageUp", 33, None),
        "pagedown" => return named("PageDown", "PageDown", 34, None),
        "insert" => return named("Insert", "Insert", 45, None),
        "+" | "plus" => return named("+", "Equal", 187, Some("+")),
        "-" | "minus" => return named("-", "Minus", 189, Some("-")),
        _ => {}
    }
    if let Some(number) = lower.strip_prefix('f').and_then(|n| n.parse::<i64>().ok()) {
        if (1..=12).contains(&number) {
            return named(&format!("F{number}"), &format!("F{number}"), 111 + number, None);
        }
    }
    let mut chars = name.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if ch.is_ascii_alphabetic() {
        let upper = ch.to_ascii_uppercase();
        return named(
            &ch.to_ascii_lowercase().to_string(),
            &format!("Key{upper}"),
            upper as i64,
            Some(&ch.to_ascii_lowercase().to_string()),
        );
    }
    if ch.is_ascii_digit() {
        return named(&ch.to_string(), &format!("Digit{ch}"), ch as i64, Some(&ch.to_string()));
    }
    // Any other single character: typed as is.
    named(&ch.to_string(), "", 0, Some(&ch.to_string()))
}

/// Sets a form control's value directly: `<select>` (by value or label),
/// checkbox / radio / switch (`true`/`false`), and other inputs (date,
/// range, color, text…), firing the events frameworks listen to.
pub async fn set_value(page: &Page, node: i64, value: &Value) -> Result<String> {
    let result = cdp::call_on_node(
        page,
        node,
        r#"function(value) {
  const fire = (el) => {
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };
  const el = this;
  const tag = (el.tagName || '').toLowerCase();
  if (tag === 'select') {
    const wanted = Array.isArray(value) ? value.map(String) : [String(value)];
    let matched = 0;
    for (const option of el.options) {
      const hit = wanted.includes(option.value) || wanted.includes(option.label) || wanted.includes(option.text.trim());
      if (el.multiple) { option.selected = hit; } else if (hit && !matched) { el.value = option.value; }
      if (hit) matched += 1;
    }
    if (!matched) return 'error: no option matches ' + JSON.stringify(value) + '; options: ' + Array.from(el.options).slice(0, 30).map((o) => o.label || o.value).join(' | ');
    fire(el);
    return 'selected ' + Array.from(el.selectedOptions).map((o) => o.label || o.value).join(', ');
  }
  if (tag === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {
    const want = value === true || value === 'true' || value === 'on' || value === 1;
    if (el.checked !== want) el.click();
    return (el.checked ? 'checked' : 'unchecked');
  }
  const role = el.getAttribute && el.getAttribute('role');
  if (role === 'checkbox' || role === 'switch' || role === 'radio') {
    const want = value === true || value === 'true';
    const now = el.getAttribute('aria-checked') === 'true';
    if (now !== want) el.click();
    return el.getAttribute('aria-checked') === 'true' ? 'checked' : 'unchecked';
  }
  if (tag === 'input' || tag === 'textarea') {
    const proto = tag === 'input' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, 'value').set;
    el.focus();
    setter.call(el, String(value));
    fire(el);
    return 'value set to ' + JSON.stringify(el.value);
  }
  if (el.isContentEditable) {
    el.focus();
    el.textContent = String(value);
    fire(el);
    return 'text set';
  }
  return 'error: not a form control (' + tag + '); use browser_click or browser_type';
}"#,
        &[value.clone()],
    )
    .await?;
    let message = result.as_str().unwrap_or("done").to_string();
    if let Some(error) = message.strip_prefix("error: ") {
        bail!("{error}");
    }
    Ok(message)
}

/// Sets the files of an `<input type=file>`.
pub async fn upload(page: &Page, node: i64, files: &[String]) -> Result<()> {
    cdp::call(
        page,
        "DOM.setFileInputFiles",
        json!({ "files": files, "backendNodeId": node }),
    )
    .await?;
    Ok(())
}
