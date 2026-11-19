use serde::{Deserialize, Serialize};
/// MCP server configuration stored in ~/.dragent/mcp.json.
///
/// Shared MCP config format so users can copy configs between tools:
///   { "mcpServers": { "name": { "url": "https://..." } } }
///   { "mcpServers": { "name": { "command": "npx", "args": [...], "env": {...} } } }
///
/// The file is the single source of truth for MCP servers. The old
/// app_settings.json mcp_servers field is migrated here on first load.
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::app_settings::{McpServerEntry, McpTransport};

// ── On-disk schema ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpFileDef {
    /// HTTP/SSE endpoint URL — present for remote (SSE/HTTP) servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Executable — present for stdio (local process) servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// CLI arguments (stdio servers only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,

    /// Environment variables injected into the process (stdio servers only).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// Static Bearer token for HTTP servers that don't use browser OAuth.
    #[serde(
        rename = "bearerToken",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bearer_token: Option<String>,

    /// Set true to keep the entry saved without connecting.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,

    /// Other coding agents Dragent also writes this server into.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpFile {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: BTreeMap<String, McpFileDef>,
}

// ── File paths ────────────────────────────────────────────────────────────────

/// ~/.dragent/mcp.json — global user-level MCP config.
pub fn global_mcp_path() -> PathBuf {
    dragent_paths::home().join("mcp.json")
}

// ── Load / save ───────────────────────────────────────────────────────────────

/// Load all enabled MCP server entries from ~/.dragent/mcp.json.
/// Order follows alphabetical key order (BTreeMap).
pub fn load_entries() -> Vec<McpServerEntry> {
    load_from_path(&global_mcp_path())
}

/// Load entries from a specific mcp.json path (used for project-level loading).
pub fn load_from_path(path: &std::path::Path) -> Vec<McpServerEntry> {
    let file: McpFile = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    file.mcp_servers
        .into_iter()
        .map(|(name, def)| def_to_entry(name, def))
        .collect()
}

/// Write the given entries back to ~/.dragent/mcp.json.
pub fn save_entries(entries: &[McpServerEntry]) {
    let path = global_mcp_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut mcp_servers = BTreeMap::new();
    for entry in entries {
        mcp_servers.insert(entry.name.clone(), entry_to_def(entry));
    }
    let file = McpFile { mcp_servers };
    if let Ok(json) = serde_json::to_string_pretty(&file) {
        let _ = std::fs::write(path, json);
    }
}

/// One-time migration: if ~/.dragent/mcp.json doesn't exist yet but
/// app_settings.json had servers, copy them across.
pub fn migrate_from_app_settings(old_servers: &[McpServerEntry]) {
    let path = global_mcp_path();
    if path.exists() || old_servers.is_empty() {
        return;
    }
    save_entries(old_servers);
    log::info!(
        "[mcp] migrated {} server(s) from app_settings.json → ~/.dragent/mcp.json",
        old_servers.len()
    );
}

// ── Conversions ───────────────────────────────────────────────────────────────

fn def_to_entry(name: String, def: McpFileDef) -> McpServerEntry {
    if let Some(url) = def.url {
        McpServerEntry {
            name,
            command: url,
            transport: McpTransport::Sse,
            args: vec![],
            env: Default::default(),
            enabled: !def.disabled,
            bearer_token: def.bearer_token,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: def.agents,
        }
    } else {
        McpServerEntry {
            name,
            command: def.command.unwrap_or_default(),
            transport: McpTransport::Stdio,
            args: def.args,
            env: def.env.into_iter().collect(),
            enabled: !def.disabled,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: def.agents,
        }
    }
}

fn entry_to_def(entry: &McpServerEntry) -> McpFileDef {
    match entry.transport {
        McpTransport::Sse => McpFileDef {
            url: Some(entry.command.clone()),
            command: None,
            args: vec![],
            env: BTreeMap::new(),
            bearer_token: entry.bearer_token.clone(),
            disabled: !entry.enabled,
            agents: entry.sync_agents.clone(),
        },
        McpTransport::Stdio => McpFileDef {
            url: None,
            command: Some(entry.command.clone()),
            args: entry.args.clone(),
            env: entry
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            bearer_token: None,
            disabled: !entry.enabled,
            agents: entry.sync_agents.clone(),
        },
    }
}
