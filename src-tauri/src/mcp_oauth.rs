use crate::*;

use sinew_app::{
    mcp_oauth_authorize_url, mcp_oauth_discover, mcp_oauth_exchange_code, mcp_oauth_generate_pkce,
    mcp_oauth_generate_state, mcp_oauth_refresh, mcp_oauth_register_client, McpOAuthMetadata,
    McpOAuthRecord, McpPkcePair, McpRegisteredClient, McpServerConfig, McpSettings, McpTransport,
};

const MCP_CLIENT_NAME: &str = "Sinew";

/// Inject fresh `Authorization: Bearer` headers into every HTTP MCP server
/// that has stored OAuth tokens, refreshing expired access tokens first.
/// Used everywhere MCP settings are loaded for actual use (probe, turns).
pub(super) async fn resolve_mcp_oauth_settings(
    store: &AppStore,
    mut settings: McpSettings,
) -> McpSettings {
    let tokens = match store.load_mcp_oauth_tokens() {
        Ok(tokens) => tokens,
        Err(_) => return settings,
    };
    if tokens.is_empty() {
        return settings;
    }

    let http = match reqwest::Client::builder().user_agent("sinew/0.1").build() {
        Ok(client) => client,
        Err(_) => return settings,
    };

    for server in &mut settings.servers {
        let is_remote = server
            .url
            .as_deref()
            .map(str::trim)
            .is_some_and(|u| !u.is_empty())
            || matches!(server.transport, McpTransport::Http | McpTransport::Sse);
        if !is_remote {
            continue;
        }
        let Some(record) = tokens.get(&server.id) else {
            continue;
        };

        let mut record = record.clone();
        if record.needs_refresh() && record.refresh_token.is_some() {
            match mcp_oauth_refresh(&http, &record).await {
                Ok(refreshed) => {
                    let _ = store.save_mcp_oauth_token(&server.id, &refreshed);
                    record = refreshed;
                }
                Err(err) => {
                    tracing::warn!("MCP OAuth refresh failed for {}: {err}", server.name);
                }
            }
        }

        set_bearer_header(server, &record.access_token);
    }

    settings
}

fn set_bearer_header(server: &mut McpServerConfig, access_token: &str) {
    server
        .headers
        .retain(|h| !h.key.trim().eq_ignore_ascii_case("authorization"));
    server.headers.push(sinew_app::mcp::McpEnvVar {
        key: "Authorization".to_string(),
        value: format!("Bearer {access_token}"),
    });
}

pub(super) async fn bind_mcp_oauth_listener() -> Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("unable to bind MCP OAuth callback port")
}

#[tauri::command]
pub(super) async fn start_mcp_oauth_login(
    state: State<'_, DesktopState>,
    server_id: String,
) -> std::result::Result<StartMcpLoginOutput, String> {
    // cancel any in-flight attempt
    if let Some(existing) = state.mcp_login.lock().await.take() {
        existing.cancel.notify_one();
    }

    let settings = state.store.load_mcp_settings().map_err(error_to_string)?;
    let server = settings
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .cloned()
        .ok_or_else(|| format!("MCP server `{server_id}` not found"))?;
    let server_url = server
        .url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            format!(
                "MCP server `{}` is not a remote HTTP/SSE server",
                server.name
            )
        })?
        .to_string();

    let http = reqwest::Client::builder()
        .user_agent("sinew/0.1")
        .build()
        .map_err(error_to_string)?;

    // 1. Discover authorization server + endpoints.
    let mut metadata = mcp_oauth_discover(&http, &server_url)
        .await
        .map_err(error_to_string)?;
    apply_oauth_overrides(&mut metadata, &server);

    // 2. Bind loopback, build redirect URI.
    let listener = bind_mcp_oauth_listener().await.map_err(error_to_string)?;
    let port = listener.local_addr().map_err(error_to_string)?.port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    // 3. Resolve OAuth client. Prefer an explicit pre-registered client from
    // config; otherwise use MCP dynamic client registration.
    let registered = resolve_oauth_client(&http, &metadata, &server, &redirect_uri)
        .await
        .map_err(error_to_string)?;

    // 4. PKCE + state, build the browser URL.
    let pkce = mcp_oauth_generate_pkce();
    let oauth_state = mcp_oauth_generate_state();
    let auth_url = mcp_oauth_authorize_url(
        &metadata,
        &registered.client_id,
        &redirect_uri,
        &pkce.challenge,
        &oauth_state,
        server
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.scope.as_deref()),
    );

    let login_id = mcp_oauth_generate_state();
    let cancel = Arc::new(Notify::new());
    let outcome = Arc::new(StdMutex::new(None));

    {
        let mut active = state.mcp_login.lock().await;
        *active = Some(McpLoginAttempt {
            id: login_id.clone(),
            server_id: server_id.clone(),
            cancel: cancel.clone(),
            outcome: outcome.clone(),
        });
    }

    let store = state.store.clone();
    let client_secret = registered.client_secret.clone();
    let token_endpoint_auth_method = registered.token_endpoint_auth_method.clone();
    let client_id = registered.client_id.clone();
    tauri::async_runtime::spawn(async move {
        let result = run_mcp_oauth_server(McpOAuthServerCtx {
            http,
            listener,
            redirect_uri,
            expected_state: oauth_state,
            pkce,
            metadata,
            client_id,
            client_secret,
            token_endpoint_auth_method,
            server_id,
            store,
            cancel,
        })
        .await;
        let login_outcome = match result {
            Ok(()) => McpLoginOutcome {
                success: true,
                error: None,
            },
            Err(err) => McpLoginOutcome {
                success: false,
                error: Some(err.to_string()),
            },
        };
        if let Ok(mut slot) = outcome.lock() {
            *slot = Some(login_outcome);
        }
    });

    Ok(StartMcpLoginOutput { login_id, auth_url })
}

#[tauri::command]
pub(super) async fn poll_mcp_oauth_login(
    state: State<'_, DesktopState>,
) -> std::result::Result<McpLoginStatus, String> {
    let mut active = state.mcp_login.lock().await;
    let Some(attempt) = active.clone() else {
        return Ok(McpLoginStatus {
            pending: false,
            success: false,
            error: None,
        });
    };

    let outcome = attempt
        .outcome
        .lock()
        .map_err(|_| "login state is unavailable".to_string())?
        .clone();

    match outcome {
        Some(outcome) => {
            *active = None;
            Ok(McpLoginStatus {
                pending: false,
                success: outcome.success,
                error: outcome.error,
            })
        }
        None => Ok(McpLoginStatus {
            pending: true,
            success: false,
            error: None,
        }),
    }
}

#[tauri::command]
pub(super) async fn cancel_mcp_oauth_login(
    state: State<'_, DesktopState>,
) -> std::result::Result<(), String> {
    if let Some(attempt) = state.mcp_login.lock().await.take() {
        attempt.cancel.notify_one();
    }
    Ok(())
}

#[tauri::command]
pub(super) async fn disconnect_mcp_oauth(
    state: State<'_, DesktopState>,
    server_id: String,
) -> std::result::Result<(), String> {
    if let Some(attempt) = state.mcp_login.lock().await.take() {
        attempt.cancel.notify_one();
    }
    state
        .store
        .delete_mcp_oauth_token(&server_id)
        .map_err(error_to_string)?;
    // Also strip any stored Authorization header from the server config.
    let mut settings = state.store.load_mcp_settings().map_err(error_to_string)?;
    if let Some(server) = settings.servers.iter_mut().find(|s| s.id == server_id) {
        server
            .headers
            .retain(|h| !h.key.trim().eq_ignore_ascii_case("authorization"));
    }
    state
        .store
        .save_mcp_settings(&settings)
        .map_err(error_to_string)?;
    Ok(())
}

struct McpOAuthServerCtx {
    http: reqwest::Client,
    listener: tokio::net::TcpListener,
    redirect_uri: String,
    expected_state: String,
    pkce: McpPkcePair,
    metadata: McpOAuthMetadata,
    client_id: String,
    client_secret: Option<String>,
    token_endpoint_auth_method: Option<String>,
    server_id: String,
    store: AppStore,
    cancel: Arc<Notify>,
}

async fn run_mcp_oauth_server(ctx: McpOAuthServerCtx) -> Result<()> {
    let McpOAuthServerCtx {
        http,
        listener,
        redirect_uri,
        expected_state,
        pkce,
        metadata,
        client_id,
        client_secret,
        token_endpoint_auth_method,
        server_id,
        store,
        cancel,
    } = ctx;

    loop {
        tokio::select! {
            _ = cancel.notified() => {
                anyhow::bail!("Login canceled");
            }
            accepted = listener.accept() => {
                let (mut stream, _) = accepted.context("OAuth callback accept failed")?;
                if let Some(result) = handle_mcp_oauth_request(
                    &http,
                    &mut stream,
                    &redirect_uri,
                    &expected_state,
                    &pkce,
                    &metadata,
                    &client_id,
                    client_secret.as_deref(),
                    token_endpoint_auth_method.as_deref(),
                    &server_id,
                    &store,
                ).await? {
                    return result;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_mcp_oauth_request(
    http: &reqwest::Client,
    stream: &mut tokio::net::TcpStream,
    redirect_uri: &str,
    expected_state: &str,
    pkce: &McpPkcePair,
    metadata: &McpOAuthMetadata,
    client_id: &str,
    client_secret: Option<&str>,
    token_endpoint_auth_method: Option<&str>,
    server_id: &str,
    store: &AppStore,
) -> Result<Option<Result<()>>> {
    let mut buffer = [0u8; 8192];
    let read = stream
        .read(&mut buffer)
        .await
        .context("OAuth callback read failed")?;
    if read == 0 {
        return Ok(None);
    }

    let request = String::from_utf8_lossy(&buffer[..read]);
    let Some(first_line) = request.lines().next() else {
        write_http_response(stream, 400, "Bad Request", "Bad Request").await?;
        return Ok(None);
    };
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        write_http_response(stream, 405, "Method Not Allowed", "Method Not Allowed").await?;
        return Ok(None);
    }

    let parsed = parse_local_oauth_url(target)?;
    if parsed.path() != "/callback" {
        write_http_response(stream, 404, "Not Found", "Not Found").await?;
        return Ok(None);
    }

    let params = parsed
        .query_pairs()
        .into_owned()
        .collect::<HashMap<String, String>>();

    if params.get("state").map(String::as_str) != Some(expected_state) {
        write_html_response(stream, 400, mcp_login_error_html("State mismatch")).await?;
        return Ok(Some(Err(anyhow::anyhow!("State mismatch"))));
    }
    if let Some(error) = params.get("error") {
        let message = params
            .get("error_description")
            .filter(|v| !v.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| error.clone());
        write_html_response(stream, 400, mcp_login_error_html(&message)).await?;
        return Ok(Some(Err(anyhow::anyhow!(message))));
    }
    let Some(code) = params.get("code").filter(|v| !v.is_empty()) else {
        write_html_response(
            stream,
            400,
            mcp_login_error_html("Missing authorization code"),
        )
        .await?;
        return Ok(Some(Err(anyhow::anyhow!("Missing authorization code"))));
    };

    match mcp_oauth_exchange_code(
        http,
        metadata,
        client_id,
        client_secret,
        code,
        &pkce.verifier,
        redirect_uri,
        token_endpoint_auth_method,
    )
    .await
    {
        Ok(record) => {
            if let Err(err) = persist_mcp_oauth(store, server_id, &record) {
                let message = err.to_string();
                write_html_response(stream, 500, mcp_login_error_html(&message)).await?;
                return Ok(Some(Err(anyhow::anyhow!(message))));
            }
            write_html_response(stream, 200, mcp_login_success_html()).await?;
            Ok(Some(Ok(())))
        }
        Err(err) => {
            let message = err.to_string();
            write_html_response(stream, 500, mcp_login_error_html(&message)).await?;
            Ok(Some(Err(anyhow::anyhow!(message))))
        }
    }
}

async fn resolve_oauth_client(
    http: &reqwest::Client,
    metadata: &McpOAuthMetadata,
    server: &McpServerConfig,
    redirect_uri: &str,
) -> Result<McpRegisteredClient> {
    if let Some(client_id) = server
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let oauth = server.oauth.as_ref();
        return Ok(McpRegisteredClient {
            client_id: client_id.to_string(),
            client_secret: oauth.and_then(|oauth| oauth.client_secret.clone()),
            token_endpoint_auth_method: oauth
                .and_then(|oauth| oauth.token_endpoint_auth_method.clone())
                .or_else(|| {
                    infer_token_endpoint_auth_method(
                        metadata,
                        oauth.and_then(|o| o.client_secret.as_deref()),
                    )
                }),
        });
    }

    let Some(registration_endpoint) = metadata.registration_endpoint.clone() else {
        anyhow::bail!(
            "authorization server does not advertise dynamic client registration; add oauth.clientId/clientSecret to this MCP server config"
        );
    };

    mcp_oauth_register_client(
        http,
        &registration_endpoint,
        redirect_uri,
        MCP_CLIENT_NAME,
        &metadata.token_endpoint_auth_methods_supported,
    )
    .await
    .map_err(|err| {
        anyhow::anyhow!(
            "{err}. This server may only allow approved MCP clients or may require a pre-registered OAuth client; add oauth.clientId/clientSecret if you have one."
        )
    })
}

fn apply_oauth_overrides(metadata: &mut McpOAuthMetadata, server: &McpServerConfig) {
    let Some(oauth) = server.oauth.as_ref() else {
        return;
    };
    if let Some(value) = oauth
        .authorization_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        metadata.authorization_endpoint = value.to_string();
    }
    if let Some(value) = oauth
        .token_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        metadata.token_endpoint = value.to_string();
    }
    if let Some(value) = oauth
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        metadata.resource = value.to_string();
    }
}

fn infer_token_endpoint_auth_method(
    metadata: &McpOAuthMetadata,
    client_secret: Option<&str>,
) -> Option<String> {
    let has_secret = client_secret.is_some_and(|secret| !secret.trim().is_empty());
    if has_secret {
        if metadata
            .token_endpoint_auth_methods_supported
            .iter()
            .any(|method| method == "client_secret_post")
        {
            return Some("client_secret_post".to_string());
        }
        if metadata
            .token_endpoint_auth_methods_supported
            .iter()
            .any(|method| method == "client_secret_basic")
        {
            return Some("client_secret_basic".to_string());
        }
    }
    None
}

fn persist_mcp_oauth(
    store: &AppStore,
    server_id: &str,
    record: &McpOAuthRecord,
) -> std::result::Result<(), String> {
    store
        .save_mcp_oauth_token(server_id, record)
        .map_err(error_to_string)?;
    // Mirror the access token into the server config headers so the
    // existing HTTP transport authenticates without a resolution step.
    let mut settings = store.load_mcp_settings().map_err(error_to_string)?;
    if let Some(server) = settings.servers.iter_mut().find(|s| s.id == server_id) {
        set_bearer_header(server, &record.access_token);
    }
    store
        .save_mcp_settings(&settings)
        .map_err(error_to_string)?;
    Ok(())
}

fn mcp_login_success_html() -> String {
    r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Sinew connected</title>
    <style>
      body{margin:0;min-height:100vh;display:grid;place-items:center;background:#0a0b0d;color:#f4f4f5;font:15px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}
      main{max-width:420px;padding:32px;text-align:center}
      h1{font-size:22px;margin:0 0 10px}
      p{margin:0;color:#a1a1aa;line-height:1.5}
    </style>
  </head>
  <body><main><h1>MCP server connected</h1><p>You can close this tab and return to Sinew.</p></main></body>
</html>"#
        .to_string()
}

fn mcp_login_error_html(message: &str) -> String {
    let escaped = html_escape(message);
    format!(
        r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>Sinew connection failed</title>
    <style>
      body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0a0b0d;color:#f4f4f5;font:15px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}}
      main{{max-width:480px;padding:32px;text-align:center}}
      h1{{font-size:22px;margin:0 0 10px}}
      p{{margin:0;color:#a1a1aa;line-height:1.5;overflow-wrap:anywhere}}
    </style>
  </head>
  <body><main><h1>Connection failed</h1><p>{escaped}</p></main></body>
</html>"#
    )
}
