use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Value};

use sinew_core::{ChatMessage, Part, Role};

use crate::{ReadFingerprint, ReadTool};

use super::clean_context::tool_result_cleaned;

pub(super) fn tool_result_content_with_id(tool_call_id: &str, content: &str) -> String {
    format!("tool_call_id: {tool_call_id}\n{content}")
}

pub(super) fn history_with_current_tool_result_ids(
    history: &[ChatMessage],
    current_turn_tool_result_ids: &BTreeSet<String>,
) -> Vec<ChatMessage> {
    let mut history = history.to_vec();
    if current_turn_tool_result_ids.is_empty() {
        return history;
    }

    for message in &mut history {
        for part in &mut message.parts {
            let Part::ToolResult {
                tool_call_id,
                content,
                meta,
                ..
            } = part
            else {
                continue;
            };
            if !current_turn_tool_result_ids.contains(tool_call_id) || tool_result_cleaned(meta) {
                continue;
            }
            let stripped = strip_visible_tool_result_id(content);
            *content = tool_result_content_with_id(tool_call_id, &stripped);
        }
    }

    history
}

pub(super) fn strip_all_visible_tool_result_ids(history: &mut [ChatMessage]) {
    for message in history {
        for part in &mut message.parts {
            let Part::ToolResult { content, .. } = part else {
                continue;
            };
            *content = strip_visible_tool_result_id(content);
        }
    }
}

pub(super) fn normalize_tool_call_inputs(history: &mut [ChatMessage]) {
    for message in history {
        for part in &mut message.parts {
            let Part::ToolCall { input, .. } = part else {
                continue;
            };
            let normalized = normalize_tool_call_input(std::mem::take(input));
            *input = normalized;
        }
    }
}

pub(super) fn normalize_tool_call_input(input: Value) -> Value {
    match input {
        Value::Object(_) => input,
        Value::Null => json!({}),
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                json!({})
            } else {
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(Value::Object(map)) => Value::Object(map),
                    Ok(value) => json!({ "value": value }),
                    Err(_) => json!({ "value": raw }),
                }
            }
        }
        value => json!({ "value": value }),
    }
}

/// Makes every tool call answered right away, as all providers require: the
/// message after an assistant message with tool calls holds exactly their
/// results, in call order. Results spread over several messages or placed
/// after a user's text are moved up, a missing one gets an "interrupted"
/// result, and a result with no call before it is dropped. A history mixes
/// turns from several harnesses (Claude Code, other models, interrupted
/// runs), so this runs before every request, not only when a turn starts.
pub(super) fn repair_missing_tool_results(history: &mut Vec<ChatMessage>) {
    let mut repaired = Vec::with_capacity(history.len());
    let mut index = 0usize;
    while index < history.len() {
        let message = &history[index];
        index += 1;
        if message.role == Role::User {
            // Results here have no call right before them.
            let mut message = message.clone();
            message.parts.retain(|part| !matches!(part, Part::ToolResult { .. }));
            if !message.parts.is_empty() {
                repaired.push(message);
            }
            continue;
        }
        repaired.push(message.clone());
        let ids = tool_call_ids(message);
        if ids.is_empty() {
            continue;
        }
        // The user messages up to the next assistant message answer it.
        let mut results: HashMap<String, Part> = HashMap::new();
        let mut followers = Vec::new();
        while let Some(next) = history.get(index).filter(|next| next.role == Role::User) {
            let mut rest = next.clone();
            rest.parts.retain(|part| match part {
                Part::ToolResult { tool_call_id, .. } if ids.contains(tool_call_id) => {
                    results.entry(tool_call_id.clone()).or_insert_with(|| part.clone());
                    false
                }
                // A result for no call of this message is invalid anywhere.
                Part::ToolResult { .. } => false,
                _ => true,
            });
            if !rest.parts.is_empty() {
                followers.push(rest);
            }
            index += 1;
        }
        repaired.push(ChatMessage {
            role: Role::User,
            parts: ids
                .into_iter()
                .map(|id| {
                    results.remove(&id).unwrap_or_else(|| {
                        interrupted_tool_result(id, "tool call was interrupted before a result was saved")
                    })
                })
                .collect(),
        });
        repaired.extend(followers);
    }
    if repaired.len() != history.len()
        || repaired
            .iter()
            .zip(history.iter())
            .any(|(left, right)| !same_message(left, right))
    {
        *history = repaired;
    }
}

fn same_message(left: &ChatMessage, right: &ChatMessage) -> bool {
    left.role == right.role
        && serde_json::to_value(&left.parts).ok() == serde_json::to_value(&right.parts).ok()
}

pub(super) fn append_interrupted_tool_results(
    assistant: &ChatMessage,
    tool_results: &mut Vec<Part>,
) {
    let completed = tool_results
        .iter()
        .filter_map(|part| match part {
            Part::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for id in tool_call_ids(assistant) {
        if completed.contains(&id) {
            continue;
        }
        tool_results.push(interrupted_tool_result(id, "tool call interrupted by user"));
    }
}

fn tool_call_ids(message: &ChatMessage) -> Vec<String> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::ToolCall { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(message: &ChatMessage) -> BTreeSet<String> {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

fn interrupted_tool_result(id: String, content: &'static str) -> Part {
    Part::ToolResult {
        tool_call_id: id,
        content: content.to_string(),
        images: Vec::new(),
        is_error: true,
        meta: Some(json!({ "interrupted": true })),
    }
}

fn strip_visible_tool_result_id(content: &str) -> String {
    let Some(rest) = content.strip_prefix("tool_call_id:") else {
        return content.to_string();
    };
    let Some(newline_index) = rest.find('\n') else {
        return String::new();
    };
    rest[newline_index + 1..].to_string()
}

#[cfg(test)]
pub(super) fn tool_result_exposes_id(content: &str) -> bool {
    content.starts_with("tool_call_id:")
}

pub(super) fn successful_read_fingerprints(
    history: &[ChatMessage],
    read: &ReadTool,
) -> HashMap<String, ReadFingerprint> {
    let mut pending_reads = HashMap::new();
    let mut successful = HashMap::new();

    for message in history {
        match message.role {
            Role::Assistant => {
                for part in &message.parts {
                    let Part::ToolCall {
                        id, name, input, ..
                    } = part
                    else {
                        continue;
                    };
                    if name == "read" {
                        let Some(path) = input.get("path").and_then(|value| value.as_str()) else {
                            continue;
                        };
                        if let Ok(normalized) = read.normalize_path(path) {
                            pending_reads.insert(id.clone(), Some(normalized));
                        }
                    } else if name == "edit_file" || name == "write_file" {
                        pending_reads.insert(id.clone(), None);
                    }
                }
            }
            Role::User => {
                for part in &message.parts {
                    let Part::ToolResult {
                        tool_call_id,
                        is_error,
                        meta,
                        ..
                    } = part
                    else {
                        continue;
                    };
                    let Some(pending_path) = pending_reads.remove(tool_call_id) else {
                        continue;
                    };
                    if *is_error || tool_result_cleaned(meta) {
                        continue;
                    }
                    let Some(meta) = meta else {
                        continue;
                    };
                    if let Some(fingerprint) = read_fingerprint_from_meta(meta) {
                        let path =
                            pending_path.unwrap_or_else(|| fingerprint.relative_path.clone());
                        successful.insert(path, fingerprint);
                    }
                    for fingerprint in read_fingerprints_from_meta(meta) {
                        successful.insert(fingerprint.relative_path.clone(), fingerprint);
                    }
                }
            }
        }
    }

    successful
}

fn read_fingerprint_from_meta(meta: &Value) -> Option<ReadFingerprint> {
    meta.get("read_fingerprint")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

fn read_fingerprints_from_meta(meta: &Value) -> Vec<ReadFingerprint> {
    meta.get("read_fingerprints")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| serde_json::from_value(value.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
