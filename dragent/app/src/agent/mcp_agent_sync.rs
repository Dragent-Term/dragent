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
#[cfg(not(target_family = "wasm"))]
pub fn sync_agents(entries: &[McpServerEntry]) -> SyncReport {
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let record = dragent_paths::home().join("mcp-agent-sync.json");
    match dirs::home_dir() {
        Some(home) => {
            // The connected services' bridge rides along; a server of the
            // user's own by that name wins.
            let mut entries = entries.to_vec();
            if let Some(bridge) = crate::integrations::agents::bridge_entry(&home) {
                if !entries.iter().any(|entry| entry.name == bridge.name) {
                    entries.push(bridge);
                }
            }
            sync(&entries, &home, &record)
        }
        None => SyncReport {
            problems: vec!["Couldn't find your home folder to update other agents.".into()],
            ..Default::default()
        },
    }
}
