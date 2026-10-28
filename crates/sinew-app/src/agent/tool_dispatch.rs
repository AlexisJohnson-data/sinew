use std::{collections::HashMap, path::Path, sync::Arc};

use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    permissions::{self, Decision},
    lsp, rtk, tool_names, BashTool, BrowserTools, CreateImageTool, EditFileTool, GlobTool, GrepTool,
    McpToolRegistry, QuestionTool, ReadFingerprint, ReadTool, SkillTool, SubAgentTool, TeamTool,
    ToDoListTool, TodoListState, ToolRunResult, ToolSettings, WebFetchTool, WebSearchTool,
    WriteFileTool,
};

use super::{
    cancel::{ApprovalReply, TurnCancel},
    context::AgentMode,
    events::AgentEvent,
};

pub(super) fn should_wait_for_cooperative_cancel(
    name: &str,
    subagents: Option<&Arc<SubAgentTool>>,
    teams: Option<&Arc<TeamTool>>,
) -> bool {
    name.starts_with("subagent_")
        || teams
            .and_then(|tool| tool.summary_for_tool_name(name))
            .is_some()
        || subagents
            .and_then(|tool| tool.summary_for_tool_name(name))
            .is_some()
}

pub(super) async fn run_tool(
    bash: &BashTool,
    glob: &GlobTool,
    grep: &GrepTool,
    read: &ReadTool,
    edit_file: &EditFileTool,
    write_file: &WriteFileTool,
    create_image: &CreateImageTool,
    todo_list_tool: Option<&ToDoListTool>,
    question: Option<&QuestionTool>,
    web_search: &WebSearchTool,
    web_fetch: &WebFetchTool,
    browser: &BrowserTools,
    skill: &SkillTool,
    mcp: &McpToolRegistry,
    subagents: Option<&SubAgentTool>,
    teams: Option<&TeamTool>,
    tool_settings: &ToolSettings,
    read_fingerprints: &HashMap<String, ReadFingerprint>,
    todo_list: &mut TodoListState,
    mode: AgentMode,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &TurnCancel,
    tool_call_id: &str,
    name: &str,
    input: Value,
) -> ToolRunResult {
    let canonical_name = tool_names::canonical_tool_name(name);
    if !tool_settings.is_enabled(canonical_name) {
        return ToolRunResult::err(
            format!("{canonical_name} is disabled in Settings"),
            Vec::new(),
        );
    }
    if let Some(stopped) = permission_gate(
        bash.workspace_root(),
        tool_settings,
        event_tx,
        cancel,
        tool_call_id,
        name,
        &input,
    )
    .await
    {
        return stopped;
    }
    if canonical_name == tool_names::BASH {
        let (input, rtk_meta) = if tool_settings.rtk_enabled {
            if let Some(command) = input.get("command").and_then(|v| v.as_str()) {
                if let Some(rewrite) = rtk::maybe_rewrite_command(command) {
                    let mut patched = input.clone();
                    patched["command"] = serde_json::Value::String(rewrite.rewritten.clone());
                    let meta = serde_json::json!({
                        "rtk": {
                            "original": rewrite.original,
                            "rewritten": rewrite.rewritten,
                        }
                    });
                    (patched, Some(meta))
                } else {
                    (input, None)
                }
            } else {
                (input, None)
            }
        } else {
            (input, None)
        };
        let mut result = bash.run(input).await;
        if let Some(meta) = rtk_meta {
            result.meta = Some(meta);
        }
        result
    } else if canonical_name == tool_names::BASH_INPUT {
        bash.run_input(input).await
    } else if canonical_name == tool_names::GLOB {
        glob.run(input).await
    } else if canonical_name == tool_names::GREP {
        grep.run(input).await
    } else if canonical_name == tool_names::READ {
        read.run(input).await
    } else if canonical_name == tool_names::EDIT_FILE {
        if mode == AgentMode::Plan {
            return ToolRunResult::err("edit_file is unavailable in Plan mode", Vec::new());
        }
        let mut result = edit_file.run(input, read_fingerprints).await;
        lsp::attach_after_edit(bash.workspace_root(), tool_settings, &mut result).await;
        result
    } else if canonical_name == tool_names::WRITE_FILE {
        if mode == AgentMode::Plan {
            return ToolRunResult::err("write_file is unavailable in Plan mode", Vec::new());
        }
        let mut result = write_file.run(input, read_fingerprints).await;
        lsp::attach_after_edit(bash.workspace_root(), tool_settings, &mut result).await;
        result
    } else if canonical_name == tool_names::DIAGNOSTICS {
        lsp::run_tool(bash.workspace_root(), tool_settings, input).await
    } else if canonical_name == tool_names::CREATE_IMAGE {
        if mode == AgentMode::Plan {
            return ToolRunResult::err("create_image is unavailable in Plan mode", Vec::new());
        }
        create_image.run(input).await
    } else if canonical_name == tool_names::TODO_LIST {
        let Some(todo_list_tool) = todo_list_tool else {
            return ToolRunResult::err("todo_list is unavailable in this context", Vec::new());
        };
        todo_list_tool.run(input, todo_list).await
    } else if canonical_name == tool_names::QUESTION {
        let Some(question) = question else {
            return ToolRunResult::err("question is unavailable in this context", Vec::new());
        };
        question.run(tool_call_id, input, cancel).await
    } else if canonical_name == tool_names::WEB_SEARCH {
        web_search.run(input).await
    } else if canonical_name == tool_names::WEB_FETCH {
        web_fetch.run(input).await
    } else if name.starts_with("browser_") {
        if mode == AgentMode::Plan {
            return ToolRunResult::err("browser tools are unavailable in Plan mode", Vec::new());
        }
        if !tool_settings.browser_enabled {
            return ToolRunResult::err("browser tools are disabled in Settings", Vec::new());
        }
        browser.run(name, input).await
    } else if canonical_name == tool_names::SKILL {
        skill.run(input).await
    } else if name.starts_with("subagent_") {
        let Some(subagents) = subagents else {
            return ToolRunResult::err(format!("unknown tool: {name}"), Vec::new());
        };
        subagents
            .run(tool_call_id, name, input, mode, event_tx.clone())
            .await
            .unwrap_or_else(|| ToolRunResult::err(format!("unknown tool: {name}"), Vec::new()))
    } else if let Some(teams) = teams {
        if let Some(result) = teams
            .run(tool_call_id, name, input.clone(), mode, event_tx.clone())
            .await
        {
            result
        } else if let Some(result) = mcp.run_tool(name, input).await {
            result
        } else {
            ToolRunResult::err(format!("unknown tool: {name}"), Vec::new())
        }
    } else if let Some(result) = mcp.run_tool(name, input).await {
        result
    } else {
        ToolRunResult::err(format!("unknown tool: {name}"), Vec::new())
    }
}

/// Check the call against the permission rules. `None` means it may run;
/// otherwise the returned result explains to the agent why it did not.
/// "Ask" decisions park the call until the user answers in the chat (or from
/// the phone); cancelling the turn denies it.
async fn permission_gate(
    workspace_root: &Path,
    tool_settings: &ToolSettings,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    cancel: &TurnCancel,
    tool_call_id: &str,
    name: &str,
    input: &Value,
) -> Option<ToolRunResult> {
    let rules = permissions::load_rules(&tool_settings.permission_rules, workspace_root);
    let (reason, suggested_rule) = match permissions::evaluate(
        tool_settings.permission_mode,
        &rules,
        workspace_root,
        name,
        input,
    ) {
        Decision::Allow => return None,
        Decision::Deny { reason } => {
            let mut result = ToolRunResult::err(reason, Vec::new());
            result.meta = Some(serde_json::json!({ "permission": "blocked" }));
            return Some(result);
        }
        Decision::Ask {
            reason,
            suggested_rule,
        } => (reason, suggested_rule),
    };

    let Ok(reply) = cancel.register_approval(tool_call_id) else {
        return Some(ToolRunResult::err(
            "tool call interrupted by user",
            Vec::new(),
        ));
    };
    // Sent unscoped on purpose: a subagent's prompt surfaces in the main chat,
    // where the user can answer it.
    let _ = event_tx.send(AgentEvent::ApprovalRequested {
        id: tool_call_id.to_string(),
        tool_name: tool_names::canonical_tool_name(name).to_string(),
        subject: permissions::subject(name, input),
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
        id: tool_call_id.to_string(),
        outcome: outcome.to_string(),
    });
    match reply {
        ApprovalReply::AllowOnce => None,
        ApprovalReply::AllowAlways => {
            if let Err(err) = permissions::add_project_allow_rule(workspace_root, &suggested_rule) {
                tracing::warn!(error = %err, rule = %suggested_rule, "could not save permission rule");
            }
            None
        }
        ApprovalReply::Deny { message } => {
            let mut content = String::from(
                "The user denied permission for this tool call. Do not retry it as is.",
            );
            if let Some(message) = message
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
            {
                content.push_str(&format!(" The user says: {message}"));
            }
            let mut result = ToolRunResult::err(content, Vec::new());
            result.meta = Some(serde_json::json!({ "permission": "denied" }));
            Some(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionMode;
    use serde_json::json;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("sinew-gate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn gate_with_reply(
        root: &Path,
        mode: PermissionMode,
        command: &str,
        reply: ApprovalReply,
    ) -> (Option<ToolRunResult>, Vec<AgentEvent>) {
        let settings = ToolSettings {
            permission_mode: mode,
            ..ToolSettings::default()
        };
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let cancel = TurnCancel::empty();
        let input = json!({ "command": command });
        let answerer = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                // Answer as soon as the prompt is registered.
                for _ in 0..200 {
                    if cancel.answer_approval("call-1", reply.clone()) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
        };
        let result = permission_gate(
            root, &settings, &event_tx, &cancel, "call-1", "bash", &input,
        )
        .await;
        answerer.await.unwrap();
        drop(event_tx);
        let mut events = Vec::new();
        while let Some(event) = event_rx.recv().await {
            events.push(event);
        }
        (result, events)
    }

    #[tokio::test]
    async fn approved_call_runs_and_always_allow_saves_the_rule() {
        let root = temp_root("allow");
        let (result, events) = gate_with_reply(
            &root,
            PermissionMode::AskShell,
            "npm run test",
            ApprovalReply::AllowAlways,
        )
        .await;
        assert!(result.is_none(), "an approved call must run");
        assert!(matches!(
            &events[0],
            AgentEvent::ApprovalRequested { subject, .. } if subject == "npm run test"
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::ApprovalResolved { outcome, .. } if outcome == "allowed_always"
        ));
        assert_eq!(
            permissions::load_project_rules(&root).allow,
            vec!["bash: npm run *".to_string()]
        );
        // The saved rule now lets the next call through without asking.
        let settings = ToolSettings {
            permission_mode: PermissionMode::AskShell,
            ..ToolSettings::default()
        };
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let next = permission_gate(
            &root,
            &settings,
            &event_tx,
            &TurnCancel::empty(),
            "call-2",
            "bash",
            &json!({ "command": "npm run lint" }),
        )
        .await;
        assert!(next.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn denied_call_returns_the_users_message() {
        let root = temp_root("deny");
        let (result, _) = gate_with_reply(
            &root,
            PermissionMode::Auto,
            "git push --force origin main",
            ApprovalReply::Deny {
                message: Some("push to a branch instead".into()),
            },
        )
        .await;
        let result = result.expect("a denied call must not run");
        assert!(result.is_error);
        assert!(result.content.contains("push to a branch instead"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn blocked_command_never_asks() {
        let root = temp_root("block");
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let result = permission_gate(
            &root,
            &ToolSettings::default(),
            &event_tx,
            &TurnCancel::empty(),
            "call-1",
            "bash",
            &json!({ "command": "rm -rf /" }),
        )
        .await
        .expect("blocked");
        assert!(result.is_error);
        assert_eq!(result.meta, Some(json!({ "permission": "blocked" })));
        drop(event_tx);
        assert!(
            event_rx.recv().await.is_none(),
            "no prompt for blocked commands"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cancelling_the_turn_denies_a_waiting_prompt() {
        let root = temp_root("cancel");
        let settings = ToolSettings {
            permission_mode: PermissionMode::AskShell,
            ..ToolSettings::default()
        };
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        let cancel = TurnCancel::empty();
        let canceller = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                cancel.cancel_all();
            })
        };
        let result = permission_gate(
            &root,
            &settings,
            &event_tx,
            &cancel,
            "call-1",
            "bash",
            &json!({ "command": "ls" }),
        )
        .await;
        canceller.await.unwrap();
        assert!(result.expect("cancelled prompt must not run").is_error);
        let _ = std::fs::remove_dir_all(&root);
    }
}
