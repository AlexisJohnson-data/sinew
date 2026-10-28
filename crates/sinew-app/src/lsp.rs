//! Language-server diagnostics for the agent.
//!
//! Sinew starts the project's language servers (rust-analyzer,
//! typescript-language-server, pyright, gopls) on demand, keeps them alive per
//! workspace, and asks them for diagnostics after every `edit_file` /
//! `write_file`, so the agent sees the errors it just introduced without
//! waiting for a full build. Nothing is installed automatically: a server that
//! is not found is skipped. In a `\\wsl$\…` workspace the servers run inside
//! WSL and paths are translated between the UNC and Linux forms.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicI64, AtomicU64, Ordering},
        Arc, Mutex as StdMutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sinew_core::ToolDescriptor;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{oneshot, watch, Mutex, Notify},
};

use crate::{
    bash::path_targets_wsl_filesystem,
    store::ToolSettings,
    tool_names,
    tool_run::{FileChangeKind, ToolRunResult},
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// How long an edit waits for the server's verdict before moving on.
const AFTER_EDIT_BUDGET: Duration = Duration::from_secs(3);
/// How long the `diagnostics` tool waits.
const TOOL_BUDGET: Duration = Duration::from_secs(15);
/// After the first fresh report for a file, wait this long for follow-up
/// reports (servers often publish syntax errors first, then semantic ones).
const SETTLE: Duration = Duration::from_millis(300);
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(90);
/// A server that failed is retried after this delay.
const RETRY_AFTER: Duration = Duration::from_secs(120);
const MAX_AFTER_EDIT_LISTED: usize = 20;
const MAX_TOOL_LISTED: usize = 100;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MESSAGE_CHARS: usize = 400;
const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    TypeScript,
    Python,
    Go,
}

pub const LANGUAGES: [Language; 4] = [
    Language::Rust,
    Language::TypeScript,
    Language::Python,
    Language::Go,
];

impl Language {
    pub fn id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Go => "go",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::TypeScript => "TypeScript / JavaScript",
            Self::Python => "Python",
            Self::Go => "Go",
        }
    }

    pub fn server(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript-language-server",
            Self::Python => "pyright-langserver",
            Self::Go => "gopls",
        }
    }

    fn args(self) -> &'static [&'static str] {
        match self {
            Self::TypeScript | Self::Python => &["--stdio"],
            Self::Rust | Self::Go => &[],
        }
    }

    pub fn install_hint(self) -> &'static str {
        match self {
            Self::Rust => "rustup component add rust-analyzer",
            Self::TypeScript => "npm install -g typescript typescript-language-server",
            Self::Python => "npm install -g pyright",
            Self::Go => "go install golang.org/x/tools/gopls@latest",
        }
    }

    fn markers(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["Cargo.toml"],
            Self::TypeScript => &["tsconfig.json", "jsconfig.json", "package.json"],
            Self::Python => &["pyproject.toml", "requirements.txt", "setup.py", "setup.cfg"],
            Self::Go => &["go.mod"],
        }
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Some(Self::TypeScript),
            "py" | "pyi" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    fn language_id(self, path: &Path) -> &'static str {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
            Self::TypeScript => match extension.as_str() {
                "tsx" => "typescriptreact",
                "jsx" => "javascriptreact",
                "js" | "mjs" | "cjs" => "javascript",
                _ => "typescript",
            },
        }
    }

    /// Settings the server receives at startup and when it asks for its
    /// configuration section.
    fn settings(self) -> Value {
        match self {
            // A separate target directory keeps rust-analyzer's `cargo check`
            // from holding the build lock while the agent runs cargo itself.
            Self::Rust => json!({ "cargo": { "targetDir": true } }),
            _ => json!({}),
        }
    }

    fn settings_section(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript",
            Self::Python => "python",
            Self::Go => "gopls",
        }
    }

    fn detected_in(self, root: &Path) -> bool {
        self.markers().iter().any(|marker| root.join(marker).exists())
    }
}

// ---------------------------------------------------------------------------
// Paths: what the server sees vs. what Windows sees.

#[derive(Debug, Clone, PartialEq, Eq)]
enum Host {
    Native,
    /// Workspace under `\\wsl$\<distro>\…`: the server runs in that distro.
    Wsl { distro: String },
}

#[derive(Debug, Clone)]
struct PathMap {
    host: Host,
}

impl PathMap {
    fn for_root(root: &Path) -> Self {
        if cfg!(windows) && path_targets_wsl_filesystem(root) {
            if let Some((distro, _)) = split_wsl_unc(&root.to_string_lossy()) {
                return Self {
                    host: Host::Wsl { distro },
                };
            }
        }
        Self { host: Host::Native }
    }

    fn is_wsl(&self) -> bool {
        matches!(self.host, Host::Wsl { .. })
    }

    /// The path as the server sees it, with forward slashes.
    fn server_path(&self, path: &Path) -> Option<String> {
        let text = path.to_string_lossy();
        match &self.host {
            Host::Wsl { .. } => split_wsl_unc(&text).map(|(_, linux)| linux),
            Host::Native => {
                let plain = text.strip_prefix(r"\\?\").unwrap_or(&text);
                Some(plain.replace('\\', "/"))
            }
        }
    }

    fn uri(&self, path: &Path) -> Option<String> {
        let server_path = self.server_path(path)?;
        let mut url = url::Url::parse("file:///").ok()?;
        if server_path.starts_with('/') {
            url.set_path(&server_path);
        } else {
            url.set_path(&format!("/{server_path}"));
        }
        Some(url.to_string())
    }

    /// Comparison key for a server-side path. Windows paths are compared
    /// case-insensitively; servers also differ in how they spell drive letters
    /// (`C:` vs `c%3A`).
    fn key_for_server_path(&self, server_path: &str) -> String {
        if self.host == Host::Native && cfg!(windows) {
            let trimmed = strip_drive_slash(server_path);
            trimmed.replace('\\', "/").to_lowercase()
        } else {
            server_path.to_string()
        }
    }

    fn key(&self, path: &Path) -> Option<String> {
        self.server_path(path)
            .map(|server_path| self.key_for_server_path(&server_path))
    }

    /// Decoded server-side path of a `file://` URI (drive letter normalized).
    fn decode_uri(&self, uri: &str) -> Option<String> {
        let url = url::Url::parse(uri).ok()?;
        if url.scheme() != "file" {
            return None;
        }
        let decoded = percent_decode(url.path());
        Some(if self.host == Host::Native && cfg!(windows) {
            strip_drive_slash(&decoded).to_string()
        } else {
            decoded
        })
    }
}

/// `\\wsl.localhost\Ubuntu\home\me` (any spelling) → (`Ubuntu`, `/home/me`).
fn split_wsl_unc(text: &str) -> Option<(String, String)> {
    let rest = text
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| text.strip_prefix(r"\\"))?;
    let mut parts = rest.splitn(3, '\\');
    let host = parts.next()?.to_ascii_lowercase();
    if host != "wsl$" && host != "wsl.localhost" {
        return None;
    }
    let distro = parts.next().filter(|value| !value.is_empty())?.to_string();
    let linux = format!("/{}", parts.next().unwrap_or("").replace('\\', "/"));
    let linux = if linux.len() > 1 {
        linux.trim_end_matches('/').to_string()
    } else {
        linux
    };
    Some((distro, linux))
}

fn strip_drive_slash(path: &str) -> &str {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':'
    {
        &path[1..]
    } else {
        path
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| (byte as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn posix_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// Finding the server binaries.

/// Resolved way to start a server.
#[derive(Debug, Clone)]
struct Launch {
    /// Path (native) or command name/path inside WSL.
    program: String,
}

fn native_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(base) = directories::BaseDirs::new() {
        let home = base.home_dir();
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join("go").join("bin"));
        dirs.push(home.join(".local").join("bin"));
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    #[cfg(not(windows))]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

fn find_native(name: &str) -> Option<PathBuf> {
    let extensions: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    native_search_dirs().into_iter().find_map(|dir| {
        extensions
            .iter()
            .map(|extension| dir.join(format!("{name}{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

type WslLookupCache = StdMutex<HashMap<(String, String), (Instant, Option<String>)>>;

fn wsl_lookup_cache() -> &'static WslLookupCache {
    static CACHE: OnceLock<WslLookupCache> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `command -v <name>` in the distro's login shell, cached for a few minutes
/// because each probe costs a `wsl.exe` round trip.
async fn find_in_wsl(distro: &str, name: &str) -> Option<String> {
    let key = (distro.to_string(), name.to_string());
    if let Some((at, found)) = wsl_lookup_cache().lock().ok()?.get(&key).cloned() {
        if at.elapsed() < Duration::from_secs(300) {
            return found;
        }
    }
    let mut command = Command::new("wsl.exe");
    command
        .args(["-d", distro, "--", "bash", "-lc"])
        .arg(format!("command -v {}", posix_single_quote(name)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let found = match tokio::time::timeout(Duration::from_secs(15), command.output()).await {
        Ok(Ok(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with('/'))
            .map(str::to_string),
        _ => None,
    };
    if let Ok(mut cache) = wsl_lookup_cache().lock() {
        cache.insert(key, (Instant::now(), found.clone()));
    }
    found
}

/// rustup puts a `rust-analyzer` proxy on PATH even when the component is not
/// installed; it then fails with "Unknown binary". `--version` tells them
/// apart (cached, since it costs a process launch).
async fn rustup_proxy_works(program: &Path) -> bool {
    static CACHE: OnceLock<StdMutex<HashMap<PathBuf, (Instant, bool)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, works)) = cache.lock().ok().and_then(|c| c.get(program).copied()) {
        if at.elapsed() < Duration::from_secs(300) {
            return works;
        }
    }
    let mut command = Command::new(program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let works = matches!(
        tokio::time::timeout(Duration::from_secs(10), command.status()).await,
        Ok(Ok(status)) if status.success()
    );
    if let Ok(mut cache) = cache.lock() {
        cache.insert(program.to_path_buf(), (Instant::now(), works));
    }
    works
}

async fn resolve_launch(
    language: Language,
    paths: &PathMap,
    settings: &ToolSettings,
) -> Option<Launch> {
    let custom = settings
        .lsp_server_paths
        .get(language.id())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());
    match &paths.host {
        Host::Native => {
            let program = match custom {
                Some(custom) => {
                    let path = PathBuf::from(custom);
                    if path.is_file() {
                        path
                    } else {
                        find_native(custom)?
                    }
                }
                None => find_native(language.server())?,
            };
            if language == Language::Rust && !rustup_proxy_works(&program).await {
                return None;
            }
            Some(Launch {
                program: program.to_string_lossy().into_owned(),
            })
        }
        Host::Wsl { distro } => {
            let program = find_in_wsl(distro, custom.unwrap_or(language.server())).await?;
            Some(Launch { program })
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostics as Sinew reports them.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn from_lsp(value: Option<i64>) -> Self {
        match value {
            Some(2) => Self::Warning,
            Some(3) => Self::Info,
            Some(4) => Self::Hint,
            _ => Self::Error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// 1-based.
    pub line: u32,
    /// 1-based, in UTF-16 code units as the protocol counts them.
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn parse_diagnostic(value: &Value) -> Option<Diagnostic> {
    let start = value.get("range")?.get("start")?;
    let line = start.get("line")?.as_u64()? as u32 + 1;
    let column = start.get("character")?.as_u64()? as u32 + 1;
    let code = match value.get("code") {
        Some(Value::String(code)) => Some(code.clone()),
        Some(Value::Number(code)) => Some(code.to_string()),
        _ => None,
    };
    let message = value.get("message")?.as_str()?;
    Some(Diagnostic {
        line,
        column,
        severity: Severity::from_lsp(value.get("severity").and_then(Value::as_i64)),
        message: one_line(message),
        code,
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn one_line(message: &str) -> String {
    let joined = message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" — ");
    if joined.chars().count() > MAX_MESSAGE_CHARS {
        let cut: String = joined.chars().take(MAX_MESSAGE_CHARS).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

// ---------------------------------------------------------------------------
// One running language server.

#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerState {
    Starting,
    Ready,
    Failed(String),
}

struct OpenDoc {
    version: i64,
    text: String,
    /// Local (Windows-side) absolute path, to re-check it later.
    path: PathBuf,
}

struct Published {
    seq: u64,
    version: Option<i64>,
    /// Decoded server-side path, original casing, for display.
    path: String,
    items: Vec<Diagnostic>,
}

enum Outcome {
    Checked(Vec<Diagnostic>),
    NotReady,
    Failed(String),
}

struct Server {
    language: Language,
    paths: PathMap,
    root_server_path: String,
    program: String,
    stdin: Mutex<ChildStdin>,
    child: StdMutex<Option<Child>>,
    next_id: AtomicI64,
    pending: StdMutex<HashMap<i64, oneshot::Sender<Value>>>,
    state: watch::Sender<ServerState>,
    failed_at: StdMutex<Option<Instant>>,
    progress: StdMutex<HashSet<String>>,
    docs: Mutex<HashMap<String, OpenDoc>>,
    published: StdMutex<HashMap<String, Published>>,
    publish_seq: AtomicU64,
    published_notify: Notify,
    stderr_tail: Arc<StdMutex<VecDeque<String>>>,
}

impl Server {
    async fn spawn(language: Language, root: &Path, launch: Launch) -> Result<Arc<Self>> {
        let paths = PathMap::for_root(root);
        let root_server_path = paths
            .server_path(root)
            .ok_or_else(|| anyhow!("unsupported workspace path {}", root.display()))?;
        let mut command = match &paths.host {
            Host::Native => {
                let mut command = Command::new(&launch.program);
                command.args(language.args()).current_dir(root);
                command
            }
            Host::Wsl { distro } => {
                let mut line = format!("exec {}", posix_single_quote(&launch.program));
                for arg in language.args() {
                    line.push(' ');
                    line.push_str(arg);
                }
                let mut command = Command::new("wsl.exe");
                command
                    .args(["-d", distro, "--cd", &root_server_path, "--", "bash", "-lc"])
                    .arg(line);
                command
            }
        };
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command
            .spawn()
            .with_context(|| format!("unable to start {}", launch.program))?;
        let stdin = child.stdin.take().context("server stdin unavailable")?;
        let stdout = child.stdout.take().context("server stdout unavailable")?;
        let stderr = child.stderr.take().context("server stderr unavailable")?;

        let (state, _) = watch::channel(ServerState::Starting);
        let server = Arc::new(Self {
            language,
            paths,
            root_server_path,
            program: launch.program,
            stdin: Mutex::new(stdin),
            child: StdMutex::new(Some(child)),
            next_id: AtomicI64::new(1),
            pending: StdMutex::new(HashMap::new()),
            state,
            failed_at: StdMutex::new(None),
            progress: StdMutex::new(HashSet::new()),
            docs: Mutex::new(HashMap::new()),
            published: StdMutex::new(HashMap::new()),
            publish_seq: AtomicU64::new(0),
            published_notify: Notify::new(),
            stderr_tail: Arc::new(StdMutex::new(VecDeque::new())),
        });

        tokio::spawn(drain_stderr(stderr, server.stderr_tail.clone()));
        tokio::spawn(read_loop(Arc::downgrade(&server), stdout));
        let initializing = server.clone();
        tokio::spawn(async move {
            if let Err(err) = initializing.initialize().await {
                initializing.fail(format!("initialization failed: {err}"));
            }
        });
        Ok(server)
    }

    fn state(&self) -> ServerState {
        self.state.borrow().clone()
    }

    fn fail(&self, reason: String) {
        // The server's last words on stderr usually say why.
        let last_line = self.stderr_tail.lock().ok().and_then(|tail| {
            tail.iter()
                .rev()
                .map(|line| line.trim())
                .find(|line| !line.is_empty())
                .map(str::to_string)
        });
        let reason = match last_line {
            Some(line) => format!("{reason}: {line}"),
            None => reason,
        };
        tracing::warn!(server = self.language.server(), %reason, "language server stopped");
        if let Ok(mut failed_at) = self.failed_at.lock() {
            failed_at.get_or_insert_with(Instant::now);
        }
        self.state.send_replace(ServerState::Failed(reason));
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
        self.published_notify.notify_waiters();
    }

    fn retry_due(&self) -> bool {
        self.failed_at
            .lock()
            .ok()
            .and_then(|at| *at)
            .is_some_and(|at| at.elapsed() >= RETRY_AFTER)
    }

    fn indexing(&self) -> bool {
        self.progress
            .lock()
            .map(|progress| !progress.is_empty())
            .unwrap_or(false)
    }

    async fn initialize(&self) -> Result<()> {
        let root_uri = self
            .root_uri()
            .ok_or_else(|| anyhow!("invalid workspace path"))?;
        let name = self
            .root_server_path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("workspace")
            .to_string();
        let process_id = if self.paths.is_wsl() {
            Value::Null
        } else {
            json!(std::process::id())
        };
        let params = json!({
            "processId": process_id,
            "clientInfo": { "name": "Sinew" },
            "rootPath": self.root_server_path,
            "rootUri": root_uri,
            "workspaceFolders": [{ "uri": root_uri, "name": name }],
            "initializationOptions": self.language.settings(),
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "window": { "workDoneProgress": true },
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                },
                "textDocument": {
                    "synchronization": { "didSave": true, "dynamicRegistration": false },
                    "publishDiagnostics": { "versionSupport": true, "relatedInformation": false },
                },
            },
        });
        self.request("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        self.notify("initialized", json!({})).await?;
        self.state.send_replace(ServerState::Ready);
        Ok(())
    }

    fn root_uri(&self) -> Option<String> {
        let mut url = url::Url::parse("file:///").ok()?;
        if self.root_server_path.starts_with('/') {
            url.set_path(&self.root_server_path);
        } else {
            url.set_path(&format!("/{}", self.root_server_path));
        }
        Some(url.to_string())
    }

    async fn write_message(&self, message: &Value) -> Result<()> {
        let body = serde_json::to_vec(message)?;
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
            .await?;
        stdin.write_all(&body).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("pending requests poisoned"))?
            .insert(id, tx);
        self.write_message(
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )
        .await?;
        let response = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => bail!("{} stopped before answering {method}", self.language.server()),
            Err(_) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                bail!("{method} timed out after {}s", timeout.as_secs())
            }
        };
        if let Some(error) = response.get("error") {
            bail!(
                "{method} failed: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            );
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn handle(&self, message: Value) {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            (None, Some(id)) => {
                if let Some(id) = id.as_i64() {
                    let sender = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
                    if let Some(sender) = sender {
                        let _ = sender.send(message);
                    }
                }
            }
            (Some(method), Some(id)) => {
                // Requests from the server must be answered or some servers
                // stall; Sinew has nothing to add beyond its settings.
                let result = if method == "workspace/configuration" {
                    let items = message
                        .get("params")
                        .and_then(|params| params.get("items"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    Value::Array(
                        items
                            .iter()
                            .map(|item| {
                                let section = item.get("section").and_then(Value::as_str);
                                if section == Some(self.language.settings_section()) {
                                    self.language.settings()
                                } else {
                                    Value::Null
                                }
                            })
                            .collect(),
                    )
                } else {
                    Value::Null
                };
                let _ = self
                    .write_message(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                    .await;
            }
            (Some("textDocument/publishDiagnostics"), None) => {
                let Some(params) = message.get("params") else {
                    return;
                };
                let Some(path) = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .and_then(|uri| self.paths.decode_uri(uri))
                else {
                    return;
                };
                let items = params
                    .get("diagnostics")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(parse_diagnostic).collect())
                    .unwrap_or_default();
                let seq = self.publish_seq.fetch_add(1, Ordering::SeqCst) + 1;
                let key = self.paths.key_for_server_path(&path);
                if let Ok(mut published) = self.published.lock() {
                    published.insert(
                        key,
                        Published {
                            seq,
                            version: params.get("version").and_then(Value::as_i64),
                            path,
                            items,
                        },
                    );
                }
                self.published_notify.notify_waiters();
            }
            (Some("$/progress"), None) => {
                let Some(params) = message.get("params") else {
                    return;
                };
                let token = match params.get("token") {
                    Some(Value::String(token)) => token.clone(),
                    Some(other) => other.to_string(),
                    None => return,
                };
                let kind = params
                    .get("value")
                    .and_then(|value| value.get("kind"))
                    .and_then(Value::as_str);
                if let Ok(mut progress) = self.progress.lock() {
                    match kind {
                        Some("begin") => {
                            progress.insert(token);
                        }
                        Some("end") => {
                            progress.remove(&token);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    async fn wait_ready(&self, deadline: Instant) -> std::result::Result<(), Outcome> {
        let mut rx = self.state.subscribe();
        loop {
            match rx.borrow_and_update().clone() {
                ServerState::Ready => return Ok(()),
                ServerState::Failed(reason) => return Err(Outcome::Failed(reason)),
                ServerState::Starting => {}
            }
            match tokio::time::timeout_at(deadline.into(), rx.changed()).await {
                Ok(Ok(())) => {}
                _ => return Err(Outcome::NotReady),
            }
        }
    }

    /// Send the file's current on-disk content to the server if it changed,
    /// then wait for fresh diagnostics until `deadline`.
    async fn check(&self, path: &Path, deadline: Instant) -> Outcome {
        if let Err(outcome) = self.wait_ready(deadline).await {
            return outcome;
        }
        let (Some(key), Some(uri)) = (self.paths.key(path), self.paths.uri(path)) else {
            return Outcome::Checked(Vec::new());
        };
        let text = match tokio::fs::metadata(path).await {
            Ok(meta) if meta.len() > MAX_FILE_BYTES => return Outcome::Checked(Vec::new()),
            Ok(_) => match tokio::fs::read(path).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => return Outcome::Checked(Vec::new()),
            },
            Err(_) => {
                self.forget(path).await;
                return Outcome::Checked(Vec::new());
            }
        };

        let seq_before = self.publish_seq.load(Ordering::SeqCst);
        let sent = {
            let mut docs = self.docs.lock().await;
            match docs.get_mut(&key) {
                None => {
                    let opened = self
                        .notify(
                            "textDocument/didOpen",
                            json!({ "textDocument": {
                                "uri": uri,
                                "languageId": self.language.language_id(path),
                                "version": 1,
                                "text": text,
                            }}),
                        )
                        .await;
                    if opened.is_err() {
                        return Outcome::NotReady;
                    }
                    docs.insert(
                        key.clone(),
                        OpenDoc {
                            version: 1,
                            text,
                            path: path.to_path_buf(),
                        },
                    );
                    Some(1)
                }
                Some(doc) if doc.text != text => {
                    doc.version += 1;
                    let changed = self
                        .notify(
                            "textDocument/didChange",
                            json!({
                                "textDocument": { "uri": uri, "version": doc.version },
                                "contentChanges": [{ "text": text }],
                            }),
                        )
                        .await;
                    if changed.is_err() {
                        return Outcome::NotReady;
                    }
                    doc.text = text;
                    Some(doc.version)
                }
                Some(_) => None,
            }
        };

        let Some(version) = sent else {
            // Unchanged since the last check: the latest report still holds.
            if let Some(items) = self.items_for(&key) {
                return Outcome::Checked(items);
            }
            return self.wait_fresh(&key, 0, None, deadline).await;
        };
        // The file is already saved on disk; rust-analyzer only runs
        // `cargo check` on save.
        let _ = self
            .notify(
                "textDocument/didSave",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await;
        self.wait_fresh(&key, seq_before, Some(version), deadline)
            .await
    }

    async fn wait_fresh(
        &self,
        key: &str,
        seq_before: u64,
        version: Option<i64>,
        deadline: Instant,
    ) -> Outcome {
        let mut last_seq = None;
        let mut settle_until: Option<Instant> = None;
        loop {
            let notified = self.published_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let ServerState::Failed(reason) = self.state() {
                return Outcome::Failed(reason);
            }
            if let Some(seq) = self.fresh_seq(key, seq_before, version) {
                if last_seq != Some(seq) {
                    last_seq = Some(seq);
                    settle_until = Some((Instant::now() + SETTLE).min(deadline));
                }
            }
            let until = settle_until.unwrap_or(deadline);
            if Instant::now() >= until {
                break;
            }
            let _ = tokio::time::timeout_at(until.into(), notified).await;
        }
        match last_seq.and_then(|_| self.items_for(key)) {
            Some(items) => Outcome::Checked(items),
            None => Outcome::NotReady,
        }
    }

    fn fresh_seq(&self, key: &str, seq_before: u64, version: Option<i64>) -> Option<u64> {
        let published = self.published.lock().ok()?;
        let entry = published.get(key)?;
        let version_matches = match (entry.version, version) {
            (Some(published), Some(sent)) => published >= sent,
            _ => true,
        };
        (entry.seq > seq_before && version_matches).then_some(entry.seq)
    }

    fn items_for(&self, key: &str) -> Option<Vec<Diagnostic>> {
        self.published
            .lock()
            .ok()?
            .get(key)
            .map(|entry| entry.items.clone())
    }

    /// Drop a deleted file from the server's view.
    async fn forget(&self, path: &Path) {
        let (Some(key), Some(uri)) = (self.paths.key(path), self.paths.uri(path)) else {
            return;
        };
        let was_open = self.docs.lock().await.remove(&key).is_some();
        if was_open {
            let _ = self
                .notify(
                    "textDocument/didClose",
                    json!({ "textDocument": { "uri": uri } }),
                )
                .await;
        }
        if let Ok(mut published) = self.published.lock() {
            published.remove(&key);
        }
    }

    /// Every non-empty report the server has published, as
    /// (workspace-relative path, diagnostics).
    fn all_published(&self) -> Vec<(String, Vec<Diagnostic>)> {
        let Ok(published) = self.published.lock() else {
            return Vec::new();
        };
        published
            .values()
            .filter(|entry| !entry.items.is_empty())
            .map(|entry| (self.relative(&entry.path), entry.items.clone()))
            .collect()
    }

    fn relative(&self, server_path: &str) -> String {
        let root = self.root_server_path.trim_end_matches('/');
        let windows = self.paths.host == Host::Native && cfg!(windows);
        let matches_root = if windows {
            server_path.len() > root.len()
                && server_path[..root.len()].eq_ignore_ascii_case(root)
        } else {
            server_path.starts_with(root)
        };
        if matches_root {
            if let Some(rest) = server_path[root.len()..].strip_prefix('/') {
                return rest.to_string();
            }
        }
        server_path.to_string()
    }

    async fn open_paths(&self) -> Vec<PathBuf> {
        self.docs
            .lock()
            .await
            .values()
            .map(|doc| doc.path.clone())
            .collect()
    }

    async fn shutdown(&self) {
        if self.state() == ServerState::Ready {
            let _ = self
                .request("shutdown", Value::Null, Duration::from_secs(2))
                .await;
            let _ = self.notify("exit", Value::Null).await;
        }
        let child = self.child.lock().ok().and_then(|mut child| child.take());
        if let Some(mut child) = child {
            let _ = child.start_kill();
        }
    }
}

async fn drain_stderr(
    stderr: impl AsyncRead + Unpin,
    tail: Arc<StdMutex<VecDeque<String>>>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(mut tail) = tail.lock() {
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
}

async fn read_loop(server: Weak<Server>, stdout: impl AsyncRead + Unpin) {
    let mut reader = BufReader::new(stdout);
    loop {
        let message = match read_message(&mut reader).await {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => break,
        };
        let Some(server) = server.upgrade() else {
            return;
        };
        server.handle(message).await;
    }
    // Let the stderr reader catch up so the failure carries the reason.
    tokio::time::sleep(Duration::from_millis(200)).await;
    if let Some(server) = server.upgrade() {
        server.fail(format!("{} exited", server.language.server()));
    }
}

/// One `Content-Length`-framed JSON-RPC message, or `None` at end of stream.
async fn read_message(reader: &mut (impl AsyncBufReadExt + Unpin)) -> Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length.unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).ok())
}

// ---------------------------------------------------------------------------
// The per-workspace registry.

#[derive(Default)]
struct Manager {
    servers: Mutex<HashMap<(String, Language), Arc<Server>>>,
}

fn manager() -> &'static Manager {
    static MANAGER: OnceLock<Manager> = OnceLock::new();
    MANAGER.get_or_init(Manager::default)
}

fn root_key(root: &Path) -> String {
    let paths = PathMap::for_root(root);
    paths
        .key(root)
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
        .trim_end_matches('/')
        .to_string()
}

/// The running server for this workspace and language, starting it if it is
/// installed. `None` when it is not installed or recently failed.
async fn server_for(
    root: &Path,
    language: Language,
    settings: &ToolSettings,
) -> Option<Arc<Server>> {
    let key = (root_key(root), language);
    let mut servers = manager().servers.lock().await;
    if let Some(server) = servers.get(&key) {
        match server.state() {
            ServerState::Failed(_) if server.retry_due() => {
                servers.remove(&key);
            }
            ServerState::Failed(_) => return None,
            _ => return Some(server.clone()),
        }
    }
    let paths = PathMap::for_root(root);
    let launch = resolve_launch(language, &paths, settings).await?;
    match Server::spawn(language, root, launch).await {
        Ok(server) => {
            servers.insert(key, server.clone());
            Some(server)
        }
        Err(err) => {
            tracing::warn!(server = language.server(), %err, "unable to start language server");
            None
        }
    }
}

async fn running_servers(root: &Path) -> Vec<Arc<Server>> {
    let key = root_key(root);
    manager()
        .servers
        .lock()
        .await
        .iter()
        .filter(|((root, _), _)| *root == key)
        .map(|(_, server)| server.clone())
        .collect()
}

/// Stop every language server of a workspace; they restart on the next edit.
pub async fn restart_workspace(root: &Path) {
    let key = root_key(root);
    let stopped: Vec<Arc<Server>> = {
        let mut servers = manager().servers.lock().await;
        let keys: Vec<_> = servers
            .keys()
            .filter(|(root, _)| *root == key)
            .cloned()
            .collect();
        keys.iter().filter_map(|key| servers.remove(key)).collect()
    };
    for server in stopped {
        server.shutdown().await;
    }
    if let Ok(mut cache) = wsl_lookup_cache().lock() {
        cache.clear();
    }
}

// ---------------------------------------------------------------------------
// Checking files.

struct FileReport {
    relative: String,
    language: Language,
    outcome: FileOutcome,
}

enum FileOutcome {
    Checked(Vec<Diagnostic>),
    NotReady { indexing: bool },
    Failed(String),
    NotInstalled,
}

async fn check_files(
    root: &Path,
    settings: &ToolSettings,
    files: Vec<(String, PathBuf)>,
    deadline: Instant,
) -> Vec<FileReport> {
    let checks = files.into_iter().filter_map(|(relative, absolute)| {
        let language = Language::for_path(&absolute)?;
        Some(async move {
            let outcome = match server_for(root, language, settings).await {
                None => FileOutcome::NotInstalled,
                Some(server) => match server.check(&absolute, deadline).await {
                    Outcome::Checked(items) => FileOutcome::Checked(items),
                    Outcome::NotReady => FileOutcome::NotReady {
                        indexing: server.indexing() || server.state() == ServerState::Starting,
                    },
                    Outcome::Failed(reason) => FileOutcome::Failed(reason),
                },
            };
            FileReport {
                relative,
                language,
                outcome,
            }
        })
    });
    futures_util::future::join_all(checks).await
}

/// Append the language servers' errors for the files an edit just changed to
/// its result, so the agent sees them right away.
pub async fn attach_after_edit(root: &Path, settings: &ToolSettings, result: &mut ToolRunResult) {
    if !settings.lsp_enabled || result.is_error {
        return;
    }
    let mut files = Vec::new();
    for change in &result.file_changes {
        if change.binary || Language::for_path(Path::new(&change.relative_path)).is_none() {
            continue;
        }
        let absolute = root.join(&change.relative_path);
        if matches!(change.kind, FileChangeKind::Deleted) {
            for server in running_servers(root).await {
                server.forget(&absolute).await;
            }
            continue;
        }
        files.push((change.relative_path.clone(), absolute));
    }
    if files.is_empty() {
        return;
    }
    let reports = check_files(
        root,
        settings,
        files,
        Instant::now() + AFTER_EDIT_BUDGET,
    )
    .await;

    let mut errors = Vec::new();
    let mut waiting = Vec::new();
    for report in &reports {
        match &report.outcome {
            FileOutcome::Checked(items) => errors.extend(
                items
                    .iter()
                    .filter(|item| item.severity == Severity::Error)
                    .map(|item| (report.relative.clone(), item.clone())),
            ),
            FileOutcome::NotReady { .. } => waiting.push(report),
            FileOutcome::Failed(_) | FileOutcome::NotInstalled => {}
        }
    }
    let checked = reports
        .iter()
        .any(|report| matches!(report.outcome, FileOutcome::Checked(_)));
    if !checked && waiting.is_empty() {
        return;
    }

    let mut text = String::new();
    if !errors.is_empty() {
        text.push_str(&format!(
            "\n\n<diagnostics>\nThe language server reports {} after this change:\n",
            plural(errors.len(), "error")
        ));
        for (relative, item) in errors.iter().take(MAX_AFTER_EDIT_LISTED) {
            text.push_str(&format_line(relative, item));
            text.push('\n');
        }
        if errors.len() > MAX_AFTER_EDIT_LISTED {
            text.push_str(&format!(
                "… and {} more; call `{}` to see them all.\n",
                errors.len() - MAX_AFTER_EDIT_LISTED,
                tool_names::DIAGNOSTICS
            ));
        }
        text.push_str("</diagnostics>");
    }
    if !waiting.is_empty() {
        let servers: Vec<&str> = waiting
            .iter()
            .map(|report| report.language.server())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        text.push_str(&format!(
            "\n\n(Diagnostics not ready yet: {} is still starting or indexing. Call `{}` later to check {}.)",
            servers.join(", "),
            tool_names::DIAGNOSTICS,
            waiting
                .iter()
                .map(|report| report.relative.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    result.content.push_str(&text);

    let summary = json!({
        "errors": errors.len(),
        "pending": !waiting.is_empty(),
        "items": errors
            .iter()
            .take(MAX_AFTER_EDIT_LISTED)
            .map(|(relative, item)| item_json(relative, item))
            .collect::<Vec<_>>(),
    });
    match result.meta.as_mut().and_then(Value::as_object_mut) {
        Some(meta) => {
            meta.insert("diagnostics".into(), summary);
        }
        None => result.meta = Some(json!({ "diagnostics": summary })),
    }
}

fn item_json(relative: &str, item: &Diagnostic) -> Value {
    let mut value = serde_json::to_value(item).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("path".into(), json!(relative));
    }
    value
}

fn format_line(relative: &str, item: &Diagnostic) -> String {
    let code = item
        .code
        .as_deref()
        .map(|code| format!("[{code}]"))
        .unwrap_or_default();
    let source = item
        .source
        .as_deref()
        .map(|source| format!(" ({source})"))
        .unwrap_or_default();
    format!(
        "{relative}:{}:{}: {}{code}: {}{source}",
        item.line,
        item.column,
        item.severity.label(),
        item.message
    )
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

// ---------------------------------------------------------------------------
// The `diagnostics` tool.

pub fn descriptor() -> ToolDescriptor {
    ToolDescriptor {
        name: tool_names::DIAGNOSTICS.into(),
        description: "Report compiler and type-checker errors and warnings from the project's language servers (rust-analyzer, TypeScript, Pyright, gopls). Pass `paths` to re-check specific files against their current content on disk; omit it to list everything the servers currently report for the workspace. Errors in edited files are already appended to edit_file/write_file results; use this to check files affected indirectly, or before declaring a code change done.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files to check, relative to the workspace root (absolute paths inside the workspace are allowed). Omit to list every diagnostic the running servers report."
                },
                "include_warnings": {
                    "type": "boolean",
                    "description": "Include warnings as well as errors. Defaults to true."
                }
            },
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Default, Deserialize)]
struct DiagnosticsInput {
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    include_warnings: Option<bool>,
}

pub async fn run_tool(root: &Path, settings: &ToolSettings, input: Value) -> ToolRunResult {
    if !settings.lsp_enabled {
        return ToolRunResult::err(
            "language-server diagnostics are disabled in Settings",
            Vec::new(),
        );
    }
    let parsed: DiagnosticsInput = match serde_json::from_value(input) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ToolRunResult::err(format!("invalid diagnostics input: {err}"), Vec::new())
        }
    };
    let include_warnings = parsed.include_warnings.unwrap_or(true);
    let deadline = Instant::now() + TOOL_BUDGET;

    let mut notes = Vec::new();
    let mut by_file: BTreeMap<String, Vec<Diagnostic>> = BTreeMap::new();
    let mut checked_files = 0usize;

    if parsed.paths.is_empty() {
        let servers = running_servers(root).await;
        if servers.is_empty() {
            return ToolRunResult::ok(
                "No language server is running for this workspace yet. They start when a supported file (Rust, TypeScript/JavaScript, Python, Go) is edited; pass `paths` to check specific files now.",
                Vec::new(),
            );
        }
        // Re-sync the files the agent touched: the shell may have changed
        // them since.
        let mut open = Vec::new();
        for server in &servers {
            for absolute in server.open_paths().await {
                let relative = absolute
                    .strip_prefix(root)
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| absolute.to_string_lossy().into_owned());
                open.push((relative, absolute));
            }
        }
        let reports = check_files(root, settings, open, deadline).await;
        collect_notes(&reports, &mut notes);
        for server in &servers {
            if server.indexing() {
                notes.push(format!(
                    "{} is still indexing; results may be incomplete.",
                    server.language.server()
                ));
            }
            for (relative, items) in server.all_published() {
                by_file.entry(relative).or_default().extend(items);
            }
        }
        checked_files = by_file.len();
    } else {
        let mut files = Vec::new();
        for raw in &parsed.paths {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            let absolute = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                root.join(raw)
            };
            if Language::for_path(&absolute).is_none() {
                notes.push(format!("{raw}: no supported language server for this file type."));
                continue;
            }
            if !absolute.is_file() {
                notes.push(format!("{raw}: file not found."));
                continue;
            }
            let relative = absolute
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| raw.to_string());
            files.push((relative, absolute));
        }
        let reports = check_files(root, settings, files, deadline).await;
        collect_notes(&reports, &mut notes);
        for report in reports {
            if let FileOutcome::Checked(items) = report.outcome {
                checked_files += 1;
                by_file.insert(report.relative, items);
            }
        }
    }

    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut lines = Vec::new();
    for (relative, items) in &mut by_file {
        items.sort_by_key(|item| (item.severity, item.line, item.column));
        for item in items.iter() {
            match item.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                _ => continue,
            }
            if item.severity == Severity::Warning && !include_warnings {
                continue;
            }
            lines.push(format_line(relative, item));
        }
    }

    let mut output = String::new();
    if parsed.paths.is_empty() {
        output.push_str(&format!(
            "{}, {} reported across the workspace.",
            plural(errors, "error"),
            plural(warnings, "warning")
        ));
    } else {
        output.push_str(&format!(
            "{} checked: {}, {}.",
            plural(checked_files, "file"),
            plural(errors, "error"),
            plural(warnings, "warning")
        ));
    }
    if !lines.is_empty() {
        output.push('\n');
        for line in lines.iter().take(MAX_TOOL_LISTED) {
            output.push('\n');
            output.push_str(line);
        }
        if lines.len() > MAX_TOOL_LISTED {
            output.push_str(&format!(
                "\n… and {} more.",
                lines.len() - MAX_TOOL_LISTED
            ));
        }
    }
    if !notes.is_empty() {
        output.push_str("\n\n");
        output.push_str(&notes.join("\n"));
    }
    let items: Vec<Value> = by_file
        .iter()
        .flat_map(|(relative, items)| {
            items
                .iter()
                .filter(|item| {
                    item.severity == Severity::Error
                        || (include_warnings && item.severity == Severity::Warning)
                })
                .map(move |item| item_json(relative, item))
        })
        .take(MAX_TOOL_LISTED)
        .collect();
    ToolRunResult::ok_with_meta(
        output,
        Vec::new(),
        json!({ "diagnostics": { "errors": errors, "warnings": warnings, "items": items } }),
    )
}

fn collect_notes(reports: &[FileReport], notes: &mut Vec<String>) {
    for report in reports {
        match &report.outcome {
            FileOutcome::Checked(_) => {}
            FileOutcome::NotReady { indexing } => notes.push(format!(
                "{}: {} did not answer in time{}.",
                report.relative,
                report.language.server(),
                if *indexing { " (still starting or indexing)" } else { "" }
            )),
            FileOutcome::Failed(reason) => notes.push(format!(
                "{}: {} failed: {}",
                report.relative,
                report.language.server(),
                reason.lines().next().unwrap_or("unknown error")
            )),
            FileOutcome::NotInstalled => notes.push(format!(
                "{}: {} is not installed (install it with `{}`).",
                report.relative,
                report.language.server(),
                report.language.install_hint()
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Status for Settings.

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspServerStatus {
    pub language: &'static str,
    pub label: &'static str,
    pub server: &'static str,
    /// Where the binary was found, if it was.
    pub command: Option<String>,
    /// The workspace has this language's project files.
    pub detected: bool,
    /// `not_installed`, `idle`, `starting`, `indexing`, `ready` or `failed`.
    pub state: &'static str,
    pub detail: Option<String>,
    pub install_hint: &'static str,
    pub wsl: bool,
}

pub async fn status(root: &Path, settings: &ToolSettings) -> Vec<LspServerStatus> {
    let paths = PathMap::for_root(root);
    let running = running_servers(root).await;
    let mut statuses = Vec::new();
    for language in LANGUAGES {
        let server = running.iter().find(|server| server.language == language);
        let command = match server {
            Some(server) => Some(server.program.clone()),
            None => resolve_launch(language, &paths, settings)
                .await
                .map(|launch| launch.program),
        };
        let (state, detail) = match (server, &command) {
            (Some(server), _) => match server.state() {
                ServerState::Starting => ("starting", None),
                ServerState::Ready if server.indexing() => ("indexing", None),
                ServerState::Ready => ("ready", None),
                // rustup installs a `rust-analyzer` proxy even when the
                // component itself is missing.
                ServerState::Failed(reason) if reason.contains("Unknown binary") => {
                    ("not_installed", Some(reason))
                }
                ServerState::Failed(reason) => ("failed", Some(reason)),
            },
            (None, Some(_)) => ("idle", None),
            (None, None) => ("not_installed", None),
        };
        statuses.push(LspServerStatus {
            language: language.id(),
            label: language.label(),
            server: language.server(),
            command,
            detected: language.detected_in(root),
            state,
            detail,
            install_hint: language.install_hint(),
            wsl: paths.is_wsl(),
        });
    }
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_follow_extensions() {
        assert_eq!(Language::for_path(Path::new("src/main.rs")), Some(Language::Rust));
        assert_eq!(
            Language::for_path(Path::new("App.TSX")),
            Some(Language::TypeScript)
        );
        assert_eq!(Language::for_path(Path::new("a.py")), Some(Language::Python));
        assert_eq!(Language::for_path(Path::new("x.go")), Some(Language::Go));
        assert_eq!(Language::for_path(Path::new("README.md")), None);
        assert_eq!(
            Language::TypeScript.language_id(Path::new("a.jsx")),
            "javascriptreact"
        );
    }

    #[test]
    fn wsl_unc_paths_map_to_linux_paths() {
        assert_eq!(
            split_wsl_unc(r"\\wsl.localhost\Ubuntu\home\me\proj"),
            Some(("Ubuntu".into(), "/home/me/proj".into()))
        );
        assert_eq!(
            split_wsl_unc(r"\\?\UNC\wsl$\Debian\srv\app\src\main.rs"),
            Some(("Debian".into(), "/srv/app/src/main.rs".into()))
        );
        assert_eq!(split_wsl_unc(r"\\server\share\x"), None);
        assert_eq!(split_wsl_unc(r"C:\proj"), None);
    }

    #[test]
    fn uris_round_trip_to_the_same_key() {
        let paths = PathMap { host: Host::Native };
        let file = if cfg!(windows) {
            PathBuf::from(r"C:\My Projects\app\src\main.rs")
        } else {
            PathBuf::from("/home/me/My Projects/app/src/main.rs")
        };
        let uri = paths.uri(&file).unwrap();
        assert!(uri.starts_with("file:///"));
        assert!(uri.contains("My%20Projects"));
        let decoded = paths.decode_uri(&uri).unwrap();
        assert_eq!(paths.key_for_server_path(&decoded), paths.key(&file).unwrap());
        if cfg!(windows) {
            // typescript-language-server lowercases and escapes the drive.
            let decoded = paths
                .decode_uri("file:///c%3A/My%20Projects/app/src/main.rs")
                .unwrap();
            assert_eq!(paths.key_for_server_path(&decoded), paths.key(&file).unwrap());
        }

        let wsl = PathMap {
            host: Host::Wsl {
                distro: "Ubuntu".into(),
            },
        };
        let file = PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\app\src\lib.rs");
        assert_eq!(
            wsl.uri(&file).as_deref(),
            Some("file:///home/me/app/src/lib.rs")
        );
    }

    #[test]
    fn diagnostics_parse_and_format() {
        let item = parse_diagnostic(&json!({
            "range": { "start": { "line": 11, "character": 4 }, "end": { "line": 11, "character": 9 } },
            "severity": 1,
            "code": "E0308",
            "source": "rustc",
            "message": "mismatched types\nexpected `u32`, found `&str`"
        }))
        .unwrap();
        assert_eq!(item.severity, Severity::Error);
        assert_eq!(
            format_line("src/main.rs", &item),
            "src/main.rs:12:5: error[E0308]: mismatched types — expected `u32`, found `&str` (rustc)"
        );
        let warning = parse_diagnostic(&json!({
            "range": { "start": { "line": 0, "character": 0 } },
            "severity": 2,
            "code": 6133,
            "message": "'x' is declared but its value is never read."
        }))
        .unwrap();
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(warning.code.as_deref(), Some("6133"));
    }

    #[test]
    fn percent_decoding_handles_escapes_and_plain_text() {
        assert_eq!(percent_decode("/c%3A/a%20b/%E2%9C%93"), "/c:/a b/✓");
        assert_eq!(percent_decode("/plain/100%"), "/plain/100%");
    }

    /// A tiny language server: flags every `BROKEN` as an error, always adds
    /// one warning, and publishes a stale report before each fresh one.
    const FAKE_SERVER: &str = r#"
let buf = Buffer.alloc(0);
const send = (m) => {
  const b = Buffer.from(JSON.stringify(m));
  process.stdout.write(`Content-Length: ${b.length}\r\n\r\n`);
  process.stdout.write(b);
};
const publish = (uri, version, text) => {
  const diagnostics = [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, severity: 2, message: 'just a warning' }];
  text.split('\n').forEach((line, i) => {
    const c = line.indexOf('BROKEN');
    if (c >= 0) diagnostics.push({ range: { start: { line: i, character: c }, end: { line: i, character: c + 6 } }, severity: 1, code: 'E1', source: 'fake', message: 'broken here' });
  });
  send({ jsonrpc: '2.0', method: 'textDocument/publishDiagnostics', params: { uri, version, diagnostics } });
};
const handle = (msg) => {
  if (msg.method === 'initialize') {
    send({ jsonrpc: '2.0', id: 900, method: 'workspace/configuration', params: { items: [{ section: 'gopls' }] } });
    send({ jsonrpc: '2.0', method: '$/progress', params: { token: 'index', value: { kind: 'begin' } } });
    send({ jsonrpc: '2.0', method: '$/progress', params: { token: 'index', value: { kind: 'end' } } });
    send({ jsonrpc: '2.0', id: msg.id, result: { capabilities: { textDocumentSync: 1 } } });
  } else if (msg.method === 'textDocument/didOpen') {
    const d = msg.params.textDocument;
    setTimeout(() => publish(d.uri, d.version, d.text), 50);
  } else if (msg.method === 'textDocument/didChange') {
    const d = msg.params.textDocument;
    publish(d.uri, d.version - 1, 'stale BROKEN');
    setTimeout(() => publish(d.uri, d.version, msg.params.contentChanges[0].text), 50);
  } else if (msg.method === 'shutdown') {
    send({ jsonrpc: '2.0', id: msg.id, result: null });
  } else if (msg.method === 'exit') {
    process.exit(0);
  }
};
process.stdin.on('data', (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  for (;;) {
    const sep = buf.indexOf('\r\n\r\n');
    if (sep < 0) return;
    const len = Number(/Content-Length: (\d+)/i.exec(buf.subarray(0, sep).toString())[1]);
    if (buf.length < sep + 4 + len) return;
    const msg = JSON.parse(buf.subarray(sep + 4, sep + 4 + len).toString());
    buf = buf.subarray(sep + 4 + len);
    handle(msg);
  }
});
"#;

    fn changed(relative: &str) -> crate::tool_run::FileChange {
        crate::tool_run::FileChange {
            relative_path: relative.into(),
            kind: FileChangeKind::Modified,
            summary: String::new(),
            binary: false,
            added_lines: 1,
            removed_lines: 0,
            truncated: false,
            lines: Vec::new(),
        }
    }

    #[tokio::test]
    #[ignore = "needs node on PATH"]
    async fn edits_get_the_servers_errors() {
        let root = std::env::temp_dir().join(format!("sinew-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("fake-lsp.js"), FAKE_SERVER).unwrap();
        let launcher = if cfg!(windows) {
            let path = root.join("fake-lsp.cmd");
            std::fs::write(&path, "@node \"%~dp0fake-lsp.js\" %*\r\n").unwrap();
            path
        } else {
            let path = root.join("fake-lsp");
            std::fs::write(
                &path,
                "#!/bin/sh\nexec node \"$(dirname \"$0\")/fake-lsp.js\" \"$@\"\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            path
        };
        let mut settings = ToolSettings::default();
        settings
            .lsp_server_paths
            .insert("go".into(), launcher.to_string_lossy().into_owned());

        let file = root.join("src").join("main.go");
        std::fs::write(&file, "package main\nfunc main() { BROKEN }\n").unwrap();
        let mut result = ToolRunResult::ok("Wrote src/main.go", vec![changed("src/main.go")]);
        attach_after_edit(&root, &settings, &mut result).await;
        assert!(
            result
                .content
                .contains("src/main.go:2:15: error[E1]: broken here (fake)"),
            "{}",
            result.content
        );
        assert!(!result.content.contains("warning"), "{}", result.content);
        assert_eq!(result.meta.as_ref().unwrap()["diagnostics"]["errors"], 1);

        // Fixed: the stale report published first must not count.
        std::fs::write(&file, "package main\nfunc main() {}\n").unwrap();
        let mut result = ToolRunResult::ok("Edited src/main.go", vec![changed("src/main.go")]);
        attach_after_edit(&root, &settings, &mut result).await;
        assert!(!result.content.contains("<diagnostics>"), "{}", result.content);
        assert_eq!(result.meta.as_ref().unwrap()["diagnostics"]["errors"], 0);

        let all = run_tool(&root, &settings, json!({})).await;
        assert!(
            all.content.starts_with("0 errors, 1 warning reported"),
            "{}",
            all.content
        );
        let errors_only = run_tool(
            &root,
            &settings,
            json!({ "paths": ["src/main.go"], "include_warnings": false }),
        )
        .await;
        assert!(
            errors_only.content.starts_with("1 file checked: 0 errors, 1 warning."),
            "{}",
            errors_only.content
        );
        assert!(!errors_only.content.contains("just a warning"));

        let go = status(&root, &settings)
            .await
            .into_iter()
            .find(|status| status.language == "go")
            .unwrap();
        assert_eq!(go.state, "ready");
        restart_workspace(&root).await;
        let go = status(&root, &settings)
            .await
            .into_iter()
            .find(|status| status.language == "go")
            .unwrap();
        assert_eq!(go.state, "idle");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn framed_messages_are_read_one_by_one() {
        let first = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let second = r#"{"jsonrpc":"2.0","method":"x"}"#;
        let stream = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{first}Content-Length: {}\r\n\r\n{second}",
            first.len(),
            second.len()
        );
        let mut reader = BufReader::new(stream.as_bytes());
        let one = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(one["id"], 1);
        let two = read_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(two["method"], "x");
        assert!(read_message(&mut reader).await.unwrap().is_none());
    }
}
