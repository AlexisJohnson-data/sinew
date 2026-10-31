//! A running `claude -p` process spoken to in stream-json, the protocol the
//! Agent SDK uses: one JSON object per line each way, plus `control_request`
//! / `control_response` pairs in both directions (initialize, permission
//! prompts, in-process MCP servers, interrupts, usage).

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use sinew_core::{AppError, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};

use crate::cli;

/// Last lines of stderr kept for error messages.
const STDERR_TAIL_LINES: usize = 20;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// Writes to the process. Cheap to clone, so permission prompts and MCP
/// calls can answer from their own tasks.
#[derive(Clone)]
pub struct ProcessSender {
    stdin: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl ProcessSender {
    pub fn send(&self, message: &Value) {
        let _ = self.stdin.send(message.to_string());
    }

    /// Answers a `control_request` the CLI sent.
    pub fn respond(&self, request_id: &str, response: Value) {
        self.send(&json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        }));
    }

    pub fn respond_error(&self, request_id: &str, error: &str) {
        self.send(&json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": request_id,
                "error": error,
            },
        }));
    }

    /// Sends a `control_request` and waits for its `control_response`.
    pub async fn request(&self, request: Value, timeout: Duration) -> Result<Value> {
        let request_id = format!("sinew-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(request_id.clone(), sender);
        }
        self.send(&json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        }));
        let outcome = tokio::time::timeout(timeout, receiver).await;
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request_id);
        }
        match outcome {
            Ok(Ok(Ok(response))) => Ok(response),
            Ok(Ok(Err(error))) => Err(AppError::Provider(format!("Claude Code: {error}"))),
            Ok(Err(_)) => Err(AppError::Provider("Claude Code exited".into())),
            Err(_) => Err(AppError::Provider("Claude Code did not answer in time".into())),
        }
    }
}

pub struct ClaudeProcess {
    child: Child,
    sender: ProcessSender,
    /// Every stdout message except answers to our own control requests.
    messages: mpsc::UnboundedReceiver<Value>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

impl ClaudeProcess {
    /// Starts `claude` with `args` (stream-json input and output are added).
    pub fn spawn(args: &[String], cwd: &Path) -> Result<Self> {
        let mut command = cli::command().ok_or_else(|| {
            AppError::Auth(
                "Claude Code is not installed. Install it (https://claude.com/claude-code), run `claude auth login`, then pick a Claude model again.".into(),
            )
        })?;
        command
            .args([
                "-p",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
            ])
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("NODE_OPTIONS");
        let mut child = command
            .spawn()
            .map_err(|err| AppError::Provider(format!("unable to start Claude Code: {err}")))?;

        let mut stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let pending: Pending = Arc::default();
        let (message_tx, messages) = mpsc::unbounded_channel();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                if message.get("type").and_then(Value::as_str) == Some("control_response") {
                    let response = &message["response"];
                    let request_id = response["request_id"].as_str().unwrap_or_default();
                    let waiter = reader_pending
                        .lock()
                        .ok()
                        .and_then(|mut pending| pending.remove(request_id));
                    if let Some(waiter) = waiter {
                        let outcome = if response["subtype"] == "error" {
                            Err(response["error"].as_str().unwrap_or("error").to_string())
                        } else {
                            Ok(response.get("response").cloned().unwrap_or(Value::Null))
                        };
                        let _ = waiter.send(outcome);
                        continue;
                    }
                }
                if message_tx.send(message).is_err() {
                    break;
                }
            }
        });

        let stderr: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let stderr_tail = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut tail) = stderr_tail.lock() {
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            }
        });

        Ok(Self {
            child,
            sender: ProcessSender {
                stdin: stdin_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            messages,
            stderr,
        })
    }

    pub fn sender(&self) -> ProcessSender {
        self.sender.clone()
    }

    /// Next stdout message; `None` once the process closed stdout.
    pub async fn next_message(&mut self) -> Option<Value> {
        self.messages.recv().await
    }

    /// The end of stderr, for an error message when the process dies early.
    pub fn stderr_tail(&self) -> String {
        self.stderr
            .lock()
            .map(|tail| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    /// Sends the `initialize` control request.
    pub async fn initialize(&self, options: Value) -> Result<Value> {
        let mut request = json!({ "subtype": "initialize" });
        if let (Some(request), Some(options)) = (request.as_object_mut(), options.as_object()) {
            for (key, value) in options {
                request.insert(key.clone(), value.clone());
            }
        }
        self.sender.request(request, Duration::from_secs(60)).await
    }

    /// Lets the process finish on its own for `grace`, then kills it.
    pub async fn shutdown(mut self, grace: Duration) {
        // Closing stdin tells a stream-json session there is no more input.
        drop(self.sender);
        if tokio::time::timeout(grace, self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }
}
