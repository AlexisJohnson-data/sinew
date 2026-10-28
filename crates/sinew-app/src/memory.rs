//! Persistent memory across conversations.
//!
//! Two plain Markdown files, outside the repository so they stay private:
//! `~/.sinew/memory/user.md` (about the user, every project) and
//! `~/.sinew/memory/projects/<project>.md` (this workspace). One entry per
//! line, `- [id] text`; the files can be edited by hand. Both are injected
//! into the system prompt when a turn starts (so a change made during a turn
//! applies from the next one and the prompt cache holds), and the main agent
//! edits them through the `memory` tool. Sub-agents and swarm members only
//! read them.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{anyhow, bail, Context, Result};
use rand::Rng;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sinew_core::ToolDescriptor;

use crate::{store::ToolSettings, tool_names, tool_run::ToolRunResult};

/// Prompt budget per scope; entries past it are left out (and the agent is
/// told to consolidate).
const PROMPT_BUDGET_BYTES: usize = 8 * 1024;
const MAX_ENTRY_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Project,
}

impl MemoryScope {
    fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction {
    Add,
    Update,
    Remove,
}

/// What a write did, for the chat card and its undo button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryChange {
    pub action: MemoryAction,
    pub scope: MemoryScope,
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

// ---------------------------------------------------------------------------
// Files.

fn sinew_home() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or_else(|| anyhow!("no home directory"))?;
    Ok(base.home_dir().join(".sinew"))
}

/// `SINEW_MEMORY_DIR` overrides the location (tests, portable setups).
pub fn memory_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("SINEW_MEMORY_DIR").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(sinew_home()?.join("memory"))
}

/// `~/.sinew/AGENTS.md`: the user's own instructions for every project.
pub fn global_instructions_path() -> Result<PathBuf> {
    Ok(sinew_home()?.join("AGENTS.md"))
}

/// Stable per-workspace file name: readable folder name plus a short hash
/// of the normalized path, so two `app` folders do not collide.
fn project_slug(root: &Path) -> String {
    let text = root.to_string_lossy();
    let plain = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or_else(|| text.into_owned());
    let normalized = plain.replace('\\', "/").trim_end_matches('/').to_string();
    let normalized = if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    };
    let name: String = normalized
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("workspace")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let digest = Sha256::digest(normalized.as_bytes());
    let hash: String = digest[..4].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{name}-{hash}")
}

pub fn scope_path(scope: MemoryScope, root: &Path) -> Result<PathBuf> {
    let dir = memory_dir()?;
    Ok(match scope {
        MemoryScope::User => dir.join("user.md"),
        MemoryScope::Project => dir.join("projects").join(format!("{}.md", project_slug(root))),
    })
}

fn entry_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^\s*[-*]\s+(?:\[([A-Za-z0-9_-]{1,32})\]\s+)?(.+?)\s*$").unwrap())
}

/// Entries of a memory file. Hand-written bullets without an id get a
/// stable one derived from their text.
pub fn parse_entries(text: &str) -> Vec<MemoryEntry> {
    text.lines()
        .filter_map(|line| {
            let captures = entry_pattern().captures(line)?;
            let body = captures.get(2)?.as_str().to_string();
            let id = captures
                .get(1)
                .map(|id| id.as_str().to_string())
                .unwrap_or_else(|| text_id(&body));
            Some(MemoryEntry { id, text: body })
        })
        .collect()
}

fn text_id(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..3].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn header(scope: MemoryScope, root: &Path) -> String {
    let about = match scope {
        MemoryScope::User => "about you, shared by every project".to_string(),
        MemoryScope::Project => format!("for the project {}", root.display()),
    };
    format!(
        "# Sinew memory — {about}\n\n<!-- One entry per line: `- [id] text`. Edit freely; Sinew reads this file when a turn starts. -->\n\n"
    )
}

pub fn load(scope: MemoryScope, root: &Path) -> Vec<MemoryEntry> {
    scope_path(scope, root)
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| parse_entries(&text))
        .unwrap_or_default()
}

pub fn save(scope: MemoryScope, root: &Path, entries: &[MemoryEntry]) -> Result<()> {
    let path = scope_path(scope, root)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }
    let mut text = header(scope, root);
    for entry in entries {
        text.push_str(&format!("- [{}] {}\n", entry.id, entry.text));
    }
    let temp = path.with_extension("md.tmp");
    fs::write(&temp, text).with_context(|| format!("unable to write {}", temp.display()))?;
    fs::rename(&temp, &path).with_context(|| format!("unable to write {}", path.display()))?;
    Ok(())
}

fn new_id(existing: &[MemoryEntry]) -> String {
    let mut rng = rand::thread_rng();
    loop {
        let id = format!("{:05x}", rng.gen_range(0..0x100000u32));
        if !existing.iter().any(|entry| entry.id == id) {
            return id;
        }
    }
}

// ---------------------------------------------------------------------------
// Writes.

/// Refuse entries that look like credentials: memory files are plain text
/// and end up in every prompt.
fn secret_reason(text: &str) -> Option<&'static str> {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            (r"\bsk-[A-Za-z0-9_-]{16,}", "an API key"),
            (r"\b(?:ghp|gho|ghu|ghs|github_pat)_[A-Za-z0-9_]{20,}", "a GitHub token"),
            (r"\bAKIA[0-9A-Z]{16}\b", "an AWS access key"),
            (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "a Slack token"),
            (r"-----BEGIN [A-Z ]*PRIVATE KEY-----", "a private key"),
            (r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.", "a JWT"),
            (
                r"(?i)\b(?:password|passwd|pwd|api[_-]?key|secret|token)\b\s*[:=]\s*\S{6,}",
                "a password or secret",
            ),
        ]
        .into_iter()
        .map(|(pattern, label)| (Regex::new(pattern).unwrap(), label))
        .collect()
    });
    patterns
        .iter()
        .find(|(pattern, _)| pattern.is_match(text))
        .map(|(_, label)| *label)
}

fn clean_text(text: &str) -> Result<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        bail!("text is required");
    }
    if text.chars().count() > MAX_ENTRY_CHARS {
        bail!("an entry holds one fact in at most {MAX_ENTRY_CHARS} characters; split or shorten it");
    }
    if let Some(reason) = secret_reason(&text) {
        bail!("refused: this looks like {reason}. Never store credentials in memory");
    }
    Ok(text)
}

/// Apply one change to a scope's file. `id` is required for update/remove;
/// for add it is optional (undoing a removal restores the original id).
pub fn apply(
    root: &Path,
    scope: MemoryScope,
    action: MemoryAction,
    id: Option<&str>,
    text: Option<&str>,
) -> Result<MemoryChange> {
    let mut entries = load(scope, root);
    let find = |entries: &[MemoryEntry], id: Option<&str>| -> Result<usize> {
        let id = id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("id is required"))?;
        entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or_else(|| anyhow!("no {} memory entry with id {id}", scope.label()))
    };
    let change = match action {
        MemoryAction::Add => {
            let text = clean_text(text.unwrap_or_default())?;
            if let Some(existing) = entries.iter().find(|entry| entry.text == text) {
                bail!(
                    "this is already in {} memory as [{}]",
                    scope.label(),
                    existing.id
                );
            }
            let id = match id.map(str::trim).filter(|id| !id.is_empty()) {
                Some(id) if !entries.iter().any(|entry| entry.id == id) => id.to_string(),
                _ => new_id(&entries),
            };
            entries.push(MemoryEntry {
                id: id.clone(),
                text: text.clone(),
            });
            MemoryChange {
                action,
                scope,
                id,
                text,
                previous: None,
            }
        }
        MemoryAction::Update => {
            let index = find(&entries, id)?;
            let text = clean_text(text.unwrap_or_default())?;
            let previous = std::mem::replace(&mut entries[index].text, text.clone());
            MemoryChange {
                action,
                scope,
                id: entries[index].id.clone(),
                text,
                previous: Some(previous),
            }
        }
        MemoryAction::Remove => {
            let index = find(&entries, id)?;
            let removed = entries.remove(index);
            MemoryChange {
                action,
                scope,
                id: removed.id,
                text: removed.text,
                previous: None,
            }
        }
    };
    save(scope, root, &entries)?;
    Ok(change)
}

// ---------------------------------------------------------------------------
// Prompt.

fn render_scope(entries: &[MemoryEntry]) -> (String, usize) {
    let mut out = String::new();
    let mut left_out = 0;
    for entry in entries {
        let line = format!("- [{}] {}\n", entry.id, entry.text);
        if out.len() + line.len() > PROMPT_BUDGET_BYTES {
            left_out += 1;
            continue;
        }
        out.push_str(&line);
    }
    (out, left_out)
}

/// The memory section appended to the system prompt when a turn starts.
/// `writable` is false for sub-agents and swarm members.
pub fn prompt_section(root: &Path, settings: &ToolSettings, writable: bool) -> Option<String> {
    if !settings.memory_enabled {
        return None;
    }
    let user = load(MemoryScope::User, root);
    let project = load(MemoryScope::Project, root);
    if !writable && user.is_empty() && project.is_empty() {
        return None;
    }

    let mut out = String::from("# Memory\n\n");
    if writable {
        out.push_str(&format!(
            "You have a persistent memory that survives across conversations, edited with the `{}` tool. Two scopes: `user` (who the user is, their preferences and how they want you to work — shared by every project) and `project` (durable facts about this workspace that the code and git history do not show: decisions, conventions, known pitfalls, how to release).\n",
            tool_names::MEMORY
        ));
        if settings.memory_auto_save {
            out.push_str("Save an entry when the user asks you to remember something, when they correct you on something that will matter again, or when you learn a stable, non-obvious fact worth knowing next time. ");
        } else {
            out.push_str("Save an entry only when the user explicitly asks you to remember something. ");
        }
        out.push_str("Do not save what the code, the docs or git already record, the state of the current task, or anything secret. One short fact per entry. When an entry turns out wrong or outdated, update or remove it instead of adding a contradicting one. Changes apply from the next turn.\n");
    } else {
        out.push_str("Persistent memory the user's main agent keeps across conversations (read-only here):\n");
    }

    for (scope, entries, title) in [
        (MemoryScope::User, &user, "About the user (all projects)"),
        (MemoryScope::Project, &project, "This project"),
    ] {
        out.push_str(&format!("\n## {title}\n\n"));
        if entries.is_empty() {
            out.push_str("(empty)\n");
            continue;
        }
        let (rendered, left_out) = render_scope(entries);
        out.push_str(&rendered);
        if left_out > 0 {
            out.push_str(&format!(
                "({left_out} more {} entries are over the prompt budget and were left out{})\n",
                scope.label(),
                if writable {
                    "; merge or remove entries to make room"
                } else {
                    ""
                }
            ));
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The `memory` tool.

pub fn descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: tool_names::MEMORY.into(),
        description: "Edit your persistent memory, which is shown to you at the start of every conversation. `add` a new fact, `update` or `remove` an entry by its id (shown in brackets in the Memory section), or `list` the current entries. Scope `user` holds facts about the user for every project; `project` holds facts about this workspace.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "update", "remove", "list"]
                },
                "scope": {
                    "type": "string",
                    "enum": ["user", "project"],
                    "description": "Required for add, update and remove."
                },
                "id": {
                    "type": "string",
                    "description": "Entry id, for update and remove."
                },
                "text": {
                    "type": "string",
                    "description": "The fact, one short sentence, for add and update. Never credentials."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
struct MemoryInput {
    action: String,
    #[serde(default)]
    scope: Option<MemoryScope>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

pub fn run_tool(root: &Path, settings: &ToolSettings, input: Value) -> ToolRunResult {
    if !settings.memory_enabled {
        return ToolRunResult::err("memory is disabled in Settings", Vec::new());
    }
    let parsed: MemoryInput = match serde_json::from_value(input) {
        Ok(parsed) => parsed,
        Err(err) => return ToolRunResult::err(format!("invalid memory input: {err}"), Vec::new()),
    };
    let action = match parsed.action.as_str() {
        "list" => {
            let mut out = String::new();
            for scope in [MemoryScope::User, MemoryScope::Project] {
                let entries = load(scope, root);
                out.push_str(&format!("{} ({}):\n", scope.label(), entries.len()));
                for entry in entries {
                    out.push_str(&format!("- [{}] {}\n", entry.id, entry.text));
                }
            }
            return ToolRunResult::ok(out.trim_end().to_string(), Vec::new());
        }
        "add" => MemoryAction::Add,
        "update" => MemoryAction::Update,
        "remove" => MemoryAction::Remove,
        other => {
            return ToolRunResult::err(format!("unknown memory action `{other}`"), Vec::new())
        }
    };
    let Some(scope) = parsed.scope else {
        return ToolRunResult::err("scope is required (`user` or `project`)", Vec::new());
    };
    // Adding never takes an id from the model: ids are Sinew's.
    let id = if action == MemoryAction::Add {
        None
    } else {
        parsed.id.as_deref()
    };
    match apply(root, scope, action, id, parsed.text.as_deref()) {
        Ok(change) => {
            let verb = match change.action {
                MemoryAction::Add => "Saved",
                MemoryAction::Update => "Updated",
                MemoryAction::Remove => "Removed",
            };
            ToolRunResult::ok_with_meta(
                format!(
                    "{verb} {} memory [{}]: {}",
                    change.scope.label(),
                    change.id,
                    change.text
                ),
                Vec::new(),
                json!({ "memory": change }),
            )
        }
        Err(err) => ToolRunResult::err(err.to_string(), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_parse_with_and_without_ids() {
        let entries = parse_entries(
            "# Sinew memory\n\n<!-- note -->\n- [a1b2c] Answer in French\n* hand-written fact\nnot a bullet\n",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "a1b2c");
        assert_eq!(entries[0].text, "Answer in French");
        assert_eq!(entries[1].text, "hand-written fact");
        assert_eq!(entries[1].id, text_id("hand-written fact"));
    }

    #[test]
    fn secrets_are_refused() {
        assert!(clean_text("my key is sk-abcdefghijklmnopqrstuv").is_err());
        assert!(clean_text("password: hunter22").is_err());
        assert!(clean_text("ghp_abcdefghijklmnopqrstuvwxyz0123").is_err());
        assert!(clean_text("The release is triggered by pushing a vX.Y.Z tag").is_ok());
        assert!(clean_text("Tokens are refreshed by auth.rs").is_ok());
    }

    #[test]
    fn project_slugs_are_stable_and_distinct() {
        let a = project_slug(Path::new(r"C:\code\app"));
        let b = project_slug(Path::new(r"D:\other\app"));
        assert!(a.starts_with("app-"));
        assert_ne!(a, b);
        if cfg!(windows) {
            assert_eq!(a, project_slug(Path::new(r"\\?\C:\Code\App\")));
        }
    }

    #[test]
    fn writes_round_trip_and_undo() {
        let dir = std::env::temp_dir().join(format!("sinew-memory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("SINEW_MEMORY_DIR", &dir);
        let root = Path::new("/work/app");

        let added = apply(root, MemoryScope::Project, MemoryAction::Add, None, Some("Release by pushing a tag")).unwrap();
        assert!(apply(root, MemoryScope::Project, MemoryAction::Add, None, Some("Release by pushing a tag")).is_err());
        let updated = apply(
            root,
            MemoryScope::Project,
            MemoryAction::Update,
            Some(&added.id),
            Some("Release by pushing a vX.Y.Z tag"),
        )
        .unwrap();
        assert_eq!(updated.previous.as_deref(), Some("Release by pushing a tag"));
        assert_eq!(load(MemoryScope::Project, root)[0].text, "Release by pushing a vX.Y.Z tag");
        assert!(load(MemoryScope::User, root).is_empty());

        // Undoing a removal puts the entry back under the same id.
        let removed = apply(root, MemoryScope::Project, MemoryAction::Remove, Some(&added.id), None).unwrap();
        assert!(load(MemoryScope::Project, root).is_empty());
        apply(root, MemoryScope::Project, MemoryAction::Add, Some(&removed.id), Some(&removed.text)).unwrap();
        assert_eq!(load(MemoryScope::Project, root)[0].id, added.id);

        let mut settings = ToolSettings::default();
        let section = prompt_section(root, &settings, true).unwrap();
        assert!(section.contains(&format!("- [{}] Release by pushing a vX.Y.Z tag", added.id)));
        assert!(section.contains("## About the user (all projects)\n\n(empty)"));
        settings.memory_enabled = false;
        assert!(prompt_section(root, &settings, true).is_none());

        std::env::remove_var("SINEW_MEMORY_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompt_respects_the_budget() {
        let entries: Vec<MemoryEntry> = (0..200)
            .map(|index| MemoryEntry {
                id: format!("{index:05x}"),
                text: "x".repeat(100),
            })
            .collect();
        let (rendered, left_out) = render_scope(&entries);
        assert!(rendered.len() <= PROMPT_BUDGET_BYTES);
        assert!(left_out > 0);
        assert_eq!(rendered.lines().count() + left_out, 200);
    }
}
