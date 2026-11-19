//! Conversation document renderer. Messages and activity flow in order, without
//! shell headers or per-message terminal cards. Interactive approval/question
//! components retain their existing action handlers and persisted identities.
use crate::agent::controller::block::{AgentBlock, AgentBlockKind};
use crate::terminal::render::{markdown::render_markdown_agent_document, BlockRenderContext};
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;
use std::collections::HashSet;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, Hoverable, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::platform::Cursor;

/// The gap between one message and the next, close to the blank line a
/// terminal agent leaves between its turns.
pub const MESSAGE_GAP: f32 = 18.0;

pub fn render(
    block: &AgentBlock,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
    checkpoints: &HashSet<usize>,
) -> Box<dyn Element> {
    match &block.kind {
        AgentBlockKind::AgentMessage { text, is_streaming } => {
            message(block, text, *is_streaming, true, ctx, theme, mono)
        }
        AgentBlockKind::ActionResult {
            command,
            exit_code,
            output_preview,
            ..
        } => {
            let label = exit_code
                .map(|code| format!("Command · exit {code}"))
                .unwrap_or_else(|| "Command result".into());
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(
                    Text::new(label, mono, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(
                    super::agent_presentation::command_text(
                        "",
                        command,
                        mono,
                        crate::theme::text_size(13.0),
                        &theme,
                    )
                    .finish(),
                )
                .with_child(output_panel(
                    &format!("action:{}", block.id),
                    output_preview,
                    ctx,
                    theme,
                    mono,
                ))
                .finish()
        }
        AgentBlockKind::TurnGroup {
            children,
            expanded,
            duration_ms,
            status_note,
            ..
        } => {
            let label = match duration_ms {
                None => "Dragent · Working".to_string(),
                Some(ms) => format!(
                    "Dragent · {}{}",
                    status_note
                        .as_ref()
                        .map(|note| format!("{note} · "))
                        .unwrap_or_default(),
                    super::agent_block_chrome::conversation_receipt(children, *ms)
                ),
            };
            group(
                block.id,
                true,
                duration_ms.is_some(),
                &label,
                children,
                *expanded,
                ctx,
                theme,
                mono,
                checkpoints,
            )
        }
        AgentBlockKind::ActivityGroup { blocks, expanded } => group(
            block.id,
            false,
            false,
            &format!("{} actions", blocks.len()),
            blocks,
            *expanded,
            ctx,
            theme,
            mono,
            checkpoints,
        ),
        _ => super::agent_block_element::build_interactive_agent_block(
            block,
            ctx,
            theme,
            mono,
            checkpoints,
        ),
    }
}

fn label(
    text: impl Into<String>,
    font: FamilyId,
    size: f32,
    color: warpui::color::ColorU,
) -> Box<dyn Element> {
    Text::new(text.into(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

fn message(
    block: &AgentBlock,
    text: &str,
    streaming: bool,
    response: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    if text.trim().is_empty() {
        return Empty::new().finish();
    }
    let ui = crate::fonts::ui_role(mono);
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(render_markdown_agent_document(
            text, block.id, ctx, theme, ui, mono, !streaming,
        ));
    if response {
        let copy_text = text.to_string();
        let copy_key = format!("copy-response:{}", block.id);
        let copied = ctx.is_copied(&copy_key);
        let copy = Hoverable::new(
            ctx.mouse_state(&copy_key),
            move |state| {
                label(
                    if copied { "Copied" } else { "Copy response" },
                    ui,
                    11.0,
                    if copied {
                        theme.ui_green()
                    } else if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    },
                )
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event, _, _| {
            event.dispatch_typed_action(TerminalAction::CopyWithConfirmation {
                key: copy_key.clone(),
                text: copy_text.clone(),
            })
        });
        column = column.with_child(Box::new(copy));
    }
    column.finish()
}

fn group(
    id: usize,
    turn: bool,
    finished: bool,
    summary: &str,
    children: &[AgentBlock],
    expanded: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
    checkpoints: &HashSet<usize>,
) -> Box<dyn Element> {
    let ui = crate::fonts::ui_role(mono);
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0);
    if turn {
        let summary = summary.strip_prefix("Dragent · ").unwrap_or(summary);
        let identity = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0)
            .with_child(
                Text::new("Dragent", ui, crate::theme::text_size(13.5))
                    .with_color(theme.main_text())
                    .with_style(warpui::fonts::Properties {
                        weight: warpui::fonts::Weight::Semibold,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_child(label(summary, ui, 11.5, theme.sub_text()))
            .finish();
        let icon = Container::new(crate::icons::render_icon(
            crate::icons::AppIcon::Sparkle,
            theme.accent,
            14.0,
        ))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(theme.accent_overlay())
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(6.0),
        ))
        .finish();
        column = column.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(icon)
                .with_child(Box::new(Expanded::new(1.0, identity)))
                .finish(),
        );
    }
    let final_message = if finished {
        children.iter().rposition(|child| matches!(&child.kind, AgentBlockKind::AgentMessage { text, .. } if !text.trim().is_empty()))
    } else {
        None
    };
    let mut index = 0;
    while index < children.len() {
        let child = &children[index];
        if let AgentBlockKind::AgentMessage { text, is_streaming } = &child.kind {
            if text.trim().is_empty() {
                index += 1;
                continue;
            }
            column = column.with_child(message(
                child,
                text,
                *is_streaming,
                final_message == Some(index),
                ctx,
                theme,
                mono,
            ));
            index += 1;
        } else if is_activity(child) {
            let start = index;
            while index < children.len() && is_activity(&children[index]) {
                index += 1;
            }
            column = column.with_child(activity(
                id,
                start,
                &children[start..index],
                expanded,
                ctx,
                theme,
                mono,
                checkpoints,
            ));
        } else {
            if expanded || remains_visible_when_collapsed(child) {
                column = column.with_child(render(child, ctx, theme, mono, checkpoints));
            }
            index += 1;
        }
    }
    column.finish()
}

/// Blocks that draw nothing. They are left out of the conversation so they
/// don't add an empty gap between messages.
pub(crate) fn renders_nothing(block: &AgentBlock) -> bool {
    match &block.kind {
        AgentBlockKind::Empty => true,
        AgentBlockKind::AgentMessage { text, .. } => text.trim().is_empty(),
        _ => false,
    }
}

fn is_activity(block: &AgentBlock) -> bool {
    matches!(
        block.kind,
        AgentBlockKind::ToolStep { .. }
            | AgentBlockKind::ThinkingStep { .. }
            | AgentBlockKind::ActivityGroup { .. }
            | AgentBlockKind::ActionResult { .. }
    )
}

fn activity(
    id: usize,
    start: usize,
    children: &[AgentBlock],
    expanded: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
    checkpoints: &HashSet<usize>,
) -> Box<dyn Element> {
    let ui = crate::fonts::ui_role(mono);
    let mut items = Vec::new();
    for child in children {
        collect_activity(child, &mut items);
    }
    let count = items.len();
    let header = Hoverable::new(
        ctx.mouse_state(&format!("activity:{id}:{start}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(crate::icons::render_icon(
                        crate::icons::AppIcon::Terminal,
                        theme.sub_text(),
                        14.0,
                    ))
                    .with_child(Box::new(Expanded::new(
                        1.0,
                        label(
                            format!(
                                "Activity · {count} {}",
                                if count == 1 { "step" } else { "steps" }
                            ),
                            ui,
                            12.0,
                            theme.main_text(),
                        ),
                    )))
                    .with_child(label(
                        if expanded {
                            "Hide details ▴"
                        } else {
                            "Show details ▾"
                        },
                        ui,
                        11.0,
                        theme.sub_text(),
                    ))
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                warpui::color::ColorU::transparent_black()
            })
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event, _, _| {
        event.dispatch_typed_action(TerminalAction::ToggleBlockExpansion { block_id: id })
    });
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0)
        .with_child(Box::new(header));
    for child in items {
        if expanded || remains_visible_when_collapsed(child) {
            column = column.with_child(render(child, ctx, theme, mono, checkpoints));
        }
    }
    Container::new(column.finish())
        .with_border(warpui_core::scene::Border::left(1.0).with_border_color(theme.outline()))
        .with_padding_left(10.0)
        .finish()
}

fn collect_activity<'a>(block: &'a AgentBlock, items: &mut Vec<&'a AgentBlock>) {
    if let AgentBlockKind::ActivityGroup { blocks, .. } = &block.kind {
        for child in blocks {
            collect_activity(child, items);
        }
    } else {
        items.push(block);
    }
}

pub(crate) fn output_panel(
    key: &str,
    raw: &str,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let scroll = ctx
        .diagram_states
        .borrow_mut()
        .entry(format!("output:{key}"))
        .or_default()
        .scroll
        .clone();
    // Shown in colour; copied as plain text.
    let (text, paint) = super::term_colors::command_output(raw, &theme);
    // Sized to the text's real line height, so short output leaves no blank
    // band under it; long output scrolls inside the panel.
    let size = crate::theme::text_size(12.5);
    let line = size * warpui_core::elements::ui_line_height_ratio();
    let height = ((text.lines().count().max(1) as f32) * line + 12.0).clamp(30.0, 280.0);
    let output = Container::new(
        super::term_colors::painted_text(text.clone(), &paint, mono, size, theme.main_text())
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(4.0)
    .with_padding_bottom(6.0)
    .finish();
    let output = warpui::elements::ClippedScrollable::vertical(
        scroll,
        output,
        warpui::elements::ScrollbarWidth::Auto,
        theme.sub_text().into(),
        theme.main_text().into(),
        theme.surface_1().into(),
    )
    .with_overlayed_scrollbar()
    .finish();
    let output = warpui::elements::ConstrainedBox::new(output)
        .with_height(height)
        .finish();
    let copy_key = format!("copy-output:{key}");
    let copied = ctx.is_copied(&copy_key);
    let copy = Hoverable::new(ctx.mouse_state(&copy_key), move |_| {
        Text::new(
            if copied { "Copied" } else { "Copy" },
            crate::fonts::ui_role(mono),
            crate::theme::text_size(11.0),
        )
        .with_color(if copied {
            theme.ui_green()
        } else {
            theme.sub_text()
        })
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event, _, _| {
        event.dispatch_typed_action(TerminalAction::CopyWithConfirmation {
            key: copy_key.clone(),
            text: text.clone(),
        })
    });
    let header = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Box::new(Expanded::new(
                1.0,
                label(
                    "OUTPUT",
                    crate::fonts::ui_role(mono),
                    10.0,
                    theme.sub_text(),
                ),
            )))
            .with_child(Box::new(copy))
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(4.0)
    .with_padding_bottom(2.0)
    .finish();
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(output)
            .finish(),
    )
    .finish()
}

fn remains_visible_when_collapsed(block: &AgentBlock) -> bool {
    match &block.kind {
        AgentBlockKind::AgentMessage { .. } | AgentBlockKind::Error { .. } => true,
        AgentBlockKind::ApprovalRequest { status, .. } => matches!(
            status,
            crate::agent::controller::block::ApprovalStatus::Pending
        ),
        AgentBlockKind::UserQuestion { status, .. } => matches!(
            status,
            crate::agent::controller::block::QuestionStatus::Pending
        ),
        AgentBlockKind::PlanReview { status, .. } => {
            matches!(status, crate::agent::controller::block::PlanStatus::Pending)
        }
        AgentBlockKind::ToolStep {
            tool_id, status, ..
        } => {
            matches!(
                status,
                crate::agent::controller::block::ToolStepStatus::Running
            ) || crate::agent::local_tools::running_agent_commands()
                .iter()
                .any(|run| &run.id == tool_id)
        }
        AgentBlockKind::ActivityGroup { blocks, .. } => {
            blocks.iter().any(remains_visible_when_collapsed)
        }
        AgentBlockKind::TurnGroup { children, .. } => {
            children.iter().any(remains_visible_when_collapsed)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collapsing_activity_keeps_nested_decisions_and_responses_visible() {
        let block = |id, kind| AgentBlock::new(id, uuid::Uuid::nil(), kind);
        let message = block(
            1,
            AgentBlockKind::AgentMessage {
                text: "Result".into(),
                is_streaming: false,
            },
        );
        assert!(remains_visible_when_collapsed(&message));
        let decision = block(
            2,
            AgentBlockKind::ApprovalRequest {
                action_id: uuid::Uuid::nil(),
                command: "deploy".into(),
                reason: "approval".into(),
                status: crate::agent::controller::block::ApprovalStatus::Pending,
            },
        );
        let group = block(
            3,
            AgentBlockKind::ActivityGroup {
                blocks: vec![decision],
                expanded: false,
            },
        );
        assert!(remains_visible_when_collapsed(&group));
        assert!(!remains_visible_when_collapsed(&block(
            4,
            AgentBlockKind::ThinkingStep {
                content: "Thinking".into(),
                complete: true,
                expanded: false
            }
        )));
    }
    #[test]
    fn command_control_sequences_are_inert_document_text() {
        // Terminal control sequences must never execute against the
        // conversation's layout or leak OSC payloads into visible text.
        let plain_output =
            |raw: &str| super::super::term_colors::command_output(raw, &Theme::default()).0;
        assert_eq!(
            plain_output("\x1b[?1049h\x1b[2J\x1b[31mFAIL\x1b[0m\nnext"),
            "FAIL\nnext"
        );
        assert_eq!(
            plain_output("\x1b]8;;https://example.com\x07label\x1b]8;;\x1b\\"),
            "label"
        );
        assert_eq!(plain_output("你好\n\tpassed"), "你好\n\tpassed");
    }
}
