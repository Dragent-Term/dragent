//! Automatic registration uses the desktop's exact config editor. Global
//! entries point to a context-neutral dispatcher; they never pin an org,
//! endpoint or credential into the provider's settings.
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
        enabled: enabled_at(home),
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
    use serde_json::{Value, json};
    #[test]
    fn disable_preserves_user_servers_and_enable_restores_registration() {
        let home =
            std::env::temp_dir().join(format!("dragent-mcp-toggle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"mcpServers":{"keep":{"command":"keep"}}}"#,
        )
        .unwrap();
        register_at(&home, &["claude".into()]).unwrap();
        configure_at(&home, false).unwrap();
        let read = || {
            serde_json::from_str::<Value>(
                &std::fs::read_to_string(home.join(".claude.json")).unwrap(),
            )
            .unwrap()
        };
        assert!(read()["mcpServers"].get("dragent").is_none());
        assert_eq!(read()["mcpServers"]["keep"]["command"], "keep");
        configure_at(&home, true).unwrap();
        assert!(read()["mcpServers"].get("dragent").is_some());
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
    fn registers_all_six_formats_idempotently_without_pinning_a_namespace() {
        let home =
            std::env::temp_dir().join(format!("dragent-remote-mcp-{}", uuid::Uuid::new_v4()));
        for dir in [
            ".codex",
            ".cursor",
            ".gemini",
            ".qwen",
            ".config/opencode",
            ".claude",
        ] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        std::fs::write(
            home.join(".claude.json"),
            json!({"keep":"user setting","mcpServers":{"existing":{"command":"keep-me"}}})
                .to_string(),
        )
        .unwrap();
        let report = register_at(&home, &[]).unwrap();
        assert_eq!(report.updated.len(), 6);
        for (path, key) in [
            (".claude.json", "mcpServers"),
            (".cursor/mcp.json", "mcpServers"),
            (".gemini/settings.json", "mcpServers"),
            (".qwen/settings.json", "mcpServers"),
            (".config/opencode/opencode.json", "mcp"),
        ] {
            let value: Value =
                serde_json::from_str(&std::fs::read_to_string(home.join(path)).unwrap()).unwrap();
            let entry = &value[key]["dragent"];
            assert!(!entry.is_null(), "{path}");
            assert!(entry.get("env").is_none() && entry.get("environment").is_none());
        }
        let claude: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(claude["keep"], "user setting");
        assert_eq!(claude["mcpServers"]["existing"]["command"], "keep-me");
        let codex = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
        assert!(codex.contains("mcp_servers.dragent"));
        assert!(!codex.contains("DRAGENT_HOME"));
        assert!(register_at(&home, &[]).unwrap().updated.is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn first_launch_registers_a_selected_agent_but_preserves_user_owned_servers() {
        let home = std::env::temp_dir().join(format!(
            "dragent-remote-mcp-conflict-{}",
            uuid::Uuid::new_v4()
        ));
        register_at(&home, &["codex".into()]).unwrap();
        assert!(home.join(".codex/config.toml").is_file());
        let text = r#"{"mcpServers":{"dragent":{"command":"users-own-server"}}}"#;
        std::fs::write(home.join(".claude.json"), text).unwrap();
        assert!(
            register_at(&home, &["claude".into()])
                .unwrap_err()
                .contains("own \"dragent\" server")
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".claude.json")).unwrap(),
            text
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
