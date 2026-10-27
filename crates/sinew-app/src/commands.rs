//! Slash commands and invocations.
//!
//! A slash command is a prompt template: `/review src/` sends the template of
//! `review` with `$ARGUMENTS` = `src/`. Templates come from Markdown files
//! (Claude Code compatible) or from the built-ins below:
//!
//! - project: `<workspace>/.sinew/commands/*.md`, `<workspace>/.claude/commands/*.md`
//! - global: `~/.sinew/commands/*.md`, `~/.claude/commands/*.md` (plus the
//!   WSL homes on Windows)
//! - built-in: `review`, `commit`, `push`, `init`, `test`
//!
//! Project beats global beats built-in, so a file can override a built-in.
//! Files may start with a `---` frontmatter carrying `description:` and
//! `argument-hint:`.
//!
//! This module also finds what the user *invoked* in a conversation
//! (`/skill-name`, `@mcp_server`) so disabled skills and MCP servers can be
//! switched on for that conversation only.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::Serialize;
use sinew_core::{ChatMessage, Part, Role};

use crate::mcp::McpSettings;
use crate::skill::{parse_frontmatter, SkillSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandSource {
    Project,
    Global,
    Builtin,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    pub source: CommandSource,
    /// File the command was read from (none for built-ins).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip)]
    pub template: String,
}

struct Builtin {
    name: &'static str,
    description: &'static str,
    argument_hint: Option<&'static str>,
    template: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "review",
        description: "Review the uncommitted changes for bugs",
        argument_hint: Some("[focus]"),
        template:
            "Review the uncommitted changes in this repository (`git status`, `git diff` and \
`git diff --staged`). Look for bugs, regressions, unhandled edge cases and security issues; skip \
style nitpicks. Report the findings ranked by severity, each with its file:line and a concrete \
fix. Do not modify any file unless I ask.
Focus on: $ARGUMENTS",
    },
    Builtin {
        name: "commit",
        description: "Commit the current changes with a good message",
        argument_hint: Some("[instructions]"),
        template: "Create a git commit for the current changes. Inspect `git status` and \
`git diff`, stage the relevant files (never secrets, build output or unrelated scratch files) and \
write a concise commit message that follows the style of `git log -5`. Do not push.
Extra instructions: $ARGUMENTS",
    },
    Builtin {
        name: "push",
        description: "Commit pending changes if any, then push the branch",
        argument_hint: Some("[instructions]"),
        template: "Push this repository's work to its remote:
1. Run `git status`. If there are uncommitted changes, review them, stage the relevant files \
(never secrets or build output) and commit them with a concise message that follows the style of \
`git log -5`.
2. Push the current branch. If it has no upstream yet, use `git push -u origin <branch>`.
3. If there is nothing to commit and nothing to push, just say so.
Never force-push. If the push is rejected because the remote moved, stop and explain the \
situation instead of rewriting history.
Extra instructions: $ARGUMENTS",
    },
    Builtin {
        name: "init",
        description: "Create or update AGENTS.md from the codebase",
        argument_hint: None,
        template: "Create or update `AGENTS.md` at the repository root so a coding agent can work \
here effectively: what the project is, how the code is laid out, the build / test / lint \
commands, and the conventions worth knowing. Explore the codebase first. Keep it concise and \
factual. If the file already exists, update it and keep the sections a human wrote.
Extra instructions: $ARGUMENTS",
    },
    Builtin {
        name: "test",
        description: "Run the tests and fix what fails",
        argument_hint: Some("[scope]"),
        template: "Run this project's test suite (work out the right command from the build \
files). If tests fail, find the root cause and fix the code — change a test only if the test \
itself is wrong — then rerun until everything passes. Report what failed and what you changed.
Scope: $ARGUMENTS",
    },
];

/// Every command available in `workspace_root`, deduplicated by name with
/// project > global > built-in, sorted by name.
pub fn discover_commands(workspace_root: &Path) -> Vec<SlashCommand> {
    let mut by_name: HashMap<String, SlashCommand> = HashMap::new();
    for builtin in BUILTINS {
        by_name.insert(
            builtin.name.to_string(),
            SlashCommand {
                name: builtin.name.to_string(),
                description: builtin.description.to_string(),
                argument_hint: builtin.argument_hint.map(str::to_string),
                source: CommandSource::Builtin,
                path: None,
                template: builtin.template.to_string(),
            },
        );
    }
    // Later directories win, so list global first and project last.
    let mut roots: Vec<(PathBuf, CommandSource)> = global_command_roots()
        .into_iter()
        .map(|root| (root, CommandSource::Global))
        .collect();
    for sub in [".claude", ".sinew"] {
        roots.push((
            workspace_root.join(sub).join("commands"),
            CommandSource::Project,
        ));
    }
    for (root, source) in roots {
        for command in read_command_dir(&root, source) {
            by_name.insert(command.name.clone(), command);
        }
    }
    let mut commands: Vec<SlashCommand> = by_name.into_values().collect();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands
}

pub fn find_command(workspace_root: &Path, name: &str) -> Option<SlashCommand> {
    discover_commands(workspace_root)
        .into_iter()
        .find(|command| command.name == name)
}

fn global_command_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dirs) = BaseDirs::new() {
        for sub in [".claude", ".sinew"] {
            roots.push(dirs.home_dir().join(sub).join("commands"));
        }
    }
    roots.extend(wsl_command_roots());
    roots
}

/// `~/.claude/commands` and `~/.sinew/commands` inside each WSL distro, since
/// Sinew's own home is the Windows one. Cached like the skill roots.
fn wsl_command_roots() -> Vec<PathBuf> {
    #[cfg(not(windows))]
    {
        Vec::new()
    }
    #[cfg(windows)]
    {
        use std::sync::{Mutex, OnceLock};
        use std::time::{Duration, Instant};

        static CACHE: OnceLock<Mutex<Option<(Instant, Vec<PathBuf>)>>> = OnceLock::new();
        const TTL: Duration = Duration::from_secs(300);
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let mut guard = cache.lock().unwrap_or_else(|err| err.into_inner());
        if let Some((cached_at, roots)) = guard.as_ref() {
            if cached_at.elapsed() < TTL {
                return roots.clone();
            }
        }
        let mut roots = Vec::new();
        for distro in crate::skill::wsl_distros() {
            let home_base = PathBuf::from(format!(r"\\wsl.localhost\{distro}\home"));
            let Ok(entries) = fs::read_dir(&home_base) else {
                continue;
            };
            for entry in entries.flatten() {
                for sub in [".claude", ".sinew"] {
                    let root = entry.path().join(sub).join("commands");
                    if root.is_dir() {
                        roots.push(root);
                    }
                }
            }
        }
        *guard = Some((Instant::now(), roots.clone()));
        roots
    }
}

fn read_command_dir(root: &Path, source: CommandSource) -> Vec<SlashCommand> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_command_name(name) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        commands.push(parse_command(name, &content, source, Some(path.clone())));
    }
    commands
}

fn parse_command(
    name: &str,
    content: &str,
    source: CommandSource,
    path: Option<PathBuf>,
) -> SlashCommand {
    let fields = parse_frontmatter(content);
    let template = strip_frontmatter(content).trim().to_string();
    let description = fields
        .get("description")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| first_line_summary(&template));
    SlashCommand {
        name: name.to_string(),
        description,
        argument_hint: fields
            .get("argument-hint")
            .filter(|value| !value.is_empty())
            .cloned(),
        source,
        path,
        template,
    }
}

fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return trimmed;
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            after.split_once('\n').map(|(_, body)| body).unwrap_or("")
        }
        None => trimmed,
    }
}

fn first_line_summary(template: &str) -> String {
    let line = template
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .trim_start_matches('#')
        .trim();
    let summary: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        format!("{summary}…")
    } else {
        summary
    }
}

fn is_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// `/name rest of the line` at the very start of a message.
pub fn leading_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_start().strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..end];
    is_command_name(name).then(|| (name, rest[end..].trim()))
}

/// Fill a template: `$ARGUMENTS` gets the whole argument string, `$1`…`$9`
/// the individual words. A line whose only dynamic part is an empty
/// `$ARGUMENTS` is dropped; arguments to a template that never mentions them
/// are appended at the end.
pub fn expand(template: &str, arguments: &str) -> String {
    let arguments = arguments.trim();
    let words: Vec<&str> = arguments.split_whitespace().collect();
    let mut lines = Vec::new();
    for line in template.lines() {
        if line.contains("$ARGUMENTS") && arguments.is_empty() {
            continue;
        }
        let mut expanded = line.replace("$ARGUMENTS", arguments);
        for index in (1..=9).rev() {
            let placeholder = format!("${index}");
            if expanded.contains(&placeholder) {
                expanded =
                    expanded.replace(&placeholder, words.get(index - 1).copied().unwrap_or(""));
            }
        }
        lines.push(expanded);
    }
    let mut body = lines.join("\n").trim_end().to_string();
    let mentions_arguments = template.contains("$ARGUMENTS")
        || (1..=9).any(|index| template.contains(&format!("${index}")));
    if !arguments.is_empty() && !mentions_arguments {
        body.push_str(&format!("\n\nArguments: {arguments}"));
    }
    body
}

/// Names the user typed as `/name` or `@name` in the visible text of their
/// messages. `@server.tool` counts for `server`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Invocations {
    pub slash: BTreeSet<String>,
    pub at: BTreeSet<String>,
}

pub fn invocations(history: &[ChatMessage]) -> Invocations {
    let mut found = Invocations::default();
    for message in history.iter().filter(|message| message.role == Role::User) {
        for part in &message.parts {
            // Hidden parts (expansions, reminders, attachments) carry meta.
            let Part::Text { text, meta: None } = part else {
                continue;
            };
            collect_tokens(text, &mut found);
        }
    }
    found
}

/// Switch on, for this conversation, the disabled skills and MCP servers the
/// user invoked by name (`/skill-name`, `@server`) anywhere in it. What the
/// user did not name stays off, so the agent cannot reach for it on its own.
/// Returns the names that were switched on.
pub fn apply_invocations(
    history: &[ChatMessage],
    skills: &mut SkillSettings,
    mcp: &mut McpSettings,
) -> Vec<String> {
    let found = invocations(history);
    let mut activated = Vec::new();
    for name in &found.slash {
        match skills.skills.iter_mut().find(|skill| &skill.name == name) {
            Some(skill) if !skill.enabled => {
                skill.enabled = true;
                activated.push(format!("/{name}"));
            }
            Some(_) => {}
            // Unknown to the settings means enabled already (the default).
            None => {}
        }
    }
    for server in mcp.servers.iter_mut().filter(|server| !server.enabled) {
        if found.at.contains(&server.mention_name()) {
            server.enabled = true;
            activated.push(format!("@{}", server.mention_name()));
        }
    }
    activated
}

fn collect_tokens(text: &str, found: &mut Invocations) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let sigil = chars[i];
        let at_boundary = i == 0 || chars[i - 1].is_whitespace();
        if (sigil == '/' || sigil == '@') && at_boundary {
            let start = i + 1;
            let mut end = start;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric()
                    || matches!(chars[end], '-' | '_' | '.' | ':'))
            {
                end += 1;
            }
            let token: String = chars[start..end].iter().collect();
            let token = token.trim_end_matches(['.', ':']).to_string();
            if !token.is_empty() {
                if sigil == '/' {
                    found.slash.insert(token);
                } else {
                    let server = token.split('.').next().unwrap_or(&token).to_lowercase();
                    found.at.insert(server);
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::SkillConfig;

    fn disabled_skill(name: &str) -> SkillConfig {
        SkillConfig {
            name: name.to_string(),
            enabled: false,
        }
    }

    #[test]
    fn leading_command_splits_name_and_arguments() {
        assert_eq!(
            leading_command("/review src/ lib"),
            Some(("review", "src/ lib"))
        );
        assert_eq!(leading_command("  /push"), Some(("push", "")));
        assert_eq!(leading_command("please /review"), None);
        assert_eq!(leading_command("/"), None);
        assert_eq!(leading_command("/src/main.rs is broken"), None);
    }

    #[test]
    fn expand_fills_and_drops_argument_lines() {
        let template = "Do the thing.\nFocus on: $ARGUMENTS\nFirst: $1";
        assert_eq!(
            expand(template, "auth api"),
            "Do the thing.\nFocus on: auth api\nFirst: auth"
        );
        assert_eq!(expand(template, ""), "Do the thing.\nFirst:");
        assert_eq!(
            expand("Plain body", "extra"),
            "Plain body\n\nArguments: extra"
        );
    }

    #[test]
    fn parse_command_reads_frontmatter() {
        let command = parse_command(
            "deploy",
            "---\ndescription: Deploy to staging\nargument-hint: <env>\n---\nDeploy to $ARGUMENTS now.\n",
            CommandSource::Project,
            None,
        );
        assert_eq!(command.description, "Deploy to staging");
        assert_eq!(command.argument_hint.as_deref(), Some("<env>"));
        assert_eq!(command.template, "Deploy to $ARGUMENTS now.");

        let bare = parse_command(
            "notes",
            "# Summarise the notes\nBody",
            CommandSource::Global,
            None,
        );
        assert_eq!(bare.description, "Summarise the notes");
    }

    #[test]
    fn project_files_override_builtins() {
        let root = std::env::temp_dir().join(format!("sinew-commands-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".sinew/commands")).unwrap();
        fs::write(
            root.join(".sinew/commands/push.md"),
            "Custom push for $ARGUMENTS",
        )
        .unwrap();
        fs::write(root.join(".sinew/commands/ship.md"), "Ship it").unwrap();
        fs::write(root.join(".sinew/commands/ignored.txt"), "nope").unwrap();

        let commands = discover_commands(&root);
        let push = commands.iter().find(|c| c.name == "push").unwrap();
        assert_eq!(push.source, CommandSource::Project);
        assert_eq!(push.template, "Custom push for $ARGUMENTS");
        assert!(commands.iter().any(|c| c.name == "ship"));
        assert!(commands
            .iter()
            .any(|c| c.name == "review" && c.source == CommandSource::Builtin));
        assert!(!commands.iter().any(|c| c.name == "ignored"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invoking_turns_on_disabled_skills_and_servers_only() {
        let mut skills = SkillSettings {
            skills: vec![disabled_skill("frontend-design"), disabled_skill("pdf")],
        };
        let mut mcp: McpSettings = serde_json::from_value(serde_json::json!({
            "servers": [
                { "id": "1", "name": "GitHub", "command": "gh-mcp", "enabled": false },
                { "id": "2", "name": "Figma", "command": "figma-mcp", "enabled": false }
            ]
        }))
        .unwrap();
        let history = vec![ChatMessage::user_text(
            "/frontend-design for the page, file it with @github",
        )];
        let activated = apply_invocations(&history, &mut skills, &mut mcp);
        assert_eq!(activated, vec!["/frontend-design", "@github"]);
        assert!(skills.is_enabled("frontend-design"));
        assert!(!skills.is_enabled("pdf"));
        assert!(mcp.servers[0].enabled);
        assert!(!mcp.servers[1].enabled);
    }

    #[test]
    fn invocations_come_from_visible_user_text() {
        let mut hidden = ChatMessage::user_text("/secret @hidden");
        if let Part::Text { meta, .. } = &mut hidden.parts[0] {
            *meta = Some(serde_json::json!({ "system_reminder": true }));
        }
        let history = vec![
            ChatMessage::user_text(
                "use /frontend-design then ask @github.create_issue, see @src/main.rs",
            ),
            ChatMessage::assistant_text("/not-from-user @nope"),
            hidden,
            ChatMessage::user_text("mail me@example.com or https://x.y/a"),
        ];
        let found = invocations(&history);
        assert_eq!(
            found.slash.into_iter().collect::<Vec<_>>(),
            vec!["frontend-design".to_string()]
        );
        // `@src/main.rs` yields `src`, which simply matches no MCP server.
        assert!(found.at.contains("github"));
        assert!(!found.at.contains("hidden") && !found.at.contains("nope"));
        assert!(!found.at.iter().any(|name| name.contains("example")));
    }
}
