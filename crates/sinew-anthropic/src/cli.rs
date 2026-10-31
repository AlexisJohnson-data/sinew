//! Finds the user's Claude Code install and reads its sign-in state.
//!
//! Sinew never signs in to Claude itself: the user runs `claude auth login`
//! (Anthropic's own flow) and Sinew drives the unmodified binary, which keeps
//! and refreshes its own credentials.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const STATUS_TIMEOUT: Duration = Duration::from_secs(15);

/// Sign-in state of the local Claude Code install.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeStatus {
    pub installed: bool,
    pub logged_in: bool,
    /// `claude.ai` (subscription), `console` / `api_key` (API billing), …
    pub auth_method: Option<String>,
    pub email: Option<String>,
    pub subscription: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatusBody {
    #[serde(default)]
    logged_in: bool,
    auth_method: Option<String>,
    email: Option<String>,
    subscription_type: Option<String>,
}

/// Path of the `claude` executable: `SINEW_CLAUDE_PATH`, then `PATH`, then
/// the installer's usual locations (a GUI app may not inherit the shell's
/// `PATH`).
pub fn claude_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SINEW_CLAUDE_PATH").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    let mut candidates = vec![home.join(".local").join("bin"), home.join(".claude").join("local")];
    if cfg!(windows) {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            candidates.push(Path::new(&app_data).join("npm"));
        }
    } else {
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
    }
    candidates
        .into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// A `claude` command with no console window and piped stdio. `None` when
/// Claude Code is not installed.
pub fn command() -> Option<Command> {
    let binary = claude_binary()?;
    let mut command = if binary
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"))
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(binary);
        command
    } else {
        Command::new(binary)
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    Some(command)
}

/// Runs `claude auth status`.
pub async fn status() -> ClaudeCodeStatus {
    let Some(mut command) = command() else {
        return ClaudeCodeStatus::default();
    };
    command.args(["auth", "status"]);
    let output = match tokio::time::timeout(STATUS_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        _ => {
            return ClaudeCodeStatus {
                installed: true,
                ..ClaudeCodeStatus::default()
            }
        }
    };
    let body = serde_json::from_slice::<AuthStatusBody>(&output.stdout).ok();
    ClaudeCodeStatus {
        installed: true,
        logged_in: body.as_ref().is_some_and(|body| body.logged_in),
        auth_method: body
            .as_ref()
            .and_then(|body| body.auth_method.clone())
            .filter(|method| method != "none"),
        email: body.as_ref().and_then(|body| body.email.clone()),
        subscription: body.and_then(|body| body.subscription_type),
    }
}

/// Opens Claude Code's own sign-in (`claude auth login`), which opens the
/// browser on Anthropic's page. On Windows it runs in a console window so
/// the user can see and finish it.
pub fn start_login() -> std::io::Result<tokio::process::Child> {
    let binary = claude_binary().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Claude Code is not installed")
    })?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // `start` takes the window title as its first quoted argument, which
        // Rust's argument quoting cannot express.
        let mut command = Command::new("cmd");
        command.as_std_mut().raw_arg(format!(
            "/C start \"Claude Code sign-in\" /WAIT \"{}\" auth login",
            binary.display()
        ));
        command.kill_on_drop(true).creation_flags(CREATE_NO_WINDOW);
        command.spawn()
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(binary);
        command
            .args(["auth", "login"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command.spawn()
    }
}
