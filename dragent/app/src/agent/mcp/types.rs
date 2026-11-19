use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level entry in mcp_servers.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub transport: TransportType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransportType {
    CliServer(CliConfig),
    ServerSentEvents(SseConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd_parameter: Option<PathBuf>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseConfig {
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
}
