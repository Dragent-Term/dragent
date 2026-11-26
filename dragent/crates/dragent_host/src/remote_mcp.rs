//! Dragent's tools on a remote machine. Agents started there get the
//! context-neutral dispatcher at launch, for that session only (see
//! [`launcher`]); nothing is written into the agents' own settings. Earlier
//! builds registered it there: "registering" now takes out only the entries
//! this installation wrote, with the desktop's exact config editor.
use crate::mcp_config::{McpServerEntry, McpTransport};
use crate::mcp_sync::{SyncAgent, SyncReport};
use std::path::{Path, PathBuf};

fn root(home: &Path) -> PathBuf {
    home.join(".dragent/remote")
}
fn dispatcher(home: &Path) -> PathBuf {
    root(home).join("bin/dragent-mcp")
}

const DISPATCH: &str = r#"#!/bin/sh
set -eu
if [ -z "${DRAGENT_REMOTE_HOST_BINARY:-}" ] || [ -z "${DRAGENT_HOME:-}" ] || [ -z "${DRAGENT_HOST_ENDPOINT_FILE:-}" ]; then
  echo 'Start this agent in its Dragent SSH workspace to select the correct organisation.' >&2
  exit 1
fi
exec "$DRAGENT_REMOTE_HOST_BINARY" --dragent-organisation=local --dragent-cli mcp
"#;

fn install_dispatcher(home: &Path) -> Result<(), String> {
    let path = dispatcher(home);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(uuid::Uuid::new_v4().to_string());
    std::fs::write(&temporary, DISPATCH).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temporary, &path).map_err(|e| e.to_string())
}

/// The dispatcher agents started on this machine get as their tools server.
pub fn launcher() -> Option<PathBuf> {
    std::env::var_os("DRAGENT_REMOTE_HOST_BINARY")?;
    let home = dirs::home_dir()?;
    install_dispatcher(&home).ok()?;
    Some(dispatcher(&home))
}

/// What the dispatcher reads from its session, for an agent (Codex) that
/// starts tools servers with only the variables it is told to pass.
pub const DISPATCH_ENV: [&str; 3] = ["DRAGENT_REMOTE_HOST_BINARY", "DRAGENT_HOME", "DRAGENT_HOST_ENDPOINT_FILE"];

pub fn register(selected: &[String]) -> Result<SyncReport, String> {
    if std::env::var_os("DRAGENT_REMOTE_HOST_BINARY").is_none() {
        return Ok(SyncReport::default());
    }
    let home = dirs::home_dir().ok_or("Couldn't locate the remote account's home")?;
    register_at(&home, selected)
}

/// Register inside the selected account's config using the same ownership rules.
pub fn register_account(binding: &crate::agent_accounts::Binding) -> Result<SyncReport, String> {
    if std::env::var_os("DRAGENT_REMOTE_HOST_BINARY").is_none() {
        return Ok(SyncReport::default());
    }
    let mut command = std::process::Command::new("unused");
    crate::agent_accounts::configure(&mut command, binding)?;
    let directory = command.get_envs().find_map(|(key, value)| {
        (key == "CLAUDE_CONFIG_DIR" || key == "CODEX_HOME")
            .then(|| value.map(PathBuf::from))
            .flatten()
    });
    let Some(directory) = directory else {
        return register(&[binding.provider.clone()]);
    };
    let home = dirs::home_dir().ok_or("Couldn't locate the remote account's home")?;
    install_dispatcher(&home)?;
    let agent = match binding.provider.as_str() {
        "claude" => SyncAgent::ClaudeCode,
        "codex" => SyncAgent::Codex,
        _ => return Err("Account MCP adapter unavailable".into()),
    };
    let path = directory.join(if agent == SyncAgent::ClaudeCode {
        ".claude.json"
    } else {
        "config.toml"
    });
    let entry = entry_at(&home, vec![agent.key().into()]);
    let report = crate::mcp_sync::sync_paths(
        &[entry],
        &home,
        &directory.join("dragent-mcp-sync.json"),
        &[agent],
        &[(agent.key().to_string(), path)].into_iter().collect(),
    );
    if !report.problems.is_empty() {
        return Err(report.problems.join("\n"));
    }
    Ok(report)
}

fn entry_at(home: &Path, agents: Vec<String>) -> McpServerEntry {
    McpServerEntry {
        name: "dragent".into(),
        command: dispatcher(home).to_string_lossy().into_owned(),
        transport: McpTransport::Stdio,
        args: Vec::new(),
        env: Default::default(),
        // Never written: a sync with it off removes what was written before.
        enabled: false,
        bearer_token: None,
        auth_url: None,
        token_url: None,
        oauth_scope: None,
        sync_agents: agents,
    }
}

fn register_at(home: &Path, selected: &[String]) -> Result<SyncReport, String> {
    install_dispatcher(home)?;
    let selected_agents: Vec<_> = SyncAgent::ALL
        .into_iter()
        .filter(|agent| {
            if selected.is_empty() {
                agent.is_present(home)
            } else {
                selected.iter().any(|key| {
                    agent.key() == key || (*agent == SyncAgent::Cursor && key == "cursor-agent")
                })
            }
        })
        .collect();
    let agents = selected_agents
        .iter()
        .map(|agent| agent.key().to_string())
        .collect();
    let entry = entry_at(home, agents);
    let report = crate::mcp_sync::sync_selected(
        &[entry],
        home,
        &root(home).join("mcp-agent-sync.json"),
        &selected_agents,
    );
    if !report.problems.is_empty() {
        return Err(report.problems.join("\n"));
    }
    Ok(report)
}

pub fn enabled() -> bool {
    dirs::home_dir()
        .map(|home| enabled_at(&home))
        .unwrap_or(false)
}

fn enabled_at(home: &Path) -> bool {
    std::fs::read(root(home).join("preferences.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["enabled"].as_bool())
        .unwrap_or(true)
}

/// Disabling removes only entries owned by this remote installation. Existing
/// user MCP servers and other settings remain intact, as on the desktop.
pub fn configure(enabled: bool) -> Result<SyncReport, String> {
    let home = dirs::home_dir().ok_or("Couldn't locate the remote account's home")?;
    configure_at(&home, enabled)
}
fn configure_at(home: &Path, enabled: bool) -> Result<SyncReport, String> {
    let folder = root(home);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join("preferences.json");
    let temporary = path.with_extension(uuid::Uuid::new_v4().to_string());
    std::fs::write(
        &temporary,
        serde_json::json!({"enabled":enabled}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
    register_at(home, &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn entries_an_earlier_build_wrote_are_taken_out_and_user_servers_stay() {
        let home = std::env::temp_dir().join(format!("dragent-mcp-cleanup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"keep":{"command":"keep"}}}"#).unwrap();
        // What an earlier build wrote, with this installation's ownership record.
        let mut written = entry_at(&home, vec!["claude".into(), "codex".into()]);
        written.enabled = true;
        let record = root(&home).join("mcp-agent-sync.json");
        crate::mcp_sync::sync_selected(&[written], &home, &record, &[SyncAgent::ClaudeCode, SyncAgent::Codex]);
        let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert!(read()["mcpServers"].get("dragent").is_some());
        register_at(&home, &[]).unwrap();
        assert!(read()["mcpServers"].get("dragent").is_none());
        assert_eq!(read()["mcpServers"]["keep"]["command"], "keep");
        assert!(!std::fs::read_to_string(home.join(".codex/config.toml")).unwrap_or_default().contains("mcp_servers.dragent"));
        // A start writes nothing new.
        register_at(&home, &["claude".into()]).unwrap();
        configure_at(&home, true).unwrap();
        assert!(read()["mcpServers"].get("dragent").is_none());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dispatcher_inherits_each_session_context_and_fails_closed_without_one() {
        use std::os::unix::fs::PermissionsExt;
        let home =
            std::env::temp_dir().join(format!("dragent-mcp-context-{}", uuid::Uuid::new_v4()));
        install_dispatcher(&home).unwrap();
        let binary = home.join("fake-host");
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s|%s' \"$DRAGENT_HOME\" \"$DRAGENT_HOST_ENDPOINT_FILE\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        for context in ["org-one", "org-two"] {
            let output = std::process::Command::new(dispatcher(&home))
                .env_clear()
                .env("DRAGENT_REMOTE_HOST_BINARY", &binary)
                .env("DRAGENT_HOME", context)
                .env("DRAGENT_HOST_ENDPOINT_FILE", format!("{context}/endpoint"))
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!("{context}|{context}/endpoint")
            );
        }
        assert!(
            !std::process::Command::new(dispatcher(&home))
                .env_clear()
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn a_users_own_dragent_server_is_left_alone() {
        let home = std::env::temp_dir().join(format!("dragent-remote-mcp-own-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let text = r#"{"mcpServers":{"dragent":{"command":"users-own-server"}}}"#;
        std::fs::write(home.join(".claude.json"), text).unwrap();
        let _ = register_at(&home, &["claude".into()]);
        assert_eq!(std::fs::read_to_string(home.join(".claude.json")).unwrap(), text);
        std::fs::remove_dir_all(home).unwrap();
    }
}
