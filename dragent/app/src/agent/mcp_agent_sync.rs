//! Desktop adapter for the shared MCP configuration synchronizer.
use crate::settings::McpServerEntry;
pub use dragent_host::mcp_sync::{SyncAgent, SyncReport, agent_labels, sync};

/// The agent whose name and icon stand for `agent` in the app.
pub fn prompt_agent(agent: SyncAgent) -> crate::workspace::PromptAgent {
    use crate::workspace::PromptAgent;
    match agent {
        SyncAgent::ClaudeCode => PromptAgent::Claude,
        SyncAgent::Codex => PromptAgent::Codex,
        SyncAgent::Cursor => PromptAgent::Cursor,
        SyncAgent::Gemini => PromptAgent::Gemini,
        SyncAgent::Qwen => PromptAgent::Qwen,
        SyncAgent::OpenCode => PromptAgent::OpenCode,
    }
}

/// Bring every agent's config in line with `entries`, using the user's home
/// for the agents' files. One sync at a time: quick successive edits must not
/// interleave their reads and writes of the same files.
///
/// A Dev build never touches other apps' settings: it reports nothing to
/// sync and writes nothing.
#[cfg(not(target_family = "wasm"))]
pub fn sync_agents(entries: &[McpServerEntry]) -> SyncReport {
    match dirs::home_dir() {
        Some(home) => sync_agents_at(entries, &home),
        None => SyncReport {
            problems: vec!["Couldn't find your home folder to update other agents.".into()],
            ..Default::default()
        },
    }
}

/// [`sync_agents`] against an explicit home folder (tests point this at a
/// throwaway folder).
#[cfg(not(target_family = "wasm"))]
pub(crate) fn sync_agents_at(entries: &[McpServerEntry], home: &std::path::Path) -> SyncReport {
    if dragent_paths::is_dev() {
        log::info!("[mcp] Dev build: leaving other apps' MCP settings alone");
        return SyncReport::default();
    }
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let record = dragent_paths::home().join("mcp-agent-sync.json");
    // Dragent's own tools bridge is never written here: agents get it at
    // launch. One an earlier build wrote is owned, so this takes it out.
    sync(entries, home, &record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::McpTransport;

    fn synced_entry(name: &str, agent: SyncAgent) -> McpServerEntry {
        McpServerEntry {
            name: name.into(),
            command: "npx".into(),
            transport: McpTransport::Stdio,
            args: vec!["-y".into()],
            env: Default::default(),
            enabled: true,
            bearer_token: None,
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: vec![agent.key().to_string()],
        }
    }

    #[test]
    fn dev_builds_write_nothing_into_other_agents() {
        // Test builds are the Dev edition; the sync must leave the home
        // folder alone even with an agent present and a server to sync.
        if !dragent_paths::is_dev() {
            return;
        } // only meaningful in the Dev edition
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".cursor")).unwrap();
        let report = sync_agents_at(&[synced_entry("demo", SyncAgent::Cursor)], home.path());
        assert!(report.updated.is_empty());
        assert!(report.problems.is_empty());
        assert!(!home.path().join(".cursor/mcp.json").exists());
    }
}
