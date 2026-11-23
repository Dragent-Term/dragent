//! MCP authentication flows and token storage.
//!
//! Moved verbatim; same behavior.

//! Local tool execution — implements the tool calls delegated by the remote server.
//!
//! When the LLM requests a tool (read_file, grep, run_command, etc.), the server
//! emits a `KindToolCall` SSE event and waits. This module executes that tool
//! locally and returns (output, is_error) so the client can POST the result back.

use super::tools_background::*;
use super::tools_command::*;
use super::tools_files::*;
use super::tools_search::*;
use super::tools_shell::*;
use super::tools_subagent::*;
use crate::process_ext::NoWindow;
use base64::Engine as _;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use uuid::Uuid;
pub(crate) fn exec_sleep(args: &serde_json::Value) -> (String, bool) {
    let ms = args["ms"].as_u64().unwrap_or(1000).clamp(1, 300_000);
    std::thread::sleep(std::time::Duration::from_millis(ms));
    (format!("Slept {ms}ms."), false)
}

// ── mcp_auth ──────────────────────────────────────────────────────────────────
// OAuth 2.0 + PKCE browser flow for MCP server authentication.
//
// Flow:
//   1. Generate PKCE verifier (43 random URL-safe chars) + SHA-256 challenge.
//   2. Bind ephemeral localhost port as the redirect URI.
//   3. Open the authorization URL in the system browser.
//   4. Accept one TCP connection on the redirect port, parse the `code` param.
//   5. Exchange code + verifier for tokens at the token endpoint.
//   6. Store tokens in platform app data at dragent/mcp-tokens/<server_name>.json.

pub fn exec_mcp_auth(args: &serde_json::Value) -> (String, bool) {
    let server_name = match str_arg(args, "server_name") {
        Ok(s) => s,
        Err(e) => return (e, true),
    };
    let auth_url = match str_arg(args, "auth_url") {
        Ok(s) => s,
        Err(e) => return (e, true),
    };
    let token_url = match str_arg(args, "token_url") {
        Ok(s) => s,
        Err(e) => return (e, true),
    };
    let client_id = args["client_id"].as_str().unwrap_or("dragent");
    let scope = args["scope"].as_str().unwrap_or("");

    // 1. PKCE
    let verifier = pkce_generate_verifier();
    let challenge = pkce_challenge(&verifier);

    // 2. Ephemeral redirect listener
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            return (
                format!("mcp_auth: could not bind callback listener: {e}"),
                true,
            )
        }
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(e) => return (format!("mcp_auth: could not get local port: {e}"), true),
    };
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 3. Build and open the authorization URL
    let scope_param = if scope.is_empty() {
        String::new()
    } else {
        format!("&scope={}", oauth_encode(scope))
    };
    let full_auth_url = format!(
        "{auth_url}?response_type=code\
         &client_id={cid}\
         &redirect_uri={redir}\
         &code_challenge={challenge}\
         &code_challenge_method=S256{scope_param}",
        cid = oauth_encode(client_id),
        redir = oauth_encode(&redirect_uri),
    );

    log::info!("[mcp_auth] opening browser for {server_name}: {full_auth_url}");
    open_in_browser(&full_auth_url);

    // 4. Wait for the callback (2-minute timeout)
    let code = match wait_for_oauth_code(listener, 120) {
        Ok(c) => c,
        Err(e) => return (format!("mcp_auth: OAuth callback failed: {e}"), true),
    };

    // 5. Exchange code for tokens
    let token_resp =
        match exchange_oauth_code(token_url, &code, &verifier, &redirect_uri, client_id) {
            Ok(v) => v,
            Err(e) => return (format!("mcp_auth: token exchange failed: {e}"), true),
        };

    // 6. Store tokens
    let token_path = match store_oauth_token(server_name, &token_resp) {
        Ok(p) => p,
        Err(e) => return (format!("mcp_auth: token storage failed: {e}"), true),
    };

    // 7. Reconnect the server immediately so the agent can use tools without a restart.
    crate::agent::mcp::reinit_server(server_name, None);

    let access_token_preview = token_resp["access_token"]
        .as_str()
        .map(|t| format!("{}…", utf8_prefix(t, 8)))
        .unwrap_or_else(|| "(none)".to_string());

    (
        format!(
            "MCP authentication succeeded for '{server_name}'.\n\
             Token stored at: {token_path}\n\
             Access token: {access_token_preview}\n\
             You can now use tools from this MCP server."
        ),
        false,
    )
}

pub(crate) fn pkce_generate_verifier() -> String {
    use rand::Rng;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::thread_rng();
    (0..43)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
}

pub(crate) fn open_in_browser(url: &str) {
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[url] as &[&str])
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

pub(crate) fn wait_for_oauth_code(
    listener: std::net::TcpListener,
    timeout_secs: u64,
) -> Result<String, String> {
    // Non-blocking poll so we can check the deadline without platform-specific socket options.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Could not set listener to non-blocking: {e}"))?;

    let deadline = web_time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if web_time::Instant::now() >= deadline {
            return Err(format!(
                "OAuth timeout: no browser callback received within {timeout_secs}s. \
                 Make sure you approved the authorization in the browser."
            ));
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                use std::io::{BufRead, Write};
                // Switch the accepted stream back to blocking for simple line-read.
                let _ = stream.set_nonblocking(false);
                let mut reader = std::io::BufReader::new(&stream);
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);

                // Consume the rest of the HTTP headers to prevent TCP connection resets
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(n) if n > 0 => {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                break;
                            }
                        }
                        _ => break,
                    }
                }

                // Immediately return a success page so the browser doesn't hang.
                let body = b"<html><body><h2>Authentication complete.</h2>\
                      <p>You may close this tab and return to Dragent.</p></body></html>";
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(body);

                // GET /callback?code=XYZ&state=... HTTP/1.1
                if let Some(code) = extract_query_param(&request_line, "code") {
                    return Ok(code);
                }
                if let Some(err) = extract_query_param(&request_line, "error") {
                    let desc =
                        extract_query_param(&request_line, "error_description").unwrap_or_default();
                    return Err(format!("OAuth error from server: {err} — {desc}"));
                }
                // Browser may make additional requests (favicon, etc.) — keep listening.
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Listener accept error: {e}")),
        }
    }
}

pub(crate) fn extract_query_param(request_line: &str, param: &str) -> Option<String> {
    // "GET /callback?code=xyz&state=abc HTTP/1.1"
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for part in query.split('&') {
        if let Some((key, val)) = part.split_once('=') {
            if key == param {
                return Some(oauth_decode(val));
            }
        }
    }
    None
}

#[cfg(target_family = "wasm")]
pub(crate) fn exchange_oauth_code(
    _token_url: &str,
    _code: &str,
    _verifier: &str,
    _redirect_uri: &str,
    _client_id: &str,
) -> Result<serde_json::Value, String> {
    Err("OAuth exchange unavailable in the web viewer".to_string())
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn exchange_oauth_code(
    token_url: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
    client_id: &str,
) -> Result<serde_json::Value, String> {
    let body = format!(
        "grant_type=authorization_code&code={}&code_verifier={}&redirect_uri={}&client_id={}",
        oauth_encode(code),
        oauth_encode(verifier),
        oauth_encode(redirect_uri),
        oauth_encode(client_id),
    );

    let resp = ureq::post(token_url)
        .timeout(std::time::Duration::from_secs(30))
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
        .map_err(|e| format!("HTTP error: {e}"))?;

    resp.into_json::<serde_json::Value>()
        .map_err(|e| format!("JSON parse error: {e}"))
}

// ── MCP token storage ─────────────────────────────────────────────────────────
// Mirrors the session_store pattern:
//   Primary   — platform app data/dragent/mcp-tokens/<name>.json  (0o600 on Unix)
//   Secondary — OS Keychain, service `dragent_paths::keychain_service()`, account "mcp_token:<name>"
//               (best-effort; silently skipped when unavailable)

pub(crate) fn mcp_safe_name(server_name: &str) -> String {
    server_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn mcp_token_path(server_name: &str) -> Option<std::path::PathBuf> {
    Some(
        dragent_paths::platform_support()
            .join("mcp-tokens")
            .join(format!("{}.json", mcp_safe_name(server_name))),
    )
}

pub(crate) fn mcp_file_save(path: &std::path::Path, json: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create mcp-tokens dir: {e}"))?;

        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(parent) {
                let mut perms = meta.permissions();
                perms.set_mode(0o700);
                let _ = std::fs::set_permissions(parent, perms);
            }
        }
    }

    #[cfg(target_family = "unix")]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut f| f.write_all(json.as_bytes()))
            .map_err(|e| format!("write mcp token file: {e}"))
    }

    #[cfg(not(target_family = "unix"))]
    std::fs::write(path, json).map_err(|e| format!("write mcp token file: {e}"))
}

// OS-keychain copy is native-only (no `keyring` backend on wasm). MCP itself is
// native-only, so on wasm these are unreachable no-ops kept only for type-checking.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn mcp_keychain_save(server_name: &str, json: &str) {
    if let Ok(entry) = keyring::Entry::new(
        dragent_paths::keychain_service(),
        &format!("mcp_token:{server_name}"),
    ) {
        if let Err(e) = entry.set_password(json) {
            log::debug!("[mcp_auth] keychain save skipped for {server_name}: {e}");
        }
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn mcp_keychain_load(server_name: &str) -> Option<serde_json::Value> {
    let json = keyring::Entry::new(
        dragent_paths::keychain_service(),
        &format!("mcp_token:{server_name}"),
    )
    .ok()?
    .get_password()
    .ok()?;
    serde_json::from_str(&json).ok()
}

#[cfg(target_family = "wasm")]
pub(crate) fn mcp_keychain_save(_server_name: &str, _json: &str) {}

#[cfg(target_family = "wasm")]
pub(crate) fn mcp_keychain_load(_server_name: &str) -> Option<serde_json::Value> {
    None
}

/// Persist an MCP OAuth token using the same layered strategy as the app's own
/// refresh token: 0o600 file in the XDG config dir + best-effort OS Keychain copy.
pub fn store_mcp_token(server_name: &str, token: &serde_json::Value) -> Result<String, String> {
    store_oauth_token(server_name, token)
}

pub(crate) fn store_oauth_token(
    server_name: &str,
    token: &serde_json::Value,
) -> Result<String, String> {
    let path = mcp_token_path(server_name)
        .ok_or_else(|| "cannot determine config directory".to_string())?;
    let json = serde_json::to_string_pretty(token).unwrap_or_else(|_| token.to_string());

    mcp_file_save(&path, &json)?;
    mcp_keychain_save(server_name, &json);

    Ok(path.to_string_lossy().into_owned())
}

/// Delete a stored MCP OAuth token (both file and keychain).
/// Called when a token is detected as expired so the user is prompted to re-authorize.
pub fn clear_mcp_token(server_name: &str) {
    if let Some(path) = mcp_token_path(server_name) {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(not(target_family = "wasm"))]
    if let Ok(entry) = keyring::Entry::new(
        dragent_paths::keychain_service(),
        &format!("mcp_token:{server_name}"),
    ) {
        let _ = entry.delete_credential();
    }
}

/// Load a previously stored MCP OAuth token.
///
/// Load order:
///   1. OS Keychain (fastest path when available)
///   2. platform app data/dragent/mcp-tokens/<name>.json
pub fn load_mcp_token(server_name: &str) -> Option<serde_json::Value> {
    if let Some(v) = mcp_keychain_load(server_name) {
        return Some(v);
    }
    let path = mcp_token_path(server_name)?;
    let json = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

pub(crate) fn oauth_encode(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

pub(crate) fn oauth_decode(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().unwrap_or('0');
            let h2 = chars.next().unwrap_or('0');
            if let Ok(byte) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                out.push(byte as char);
            }
        } else if c == '+' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────────────
