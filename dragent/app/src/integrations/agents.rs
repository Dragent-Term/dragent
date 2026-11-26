//! Dragent's tools (`dragent mcp`) belong to agents started from Dragent:
//! each gets them at launch, for that session only, and the bridge refuses
//! to serve anywhere else. Nothing is written into other agents' own MCP
//! settings. Earlier builds added the bridge there; a sync takes those
//! entries back out.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Provider;
use crate::agent::mcp_agent_sync::SyncAgent;
use crate::settings::{McpServerEntry, McpTransport};

/// The server's name in the agents' settings: its tools appear as
/// `mcp__dragent__search` and the like.
pub const BRIDGE_NAME: &str = "dragent";

/// The agents whose own settings hold Dragent's bridge, as written by
/// Dragent (its sync record), by name.
fn agents_with_bridge() -> std::collections::BTreeSet<String> {
    let record = dragent_paths::home().join("mcp-agent-sync.json");
    let Some(value) = std::fs::read(record)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return Default::default();
    };
    value
        .get("agents")
        .and_then(|agents| agents.as_object())
        .map(|agents| {
            agents
                .iter()
                .filter(|(_, names)| {
                    names.as_array().is_some_and(|names| names.iter().any(|name| name == BRIDGE_NAME))
                })
                .map(|(key, _)| SyncAgent::from_key(key).map_or_else(|| key.clone(), |agent| agent.label().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Something agents could use is connected. GitHub doesn't count: agents
/// already use the GitHub CLI themselves.
pub(crate) fn any_connected() -> bool {
    Provider::ALL
        .into_iter()
        .filter(|provider| *provider != Provider::Github)
        .any(|provider| super::connection(provider).is_some())
}

/// Rewrite the agents' MCP settings with the user's servers (taking out a
/// bridge an earlier build wrote), off the calling thread. A Dev build never touches other apps'
/// settings: it leaves the bridge (and their servers) alone.
pub fn sync_in_background() {
    if dragent_paths::is_dev() {
        log::info!("[integrations] Dev build: leaving other apps' MCP settings alone");
        return;
    }
    std::thread::spawn(|| {
        let Some(entries) = user_servers() else {
            log::warn!(
                "[integrations] mcp.json couldn't be read: agents' MCP settings left as they are"
            );
            return;
        };
        let before = agents_with_bridge();
        let report = crate::agent::mcp_agent_sync::sync_agents(&entries);
        if let Some(summary) = report.summary() {
            log::info!("[integrations] {summary}");
        }
        let after = agents_with_bridge();
        let removed: Vec<_> = before.difference(&after).cloned().collect();
        if !removed.is_empty() {
            crate::cli::agent_skill::add_cleanup_note(&format!(
                "Removed Dragent's tools from the settings of {}.",
                removed.join(", ")
            ));
        }
        // Each connected machine takes out what it wrote into its agents'
        // settings too; its tools service keeps running for its sessions.
        for machine in crate::remote_fs::connected_machines() {
            if let Ok(client) = crate::remote_fs::client(&machine) {
                if let Err(error) = client.coordination(
                    "host_mcp_configure",
                    serde_json::json!({"enabled": true}),
                    serde_json::to_value(crate::cli::team::Caller::person()).unwrap(),
                ) {
                    log::warn!("[integrations] remote MCP registration: {error}");
                }
            }
        }
    });
}

/// The servers in Settings → MCP servers; `None` when their file exists
/// but can't be read, so a sync doesn't take them out of every agent.
fn user_servers() -> Option<Vec<McpServerEntry>> {
    let path = crate::settings::mcp_config::global_mcp_path();
    match std::fs::read_to_string(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .map(|_| crate::settings::mcp_config::load_from_path(&path)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_an_earlier_build_wrote_is_taken_out_and_the_persons_servers_stay() {
        let home = tempfile::tempdir().unwrap();
        let record = home.path().join("sync.json");
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        let server = |name: &str| McpServerEntry {
            name: name.to_string(),
            command: format!("/bin/{name}"),
            transport: McpTransport::Stdio,
            args: Vec::new(),
            env: Default::default(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: vec!["codex".into()],
        };
        // What an earlier build did: the bridge beside the person's server.
        dragent_host::mcp_sync::sync(&[server("mine"), server(BRIDGE_NAME)], home.path(), &record);
        let config = || std::fs::read_to_string(home.path().join(".codex/config.toml")).unwrap();
        assert!(config().contains("[mcp_servers.dragent]"));
        // Now: only the person's servers are synced.
        dragent_host::mcp_sync::sync(&[server("mine")], home.path(), &record);
        assert!(!config().contains("[mcp_servers.dragent]"), "{}", config());
        assert!(config().contains("[mcp_servers.mine]"));
    }
}
