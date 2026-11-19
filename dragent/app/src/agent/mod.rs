pub mod agent_share;
pub mod api;
pub use dragent_agent::approval_policy;
pub mod attachments;
pub(crate) mod file_drag;
// Moved to the shared native-free `dragent_viewer` crate.
pub use dragent_agent::checkpoint_store;
pub use dragent_viewer::agent::chat_mode;
pub mod command_intent;
pub mod context_builder;
pub mod controller;
pub mod conversation_view;
pub mod detected_input_type;
pub mod dream_consolidator;
pub use dragent_viewer::agent::edit_diff;
pub mod error_display;
pub mod events;
pub mod explore;
pub mod hooks;
pub(crate) mod host_impls;
pub mod host_verification;
pub mod input_classifier;
pub mod input_config;
pub mod local_tools;
pub(crate) mod tools_background;
pub(crate) mod tools_command;
pub(crate) mod tools_files;
pub(crate) mod tools_mcp_auth;
pub(crate) mod tools_search;
pub(crate) mod tools_shell;
pub(crate) mod tools_subagent;
// MCP client (network/process) is native-only; the wasm viewer swaps in a no-op
// stub with the same public API so `crate::agent::mcp::*` call sites compile.
#[cfg(not(target_family = "wasm"))]
pub mod mcp;
#[cfg(target_family = "wasm")]
#[path = "mcp_stub.rs"]
pub mod mcp;
/// Writes chosen MCP servers into other agents' config files (all targets:
/// the settings UI lists the agents in the web viewer too).
pub mod mcp_agent_sync;
pub mod memory_extractor;
pub mod memory_store;
pub use dragent_agent::permission_store;
pub use dragent_agent::permissions;
pub mod pipeline;
pub mod principal_coordination;
pub use dragent_agent::project as project_context;
pub mod query_log;
pub mod self_heal;
pub mod session_context;
pub use dragent_agent::session_facts;
pub use dragent_agent::session_store;
pub(crate) mod execution_launch;
#[cfg(test)]
mod execution_launch_tests;
pub(crate) mod execution_pipeline;
#[cfg(test)]
mod execution_pipeline_tests;
pub(crate) mod execution_review;
#[cfg(test)]
mod execution_review_tests;
pub(crate) mod execution_runtime;
#[cfg(test)]
mod execution_runtime_tests;
pub mod skill_discovery;
pub mod task_profile;
pub mod tool_output_spill;
pub mod tools;
pub mod workspace_execution;

pub use session_context::AgentSessionContextBuilder;
use std::sync::Mutex;

pub use api::{default_agent_api, AgentApi, AgentError};
pub use command_intent::{explicitly_requests_terminal_cwd_change, is_command_action_request};
pub use controller::{AgentBlock, AgentBlockKind, AgentController, AgentControllerEvent};
pub use detected_input_type::DetectedInputType;
pub use events::AgentEvent;
pub use input_classifier::{ClassifyContext, InputClassifier};
pub use input_config::{InputConfig, TabSessionKind};

use api::types::{AgentActionKind, AgentQueryResponse};
use query_log::{AgentQueryEntry, AgentQueryLog};

static QUERY_LOG: Mutex<Option<AgentQueryLog>> = Mutex::new(None);

fn with_query_log<F: FnOnce(&mut AgentQueryLog)>(f: F) {
    if let Ok(mut guard) = QUERY_LOG.lock() {
        let log = guard.get_or_insert_with(AgentQueryLog::new);
        f(log);
    }
}

fn action_summary(response: &AgentQueryResponse) -> String {
    response
        .actions
        .iter()
        .map(|a| match &a.kind {
            AgentActionKind::Message { .. } => "Message",
            AgentActionKind::RunCommand { .. } => "RunCommand",
            AgentActionKind::Done => "Done",
            AgentActionKind::ReadFiles { .. } => "ReadFiles",
            AgentActionKind::ListDirectory { .. } => "ListDirectory",
            AgentActionKind::Grep { .. } => "Grep",
            AgentActionKind::SearchCodebase { .. } => "SearchCodebase",
            AgentActionKind::WriteFile { .. } => "WriteFile",
            AgentActionKind::EditFile { .. } => "EditFile",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Log a completed agent response and store it in the query ring-buffer.
pub fn log_agent_response(
    query: &str,
    is_autodetected: bool,
    response: &AgentQueryResponse,
    cwd: &std::path::PathBuf,
) {
    let summary = action_summary(response);
    let truncated = tools_background::utf8_prefix(query, 200);

    log::info!(
        "[agent] query={:?} conversation_id={} actions=[{}]",
        truncated,
        response.conversation_id,
        summary,
    );

    with_query_log(|log| {
        log.push(AgentQueryEntry {
            query: query.to_string(),
            cwd: cwd.clone(),
            is_autodetected,
            conversation_id: Some(response.conversation_id),
            action_summary: Some(summary),
        });
    });
}
