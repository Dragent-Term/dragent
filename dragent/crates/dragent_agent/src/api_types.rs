use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Actions ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentActionKind {
    Message {
        text: String,
    },
    RunCommand {
        command: String,
        reason: String,
    },
    Done,
    /// Read the contents of one or more files.
    ReadFiles {
        paths: Vec<PathBuf>,
    },
    /// List the contents of a directory (non-recursive by default).
    ListDirectory {
        path: PathBuf,
        recursive: bool,
    },
    /// Search for a pattern with ripgrep / grep.
    Grep {
        pattern: String,
        path: Option<PathBuf>,
    },
    /// Search codebase using local index metadata & offline search.
    SearchCodebase {
        query: String,
        repo_root: PathBuf,
    },
    /// Write (create or overwrite) a file. Requires user approval.
    WriteFile {
        path: PathBuf,
        content: String,
    },
    /// Replace an exact string inside a file. Produces a diff for review. Requires user approval.
    EditFile {
        path: PathBuf,
        old_str: String,
        new_str: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentAction {
    pub id: Uuid,
    pub kind: AgentActionKind,
    pub requires_approval: bool,
}

impl AgentAction {
    pub fn message(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::Message { text: text.into() },
            requires_approval: false,
        }
    }

    pub fn run_command(command: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::RunCommand {
                command: command.into(),
                reason: reason.into(),
            },
            requires_approval: true,
        }
    }

    pub fn done() -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::Done,
            requires_approval: false,
        }
    }

    pub fn read_files(paths: Vec<PathBuf>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::ReadFiles { paths },
            requires_approval: false,
        }
    }

    pub fn grep(pattern: impl Into<String>, path: Option<PathBuf>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::Grep {
                pattern: pattern.into(),
                path,
            },
            requires_approval: false,
        }
    }

    pub fn search_codebase(query: impl Into<String>, repo_root: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: AgentActionKind::SearchCodebase {
                query: query.into(),
                repo_root,
            },
            requires_approval: false,
        }
    }
}

// ── Agentic loop ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub args_json: String,
}

/// Provider-neutral envelope for a locally executed tool result. `call_id` is
/// the idempotency key shared by SSE, GraphQL, and any future provider adapter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResultEnvelope {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
    pub cancelled: bool,
}

impl ToolResultEnvelope {
    pub fn success(call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            output: output.into(),
            is_error: false,
            cancelled: false,
        }
    }

    pub fn error(call_id: impl Into<String>, output: impl Into<String>, cancelled: bool) -> Self {
        Self {
            call_id: call_id.into(),
            output: output.into(),
            is_error: true,
            cancelled,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub content: String,
    /// For role=Tool: the id of the tool call being responded to.
    pub tool_call_id: Option<String>,
    /// For role=Assistant: tool calls the LLM requested.
    pub tool_calls: Vec<AgentToolCall>,
}

impl AgentMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: AgentRole::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: vec![],
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: AgentRole::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: vec![],
        }
    }

    pub fn assistant_tool_calls(calls: Vec<AgentToolCall>) -> Self {
        Self {
            role: AgentRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: calls,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema (as string) for the tool's parameters object.
    pub parameters_json: String,
}

#[derive(Clone, Debug)]
pub struct AgentStepRequest {
    pub model_name: String,
    pub system: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<AgentToolDef>,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AgentStepResponse {
    /// Non-empty → LLM wants these tools executed; call agent_step again with results.
    pub tool_calls: Vec<AgentToolCall>,
    /// Non-empty → LLM produced a final answer.
    pub content: String,
    pub rotated_refresh_token: Option<String>,
    pub rotated_access_token: Option<String>,
    pub rotated_access_token_expires_in: Option<u64>,
}

impl AgentStepResponse {
    pub fn is_final(&self) -> bool {
        self.tool_calls.is_empty()
    }
}

// ── Request / response ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AgentQueryRequest {
    pub query: String,
    pub session: super::session::AgentSessionContext,
}

#[derive(Clone, Debug)]
pub struct AgentQueryResponse {
    pub conversation_id: Uuid,
    pub actions: Vec<AgentAction>,
}

// ── Block analysis ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockAnalysis {
    pub explanation: String,
    pub suggested_command: Option<String>,
    pub confidence: f32,
    /// New refresh token returned by the server after rotation. `None` for mock/offline analysis.
    #[serde(skip)]
    pub rotated_refresh_token: Option<String>,
    /// New access token obtained during the same rotation exchange.
    #[serde(skip)]
    pub rotated_access_token: Option<String>,
    /// Server-reported lifetime of `rotated_access_token` in seconds.
    #[serde(skip)]
    pub rotated_access_token_expires_in: Option<u64>,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_message_no_approval() {
        let a = AgentAction::message("hello");
        assert!(!a.requires_approval);
        assert!(matches!(a.kind, AgentActionKind::Message { .. }));
    }

    #[test]
    fn action_run_command_requires_approval() {
        let a = AgentAction::run_command("cargo build", "fix the build");
        assert!(a.requires_approval);
        assert!(matches!(a.kind, AgentActionKind::RunCommand { .. }));
    }

    #[test]
    fn action_serde_round_trip() {
        let actions = vec![
            AgentAction::message("hey"),
            AgentAction::run_command("echo hi", "test"),
            AgentAction::done(),
            AgentAction::read_files(vec![PathBuf::from("src/main.rs")]),
            AgentAction::grep("TODO", Some(PathBuf::from("src/"))),
            AgentAction::search_codebase("entry point", PathBuf::from("/tmp")),
        ];
        for action in &actions {
            let json = serde_json::to_string(action).expect("serialize");
            let back: AgentAction = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(action.kind, back.kind);
            assert_eq!(action.requires_approval, back.requires_approval);
        }
    }

    #[test]
    fn unique_action_ids() {
        let a = AgentAction::done();
        let b = AgentAction::done();
        assert_ne!(a.id, b.id);
    }
}
