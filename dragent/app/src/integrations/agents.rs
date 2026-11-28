//! Dragent's tools (`dragent mcp`) belong to agents started from Dragent.
//! Claude Code, Codex and OpenCode get them at launch, for that session
//! only. Cursor, Gemini and Qwen read MCP servers only from their own
//! settings file, so they get them only after the person allows it for that
//! agent: one `dragent` entry is written there, and outside Dragent it
//! answers as a server with no tools. Withdrawing takes it back out; an
//! entry of the person's own by that name is never touched. Bridges an
//! earlier build wrote unasked are taken out by the same sync.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Provider;
use crate::agent::mcp_agent_sync::SyncAgent;
use crate::settings::{McpServerEntry, McpTransport};

/// The server's name in the agents' settings: its tools appear as
/// `mcp__dragent__search` and the like.
pub const BRIDGE_NAME: &str = "dragent";

/// Agents that read MCP servers only from their own settings file: they get
/// Dragent's tools only after the person allows it for that agent.
pub const NEEDS_OWN_SETTINGS: [SyncAgent; 3] = [SyncAgent::Cursor, SyncAgent::Gemini, SyncAgent::Qwen];

/// Set in the entry written into an agent's settings: outside Dragent the
/// bridge then answers with no tools instead of failing in the agent.
pub const FROM_AGENT_SETTINGS_ENV: &str = "DRAGENT_MCP_FROM_AGENT_SETTINGS";

/// Agents that take Dragent's tools at launch, with nothing written.
pub fn takes_tools_at_launch(agent: SyncAgent) -> bool {
    matches!(agent, SyncAgent::ClaudeCode | SyncAgent::Codex | SyncAgent::OpenCode)
}

fn allowed_path(dir: &Path) -> PathBuf {
    dir.join("agent-tools-allowed.json")
}

#[derive(Default, Serialize, Deserialize)]
struct Allowed {
    #[serde(default)]
    agents: Vec<String>,
}

/// The agents the person allowed Dragent's entry in their own settings.
pub fn allowed() -> std::collections::BTreeSet<SyncAgent> {
    allowed_in(&dragent_paths::home())
}

/// Whether the person ever used the per-agent switch (their withdrawals
/// aren't reported as cleanups).
fn chosen() -> bool {
    allowed_path(&dragent_paths::home()).is_file()
}

fn allowed_in(dir: &Path) -> std::collections::BTreeSet<SyncAgent> {
    std::fs::read(allowed_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Allowed>(&bytes).ok())
        .unwrap_or_default()
        .agents
        .iter()
        .filter_map(|key| SyncAgent::from_key(key))
        .filter(|agent| NEEDS_OWN_SETTINGS.contains(agent))
        .collect()
}

fn set_allowed_in(dir: &Path, agent: SyncAgent, on: bool) -> Result<(), String> {
    if !NEEDS_OWN_SETTINGS.contains(&agent) {
        return Err(format!("{} gets Dragent's tools at launch; nothing to allow.", agent.label()));
    }
    let mut agents = allowed_in(dir);
    if on {
        agents.insert(agent);
    } else {
        agents.remove(&agent);
    }
    let record = Allowed { agents: agents.iter().map(|agent| agent.key().to_string()).collect() };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = allowed_path(dir);
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
}

/// Allow or withdraw Dragent's entry in `agent`'s own settings, then bring
/// the agents' settings in line. Answers what went wrong, if anything.
#[cfg(not(target_family = "wasm"))]
pub fn set_allowed(agent: SyncAgent, on: bool) -> Result<(), String> {
    set_allowed_in(&dragent_paths::home(), agent, on)?;
    let entries = user_servers().ok_or("Your MCP servers file couldn't be read, so other agents' settings were left as they are.")?;
    let report = crate::agent::mcp_agent_sync::sync_agents(&entries);
    if let Some(summary) = report.summary() {
        log::info!("[integrations] {summary}");
    }
    match report.problems.is_empty() {
        true => Ok(()),
        false => Err(report.problems.join("\n")),
    }
}

/// What tells Dragent's bridge which pane and workspace it serves. Cursor
/// starts its servers with only a few fixed variables, so its entry names
/// these to be copied from Cursor's own (`${env:…}`; outside Dragent
/// Cursor leaves the text as it is, which counts as unset).
const CURSOR_PASSES: [&str; 2] = ["DRAGENT_HOST_TERMINAL_ID", "DRAGENT_HOST_WORKSPACE_ID"];

/// Dragent's entries for the agents in `allowed`, run through `launcher`:
/// one shared, plus Cursor's own. None when nobody allowed it, the launcher
/// is missing, or the person has a server of their own by that name
/// (theirs wins).
pub(crate) fn bridge_entries(
    allowed: &std::collections::BTreeSet<SyncAgent>,
    launcher: Option<&Path>,
    servers: &[McpServerEntry],
) -> Vec<McpServerEntry> {
    let Some(launcher) = launcher else { return Vec::new() };
    if servers.iter().any(|server| server.name == BRIDGE_NAME) {
        return Vec::new();
    }
    let entry = |agents: Vec<SyncAgent>, passes: &[&str]| {
        let mut env: std::collections::HashMap<String, String> =
            [(FROM_AGENT_SETTINGS_ENV.to_string(), "1".to_string())].into_iter().collect();
        for name in passes {
            env.insert(name.to_string(), format!("${{env:{name}}}"));
        }
        McpServerEntry {
            name: BRIDGE_NAME.into(),
            command: launcher.to_string_lossy().into_owned(),
            transport: McpTransport::Stdio,
            args: vec!["mcp".into()],
            env,
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: agents.iter().map(|agent| agent.key().to_string()).collect(),
        }
    };
    let (cursor, others): (Vec<_>, Vec<_>) =
        allowed.iter().copied().partition(|agent| *agent == SyncAgent::Cursor);
    let mut entries = Vec::new();
    if !others.is_empty() {
        entries.push(entry(others, &[]));
    }
    if !cursor.is_empty() {
        entries.push(entry(cursor, &CURSOR_PASSES));
    }
    entries
}

/// The person's servers plus Dragent's entry for the agents they allowed.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn with_bridge(servers: &[McpServerEntry]) -> Vec<McpServerEntry> {
    let launcher = dragent_host::agent_skills::dragent_launcher();
    let mut all = servers.to_vec();
    all.extend(bridge_entries(&allowed(), launcher.as_deref(), servers));
    all
}

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
        // A withdrawal the person made isn't a cleanup to report.
        let theirs: Vec<&str> = if chosen() { NEEDS_OWN_SETTINGS.iter().map(|agent| agent.label()).collect() } else { Vec::new() };
        let removed: Vec<_> = before.difference(&after).filter(|label| !theirs.contains(&label.as_str())).cloned().collect();
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
pub(crate) fn user_servers() -> Option<Vec<McpServerEntry>> {
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

    #[test]
    fn only_agents_that_need_their_own_settings_can_be_allowed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(allowed_in(dir.path()).is_empty());
        set_allowed_in(dir.path(), SyncAgent::Cursor, true).unwrap();
        set_allowed_in(dir.path(), SyncAgent::Gemini, true).unwrap();
        assert!(set_allowed_in(dir.path(), SyncAgent::ClaudeCode, true).is_err());
        set_allowed_in(dir.path(), SyncAgent::Gemini, false).unwrap();
        assert_eq!(allowed_in(dir.path()), [SyncAgent::Cursor].into_iter().collect());
    }

    #[test]
    fn an_allowed_agent_gets_one_entry_and_withdrawing_takes_it_out() {
        let home = tempfile::tempdir().unwrap();
        let record = home.path().join("sync.json");
        std::fs::create_dir(home.path().join(".cursor")).unwrap();
        std::fs::create_dir(home.path().join(".gemini")).unwrap();
        // The person's own server by that name, in Gemini: never touched.
        std::fs::write(
            home.path().join(".gemini/settings.json"),
            r#"{"mcpServers":{"dragent":{"command":"/mine"}}}"#,
        )
        .unwrap();
        let launcher = std::path::Path::new("/opt/dragent/bin/dragent");
        let both: std::collections::BTreeSet<_> = [SyncAgent::Cursor, SyncAgent::Gemini].into_iter().collect();
        assert!(bridge_entries(&Default::default(), Some(launcher), &[]).is_empty());
        assert!(bridge_entries(&both, None, &[]).is_empty());
        let entries = bridge_entries(&both, Some(launcher), &[]);
        assert_eq!(entries.len(), 2, "Cursor's entry is its own");
        dragent_host::mcp_sync::sync(&entries, home.path(), &record);
        let read = |file: &str| {
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(home.path().join(file)).unwrap()).unwrap()
        };
        let cursor = read(".cursor/mcp.json");
        assert_eq!(cursor["mcpServers"]["dragent"]["command"], "/opt/dragent/bin/dragent");
        assert_eq!(cursor["mcpServers"]["dragent"]["args"][0], "mcp");
        assert_eq!(cursor["mcpServers"]["dragent"]["env"][FROM_AGENT_SETTINGS_ENV], "1");
        assert_eq!(
            cursor["mcpServers"]["dragent"]["env"]["DRAGENT_HOST_TERMINAL_ID"],
            "${env:DRAGENT_HOST_TERMINAL_ID}"
        );
        assert_eq!(read(".gemini/settings.json")["mcpServers"]["dragent"]["command"], "/mine");
        // Withdrawn: nothing allowed, so no entry, and Dragent's comes out.
        dragent_host::mcp_sync::sync(&[], home.path(), &record);
        assert!(read(".cursor/mcp.json")["mcpServers"].get("dragent").is_none());
        assert_eq!(read(".gemini/settings.json")["mcpServers"]["dragent"]["command"], "/mine");
    }

    #[test]
    fn a_server_of_the_persons_own_named_dragent_wins() {
        let mine = McpServerEntry {
            name: BRIDGE_NAME.into(),
            command: "/mine".into(),
            transport: McpTransport::Stdio,
            args: Vec::new(),
            env: Default::default(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: vec!["cursor".into()],
        };
        let cursor: std::collections::BTreeSet<_> = [SyncAgent::Cursor].into_iter().collect();
        assert!(bridge_entries(&cursor, Some(std::path::Path::new("/d")), &[mine]).is_empty());
    }
}
