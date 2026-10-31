//! Plain-text rendering of a Sinew history, for a Claude Code session that
//! starts without one (first Claude turn after another model, a rewind, a
//! compaction) and for tool-less requests such as compaction itself.

use sinew_core::{ChatMessage, Part, Role};

/// Longest tool input or output kept per call.
const TOOL_TEXT_LIMIT: usize = 2_000;

/// `messages` as tagged text, oldest first. Over `max_chars`, the oldest
/// messages are dropped and a note says so.
pub fn render_transcript(messages: &[ChatMessage], max_chars: usize) -> String {
    let mut blocks: Vec<String> = messages.iter().filter_map(render_message).collect();
    let mut total: usize = blocks.iter().map(|block| block.len() + 1).sum();
    let mut dropped = 0usize;
    while total > max_chars && blocks.len() > 1 {
        total -= blocks.remove(0).len() + 1;
        dropped += 1;
    }
    let mut out = String::new();
    if dropped > 0 {
        out.push_str(&format!(
            "[{dropped} earlier messages omitted for length]\n"
        ));
    }
    out.push_str(&blocks.join("\n"));
    out
}

fn render_message(message: &ChatMessage) -> Option<String> {
    let mut lines = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text { text, .. } if !text.trim().is_empty() => lines.push(text.trim().to_string()),
            Part::Image { .. } => lines.push("[image]".into()),
            Part::ToolCall { name, input, .. } => lines.push(format!(
                "[tool call {name}: {}]",
                clip(&input.to_string(), TOOL_TEXT_LIMIT)
            )),
            Part::ToolResult {
                content, is_error, ..
            } => lines.push(format!(
                "[tool {}: {}]",
                if *is_error { "error" } else { "result" },
                clip(content.trim(), TOOL_TEXT_LIMIT)
            )),
            _ => {}
        }
    }
    if lines.is_empty() {
        return None;
    }
    let tag = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    Some(format!("<{tag}>\n{}\n</{tag}>", lines.join("\n")))
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(role: Role, text: &str) -> ChatMessage {
        ChatMessage {
            role,
            parts: vec![Part::Text {
                text: text.into(),
                meta: None,
            }],
        }
    }

    #[test]
    fn renders_roles_and_tool_calls() {
        let history = vec![
            text(Role::User, "fix the bug"),
            ChatMessage {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "t1".into(),
                    name: "read".into(),
                    input: json!({ "path": "src/main.rs" }),
                    meta: None,
                }],
            },
        ];
        let out = render_transcript(&history, 10_000);
        assert!(out.starts_with("<user>\nfix the bug\n</user>"));
        assert!(out.contains("[tool call read: {\"path\":\"src/main.rs\"}]"));
    }

    #[test]
    fn drops_the_oldest_messages_first() {
        let history = vec![text(Role::User, &"a".repeat(50)), text(Role::Assistant, "recent")];
        let out = render_transcript(&history, 40);
        assert!(out.starts_with("[1 earlier messages omitted for length]"));
        assert!(out.contains("recent"));
    }
}
