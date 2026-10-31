//! The `anthropic` provider. Agent turns with a Claude model are run by
//! Claude Code itself (see `sinew_app`'s delegated turn); this provider only
//! answers tool-less requests such as compaction, through a one-shot
//! `claude -p` with no tools.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use serde_json::{json, Value};
use sinew_core::{
    AppError, ModelCapabilities, ModelRef, PartKind, Provider, ProviderRequest, ProviderStream,
    Result, StopReason, StreamEvent, TokenEstimate, Usage,
};
use tokio::sync::mpsc;

use crate::model_info;
use crate::process::ClaudeProcess;
use crate::transcript::render_transcript;

/// Longest history sent to a tool-less request, in characters.
const TRANSCRIPT_LIMIT: usize = 600_000;

pub struct ClaudeCodeProvider {
    cwd: PathBuf,
}

impl ClaudeCodeProvider {
    pub fn new() -> Self {
        Self {
            cwd: std::env::temp_dir(),
        }
    }
}

impl Default for ClaudeCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Usage of one model response, from the API's `usage` object.
pub fn usage_from(value: &Value) -> Usage {
    let field = |name: &str| value[name].as_u64().unwrap_or(0) as u32;
    let input_tokens = field("input_tokens");
    let output_tokens = field("output_tokens");
    let cache_read_tokens = field("cache_read_input_tokens");
    let cache_creation_tokens = field("cache_creation_input_tokens");
    Usage {
        input_tokens,
        output_tokens,
        total_tokens: input_tokens
            .saturating_add(cache_read_tokens)
            .saturating_add(cache_creation_tokens)
            .saturating_add(output_tokens),
        reasoning_tokens: 0,
        cache_read_tokens,
        cache_creation_tokens,
    }
}

/// `low`…`max` for `--effort`, or `None` for the CLI default.
pub fn effort_flag(effort: Option<sinew_core::Effort>) -> Option<&'static str> {
    use sinew_core::Effort;
    match effort? {
        Effort::None => None,
        Effort::Low => Some("low"),
        Effort::Medium => Some("medium"),
        Effort::High => Some("high"),
        Effort::Xhigh => Some("xhigh"),
        Effort::Max => Some("max"),
    }
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self, model: &ModelRef) -> Option<ModelCapabilities> {
        Some(model_info::capabilities(model))
    }

    async fn estimate_tokens(&self, request: ProviderRequest) -> Result<TokenEstimate> {
        let chars = render_transcript(&request.transcript, usize::MAX).len()
            + request.system_prompt.as_deref().map_or(0, str::len);
        Ok(TokenEstimate {
            input_tokens: (chars / 4) as u32,
            exact: false,
        })
    }

    async fn stream(&self, request: ProviderRequest) -> Result<ProviderStream> {
        if !request.tools.is_empty() {
            return Err(AppError::Unsupported(
                "Claude models run their tools through Claude Code".into(),
            ));
        }
        let mut args = vec![
            "--model".to_string(),
            request.model.name.clone(),
            "--tools".into(),
            String::new(),
            "--strict-mcp-config".into(),
            "--no-session-persistence".into(),
            "--include-partial-messages".into(),
        ];
        if let Some(system) = request.system_prompt.as_deref().filter(|s| !s.trim().is_empty()) {
            args.push("--system-prompt".into());
            args.push(system.to_string());
        }
        if let Some(effort) = effort_flag(request.effective_effort()) {
            args.push("--effort".into());
            args.push(effort.into());
        }
        let mut process = ClaudeProcess::spawn(&args, &self.cwd)?;
        process.initialize(json!({})).await?;
        let prompt = render_transcript(&request.transcript, TRANSCRIPT_LIMIT);
        process.sender().send(&json!({
            "type": "user",
            "message": { "role": "user", "content": prompt },
            "parent_tool_use_id": null,
            "session_id": "",
        }));

        let model = request.model.name.clone();
        let (tx, rx) = mpsc::unbounded_channel::<Result<StreamEvent>>();
        tokio::spawn(async move {
            let _ = tx.send(Ok(StreamEvent::MessageStart { model }));
            let mut usage = Usage::default();
            let mut parts = 0usize;
            let mut open: Option<usize> = None;
            while let Some(message) = process.next_message().await {
                match message["type"].as_str() {
                    Some("stream_event") if message["parent_tool_use_id"].is_null() => {
                        let event = &message["event"];
                        match event["type"].as_str() {
                            Some("content_block_start") => {
                                let kind = match event["content_block"]["type"].as_str() {
                                    Some("text") => PartKind::Text,
                                    Some("thinking") => PartKind::Thinking,
                                    _ => continue,
                                };
                                let index = parts;
                                parts += 1;
                                open = Some(index);
                                let _ = tx.send(Ok(StreamEvent::PartStart {
                                    index,
                                    kind,
                                    tool: None,
                                }));
                            }
                            Some("content_block_delta") => {
                                let Some(index) = open else { continue };
                                let delta = &event["delta"];
                                let event = match delta["type"].as_str() {
                                    Some("text_delta") => StreamEvent::TextDelta {
                                        index,
                                        delta: delta["text"].as_str().unwrap_or_default().into(),
                                    },
                                    Some("thinking_delta") => StreamEvent::ThinkingDelta {
                                        index,
                                        delta: delta["thinking"]
                                            .as_str()
                                            .unwrap_or_default()
                                            .into(),
                                    },
                                    _ => continue,
                                };
                                let _ = tx.send(Ok(event));
                            }
                            Some("content_block_stop") => {
                                if let Some(index) = open.take() {
                                    let _ = tx.send(Ok(StreamEvent::PartStop { index }));
                                }
                            }
                            _ => {}
                        }
                    }
                    Some("result") => {
                        if message["is_error"].as_bool().unwrap_or(false) {
                            let error = message["result"]
                                .as_str()
                                .unwrap_or("Claude Code request failed");
                            let _ = tx.send(Err(AppError::Provider(error.to_string())));
                        } else {
                            usage = usage_from(&message["usage"]);
                        }
                        break;
                    }
                    _ => {}
                }
            }
            let _ = tx.send(Ok(StreamEvent::MessageStop {
                stop_reason: StopReason::EndTurn,
                usage,
            }));
            process.shutdown(Duration::from_secs(2)).await;
        });

        Ok(Box::pin(stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })))
    }
}
