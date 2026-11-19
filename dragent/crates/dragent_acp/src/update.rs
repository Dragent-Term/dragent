//! What the agent reports during a session (`session/update`).

use serde_json::Value;

use crate::options::SessionOptions;

/// Part of a tool call's output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolContent {
    Text(String),
    /// A file change: `old` is `None` for a new file.
    Diff { path: String, old: Option<String>, new: String },
    /// A file change as a unified diff (`+` / `-` lines).
    Patch { path: String, unified: String },
}

/// A tool call as first reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    /// `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `other`.
    pub kind: String,
    /// `pending`, `in_progress`, `completed` or `failed`.
    pub status: String,
    /// Files it touches.
    pub locations: Vec<String>,
    pub content: Vec<ToolContent>,
    pub raw_input: Option<Value>,
}

/// A later report on a tool call: only what changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolPatch {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub locations: Option<Vec<String>>,
    pub content: Option<Vec<ToolContent>>,
    pub raw_input: Option<Value>,
}

impl ToolCall {
    /// Apply a later report.
    pub fn apply(&mut self, patch: ToolPatch) {
        if let Some(v) = patch.title {
            self.title = v;
        }
        if let Some(v) = patch.kind {
            self.kind = v;
        }
        if let Some(v) = patch.status {
            self.status = v;
        }
        if let Some(v) = patch.locations {
            self.locations = v;
        }
        if let Some(v) = patch.content {
            self.content = v;
        }
        if patch.raw_input.is_some() {
            self.raw_input = patch.raw_input;
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.status.as_str(), "completed" | "failed")
    }
}

/// One entry of the agent's plan (its to-do list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    pub content: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
}

/// A `session/update`.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// A piece of the agent's reply. Pieces with the same message id (or
    /// consecutive pieces, when there is none) make one message.
    AgentText { message_id: Option<String>, text: String },
    /// A piece of the agent's reasoning.
    Thought { text: String },
    /// A piece of one of your messages (replayed when a session is loaded).
    UserText { text: String },
    ToolCall(ToolCall),
    ToolPatch(ToolPatch),
    Plan(Vec<PlanEntry>),
    /// Context used and its size, in tokens.
    Usage { used: u64, size: u64 },
    /// The agent switched mode itself (e.g. after leaving plan mode).
    ModeChanged(String),
    /// The agent's choices changed (a new model list, a new current value).
    OptionsChanged(SessionOptions),
    /// The slash commands it offers.
    Commands(Vec<(String, String)>),
    /// Anything this client doesn't read yet, by its kind.
    Other(String),
}

fn text_of(content: &Value) -> Option<String> {
    match content.get("type").and_then(Value::as_str) {
        Some("text") | None => content.get("text").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn locations(value: &Value) -> Option<Vec<String>> {
    value.get("locations").and_then(Value::as_array).map(|list| {
        list.iter().filter_map(|l| l.get("path").and_then(Value::as_str).map(str::to_string)).collect()
    })
}

fn tool_content(value: &Value) -> Option<Vec<ToolContent>> {
    value.get("content").and_then(Value::as_array).map(|list| {
        list.iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("content") => item.get("content").and_then(text_of).map(ToolContent::Text),
                Some("diff") => Some(ToolContent::Diff {
                    path: item.get("path")?.as_str()?.to_string(),
                    old: item.get("oldText").and_then(Value::as_str).map(str::to_string),
                    new: item.get("newText").and_then(Value::as_str).unwrap_or_default().to_string(),
                }),
                _ => None,
            })
            .collect()
    })
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

impl Update {
    /// Read the `update` object of a `session/update` notification.
    pub fn parse(update: &Value) -> Update {
        let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default();
        let content_text = || update.get("content").and_then(text_of).unwrap_or_default();
        match kind {
            "agent_message_chunk" => Update::AgentText { message_id: string(update, "messageId"), text: content_text() },
            "agent_thought_chunk" => Update::Thought { text: content_text() },
            "user_message_chunk" => Update::UserText { text: content_text() },
            "tool_call" => Update::ToolCall(ToolCall {
                id: string(update, "toolCallId").unwrap_or_default(),
                title: string(update, "title").unwrap_or_default(),
                kind: string(update, "kind").unwrap_or_else(|| "other".into()),
                status: string(update, "status").unwrap_or_else(|| "pending".into()),
                locations: locations(update).unwrap_or_default(),
                content: tool_content(update).unwrap_or_default(),
                raw_input: update.get("rawInput").cloned(),
            }),
            "tool_call_update" => Update::ToolPatch(ToolPatch {
                id: string(update, "toolCallId").unwrap_or_default(),
                title: string(update, "title"),
                kind: string(update, "kind"),
                status: string(update, "status"),
                locations: locations(update),
                content: tool_content(update),
                raw_input: update.get("rawInput").cloned(),
            }),
            "plan" => Update::Plan(
                update
                    .get("entries")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|e| PlanEntry {
                        content: string(e, "content").unwrap_or_default(),
                        status: string(e, "status").unwrap_or_else(|| "pending".into()),
                    })
                    .collect(),
            ),
            "usage_update" => Update::Usage {
                used: update.get("used").and_then(Value::as_u64).unwrap_or(0),
                size: update.get("size").and_then(Value::as_u64).unwrap_or(0),
            },
            "current_mode_update" => Update::ModeChanged(string(update, "currentModeId").unwrap_or_default()),
            "config_option_update" => Update::OptionsChanged(SessionOptions::parse(update)),
            "available_commands_update" => Update::Commands(
                update
                    .get("availableCommands")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| Some((string(c, "name")?, string(c, "description").unwrap_or_default())))
                    .collect(),
            ),
            other => Update::Other(other.to_string()),
        }
    }
}
