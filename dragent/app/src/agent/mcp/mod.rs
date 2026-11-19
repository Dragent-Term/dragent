mod credential_store;
mod legacy_sse;
mod manager;
mod oauth;
mod oauth_pending;
mod oauth_static;
mod types;

pub use manager::McpRuntimeManager;
pub use types::{CliConfig, McpServerConfig, SseConfig, TransportType};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::sync::RwLock;

pub static MCP_MANAGER: Lazy<RwLock<Option<McpRuntimeManager>>> = Lazy::new(|| RwLock::new(None));

/// Sentinel error message returned by `connect_server` when the OAuth deep-link
/// flow has been started but not yet completed (browser is open, waiting for callback).
pub(crate) const MCP_OAUTH_PENDING_ERR: &str = "__mcp_oauth_pending__";

/// Global channel for notifying the settings UI when an MCP server reconnects.
/// SettingsView registers its sender here; `handle_mcp_oauth_callback` sends
/// the server name so the UI can display the updated connection status.
static MCP_RECONNECT_NOTIFIER: Lazy<Mutex<Option<async_channel::Sender<String>>>> =
    Lazy::new(|| Mutex::new(None));

/// Called by SettingsView at construction time to register itself for MCP reconnect
/// notifications that originate outside the view (e.g. deep-link OAuth callbacks).
pub fn register_reconnect_notifier(tx: async_channel::Sender<String>) {
    *MCP_RECONNECT_NOTIFIER.lock() = Some(tx);
}

/// Load config and initialize the MCP manager. Called once at startup.
///
/// SSE servers without a stored OAuth token are skipped — they require interactive
/// browser auth and must be connected explicitly via the "Connect" button.
/// Stdio servers and SSE servers with a saved token are connected immediately.
///
/// The plugin-system overlay applies on top of each server's own `enabled`
/// flag: when the plugin system is off, or a server is in `disabled_plugins`,
/// the server is not started at all.
pub fn init() {
    let settings = crate::settings::AppSettings::global();
    let configs: Vec<McpServerConfig> = crate::settings::mcp_config::load_entries()
        .into_iter()
        .filter(|e| e.enabled && settings.plugin.is_plugin_enabled(&e.name))
        // SSE servers without saved tokens are probed at connect time.
        // connect_server(allow_oauth=false) skips servers that need OAuth silently.
        .map(entry_to_config)
        .collect();

    if configs.is_empty() {
        return;
    }
    log::info!("[mcp] connecting to {} server(s) at startup", configs.len());
    // Legacy developer manifest entries remain diagnostic metadata only.
    let dev = settings.plugin.dev_manifests();
    if !dev.is_empty() {
        log::info!(
            "[plugins] {} dev manifest(s): {}",
            dev.len(),
            dev.iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mgr = McpRuntimeManager::connect(configs);
    if let Ok(mut guard) = MCP_MANAGER.write() {
        *guard = Some(mgr);
    }
}

/// Plugin gate shared by every MCP entry point. Returns an error message when
/// the plugin system blocks this server id: system switch off, id disabled,
/// or first-run consent not yet recorded. `None` = allowed.
fn plugin_block_reason(sanitized_server_id: &str) -> Option<String> {
    let settings = crate::settings::AppSettings::global();
    if !settings.plugin.system_enabled {
        return Some(
            "Plugin system is disabled — enable it in Settings → MCP Servers → Tool access in Dragent to use plugin tools."
                .to_string(),
        );
    }
    if !settings
        .plugin
        .is_plugin_enabled_sanitized(sanitized_server_id)
    {
        return Some(format!(
            "Plugin '{sanitized_server_id}' is disabled in Settings → MCP Servers → Tool access in Dragent."
        ));
    }
    if !settings.plugin.consents.keys().any(|id| {
        id == sanitized_server_id || id.replace('-', "_") == sanitized_server_id
    }) {
        return Some(format!(
            "Plugin '{sanitized_server_id}' needs approval before its first run — approve it in Settings → MCP Servers → Tool access in Dragent."
        ));
    }
    None
}

/// Extract the (sanitized) server id from a prefixed tool/prompt/resource name.
fn prefixed_server_id<'a>(prefixed_name: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = prefixed_name.strip_prefix(prefix)?;
    let sep = rest.find("__")?;
    Some(&rest[..sep])
}

/// True when a plugin id may contribute tool/prompt/resource definitions to
/// the model: master switch on and id not disabled. Consent is enforced at
/// call time, not here, so the model can attempt a tool and receive the
/// "needs approval" error that tells the user where to approve it.
fn plugin_defs_visible(sanitized_server_id: &str) -> bool {
    crate::settings::AppSettings::global()
        .plugin
        .is_plugin_enabled_sanitized(sanitized_server_id)
}

/// Drop defs that belong to plugins the system currently disables.
fn retain_visible_defs(
    defs: Vec<(String, String, String)>,
    prefix: &str,
) -> Vec<(String, String, String)> {
    defs.into_iter()
        .filter(|(name, _, _)| match prefixed_server_id(name, prefix) {
            Some(id) => plugin_defs_visible(id),
            None => true,
        })
        .collect()
}

/// Call an MCP tool synchronously. Returns (output, is_error).
pub fn call_tool(prefixed_name: &str, args_json: &str) -> (String, bool) {
    if let Some(id) = prefixed_server_id(prefixed_name, "mcp__") {
        if let Some(reason) = plugin_block_reason(id) {
            return (reason, true);
        }
    }
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return ("MCP manager lock poisoned".to_string(), true),
    };
    match &*guard {
        Some(mgr) => mgr.call_tool(prefixed_name, args_json),
        None => ("MCP not initialized".to_string(), true),
    }
}

/// Reconnect a single server after its OAuth token has been stored.
///
/// Returns immediately — the actual connection runs on a background OS thread
/// so the UI is never blocked waiting for network I/O or lock contention.
/// The `MCP_MANAGER` write lock is held only briefly (to remove the stale entry
/// and clone the runtime handle), never across any network operation.
///
/// `on_done` is called on the background thread once the connection attempt
/// completes (whether it succeeds or fails), so callers can post UI notifications
/// that reflect the real outcome rather than firing prematurely.
pub fn reinit_server(name: &str, on_done: Option<Box<dyn FnOnce() + Send + 'static>>) {
    let Some(cfg) = find_server_config(name) else {
        log::warn!("[mcp] reinit_server: no config found for '{name}'");
        if let Some(cb) = on_done {
            cb();
        }
        return;
    };
    // The plugin overlay may have changed since the config was written.
    if !crate::settings::AppSettings::global()
        .plugin
        .is_plugin_enabled(&cfg.name)
    {
        log::info!("[mcp] reinit_server: plugin '{name}' is disabled — skipping");
        if let Some(cb) = on_done {
            cb();
        }
        return;
    }

    crate::bg::detach(move || {
        // Step 1 — briefly acquire the write lock:
        //   • remove the stale server entry
        //   • clone the Arc<Runtime> so we can drive async work without the lock
        let rt = {
            let Ok(mut guard) = MCP_MANAGER.write() else {
                if let Some(cb) = on_done {
                    cb();
                }
                return;
            };
            match &mut *guard {
                Some(mgr) => {
                    mgr.remove_server(&cfg.name);
                    mgr.rt_handle()
                }
                None => {
                    // No manager yet — stand up an empty one to own the runtime.
                    let new_mgr = McpRuntimeManager::new_empty();
                    let rt = new_mgr.rt_handle();
                    *guard = Some(new_mgr);
                    rt
                }
            }
        }; // ← write lock released here, before any network I/O

        // Step 2 — connect OUTSIDE the lock (can take 10-30 s).
        // allow_oauth=true: reinit is triggered by the user clicking "Connect" in Settings,
        // so opening the OAuth browser flow here is expected.
        match rt.block_on(manager::connect_server(&cfg, true)) {
            Ok(server) => {
                log::info!(
                    "[mcp] reconnected '{}' ({} tools)",
                    cfg.name,
                    server.tools.len()
                );
                let Ok(mut guard) = MCP_MANAGER.write() else {
                    if let Some(cb) = on_done {
                        cb();
                    }
                    return;
                };
                if let Some(mgr) = &mut *guard {
                    mgr.push_server(server);
                }
            }
            Err(e) if e.to_string().contains(MCP_OAUTH_PENDING_ERR) => {
                log::info!(
                    "[mcp] '{}' OAuth flow started — waiting for browser callback",
                    cfg.name
                );
                // on_done still fires so the UI can stop its spinner.
            }
            Err(e) => log::warn!("[mcp] reconnect '{}' failed: {e}", cfg.name),
        }

        if let Some(cb) = on_done {
            cb();
        }
    });
}

/// Returns true if `url` is a MCP OAuth deep-link callback delivered by the OS.
pub fn is_mcp_oauth_callback(url: &str) -> bool {
    use oauth_pending::{MCP_OAUTH_CALLBACK_HOST, MCP_OAUTH_CALLBACK_PATH};
    if let Ok(parsed) = url::Url::parse(url) {
        return parsed.scheme() == "dragent"
            && parsed.host_str() == Some(MCP_OAUTH_CALLBACK_HOST)
            && parsed.path() == MCP_OAUTH_CALLBACK_PATH;
    }
    false
}

/// Handle an MCP OAuth deep-link callback from the OS.
///
/// Parses `code` + `state`, looks up the pending PKCE session, exchanges the
/// code for a token on a background thread, stores the token, then reconnects
/// the server and notifies the settings UI.
pub fn handle_mcp_oauth_callback(url: &str) {
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(e) => {
            log::warn!("[mcp oauth] invalid callback URL: {e}");
            return;
        }
    };

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_desc = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => error_desc = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(e) = error {
        log::warn!(
            "[mcp oauth] server returned error: {} — {}",
            e,
            error_desc.unwrap_or_default()
        );
        return;
    }

    let (code, state) = match (code, state) {
        (Some(c), Some(s)) => (c, s),
        _ => {
            log::warn!("[mcp oauth] callback missing code or state");
            return;
        }
    };

    let session = match oauth_pending::take(&state) {
        Some(s) => s,
        None => {
            log::warn!("[mcp oauth] no pending session for state — callback ignored");
            return;
        }
    };

    let server_name = session.server_name.clone();
    log::info!(
        "[mcp oauth] '{}' callback received — exchanging token",
        server_name
    );

    crate::bg::detach(move || {
        match exchange_mcp_token(&session, &code) {
            Ok(mut token_json) => {
                // Augment with refresh metadata (same as run_pkce_flow does).
                token_json["_mcp_token_endpoint"] =
                    serde_json::Value::String(session.token_endpoint.clone());
                if let Some(exp) = token_json["expires_in"].as_u64() {
                    let at = web_time::SystemTime::now()
                        .duration_since(web_time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .saturating_add(exp);
                    token_json["_mcp_expires_at"] = serde_json::Value::Number(at.into());
                }

                if let Err(e) =
                    crate::agent::local_tools::store_mcp_token(&session.server_name, &token_json)
                {
                    log::warn!("[mcp oauth] token storage failed: {e}");
                    return;
                }

                log::info!(
                    "[mcp oauth] '{}' token stored — reconnecting",
                    session.server_name
                );

                let notifier = MCP_RECONNECT_NOTIFIER.lock().clone();
                let name_cb = session.server_name.clone();
                reinit_server(
                    &session.server_name,
                    Some(Box::new(move || {
                        if let Some(tx) = notifier {
                            let _ = tx.send_blocking(name_cb);
                        }
                    })),
                );
            }
            Err(e) => {
                log::warn!(
                    "[mcp oauth] '{}' token exchange failed: {e}",
                    session.server_name
                );
            }
        }
    });

    let _ = server_name; // consumed above
}

/// Synchronous token exchange (runs on background thread).
fn exchange_mcp_token(
    session: &oauth_pending::McpPkceSession,
    code: &str,
) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context as _;

    let body = format!(
        "grant_type=authorization_code&code={}&code_verifier={}&redirect_uri={}&client_id={}",
        pct_encode(code),
        pct_encode(&session.verifier),
        pct_encode(&session.redirect_uri),
        pct_encode(&session.client_id),
    );

    let resp = ureq::post(&session.token_endpoint)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("Accept", "application/json")
        .send_string(&body);

    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(status, r)) => {
            let text = r.into_string().unwrap_or_default();
            anyhow::bail!("token exchange failed ({status}): {text}");
        }
        Err(e) => anyhow::bail!("token exchange request failed: {e}"),
    };

    let json: serde_json::Value = resp.into_json().context("invalid token response JSON")?;
    Ok(json)
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

fn find_server_config(name: &str) -> Option<McpServerConfig> {
    crate::settings::mcp_config::load_entries()
        .into_iter()
        .find(|e| e.name == name && e.enabled)
        .map(entry_to_config)
}

fn entry_to_config(entry: crate::settings::McpServerEntry) -> McpServerConfig {
    let transport = match entry.transport {
        crate::settings::McpTransport::Stdio => TransportType::CliServer(CliConfig {
            command: entry.command.clone(),
            args: entry.args.clone(),
            cwd_parameter: None,
            env: entry
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }),
        crate::settings::McpTransport::Sse => TransportType::ServerSentEvents(SseConfig {
            url: entry.command.clone(),
            headers: std::collections::HashMap::new(),
            bearer_token: entry.bearer_token.clone(),
        }),
    };
    McpServerConfig {
        name: entry.name,
        description: None,
        transport,
    }
}

/// If `server_name` failed to connect at startup, returns the error string.
/// Returns None if the server is connected (or unknown to the manager).
pub fn connection_error_for(server_name: &str) -> Option<String> {
    let Ok(guard) = MCP_MANAGER.read() else {
        return None;
    };
    guard.as_ref().and_then(|mgr| {
        mgr.failed_servers
            .iter()
            .find(|(n, _)| n == server_name)
            .map(|(_, e)| e.clone())
    })
}

/// How many tools a specific server has connected (0 = failed/not connected).
pub fn tool_count_for(server_name: &str) -> usize {
    let Ok(guard) = MCP_MANAGER.read() else {
        return 0;
    };
    match &*guard {
        Some(mgr) => mgr.tool_count(server_name),
        None => 0,
    }
}

/// Names of every tool on a specific connected server. Empty if not connected.
pub fn tools_for(server_name: &str) -> Vec<String> {
    let Ok(guard) = MCP_MANAGER.read() else {
        return Vec::new();
    };
    match &*guard {
        Some(mgr) => mgr
            .servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.tools.iter().map(|t| t.name.to_string()).collect())
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Return tool definitions (name, description, schema_json) for all connected MCP servers.
pub fn all_tool_defs() -> Vec<(String, String, String)> {
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let defs = match &*guard {
        Some(mgr) => mgr.all_tool_defs(),
        None => Vec::new(),
    };
    retain_visible_defs(defs, "mcp__")
}

/// Return prompt definitions (prefixed_name, description, schema_json) for all connected servers.
pub fn all_prompt_defs() -> Vec<(String, String, String)> {
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let defs = match &*guard {
        Some(mgr) => mgr.all_prompt_defs(),
        None => Vec::new(),
    };
    retain_visible_defs(defs, "prompt__")
}

/// Execute a prompt synchronously. prefixed_name is "prompt__server__name".
pub fn call_prompt(prefixed_name: &str, args_json: &str) -> (String, bool) {
    if let Some(id) = prefixed_server_id(prefixed_name, "prompt__") {
        if let Some(reason) = plugin_block_reason(id) {
            return (reason, true);
        }
    }
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return ("MCP manager lock poisoned".to_string(), true),
    };
    match &*guard {
        Some(mgr) => mgr.call_prompt(prefixed_name, args_json),
        None => ("MCP not initialized".to_string(), true),
    }
}

/// How many prompts a specific server has (0 = failed/not connected).
pub fn prompt_count_for(server_name: &str) -> usize {
    let Ok(guard) = MCP_MANAGER.read() else {
        return 0;
    };
    match &*guard {
        Some(mgr) => mgr.prompt_count(server_name),
        None => 0,
    }
}

/// Names of every prompt on a specific connected server.
pub fn prompts_for(server_name: &str) -> Vec<String> {
    let Ok(guard) = MCP_MANAGER.read() else {
        return Vec::new();
    };
    match &*guard {
        Some(mgr) => mgr.prompts_for(server_name),
        None => Vec::new(),
    }
}

/// Return resource definitions (prefixed_name, description, schema_json) for all connected servers.
pub fn all_resource_defs() -> Vec<(String, String, String)> {
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    let defs = match &*guard {
        Some(mgr) => mgr.all_resource_defs(),
        None => Vec::new(),
    };
    retain_visible_defs(defs, "resource__")
}

/// Read an MCP resource synchronously. prefixed_name is "resource__server__name".
pub fn call_resource(prefixed_name: &str) -> (String, bool) {
    if let Some(id) = prefixed_server_id(prefixed_name, "resource__") {
        if let Some(reason) = plugin_block_reason(id) {
            return (reason, true);
        }
    }
    let guard = match MCP_MANAGER.read() {
        Ok(g) => g,
        Err(_) => return ("MCP manager lock poisoned".to_string(), true),
    };
    match &*guard {
        Some(mgr) => mgr.call_resource(prefixed_name),
        None => ("MCP not initialized".to_string(), true),
    }
}

/// How many resources a specific server exposes (0 = failed/not connected).
pub fn resource_count_for(server_name: &str) -> usize {
    let Ok(guard) = MCP_MANAGER.read() else {
        return 0;
    };
    match &*guard {
        Some(mgr) => mgr.resource_count(server_name),
        None => 0,
    }
}

/// URIs of every resource on a specific connected server.
pub fn resources_for(server_name: &str) -> Vec<String> {
    let Ok(guard) = MCP_MANAGER.read() else {
        return Vec::new();
    };
    match &*guard {
        Some(mgr) => mgr.resources_for(server_name),
        None => Vec::new(),
    }
}
