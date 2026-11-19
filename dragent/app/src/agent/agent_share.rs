//! Project agent blocks to the shared-session wire form.
//!
//! The desktop `AgentBlockKind` is a rich, interactive model. A remote viewer
//! only *displays* the agent turn (approvals/questions/plans are resolved on the
//! host), so this maps each block to the read-only [`WireAgentBlock`] projection,
//! flattening status enums to strings and attaching captured edit diffs.

use dragent_session_protocol::{
    WireAgentAuthor, WireAgentBlock, WireAgentKind, WireDiff, WireDiffLine, WireFileDiff,
};

use super::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionOption, QuestionStatus, ToolStepStatus,
};
use super::edit_diff::{DiffLine, DiffLineKind, EditDiff, FileDiff};

pub fn wire_agent_blocks(blocks: &[AgentBlock]) -> Vec<WireAgentBlock> {
    blocks.iter().map(wire_block).collect()
}

/// Count every visible agent block, including nested turn/activity groups.
/// Structural growth is a lifecycle event and should bypass token throttling.
pub fn agent_block_count(blocks: &[AgentBlock]) -> usize {
    blocks
        .iter()
        .map(|block| {
            1 + match &block.kind {
                AgentBlockKind::TurnGroup { children, .. } => agent_block_count(children),
                AgentBlockKind::ActivityGroup { blocks, .. } => agent_block_count(blocks),
                _ => 0,
            }
        })
        .sum()
}

/// Rehydrate the host's read-only wire projection for a remote/WASM mirror.
/// Interactive IDs are intentionally local placeholders: approvals, questions,
/// and plans continue to be resolved on the host desktop.
pub fn agent_blocks_from_wire(blocks: Vec<WireAgentBlock>) -> Vec<AgentBlock> {
    blocks.into_iter().map(block_from_wire).collect()
}

fn block_from_wire(block: WireAgentBlock) -> AgentBlock {
    AgentBlock::new_with_timestamp(
        block.id,
        uuid::Uuid::nil(),
        kind_from_wire(block.id, block.kind),
        block.ts_ms,
    )
}

fn kind_from_wire(id: usize, kind: WireAgentKind) -> AgentBlockKind {
    match kind {
        WireAgentKind::UserQuery { text, author } => AgentBlockKind::UserQuery {
            text,
            author: author.map(|author| super::controller::block::AgentAuthor {
                display_name: author.display_name,
                avatar_url: author.avatar_url,
            }),
            attached_blocks: Vec::new(),
            attachments: Vec::new(),
            checkpoint: None,
        },
        WireAgentKind::AgentMessage { text, streaming } => AgentBlockKind::AgentMessage {
            text,
            is_streaming: streaming,
        },
        WireAgentKind::SystemNote { text } => AgentBlockKind::SystemNote { text },
        WireAgentKind::Thinking { content, complete } => AgentBlockKind::ThinkingStep {
            content,
            complete,
            expanded: false,
        },
        WireAgentKind::ToolStep {
            name,
            icon,
            args,
            status,
            result,
            duration_ms,
            diff,
        } => AgentBlockKind::ToolStep {
            tool_id: format!("remote-{id}"),
            name,
            icon,
            args_summary: args,
            status: match status.as_str() {
                "done" => ToolStepStatus::Done,
                "error" => ToolStepStatus::Error,
                _ => ToolStepStatus::Running,
            },
            result_summary: result,
            duration_ms,
            expanded: false,
            diff: diff.map(diff_from_wire),
        },
        WireAgentKind::ActionResult {
            command,
            exit_code,
            output_preview,
        } => AgentBlockKind::ActionResult {
            action_id: uuid::Uuid::nil(),
            command,
            exit_code,
            output_preview,
        },
        WireAgentKind::Approval {
            command,
            reason,
            status,
        } => {
            if status == "pending" {
                AgentBlockKind::SystemNote {
                    text: format!(
                        "Waiting for approval on the host desktop: {command}\n\n{reason}"
                    ),
                }
            } else {
                AgentBlockKind::ApprovalRequest {
                    action_id: uuid::Uuid::nil(),
                    command,
                    reason,
                    status: match status.as_str() {
                        "approved" => ApprovalStatus::Approved,
                        "approved_always" => ApprovalStatus::ApprovedAlways,
                        _ => ApprovalStatus::Rejected,
                    },
                }
            }
        }
        WireAgentKind::Question {
            header,
            question,
            options,
            status,
        } => {
            if status == "pending" {
                AgentBlockKind::SystemNote {
                    text: format!(
                        "Waiting for an answer on the host desktop — {header}: {question}"
                    ),
                }
            } else {
                AgentBlockKind::UserQuestion {
                    action_id: uuid::Uuid::nil(),
                    questions: vec![QuestionItem {
                        header,
                        question,
                        options: options
                            .into_iter()
                            .map(|label| QuestionOption {
                                label,
                                description: String::new(),
                            })
                            .collect(),
                        multi_select: false,
                        custom: true,
                    }],
                    current: 0,
                    answers: vec![Vec::new()],
                    cursor: 0,
                    status: if status == "answered" {
                        QuestionStatus::Answered
                    } else {
                        QuestionStatus::Dismissed
                    },
                }
            }
        }
        WireAgentKind::PlanReview { content, status } => {
            if status == "pending" {
                AgentBlockKind::SystemNote {
                    text: format!("Waiting for plan approval on the host desktop.\n\n{content}"),
                }
            } else {
                AgentBlockKind::PlanReview {
                    plan_id: uuid::Uuid::nil(),
                    content,
                    status: if status == "approved" {
                        PlanStatus::Approved
                    } else {
                        PlanStatus::Refining
                    },
                }
            }
        }
        WireAgentKind::Queued { text } => AgentBlockKind::QueuedMessage {
            queue_id: uuid::Uuid::nil(),
            text,
        },
        WireAgentKind::Loading { message } => AgentBlockKind::Loading { message },
        WireAgentKind::Error { message } => AgentBlockKind::Error {
            message,
            kind: ErrorKind::Internal,
            task_id: None,
        },
        WireAgentKind::TurnGroup {
            children,
            duration_ms,
            status_note,
            expanded,
        } => AgentBlockKind::TurnGroup {
            children: agent_blocks_from_wire(children),
            expanded,
            started_at_ms: 0,
            duration_ms,
            status_note,
        },
        WireAgentKind::ActivityGroup { blocks, expanded } => AgentBlockKind::ActivityGroup {
            blocks: agent_blocks_from_wire(blocks),
            expanded,
        },
        WireAgentKind::Empty => AgentBlockKind::Empty,
    }
}

fn diff_from_wire(diff: WireDiff) -> EditDiff {
    EditDiff {
        files: diff
            .files
            .into_iter()
            .map(|file| FileDiff {
                path: file.path,
                added: file.added,
                removed: file.removed,
                truncated: file.truncated,
                lines: file
                    .lines
                    .into_iter()
                    .map(|line| DiffLine {
                        kind: match line.kind.as_str() {
                            "add" => DiffLineKind::Add,
                            "remove" => DiffLineKind::Remove,
                            _ => DiffLineKind::Context,
                        },
                        text: line.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub(crate) fn wire_block(block: &AgentBlock) -> WireAgentBlock {
    WireAgentBlock {
        id: block.id,
        ts_ms: block.created_at_unix_ms,
        kind: wire_kind(&block.kind),
    }
}

fn wire_kind(kind: &AgentBlockKind) -> WireAgentKind {
    match kind {
        AgentBlockKind::UserQuery { text, author, .. } => WireAgentKind::UserQuery {
            text: text.clone(),
            author: author.as_ref().map(|author| WireAgentAuthor {
                display_name: author.display_name.clone(),
                avatar_url: author.avatar_url.clone(),
            }),
        },
        AgentBlockKind::AgentMessage { text, is_streaming } => WireAgentKind::AgentMessage {
            text: text.clone(),
            streaming: *is_streaming,
        },
        AgentBlockKind::SystemNote { text } => WireAgentKind::SystemNote { text: text.clone() },
        AgentBlockKind::ThinkingStep {
            content, complete, ..
        } => WireAgentKind::Thinking {
            content: content.clone(),
            complete: *complete,
        },
        AgentBlockKind::ToolStep {
            tool_id,
            name,
            icon,
            args_summary,
            status,
            result_summary,
            duration_ms,
            diff,
            ..
        } => WireAgentKind::ToolStep {
            name: name.clone(),
            icon: icon.clone(),
            args: args_summary.clone(),
            status: tool_status(status).to_string(),
            result: result_summary.clone(),
            duration_ms: *duration_ms,
            // Prefer the persisted diff; fall back to the live capture registry.
            diff: diff
                .clone()
                .or_else(|| super::edit_diff::get(tool_id))
                .map(wire_diff),
        },
        AgentBlockKind::ActionResult {
            command,
            exit_code,
            output_preview,
            ..
        } => WireAgentKind::ActionResult {
            command: command.clone(),
            exit_code: *exit_code,
            output_preview: output_preview.clone(),
        },
        AgentBlockKind::ApprovalRequest {
            command,
            reason,
            status,
            ..
        } => WireAgentKind::Approval {
            command: command.clone(),
            reason: reason.clone(),
            status: approval_status(status).to_string(),
        },
        AgentBlockKind::UserQuestion {
            questions,
            current,
            status,
            ..
        } => {
            let q = questions.get(*current).or_else(|| questions.first());
            WireAgentKind::Question {
                header: q.map(|q| q.header.clone()).unwrap_or_default(),
                question: q.map(|q| q.question.clone()).unwrap_or_default(),
                options: q
                    .map(|q| q.options.iter().map(|o| o.label.clone()).collect())
                    .unwrap_or_default(),
                status: question_status(status).to_string(),
            }
        }
        AgentBlockKind::PlanReview {
            content, status, ..
        } => WireAgentKind::PlanReview {
            content: content.clone(),
            status: plan_status(status).to_string(),
        },
        AgentBlockKind::QueuedMessage { text, .. } => WireAgentKind::Queued { text: text.clone() },
        AgentBlockKind::Loading { message } => WireAgentKind::Loading {
            message: message.clone(),
        },
        AgentBlockKind::Error { message, .. } => WireAgentKind::Error {
            message: message.clone(),
        },
        AgentBlockKind::TurnGroup {
            children,
            expanded,
            duration_ms,
            status_note,
            ..
        } => WireAgentKind::TurnGroup {
            children: wire_agent_blocks(children),
            duration_ms: *duration_ms,
            status_note: status_note.clone(),
            expanded: *expanded,
        },
        AgentBlockKind::ActivityGroup { blocks, expanded } => WireAgentKind::ActivityGroup {
            blocks: wire_agent_blocks(blocks),
            expanded: *expanded,
        },
        // No Todos wire variant: share links flatten the list to readable
        // text (same precedent as pending questions → SystemNote).
        AgentBlockKind::TodoList { todos } => WireAgentKind::SystemNote {
            text: todos
                .iter()
                .map(|t| {
                    let marker = match t.status.as_str() {
                        "completed" => "✓",
                        "in_progress" => "•",
                        _ => " ",
                    };
                    format!("[{marker}] {}", t.content)
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        AgentBlockKind::Empty => WireAgentKind::Empty,
    }
}

fn wire_diff(diff: EditDiff) -> WireDiff {
    WireDiff {
        files: diff
            .files
            .into_iter()
            .map(|f| WireFileDiff {
                path: f.path,
                added: f.added,
                removed: f.removed,
                truncated: f.truncated,
                lines: f
                    .lines
                    .into_iter()
                    .map(|l| WireDiffLine {
                        kind: diff_line_kind(&l.kind).to_string(),
                        text: l.text,
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn diff_line_kind(k: &DiffLineKind) -> &'static str {
    match k {
        DiffLineKind::Context => "context",
        DiffLineKind::Add => "add",
        DiffLineKind::Remove => "remove",
    }
}

fn tool_status(s: &ToolStepStatus) -> &'static str {
    match s {
        ToolStepStatus::Running => "running",
        ToolStepStatus::Done => "done",
        ToolStepStatus::Error => "error",
    }
}

fn approval_status(s: &ApprovalStatus) -> &'static str {
    match s {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::ApprovedAlways => "approved_always",
        ApprovalStatus::Rejected => "rejected",
    }
}

fn question_status(s: &QuestionStatus) -> &'static str {
    match s {
        QuestionStatus::Pending => "pending",
        QuestionStatus::Answered => "answered",
        QuestionStatus::Dismissed => "dismissed",
    }
}

fn plan_status(s: &PlanStatus) -> &'static str {
    match s {
        PlanStatus::Pending => "pending",
        PlanStatus::Approved => "approved",
        PlanStatus::Refining => "refining",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_host_decisions_are_read_only_notes_in_a_mirror() {
        let blocks = agent_blocks_from_wire(vec![WireAgentBlock {
            id: 7,
            ts_ms: 42,
            kind: WireAgentKind::Approval {
                command: "npm test".into(),
                reason: "Run the focused suite".into(),
                status: "pending".into(),
            },
        }]);

        assert_eq!(blocks[0].id, 7);
        assert_eq!(blocks[0].created_at_unix_ms, 42);
        match &blocks[0].kind {
            AgentBlockKind::SystemNote { text } => {
                assert!(text.contains("Waiting for approval on the host desktop"));
                assert!(text.contains("npm test"));
            }
            other => panic!("pending remote approval must not be interactive: {other:?}"),
        }
    }

    #[test]
    fn recursive_block_count_detects_new_nested_turn_content() {
        let child = AgentBlock::new_with_timestamp(
            2,
            uuid::Uuid::new_v4(),
            AgentBlockKind::AgentMessage {
                text: "Ready".into(),
                is_streaming: true,
            },
            2,
        );
        let group = AgentBlock::new_with_timestamp(
            1,
            uuid::Uuid::new_v4(),
            AgentBlockKind::TurnGroup {
                children: vec![child],
                expanded: true,
                started_at_ms: 1,
                duration_ms: None,
                status_note: None,
            },
            1,
        );

        assert_eq!(agent_block_count(&[group]), 2);
    }

    #[test]
    fn prompt_author_survives_the_shared_wire_round_trip() {
        let original = AgentBlock::new_with_timestamp(
            9,
            uuid::Uuid::new_v4(),
            AgentBlockKind::UserQuery {
                text: "Review this".into(),
                author: Some(super::super::controller::AgentAuthor {
                    display_name: "Ada".into(),
                    avatar_url: "https://example.test/ada.png".into(),
                }),
                attached_blocks: Vec::new(),
                attachments: Vec::new(),
                checkpoint: None,
            },
            10,
        );

        let mirrored = agent_blocks_from_wire(wire_agent_blocks(&[original]));
        match &mirrored[0].kind {
            AgentBlockKind::UserQuery {
                author: Some(author),
                ..
            } => {
                assert_eq!(author.display_name, "Ada");
                assert_eq!(author.avatar_url, "https://example.test/ada.png");
            }
            other => panic!("prompt author was not preserved: {other:?}"),
        }
    }

    #[test]
    fn tool_diff_survives_the_wire_projection() {
        let original = AgentBlock::new_with_timestamp(
            3,
            uuid::Uuid::new_v4(),
            AgentBlockKind::ToolStep {
                tool_id: "edit-3".into(),
                name: "apply_patch".into(),
                icon: "edit".into(),
                args_summary: "src/main.rs".into(),
                status: ToolStepStatus::Done,
                result_summary: Some("updated".into()),
                duration_ms: Some(12),
                expanded: true,
                diff: Some(EditDiff {
                    files: vec![FileDiff {
                        path: "src/main.rs".into(),
                        added: 1,
                        removed: 0,
                        truncated: false,
                        lines: vec![DiffLine {
                            kind: DiffLineKind::Add,
                            text: "+fn main() {}".into(),
                        }],
                    }],
                }),
            },
            99,
        );

        let mirrored = agent_blocks_from_wire(wire_agent_blocks(&[original]));
        match &mirrored[0].kind {
            AgentBlockKind::ToolStep {
                diff: Some(diff), ..
            } => {
                assert_eq!(diff.files[0].path, "src/main.rs");
                assert!(matches!(diff.files[0].lines[0].kind, DiffLineKind::Add));
            }
            other => panic!("tool diff was not preserved: {other:?}"),
        }
    }
}
