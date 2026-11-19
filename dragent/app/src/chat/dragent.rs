//! Dragent's own conversation as chat entries. Your messages, replies and
//! steps (tool calls, thoughts, commands) take the shared chat layout;
//! everything that needs you or stands on its own (approvals, questions,
//! plans, errors, task lists, notes) stays a card drawn by its existing
//! renderer, so its buttons keep working.

use super::layout::{first_line, Entry, Step, StepKind};
use super::steps::step_from_row;
use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, PlanStatus, QuestionStatus, ToolStepStatus,
};
use dragent_host::agent_transcript::Row;

/// A Dragent tool call as a step.
fn tool_step(block: &AgentBlock) -> Option<Step> {
    let AgentBlockKind::ToolStep { tool_id, name, args_summary, status, result_summary, diff, .. } = &block.kind else {
        return None;
    };
    let diff = diff.clone().or_else(|| crate::agent::edit_diff::get(tool_id));
    let mut row = Row::new(format!("tool:{}", block.id), "tool", args_summary.clone());
    row.tool = Some(name.clone());
    row.running = matches!(status, ToolStepStatus::Running);
    row.error = matches!(status, ToolStepStatus::Error);
    row.at = Some(block.created_at_unix_ms);
    if let Some(diff) = diff.as_ref().filter(|d| !d.files.is_empty()) {
        row.path = Some(diff.files[0].path.clone());
        row.added = Some(diff.total_added() as u32);
        row.removed = Some(diff.total_removed() as u32);
    }
    let mut step = step_from_row(&row, true);
    // An edit reads as one whatever the tool's name.
    if let Some(diff) = diff.as_ref().filter(|d| !d.files.is_empty()) {
        if step.kind != StepKind::Edit {
            let path = diff.files[0].path.clone();
            step.kind = StepKind::Edit;
            step.verb = Some(if step.running { "Editing" } else { "Edited" }.into());
            step.file = Some(path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string());
            step.label = String::new();
            step.mono = false;
            step.detail = Some(path);
        }
    }
    if step.detail.is_none() {
        step.detail = result_summary.clone().filter(|r| !r.trim().is_empty());
    }
    Some(step)
}

fn push(block: &AgentBlock, out: &mut Vec<Entry<AgentBlock>>) {
    let at = Some(block.created_at_unix_ms);
    let id = format!("b{}", block.id);
    match &block.kind {
        AgentBlockKind::UserQuery { text, .. } => out.push(Entry::You { id: block.id.to_string(), text: text.clone(), at, note: None }),
        AgentBlockKind::QueuedMessage { text, .. } => {
            out.push(Entry::You { id: block.id.to_string(), text: text.clone(), at, note: Some(super::layout::YouNote::Queued) })
        }
        AgentBlockKind::AgentMessage { text, is_streaming } => {
            if !text.trim().is_empty() || *is_streaming {
                out.push(Entry::Reply { id, text: text.clone(), at, streaming: *is_streaming });
            }
        }
        AgentBlockKind::ToolStep { .. } => {
            if let Some(step) = tool_step(block) {
                out.push(Entry::Step { step, at });
            }
        }
        AgentBlockKind::ThinkingStep { content, complete, .. } => {
            let label = if *complete { first_line(content, 120) } else { "Thinking…".into() };
            out.push(Entry::Step {
                step: Step {
                    id,
                    kind: StepKind::Think,
                    verb: None,
                    file: None,
                    label: if label.is_empty() { "Thought".into() } else { label },
                    mono: false,
                    detail: (*complete && content.chars().count() > 120).then(|| content.clone()),
                    added: None,
                    removed: None,
                    running: !complete,
                    failed: false,
                    started_at: at,
                },
                at,
            });
        }
        AgentBlockKind::ActionResult { command, exit_code, output_preview, .. } => out.push(Entry::Step {
            step: Step {
                id,
                kind: StepKind::Execute,
                verb: None,
                file: None,
                label: first_line(command, 200),
                mono: true,
                detail: (!output_preview.trim().is_empty()).then(|| format!("$ {command}\n{output_preview}")),
                added: None,
                removed: None,
                running: false,
                failed: exit_code.is_some_and(|code| code != 0),
                started_at: at,
            },
            at,
        }),
        AgentBlockKind::ActivityGroup { blocks, .. } => {
            for child in blocks {
                push(child, out);
            }
        }
        AgentBlockKind::TurnGroup { children, started_at_ms, duration_ms, .. } => {
            for child in children {
                push(child, out);
            }
            if let Some(ms) = duration_ms {
                out.push(Entry::End { id: format!("{id}:end"), at: Some(started_at_ms + ms), duration_ms: Some(*ms) });
            }
        }
        AgentBlockKind::Loading { .. } | AgentBlockKind::Empty => {}
        _ => out.push(Entry::Card { id, card: block.clone(), at }),
    }
}

/// Dragent's blocks as chat entries.
pub fn entries_from_blocks(blocks: &[AgentBlock]) -> Vec<Entry<AgentBlock>> {
    let mut out = Vec::new();
    for block in blocks {
        push(block, &mut out);
    }
    out
}

/// Something in the conversation waits on you: an approval, a question, a plan.
pub fn waiting_on_you(blocks: &[AgentBlock]) -> bool {
    fn pending(block: &AgentBlock) -> bool {
        match &block.kind {
            AgentBlockKind::ApprovalRequest { status, .. } => matches!(status, ApprovalStatus::Pending),
            AgentBlockKind::UserQuestion { status, .. } => matches!(status, QuestionStatus::Pending),
            AgentBlockKind::PlanReview { status, .. } => matches!(status, PlanStatus::Pending),
            AgentBlockKind::TurnGroup { children, .. } => children.iter().any(pending),
            AgentBlockKind::ActivityGroup { blocks, .. } => blocks.iter().any(pending),
            _ => false,
        }
    }
    blocks.iter().any(pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::layout::{layout, Line};
    use uuid::Uuid;

    fn block(id: usize, kind: AgentBlockKind) -> AgentBlock {
        AgentBlock::new(id, Uuid::nil(), kind)
    }

    #[test]
    fn a_dragent_turn_reads_as_a_chat() {
        let blocks = vec![
            block(1, AgentBlockKind::UserQuery { text: "Fix it".into(), author: None, attached_blocks: vec![], attachments: vec![], checkpoint: None }),
            block(
                2,
                AgentBlockKind::TurnGroup {
                    children: vec![
                        block(3, AgentBlockKind::ToolStep {
                            tool_id: "t1".into(),
                            name: "run_command".into(),
                            icon: String::new(),
                            args_summary: "cargo test".into(),
                            status: ToolStepStatus::Done,
                            result_summary: None,
                            duration_ms: Some(10),
                            expanded: false,
                            diff: None,
                        }),
                        block(4, AgentBlockKind::AgentMessage { text: "Done.".into(), is_streaming: false }),
                        block(5, AgentBlockKind::ApprovalRequest {
                            action_id: Uuid::nil(),
                            command: "rm -rf build".into(),
                            reason: String::new(),
                            status: ApprovalStatus::Pending,
                        }),
                    ],
                    expanded: false,
                    started_at_ms: 1_000,
                    duration_ms: Some(5_000),
                    status_note: None,
                },
            ),
        ];
        let entries = entries_from_blocks(&blocks);
        let lines = layout(&entries, false, waiting_on_you(&blocks));
        let shape: Vec<&str> = lines
            .iter()
            .map(|line| match line {
                Line::You { .. } => "you",
                Line::Clock { .. } => "clock",
                Line::Step { .. } => "step",
                Line::Run { .. } => "run",
                Line::Reply { .. } => "reply",
                Line::Card { .. } => "card",
                Line::Changes { .. } => "changes",
            })
            .collect();
        assert_eq!(shape, ["you", "clock", "step", "reply", "card"]);
        assert!(waiting_on_you(&blocks));
    }
}
