//! OAuth 2.1 support for remote (HTTP) MCP servers.
//!
//! Modern hosted MCP servers (Figma, Linear, Notion, Sentry, GitHub's remote
//! server…) don't accept a static API key — they require the MCP OAuth flow:
//! discovery (RFC 9728 + RFC 8414), dynamic client registration (RFC 7591),
//! and an authorization-code grant with PKCE. This module holds the pure,
//! headless pieces; the desktop crate drives the interactive bits (loopback
//! server, opening the browser).

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::store::AppStore;

/// Clock skew we subtract when deciding whether an access token is still good.
const EXPIRY_SKEW_MS: i64 = 120_000;
/// Longest `scope` value sent to the browser. Some servers advertise every
/// scope they have (Cloudflare: 403 of them, 8 KB), too long for a URL.
const MAX_SCOPE_CHARS: usize = 2_000;
const OFFLINE_ACCESS: &str = "offline_access";
const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const SIGN_IN_EXPIRED: &str =
    "The sign-in for this server has expired. Use Re-authenticate (OAuth) to sign in again.";

/// Persisted OAuth state for one MCP server (keyed by server id in the store).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthRecord {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Absolute expiry (epoch ms). None = unknown / never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// How the client authenticates to the token endpoint. Usually `none` for
    /// public PKCE clients, or `client_secret_post` / `client_secret_basic` for
    /// servers such as Figma that issue a client secret during registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,
}

impl McpOAuthRecord {
    /// True when the access token is missing or within the skew window of
    /// expiring — i.e. we should refresh before using it.
    pub fn needs_refresh(&self) -> bool {
        match self.expires_at_ms {
            Some(expires) => now_ms() + EXPIRY_SKEW_MS >= expires,
            None => false,
        }
    }
}

/// Endpoints + metadata discovered for an authorization server.
#[derive(Debug, Clone)]
pub struct OAuthMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    /// Scopes to ask for: what the server's 401 or resource metadata asks,
    /// else what its authorization server advertises.
    pub scopes_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    /// Canonical resource indicator (RFC 8707) — the MCP server URL.
    pub resource: String,
}

#[derive(Debug, Clone)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint_auth_method: Option<String>,
}

/// PKCE verifier + S256 challenge pair.
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate_pkce() -> PkcePair {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    PkcePair {
        verifier,
        challenge,
    }
}

pub fn generate_state() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Discover the authorization server for an MCP server URL.
///
/// Follows the MCP auth spec: try Protected Resource Metadata (RFC 9728) to
/// find the authorization server, then its metadata (RFC 8414 / OpenID). Falls
/// back to conventional endpoints on the origin when discovery is unavailable.
pub async fn discover(http: &reqwest::Client, server_url: &str) -> Result<OAuthMetadata> {
    let base = url::Url::parse(server_url)
        .with_context(|| format!("invalid MCP server url `{server_url}`"))?;
    let challenge = read_auth_challenge(http, server_url).await;
    discover_with_auth_server(http, &base, server_url, challenge).await
}

/// What a server's `401` says about how to sign in to it.
#[derive(Debug, Default, PartialEq, Eq)]
struct AuthChallenge {
    auth_server: Option<String>,
    resource_metadata: Option<String>,
    scope: Option<String>,
}

async fn read_auth_challenge(http: &reqwest::Client, server_url: &str) -> AuthChallenge {
    let response = http
        .post(server_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "sinew", "version": env!("CARGO_PKG_VERSION") }
            }
        }))
        .send()
        .await;
    response
        .ok()
        .and_then(|response| {
            response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .map(parse_auth_challenge)
        })
        .unwrap_or_default()
}

fn parse_auth_challenge(header: &str) -> AuthChallenge {
    let params = challenge_parameters(header);
    let get = |key: &str| {
        params
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
            .filter(|value| !value.trim().is_empty())
    };
    AuthChallenge {
        auth_server: get("authorization_uri")
            .or_else(|| get("authorization_url"))
            .or_else(|| get("authorization_server")),
        resource_metadata: get("resource_metadata"),
        scope: get("scope"),
    }
}

/// The `name="value"` pairs of a `WWW-Authenticate` header; quoted values may
/// hold commas and spaces.
fn challenge_parameters(header: &str) -> Vec<(String, String)> {
    let mut rest = header.trim();
    if let Some((scheme, after)) = rest.split_once(char::is_whitespace) {
        if !scheme.contains('=') {
            rest = after;
        }
    }
    let mut params = Vec::new();
    while let Some((name, after)) = rest.split_once('=') {
        let name = name.trim().trim_start_matches(',').trim().to_string();
        let after = after.trim_start();
        let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
            match quoted.split_once('"') {
                Some((value, remainder)) => (value, remainder),
                None => (quoted, ""),
            }
        } else {
            after.split_once(',').unwrap_or((after, ""))
        };
        params.push((name, value.trim().to_string()));
        rest = remainder.trim_start().trim_start_matches(',');
    }
    params
}

/// The scopes to ask for. Asking for everything an authorization server
/// advertises is wrong (the MCP spec starts from the server's 401, then its
/// resource metadata) and can make the URL too long to open. `offline_access`
/// is added when advertised: without it, some servers give no refresh token
/// and the connection dies when the access token expires.
fn scopes_to_request(
    challenge: Option<&str>,
    resource: Vec<String>,
    advertised: Vec<String>,
) -> Vec<String> {
    let offline = advertised.iter().any(|scope| scope == OFFLINE_ACCESS);
    let mut scopes = match challenge {
        Some(scope) => scope.split_whitespace().map(str::to_string).collect(),
        None if !resource.is_empty() => resource,
        None => advertised,
    };
    if scopes.join(" ").len() > MAX_SCOPE_CHARS {
        scopes.clear();
    }
    if offline && !scopes.iter().any(|scope| scope == OFFLINE_ACCESS) {
        scopes.push(OFFLINE_ACCESS.to_string());
    }
    scopes
}

async fn discover_with_auth_server(
    http: &reqwest::Client,
    base: &url::Url,
    server_url: &str,
    challenge: AuthChallenge,
) -> Result<OAuthMetadata> {
    let origin = format!(
        "{}://{}",
        base.scheme(),
        base.host_str()
            .ok_or_else(|| anyhow!("MCP url has no host"))?
    );
    let origin = match base.port() {
        Some(port) => format!("{origin}:{port}"),
        None => origin,
    };
    let path = base.path().trim_end_matches('/');

    // 1. Protected Resource Metadata → authorization_servers
    let mut auth_server: Option<String> = challenge.auth_server.clone();
    let mut resource = server_url.to_string();
    let mut resource_scopes_supported = Vec::new();
    let mut resource_candidates: Vec<String> =
        challenge.resource_metadata.iter().cloned().collect();
    resource_candidates.extend(well_known_urls(&origin, path, "oauth-protected-resource"));
    for candidate in resource_candidates {
        if let Some(doc) = fetch_json(http, &candidate).await {
            if auth_server.is_none() {
                if let Some(servers) = doc.get("authorization_servers").and_then(Value::as_array) {
                    if let Some(first) = servers.first().and_then(Value::as_str) {
                        auth_server = Some(first.to_string());
                    }
                }
            }
            if let Some(res) = doc.get("resource").and_then(Value::as_str) {
                resource = res.to_string();
            }
            if let Some(scopes) = doc.get("scopes_supported").and_then(json_string_array) {
                resource_scopes_supported = scopes;
            }
            if auth_server.is_some() {
                break;
            }
        }
    }

    // The authorization server to introspect: discovered one, or the origin.
    let as_base = auth_server.clone().unwrap_or_else(|| origin.clone());
    let as_url = url::Url::parse(&as_base).unwrap_or(base.clone());
    let as_origin = {
        let scheme = as_url.scheme();
        let host = as_url.host_str().unwrap_or_default();
        match as_url.port() {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        }
    };
    let as_path = as_url.path().trim_end_matches('/');

    // 2. Authorization Server Metadata (RFC 8414) then OpenID config.
    let mut metadata_doc: Option<Value> = None;
    let mut candidates = well_known_urls(&as_origin, as_path, "oauth-authorization-server");
    candidates.extend(well_known_urls(&as_origin, as_path, "openid-configuration"));
    for candidate in candidates {
        if let Some(doc) = fetch_json(http, &candidate).await {
            if doc.get("authorization_endpoint").is_some() && doc.get("token_endpoint").is_some() {
                metadata_doc = Some(doc);
                break;
            }
        }
    }

    let (
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        advertised_scopes,
        token_endpoint_auth_methods_supported,
    ) = match metadata_doc {
        Some(doc) => (
            doc.get("authorization_endpoint")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    anyhow!("authorization server metadata missing authorization_endpoint")
                })?,
            doc.get("token_endpoint")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow!("authorization server metadata missing token_endpoint"))?,
            doc.get("registration_endpoint")
                .and_then(Value::as_str)
                .map(str::to_string),
            doc.get("scopes_supported")
                .and_then(json_string_array)
                .unwrap_or_default(),
            doc.get("token_endpoint_auth_methods_supported")
                .and_then(json_string_array)
                .unwrap_or_default(),
        ),
        // Fallback: conventional endpoints on the authorization server origin.
        None => (
            format!("{as_origin}/authorize"),
            format!("{as_origin}/token"),
            Some(format!("{as_origin}/register")),
            Vec::new(),
            Vec::new(),
        ),
    };

    let scopes_supported = scopes_to_request(
        challenge.scope.as_deref(),
        resource_scopes_supported,
        advertised_scopes,
    );

    Ok(OAuthMetadata {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scopes_supported,
        token_endpoint_auth_methods_supported,
        resource,
    })
}

/// Register a public client via Dynamic Client Registration (RFC 7591).
pub async fn register_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
    client_name: &str,
    token_endpoint_auth_methods_supported: &[String],
) -> Result<RegisteredClient> {
    let token_endpoint_auth_method = select_token_endpoint_auth_method(
        token_endpoint_auth_methods_supported,
        // DCR can issue a secret when we request a confidential method.
        token_endpoint_auth_methods_supported
            .iter()
            .any(|method| method == "client_secret_post" || method == "client_secret_basic"),
    );
    let body = json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": token_endpoint_auth_method.as_deref().unwrap_or("none"),
        "application_type": "native"
    });

    let resp = http
        .post(registration_endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .context("dynamic client registration request failed")?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("dynamic client registration failed ({status}): {text}");
    }

    let doc: Value =
        serde_json::from_str(&text).context("dynamic client registration returned invalid JSON")?;
    let client_id = doc
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("registration response missing client_id"))?;
    let client_secret = doc
        .get("client_secret")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(RegisteredClient {
        client_id,
        client_secret,
        token_endpoint_auth_method,
    })
}

/// Build the browser authorization URL (authorization-code + PKCE).
pub fn build_authorize_url(
    meta: &OAuthMetadata,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    scope_override: Option<&str>,
) -> String {
    let mut url = url::Url::parse(&meta.authorization_endpoint)
        .unwrap_or_else(|_| url::Url::parse("http://invalid.local/").unwrap());
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", client_id);
        q.append_pair("redirect_uri", redirect_uri);
        q.append_pair("code_challenge", challenge);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("state", state);
        q.append_pair("resource", &meta.resource);
        if let Some(scope) = scope_override.filter(|scope| !scope.trim().is_empty()) {
            q.append_pair("scope", scope.trim());
        } else if !meta.scopes_supported.is_empty() {
            q.append_pair("scope", &meta.scopes_supported.join(" "));
        }
    }
    url.to_string()
}

/// Exchange an authorization code for tokens.
#[allow(clippy::too_many_arguments)]
pub async fn exchange_code(
    http: &reqwest::Client,
    meta: &OAuthMetadata,
    client_id: &str,
    client_secret: Option<&str>,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    token_endpoint_auth_method: Option<&str>,
) -> Result<McpOAuthRecord> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
        ("client_id", client_id.to_string()),
        ("code_verifier", verifier.to_string()),
        ("resource", meta.resource.clone()),
    ];

    let resp = apply_client_auth(
        http.post(&meta.token_endpoint)
            .header("Accept", "application/json"),
        &mut form,
        client_id,
        client_secret,
        token_endpoint_auth_method,
    )
    .form(&form)
    .send()
    .await
    .context("token exchange request failed")?;

    let record = parse_token_response(
        resp,
        &meta.token_endpoint,
        client_id,
        client_secret,
        token_endpoint_auth_method,
        Some(meta.resource.clone()),
        None,
    )
    .await?;
    Ok(record)
}

/// Refresh an access token using the stored refresh token.
pub async fn refresh(http: &reqwest::Client, record: &McpOAuthRecord) -> Result<McpOAuthRecord> {
    let refresh_token = record
        .refresh_token
        .as_deref()
        .ok_or_else(|| anyhow!("no refresh token available; re-authentication required"))?;

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
        ("client_id", record.client_id.clone()),
    ];
    if let Some(resource) = &record.resource {
        form.push(("resource", resource.clone()));
    }

    let resp = apply_client_auth(
        http.post(&record.token_endpoint)
            .header("Accept", "application/json"),
        &mut form,
        &record.client_id,
        record.client_secret.as_deref(),
        record.token_endpoint_auth_method.as_deref(),
    )
    .form(&form)
    .send()
    .await
    .context("token refresh request failed")?;

    parse_token_response(
        resp,
        &record.token_endpoint,
        &record.client_id,
        record.client_secret.as_deref(),
        record.token_endpoint_auth_method.as_deref(),
        record.resource.clone(),
        // keep the old refresh token if the server doesn't rotate it
        record.refresh_token.clone(),
    )
    .await
}

async fn parse_token_response(
    resp: reqwest::Response,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    token_endpoint_auth_method: Option<&str>,
    resource: Option<String>,
    fallback_refresh: Option<String>,
) -> Result<McpOAuthRecord> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("OAuth token endpoint returned {status}: {text}");
    }
    let doc: Value =
        serde_json::from_str(&text).context("OAuth token response was not valid JSON")?;

    let access_token = doc
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("token response missing access_token"))?;
    let refresh_token = doc
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(fallback_refresh);
    let expires_at_ms = doc
        .get("expires_in")
        .and_then(Value::as_i64)
        .map(|secs| now_ms() + secs * 1000);
    let scope = doc.get("scope").and_then(Value::as_str).map(str::to_string);

    Ok(McpOAuthRecord {
        access_token,
        refresh_token,
        expires_at_ms,
        token_endpoint: token_endpoint.to_string(),
        client_id: client_id.to_string(),
        client_secret: client_secret.map(str::to_string),
        token_endpoint_auth_method: token_endpoint_auth_method.map(str::to_string),
        scope,
        resource,
    })
}

/// Candidate well-known URLs. The spec allows the path-aware form (e.g.
/// `/.well-known/oauth-protected-resource/mcp`) and the plain origin form.
fn well_known_urls(origin: &str, path: &str, suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let path = path.trim_start_matches('/').trim_end_matches('/');
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{suffix}/{path}"));
    }
    out.push(format!("{origin}/.well-known/{suffix}"));
    out
}

async fn fetch_json(http: &reqwest::Client, url: &str) -> Option<Value> {
    let resp = http
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok()
}

fn json_string_array(value: &Value) -> Option<Vec<String>> {
    value.as_array().map(|arr| {
        arr.iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    })
}

fn select_token_endpoint_auth_method(
    supported: &[String],
    has_or_wants_secret: bool,
) -> Option<String> {
    let supports = |name: &str| supported.iter().any(|method| method == name);
    if has_or_wants_secret {
        if supports("client_secret_post") {
            return Some("client_secret_post".to_string());
        }
        if supports("client_secret_basic") {
            return Some("client_secret_basic".to_string());
        }
    }
    if supported.is_empty() || supports("none") {
        Some("none".to_string())
    } else {
        supported.first().cloned()
    }
}

fn apply_client_auth(
    request: reqwest::RequestBuilder,
    form: &mut Vec<(&str, String)>,
    client_id: &str,
    client_secret: Option<&str>,
    method: Option<&str>,
) -> reqwest::RequestBuilder {
    match (method, client_secret) {
        (Some("client_secret_basic"), Some(secret)) => request.basic_auth(client_id, Some(secret)),
        (Some("client_secret_post"), Some(secret)) | (None, Some(secret)) => {
            form.push(("client_secret", secret.to_string()));
            request
        }
        _ => request,
    }
}

static TOKEN_STORE: std::sync::OnceLock<AppStore> = std::sync::OnceLock::new();

/// Lets MCP calls renew a rejected OAuth token mid-turn. Set once at startup.
pub fn install_token_store(store: AppStore) {
    let _ = TOKEN_STORE.set(store);
}

/// One refresh at a time across the whole app. Many servers rotate refresh
/// tokens: two callers refreshing with the same one get the second rejected,
/// and some servers then revoke the whole session. Each caller reads the
/// record inside the lock, so it sees what the previous one just saved.
fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// The access token for `server_id`, refreshed if it is about to expire.
/// `None` when the server has no OAuth sign-in.
pub async fn current_access_token(store: &AppStore, server_id: &str) -> Result<Option<String>> {
    token_for(store, server_id, None).await
}

/// A new access token after the server rejected `rejected`. `None` when
/// there is no OAuth sign-in to renew.
pub async fn renew_rejected_token(server_id: &str, rejected: &str) -> Result<Option<String>> {
    let Some(store) = TOKEN_STORE.get() else {
        return Ok(None);
    };
    token_for(store, server_id, Some(rejected)).await
}

async fn token_for(
    store: &AppStore,
    server_id: &str,
    rejected: Option<&str>,
) -> Result<Option<String>> {
    let _guard = refresh_lock().lock().await;
    let Some(record) = store.load_mcp_oauth_tokens()?.remove(server_id) else {
        return Ok(None);
    };
    match rejected {
        // Someone else already replaced the rejected token.
        Some(rejected) if record.access_token != rejected => {
            return Ok(Some(record.access_token));
        }
        None if !record.needs_refresh() => return Ok(Some(record.access_token)),
        _ => {}
    }
    if record.refresh_token.is_none() {
        return match rejected {
            Some(_) => Err(anyhow!(SIGN_IN_EXPIRED)),
            None => Ok(Some(record.access_token)),
        };
    }
    let http = reqwest::Client::builder()
        .user_agent("sinew/0.1")
        .timeout(REFRESH_TIMEOUT)
        .build()
        .context("unable to build OAuth client")?;
    match refresh(&http, &record).await {
        Ok(refreshed) => {
            store.save_mcp_oauth_token(server_id, &refreshed)?;
            Ok(Some(refreshed.access_token))
        }
        Err(err) => {
            // Another Sinew process may have used this refresh token first.
            if let Some(latest) = store.load_mcp_oauth_tokens()?.remove(server_id) {
                if latest.refresh_token != record.refresh_token {
                    return Ok(Some(latest.access_token));
                }
            }
            if refresh_was_refused(&err.to_string()) {
                return Err(anyhow!(SIGN_IN_EXPIRED));
            }
            match rejected {
                Some(_) => Err(err),
                // A network blip: the token may still work.
                None => Ok(Some(record.access_token)),
            }
        }
    }
}

/// The server said no, as opposed to being unreachable.
fn refresh_was_refused(error: &str) -> bool {
    error.contains("invalid_grant")
        || error.contains("returned 400")
        || error.contains("returned 401")
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_challenge_parameters() {
        let challenge = parse_auth_challenge(
            "Bearer realm=\"OAuth\", resource_metadata=\"https://x.dev/.well-known/oauth-protected-resource/mcp\", scope=\"user:read account:read\"",
        );
        assert_eq!(
            challenge.resource_metadata.as_deref(),
            Some("https://x.dev/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(challenge.scope.as_deref(), Some("user:read account:read"));
        assert_eq!(challenge.auth_server, None);
        let comma = parse_auth_challenge("Bearer error=\"a, b\", scope=\"c\"");
        assert_eq!(comma.scope.as_deref(), Some("c"));
    }

    #[test]
    fn asks_for_the_scopes_the_server_needs_not_all_it_has() {
        let advertised: Vec<String> = (0..403)
            .map(|i| format!("scope-number-{i}.read"))
            .chain([OFFLINE_ACCESS.to_string()])
            .collect();
        assert_eq!(
            scopes_to_request(Some("user:read account:read"), vec!["x".into()], advertised.clone()),
            ["user:read", "account:read", OFFLINE_ACCESS]
        );
        assert_eq!(scopes_to_request(None, vec!["mcp".into()], vec![]), ["mcp"]);
        // Nothing narrower and too long for a URL: let the server decide.
        assert_eq!(scopes_to_request(None, vec![], advertised), [OFFLINE_ACCESS]);
    }

    /// Against the real servers; signs nothing in.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn real_servers_give_a_url_short_enough_to_open() {
        let http = reqwest::Client::new();
        for server in ["https://mcp.cloudflare.com/mcp", "https://10kdesign.xyz/api/mcp"] {
            let meta = discover(&http, server).await.unwrap();
            let url = build_authorize_url(&meta, "client", "http://localhost:1/callback", "c", "s", None);
            eprintln!("{server}: {} chars, scopes {:?}", url.len(), meta.scopes_supported);
            assert!(url.len() < 4096);
        }
    }

    #[test]
    fn tells_a_refused_refresh_from_a_network_failure() {
        assert!(refresh_was_refused(
            "OAuth token endpoint returned 400 Bad Request: {\"error\":\"invalid_grant\"}"
        ));
        assert!(!refresh_was_refused("token refresh request failed"));
        assert!(!refresh_was_refused("OAuth token endpoint returned 503 Service Unavailable: "));
    }
}
