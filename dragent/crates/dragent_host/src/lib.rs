//! Persistent Dragent Host runtime.
//!
//! Phase 1: foundation only — identity, runtime snapshot shape, heartbeats,
//! event stream. No agents, workspaces, or PTYs move yet.
//!
//! [`client`]: UI-side connector. [`discovery`]: endpoint + spawn-lock.
//! [`protocol`]: transport-independent domain types. [`server`]: runtime loop.

#[cfg(not(target_family = "wasm"))]
pub mod agents;
pub mod agent_recovery;
#[cfg(not(target_family = "wasm"))]
pub mod agent_accounts;
#[cfg(not(target_family = "wasm"))]
pub mod agent_usage;
#[cfg(not(target_family = "wasm"))]
pub mod agent_history;
#[cfg(not(target_family = "wasm"))]
pub mod browser_cookies;
#[cfg(not(target_family = "wasm"))]
pub mod token_usage;
#[cfg(not(target_family = "wasm"))]
pub mod agent_user_hooks;
#[cfg(not(target_family = "wasm"))]
pub mod agent_quiet;
pub mod agent_sessions;
#[cfg(not(target_family = "wasm"))]
pub mod agent_sleep;
#[cfg(not(target_family = "wasm"))]
pub mod agent_transcript;
#[cfg(not(target_family = "wasm"))]
pub mod agent_skills;
mod build_identity;

/// Capture this process's executable identity before the file can be replaced
/// by a background update or rebuild. Call at native process startup.
pub fn capture_build_identity() {
    let _ = build_identity::current();
}
pub mod client;
pub mod discovery;
#[cfg(not(target_family = "wasm"))]
pub mod fanout;
#[cfg(not(target_family = "wasm"))]
pub mod history;
pub mod protocol;
#[cfg(not(target_family = "wasm"))]
pub mod pty_service;
#[cfg(not(target_family = "wasm"))]
pub mod pty_supervisor;
#[cfg(not(target_family = "wasm"))]
pub mod resources;
#[cfg(not(target_family = "wasm"))]
pub mod router;
pub mod scheduler;
pub mod screen_protocol;
pub mod server;
#[cfg(not(target_family = "wasm"))]
pub mod stale;
#[cfg(not(target_family = "wasm"))]
mod terminal_screen;
pub mod team;
#[cfg(not(target_family = "wasm"))]
pub mod team_courier;
#[cfg(not(target_family = "wasm"))]
pub mod terminals;
#[cfg(not(target_family = "wasm"))]
pub mod transport;
#[cfg(not(target_family = "wasm"))]
pub mod watch;
#[cfg(not(target_family = "wasm"))]
pub mod workspaces;
#[cfg(not(target_family = "wasm"))]
pub mod worktrees;

#[cfg(not(target_family = "wasm"))]
pub use agents::AgentRegistry;
pub use client::{HostClient, TerminalChunk};
pub use protocol::pty_stable_port;
pub use protocol::{
    AgentConfig, AgentDispatch, AgentOverrides, AgentReadiness, AgentRun, AgentRunState,
    DAEMON_PROTOCOL_VERSION, DAEMON_SUPPORTED_VERSIONS, DaemonRequest, DaemonRequestEnvelope,
    DaemonResponse, DaemonResponseEnvelope, ENDPOINT_FILE_ENV, Event, GitBinding, HistoryRecord,
    HomeKind, HostInfo, PROTOCOL_VERSION, PTY_MANIFEST_FILENAME, PTY_PORT_BASE, PTY_SERVICE_ARG,
    PTY_TOKEN_ENV, PromptTransport, PtyDaemonManifest, Request, Response, RestorePlan,
    RestoreSession, RuntimeState, SERVICE_ARG, ServiceEndpoint, TeardownReport, TerminalSession,
    TerminalState, WorktreeHome, WorktreeState,
};
#[cfg(not(target_family = "wasm"))]
pub use terminals::{READ_LINE_CAP, REPLAY_CAP, TerminalRegistry};

pub mod development;
#[cfg(not(target_family = "wasm"))]
pub mod host_tools;

/// Folders git may not climb into while looking for a repository: the
/// user's home folder (often a dotfiles repository) and Dragent's own data
/// folder. A folder below them with no repository of its own is then not in
/// a repository, instead of in the whole home folder, whose `git status`
/// lists every file the user has and can run for minutes.
pub fn git_discovery_ceilings() -> Vec<std::path::PathBuf> {
    let mut ceilings: Vec<std::path::PathBuf> = Vec::new();
    for dir in [dirs::home_dir(), Some(dragent_paths::home())].into_iter().flatten() {
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        if !ceilings.contains(&dir) {
            ceilings.push(dir);
        }
    }
    ceilings
}

/// `GIT_CEILING_DIRECTORIES` for [`git_discovery_ceilings`], keeping any
/// the user set themselves.
pub fn git_ceiling_env() -> Option<std::ffi::OsString> {
    let mut ceilings = git_discovery_ceilings();
    if let Some(existing) = std::env::var_os("GIT_CEILING_DIRECTORIES") {
        ceilings.extend(std::env::split_paths(&existing).filter(|dir| !dir.as_os_str().is_empty()));
    }
    std::env::join_paths(ceilings).ok()
}

/// `git`: the copy chosen for this machine in Settings → Machines, else the
/// one on the PATH ([`host_tools::command`]), kept from climbing past
/// [`git_discovery_ceilings`].
pub(crate) fn git_command() -> std::process::Command {
    #[cfg(not(target_family = "wasm"))]
    {
        let mut command = host_tools::command("git");
        if let Some(ceilings) = git_ceiling_env() {
            command.env("GIT_CEILING_DIRECTORIES", ceilings);
        }
        command
    }
    #[cfg(target_family = "wasm")]
    {
        std::process::Command::new("git")
    }
}
pub mod machine_registry;
#[cfg(not(target_family = "wasm"))]
pub mod remote;

pub mod pull_requests;

#[cfg(not(target_family = "wasm"))]
pub mod cli_requests;
pub mod outcome;
#[cfg(not(target_family = "wasm"))]
pub mod scope;
#[cfg(not(target_family = "wasm"))]
pub mod team_commands;
#[cfg(not(target_family = "wasm"))]
mod team_command_support;

#[cfg(not(target_family = "wasm"))]
pub mod team_runtime;

#[cfg(not(target_family = "wasm"))]
pub mod mcp;

pub mod mcp_config;
pub mod mcp_sync;
#[cfg(not(target_family = "wasm"))]
pub mod remote_mcp;

#[cfg(not(target_family = "wasm"))]
pub mod service_mcp;
#[cfg(not(target_family = "wasm"))]
pub mod service_relay;

#[cfg(not(target_family = "wasm"))]
pub mod integration_commands;
