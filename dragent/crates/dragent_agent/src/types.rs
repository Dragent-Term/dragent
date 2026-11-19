//! AgentController shared types.
//!
//! Plain data types used across controller modules.

use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use super::block::AgentAuthor;
use super::pipeline::PipelineEvent;

pub type ToolPolicy =
    Arc<dyn Fn(&PipelineEvent) -> super::pipeline::WorkspaceToolDisposition + Send + Sync>;

/// One tool call issued during a turn — see `AgentController::turn_tool_calls`.
#[derive(Clone, Debug, Default)]
pub struct TurnToolCall {
    pub call_id: String,
    pub name: String,
    pub args_json: String,
    /// Short result text from the ToolEnd event; `None` while running or for
    /// locally intercepted tools (ask_user / todowrite).
    pub result_summary: Option<String>,
    pub is_error: bool,
}

pub fn truncate_chars(text: &str, max: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max {
        compact
    } else {
        format!("{}…", compact.chars().take(max).collect::<String>())
    }
}

/// See `AgentController::set_display_text_filter`.
pub type DisplayTextFilter = Arc<dyn Fn(&str, bool) -> String + Send + Sync>;

#[derive(Clone, Debug, Default)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    /// Image attachments on user messages (empty for assistant/system turns).
    pub attachments: Vec<super::attachments::MessageAttachment>,
}

/// A tool call from the server that requires user approval before executing locally.
/// A tool call a restart-recovered local run is still waiting on: it was
/// parked for consent when the app stopped, and its card must come back.
#[derive(Clone, Debug)]
pub struct RecoveredParkedCall {
    pub call_id: String,
    pub tool_name: String,
    pub args_json: String,
    /// Why consent was needed, as the host recorded it (Attention summary).
    pub reason: String,
}

pub struct PendingToolCall {
    /// The LLM's tool call ID — used as call_id when POSTing the result back.
    pub call_id: String,
    pub tool_name: String,
    pub args_json: String,
    /// Legacy routing hint, retained for compatibility. Approval execution
    /// ignores it: agent commands always use the isolated background executor.
    pub run_in_terminal: bool,
    /// User authorization for a visible `cd`, captured before model execution.
    pub allow_terminal_cwd_change: bool,
    /// Directory scopes requested by an out-of-project filesystem read. Empty
    /// for ordinary tool approvals. Used for one-shot execution and path-scoped
    /// “Always allow” persistence.
    pub external_read_roots: Vec<PathBuf>,
}

/// A user prompt waiting behind the active turn. Holds everything needed to
/// start it later — including the one-shot system-prompt builder captured at
/// submit time. In-memory only (the closure can't be persisted).
pub struct QueuedMessage {
    pub queue_id: Uuid,
    /// The QueuedMessage block showing this in the transcript.
    pub block_id: usize,
    pub query: String,
    pub author: Option<AgentAuthor>,
    pub attachments: Vec<super::attachments::MessageAttachment>,
    pub model_name: String,
    pub system_builder: Box<dyn FnOnce() -> String + Send + 'static>,
    /// Submitted by an automation: its turn runs unattended.
    pub unattended: bool,
    /// That automation lets it change things in connected services.
    pub service_writes: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ResponseStyle {
    #[default]
    Default,
    Concise,
    Explanatory,
    Formal,
}

impl ResponseStyle {
    /// One-liner appended to the dynamic section of the system prompt.
    pub fn prompt_suffix(&self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Concise => {
                "Be maximally concise. Lead with the answer. One sentence is better than three."
            }
            Self::Explanatory => {
                "Be thorough and educational. Include reasoning, alternatives, and potential pitfalls."
            }
            Self::Formal => {
                "Use formal, professional language. Avoid contractions and colloquialisms."
            }
        }
    }
}
