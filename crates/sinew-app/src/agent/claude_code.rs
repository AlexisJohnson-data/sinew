//! Agent turns with a Claude model, run by the user's own Claude Code.
//!
//! Sinew starts the unmodified `claude` binary in stream-json mode (the Agent
//! SDK protocol) and Claude Code runs the whole turn with its own tools and
//! its own sign-in. Sinew renders the stream, answers Claude Code's
//! permission prompts with its approval cards and permission rules, shows
//! `AskUserQuestion` as its question card, and lends its browser through an
//! in-process MCP server. The Claude Code session id is kept on the turn's
//! last assistant message so the next turn resumes it.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use sinew_anthropic::{effort_flag, render_transcript, usage_from, ClaudeProcess, ProcessSender};
use sinew_core::{ChatMessage, Part, Role, ToolResultImage, Usage};
use tokio::sync::mpsc;

use crate::permissions::{self, Decision};
use crate::todo::{TodoStatus, TodoTask};
use crate::tool_run::{DiffLine, DiffLineKind, FileChange, FileChangeKind, ToolRunImage};
use crate::{tool_names, BrowserTools, QuestionTool, TodoListState, ToolSettings};

use super::cancel::{ApprovalReply, EngineCommand, TurnCancel};
use super::context::{AgentMode, TurnContext, TurnOutput};
use super::events::{send_event, send_token_usage_event, AgentEvent, AgentEventScope};
use super::tool_summary::summarize_tool;

pub(super) const PROVIDER: &str = "anthropic";
/// Key of the session record in a part's `meta`.
const SESSION_META: &str = "claude_code";
const SINEW_MCP: &str = "sinew";
const SINEW_MCP_PREFIX: &str = "mcp__sinew__";
/// Longest earlier conversation replayed into a new Claude Code session.
const TRANSCRIPT_LIMIT: usize = 400_000;
/// How long an interrupted turn may take to wind down before it is killed.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Where the previous Claude Code session left off.
struct SessionRecord {
    session_id: String,
    message_uuid: Option<String>,
    conversation: Option<String>,
}

pub(super) async fn run_claude_code_turn(ctx: TurnContext) -> TurnOutput {
    let TurnContext {
        provider,
        model,
        cache_key,
        mode,
        mut history,
        mut todo_list,
        goal_workflow,
        bash,
        browser,
        question,
        tool_settings,
        event_scope,
        event_tx,
        cancel,
        mut cmd_rx,
        ..
    } = ctx;
    let scope = event_scope.as_ref();
    send_event(&event_tx, scope, AgentEvent::TurnStarted);
    let started = Instant::now();
    let root = bash.workspace_root().to_path_buf();

    let (record, prompt_start) = resume_point(&history);
    let content = user_content(&history, record.is_none().then_some(prompt_start), prompt_start);
    let browser_on = tool_settings.browser_enabled && mode != AgentMode::Plan;

    let mut args = vec![
        "--include-partial-messages".to_string(),
        "--permission-prompt-tool".into(),
        "stdio".into(),
        "--model".into(),
        model.name.clone(),
        "--permission-mode".into(),
        if mode == AgentMode::Plan { "plan" } else { "default" }.into(),
    ];
    if let Some(effort) = effort_flag(model.effort) {
        args.extend(["--effort".into(), effort.into()]);
    }
    if let Some(record) = &record {
        args.extend(["--resume".into(), record.session_id.clone()]);
        if let Some(uuid) = &record.message_uuid {
            args.extend(["--resume-session-at".into(), uuid.clone()]);
        }
        // A forked conversation continues a copy, leaving the original's
        // session as it was.
        if record.conversation.is_some() && record.conversation != cache_key {
            args.push("--fork-session".into());
        }
    }
    if browser_on {
        args.extend([
            "--mcp-config".into(),
            json!({ "mcpServers": { SINEW_MCP: { "type": "sdk", "name": SINEW_MCP } } })
                .to_string(),
            "--append-system-prompt".into(),
            super::turn::BROWSER_TOOLS_PROMPT
                .replace("browser_* tools", "mcp__sinew__browser_* tools")
                .trim()
                .to_string(),
        ]);
    }

    let mut turn = Turn {
        model_name: model.name.clone(),
        event_tx: event_tx.clone(),
        scope: event_scope.clone(),
        root: root.clone(),
        history_start: history.len(),
        current: None,
        streamed: Vec::new(),
        blocks: Vec::new(),
        call_usage: Usage::default(),
        session_id: record.as_ref().map(|record| record.session_id.clone()),
        message_uuid: None,
        failed: false,
    };

    let mut process = match ClaudeProcess::spawn(&args, &root) {
        Ok(process) => process,
        Err(err) => {
            send_event(&event_tx, scope, AgentEvent::Error { message: err.to_string() });
            return finish(history, todo_list, goal_workflow, false, &event_tx, scope, started);
        }
    };
    let sender = process.sender();
    let init = if browser_on {
        json!({ "sdkMcpServers": [SINEW_MCP] })
    } else {
        json!({})
    };
    if let Err(err) = process.initialize(init).await {
        let detail = process.stderr_tail();
        send_event(
            &event_tx,
            scope,
            AgentEvent::Error {
                message: format!("{err}{}", if detail.is_empty() { String::new() } else { format!("\n{detail}") }),
            },
        );
        process.shutdown(Duration::from_secs(1)).await;
        return finish(history, todo_list, goal_workflow, false, &event_tx, scope, started);
    }
    sender.send(&json!({
        "type": "user",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": "",
    }));

    let helpers = Helpers {
        sender: sender.clone(),
        event_tx: event_tx.clone(),
        cancel: cancel.clone(),
        root: root.clone(),
        tool_settings: tool_settings.clone(),
        question: question.clone(),
        browser: browser.clone(),
    };

    let mut interrupted = false;
    let mut deadline: Option<tokio::time::Instant> = None;
    let mut got_result = false;
    loop {
        let message = tokio::select! {
            biased;
            command = cmd_rx.recv(), if !interrupted => {
                if matches!(command, Some(EngineCommand::Cancel)) {
                    interrupted = true;
                    deadline = Some(tokio::time::Instant::now() + INTERRUPT_GRACE);
                    let sender = sender.clone();
                    tokio::spawn(async move {
                        let _ = sender.request(json!({ "subtype": "interrupt" }), REQUEST_TIMEOUT).await;
                    });
                }
                continue;
            }
            _ = sleep_until(deadline) => break,
            message = process.next_message() => message,
        };
        let Some(message) = message else { break };
        match message["type"].as_str() {
            Some("control_request") => helpers.answer(message),
            Some("result") => {
                turn.on_result(&message, &provider, &model, interrupted);
                got_result = true;
                break;
            }
            _ => turn.on_message(&message, &mut history, &mut todo_list),
        }
    }

    if got_result && !interrupted {
        // Refresh the plan pill from the same process: no extra start-up.
        if let Ok(usage) = sender
            .request(json!({ "subtype": "get_usage", "skip_behaviors": true }), REQUEST_TIMEOUT)
            .await
        {
            sinew_anthropic::record_usage(&usage);
        }
    }
    if !got_result && !interrupted && !turn.failed {
        let detail = process.stderr_tail();
        send_event(
            &event_tx,
            scope,
            AgentEvent::Error {
                message: if detail.trim().is_empty() {
                    "Claude Code stopped before finishing the turn".into()
                } else {
                    format!("Claude Code stopped before finishing the turn:\n{detail}")
                },
            },
        );
    }
    turn.flush(&mut history);
    turn.record_session(&mut history, cache_key.as_deref());
    drop(helpers);
    drop(sender);
    process.shutdown(Duration::from_secs(3)).await;
    if interrupted {
        send_event(&event_tx, scope, AgentEvent::Interrupted);
    }
    finish(history, todo_list, goal_workflow, interrupted, &event_tx, scope, started)
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn finish(
    history: Vec<ChatMessage>,
    mut todo_list: TodoListState,
    goal_workflow: crate::GoalWorkflowState,
    interrupted: bool,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    scope: Option<&AgentEventScope>,
    started: Instant,
) -> TurnOutput {
    send_event(
        event_tx,
        scope,
        AgentEvent::TurnFinished {
            duration_ms: Some(started.elapsed().as_millis() as i64),
        },
    );
    todo_list.normalize();
    TurnOutput {
        history,
        todo_list,
        goal_workflow,
        interrupted,
        compacted: false,
    }
}

/// The session to resume, and where the new user input starts: the
/// messages after the last assistant message that carries a session record.
fn resume_point(history: &[ChatMessage]) -> (Option<SessionRecord>, usize) {
    let last_assistant = history
        .iter()
        .rposition(|message| message.role == Role::Assistant);
    let Some(index) = last_assistant else {
        return (None, 0);
    };
    let record = history[index].parts.iter().find_map(|part| {
        let meta = part_meta(part)?.get(SESSION_META)?;
        Some(SessionRecord {
            session_id: meta.get("session_id")?.as_str()?.to_string(),
            message_uuid: meta.get("message_uuid").and_then(Value::as_str).map(str::to_string),
            conversation: meta.get("conversation").and_then(Value::as_str).map(str::to_string),
        })
    });
    let start = history[index..]
        .iter()
        .position(|message| message.role == Role::User && has_user_input(message))
        .map_or(history.len(), |offset| index + offset);
    match record {
        Some(record) => (Some(record), start),
        // No session to resume: everything up to the new input is replayed.
        None => {
            let start = history
                .iter()
                .rposition(|message| message.role == Role::Assistant)
                .map_or(0, |index| index + 1);
            (None, start)
        }
    }
}

fn has_user_input(message: &ChatMessage) -> bool {
    message
        .parts
        .iter()
        .any(|part| matches!(part, Part::Text { .. } | Part::Image { .. }))
}

fn part_meta(part: &Part) -> Option<&Value> {
    match part {
        Part::Text { meta, .. }
        | Part::Image { meta, .. }
        | Part::Thinking { meta, .. }
        | Part::ToolCall { meta, .. }
        | Part::ToolResult { meta, .. } => meta.as_ref(),
    }
}

/// Content blocks of the user message sent to Claude Code: the new input,
/// preceded by the earlier conversation when a new session starts.
fn user_content(history: &[ChatMessage], replay_before: Option<usize>, start: usize) -> Vec<Value> {
    let mut content = Vec::new();
    if let Some(end) = replay_before {
        let earlier = &history[..end.min(history.len())];
        if !earlier.is_empty() {
            content.push(json!({
                "type": "text",
                "text": format!(
                    "<earlier_conversation>\n{}\n</earlier_conversation>\n\nThis conversation started before this session, possibly with another model. Continue it; the user's new message follows.",
                    render_transcript(earlier, TRANSCRIPT_LIMIT)
                ),
            }));
        }
    }
    for message in history.iter().skip(start) {
        if message.role != Role::User {
            continue;
        }
        for part in &message.parts {
            match part {
                Part::Text { text, .. } if !text.trim().is_empty() => {
                    content.push(json!({ "type": "text", "text": text }));
                }
                Part::Image {
                    media_type, data, ..
                } => content.push(json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                })),
                _ => {}
            }
        }
    }
    if content.is_empty() {
        content.push(json!({ "type": "text", "text": "Continue." }));
    }
    content
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Text,
    Thinking,
    Other,
}

/// Translates Claude Code's stream into Sinew events and history.
struct Turn {
    model_name: String,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    scope: Option<AgentEventScope>,
    root: std::path::PathBuf,
    history_start: usize,
    /// The assistant message being assembled, with its API message id.
    current: Option<(String, ChatMessage)>,
    /// API message ids whose content arrived as stream events.
    streamed: Vec<String>,
    /// Open stream blocks, by index.
    blocks: Vec<Block>,
    /// Usage of the latest API call, for the context meter.
    call_usage: Usage,
    session_id: Option<String>,
    message_uuid: Option<String>,
    failed: bool,
}

impl Turn {
    fn emit(&self, event: AgentEvent) {
        send_event(&self.event_tx, self.scope.as_ref(), event);
    }

    fn on_message(&mut self, message: &Value, history: &mut Vec<ChatMessage>, todo: &mut TodoListState) {
        if let Some(session_id) = message["session_id"].as_str().filter(|id| !id.is_empty()) {
            self.session_id = Some(session_id.to_string());
        }
        // Subagent traffic stays inside its Task card.
        if !message["parent_tool_use_id"].is_null() {
            return;
        }
        match message["type"].as_str() {
            Some("stream_event") => self.on_stream_event(&message["event"]),
            Some("assistant") => self.on_assistant(message, history),
            Some("user") => self.on_tool_results(message, history, todo),
            _ => {}
        }
    }

    fn on_stream_event(&mut self, event: &Value) {
        match event["type"].as_str() {
            Some("message_start") => {
                if let Some(id) = event["message"]["id"].as_str() {
                    self.streamed.push(id.to_string());
                }
                self.call_usage = usage_from(&event["message"]["usage"]);
                self.blocks.clear();
            }
            Some("content_block_start") => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                let block = &event["content_block"];
                let kind = match block["type"].as_str() {
                    Some("text") => {
                        self.emit(AgentEvent::TextStarted);
                        Block::Text
                    }
                    Some("thinking") => {
                        self.emit(AgentEvent::ThinkingStarted);
                        Block::Thinking
                    }
                    Some("tool_use") | Some("server_tool_use") => {
                        let name = block["name"].as_str().unwrap_or_default();
                        self.emit(AgentEvent::ToolStarted {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: map_tool(name, &Value::Null).0,
                        });
                        Block::Other
                    }
                    _ => Block::Other,
                };
                if self.blocks.len() <= index {
                    self.blocks.resize(index + 1, Block::Other);
                }
                self.blocks[index] = kind;
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => self.emit(AgentEvent::TextChunk {
                        delta: delta["text"].as_str().unwrap_or_default().to_string(),
                    }),
                    Some("thinking_delta") => self.emit(AgentEvent::ThinkingChunk {
                        delta: delta["thinking"].as_str().unwrap_or_default().to_string(),
                    }),
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                match self.blocks.get(index).copied() {
                    Some(Block::Text) => self.emit(AgentEvent::TextFinished),
                    Some(Block::Thinking) => self.emit(AgentEvent::ThinkingFinished),
                    _ => {}
                }
            }
            Some("message_delta") => {
                let output = event["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
                if output > 0 {
                    self.call_usage.output_tokens = output;
                    self.call_usage.total_tokens = self
                        .call_usage
                        .input_tokens
                        .saturating_add(self.call_usage.cache_read_tokens)
                        .saturating_add(self.call_usage.cache_creation_tokens)
                        .saturating_add(output);
                }
            }
            _ => {}
        }
    }

    fn on_assistant(&mut self, message: &Value, history: &mut Vec<ChatMessage>) {
        let body = &message["message"];
        if let Some(uuid) = message["uuid"].as_str() {
            self.message_uuid = Some(uuid.to_string());
        }
        let id = body["id"].as_str().unwrap_or_default().to_string();
        let streamed = !id.is_empty() && self.streamed.contains(&id);
        if message.get("error").is_some_and(|error| !error.is_null()) {
            // Synthetic message for a failed request (sign-in, rate limit…).
            let text = content_text(&body["content"]);
            self.failed = true;
            self.emit(AgentEvent::Error {
                message: describe_error(message["error"].as_str().unwrap_or_default(), &text),
            });
            return;
        }
        if self.current.as_ref().map(|(current, _)| current) != Some(&id) {
            self.flush(history);
            self.current = Some((
                id,
                ChatMessage {
                    role: Role::Assistant,
                    parts: Vec::new(),
                },
            ));
        }
        for block in body["content"].as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default().to_string();
                    if !streamed {
                        self.emit(AgentEvent::TextStarted);
                        self.emit(AgentEvent::TextChunk { delta: text.clone() });
                        self.emit(AgentEvent::TextFinished);
                    }
                    self.push_part(Part::Text { text, meta: None });
                }
                Some("thinking") => {
                    let text = block["thinking"].as_str().unwrap_or_default().to_string();
                    if !text.is_empty() {
                        self.push_part(Part::Thinking { text, meta: None });
                    }
                }
                Some("tool_use") | Some("server_tool_use") => self.on_tool_use(block, streamed),
                _ => {}
            }
        }
    }

    fn on_tool_use(&mut self, block: &Value, streamed: bool) {
        let id = block["id"].as_str().unwrap_or_default().to_string();
        let original = block["name"].as_str().unwrap_or_default();
        let input = block.get("input").cloned().unwrap_or(Value::Null);
        // A plan in plan mode is the turn's answer: show it as text.
        if original == "ExitPlanMode" {
            if let Some(plan) = input["plan"].as_str().filter(|plan| !plan.trim().is_empty()) {
                self.emit(AgentEvent::TextStarted);
                self.emit(AgentEvent::TextChunk { delta: plan.to_string() });
                self.emit(AgentEvent::TextFinished);
                self.push_part(Part::Text {
                    text: plan.to_string(),
                    meta: None,
                });
            }
            return;
        }
        let (name, mapped) = map_tool(original, &input);
        if !streamed {
            self.emit(AgentEvent::ToolStarted {
                id: id.clone(),
                name: name.clone(),
            });
        }
        self.emit(AgentEvent::ToolReady {
            id: id.clone(),
            summary: summarize_tool(&name, &mapped),
            args_pretty: serde_json::to_string_pretty(&mapped).unwrap_or_default(),
        });
        self.push_part(Part::ToolCall {
            id,
            name,
            input: mapped,
            meta: Some(json!({ "claude_code_tool": original })),
        });
    }

    fn on_tool_results(&mut self, message: &Value, history: &mut Vec<ChatMessage>, todo: &mut TodoListState) {
        let Some(blocks) = message["message"]["content"].as_array() else {
            return;
        };
        let mut parts = Vec::new();
        for block in blocks {
            if block["type"].as_str() != Some("tool_result") {
                continue;
            }
            let id = block["tool_use_id"].as_str().unwrap_or_default().to_string();
            let call = self.tool_call(&id, history);
            let Some((name, input)) = call else {
                continue;
            };
            let is_error = block["is_error"].as_bool().unwrap_or(false);
            let (output, images) = result_content(&block["content"]);
            let mut meta = Map::new();
            let mut file_changes = Vec::new();
            if !is_error {
                if let Some(change) = file_change(&self.root, &message["tool_use_result"]) {
                    file_changes.push(change);
                }
                if name == tool_names::TODO_LIST {
                    if let Some(state) = todo_state(&input) {
                        *todo = state;
                        meta.insert("todo_list".into(), json!(todo));
                    }
                }
            }
            if !file_changes.is_empty() {
                meta.insert("file_changes".into(), json!(file_changes));
            }
            let meta = (!meta.is_empty()).then_some(Value::Object(meta));
            self.emit(AgentEvent::ToolFinished {
                id: id.clone(),
                output: output.clone(),
                is_error,
                file_changes,
                images: images.clone(),
                meta: meta.clone(),
            });
            parts.push(Part::ToolResult {
                tool_call_id: id,
                content: output,
                images: images
                    .into_iter()
                    .map(|image| ToolResultImage {
                        media_type: image.media_type,
                        data: image.data,
                        path: image.path,
                    })
                    .collect(),
                is_error,
                meta,
            });
        }
        if parts.is_empty() {
            return;
        }
        self.flush(history);
        history.push(ChatMessage {
            role: Role::User,
            parts,
        });
    }

    /// Mapped name and input of the tool call `id`, from this turn.
    fn tool_call(&self, id: &str, history: &[ChatMessage]) -> Option<(String, Value)> {
        let current = self.current.iter().map(|(_, message)| message);
        history[self.history_start.min(history.len())..]
            .iter()
            .chain(current)
            .flat_map(|message| &message.parts)
            .find_map(|part| match part {
                Part::ToolCall {
                    id: call, name, input, ..
                } if call == id => Some((name.clone(), input.clone())),
                _ => None,
            })
    }

    fn on_result(
        &mut self,
        message: &Value,
        provider: &Arc<dyn sinew_core::Provider>,
        model: &sinew_core::ModelRef,
        interrupted: bool,
    ) {
        if let Some(session_id) = message["session_id"].as_str().filter(|id| !id.is_empty()) {
            self.session_id = Some(session_id.to_string());
        }
        let usage = usage_from(&message["usage"]);
        if usage.input_tokens > 0 || usage.output_tokens > 0 {
            self.emit(AgentEvent::UsageRecorded {
                provider: PROVIDER.into(),
                model: self.model_name.clone(),
                usage,
            });
        }
        if self.call_usage.total_tokens > 0 {
            send_token_usage_event(&self.event_tx, self.scope.as_ref(), provider, model, self.call_usage);
        }
        if message["is_error"].as_bool().unwrap_or(false) && !self.failed && !interrupted {
            let detail = message["result"]
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    message["errors"]
                        .as_array()
                        .map(|errors| errors.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
                })
                .unwrap_or_default();
            self.failed = true;
            self.emit(AgentEvent::Error {
                message: describe_error(message["subtype"].as_str().unwrap_or_default(), &detail),
            });
        }
    }

    fn push_part(&mut self, part: Part) {
        if let Some((_, message)) = self.current.as_mut() {
            message.parts.push(part);
        }
    }

    fn flush(&mut self, history: &mut Vec<ChatMessage>) {
        if let Some((_, message)) = self.current.take() {
            if !message.parts.is_empty() {
                history.push(message);
            }
        }
    }

    /// Stores where the session stands on this turn's last assistant message.
    fn record_session(&self, history: &mut [ChatMessage], conversation: Option<&str>) {
        let Some(session_id) = &self.session_id else {
            return;
        };
        let start = self.history_start.min(history.len());
        let Some(message) = history[start..]
            .iter_mut()
            .rev()
            .find(|message| message.role == Role::Assistant)
        else {
            return;
        };
        let Some(part) = message.parts.first_mut() else {
            return;
        };
        let slot = match part {
            Part::Text { meta, .. }
            | Part::Image { meta, .. }
            | Part::Thinking { meta, .. }
            | Part::ToolCall { meta, .. }
            | Part::ToolResult { meta, .. } => meta,
        };
        let mut map = match slot.take() {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        map.insert(
            SESSION_META.into(),
            json!({
                "session_id": session_id,
                "message_uuid": self.message_uuid,
                "conversation": conversation,
            }),
        );
        *slot = Some(Value::Object(map));
    }
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// A readable error, with the fix when Claude Code is signed out.
fn describe_error(kind: &str, detail: &str) -> String {
    let detail = detail.trim();
    let auth = kind == "authentication_failed"
        || detail.contains("Failed to authenticate")
        || detail.contains("OAuth session expired")
        || detail.contains("/login");
    if auth {
        return format!(
            "Claude Code is not signed in. Run `claude auth login` in a terminal (or Settings > Providers > Anthropic > Sign in), then send your message again.\n{detail}"
        );
    }
    if detail.is_empty() {
        format!("Claude Code error: {kind}")
    } else {
        detail.to_string()
    }
}

/// Text and images of a `tool_result` block.
fn result_content(content: &Value) -> (String, Vec<ToolRunImage>) {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    match content {
        Value::String(text) => texts.push(text.clone()),
        Value::Array(blocks) => {
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => texts.push(block["text"].as_str().unwrap_or_default().to_string()),
                    Some("image") => {
                        let source = &block["source"];
                        if let Some(data) = source["data"].as_str() {
                            images.push(ToolRunImage {
                                media_type: source["media_type"].as_str().unwrap_or("image/png").to_string(),
                                data: data.to_string(),
                                path: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (texts.join("\n"), images)
}

/// Claude Code's tool name and input, as the Sinew tool the UI already knows
/// how to show (and the permission rules know how to judge).
fn map_tool(name: &str, input: &Value) -> (String, Value) {
    let text = |key: &str| input.get(key).cloned().unwrap_or(Value::Null);
    let pick = |pairs: &[(&str, &str)]| {
        let mut out = Map::new();
        for (from, to) in pairs {
            if let Some(value) = input.get(*from).filter(|value| !value.is_null()) {
                out.insert((*to).to_string(), value.clone());
            }
        }
        Value::Object(out)
    };
    if let Some(sinew) = name.strip_prefix(SINEW_MCP_PREFIX) {
        return (sinew.to_string(), input.clone());
    }
    match name {
        "Bash" | "PowerShell" => (
            tool_names::BASH.into(),
            pick(&[("command", "command"), ("description", "description")]),
        ),
        "Read" => (
            tool_names::READ.into(),
            pick(&[("file_path", "path"), ("offset", "offset"), ("limit", "limit")]),
        ),
        "Write" => (
            tool_names::WRITE_FILE.into(),
            pick(&[("file_path", "path"), ("content", "content")]),
        ),
        "Edit" => (
            tool_names::EDIT_FILE.into(),
            json!({ "files": [{
                "path": text("file_path"),
                "edits": [{ "oldContent": text("old_string"), "newContent": text("new_string") }],
            }] }),
        ),
        "MultiEdit" => {
            let edits: Vec<Value> = input["edits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|edit| json!({ "oldContent": edit["old_string"], "newContent": edit["new_string"] }))
                .collect();
            (
                tool_names::EDIT_FILE.into(),
                json!({ "files": [{ "path": text("file_path"), "edits": edits }] }),
            )
        }
        "Grep" => (
            tool_names::GREP.into(),
            pick(&[("pattern", "pattern"), ("path", "path"), ("glob", "include")]),
        ),
        "Glob" => (tool_names::GLOB.into(), pick(&[("pattern", "pattern"), ("path", "path")])),
        "WebSearch" | "web_search" => (tool_names::WEB_SEARCH.into(), pick(&[("query", "query")])),
        "WebFetch" => (tool_names::WEB_FETCH.into(), pick(&[("url", "url"), ("prompt", "prompt")])),
        "TodoWrite" => (tool_names::TODO_LIST.into(), input.clone()),
        "AskUserQuestion" => (tool_names::QUESTION.into(), question_input(input)),
        _ => (name.to_string(), input.clone()),
    }
}

/// `AskUserQuestion` input in the shape of Sinew's question tool.
fn question_input(input: &Value) -> Value {
    let questions: Vec<Value> = input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|question| {
            let multi = question["multiSelect"].as_bool().unwrap_or(false);
            json!({
                "question": question["question"],
                "type": if multi { "multiple_choice" } else { "single_choice" },
                "options": question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|option| json!({ "label": option["label"], "description": option["description"] }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "questions": questions })
}

/// The todo strip state from a `TodoWrite` input.
fn todo_state(input: &Value) -> Option<TodoListState> {
    let todos = input["todos"].as_array()?;
    let tasks: Vec<TodoTask> = todos
        .iter()
        .enumerate()
        .map(|(index, todo)| TodoTask {
            id: (index + 1).to_string(),
            text: todo["content"].as_str().unwrap_or_default().to_string(),
            status: match todo["status"].as_str() {
                Some("completed") => TodoStatus::Done,
                Some("in_progress") => TodoStatus::InProgress,
                _ => TodoStatus::Pending,
            },
        })
        .collect();
    Some(TodoListState {
        active: !tasks.is_empty(),
        next_id: tasks.len() as u32 + 1,
        tasks,
    })
}

/// The diff card of an `Edit` / `Write` result, from its `structuredPatch`.
fn file_change(root: &Path, result: &Value) -> Option<FileChange> {
    let path = result["filePath"].as_str()?;
    let patch = result["structuredPatch"].as_array()?;
    let relative = relative_path(root, path);
    let created = result["type"].as_str() == Some("create");
    let mut lines = Vec::new();
    let (mut added_lines, mut removed_lines) = (0, 0);
    if created && patch.is_empty() {
        for line in result["content"].as_str().unwrap_or_default().lines() {
            added_lines += 1;
            lines.push(DiffLine {
                kind: DiffLineKind::Added,
                text: line.to_string(),
            });
        }
    }
    for hunk in patch {
        for line in hunk["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let (kind, text) = match line.chars().next() {
                Some('+') => {
                    added_lines += 1;
                    (DiffLineKind::Added, &line[1..])
                }
                Some('-') => {
                    removed_lines += 1;
                    (DiffLineKind::Removed, &line[1..])
                }
                Some(' ') => (DiffLineKind::Context, &line[1..]),
                _ => (DiffLineKind::Context, line),
            };
            lines.push(DiffLine {
                kind,
                text: text.to_string(),
            });
        }
    }
    let kind = if created {
        FileChangeKind::Added
    } else {
        FileChangeKind::Modified
    };
    Some(FileChange {
        summary: match kind {
            FileChangeKind::Added => format!("Added {relative}"),
            _ => format!("Updated {relative}"),
        },
        relative_path: relative,
        kind,
        binary: false,
        added_lines,
        removed_lines,
        truncated: false,
        lines,
    })
}

fn relative_path(root: &Path, path: &str) -> String {
    let normalize = |value: &str| value.replace('\\', "/");
    let root = normalize(&root.to_string_lossy());
    let path = normalize(path);
    let root = root.trim_end_matches('/');
    match path.get(..root.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(root) && path[root.len()..].starts_with('/') => {
            path[root.len() + 1..].to_string()
        }
        _ => path,
    }
}

/// Answers the control requests Claude Code sends during a turn.
struct Helpers {
    sender: ProcessSender,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: TurnCancel,
    root: std::path::PathBuf,
    tool_settings: ToolSettings,
    question: Option<Arc<QuestionTool>>,
    browser: Arc<BrowserTools>,
}

impl Helpers {
    fn answer(&self, message: Value) {
        let request_id = message["request_id"].as_str().unwrap_or_default().to_string();
        let request = message["request"].clone();
        let sender = self.sender.clone();
        match request["subtype"].as_str() {
            Some("can_use_tool") => {
                let event_tx = self.event_tx.clone();
                let cancel = self.cancel.clone();
                let root = self.root.clone();
                let settings = self.tool_settings.clone();
                let question = self.question.clone();
                tokio::spawn(async move {
                    let answer =
                        permission(&request, &event_tx, &cancel, &root, &settings, question.as_deref()).await;
                    sender.respond(&request_id, answer);
                });
            }
            Some("mcp_message") if request["server_name"] == SINEW_MCP => {
                let browser = self.browser.clone();
                let scripts = self.tool_settings.browser_scripts_enabled;
                tokio::spawn(async move {
                    let response = mcp_message(&request["message"], &browser, scripts).await;
                    sender.respond(&request_id, json!({ "mcp_response": response }));
                });
            }
            _ => sender.respond_error(&request_id, "not supported by Sinew"),
        }
    }
}

fn allow(input: &Value, tool_use_id: &str) -> Value {
    json!({ "behavior": "allow", "updatedInput": input, "toolUseID": tool_use_id })
}

fn deny(message: impl Into<String>, tool_use_id: &str) -> Value {
    json!({ "behavior": "deny", "message": message.into(), "toolUseID": tool_use_id })
}

/// Sinew's permission rules and approval cards for one Claude Code prompt.
async fn permission(
    request: &Value,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &TurnCancel,
    root: &Path,
    settings: &ToolSettings,
    question: Option<&QuestionTool>,
) -> Value {
    let tool = request["tool_name"].as_str().unwrap_or_default();
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let tool_use_id = request["tool_use_id"].as_str().unwrap_or_default();

    if tool == "ExitPlanMode" {
        return deny(
            "Sinew showed this plan to the user, who switches to Act mode to implement it. Stop here without calling more tools; end with a one-line summary.",
            tool_use_id,
        );
    }
    if tool == "AskUserQuestion" {
        let Some(question) = question else {
            return deny("The user cannot answer questions right now; decide yourself.", tool_use_id);
        };
        let result = question.run(tool_use_id, question_input(&input), cancel).await;
        if result.is_error {
            return deny(result.content, tool_use_id);
        }
        let given = result
            .meta
            .as_ref()
            .and_then(|meta| meta.get("question_answers"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut answers = Map::new();
        for (index, asked) in input["questions"].as_array().into_iter().flatten().enumerate() {
            let Some(text) = asked["question"].as_str() else {
                continue;
            };
            let answer = given
                .get(index)
                .and_then(Value::as_array)
                .map(|labels| labels.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            answers.insert(text.to_string(), Value::String(answer));
        }
        let mut updated = input.clone();
        if let Some(object) = updated.as_object_mut() {
            object.insert("answers".into(), Value::Object(answers));
        }
        return allow(&updated, tool_use_id);
    }

    let (name, mapped) = map_tool(tool, &input);
    let rules = permissions::load_rules(&settings.permission_rules, root);
    let (reason, suggested_rule) =
        match permissions::evaluate(settings.permission_mode, &rules, root, &name, &mapped) {
            Decision::Allow => return allow(&input, tool_use_id),
            Decision::Deny { reason } => return deny(reason, tool_use_id),
            Decision::Ask {
                reason,
                suggested_rule,
            } => (reason, suggested_rule),
        };

    let Ok(reply) = cancel.register_approval(tool_use_id) else {
        return deny("The user interrupted the turn.", tool_use_id);
    };
    let _ = event_tx.send(AgentEvent::ApprovalRequested {
        id: tool_use_id.to_string(),
        tool_name: tool_names::canonical_tool_name(&name).to_string(),
        subject: permissions::subject(&name, &mapped),
        reason,
        suggested_rule: suggested_rule.clone(),
    });
    let reply = reply.await.unwrap_or(ApprovalReply::Deny { message: None });
    let outcome = match &reply {
        ApprovalReply::AllowOnce => "allowed",
        ApprovalReply::AllowAlways => "allowed_always",
        ApprovalReply::Deny { .. } => "denied",
    };
    let _ = event_tx.send(AgentEvent::ApprovalResolved {
        id: tool_use_id.to_string(),
        outcome: outcome.to_string(),
    });
    match reply {
        ApprovalReply::AllowOnce => allow(&input, tool_use_id),
        ApprovalReply::AllowAlways => {
            if let Err(err) = permissions::add_project_allow_rule(root, &suggested_rule) {
                tracing::warn!(error = %err, rule = %suggested_rule, "could not save permission rule");
            }
            allow(&input, tool_use_id)
        }
        ApprovalReply::Deny { message } => {
            let mut text = String::from("The user denied permission for this tool call. Do not retry it as is.");
            if let Some(message) = message.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()) {
                text.push_str(&format!(" The user says: {message}"));
            }
            deny(text, tool_use_id)
        }
    }
}

/// Sinew's in-process MCP server: the browser tools.
async fn mcp_message(message: &Value, browser: &BrowserTools, scripts: bool) -> Value {
    let id = message.get("id").cloned();
    let Some(id) = id.filter(|id| !id.is_null()) else {
        // A notification: nothing to answer, but the control request is.
        return json!({ "jsonrpc": "2.0", "result": {}, "id": 0 });
    };
    let params = &message["params"];
    let result = match message["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SINEW_MCP, "version": env!("CARGO_PKG_VERSION") },
        }),
        Some("tools/list") => json!({
            "tools": browser
                .all_descriptors(scripts)
                .into_iter()
                .map(|tool| json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                }))
                .collect::<Vec<_>>(),
        }),
        Some("tools/call") => {
            let name = params["name"].as_str().unwrap_or_default();
            let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let result = browser.run(name, arguments, scripts).await;
            let mut content = vec![json!({ "type": "text", "text": result.content })];
            content.extend(result.images.iter().map(|image| {
                json!({ "type": "image", "data": image.data, "mimeType": image.media_type })
            }));
            json!({ "content": content, "isError": result.is_error })
        }
        Some("ping") => json!({}),
        _ => {
            return json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" },
            })
        }
    };
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            parts: vec![Part::Text {
                text: text.into(),
                meta: None,
            }],
        }
    }

    fn assistant(text: &str, meta: Option<Value>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            parts: vec![Part::Text {
                text: text.into(),
                meta,
            }],
        }
    }

    #[test]
    fn resumes_the_recorded_session_with_only_the_new_message() {
        let history = vec![
            user("first"),
            assistant(
                "answer",
                Some(json!({ SESSION_META: { "session_id": "s1", "message_uuid": "u1", "conversation": "c1" } })),
            ),
            user("second"),
        ];
        let (record, start) = resume_point(&history);
        let record = record.unwrap();
        assert_eq!(record.session_id, "s1");
        assert_eq!(record.message_uuid.as_deref(), Some("u1"));
        assert_eq!(start, 2);
        let content = user_content(&history, None, start);
        assert_eq!(content, vec![json!({ "type": "text", "text": "second" })]);
    }

    #[test]
    fn replays_history_from_another_model_into_a_new_session() {
        let history = vec![user("first"), assistant("from gpt", None), user("second")];
        let (record, start) = resume_point(&history);
        assert!(record.is_none());
        let content = user_content(&history, Some(start), start);
        assert_eq!(content.len(), 2);
        let replay = content[0]["text"].as_str().unwrap();
        assert!(replay.contains("<earlier_conversation>") && replay.contains("from gpt"));
        assert_eq!(content[1]["text"], "second");
    }

    #[test]
    fn maps_claude_code_tools_to_sinew_cards() {
        let (name, input) = map_tool("Edit", &json!({ "file_path": "/w/src/a.rs", "old_string": "a", "new_string": "b" }));
        assert_eq!(name, "edit_file");
        assert_eq!(input["files"][0]["path"], "/w/src/a.rs");
        assert_eq!(input["files"][0]["edits"][0]["newContent"], "b");
        assert_eq!(map_tool("Bash", &json!({ "command": "ls" })).1, json!({ "command": "ls" }));
        assert_eq!(map_tool("mcp__sinew__browser_click", &json!({})).0, "browser_click");
        let (name, input) = map_tool(
            "AskUserQuestion",
            &json!({ "questions": [{ "question": "Which?", "header": "Pick", "multiSelect": true, "options": [{ "label": "A", "description": "a" }] }] }),
        );
        assert_eq!(name, "question");
        assert_eq!(input["questions"][0]["type"], "multiple_choice");
    }

    #[test]
    fn builds_a_diff_from_a_structured_patch() {
        let change = file_change(
            Path::new("C:\\work\\app"),
            &json!({
                "filePath": "C:\\work\\app\\src\\main.rs",
                "structuredPatch": [{ "lines": [" fn main() {", "-    old();", "+    new();", "+    more();", " }"] }],
            }),
        )
        .unwrap();
        assert_eq!(change.relative_path, "src/main.rs");
        assert_eq!((change.added_lines, change.removed_lines), (2, 1));
        assert!(matches!(change.kind, FileChangeKind::Modified));
    }

    /// Starts the real `claude`, registers the browser MCP server and sends a
    /// prompt. Needs Claude Code installed; signed out, the prompt fails with
    /// the sign-in message, which is checked too.
    #[tokio::test]
    #[ignore = "starts the local Claude Code"]
    async fn talks_to_the_local_claude_code() {
        let root = std::env::temp_dir();
        let browser = BrowserTools::new(
            root.to_string_lossy().into_owned(),
            sinew_browser::BrowserSessions::new(),
        );
        let args = vec![
            "--include-partial-messages".to_string(),
            "--permission-prompt-tool".into(),
            "stdio".into(),
            "--model".into(),
            "claude-haiku-4-5".into(),
            "--mcp-config".into(),
            json!({ "mcpServers": { SINEW_MCP: { "type": "sdk", "name": SINEW_MCP } } }).to_string(),
        ];
        let mut process = ClaudeProcess::spawn(&args, &root).expect("claude starts");
        let sender = process.sender();
        let init = tokio::spawn({
            let sender = sender.clone();
            async move {
                let init = sender
                    .request(json!({ "subtype": "initialize", "sdkMcpServers": [SINEW_MCP] }), Duration::from_secs(60))
                    .await;
                sender.send(&json!({
                    "type": "user",
                    "message": { "role": "user", "content": "Reply with the single word: pong" },
                    "parent_tool_use_id": null,
                    "session_id": "",
                }));
                init
            }
        });
        let mut mcp_calls = Vec::new();
        let mut tools = Vec::new();
        let mut outcome = None;
        while let Ok(Some(message)) =
            tokio::time::timeout(Duration::from_secs(90), process.next_message()).await
        {
            match message["type"].as_str() {
                Some("control_request") if message["request"]["subtype"] == "mcp_message" => {
                    let rpc = &message["request"]["message"];
                    mcp_calls.push(rpc["method"].as_str().unwrap_or_default().to_string());
                    let response = mcp_message(rpc, &browser, false).await;
                    sender.respond(message["request_id"].as_str().unwrap(), json!({ "mcp_response": response }));
                }
                Some("system") if message["subtype"] == "init" => {
                    tools = message["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                }
                Some("result") => {
                    outcome = Some(message);
                    break;
                }
                _ => {}
            }
        }
        let init = init.await.unwrap().expect("initialize answered");
        assert!(init["models"].is_array(), "initialize lists models: {init}");
        assert!(mcp_calls.iter().any(|method| method == "tools/list"), "MCP calls: {mcp_calls:?}");
        assert!(tools.iter().any(|tool| tool == "mcp__sinew__browser_navigate"), "tools: {tools:?}");
        let outcome = outcome.expect("a result");
        eprintln!("result: {}", outcome["result"]);
        process.shutdown(Duration::from_secs(2)).await;
    }

    #[test]
    fn reads_todo_write_into_the_strip() {
        let state = todo_state(&json!({ "todos": [
            { "content": "a", "status": "completed" },
            { "content": "b", "status": "in_progress" }
        ] }))
        .unwrap();
        assert!(state.active);
        assert!(matches!(state.tasks[0].status, TodoStatus::Done));
        assert!(matches!(state.tasks[1].status, TodoStatus::InProgress));
    }
}
