//! MCP OAuth 2.0 discovery and PKCE flow.
//!
//! Implements the MCP spec (2025-03-26) "Authorization" section:
//!   1. Probe the MCP endpoint — if it returns 401, read `WWW-Authenticate`.
//!   2. Fetch `/.well-known/oauth-protected-resource` → find the authorization server.
//!   3. Fetch `{auth_server}/.well-known/oauth-authorization-server` → get endpoints.
//!   4. Run PKCE authorization code flow (open browser, receive callback).
//!   5. Return the token JSON for storage.

use crate::process_ext::NoWindow;
use anyhow::{anyhow, Context};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ── Probe ─────────────────────────────────────────────────────────────────────

pub enum ProbeResult {
    /// Server responded without requiring authentication.
    Accessible,
    /// Server returned 401. Contains the raw `WWW-Authenticate` header value.
    NeedsOAuth(String),
    /// Server returned 404/405 — does not accept Streamable HTTP POST.
    /// Try the legacy SSE transport (GET /sse + POST /message).
    LegacySse,
}

/// Probe the MCP endpoint to determine transport type and auth requirements.
///
/// Sends a proper JSON-RPC `initialize` request via POST (Streamable HTTP).
///   200/202/400 → Streamable HTTP, no auth needed
///   401         → needs OAuth, Streamable HTTP transport
///   404/405     → server doesn't accept POST — likely legacy SSE transport
pub async fn probe_for_auth(url: &str) -> anyhow::Result<ProbeResult> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    // Send a proper initialize request so well-behaved servers can respond correctly.
    let init_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "dragent", "version": env!("CARGO_PKG_VERSION") }
        }
    });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .context("MCP endpoint probe failed")?;

    match resp.status() {
        s if s == reqwest::StatusCode::UNAUTHORIZED => {
            let www_auth = resp
                .headers()
                .get("WWW-Authenticate")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            Ok(ProbeResult::NeedsOAuth(www_auth))
        }
        s if s == reqwest::StatusCode::NOT_FOUND
            || s == reqwest::StatusCode::METHOD_NOT_ALLOWED =>
        {
            Ok(ProbeResult::LegacySse)
        }
        _ => Ok(ProbeResult::Accessible),
    }
}

// ── Discovery ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OAuthMeta {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub scopes_supported: Vec<String>,
    /// RFC 7591 dynamic client registration endpoint (MCP servers MUST support this).
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    /// The authorization server issuer URL (scheme+host). Not part of the JSON
    /// response — populated by `discover_oauth_meta` from the discovery URL.
    #[serde(skip)]
    pub issuer: String,
}

/// Discover the OAuth authorization server metadata from a 401 response.
///
/// Discovery order (per MCP spec + RFC 9728):
///   1. `resource_metadata` param in `WWW-Authenticate` → fetch protected-resource doc
///   2. Fall back to `{origin}/.well-known/oauth-protected-resource`
///   3. From the protected-resource doc, read `authorization_servers[0]`
///   4. Fetch `{auth_server}/.well-known/oauth-authorization-server`
///   5. Fall back to `{auth_server}/.well-known/openid-configuration`
pub async fn discover_oauth_meta(server_url: &str, www_auth: &str) -> anyhow::Result<OAuthMeta> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    // Step 1: locate the protected-resource metadata document.
    let resource_meta_url =
        extract_bearer_param(www_auth, "resource_metadata").unwrap_or_else(|| {
            format!(
                "{}/.well-known/oauth-protected-resource",
                origin_of(server_url)
            )
        });

    // Step 2: fetch it to find the authorization server URL.
    let auth_server = match client.get(&resource_meta_url).send().await {
        Ok(r) if r.status().is_success() => {
            #[derive(Deserialize)]
            struct ProtectedResource {
                authorization_servers: Vec<String>,
            }
            match r.json::<ProtectedResource>().await {
                Ok(doc) => doc
                    .authorization_servers
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| origin_of(server_url)),
                Err(_) => origin_of(server_url),
            }
        }
        _ => origin_of(server_url),
    };

    log::info!("[mcp oauth] authorization server: {auth_server}");

    // Step 3: fetch the authorization server metadata.
    // Per MCP spec: if the AS URL has a path component, the metadata URL is
    // {origin}/.well-known/oauth-authorization-server{path}, not {origin}/.well-known/...
    let issuer = origin_of(&auth_server);
    for meta_url in as_metadata_urls(&auth_server) {
        if let Ok(r) = client.get(&meta_url).send().await {
            if r.status().is_success() {
                if let Ok(mut meta) = r.json::<OAuthMeta>().await {
                    log::info!("[mcp oauth] authorization server metadata from {meta_url}");
                    meta.issuer = issuer;
                    return Ok(meta);
                }
            }
        }
    }

    // Step 4: fall back to OpenID Connect discovery document.
    let oidc_url = format!("{auth_server}/.well-known/openid-configuration");
    let r = client
        .get(&oidc_url)
        .send()
        .await
        .context("could not reach OAuth metadata endpoint")?;
    let mut meta: OAuthMeta = r.json().await.context("invalid OAuth metadata response")?;
    meta.issuer = issuer;
    Ok(meta)
}

// ── PKCE flow ─────────────────────────────────────────────────────────────────

/// Non-blocking OAuth start: registers the client, builds the auth URL, opens the
/// browser, and stores the PKCE session in the global pending registry.
///
/// The token exchange completes later when the OS delivers this edition's
/// deep-link callback
/// (`<scheme>://anysphere.dragent-mcp/oauth/callback?code=…&state=…`).
pub async fn begin_mcp_oauth_flow(server_name: &str, meta: &OAuthMeta) -> anyhow::Result<()> {
    use super::oauth_pending::{mcp_redirect_uri, McpPkceSession};

    let redirect_uri = mcp_redirect_uri();
    let verifier = pkce_verifier();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = pkce_verifier();

    // 1. Static config: pre-registered credentials for servers that don't support open DCR.
    let client_id = if let Some(static_client) = super::oauth_static::find_by_issuer(&meta.issuer) {
        log::info!(
            "[mcp oauth] using pre-registered client_id for issuer '{}'",
            meta.issuer
        );
        static_client.client_id.to_string()
    // 2. Dynamic Client Registration (RFC 7591): servers that support open registration.
    } else if let Some(reg_ep) = &meta.registration_endpoint {
        match register_client(reg_ep, &redirect_uri).await {
            Ok(id) => {
                log::info!("[mcp oauth] dynamic registration succeeded: client_id={id}");
                id
            }
            Err(e) => {
                let msg = e.to_string();
                // 403 means the server allowlists clients — Dragent needs to be registered.
                if msg.contains("403") || msg.contains("Forbidden") {
                    anyhow::bail!(
                        "This server restricts OAuth access to approved clients. \
                         Dragent is not yet on the allowlist for '{}'. \
                         Apply at the provider's developer portal.",
                        meta.issuer
                    );
                }
                log::warn!("[mcp oauth] dynamic registration failed: {e} — using 'dragent'");
                "dragent".to_string()
            }
        }
    // 3. No registration mechanism at all — use app name as client_id (works for permissive servers).
    } else {
        log::info!("[mcp oauth] no registration_endpoint — using 'dragent' as client_id");
        "dragent".to_string()
    };

    let scope_str = meta.scopes_supported.join(" ");
    let scope_param = if scope_str.is_empty() {
        String::new()
    } else {
        format!("&scope={}", pct_encode(&scope_str))
    };

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}{}",
        meta.authorization_endpoint,
        pct_encode(&client_id),
        pct_encode(&redirect_uri),
        challenge,
        pct_encode(&state),
        scope_param,
    );

    super::oauth_pending::store(
        state,
        McpPkceSession {
            server_name: server_name.to_string(),
            verifier,
            client_id,
            redirect_uri,
            token_endpoint: meta.token_endpoint.clone(),
        },
    );

    log::info!("[mcp oauth] '{server_name}' — opening browser for OAuth");
    open_browser(&auth_url);
    Ok(())
}

/// Run a PKCE authorization-code flow and return the token JSON object.
///
/// Opens the user's browser, binds a local callback server on an ephemeral port,
/// waits up to 120 s for the redirect, then exchanges the code for tokens.
///
/// If the server's metadata includes a `registration_endpoint` (RFC 7591 Dynamic
/// Client Registration), we register first to obtain a valid `client_id`. This is
/// required by the MCP spec and fixes "invalid_client" errors from servers like
/// expo-mcp that don't accept arbitrary client IDs.
pub async fn run_pkce_flow(
    server_name: &str,
    meta: &OAuthMeta,
) -> anyhow::Result<serde_json::Value> {
    // 1. PKCE verifier + challenge
    let verifier = pkce_verifier();
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));

    // 2. CSRF state (RFC 6749 §10.12)
    let state = pkce_verifier();

    // 3. Ephemeral local redirect listener
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("could not bind OAuth callback listener")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 4. Obtain a client_id — via dynamic registration (RFC 7591) when available,
    //    otherwise fall back to "dragent" (works for servers that accept any id).
    let client_id = if let Some(reg_ep) = &meta.registration_endpoint {
        match register_client(reg_ep, &redirect_uri).await {
            Ok(id) => {
                log::info!("[mcp oauth] dynamic registration succeeded: client_id={id}");
                id
            }
            Err(e) => {
                log::warn!(
                    "[mcp oauth] dynamic registration failed: {e} — falling back to 'dragent'"
                );
                "dragent".to_string()
            }
        }
    } else {
        log::info!("[mcp oauth] no registration_endpoint — using client_id 'dragent'");
        "dragent".to_string()
    };

    // 5. Build authorization URL with PKCE + CSRF state.
    let scope_str = meta.scopes_supported.join(" ");
    let scope_param = if scope_str.is_empty() {
        String::new()
    } else {
        format!("&scope={}", pct_encode(&scope_str))
    };
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}{}",
        meta.authorization_endpoint,
        pct_encode(&client_id),
        pct_encode(&redirect_uri),
        challenge,
        pct_encode(&state),
        scope_param,
    );

    log::info!("[mcp oauth] opening browser for '{server_name}': {auth_url}");
    open_browser(&auth_url);

    // 6. Wait for the callback (120 s timeout, keep accepting until we get a code)
    let code = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        accept_code(listener, &state),
    )
    .await
    .context("OAuth timeout: no browser callback received within 120 s")?
    .context("OAuth callback error")?;

    // 7. Exchange code for tokens
    let client = reqwest::Client::new();
    let body = format!(
        "grant_type=authorization_code&code={}&code_verifier={}&redirect_uri={}&client_id={}",
        pct_encode(&code),
        pct_encode(&verifier),
        pct_encode(&redirect_uri),
        pct_encode(&client_id),
    );
    let resp = client
        .post(&meta.token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
        .context("token exchange request failed")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("token exchange failed ({status}): {text}"));
    }

    let mut token_json: serde_json::Value =
        resp.json().await.context("invalid token response JSON")?;

    // Augment the stored token with metadata needed for refresh (RFC 6749 §6).
    token_json["_mcp_token_endpoint"] = serde_json::Value::String(meta.token_endpoint.clone());
    if let Some(expires_in) = token_json["expires_in"].as_u64() {
        let expires_at = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(expires_in);
        token_json["_mcp_expires_at"] = serde_json::Value::Number(expires_at.into());
    }

    Ok(token_json)
}

/// Attempt to refresh an OAuth access token using the stored `refresh_token`.
///
/// Returns the refreshed token JSON on success (with updated `_mcp_expires_at`).
/// Returns `None` if refresh is not needed, not possible, or fails.
pub async fn try_refresh_token(
    server_name: &str,
    stored: &serde_json::Value,
) -> Option<serde_json::Value> {
    // Only refresh if expiry metadata is present and the token is near expiry.
    let expires_at = stored["_mcp_expires_at"].as_u64()?;
    let now = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    // Refresh 60 s before expiry to avoid races.
    if now.saturating_add(60) < expires_at {
        return Some(stored.clone()); // still valid
    }

    let refresh_token = stored["refresh_token"].as_str()?.to_string();
    let token_endpoint = stored["_mcp_token_endpoint"].as_str()?.to_string();

    log::info!("[mcp] refreshing token for '{server_name}'");

    let body = format!(
        "grant_type=refresh_token&refresh_token={}",
        pct_encode(&refresh_token)
    );
    let resp = match reqwest::Client::new()
        .post(&token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[mcp] token refresh request failed: {e}");
            return None;
        }
    };

    if !resp.status().is_success() {
        log::warn!("[mcp] token refresh returned {}", resp.status());
        return None;
    }

    let mut new_token: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[mcp] token refresh response parse error: {e}");
            return None;
        }
    };

    // Carry forward refresh_token if the server omitted it (RFC 6749 §6).
    if new_token["refresh_token"]
        .as_str()
        .map_or(true, str::is_empty)
    {
        new_token["refresh_token"] = stored["refresh_token"].clone();
    }
    // Re-attach metadata.
    new_token["_mcp_token_endpoint"] = serde_json::Value::String(token_endpoint);
    if let Some(expires_in) = new_token["expires_in"].as_u64() {
        let expires_at = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_add(expires_in);
        new_token["_mcp_expires_at"] = serde_json::Value::Number(expires_at.into());
    }

    // Persist the refreshed token.
    if let Err(e) = crate::agent::local_tools::store_mcp_token(server_name, &new_token) {
        log::warn!("[mcp] failed to store refreshed token: {e}");
    }

    Some(new_token)
}

// ── Dynamic client registration (RFC 7591) ────────────────────────────────────

/// Register a new OAuth public client and return the assigned `client_id`.
async fn register_client(
    registration_endpoint: &str,
    redirect_uri: &str,
) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    struct RegResponse {
        client_id: String,
    }

    let body = serde_json::json!({
        "client_name": "Dragent",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none"
    });

    let resp = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?
        .post(registration_endpoint)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .context("dynamic client registration request failed")?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("client registration failed ({status}): {text}"));
    }

    let reg: RegResponse = resp.json().await.context("invalid registration response")?;
    Ok(reg.client_id)
}

// ── Callback server ───────────────────────────────────────────────────────────

/// Accept connections on the listener until one delivers an authorization code.
/// Validates the CSRF `state` parameter. Ignores extra browser requests (favicon, etc.).
async fn accept_code(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> anyhow::Result<String> {
    loop {
        let (stream, _) = listener.accept().await.context("callback accept failed")?;
        let (reader, mut writer) = tokio::io::split(stream);
        let mut lines = BufReader::new(reader).lines();

        // Read request line
        let request_line = lines.next_line().await?.unwrap_or_default();

        // Drain headers
        loop {
            match lines.next_line().await? {
                Some(l) if !l.trim().is_empty() => {}
                _ => break,
            }
        }

        // Always respond 200 so the browser doesn't hang.
        let body = b"<html><body><h2>Authentication complete.</h2>\
                     <p>You may close this tab and return to Dragent.</p></body></html>";
        let _ = writer
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        let _ = writer.write_all(body).await;

        // "GET /callback?code=XYZ&state=ABC HTTP/1.1"
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        let query = path.split('?').nth(1).unwrap_or("");

        let mut code = None;
        let mut state = None;
        let mut error = None;
        for part in query.split('&') {
            if let Some((k, v)) = part.split_once('=') {
                match k {
                    "code" => code = Some(pct_decode(v)),
                    "state" => state = Some(pct_decode(v)),
                    "error" => error = Some(pct_decode(v)),
                    _ => {}
                }
            }
        }

        if let Some(e) = error {
            let desc = query
                .split('&')
                .find_map(|p| p.strip_prefix("error_description=").map(pct_decode))
                .unwrap_or_default();
            return Err(anyhow!("OAuth error from server: {e} — {desc}"));
        }

        if let Some(c) = code {
            // Validate CSRF state when the server included it.
            if let Some(s) = state {
                if s != expected_state {
                    return Err(anyhow!("OAuth CSRF state mismatch — possible CSRF attack"));
                }
            }
            return Ok(c);
        }
        // Browser made an unrelated request (favicon etc.) — keep listening.
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn pkce_verifier() -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..43)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// Extract a named parameter from a `Bearer key="value", ...` header.
fn extract_bearer_param(header: &str, key: &str) -> Option<String> {
    let rest = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix(&format!("{key}=")) {
            return Some(val.trim_matches('"').to_string());
        }
    }
    None
}

/// Build the list of candidate OAuth authorization server metadata URLs to try,
/// following the MCP spec (which references RFC 8414 §3).
///
/// If `auth_server` has a path component the spec says to insert it after
/// `.well-known/oauth-authorization-server`, e.g.:
///   "https://example.com/tenant" → "https://example.com/.well-known/oauth-authorization-server/tenant"
/// We try both that form AND the simple `{origin}/.well-known/oauth-authorization-server`
/// so that servers which don't follow the path-based form still work.
fn as_metadata_urls(auth_server: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if let Ok(parsed) = url::Url::parse(auth_server) {
        let path = parsed.path().trim_matches('/');
        let origin = origin_of(auth_server);
        if !path.is_empty() {
            urls.push(format!(
                "{origin}/.well-known/oauth-authorization-server/{path}"
            ));
        }
        urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
    } else {
        urls.push(format!(
            "{auth_server}/.well-known/oauth-authorization-server"
        ));
    }
    urls
}

/// Return the scheme+host of a URL: "https://api.foo.com/path" → "https://api.foo.com"
fn origin_of(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        let scheme = parsed.scheme();
        if let Some(host) = parsed.host_str() {
            return match parsed.port() {
                Some(p) => format!("{scheme}://{host}:{p}"),
                None => format!("{scheme}://{host}"),
            };
        }
    }
    url.to_string()
}

fn open_browser(url: &str) {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[url])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/c", "start", url])
    } else {
        ("xdg-open", &[url])
    };
    let _ = std::process::Command::new(cmd)
        .no_window()
        .args(args)
        .spawn();
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn pct_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(b as char);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
