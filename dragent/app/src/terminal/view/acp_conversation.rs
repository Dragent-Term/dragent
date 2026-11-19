//! An ACP agent's conversation as transcript rows, built from its updates.
//!
//! The chat already draws transcript rows (user / assistant / tool) for the
//! terminal agents it reads from disk; an ACP chat feeds the same rows, so it
//! looks the same and the layout rules stay in one place.

use std::collections::HashMap;

use dragent_acp::{SessionOptions, ToolCall, ToolContent, Update};
use dragent_host::agent_transcript::Row;

/// The conversation so far and what the chat shows around it.
#[derive(Default)]
pub(crate) struct AcpConversation {
    pub rows: Vec<Row>,
    pub options: SessionOptions,
    /// Context used and its size, in tokens.
    pub usage: Option<(u64, u64)>,
    tools: HashMap<String, (usize, ToolCall)>,
    /// The reply being streamed: its message id and row.
    reply: Option<(Option<String>, usize)>,
    next_id: u64,
}

impl AcpConversation {
    fn push(&mut self, role: &'static str, text: String) -> usize {
        self.next_id += 1;
        let mut row = Row::new(format!("acp-{}", self.next_id), role, text);
        row.at = Some(chrono::Utc::now().timestamp_millis());
        self.rows.push(row);
        self.rows.len() - 1
    }

    /// Start from an earlier conversation's rows (read from its transcript).
    pub fn preload(&mut self, rows: Vec<Row>) {
        if rows.is_empty() {
            return;
        }
        self.rows = rows;
        self.next_id += self.rows.len() as u64;
        self.reply = None;
    }

    /// Your message, as sent from the chat.
    pub fn you(&mut self, text: &str) {
        self.reply = None;
        self.push("user", text.to_string());
    }

    /// A line from the app (an error, a stop), shown as the agent's.
    pub fn note(&mut self, text: &str) {
        self.reply = None;
        self.push("assistant", text.to_string());
    }

    /// Fold in one update from the agent.
    pub fn apply(&mut self, update: Update) {
        match update {
            Update::AgentText { message_id, text } => {
                let last = self.rows.len().checked_sub(1);
                match &self.reply {
                    // Same message, still the newest row: keep streaming into it.
                    Some((id, at)) if Some(*at) == last && (message_id.is_none() || *id == message_id) => {
                        let at = *at;
                        self.rows[at].text.push_str(&text);
                    }
                    _ => {
                        if text.trim().is_empty() {
                            return;
                        }
                        let at = self.push("assistant", text);
                        self.reply = Some((message_id, at));
                    }
                }
            }
            Update::UserText { text } => {
                // A loaded conversation replays your messages in pieces.
                self.reply = None;
                match self.rows.last_mut() {
                    Some(row) if row.role == "user" => row.text.push_str(&text),
                    _ => {
                        self.push("user", text);
                    }
                }
            }
            Update::ToolCall(call) => {
                self.reply = None;
                let at = self.push("tool", String::new());
                self.rows[at] = tool_row(self.rows[at].id.clone(), self.rows[at].at, &call);
                self.tools.insert(call.id.clone(), (at, call));
            }
            Update::ToolPatch(patch) => {
                if let Some((at, call)) = self.tools.get_mut(&patch.id) {
                    call.apply(patch);
                    let row = &self.rows[*at];
                    self.rows[*at] = tool_row(row.id.clone(), row.at, call);
                }
            }
            Update::Usage { used, size } => self.usage = Some((used, size)),
            Update::ModeChanged(mode) => {
                if let Some(picker) = self.options.mode.as_mut() {
                    picker.current = mode;
                }
            }
            Update::OptionsChanged(options) => self.take_options(options),
            Update::Thought { .. } | Update::Plan(_) | Update::Commands(_) | Update::Other(_) => {}
        }
    }

    /// New options from the agent: each picker it sent replaces ours.
    pub fn take_options(&mut self, options: SessionOptions) {
        if options.model.is_some() {
            self.options.model = options.model;
        }
        if options.mode.is_some() {
            self.options.mode = options.mode;
        }
        if options.effort.is_some() {
            self.options.effort = options.effort;
        }
    }

    /// A turn ended: anything still marked running was abandoned.
    pub fn end_turn(&mut self) {
        self.reply = None;
        for (at, call) in self.tools.values() {
            if !call.finished() {
                self.rows[*at].running = false;
            }
        }
    }
}

/// Lines added and removed going from `old` to `new` (as a multiset of
/// lines: good enough for a count).
fn line_counts(old: Option<&str>, new: &str) -> (u32, u32) {
    let mut before: HashMap<&str, i64> = HashMap::new();
    for line in old.unwrap_or_default().lines() {
        *before.entry(line).or_default() += 1;
    }
    let mut added = 0u32;
    for line in new.lines() {
        match before.get_mut(line) {
            Some(n) if *n > 0 => *n -= 1,
            _ => added += 1,
        }
    }
    let removed = before.values().filter(|n| **n > 0).map(|n| *n as u32).sum();
    (added, removed)
}

/// A tool call as a transcript row, named the way the step renderer reads.
fn tool_row(id: String, at: Option<i64>, call: &ToolCall) -> Row {
    let input = |key: &str| call.raw_input.as_ref().and_then(|v| v.get(key)).and_then(|v| v.as_str()).map(str::to_string);
    let path = call.locations.first().cloned().or_else(|| {
        call.content.iter().find_map(|c| match c {
            ToolContent::Diff { path, .. } | ToolContent::Patch { path, .. } => Some(path.clone()),
            _ => None,
        })
    });
    let (tool, text) = match call.kind.as_str() {
        "execute" => ("Bash", input("command").unwrap_or_else(|| call.title.clone())),
        "read" => ("Read", path.clone().unwrap_or_else(|| call.title.clone())),
        "edit" => ("Edit", path.clone().unwrap_or_else(|| call.title.clone())),
        "delete" => ("Delete", path.clone().unwrap_or_else(|| call.title.clone())),
        "move" => ("Edit", path.clone().unwrap_or_else(|| call.title.clone())),
        "search" => ("Grep", input("pattern").or_else(|| input("query")).unwrap_or_else(|| call.title.clone())),
        "fetch" => ("WebFetch", input("url").unwrap_or_else(|| call.title.clone())),
        _ => ("Tool", call.title.clone()),
    };
    let mut row = Row::new(id, "tool", text);
    row.tool = Some(if tool == "Tool" && !call.title.is_empty() { call.title.clone() } else { tool.to_string() });
    row.at = at;
    row.path = matches!(call.kind.as_str(), "read" | "edit" | "delete" | "move").then_some(path).flatten();
    row.running = !call.finished();
    row.error = call.status == "failed";
    let mut counted = false;
    let (mut added, mut removed) = (0, 0);
    for content in &call.content {
        match content {
            ToolContent::Diff { old, new, .. } => {
                let (a, r) = line_counts(old.as_deref(), new);
                added += a;
                removed += r;
                counted = true;
            }
            ToolContent::Patch { unified, .. } => {
                for line in unified.lines() {
                    if line.starts_with('+') && !line.starts_with("+++") {
                        added += 1;
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        removed += 1;
                    }
                }
                counted = true;
            }
            ToolContent::Text(_) => {}
        }
    }
    if counted {
        row.added = Some(added);
        row.removed = Some(removed);
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_acp::ToolPatch;

    fn text(message: Option<&str>, text: &str) -> Update {
        Update::AgentText { message_id: message.map(str::to_string), text: text.into() }
    }

    #[test]
    fn a_streamed_reply_is_one_row_and_a_tool_call_splits_it() {
        let mut chat = AcpConversation::default();
        chat.you("list files");
        chat.apply(text(Some("m1"), "Let me "));
        chat.apply(text(Some("m1"), "look."));
        chat.apply(Update::ToolCall(ToolCall {
            id: "t1".into(),
            title: "ls".into(),
            kind: "execute".into(),
            status: "pending".into(),
            raw_input: Some(serde_json::json!({"command": "ls -la"})),
            ..ToolCall::default()
        }));
        chat.apply(Update::ToolPatch(ToolPatch { id: "t1".into(), status: Some("completed".into()), ..ToolPatch::default() }));
        chat.apply(text(Some("m2"), "Two files."));
        let roles: Vec<&str> = chat.rows.iter().map(|r| r.role).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
        assert_eq!(chat.rows[1].text, "Let me look.");
        assert_eq!(chat.rows[2].tool.as_deref(), Some("Bash"));
        assert_eq!(chat.rows[2].text, "ls -la");
        assert!(!chat.rows[2].running);
    }

    #[test]
    fn edits_count_their_lines_and_name_their_file() {
        let mut chat = AcpConversation::default();
        chat.apply(Update::ToolCall(ToolCall {
            id: "e".into(),
            title: "Edit main.rs".into(),
            kind: "edit".into(),
            status: "completed".into(),
            locations: vec!["/p/src/main.rs".into()],
            content: vec![ToolContent::Diff { path: "/p/src/main.rs".into(), old: Some("a\nb\n".into()), new: "a\nc\nd\n".into() }],
            ..ToolCall::default()
        }));
        let row = &chat.rows[0];
        assert_eq!(row.path.as_deref(), Some("/p/src/main.rs"));
        assert_eq!((row.added, row.removed), (Some(2), Some(1)));
        let step = crate::chat::steps::step_from_row(row, false);
        assert_eq!(step.kind, crate::chat::layout::StepKind::Edit);
    }

    #[test]
    fn a_codex_patch_counts_its_lines() {
        let mut chat = AcpConversation::default();
        chat.apply(Update::ToolCall(ToolCall {
            id: "f".into(),
            kind: "edit".into(),
            status: "completed".into(),
            content: vec![ToolContent::Patch { path: "/p/a.rs".into(), unified: "--- a/a.rs\n+++ b/a.rs\n-old\n+new\n+more\n".into() }],
            ..ToolCall::default()
        }));
        let row = &chat.rows[0];
        assert_eq!((row.added, row.removed), (Some(2), Some(1)));
        assert_eq!(row.path.as_deref(), Some("/p/a.rs"));
    }

    #[test]
    fn earlier_history_comes_first_and_new_rows_follow() {
        let mut chat = AcpConversation::default();
        chat.preload(vec![Row::new("old-1".into(), "user", "before".into()), Row::new("old-2".into(), "assistant", "reply".into())]);
        chat.you("now");
        assert_eq!(chat.rows.len(), 3);
        assert_eq!(chat.rows[2].text, "now");
        assert!(chat.rows.iter().map(|r| &r.id).collect::<std::collections::HashSet<_>>().len() == 3, "ids stay unique");
    }

    #[test]
    fn a_stopped_turn_leaves_no_tool_running() {
        let mut chat = AcpConversation::default();
        chat.apply(Update::ToolCall(ToolCall { id: "t".into(), kind: "execute".into(), status: "in_progress".into(), ..ToolCall::default() }));
        assert!(chat.rows[0].running);
        chat.end_turn();
        assert!(!chat.rows[0].running);
    }

    #[test]
    fn a_loaded_conversation_replays_your_messages() {
        let mut chat = AcpConversation::default();
        chat.apply(Update::UserText { text: "fix ".into() });
        chat.apply(Update::UserText { text: "the bug".into() });
        chat.apply(text(None, "Done."));
        assert_eq!(chat.rows[0].text, "fix the bug");
        assert_eq!(chat.rows.len(), 2);
    }
}
