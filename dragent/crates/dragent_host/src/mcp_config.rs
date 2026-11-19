//! MCP server settings shared by the desktop and headless host.
use serde::{Deserialize, Serialize};
fn default_true() -> bool {
    true
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    Sse,
}

impl McpTransport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "SSE",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerEntry {
    pub name: String,
    /// stdio: the executable to run. SSE: the HTTP endpoint URL.
    pub command: String,
    #[serde(default)]
    pub transport: McpTransport,
    /// Extra CLI arguments (stdio only).
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables injected into the server process (stdio only).
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Whether this server is active. Disabled servers are saved but not started.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional Bearer token for SSE servers that require HTTP Authorization headers.
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// OAuth 2.0 authorization endpoint (SSE servers that use browser-based auth).
    #[serde(default)]
    pub auth_url: Option<String>,
    /// OAuth 2.0 token exchange endpoint.
    #[serde(default)]
    pub token_url: Option<String>,
    /// Space-separated OAuth scopes to request (e.g. "read write").
    #[serde(default)]
    pub oauth_scope: Option<String>,
    /// Other coding agents this server is also written into (their keys,
    /// e.g. "claude", "codex"); see `agent::mcp::agent_sync`.
    #[serde(default)]
    pub sync_agents: Vec<String>,
}

impl McpServerEntry {
    /// Full command string for display (command + args joined).
    pub fn display_command(&self) -> String {
        if self.args.is_empty() {
            self.command.clone()
        } else {
            format!("{} {}", self.command, self.args.join(" "))
        }
    }
}
