//! Renderer for agent conversation blocks (messages, approval cards, results).

use uuid::Uuid;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    Hoverable, MainAxisSize, ParentElement, SavePosition, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::markdown::{render_markdown, render_markdown_agent, strip_emoji_str};

use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionStatus, TodoListItem, ToolStepStatus,
};
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

pub(crate) const ACCENT_WIDTH: f32 = 3.0;

pub fn build_agent_block_element(
    block: &AgentBlock,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
    live_checkpoint_ids: &std::collections::HashSet<usize>,
) -> Box<dyn Element> {
    // Full width from the pane's left edge, like the terminal output beside it.
    super::agent_transcript::render(block, ctx, theme, mono, live_checkpoint_ids)
}

pub(crate) fn build_interactive_agent_block(
    block: &AgentBlock, ctx: &BlockRenderContext<'_>, theme: Theme, mono: FamilyId,
    live_checkpoint_ids: &std::collections::HashSet<usize>,
) -> Box<dyn Element> {
    match &block.kind {
        AgentBlockKind::UserQuery {
            text,
            author,
            attached_blocks,
            attachments,
            checkpoint: _,
        } => {
            // The in-memory store is authoritative. Requiring the separately
            // persisted marker made the restore action disappear when a live
            // checkpoint was attached after the message block was created.
            let has_live_checkpoint = live_checkpoint_ids.contains(&block.id);
            build_user_query(
                block.id,
                text,
                author.as_ref(),
                attached_blocks,
                attachments,
                has_live_checkpoint,
                ctx,
                theme,
                mono,
            )
        }
        AgentBlockKind::SystemNote { text } => build_system_note(text, theme, mono),
        AgentBlockKind::AgentMessage { text, is_streaming } => {
            build_agent_message(text, *is_streaming, block, ctx, theme, mono)
        }
        AgentBlockKind::ApprovalRequest {
            action_id,
            command,
            reason,
            status,
        } => build_approval_request(
            *action_id, command, reason, status, block.id, ctx, theme, mono,
        ),
        AgentBlockKind::UserQuestion {
            action_id,
            questions,
            current,
            answers,
            status,
            cursor,
            ..
        } => build_user_question(
            *action_id, questions, *current, answers, status, *cursor, block.id, ctx, theme, mono,
        ),
        AgentBlockKind::ActionResult {
            command,
            exit_code,
            output_preview,
            ..
        } => build_action_result(command, *exit_code, output_preview, theme, mono),
        AgentBlockKind::TodoList { todos } => build_todo_list(todos, theme, mono),
        AgentBlockKind::ToolStep {
            tool_id,
            name,
            icon,
            args_summary,
            status,
            result_summary,
            duration_ms,
            expanded,
            diff,
            ..
        } => build_tool_step(
            block.id,
            tool_id,
            *expanded,
            name,
            icon,
            args_summary,
            status,
            result_summary.as_deref(),
            *duration_ms,
            diff.as_ref(),
            ctx,
            theme,
            mono,
        ),
        AgentBlockKind::ThinkingStep {
            content,
            complete,
            expanded,
        } => build_thinking_step(block.id, content, *complete, *expanded, ctx, theme, mono),
        AgentBlockKind::PlanReview {
            plan_id,
            content,
            status,
        } => build_plan_review(*plan_id, content, status, block.id, ctx, theme, mono),
        AgentBlockKind::QueuedMessage { queue_id, text } => {
            build_queued_message(*queue_id, text, block.id, ctx, theme, mono)
        }
        AgentBlockKind::Loading { message } => build_loading(message, block, ctx, theme, mono),
        AgentBlockKind::Error { message, kind, .. } => {
            build_error_card(message, kind, block.id, ctx, theme, mono)
        }
        AgentBlockKind::Empty => Empty::new().finish(),
        AgentBlockKind::ActivityGroup { blocks, expanded } => {
            build_activity_group(block.id, blocks, *expanded, ctx, theme, mono)
        }
        AgentBlockKind::TurnGroup {
            children,
            expanded,
            started_at_ms,
            duration_ms,
            status_note,
        } => build_turn_group(
            block.id,
            children,
            *expanded,
            *started_at_ms,
            *duration_ms,
            status_note.as_deref(),
            ctx,
            theme,
            mono,
        ),
    }
}
pub(crate) use super::agent_block_approval::*;
pub(crate) use super::agent_block_chrome::*;
pub(crate) use super::agent_block_message::*;
pub(crate) use super::agent_block_steps::*;
