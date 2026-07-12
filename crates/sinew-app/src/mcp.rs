use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    env,
    ffi::OsStr,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use eventsource_stream::Eventsource;
use futures_util::{stream::BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sinew_core::{ChatMessage, Part, ToolDescriptor};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::RwLock,
    time::timeout,
};
use tracing::warn;

use crate::tool_names;
use crate::tool_run::{ToolRunImage, ToolRunResult};

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const TOOL_OUTPUT_LIMIT: usize = 128 * 1024;
const TOOL_NAME_LIMIT: usize = 64;
const LOAD_MCP_TOOL_NAME: &str = tool_names::LOAD_MCP_TOOL;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpTransport {
    /// Local process speaking JSON-RPC over stdin/stdout.
    #[default]
    Stdio,
    /// Streamable HTTP (single URL, JSON responses and/or SSE response bodies).
    Http,
    /// Legacy HTTP+SSE MCP transport: GET event stream + POST message endpoint.
    Sse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<McpEnvVar>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// MCP transport. Defaults to `stdio` for command configs and is inferred
    /// as `http`/`sse` when `url` is present in imported legacy configs.
    #[serde(default)]
    pub transport: McpTransport,
    /// Remote MCP server URL. For `http`, this is the streamable HTTP endpoint
    /// (for example https://mcp.figma.com/mcp). For `sse`, this is the SSE
    /// endpoint (commonly /sse); Sinew follows endpoint events when provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Remote transports: extra request headers (e.g. Authorization: Bearer …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<McpEnvVar>,
    /// Optional OAuth hints for remote MCP servers. Most servers work with
    /// discovery + dynamic registration; some only allow pre-registered
    /// clients, so users can provide these fields in the MCP JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<McpOAuthClientConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthClientConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEnvVar {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub server_id: String,
    pub server_name: String,
    pub name: String,
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerProbe {
    pub server_id: String,
    pub server_name: String,
    pub enabled: bool,
    pub ok: bool,
    pub tools: Vec<McpToolInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct McpToolBinding {
    server: McpServerConfig,
    original_name: String,
    display_name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolLabel {
    pub server_name: String,
    pub tool_name: String,
}

#[derive(Debug)]
pub struct McpToolRegistry {
    settings: McpSettings,
    bindings: RwLock<HashMap<String, McpToolBinding>>,
    loaded: RwLock<HashSet<String>>,
}

impl McpToolRegistry {
    pub fn new(settings: McpSettings) -> Self {
        Self {
            settings,
            bindings: RwLock::new(HashMap::new()),
            loaded: RwLock::new(HashSet::new()),
        }
    }

    pub async fn refresh_catalog(&self, history: &[ChatMessage]) -> Vec<ToolDescriptor> {
        let mut next_bindings = HashMap::new();

        for server in enabled_servers(&self.settings) {
            let mut client = match McpClient::connect(server).await {
                Ok(client) => client,
                Err(err) => {
                    warn!("unable to connect MCP server {}: {err}", server.name);
                    continue;
                }
            };

            let tools = match client.list_tools().await {
                Ok(tools) => tools,
                Err(err) => {
                    warn!("unable to list MCP tools for {}: {err}", server.name);
                    continue;
                }
            };

            for tool in tools {
                let generated_name = unique_tool_name(server, &tool.name, &next_bindings);
                let display_name = mcp_tool_display_name(&tool);
                let description = mcp_tool_description(server, &tool);
                next_bindings.insert(
                    generated_name,
                    McpToolBinding {
                        server: server.clone(),
                        original_name: tool.name,
                        display_name,
                        description,
                        input_schema: normalize_input_schema(tool.input_schema),
                    },
                );
            }
        }

        let history_requests = history_loaded_mcp_tools(history);
        let mut loaded = self.loaded.read().await.clone();
        for request in history_requests {
            if let Ok(name) = resolve_mcp_tool(&next_bindings, &request) {
                loaded.insert(name);
            }
        }
        loaded.retain(|name| next_bindings.contains_key(name));

        *self.bindings.write().await = next_bindings;
        *self.loaded.write().await = loaded;
        self.descriptors().await
    }

    pub async fn descriptors(&self) -> Vec<ToolDescriptor> {
        let bindings = self.bindings.read().await;
        if bindings.is_empty() {
            return Vec::new();
        }

        let loaded = self.loaded.read().await;
        let mut descriptors = vec![load_mcp_tool_descriptor(&bindings)];
        let mut names = bindings.keys().cloned().collect::<Vec<_>>();
        names.sort();

        for name in names {
            if !loaded.contains(&name) {
                continue;
            }
            if let Some(binding) = bindings.get(&name) {
                descriptors.push(ToolDescriptor {
                    name,
                    description: binding.description.clone(),
                    input_schema: binding.input_schema.clone(),
                });
            }
        }

        descriptors
    }

    pub async fn run_tool(&self, name: &str, input: Value) -> Option<ToolRunResult> {
        if tool_names::is_tool_name(name, LOAD_MCP_TOOL_NAME) {
            return Some(self.load_tool(input).await);
        }

        let binding = self.bindings.read().await.get(name).cloned()?;
        if !self.loaded.read().await.contains(name) {
            return Some(ToolRunResult::err(
                format!("MCP tool `{name}` is not loaded yet. Use {LOAD_MCP_TOOL_NAME} first."),
                Vec::new(),
            ));
        }
        Some(call_mcp_tool(binding, input).await)
    }

    pub async fn tool_label(&self, name: &str) -> Option<McpToolLabel> {
        let binding = self.bindings.read().await.get(name).cloned()?;
        Some(McpToolLabel {
            server_name: binding.server.name,
            tool_name: binding.original_name,
        })
    }

    async fn load_tool(&self, input: Value) -> ToolRunResult {
        let request = match mcp_tool_request_from_input(&input) {
            Ok(request) => request,
            Err(err) => return ToolRunResult::err(err.to_string(), Vec::new()),
        };

        let bindings = self.bindings.read().await;
        let name = match resolve_mcp_tool(&bindings, &request) {
            Ok(name) => name,
            Err(err) => return ToolRunResult::err(err.to_string(), Vec::new()),
        };
        let Some(binding) = bindings.get(&name).cloned() else {
            return ToolRunResult::err(format!("MCP tool `{name}` is unavailable"), Vec::new());
        };
        drop(bindings);

        self.loaded.write().await.insert(name.clone());
        ToolRunResult::ok(
            format!(
                "Loaded {} / {}.\nTool name: `{}`\nUse this tool on the next step; its full description and input schema are now available.",
                display_mcp_server_name(&binding.server.name),
                binding.original_name,
                name
            ),
            Vec::new(),
        )
    }
}

#[derive(Debug, Clone)]
struct McpToolRequest {
    generated_name: Option<String>,
    server: Option<String>,
    tool: Option<String>,
}

fn load_mcp_tool_descriptor(bindings: &HashMap<String, McpToolBinding>) -> ToolDescriptor {
    let mut entries = bindings
        .values()
        .map(|binding| {
            format!(
                "- {} / {}",
                display_mcp_server_name(&binding.server.name),
                binding.original_name
            )
        })
        .collect::<Vec<_>>();
    entries.sort();

    ToolDescriptor {
        name: LOAD_MCP_TOOL_NAME.to_string(),
        description: format!(
            "Load one MCP tool before calling it. Available MCP tools:\n{}\nCall with the exact `server` and `tool` strings shown around `/`. Tools not loaded yet do not expose their full description or input schema.",
            entries.join("\n")
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server name as shown before `/` in the catalog."
                },
                "tool": {
                    "type": "string",
                    "description": "MCP tool name as shown after `/` in the catalog."
                },
                "name": {
                    "type": "string",
                    "description": "Optional generated tool name if a previous load result provided one."
                }
            },
            "required": ["server", "tool"],
            "additionalProperties": false
        }),
    }
}

fn history_loaded_mcp_tools(history: &[ChatMessage]) -> Vec<McpToolRequest> {
    let mut requests = Vec::new();
    for message in history {
        for part in &message.parts {
            let Part::ToolCall { name, input, .. } = part else {
                continue;
            };

            if tool_names::is_tool_name(name, LOAD_MCP_TOOL_NAME) {
                if let Ok(request) = mcp_tool_request_from_input(input) {
                    requests.push(request);
                }
            } else if is_mcp_generated_name(name) {
                requests.push(McpToolRequest {
                    generated_name: Some(name.clone()),
                    server: None,
                    tool: None,
                });
            }
        }
    }
    requests
}

fn mcp_tool_request_from_input(input: &Value) -> Result<McpToolRequest> {
    let generated_name = input_string(input, &["name", "toolName", "tool_name"])
        .filter(|value| is_mcp_generated_name(value));
    if generated_name.is_some() {
        return Ok(McpToolRequest {
            generated_name,
            server: input_string(input, &["server", "serverName", "server_name"]),
            tool: input_string(input, &["tool", "toolName", "tool_name"]),
        });
    }

    let server = input_string(input, &["server", "serverName", "server_name", "mcp"]);
    let tool = input_string(input, &["tool", "toolName", "tool_name", "name"]);
    if server.is_none() || tool.is_none() {
        bail!("load_mcp_tool needs `server` and `tool` from the MCP catalog");
    }

    Ok(McpToolRequest {
        generated_name: None,
        server,
        tool,
    })
}

fn input_string(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_mcp_tool(
    bindings: &HashMap<String, McpToolBinding>,
    request: &McpToolRequest,
) -> Result<String> {
    if let Some(name) = request.generated_name.as_deref() {
        if bindings.contains_key(name) {
            return Ok(name.to_string());
        }
        bail!("MCP tool `{name}` is unavailable");
    }

    let server = request
        .server
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("load_mcp_tool needs `server` from the MCP catalog"))?;
    let tool = request
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("load_mcp_tool needs `tool` from the MCP catalog"))?;

    let matches = bindings
        .iter()
        .filter(|(_, binding)| {
            mcp_server_matches(binding, server) && mcp_tool_matches(binding, tool)
        })
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [name] => Ok(name.clone()),
        [] => bail!("No MCP tool found for `{server} / {tool}`"),
        _ => bail!("Several MCP tools match `{server} / {tool}`"),
    }
}

fn mcp_server_matches(binding: &McpToolBinding, value: &str) -> bool {
    loose_label_eq(&binding.server.name, value)
        || loose_label_eq(&display_mcp_server_name(&binding.server.name), value)
        || loose_label_eq(&binding.server.id, value)
}

fn mcp_tool_matches(binding: &McpToolBinding, value: &str) -> bool {
    loose_label_eq(&binding.original_name, value) || loose_label_eq(&binding.display_name, value)
}

fn loose_label_eq(left: &str, right: &str) -> bool {
    compact_label(left) == compact_label(right)
}

fn compact_label(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn is_mcp_generated_name(name: &str) -> bool {
    name.starts_with("mcp__")
}

pub async fn probe_mcp_servers(settings: &McpSettings) -> Vec<McpServerProbe> {
    let mut probes = Vec::new();
    let mut known_names: HashMap<String, McpToolBinding> = HashMap::new();

    for server in &settings.servers {
        if !server.enabled {
            probes.push(McpServerProbe {
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                enabled: false,
                ok: true,
                tools: Vec::new(),
                error: None,
            });
            continue;
        }

        let mut client = match McpClient::connect(server).await {
            Ok(client) => client,
            Err(err) => {
                probes.push(McpServerProbe {
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    enabled: true,
                    ok: false,
                    tools: Vec::new(),
                    error: Some(err.to_string()),
                });
                continue;
            }
        };

        match client.list_tools().await {
            Ok(tools) => {
                let mut infos = Vec::with_capacity(tools.len());
                for tool in tools {
                    let tool_name = unique_tool_name(server, &tool.name, &known_names);
                    let display_name = mcp_tool_display_name(&tool);
                    known_names.insert(
                        tool_name.clone(),
                        McpToolBinding {
                            server: server.clone(),
                            original_name: tool.name.clone(),
                            display_name,
                            description: mcp_tool_description(server, &tool),
                            input_schema: normalize_input_schema(tool.input_schema.clone()),
                        },
                    );
                    infos.push(McpToolInfo {
                        server_id: server.id.clone(),
                        server_name: server.name.clone(),
                        name: tool.name,
                        tool_name,
                        title: tool.title,
                        description: tool.description,
                    });
                }
                probes.push(McpServerProbe {
                    server_id: server.id.clone(),
                    server_name: server.name.clone(),
                    enabled: true,
                    ok: true,
                    tools: infos,
                    error: None,
                });
            }
            Err(err) => probes.push(McpServerProbe {
                server_id: server.id.clone(),
                server_name: server.name.clone(),
                enabled: true,
                ok: false,
                tools: Vec::new(),
                error: Some(err.to_string()),
            }),
        }
    }

    probes
}

fn enabled_servers(settings: &McpSettings) -> impl Iterator<Item = &McpServerConfig> {
    settings.servers.iter().filter(|server| {
        server.enabled && (is_remote_server(server) || !server.command.trim().is_empty())
    })
}

async fn call_mcp_tool(binding: McpToolBinding, input: Value) -> ToolRunResult {
    match call_mcp_tool_inner(binding, input).await {
        Ok(result) => result,
        Err(err) => ToolRunResult::err(format!("MCP tool failed: {err}"), Vec::new()),
    }
}

async fn call_mcp_tool_inner(binding: McpToolBinding, input: Value) -> Result<ToolRunResult> {
    let mut client = McpClient::connect_with_timeout(&binding.server, CALL_TIMEOUT).await?;
    let result = client.call_tool(&binding.original_name, input).await?;
    Ok(format_call_result(result))
}

fn format_call_result(result: McpCallToolResult) -> ToolRunResult {
    let mut text = Vec::new();
    let mut images = Vec::new();

    for block in result.content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push(value.to_string());
                }
            }
            Some("image") => {
                let data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let media_type = block
                    .get("mimeType")
                    .or_else(|| block.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("image/png")
                    .to_string();
                if !data.is_empty() {
                    images.push(ToolRunImage {
                        media_type: media_type.clone(),
                        data,
                        path: None,
                    });
                }
                text.push(format!("[image: {media_type}]"));
            }
            Some("audio") => {
                let media_type = block
                    .get("mimeType")
                    .or_else(|| block.get("mime_type"))
                    .and_then(Value::as_str)
                    .unwrap_or("audio/*");
                text.push(format!("[audio: {media_type}]"));
            }
            _ => text.push(pretty_json(&block)),
        }
    }

    if let Some(structured) = result.structured_content {
        text.push(format!("Structured content:\n{}", pretty_json(&structured)));
    }

    let content = clip_output(text.join("\n\n"));
    if result.is_error {
        ToolRunResult::err(content, Vec::new())
    } else if images.is_empty() {
        ToolRunResult::ok(content, Vec::new())
    } else {
        ToolRunResult::ok_with_images(content, images, Vec::new())
    }
}

fn mcp_tool_description(server: &McpServerConfig, tool: &McpServerTool) -> String {
    let mut pieces = vec![format!(
        "MCP server `{}` tool `{}`.",
        server.name, tool.name
    )];
    if let Some(title) = tool
        .title
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        pieces.push(format!("Title: {title}."));
    }
    if let Some(description) = tool
        .description
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        pieces.push(description.to_string());
    }
    pieces.join(" ")
}

fn mcp_tool_display_name(tool: &McpServerTool) -> String {
    tool.title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| display_mcp_tool_name(&tool.name))
}

fn display_mcp_server_name(value: &str) -> String {
    let trimmed = value.trim();
    let Some(rest) = trimmed.get(3..) else {
        return trimmed.to_string();
    };
    if !trimmed[..3].eq_ignore_ascii_case("mcp") {
        return trimmed.to_string();
    }

    let stripped = rest
        .trim_start_matches(|ch: char| ch == '-' || ch == '_' || ch == '.' || ch.is_whitespace())
        .trim();
    if stripped.is_empty() {
        trimmed.to_string()
    } else {
        stripped.to_string()
    }
}

fn display_mcp_tool_name(value: &str) -> String {
    let mut spaced = String::new();
    let mut previous: Option<char> = None;

    for ch in value.trim().chars() {
        if matches!(ch, '_' | '-' | '.') {
            if !spaced.ends_with(' ') {
                spaced.push(' ');
            }
        } else {
            if let Some(prev) = previous {
                if ch.is_ascii_uppercase() && (prev.is_ascii_lowercase() || prev.is_ascii_digit()) {
                    spaced.push(' ');
                }
            }
            spaced.push(ch);
        }
        previous = Some(ch);
    }

    let words = spaced
        .split_whitespace()
        .map(display_mcp_word)
        .collect::<Vec<_>>();

    if words.is_empty() {
        "Tool".to_string()
    } else {
        words.join(" ")
    }
}

fn display_mcp_word(word: &str) -> String {
    if word
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
    {
        return word.to_string();
    }

    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    format!(
        "{}{}",
        first.to_uppercase().collect::<String>(),
        chars.as_str().to_ascii_lowercase()
    )
}

fn normalize_input_schema(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        })
    }
}

fn unique_tool_name(
    server: &McpServerConfig,
    original_name: &str,
    known: &HashMap<String, McpToolBinding>,
) -> String {
    let server_slug = slug(&server.name)
        .or_else(|| slug(&server.id))
        .unwrap_or_else(|| "server".into());
    let tool_slug = slug(original_name).unwrap_or_else(|| "tool".into());
    let hash = short_hash(&(server.id.as_str(), original_name));
    let mut base = format!("mcp__{server_slug}__{tool_slug}");
    if base.len() > TOOL_NAME_LIMIT {
        let budget = TOOL_NAME_LIMIT.saturating_sub(7 + hash.len());
        base = format!("{}__{}", truncate_chars(&base, budget), hash);
    }

    if !known.contains_key(&base) {
        return base;
    }

    for idx in 2..1000 {
        let suffix = format!("__{idx}");
        let candidate = if base.len() + suffix.len() > TOOL_NAME_LIMIT {
            format!(
                "{}{}",
                truncate_chars(&base, TOOL_NAME_LIMIT - suffix.len()),
                suffix
            )
        } else {
            format!("{base}{suffix}")
        };
        if !known.contains_key(&candidate) {
            return candidate;
        }
    }

    format!("mcp__tool__{hash}")
}

fn slug(value: &str) -> Option<String> {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_' || ch == ' ') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    (!out.is_empty()).then_some(out)
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn short_hash<T: Hash>(value: &T) -> String {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())[..8].to_string()
}

fn default_enabled() -> bool {
    true
}

struct McpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    request_timeout: Duration,
}

impl McpStdioClient {
    async fn connect_with_timeout(
        config: &McpServerConfig,
        request_timeout: Duration,
    ) -> Result<Self> {
        let command_name = config.command.trim();
        if command_name.is_empty() {
            bail!("missing MCP command for {}", config.name);
        }

        let search_paths = mcp_search_paths(config);
        let program = resolve_mcp_command(command_name, &search_paths)
            .unwrap_or_else(|| PathBuf::from(command_name));
        let path_env = env::join_paths(&search_paths).ok();
        let mut command = Command::new(program);
        command
            .args(config.args.iter().filter(|arg| !arg.is_empty()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(path_env) = path_env {
            command.env("PATH", path_env);
        }

        if let Some(cwd) = config
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            command.current_dir(cwd);
        }
        for env in &config.env {
            let key = env.key.trim();
            if !key.is_empty() {
                if is_path_env_key(key) {
                    continue;
                }
                command.env(key, &env.value);
            }
        }

        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);

        let mut child = command
            .spawn()
            .with_context(|| format!("unable to spawn `{}`", config.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("MCP server stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("MCP server stdout unavailable"))?;

        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut sink = Vec::new();
                let _ = stderr.read_to_end(&mut sink).await;
            });
        }

        let mut client = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            request_timeout,
        };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "sinew",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )
        .await?;
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<McpServerTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = match cursor.as_deref() {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let value = self.request("tools/list", params).await?;
            let page: McpListToolsResult =
                serde_json::from_value(value).context("invalid MCP tools/list response")?;
            tools.extend(page.tools);
            cursor = page.next_cursor;
            if cursor.as_deref().unwrap_or_default().is_empty() {
                break;
            }
        }

        Ok(tools)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallToolResult> {
        let params = json!({
            "name": name,
            "arguments": match arguments {
                Value::Object(_) => arguments,
                _ => json!({}),
            }
        });
        let value = self.request("tools/call", params).await?;
        serde_json::from_value(value).context("invalid MCP tools/call response")
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await?;

        timeout(self.request_timeout, self.read_response(id))
            .await
            .map_err(|_| anyhow!("MCP request `{method}` timed out"))?
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut message = json!({
            "jsonrpc": "2.0",
            "method": method
        });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.write_message(message).await
    }

    async fn read_response(&mut self, id: u64) -> Result<Value> {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).await?;
            if read == 0 {
                let status = self.child.try_wait().ok().flatten();
                bail!("MCP server closed stdout ({status:?})");
            }

            let value: Value = serde_json::from_str(line.trim())
                .with_context(|| "MCP server emitted invalid JSON")?;
            if value.get("id") == Some(&json!(id)) {
                if let Some(error) = value.get("error") {
                    bail!("{}", format_json_rpc_error(error));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| anyhow!("MCP response missing result"));
            }

            if let Some(request_id) = value.get("id").cloned() {
                if value.get("method").is_some() {
                    self.write_message(json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {
                            "code": -32601,
                            "message": "Method not supported by Sinew MCP client"
                        }
                    }))
                    .await?;
                }
            }
        }
    }

    async fn write_message(&mut self, value: Value) -> Result<()> {
        let mut line = serde_json::to_vec(&value)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

/* ────────────────────── Transport dispatch ────────────────────────── */

fn has_url(config: &McpServerConfig) -> bool {
    config
        .url
        .as_deref()
        .map(str::trim)
        .is_some_and(|u| !u.is_empty())
}

fn is_remote_server(config: &McpServerConfig) -> bool {
    has_url(config) || matches!(config.transport, McpTransport::Http | McpTransport::Sse)
}

enum McpClient {
    Stdio(McpStdioClient),
    Http(McpHttpClient),
    Sse(McpSseClient),
}

impl McpClient {
    async fn connect(config: &McpServerConfig) -> Result<Self> {
        Self::connect_with_timeout(config, REQUEST_TIMEOUT).await
    }

    async fn connect_with_timeout(config: &McpServerConfig, t: Duration) -> Result<Self> {
        match effective_transport(config) {
            McpTransport::Stdio => Ok(Self::Stdio(
                McpStdioClient::connect_with_timeout(config, t).await?,
            )),
            McpTransport::Http => Ok(Self::Http(
                McpHttpClient::connect_with_timeout(config, t).await?,
            )),
            McpTransport::Sse => Ok(Self::Sse(
                McpSseClient::connect_with_timeout(config, t).await?,
            )),
        }
    }

    async fn list_tools(&mut self) -> Result<Vec<McpServerTool>> {
        match self {
            Self::Stdio(c) => c.list_tools().await,
            Self::Http(c) => c.list_tools().await,
            Self::Sse(c) => c.list_tools().await,
        }
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallToolResult> {
        match self {
            Self::Stdio(c) => c.call_tool(name, arguments).await,
            Self::Http(c) => c.call_tool(name, arguments).await,
            Self::Sse(c) => c.call_tool(name, arguments).await,
        }
    }
}

fn effective_transport(config: &McpServerConfig) -> McpTransport {
    match config.transport {
        McpTransport::Stdio if has_url(config) => McpTransport::Http,
        other => other,
    }
}

fn config_headers(config: &McpServerConfig) -> Vec<(String, String)> {
    config
        .headers
        .iter()
        .filter(|h| !h.key.trim().is_empty())
        .map(|h| (h.key.trim().to_string(), h.value.clone()))
        .collect()
}

fn remote_url(config: &McpServerConfig, label: &str) -> Result<String> {
    config
        .url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("{label} MCP server `{}` missing url", config.name))
        .map(str::to_string)
}

fn apply_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    for (k, v) in headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder
}

/* ─────────────────────────── HTTP client ──────────────────────────── */

struct McpHttpClient {
    url: String,
    headers: Vec<(String, String)>,
    session_id: Option<String>,
    client: reqwest::Client,
    next_id: u64,
    request_timeout: Duration,
}

impl McpHttpClient {
    async fn connect_with_timeout(
        config: &McpServerConfig,
        request_timeout: Duration,
    ) -> Result<Self> {
        let url = remote_url(config, "HTTP")?;

        let headers = config_headers(config);

        let client = reqwest::Client::builder()
            .timeout(request_timeout + Duration::from_secs(5))
            .build()
            .context("unable to build HTTP client")?;

        let mut c = Self {
            url,
            headers,
            session_id: None,
            client,
            next_id: 1,
            request_timeout,
        };
        // initialize is best-effort — some servers skip it
        if let Err(err) = c.initialize().await {
            warn!("MCP HTTP initialize failed for `{}`: {err}", config.name);
        }
        Ok(c)
    }

    async fn initialize(&mut self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "sinew", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
            .await?;
        // Store session id if the server issued one in the JSON body. Most
        // streamable HTTP servers send it as a response header, which is
        // captured in `request`, but accepting both keeps older remotes happy.
        if let Some(id) = result
            .get("sessionId")
            .or_else(|| result.get("session_id"))
            .and_then(Value::as_str)
        {
            self.session_id = Some(id.to_string());
        }
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut body = json!({
            "jsonrpc": "2.0",
            "method": method
        });
        if let Some(params) = params {
            body["params"] = params;
        }

        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        builder = apply_headers(builder, &self.headers);
        if let Some(sid) = &self.session_id {
            builder = builder.header("Mcp-Session-Id", sid.as_str());
        }

        let resp = timeout(self.request_timeout, builder.send())
            .await
            .map_err(|_| anyhow!("HTTP MCP notification `{method}` timed out"))?
            .with_context(|| format!("HTTP MCP notification `{method}` failed"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("HTTP MCP notification returned {status}: {text}");
        }
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<McpServerTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match cursor.as_deref() {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let value = self.request("tools/list", params).await?;
            let page: McpListToolsResult =
                serde_json::from_value(value).context("invalid MCP tools/list response")?;
            tools.extend(page.tools);
            cursor = page.next_cursor;
            if cursor.as_deref().unwrap_or_default().is_empty() {
                break;
            }
        }
        Ok(tools)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallToolResult> {
        let params = json!({
            "name": name,
            "arguments": match arguments { Value::Object(_) => arguments, _ => json!({}) }
        });
        let value = self.request("tools/call", params).await?;
        serde_json::from_value(value).context("invalid MCP tools/call response")
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let mut builder = self
            .client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&body);

        builder = apply_headers(builder, &self.headers);
        if let Some(sid) = &self.session_id {
            builder = builder.header("Mcp-Session-Id", sid.as_str());
        }

        let resp = timeout(self.request_timeout, builder.send())
            .await
            .map_err(|_| anyhow!("HTTP MCP request `{method}` timed out"))?
            .with_context(|| format!("HTTP MCP request `{method}` failed"))?;

        if let Some(sid) = resp
            .headers()
            .get("Mcp-Session-Id")
            .or_else(|| resp.headers().get("mcp-session-id"))
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.trim().is_empty())
        {
            self.session_id = Some(sid.to_string());
        }

        let status = resp.status();
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("HTTP MCP server returned {status}: {text}");
        }

        if ct.contains("text/event-stream") {
            self.collect_sse(resp, id).await
        } else {
            let value: Value = resp
                .json()
                .await
                .context("HTTP MCP response was not valid JSON")?;
            extract_jsonrpc_result(&value)
        }
    }

    async fn collect_sse(&self, resp: reqwest::Response, expected_id: u64) -> Result<Value> {
        let mut stream = resp.bytes_stream().eventsource();
        while let Some(event) = stream.next().await {
            let event = event.context("SSE stream error")?;
            if let Some(value) = jsonrpc_from_sse_event(&event.data) {
                if value.get("id") == Some(&json!(expected_id)) {
                    return extract_jsonrpc_result(&value);
                }
            }
        }
        bail!("HTTP MCP SSE stream closed without matching response")
    }
}

/* ─────────────────────────── Legacy SSE client ──────────────────────── */

type SseEventStream = BoxStream<
    'static,
    Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>,
>;

struct McpSseClient {
    message_url: String,
    headers: Vec<(String, String)>,
    client: reqwest::Client,
    stream: SseEventStream,
    next_id: u64,
    request_timeout: Duration,
}

impl McpSseClient {
    async fn connect_with_timeout(
        config: &McpServerConfig,
        request_timeout: Duration,
    ) -> Result<Self> {
        let sse_url = remote_url(config, "SSE")?;
        let headers = config_headers(config);
        let client = reqwest::Client::builder()
            .timeout(request_timeout + Duration::from_secs(5))
            .build()
            .context("unable to build SSE client")?;

        let (message_url, stream) =
            open_sse_stream(&client, &sse_url, &headers, request_timeout).await?;
        let mut c = Self {
            message_url,
            headers,
            client,
            stream,
            next_id: 1,
            request_timeout,
        };
        c.initialize().await?;
        c.notify("notifications/initialized", None).await?;
        Ok(c)
    }

    async fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "sinew", "version": env!("CARGO_PKG_VERSION") }
            }),
        )
        .await?;
        Ok(())
    }

    async fn list_tools(&mut self) -> Result<Vec<McpServerTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match cursor.as_deref() {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let value = self.request("tools/list", params).await?;
            let page: McpListToolsResult =
                serde_json::from_value(value).context("invalid MCP tools/list response")?;
            tools.extend(page.tools);
            cursor = page.next_cursor;
            if cursor.as_deref().unwrap_or_default().is_empty() {
                break;
            }
        }
        Ok(tools)
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<McpCallToolResult> {
        let params = json!({
            "name": name,
            "arguments": match arguments { Value::Object(_) => arguments, _ => json!({}) }
        });
        let value = self.request("tools/call", params).await?;
        serde_json::from_value(value).context("invalid MCP tools/call response")
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.read_sse_response_after_post(id, &message).await
    }

    async fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut message = json!({
            "jsonrpc": "2.0",
            "method": method
        });
        if let Some(params) = params {
            message["params"] = params;
        }
        post_sse_message(
            self.client.clone(),
            self.message_url.clone(),
            self.headers.clone(),
            self.request_timeout,
            message,
        )
        .await
    }

    async fn read_sse_response_after_post(
        &mut self,
        expected_id: u64,
        message: &Value,
    ) -> Result<Value> {
        post_sse_message(
            self.client.clone(),
            self.message_url.clone(),
            self.headers.clone(),
            self.request_timeout,
            message.clone(),
        )
        .await?;
        while let Some(event) = self.stream.next().await {
            let event = event.context("SSE stream error")?;
            if let Some(value) = jsonrpc_from_sse_event(&event.data) {
                if value.get("id") == Some(&json!(expected_id)) {
                    return extract_jsonrpc_result(&value);
                }
            }
        }
        bail!("SSE MCP stream closed without matching response")
    }
}

async fn post_sse_message(
    client: reqwest::Client,
    message_url: String,
    headers: Vec<(String, String)>,
    request_timeout: Duration,
    message: Value,
) -> Result<()> {
    let builder = client
        .post(&message_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&message);
    let resp = timeout(request_timeout, apply_headers(builder, &headers).send())
        .await
        .map_err(|_| anyhow!("SSE MCP POST timed out"))?
        .context("SSE MCP POST failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("SSE MCP POST returned {status}: {text}");
    }
    Ok(())
}

async fn open_sse_stream(
    client: &reqwest::Client,
    sse_url: &str,
    headers: &[(String, String)],
    request_timeout: Duration,
) -> Result<(String, SseEventStream)> {
    let builder = client.get(sse_url).header("Accept", "text/event-stream");
    let resp = timeout(request_timeout, apply_headers(builder, headers).send())
        .await
        .map_err(|_| anyhow!("SSE MCP endpoint discovery timed out"))?
        .context("SSE MCP endpoint discovery failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("SSE MCP endpoint discovery returned {status}: {text}");
    }

    let base = resp.url().clone();
    let mut stream = resp.bytes_stream().eventsource().boxed();
    while let Some(event) = stream.next().await {
        let event = event.context("SSE endpoint stream error")?;
        if event.event == "endpoint" {
            let endpoint = event.data.trim();
            if endpoint.is_empty() {
                continue;
            }
            let message_url = resolve_url(&base, endpoint)
                .with_context(|| format!("invalid SSE message endpoint `{endpoint}`"))?;
            return Ok((message_url, stream));
        }
        if let Some(endpoint) = endpoint_from_json_event(&event.data) {
            let message_url = resolve_url(&base, &endpoint)
                .with_context(|| format!("invalid SSE message endpoint `{endpoint}`"))?;
            return Ok((message_url, stream));
        }
    }
    bail!("SSE MCP server did not advertise a message endpoint")
}

fn jsonrpc_from_sse_event(data: &str) -> Option<Value> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return None;
    }
    serde_json::from_str::<Value>(trimmed).ok()
}

fn endpoint_from_json_event(data: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(data.trim()).ok()?;
    input_string(&value, &["endpoint", "uri", "url"])
}

fn resolve_url(base: &url::Url, value: &str) -> Result<String> {
    Ok(base.join(value)?.to_string())
}

fn extract_jsonrpc_result(value: &Value) -> Result<Value> {
    if let Some(error) = value.get("error") {
        bail!("{}", format_json_rpc_error(error));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("MCP response missing result"))
}

static DEFAULT_MCP_SEARCH_PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();

fn mcp_search_paths(config: &McpServerConfig) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = config.env.iter().rev().find_map(|env| {
        let key = env.key.trim();
        is_path_env_key(key).then_some(env.value.as_str())
    }) {
        push_split_paths(&mut paths, &mut seen, OsStr::new(path));
    }

    for path in default_mcp_search_paths() {
        push_path(&mut paths, &mut seen, path.clone());
    }

    paths
}

fn default_mcp_search_paths() -> &'static [PathBuf] {
    DEFAULT_MCP_SEARCH_PATHS
        .get_or_init(build_default_mcp_search_paths)
        .as_slice()
}

fn build_default_mcp_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    if let Some(path) = env::var_os("PATH") {
        push_split_paths(&mut paths, &mut seen, &path);
    }

    push_common_node_paths(&mut paths, &mut seen);

    paths
}

fn push_common_node_paths(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    #[cfg(target_os = "macos")]
    {
        push_dir(paths, seen, "/opt/homebrew/bin");
        push_dir(paths, seen, "/usr/local/bin");
        push_dir(paths, seen, "/usr/bin");
        push_dir(paths, seen, "/bin");
        push_dir(paths, seen, "/usr/sbin");
        push_dir(paths, seen, "/sbin");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        push_dir(paths, seen, "/usr/local/bin");
        push_dir(paths, seen, "/usr/bin");
        push_dir(paths, seen, "/bin");
        push_dir(paths, seen, "/snap/bin");
        push_dir(paths, seen, "/home/linuxbrew/.linuxbrew/bin");
    }

    #[cfg(windows)]
    {
        if let Some(app_data) = env::var_os("APPDATA") {
            push_dir(paths, seen, PathBuf::from(app_data).join("npm"));
        }
        if let Some(program_files) = env::var_os("ProgramFiles") {
            push_dir(paths, seen, PathBuf::from(program_files).join("nodejs"));
        }
        if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
            push_dir(paths, seen, PathBuf::from(program_files_x86).join("nodejs"));
        }
    }

    let Some(home) = home_dir() else {
        return;
    };

    push_dir(paths, seen, home.join(".local/bin"));
    push_dir(paths, seen, home.join(".volta/bin"));
    push_dir(paths, seen, home.join(".asdf/shims"));
    push_dir(paths, seen, home.join(".nodenv/shims"));
    push_dir(paths, seen, home.join(".local/share/mise/shims"));
    push_dir(paths, seen, home.join(".mise/shims"));

    push_versioned_dir(paths, seen, home.join(".nvm/versions/node"), &["bin"]);
    push_versioned_dir(paths, seen, home.join(".asdf/installs/nodejs"), &["bin"]);
    push_versioned_dir(paths, seen, home.join(".nodenv/versions"), &["bin"]);
    push_versioned_dir(
        paths,
        seen,
        home.join(".local/share/mise/installs/node"),
        &["bin"],
    );
    push_versioned_dir(
        paths,
        seen,
        home.join(".local/share/fnm/node-versions"),
        &["installation", "bin"],
    );
}

fn push_split_paths(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, value: &OsStr) {
    for path in env::split_paths(value) {
        push_path(paths, seen, path);
    }
}

fn push_dir(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if path.is_dir() {
        push_path(paths, seen, path);
    }
}

fn push_versioned_dir(
    paths: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    root: PathBuf,
    suffix: &[&str],
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries = entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        version_key(&b.file_name())
            .cmp(&version_key(&a.file_name()))
            .then_with(|| b.file_name().cmp(&a.file_name()))
    });

    for entry in entries {
        let mut path = entry.path();
        for segment in suffix {
            path.push(segment);
        }
        push_dir(paths, seen, path);
    }
}

fn push_path(paths: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, path: PathBuf) {
    if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
        return;
    }
    paths.push(path);
}

fn resolve_mcp_command(command: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    if command_has_path_separator(command) {
        return None;
    }

    for dir in paths {
        let candidate = dir.join(command);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }

        #[cfg(windows)]
        if Path::new(command).extension().is_none() {
            for extension in windows_path_extensions() {
                let candidate = dir.join(format!("{command}{extension}"));
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

fn command_has_path_separator(command: &str) -> bool {
    command.contains('/') || command.contains('\\')
}

#[cfg(windows)]
fn windows_path_extensions() -> Vec<String> {
    env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| {
                    if extension.starts_with('.') {
                        extension.to_string()
                    } else {
                        format!(".{extension}")
                    }
                })
                .collect()
        })
        .unwrap_or_else(|| vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()])
}

#[cfg(windows)]
fn is_path_env_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("PATH")
}

#[cfg(not(windows))]
fn is_path_env_key(key: &str) -> bool {
    key == "PATH"
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn version_key(value: &OsStr) -> Vec<u64> {
    let numbers = value
        .to_string_lossy()
        .trim_start_matches('v')
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect::<Vec<_>>();

    if numbers.is_empty() {
        vec![0]
    } else {
        numbers
    }
}

#[derive(Debug, Deserialize)]
struct McpListToolsResult {
    #[serde(default)]
    tools: Vec<McpServerTool>,
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct McpServerTool {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpCallToolResult {
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    structured_content: Option<Value>,
    #[serde(default)]
    is_error: bool,
}

fn format_json_rpc_error(value: &Value) -> String {
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("MCP JSON-RPC error");
    let code = value.get("code").and_then(Value::as_i64);
    match code {
        Some(code) => format!("{message} ({code})"),
        None => message.to_string(),
    }
}

fn pretty_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn clip_output(value: String) -> String {
    if value.len() <= TOOL_OUTPUT_LIMIT {
        return value;
    }
    let mut clipped = value.chars().take(TOOL_OUTPUT_LIMIT).collect::<String>();
    clipped.push_str("\n\n[Output truncated]");
    clipped
}

/* ─────────────────────────── MCP import ────────────────────────────── */
//
// Import MCP server definitions from the native config files of other
// LLM CLIs the user already runs (Claude Code, Codex). The schemas line
// up almost 1-to-1 with `McpServerConfig` (command + args + env), so we
// just parse, normalise and merge — duplicates by name are skipped so a
// re-import is idempotent.

#[derive(Debug, Clone, Copy)]
pub enum McpImportFormat {
    /// Claude Code / Claude Desktop config (JSON, top-level
    /// `mcpServers` object — server name → { command, args, env }).
    ClaudeJson,
    /// Codex CLI config (TOML, sections `[mcp_servers.<name>]`).
    CodexToml,
}

impl McpImportFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-json" | "claude_json" => Ok(Self::ClaudeJson),
            "codex" | "codex-toml" | "codex_toml" => Ok(Self::CodexToml),
            other => bail!("unknown MCP import format `{other}` (expected `claude` or `codex`)"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedMcpServerInfo {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedMcpServerInfo {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMcpResult {
    pub imported: Vec<ImportedMcpServerInfo>,
    pub skipped: Vec<SkippedMcpServerInfo>,
    pub source_path: String,
}

/// Read `path` according to `format` and return parsed
/// `McpServerConfig` entries ready to merge. Each entry already has a
/// freshly generated id; callers re-key/skip them by name.
pub fn parse_mcp_import_file(path: &Path, format: McpImportFormat) -> Result<Vec<McpServerConfig>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("unable to read MCP import file {}", path.display()))?;
    match format {
        McpImportFormat::ClaudeJson => parse_claude_mcp_json(&raw),
        McpImportFormat::CodexToml => parse_codex_mcp_toml(&raw),
    }
}

fn parse_claude_mcp_json(raw: &str) -> Result<Vec<McpServerConfig>> {
    // Supports two formats:
    // 1. Claude Desktop: top-level `mcpServers` object
    // 2. Claude Code settings: `mcp.servers` object (also handles `type: "http"`)
    let root: Value = serde_json::from_str(raw).context("invalid JSON in Claude config")?;

    // Prefer top-level `mcpServers`; fall back to `mcp.servers`
    let servers = if let Some(Value::Object(map)) = root.get("mcpServers") {
        map.clone()
    } else if let Some(Value::Object(map)) = root.get("mcp").and_then(|v| v.get("servers")) {
        map.clone()
    } else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(servers.len());
    for (name, value) in servers {
        let Value::Object(entry) = value else {
            continue;
        };

        let transport = entry
            .get("type")
            .or_else(|| entry.get("transport"))
            .and_then(Value::as_str)
            .unwrap_or("stdio");

        if transport.eq_ignore_ascii_case("http")
            || transport.eq_ignore_ascii_case("streamable_http")
            || transport.eq_ignore_ascii_case("streamable-http")
            || transport.eq_ignore_ascii_case("sse")
        {
            // Remote transport
            let url = entry
                .get("url")
                .or_else(|| entry.get("serverUrl"))
                .or_else(|| entry.get("server_url"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string);
            let Some(url) = url else { continue };
            let headers = parse_env_object(entry.get("headers"));
            out.push(McpServerConfig {
                id: generate_mcp_id(&name),
                name,
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                cwd: None,
                enabled: true,
                transport: if transport.eq_ignore_ascii_case("sse") {
                    McpTransport::Sse
                } else {
                    McpTransport::Http
                },
                url: Some(url),
                headers,
                oauth: parse_oauth_config(&entry),
            });
        } else {
            // stdio transport
            let command = entry
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if command.is_empty() {
                continue;
            }
            let args = entry
                .get("args")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let env = parse_env_object(entry.get("env"));
            let cwd = entry
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            out.push(McpServerConfig {
                id: generate_mcp_id(&name),
                name,
                command,
                args,
                env,
                cwd,
                enabled: true,
                transport: McpTransport::Stdio,
                url: None,
                headers: Vec::new(),
                oauth: None,
            });
        }
    }
    Ok(out)
}

fn parse_codex_mcp_toml(raw: &str) -> Result<Vec<McpServerConfig>> {
    // Codex CLI configs typically place MCP servers under
    // `[mcp_servers.<name>]` (newer) or `[mcp.<name>]` (older). Try
    // both so the user does not have to care which Codex version they
    // are coming from.
    let root: toml::Value = raw.parse().context("invalid TOML in Codex config")?;
    let table = root
        .as_table()
        .ok_or_else(|| anyhow!("Codex config must be a top-level TOML table"))?;
    let candidates = ["mcp_servers", "mcpServers", "mcp"];
    let block = candidates
        .iter()
        .find_map(|key| table.get(*key).and_then(|v| v.as_table()))
        .cloned()
        .unwrap_or_default();
    if block.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::with_capacity(block.len());
    for (name, value) in block {
        let Some(entry) = value.as_table() else {
            continue;
        };
        let transport = entry
            .get("type")
            .or_else(|| entry.get("transport"))
            .and_then(|v| v.as_str())
            .map(parse_transport_label)
            .unwrap_or(McpTransport::Stdio);
        let url = entry
            .get("url")
            .or_else(|| entry.get("server_url"))
            .or_else(|| entry.get("serverUrl"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if matches!(transport, McpTransport::Http | McpTransport::Sse) || url.is_some() {
            let Some(url) = url else { continue };
            let headers = entry
                .get("headers")
                .and_then(|v| v.as_table())
                .map(|tbl| {
                    tbl.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .map(|(key, value)| McpEnvVar { key, value })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            out.push(McpServerConfig {
                id: generate_mcp_id(&name),
                name,
                command: String::new(),
                args: Vec::new(),
                env: Vec::new(),
                cwd: None,
                enabled: true,
                transport: if matches!(transport, McpTransport::Sse) {
                    McpTransport::Sse
                } else {
                    McpTransport::Http
                },
                url: Some(url),
                headers,
                oauth: parse_toml_oauth_config(entry),
            });
            continue;
        }
        let command = entry
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if command.is_empty() {
            continue;
        }
        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let env = entry
            .get("env")
            .and_then(|v| v.as_table())
            .map(|tbl| {
                tbl.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .map(|(key, value)| McpEnvVar { key, value })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cwd = entry
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        out.push(McpServerConfig {
            id: generate_mcp_id(&name),
            name,
            command,
            args,
            env,
            cwd,
            enabled: true,
            transport: McpTransport::Stdio,
            url: None,
            headers: Vec::new(),
            oauth: None,
        });
    }
    Ok(out)
}

fn parse_transport_label(value: &str) -> McpTransport {
    match value.trim().to_ascii_lowercase().as_str() {
        "http" | "streamable_http" | "streamable-http" => McpTransport::Http,
        "sse" => McpTransport::Sse,
        _ => McpTransport::Stdio,
    }
}

fn parse_env_object(value: Option<&Value>) -> Vec<McpEnvVar> {
    let Some(Value::Object(map)) = value else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .map(|(key, value)| McpEnvVar { key, value })
        .collect()
}

fn parse_oauth_config(entry: &serde_json::Map<String, Value>) -> Option<McpOAuthClientConfig> {
    let nested = entry.get("oauth").and_then(Value::as_object);
    let string_field = |name: &str| {
        nested
            .and_then(|oauth| oauth.get(name))
            .or_else(|| entry.get(name))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let config = McpOAuthClientConfig {
        client_id: string_field("clientId").or_else(|| string_field("client_id")),
        client_secret: string_field("clientSecret").or_else(|| string_field("client_secret")),
        scope: string_field("scope"),
        authorization_endpoint: string_field("authorizationEndpoint")
            .or_else(|| string_field("authorization_endpoint")),
        token_endpoint: string_field("tokenEndpoint").or_else(|| string_field("token_endpoint")),
        resource: string_field("resource"),
        token_endpoint_auth_method: string_field("tokenEndpointAuthMethod")
            .or_else(|| string_field("token_endpoint_auth_method")),
    };
    oauth_config_has_values(&config).then_some(config)
}

fn parse_toml_oauth_config(
    entry: &toml::map::Map<String, toml::Value>,
) -> Option<McpOAuthClientConfig> {
    let nested = entry.get("oauth").and_then(|value| value.as_table());
    let string_field = |name: &str| {
        nested
            .and_then(|oauth| oauth.get(name))
            .or_else(|| entry.get(name))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let config = McpOAuthClientConfig {
        client_id: string_field("clientId").or_else(|| string_field("client_id")),
        client_secret: string_field("clientSecret").or_else(|| string_field("client_secret")),
        scope: string_field("scope"),
        authorization_endpoint: string_field("authorizationEndpoint")
            .or_else(|| string_field("authorization_endpoint")),
        token_endpoint: string_field("tokenEndpoint").or_else(|| string_field("token_endpoint")),
        resource: string_field("resource"),
        token_endpoint_auth_method: string_field("tokenEndpointAuthMethod")
            .or_else(|| string_field("token_endpoint_auth_method")),
    };
    oauth_config_has_values(&config).then_some(config)
}

fn oauth_config_has_values(config: &McpOAuthClientConfig) -> bool {
    config.client_id.is_some()
        || config.client_secret.is_some()
        || config.scope.is_some()
        || config.authorization_endpoint.is_some()
        || config.token_endpoint.is_some()
        || config.resource.is_some()
        || config.token_endpoint_auth_method.is_some()
}

fn generate_mcp_id(name: &str) -> String {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    std::time::SystemTime::now().hash(&mut hasher);
    format!("mcp-{:x}", hasher.finish())
}

/// Merge `imported` into `current` and report what got added vs
/// skipped. Skip rule: a server is skipped when an entry with the same
/// (case-insensitive) name already exists — so re-importing the same
/// file is a safe no-op.
pub fn merge_imported_mcp_servers(
    current: &McpSettings,
    imported: Vec<McpServerConfig>,
    source_path: impl Into<String>,
) -> (McpSettings, ImportMcpResult) {
    let mut settings = current.clone();
    let mut result = ImportMcpResult {
        source_path: source_path.into(),
        ..Default::default()
    };
    let mut seen_names: HashSet<String> = settings
        .servers
        .iter()
        .map(|server| server.name.to_ascii_lowercase())
        .collect();

    for server in imported {
        let key = server.name.to_ascii_lowercase();
        if seen_names.contains(&key) {
            result.skipped.push(SkippedMcpServerInfo {
                name: server.name,
                reason: "already configured in Sinew".into(),
            });
            continue;
        }
        seen_names.insert(key);
        result.imported.push(ImportedMcpServerInfo {
            name: server.name.clone(),
        });
        settings.servers.push(server);
    }
    (settings, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_claude_stdio_http_and_sse_servers() {
        let raw = r#"{
          "mcpServers": {
            "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"] },
            "figma": { "type": "http", "url": "https://mcp.figma.com/mcp", "headers": { "Authorization": "Bearer token" } },
            "legacy": { "transport": "sse", "url": "https://example.com/sse" }
          }
        }"#;

        let parsed = parse_claude_mcp_json(raw).expect("parse Claude MCP config");
        assert_eq!(parsed.len(), 3);
        let filesystem = parsed.iter().find(|s| s.name == "filesystem").unwrap();
        assert_eq!(filesystem.transport, McpTransport::Stdio);
        assert_eq!(filesystem.command, "npx");

        let figma = parsed.iter().find(|s| s.name == "figma").unwrap();
        assert_eq!(figma.transport, McpTransport::Http);
        assert_eq!(figma.url.as_deref(), Some("https://mcp.figma.com/mcp"));
        assert_eq!(figma.headers[0].key, "Authorization");

        let legacy = parsed.iter().find(|s| s.name == "legacy").unwrap();
        assert_eq!(legacy.transport, McpTransport::Sse);
        assert_eq!(legacy.url.as_deref(), Some("https://example.com/sse"));
    }

    #[test]
    fn imports_oauth_hints() {
        let raw = r#"{
          "mcpServers": {
            "remote": {
              "type": "http",
              "url": "https://example.com/mcp",
              "oauth": { "clientId": "abc", "clientSecret": "def", "scope": "mcp:connect" }
            }
          }
        }"#;

        let parsed = parse_claude_mcp_json(raw).expect("parse MCP config with OAuth hints");
        let oauth = parsed[0].oauth.as_ref().expect("oauth hints");
        assert_eq!(oauth.client_id.as_deref(), Some("abc"));
        assert_eq!(oauth.client_secret.as_deref(), Some("def"));
        assert_eq!(oauth.scope.as_deref(), Some("mcp:connect"));
    }

    #[test]
    fn imports_codex_remote_servers() {
        let raw = r#"
[mcp_servers.figma]
type = "http"
url = "https://mcp.figma.com/mcp"
headers = { Authorization = "Bearer token" }

[mcp_servers.legacy]
transport = "sse"
url = "https://example.com/sse"

[mcp_servers.local]
command = "node"
args = ["server.js"]
"#;

        let parsed = parse_codex_mcp_toml(raw).expect("parse Codex MCP config");
        assert_eq!(parsed.len(), 3);
        assert_eq!(
            parsed.iter().find(|s| s.name == "figma").unwrap().transport,
            McpTransport::Http
        );
        assert_eq!(
            parsed
                .iter()
                .find(|s| s.name == "legacy")
                .unwrap()
                .transport,
            McpTransport::Sse
        );
        assert_eq!(
            parsed.iter().find(|s| s.name == "local").unwrap().transport,
            McpTransport::Stdio
        );
    }
}
