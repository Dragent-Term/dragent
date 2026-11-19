use super::types::{McpServerConfig, TransportType};
use crate::process_ext::NoWindow;
use rmcp::model::{
    CallToolRequestParams, GetPromptRequestParams, Prompt, ReadResourceRequestParams, Resource,
    Tool,
};
use rmcp::transport::TokioChildProcess;
use rmcp::{Peer, RoleClient, ServiceExt};
use std::sync::Arc;

pub struct McpRuntimeManager {
    pub(crate) rt: Arc<tokio::runtime::Runtime>,
    pub(crate) servers: Vec<ConnectedServer>,
    /// Servers that failed to connect at startup: (name, error_message).
    /// Exposed so the settings UI can show per-server connection status.
    pub(crate) failed_servers: Vec<(String, String)>,
}

pub(crate) struct ConnectedServer {
    pub(crate) name: String,
    peer: Peer<RoleClient>,
    pub(crate) tools: Vec<Tool>,
    pub(crate) prompts: Vec<Prompt>,
    pub(crate) resources: Vec<Resource>,
    // Keeps the background service task alive. Dropping this cancels the transport.
    _service: rmcp::service::RunningService<RoleClient, ()>,
}

impl McpRuntimeManager {
    pub fn connect(configs: Vec<McpServerConfig>) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("mcp-runtime")
                .build()
                .expect("MCP tokio runtime"),
        );

        let (servers, failed_servers) = rt.block_on(async {
            let mut connected = Vec::new();
            let mut failed = Vec::new();
            for cfg in configs {
                match connect_server(&cfg, false).await {
                    Ok(s) => {
                        log::info!("[mcp] connected to '{}' ({} tools)", s.name, s.tools.len());
                        connected.push(s);
                    }
                    Err(e) => {
                        log::warn!("[mcp] '{}' failed to connect: {e}", cfg.name);
                        failed.push((cfg.name.clone(), e.to_string()));
                    }
                }
            }
            (connected, failed)
        });

        Self {
            rt,
            servers,
            failed_servers,
        }
    }

    /// Create a manager with a runtime but no connected servers yet.
    /// Used by `reinit_server` when no manager exists and we need a runtime to connect on.
    pub(crate) fn new_empty() -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("mcp-runtime")
                .build()
                .expect("MCP tokio runtime"),
        );
        Self {
            rt,
            servers: Vec::new(),
            failed_servers: Vec::new(),
        }
    }

    /// Clone the tokio runtime handle so callers can drive async work
    /// from a thread that does NOT hold `MCP_MANAGER` write lock.
    pub(crate) fn rt_handle(&self) -> Arc<tokio::runtime::Runtime> {
        Arc::clone(&self.rt)
    }

    /// Remove a server entry by name. Fast — no network I/O.
    pub(crate) fn remove_server(&mut self, name: &str) {
        self.servers.retain(|s| s.name != name);
    }

    /// Insert a freshly-connected server. Fast — no network I/O.
    pub(crate) fn push_server(&mut self, s: ConnectedServer) {
        self.servers.push(s);
    }

    /// Number of tools connected for a single server (0 = not connected or failed).
    pub fn tool_count(&self, server_name: &str) -> usize {
        self.servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.tools.len())
            .unwrap_or(0)
    }

    /// Number of prompts exposed by a single server.
    pub fn prompt_count(&self, server_name: &str) -> usize {
        self.servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.prompts.len())
            .unwrap_or(0)
    }

    /// Names of every prompt on a specific connected server.
    pub fn prompts_for(&self, server_name: &str) -> Vec<String> {
        self.servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.prompts.iter().map(|p| p.name.to_string()).collect())
            .unwrap_or_default()
    }

    /// Returns (prefixed_prompt_name, description, schema_json) for every prompt on every server.
    /// Prompts are exposed as callable "tools" with prefix `prompt__server__name` so the LLM
    /// can invoke them the same way it calls tools. Executing one calls `prompts/get`.
    pub fn all_prompt_defs(&self) -> Vec<(String, String, String)> {
        self.servers
            .iter()
            .flat_map(|s| {
                let safe_server = s.name.replace('-', "_");
                s.prompts.iter().map(move |p| {
                    let safe_name = p.name.replace('-', "_");
                    let prefixed = format!("prompt__{}__{}", safe_server, safe_name);
                    let raw_desc = p.description.as_deref().unwrap_or("");
                    let desc = if raw_desc.is_empty() {
                        format!("[{} prompt]", s.name)
                    } else {
                        format!("[{} prompt] {}", s.name, raw_desc)
                    };
                    // Build a JSON schema from the prompt's declared arguments.
                    let schema = prompt_args_to_schema(p.arguments.as_deref());
                    (prefixed, desc, schema)
                })
            })
            .collect()
    }

    /// Returns (prefixed_tool_name, description, schema_json) for every tool on every server.
    /// Descriptions are prefixed with "[server_name]" so the LLM knows which server provides
    /// each tool without needing extra system-prompt guidance.
    pub fn all_tool_defs(&self) -> Vec<(String, String, String)> {
        self.servers
            .iter()
            .flat_map(|s| {
                // Sanitize server and tool names for the function-name field:
                // Gemini requires [a-zA-Z_][a-zA-Z0-9_]* — hyphens are rejected.
                // OpenAI allows hyphens but underscores are universally safe.
                // We replace hyphens with underscores in the name only; the
                // original server name is still shown in the description for the LLM.
                let safe_server = s.name.replace('-', "_");
                s.tools.iter().map(move |t| {
                    let safe_tool = t.name.replace('-', "_");
                    let prefixed = format!("mcp__{}__{}", safe_server, safe_tool);
                    let raw_desc = t.description.as_deref().unwrap_or("");
                    let desc = if raw_desc.is_empty() {
                        format!("[{}]", s.name)
                    } else {
                        format!("[{}] {}", s.name, raw_desc)
                    };
                    let schema = serde_json::to_string(&t.schema_as_json_value())
                        .unwrap_or_else(|_| "{}".to_string());
                    (prefixed, desc, schema)
                })
            })
            .collect()
    }

    /// Execute an MCP tool synchronously. prefixed_name is "mcp__server__tool".
    pub fn call_tool(&self, prefixed_name: &str, args_json: &str) -> (String, bool) {
        // Parse "mcp__<server>__<tool>"
        let rest = match prefixed_name.strip_prefix("mcp__") {
            Some(r) => r,
            None => return ("not an MCP tool".to_string(), true),
        };
        let sep = match rest.find("__") {
            Some(i) => i,
            None => return (format!("malformed MCP tool name: {prefixed_name}"), true),
        };
        let server_name = &rest[..sep];
        let tool_name = &rest[sep + 2..];

        // The prefixed name uses sanitized (hyphen→underscore) server/tool names.
        // Match against the sanitized form of each connected server name.
        let server = match self
            .servers
            .iter()
            .find(|s| s.name.replace('-', "_") == server_name)
        {
            Some(s) => s,
            None => {
                let connected: Vec<_> = self.servers.iter().map(|s| s.name.as_str()).collect();
                log::warn!("[mcp] tool call for unknown server '{server_name}' — connected servers: {connected:?}");
                return (format!("MCP server '{server_name}' not connected"), true);
            }
        };

        let peer = server.peer.clone();
        // The LLM called the sanitized name (hyphens→underscores). Find the original
        // tool name from the server's tool list so the MCP peer gets what it expects.
        let original_tool_name: String = server
            .tools
            .iter()
            .find(|t| t.name.replace('-', "_") == tool_name)
            .map(|t| t.name.to_string())
            .unwrap_or_else(|| tool_name.to_string());
        let args_preview = crate::agent::tools_background::utf8_prefix(args_json, 300);
        log::info!("[mcp] → call {server_name}::{original_tool_name}  args={args_preview}");
        let t = web_time::Instant::now();
        let args_json = args_json.to_string();

        self.rt.block_on(async {
            let args: Option<serde_json::Map<String, serde_json::Value>> =
                if args_json.is_empty() {
                    None
                } else {
                    match serde_json::from_str(&args_json) {
                        Ok(m) => Some(m),
                        Err(e) => return (format!("invalid args JSON: {e}"), true),
                    }
                };

            let mut params = CallToolRequestParams::new(original_tool_name.clone());
            if let Some(a) = args {
                params = params.with_arguments(a);
            }

            let timeout = tokio::time::Duration::from_secs(60);
            match tokio::time::timeout(timeout, peer.call_tool(params)).await {
                Err(_) => {
                    log::warn!("[mcp] ✗ {server_name}::{original_tool_name} TIMED OUT after {}ms",
                        t.elapsed().as_millis());
                    (format!("MCP tool timed out after 60s — '{original_tool_name}' on server '{server_name}' did not respond"), true)
                }
                Ok(Ok(result)) => {
                    let text = mcp_content_to_string(&result.content);
                    let is_error = result.is_error.unwrap_or(false);
                    if is_error {
                        log::warn!("[mcp] ✗ {server_name}::{original_tool_name} FAILED in {}ms — {}",
                            t.elapsed().as_millis(), crate::agent::tools_background::utf8_prefix(&text, 200));
                    } else {
                        log::info!("[mcp] ✓ {server_name}::{original_tool_name} OK in {}ms — {} chars returned",
                            t.elapsed().as_millis(), text.len());
                        if !text.is_empty() {
                            log::debug!("[mcp] result preview: {}", crate::agent::tools_background::utf8_prefix(&text, 500));
                        }
                    }
                    (text, is_error)
                }
                Ok(Err(e)) => {
                    log::warn!("[mcp] ✗ {server_name}::{original_tool_name} error in {}ms — {e}",
                        t.elapsed().as_millis());
                    (format!("MCP tool error: {e}"), true)
                }
            }
        })
    }

    /// Number of resources exposed by a single server.
    pub fn resource_count(&self, server_name: &str) -> usize {
        self.servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.resources.len())
            .unwrap_or(0)
    }

    /// URIs of every resource on a specific connected server.
    pub fn resources_for(&self, server_name: &str) -> Vec<String> {
        self.servers
            .iter()
            .find(|s| s.name == server_name)
            .map(|s| s.resources.iter().map(|r| r.uri.clone()).collect())
            .unwrap_or_default()
    }

    /// Returns (prefixed_name, description, schema_json) for every resource on every server.
    /// Exposed as no-argument tools so the LLM can read them by calling `resource__server__name`.
    pub fn all_resource_defs(&self) -> Vec<(String, String, String)> {
        self.servers
            .iter()
            .flat_map(|s| {
                let safe_server = s.name.replace('-', "_");
                s.resources.iter().map(move |r| {
                    // Sanitize the resource name for use as a function name identifier.
                    let safe_name: String = r
                        .name
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                        .collect();
                    let prefixed = format!("resource__{}__{}", safe_server, safe_name);
                    let raw_desc = r.description.as_deref().unwrap_or("");
                    let mime = r.mime_type.as_deref().unwrap_or("text/plain");
                    let desc = if raw_desc.is_empty() {
                        format!("[{} resource] {} ({})", s.name, r.uri, mime)
                    } else {
                        format!("[{} resource] {} — {} ({})", s.name, raw_desc, r.uri, mime)
                    };
                    // Resources take no arguments — the URI is fixed.
                    let schema = r#"{"type":"object","properties":{}}"#.to_string();
                    (prefixed, desc, schema)
                })
            })
            .collect()
    }

    /// Read an MCP resource synchronously. prefixed_name is "resource__server__name".
    pub fn call_resource(&self, prefixed_name: &str) -> (String, bool) {
        let rest = match prefixed_name.strip_prefix("resource__") {
            Some(r) => r,
            None => return ("not an MCP resource".to_string(), true),
        };
        let sep = match rest.find("__") {
            Some(i) => i,
            None => return (format!("malformed resource name: {prefixed_name}"), true),
        };
        let server_name = &rest[..sep];
        let resource_name = &rest[sep + 2..];

        let server = match self
            .servers
            .iter()
            .find(|s| s.name.replace('-', "_") == server_name)
        {
            Some(s) => s,
            None => return (format!("MCP server '{server_name}' not connected"), true),
        };

        // Match the sanitized resource name back to the original URI.
        let uri = match server.resources.iter().find(|r| {
            let safe: String = r
                .name
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect();
            safe == resource_name
        }) {
            Some(r) => r.uri.clone(),
            None => {
                return (
                    format!("resource '{resource_name}' not found on '{server_name}'"),
                    true,
                )
            }
        };

        let peer = server.peer.clone();
        self.rt.block_on(async {
            let params = ReadResourceRequestParams::new(uri);
            match peer.read_resource(params).await {
                Ok(result) => {
                    let text = resource_contents_to_string(&result.contents);
                    (text, false)
                }
                Err(e) => (format!("MCP resource error: {e}"), true),
            }
        })
    }

    /// Execute an MCP prompt synchronously. prefixed_name is "prompt__server__name".
    pub fn call_prompt(&self, prefixed_name: &str, args_json: &str) -> (String, bool) {
        let rest = match prefixed_name.strip_prefix("prompt__") {
            Some(r) => r,
            None => return ("not an MCP prompt".to_string(), true),
        };
        let sep = match rest.find("__") {
            Some(i) => i,
            None => return (format!("malformed prompt name: {prefixed_name}"), true),
        };
        let server_name = &rest[..sep];
        let prompt_name = &rest[sep + 2..];

        let server = match self
            .servers
            .iter()
            .find(|s| s.name.replace('-', "_") == server_name)
        {
            Some(s) => s,
            None => return (format!("MCP server '{server_name}' not connected"), true),
        };

        let peer = server.peer.clone();
        let original_name: String = server
            .prompts
            .iter()
            .find(|p| p.name.replace('-', "_") == prompt_name)
            .map(|p| p.name.to_string())
            .unwrap_or_else(|| prompt_name.to_string());
        let args_preview = crate::agent::tools_background::utf8_prefix(args_json, 300);
        log::info!("[mcp] → prompt {server_name}::{original_name}  args={args_preview}");
        let t = web_time::Instant::now();
        let args_json = args_json.to_string();

        self.rt.block_on(async {
            // Prompt arguments: Map<String, Value> (JsonObject) per the MCP spec.
            let args: Option<serde_json::Map<String, serde_json::Value>> =
                if args_json.is_empty() {
                    None
                } else {
                    match serde_json::from_str(&args_json) {
                        Ok(m) => Some(m),
                        Err(e) => return (format!("invalid args JSON: {e}"), true),
                    }
                };

            let mut params = GetPromptRequestParams::new(original_name.clone());
            if let Some(a) = args {
                params = params.with_arguments(a);
            }

            let timeout = tokio::time::Duration::from_secs(60);
            match tokio::time::timeout(timeout, peer.get_prompt(params)).await {
                Err(_) => {
                    log::warn!("[mcp] ✗ prompt {server_name}::{original_name} TIMED OUT after {}ms",
                        t.elapsed().as_millis());
                    (format!("MCP prompt timed out after 60s — '{original_name}' on server '{server_name}' did not respond"), true)
                }
                Ok(Ok(result)) => {
                    let text = prompt_messages_to_string(&result.messages);
                    log::info!("[mcp] ✓ prompt {server_name}::{original_name} OK in {}ms — {} chars",
                        t.elapsed().as_millis(), text.len());
                    (text, false)
                }
                Ok(Err(e)) => {
                    log::warn!("[mcp] ✗ prompt {server_name}::{original_name} error in {}ms — {e}",
                        t.elapsed().as_millis());
                    (format!("MCP prompt error: {e}"), true)
                }
            }
        })
    }
}

/// Convert MCP tool result content to a plain string the agent can read.
/// Handles all MCP content types: Text, Image, Resource, Audio, and ResourceLink.
/// Mirrors the pattern from claurst's `mcp_result_to_string`.
fn mcp_content_to_string(content: &[rmcp::model::Content]) -> String {
    use rmcp::model::{RawContent, ResourceContents};

    let parts: Vec<String> = content
        .iter()
        .filter_map(|annotated| {
            match &**annotated {
                RawContent::Text(t) => Some(t.text.clone()),
                RawContent::Image(img) => {
                    // Images can't be rendered in a text agent — give a descriptive placeholder
                    // so the LLM knows something was returned rather than silently dropping it.
                    let preview: String = img.data.chars().take(32).collect();
                    let ellipsis = if img.data.len() > 32 { "…" } else { "" };
                    Some(format!(
                        "[Image: {} | base64 preview: {}{}]",
                        img.mime_type, preview, ellipsis
                    ))
                }
                RawContent::Resource(embedded) => match &embedded.resource {
                    ResourceContents::TextResourceContents { uri, text, .. } => {
                        Some(format!("[Resource: {uri}]\n{text}"))
                    }
                    ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                        let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                        Some(format!("[Binary resource: {uri} ({mime})]"))
                    }
                },
                RawContent::Audio(_) => {
                    Some("[Audio content — not renderable in text agent]".to_string())
                }
                RawContent::ResourceLink(r) => Some(format!("[Resource link: {}]", r.uri)),
            }
        })
        .filter(|s| !s.is_empty())
        .collect();

    parts.join("\n")
}

/// Convert MCP prompt messages to a plain string the agent can read.
fn prompt_messages_to_string(messages: &[rmcp::model::PromptMessage]) -> String {
    use rmcp::model::{PromptMessageContent, PromptMessageRole, ResourceContents};
    messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                PromptMessageRole::User => "User",
                PromptMessageRole::Assistant => "Assistant",
            };
            let content = match &msg.content {
                PromptMessageContent::Text { text } => text.clone(),
                PromptMessageContent::Image { image } => {
                    format!("[Image: {}]", image.mime_type)
                }
                PromptMessageContent::Resource { resource } => match &(**resource).resource {
                    ResourceContents::TextResourceContents { uri, text, .. } => {
                        format!("[Resource: {uri}]\n{text}")
                    }
                    ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                        let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                        format!("[Binary resource: {uri} ({mime})]")
                    }
                },
                PromptMessageContent::ResourceLink { link } => {
                    format!("[Resource link: {}]", link.uri)
                }
            };
            format!("{role}: {content}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a JSON Schema object from MCP prompt arguments so the LLM knows what parameters
/// each prompt accepts when invoking it as a synthetic tool.
fn prompt_args_to_schema(args: Option<&[rmcp::model::PromptArgument]>) -> String {
    let Some(args) = args else {
        return r#"{"type":"object","properties":{}}"#.to_string();
    };
    let mut properties = serde_json::Map::new();
    let mut required: Vec<serde_json::Value> = Vec::new();
    for arg in args {
        let mut prop = serde_json::Map::new();
        prop.insert(
            "type".to_string(),
            serde_json::Value::String("string".to_string()),
        );
        if let Some(desc) = &arg.description {
            prop.insert(
                "description".to_string(),
                serde_json::Value::String(desc.clone()),
            );
        }
        properties.insert(arg.name.clone(), serde_json::Value::Object(prop));
        if arg.required.unwrap_or(false) {
            required.push(serde_json::Value::String(arg.name.clone()));
        }
    }
    let schema = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    serde_json::to_string(&schema)
        .unwrap_or_else(|_| r#"{"type":"object","properties":{}}"#.to_string())
}

/// Convert a list of ResourceContents (from read_resource) to a plain string.
fn resource_contents_to_string(contents: &[rmcp::model::ResourceContents]) -> String {
    use rmcp::model::ResourceContents;
    contents
        .iter()
        .map(|c| match c {
            ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("[Resource: {uri}]\n{text}")
            }
            ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                format!("[Binary resource: {uri} ({mime}) — binary content not renderable in text]")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract a usable Bearer token string from an OAuth token response JSON.
/// Tries common field names in order of prevalence.
fn extract_bearer_token(v: &serde_json::Value) -> Option<String> {
    for field in &["access_token", "accessToken", "token", "id_token"] {
        if let Some(s) = v[field].as_str().filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Connect to an MCP server. `allow_oauth` controls whether the PKCE browser flow
/// may be triggered. Pass `false` during startup so servers that need OAuth are
/// skipped silently (user must click "Connect" in Settings).
pub(crate) async fn connect_server(
    cfg: &McpServerConfig,
    allow_oauth: bool,
) -> anyhow::Result<ConnectedServer> {
    // Load stored OAuth token and attempt refresh if near expiry.
    let stored_token = crate::agent::local_tools::load_mcp_token(&cfg.name);
    let stored_token = if let Some(tok) = stored_token {
        super::oauth::try_refresh_token(&cfg.name, &tok)
            .await
            .or(Some(tok))
    } else {
        None
    };
    let access_token = stored_token.as_ref().and_then(|v| extract_bearer_token(v));

    // RunningService must be kept alive — dropping it cancels the background task via DropGuard.
    let (peer, service): (
        Peer<RoleClient>,
        rmcp::service::RunningService<RoleClient, ()>,
    ) = match &cfg.transport {
        TransportType::CliServer(cli) => {
            let mut cmd = tokio::process::Command::new(&cli.command);
            cmd.no_window().args(&cli.args);
            if let Some(cwd) = &cli.cwd_parameter {
                cmd.current_dir(cwd);
            }

            // Build process environment variables, injecting the OAuth access token
            let mut env_vars = cli.env.clone();
            if let Some(token) = &access_token {
                let name_lower = cfg.name.to_lowercase();
                if name_lower == "github" {
                    if !env_vars.iter().any(|(k, _)| k == "GITHUB_TOKEN") {
                        env_vars.push(("GITHUB_TOKEN".to_string(), token.clone()));
                    }
                }

                // Fallback: fill in any empty env vars with the access token
                for (_, val) in env_vars.iter_mut() {
                    if val.is_empty() {
                        *val = token.clone();
                    }
                }
            }

            for (k, v) in env_vars {
                cmd.env(k, v);
            }

            let (transport, _stderr) = TokioChildProcess::builder(cmd).spawn()?;
            let running = ().serve(transport).await?;
            let peer = running.peer().clone();
            (peer, running)
        }

        TransportType::ServerSentEvents(sse) => {
            use rmcp::transport::streamable_http_client::{
                StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
            };

            // Bearer token precedence: static config > stored OAuth token.
            let mut token = sse.bearer_token.clone().or(access_token);
            let had_cached_token = token.is_some();

            // Probe determines transport type and whether OAuth is required.
            let probe_result = if token.is_none() {
                match super::oauth::probe_for_auth(&sse.url).await {
                    Ok(r) => r,
                    Err(e) => {
                        log::warn!(
                            "[mcp] '{}' probe failed ({e}), trying Streamable HTTP",
                            cfg.name
                        );
                        super::oauth::ProbeResult::Accessible
                    }
                }
            } else {
                log::info!(
                    "[mcp] '{}' has stored token — connecting directly",
                    cfg.name
                );
                super::oauth::ProbeResult::Accessible
            };

            // Run OAuth when the server demands it.
            if let super::oauth::ProbeResult::NeedsOAuth(www_auth) = &probe_result {
                if !allow_oauth {
                    // Startup mode: do not open browser. Signal caller to skip this server.
                    return Err(anyhow::anyhow!("{}", super::MCP_OAUTH_PENDING_ERR));
                }
                log::info!("[mcp] '{}' requires OAuth — starting PKCE flow", cfg.name);
                let meta = super::oauth::discover_oauth_meta(&sse.url, www_auth)
                    .await
                    .map_err(|e| anyhow::anyhow!("[mcp oauth] discovery failed: {e}"))?;

                // Blocking: opens browser, binds a local TCP listener, waits for the
                // redirect, exchanges the code, and stores the token — all in one call.
                // This works in both dev and production without relying on the OS
                // delivering a dragent:// deep-link (which only works in a signed .app bundle).
                let token_json = super::oauth::run_pkce_flow(&cfg.name, &meta)
                    .await
                    .map_err(|e| anyhow::anyhow!("[mcp oauth] PKCE flow failed: {e}"))?;

                crate::agent::local_tools::store_mcp_token(&cfg.name, &token_json)
                    .map_err(|e| anyhow::anyhow!("[mcp oauth] token storage failed: {e}"))?;

                // Re-extract the bearer token so we can attach it to this connection
                // without requiring a full reconnect cycle.
                token = extract_bearer_token(&token_json);
                log::info!(
                    "[mcp oauth] '{}' token stored — continuing connection",
                    cfg.name
                );
            }

            // Legacy SSE transport: server returned 404/405 to POST.
            if matches!(probe_result, super::oauth::ProbeResult::LegacySse) {
                log::info!("[mcp] '{}' using legacy SSE transport", cfg.name);
                let transport = super::legacy_sse::connect(&sse.url, token)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("legacy SSE connect to '{}' failed: {e:#}", cfg.name)
                    })?;
                let running = ().serve(transport).await.map_err(|e| {
                    anyhow::anyhow!("legacy SSE serve '{}' failed: {e:#}", cfg.name)
                })?;
                let peer = running.peer().clone();
                let caps = peer.peer_info().map(|i| i.capabilities.clone());
                let tools = if caps.as_ref().map(|c| c.tools.is_some()).unwrap_or(false) {
                    peer.list_all_tools().await.unwrap_or_else(|e| {
                        log::warn!("[mcp] '{}' tools/list failed: {e}", cfg.name);
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let prompts = if caps.as_ref().map(|c| c.prompts.is_some()).unwrap_or(false) {
                    peer.list_all_prompts().await.unwrap_or_else(|e| {
                        log::warn!("[mcp] '{}' prompts/list failed: {e}", cfg.name);
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                let resources = if caps
                    .as_ref()
                    .map(|c| c.resources.is_some())
                    .unwrap_or(false)
                {
                    peer.list_all_resources().await.unwrap_or_else(|e| {
                        log::warn!("[mcp] '{}' resources/list failed: {e}", cfg.name);
                        Vec::new()
                    })
                } else {
                    Vec::new()
                };
                log::info!(
                    "[mcp] '{}' via legacy SSE ({} tools, {} prompts, {} resources)",
                    cfg.name,
                    tools.len(),
                    prompts.len(),
                    resources.len()
                );
                return Ok(ConnectedServer {
                    name: cfg.name.clone(),
                    peer,
                    tools,
                    prompts,
                    resources,
                    _service: running,
                });
            }

            // Streamable HTTP transport (default).
            let make_transport = |t: Option<&String>| {
                let mut config = StreamableHttpClientTransportConfig::with_uri(sse.url.as_str())
                    .reinit_on_expired_session(true);
                if let Some(tok) = t {
                    // auth_header expects the raw token; the transport prepends "Bearer "
                    config = config.auth_header(tok.clone());
                }
                StreamableHttpClientTransport::from_config(config)
            };

            match ().serve(make_transport(token.as_ref())).await {
                Ok(running) => {
                    let peer = running.peer().clone();
                    (peer, running)
                }
                Err(e) if had_cached_token => {
                    log::warn!(
                        "[mcp] '{}' connection with cached token failed ({e:#}) — \
                         clearing token so user can re-authorize",
                        cfg.name
                    );
                    crate::agent::local_tools::clear_mcp_token(&cfg.name);
                    return Err(anyhow::anyhow!(
                        "stored token rejected by '{}' — click Connect to re-authorize",
                        cfg.name
                    ));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "connection to '{}' failed: {e:#}",
                        cfg.name
                    ))
                }
            }
        }
    };

    let caps = peer.peer_info().map(|i| i.capabilities.clone());

    let tools = if caps.as_ref().map(|c| c.tools.is_some()).unwrap_or(false) {
        peer.list_all_tools().await.unwrap_or_else(|e| {
            log::warn!("[mcp] '{}' tools/list failed: {e}", cfg.name);
            Vec::new()
        })
    } else {
        log::info!("[mcp] '{}' did not advertise tool capability", cfg.name);
        Vec::new()
    };

    let prompts = if caps.as_ref().map(|c| c.prompts.is_some()).unwrap_or(false) {
        peer.list_all_prompts().await.unwrap_or_else(|e| {
            log::warn!("[mcp] '{}' prompts/list failed: {e}", cfg.name);
            Vec::new()
        })
    } else {
        Vec::new()
    };

    let resources = if caps
        .as_ref()
        .map(|c| c.resources.is_some())
        .unwrap_or(false)
    {
        peer.list_all_resources().await.unwrap_or_else(|e| {
            log::warn!("[mcp] '{}' resources/list failed: {e}", cfg.name);
            Vec::new()
        })
    } else {
        Vec::new()
    };

    log::info!(
        "[mcp] connected to '{}' ({} tools, {} prompts, {} resources)",
        cfg.name,
        tools.len(),
        prompts.len(),
        resources.len()
    );
    Ok(ConnectedServer {
        name: cfg.name.clone(),
        peer,
        tools,
        prompts,
        resources,
        _service: service,
    })
}
