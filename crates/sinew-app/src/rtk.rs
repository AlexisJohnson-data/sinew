use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::bash::is_wsl_shell_active;

const RTK_ENV: &str = "SINEW_RTK_PATH";

static SUPPORTED_COMMANDS: &[&str] = &[
    "git", "gh", "cargo", "npm", "pnpm", "npx", "tsc", "next", "vitest",
    "playwright", "prettier", "lint", "docker", "kubectl", "curl", "wget",
    "pytest", "ruff", "pip", "go", "golangci-lint", "prisma",
];

static RTK_NATIVE: OnceLock<Option<PathBuf>> = OnceLock::new();
static RTK_WSL: OnceLock<Option<bool>> = OnceLock::new();

fn find_executable_in_path(name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn resolve_native_rtk() -> Option<PathBuf> {
    env::var_os(RTK_ENV)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            #[cfg(windows)]
            {
                find_executable_in_path("rtk.exe")
                    .or_else(|| find_executable_in_path("rtk"))
            }
            #[cfg(not(windows))]
            {
                find_executable_in_path("rtk")
            }
        })
        .or_else(|| {
            #[cfg(not(windows))]
            let known: &[&str] = &[
                "/usr/local/bin/rtk",
                "/opt/homebrew/bin/rtk",
            ];
            #[cfg(windows)]
            let known: &[&str] = &[
                r"C:\Program Files\rtk-x86_64-pc-windows-msvc\rtk.exe",
            ];
            known.iter().map(Path::new).find(|p| p.is_file()).map(PathBuf::from)
        })
}

fn probe_wsl_rtk() -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("wsl.exe")
            .args(["--", "which", "rtk"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn rtk_available_for_context() -> bool {
    if is_wsl_shell_active() {
        *RTK_WSL.get_or_init(|| Some(probe_wsl_rtk()))
            == Some(true)
    } else {
        RTK_NATIVE.get_or_init(|| resolve_native_rtk()).is_some()
    }
}

fn rtk_prefix() -> &'static str {
    if is_wsl_shell_active() {
        "rtk"
    } else {
        RTK_NATIVE
            .get()
            .and_then(|opt| opt.as_ref())
            .and_then(|p| p.to_str())
            .unwrap_or("rtk")
    }
}

fn is_supported(cmd: &str) -> bool {
    SUPPORTED_COMMANDS.iter().any(|&c| c == cmd)
}

fn rewrite_segment(segment: &str) -> String {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return segment.to_string();
    }

    let first_token = trimmed.split_whitespace().next().unwrap_or("");

    if first_token == "rtk" {
        return segment.to_string();
    }

    let base_cmd = first_token.rsplit('/').next().unwrap_or(first_token);
    #[cfg(windows)]
    let base_cmd = base_cmd.strip_suffix(".exe").unwrap_or(base_cmd);

    if !is_supported(base_cmd) {
        return segment.to_string();
    }

    let leading_ws: &str = &segment[..segment.len() - trimmed.len()];
    format!("{leading_ws}{} {trimmed}", rtk_prefix())
}

struct CommandSplitter<'a> {
    remaining: &'a str,
}

impl<'a> CommandSplitter<'a> {
    fn new(input: &'a str) -> Self {
        Self { remaining: input }
    }
}

impl<'a> Iterator for CommandSplitter<'a> {
    type Item = (&'a str, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        let bytes = self.remaining.as_bytes();
        let mut i = 0;
        let mut in_single = false;
        let mut in_double = false;

        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\\' && !in_single && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == b'\'' && !in_double {
                in_single = !in_single;
                i += 1;
                continue;
            }
            if b == b'"' && !in_single {
                in_double = !in_double;
                i += 1;
                continue;
            }
            if in_single || in_double {
                i += 1;
                continue;
            }

            if b == b'|' && i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                let segment = &self.remaining[..i];
                let sep = &self.remaining[i..i + 2];
                self.remaining = &self.remaining[i + 2..];
                return Some((segment, sep));
            }
            if b == b'&' && i + 1 < bytes.len() && bytes[i + 1] == b'&' {
                let segment = &self.remaining[..i];
                let sep = &self.remaining[i..i + 2];
                self.remaining = &self.remaining[i + 2..];
                return Some((segment, sep));
            }
            if b == b';' {
                let segment = &self.remaining[..i];
                let sep = &self.remaining[i..i + 1];
                self.remaining = &self.remaining[i + 1..];
                return Some((segment, sep));
            }
            if b == b'|' {
                let segment = &self.remaining[..i];
                let sep = &self.remaining[i..i + 1];
                self.remaining = &self.remaining[i + 1..];
                return Some((segment, sep));
            }

            i += 1;
        }

        let segment = self.remaining;
        self.remaining = "";
        Some((segment, ""))
    }
}

pub struct RtkRewrite {
    pub original: String,
    pub rewritten: String,
}

pub fn maybe_rewrite_command(command: &str) -> Option<RtkRewrite> {
    if !rtk_available_for_context() {
        return None;
    }

    let mut result = String::with_capacity(command.len() + 64);
    let mut any_changed = false;

    for (segment, separator) in CommandSplitter::new(command) {
        let rewritten = rewrite_segment(segment);
        if rewritten != segment {
            any_changed = true;
        }
        result.push_str(&rewritten);
        result.push_str(separator);
    }

    if any_changed {
        tracing::debug!(original = command, rewritten = %result, "rtk rewrite");
        Some(RtkRewrite {
            original: command.to_string(),
            rewritten: result,
        })
    } else {
        None
    }
}

pub fn reset_cache() {
    // OnceLock has no reset, but we can use this as a marker for tests.
    // In practice the cache lives for the process lifetime, which is correct:
    // RTK doesn't appear/disappear mid-session.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rewrite_when_already_prefixed() {
        let seg = rewrite_segment("rtk git status");
        assert_eq!(seg, "rtk git status");
    }

    #[test]
    fn no_rewrite_for_unsupported_command() {
        let seg = rewrite_segment("echo hello");
        assert_eq!(seg, "echo hello");
    }

    #[test]
    fn splits_chain_correctly() {
        let parts: Vec<_> = CommandSplitter::new("git add . && git commit -m 'x' && git push")
            .collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], ("git add . ", "&&"));
        assert_eq!(parts[1], (" git commit -m 'x' ", "&&"));
        assert_eq!(parts[2], (" git push", ""));
    }

    #[test]
    fn does_not_split_inside_quotes() {
        let parts: Vec<_> = CommandSplitter::new(r#"echo "a && b" && git status"#)
            .collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].0.contains("a && b"));
    }

    #[test]
    fn pipe_handled_separately() {
        let parts: Vec<_> = CommandSplitter::new("git log | head -5")
            .collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], ("git log ", "|"));
    }

    #[test]
    fn rewrite_preserves_leading_whitespace() {
        let seg = rewrite_segment("  git status");
        assert!(seg.starts_with("  "), "leading whitespace lost: {seg:?}");
    }
}
