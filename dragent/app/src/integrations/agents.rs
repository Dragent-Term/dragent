//! Connected services for other coding agents: the `dragent mcp` bridge in
//! their MCP settings (Claude Code, Codex, Cursor, Gemini, Qwen, OpenCode),
//! written through `agent::mcp_agent_sync` like the servers in Settings →
//! MCP servers. Coordination tools are available even without a connected
//! service; the user can turn agent tools off for this machine.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Provider;
use crate::agent::mcp_agent_sync::SyncAgent;
use crate::settings::{McpServerEntry, McpTransport};

/// The server's name in the agents' settings: its tools appear as
/// `mcp__dragent__search` and the like.
pub const BRIDGE_NAME: &str = "dragent";

const RECORD_FILE: &str = "integrations.json";

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    #[serde(default = "on")]
    agent_tools: bool,
}

fn on() -> bool {
    true
}

fn record_path() -> PathBuf {
    dragent_paths::home().join(RECORD_FILE)
}

/// Whether other agents get the connected services (on unless turned off).
pub fn enabled() -> bool {
    std::fs::read(record_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
        .is_none_or(|record| record.agent_tools)
}

/// Turn it on or off, then bring the agents' settings in line.
pub fn set_enabled(on: bool) {
    let path = record_path();
    match serde_json::to_vec_pretty(&Record { agent_tools: on }) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&path, bytes) {
                log::warn!("[integrations] couldn't save {}: {error}", path.display());
            }
        }
        Err(error) => log::warn!("[integrations] {error}"),
    }
    sync_in_background();
}

/// Something agents could use is connected. GitHub doesn't count: agents
/// already use the GitHub CLI themselves.
pub(crate) fn any_connected() -> bool {
    Provider::ALL
        .into_iter()
        .filter(|provider| *provider != Provider::Github)
        .any(|provider| super::connection(provider).is_some())
}

/// How agents start the bridge: the `dragent` launcher, whose path stays
/// put while the app moves or updates. On Windows, where agents can't start
/// a `.cmd` file directly, the app itself.
fn bridge_command() -> Option<(String, Vec<String>)> {
    if cfg!(windows) {
        let exe = std::env::current_exe().ok()?;
        return Some((
            exe.to_string_lossy().to_string(),
            vec![crate::cli::CLI_ARG.to_string(), "mcp".to_string()],
        ));
    }
    let launcher = crate::cli::user_bin_dir().join("dragent");
    launcher.is_file().then(|| {
        (
            launcher.to_string_lossy().to_string(),
            vec!["mcp".to_string()],
        )
    })
}

/// The bridge as a server for every agent present under `home`, when
/// it's wanted.
pub(crate) fn bridge_entry(home: &Path) -> Option<McpServerEntry> {
    if !enabled() {
        return None;
    }
    let (command, args) = bridge_command()?;
    entry(command, args, home)
}

fn entry(command: String, args: Vec<String>, home: &Path) -> Option<McpServerEntry> {
    let agents: Vec<String> = SyncAgent::ALL
        .into_iter()
        .filter(|agent| agent.is_present(home))
        .map(|agent| agent.key().to_string())
        .collect();
    (!agents.is_empty()).then(|| McpServerEntry {
        name: BRIDGE_NAME.to_string(),
        command,
        transport: McpTransport::Stdio,
        args,
        env: Default::default(),
        enabled: true,
        bearer_token: None,
        auth_url: None,
        token_url: None,
        oauth_scope: None,
        sync_agents: agents,
    })
}

/// Rewrite the agents' MCP settings with the user's servers and the
/// bridge, off the calling thread. A Dev build never touches other apps'
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
        let report = crate::agent::mcp_agent_sync::sync_agents(&entries);
        if let Some(summary) = report.summary() {
            log::info!("[integrations] {summary}");
        }
        // Each connected machine's service bridge keeps running and follows
        // the setting itself; restarting it would fail calls in flight.
        let enabled = enabled();
        for machine in crate::remote_fs::connected_machines() {
            if let Ok(client) = crate::remote_fs::client(&machine) {
                if let Err(error) = client.coordination(
                    "host_mcp_configure",
                    serde_json::json!({"enabled": enabled}),
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
    fn the_bridge_goes_only_to_agents_used_here() {
        let home = tempfile::tempdir().unwrap();
        assert!(entry("dragent".into(), vec!["mcp".into()], home.path()).is_none());
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".claude.json"), "{}").unwrap();
        let entry = entry(
            "/home/a/.dragent/bin/dragent".into(),
            vec!["mcp".into()],
            home.path(),
        )
        .unwrap();
        assert_eq!(entry.name, "dragent");
        assert_eq!(entry.sync_agents, ["claude", "codex"]);
        assert_eq!(entry.display_command(), "/home/a/.dragent/bin/dragent mcp");
    }

    #[test]
    fn dev_builds_sync_no_bridge_into_other_agents() {
        // Test builds are the Dev edition. With an agent present and a
        // launcher that would build the bridge entry, the background sync
        // must still write nothing into the home folder.
        if !dragent_paths::is_dev() {
            return;
        } // only meaningful in the Dev edition
        let home = tempfile::tempdir().unwrap();
        let dev_home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".codex")).unwrap();
        crate::test_env::with_vars(
            &[
                ("HOME", Some(home.path())),
                (dragent_paths::DEV_HOME_ENV, Some(dev_home.path())),
            ],
            || {
                std::fs::create_dir_all(crate::cli::user_bin_dir()).unwrap();
                std::fs::write(crate::cli::user_bin_dir().join("dragent"), "#!/bin/sh\n").unwrap();
                sync_in_background();
                // No thread is spawned in Dev, so no write can be in flight;
                // poll anyway so a removed guard fails instead of passing.
                for _ in 0..20 {
                    if home.path().join(".codex/config.toml").exists() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                assert!(!home.path().join(".codex/config.toml").exists());
            },
        );
    }
}
