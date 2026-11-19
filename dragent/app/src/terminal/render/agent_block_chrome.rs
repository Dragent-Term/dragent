//! Renderer for agent conversation blocks (messages, approval cards, results).

use uuid::Uuid;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    Hoverable, MainAxisSize, ParentElement, SavePosition, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::markdown::{render_markdown, render_markdown_agent, strip_emoji_str};

use super::agent_block_element::ACCENT_WIDTH;
use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionStatus, TodoListItem, ToolStepStatus,
};
use crate::terminal::render::agent_block_steps::build_action_result;
use crate::terminal::render::agent_block_steps::build_loading;
use crate::terminal::render::agent_block_steps::build_thinking_step;
use crate::terminal::render::agent_block_steps::build_todo_list;
use crate::terminal::render::agent_block_steps::build_tool_step;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

pub(crate) fn build_plan_review(
    plan_id: Uuid,
    content: &str,
    status: &PlanStatus,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use crate::terminal::view::actions::TerminalAction;

    let accent = theme.accent;
    let header = agent_block_header("Agent · Plan", accent, theme, mono);

    let plan_text = render_markdown(content, theme, mono);

    let body_inner: Box<dyn Element> = match status {
        PlanStatus::Pending => {
            // "Implement" button — filled accent
            let implement_btn = {
                let mouse_state = ctx.mouse_state(&format!("plan-{block_id}-implement"));
                let btn_inner = Container::new(
                    Text::new(
                        "Implement".to_string(),
                        mono,
                        crate::theme::text_size(13.5),
                    )
                    .with_color(theme.accent_text())
                    .with_style(warpui::fonts::Properties {
                        weight: warpui::fonts::Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(accent)
                .with_border(warpui_core::scene::Border::all(1.0).with_border_color(accent))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .finish();
                let action = TerminalAction::ApprovePlan { plan_id };
                Box::new(
                    Hoverable::new(mouse_state, move |_| btn_inner)
                        .with_cursor(warpui::platform::Cursor::PointingHand)
                        .on_click(move |event_ctx, _app, _pos| {
                            event_ctx.dispatch_typed_action(action.clone());
                        }),
                ) as Box<dyn Element>
            };

            // "Refine" button — ghost style
            let refine_btn = {
                let mouse_state = ctx.mouse_state(&format!("plan-{block_id}-refine"));
                let btn_inner = Container::new(
                    Text::new("Refine".to_string(), mono, crate::theme::text_size(13.5))
                        .with_color(theme.main_text())
                        .with_style(warpui::fonts::Properties {
                            weight: warpui::fonts::Weight::Medium,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_background_color(theme.surface_3())
                .with_border(
                    warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()),
                )
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(4.0),
                ))
                .finish();
                let action = TerminalAction::RefinePlan { plan_id };
                Box::new(
                    Hoverable::new(mouse_state, move |_| btn_inner)
                        .with_cursor(warpui::platform::Cursor::PointingHand)
                        .on_click(move |event_ctx, _app, _pos| {
                            event_ctx.dispatch_typed_action(action.clone());
                        }),
                ) as Box<dyn Element>
            };

            let buttons = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(implement_btn)
                .with_child(refine_btn)
                .finish();

            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(10.0)
                .with_child(plan_text)
                .with_child(buttons)
                .finish()
        }
        PlanStatus::Approved => Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.0)
            .with_child(plan_text)
            .with_child(status_badge("Implementing…", theme.ui_green(), theme, mono))
            .finish(),
        PlanStatus::Refining => Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.0)
            .with_child(plan_text)
            .with_child(status_badge(
                "Refining — type your changes below",
                theme.accent,
                theme,
                mono,
            ))
            .finish(),
    };

    let body = Container::new(body_inner)
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(12.0)
        .finish();

    agent_block_card(header, Some(body), accent)
}

/// A user prompt's primary row: author avatar, prompt text, then actions.
/// The author's name remains available to the avatar renderer for fallback
/// initials, without competing visually with the prompt itself.
pub(crate) fn user_prompt_header(
    label: &str,
    avatar_url: &str,
    prompt: &str,
    _accent: ColorU,
    action: Box<dyn Element>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let avatar =
        crate::workspace::share_sheet::avatar_sized(label, "", avatar_url, &theme, mono, 22.0);

    let ui = crate::fonts::ui_role(mono);
    let prompt_el = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(2.0)
        .with_child(Text::new(label.to_string(), ui, crate::theme::text_size(11.0)).with_color(theme.sub_text()).with_style(Properties { weight: Weight::Semibold, ..Default::default() }).finish())
        .with_child(Text::new(prompt.to_string(), ui, crate::theme::text_size(14.0)).with_color(theme.main_text()).finish()).finish();

    let row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(10.0)
        .with_child(avatar)
        .with_child(Box::new(Expanded::new(1.0, prompt_el)))
        .with_child(action)
        .finish();

    // The conversation spaces messages apart, so the row carries no padding.
    row
}

pub(crate) fn agent_block_header(
    label: &str,
    accent: ColorU,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(accent)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();

    let label_el = Text::new(label.to_string(), mono, crate::theme::text_size(14.0))
        .with_color(accent)
        .with_style(Properties {
            weight: Weight::Semibold,
            ..Default::default()
        })
        .finish();

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(label_el)
        .finish();

    let header_body = Container::new(row)
        .with_padding_left(10.0)
        .with_padding_right(13.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)

        .finish();

    Container::new(header_body)

        .finish()
}

pub(crate) fn agent_block_card(
    header: Box<dyn Element>,
    body: Option<Box<dyn Element>>,
    _accent: ColorU,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header);

    if let Some(b) = body {
        col = col.with_child(b);
    }

    col.finish()
}

pub(crate) fn status_badge(
    label: &str,
    color: ColorU,
    _theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let bg = ColorU::new(color.r, color.g, color.b, 30);
    let border = ColorU::new(color.r, color.g, color.b, 60);
    Container::new(
        Text::new(label.to_string(), mono, crate::theme::text_size(12.0))
            .with_color(color)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Default::default()
            })
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .with_background_color(bg)
    .with_border(Border::all(1.0).with_border_color(border))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish()
}

fn activity_group_summary(sub_blocks: &[AgentBlock]) -> String {
    let mut files = 0;
    let mut searches = 0;
    let mut tool_steps = 0;
    let mut thought_steps = 0;

    for b in sub_blocks {
        match &b.kind {
            AgentBlockKind::ToolStep { name, .. } => match name.as_str() {
                "Read File" | "Write File" | "Edit File" | "Batch Edit" | "Apply Patch" => {
                    files += 1;
                }
                "Grep" | "Search Codebase" | "Find" => {
                    searches += 1;
                }
                _ => {
                    tool_steps += 1;
                }
            },
            AgentBlockKind::ThinkingStep { .. } => {
                thought_steps += 1;
            }
            _ => {}
        }
    }

    let mut parts = Vec::new();
    if files > 0 {
        let s = if files == 1 { "file" } else { "files" };
        parts.push(format!("{} {}", files, s));
    }
    if searches > 0 {
        let s = if searches == 1 { "search" } else { "searches" };
        parts.push(format!("{} {}", searches, s));
    }
    if tool_steps > 0 {
        let s = if tool_steps == 1 { "tool" } else { "tools" };
        parts.push(format!("{} {}", tool_steps, s));
    }

    if parts.is_empty() {
        if thought_steps > 0 {
            "Thinking...".to_string()
        } else {
            "Agent Activity".to_string()
        }
    } else {
        parts.join(", ")
    }
}

pub(crate) fn build_activity_group(
    block_id: usize,
    sub_blocks: &[AgentBlock],
    expanded: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use std::time::Duration;
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};

    let mut is_working = false;
    for b in sub_blocks {
        match &b.kind {
            AgentBlockKind::ToolStep {
                status: ToolStepStatus::Running,
                ..
            } => {
                is_working = true;
            }
            AgentBlockKind::ThinkingStep {
                complete: false, ..
            } => {
                is_working = true;
            }
            _ => {}
        }
    }

    let summary = activity_group_summary(sub_blocks);
    let prefix = if is_working { "Exploring" } else { "Explored" };
    let chevron = if expanded { "▾" } else { "▸" };
    let header_text = format!("{chevron} {prefix} {summary}");
    let header_color = if is_working {
        theme.accent
    } else {
        theme.sub_text()
    };

    // Shimmer handle for the "working" spinner — Clone (Arc inside), so safe to capture in FnMut.
    let spinner_handle = if is_working {
        Some(ctx.shimmer_state(&format!("activity-group-spinner-{}", block_id)))
    } else {
        None
    };
    let header_text_clone = header_text.clone();

    let mouse_state = ctx.mouse_state(&format!("activity-group-{}", block_id));
    let header_interactive = Hoverable::new(mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        };

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0);

        if let Some(ref handle) = spinner_handle {
            let spinner_el = ShimmeringTextElement::new(
                "●".to_string(),
                mono,
                crate::theme::text_size(11.0),
                theme.accent,
                ColorU::white(),
                ShimmerConfig {
                    period: Duration::from_millis(800),
                    shimmer_radius: 2,
                    padding: 0,
                },
                handle.clone(),
            )
            .finish();
            row = row.with_child(spinner_el);
        }

        row = row.with_child(
            Text::new(
                header_text_clone.clone(),
                mono,
                crate::theme::text_size(13.5),
            )
            .with_color(header_color)
            .with_style(warpui::fonts::Properties {
                weight: warpui::fonts::Weight::Medium,
                ..Default::default()
            })
            .finish(),
        );

        Container::new(row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::ToggleBlockExpansion { block_id });
    });

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(Box::new(header_interactive));

    if expanded {
        for sub in sub_blocks {
            let el: Box<dyn Element> = match &sub.kind {
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
                    sub.id,
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
                } => build_thinking_step(sub.id, content, *complete, *expanded, ctx, theme, mono),
                AgentBlockKind::ActionResult {
                    command,
                    exit_code,
                    output_preview,
                    ..
                } => build_action_result(command, *exit_code, output_preview, theme, mono),
                AgentBlockKind::ApprovalRequest {
                    command, status, ..
                } => {
                    let label = match status {
                        ApprovalStatus::Approved => "Approved",
                        ApprovalStatus::ApprovedAlways => "Always allowed",
                        ApprovalStatus::Rejected => "Rejected",
                        _ => "Pending",
                    };
                    let badge_color = match status {
                        ApprovalStatus::Rejected => theme.ui_red(),
                        _ => theme.ui_green(),
                    };
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(
                            Text::new(
                                format!("Action: {command}"),
                                mono,
                                crate::theme::text_size(13.0),
                            )
                            .with_color(theme.sub_text())
                            .finish(),
                        )
                        .with_child(status_badge(label, badge_color, theme, mono))
                        .finish() as Box<dyn Element>
                }
                _ => Empty::new().finish() as Box<dyn Element>,
            };

            col = col.with_child(Container::new(el).with_padding_left(16.0).finish());
        }
    }

    Container::new(col.finish())
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}

/// Aggregate stats over a turn's children for the receipt summary.
struct TurnStats {
    files_edited: usize,
    added: usize,
    removed: usize,
    commands: usize,
    reads: usize,
}

fn turn_stats(children: &[AgentBlock]) -> TurnStats {
    let mut edited_files = std::collections::HashSet::new();
    let (mut added, mut removed, mut commands, mut reads) = (0usize, 0usize, 0usize, 0usize);
    // Walk children AND one level of ActivityGroup nesting (tool steps live in
    // both containment forms today).
    let mut visit = |b: &AgentBlock| {
        if let AgentBlockKind::ToolStep {
            tool_id,
            name,
            diff,
            ..
        } = &b.kind
        {
            // Blocks store humanized display names ("Read File") — normalize
            // before matching or every count silently stays zero.
            let n = crate::terminal::render::agent_presentation::bare_tool(name);
            match n.as_str() {
                "read_file" | "read_multiple_files" => reads += 1,
                "run_command" => commands += 1,
                "edit_file" | "batch_edit" | "apply_patch" | "write_file" => {
                    // Persisted diff first — the in-memory registry is empty
                    // after a restart, and without this the receipt loses its
                    // +N −N counts on restored sessions.
                    if let Some(d) = diff
                        .clone()
                        .or_else(|| crate::agent::edit_diff::get(tool_id))
                    {
                        for f in &d.files {
                            edited_files.insert(f.path.clone());
                            added += f.added;
                            removed += f.removed;
                        }
                    } else {
                        edited_files.insert(tool_id.clone());
                    }
                }
                _ => {}
            }
        }
    };
    for c in children {
        visit(c);
        if let AgentBlockKind::ActivityGroup { blocks, .. } = &c.kind {
            for inner in blocks {
                visit(inner);
            }
        }
    }
    TurnStats {
        files_edited: edited_files.len(),
        added,
        removed,
        commands,
        reads,
    }
}

pub(crate) fn conversation_receipt(children: &[AgentBlock], duration_ms: i64) -> String {
    let stats = turn_stats(children);
    crate::terminal::render::agent_presentation::turn_receipt(
        stats.files_edited, stats.added, stats.removed, stats.commands, stats.reads, Some(duration_ms),
    )
}

pub(crate) fn build_turn_group(
    block_id: usize,
    children: &[AgentBlock],
    expanded: bool,
    started_at_ms: i64,
    duration_ms: Option<i64>,
    status_note: Option<&str>,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use std::time::Duration;
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};

    let is_streaming = duration_ms.is_none();
    let chevron = if expanded { "▾" } else { "▸" };
    let _ = started_at_ms;

    let header_text = if let Some(ms) = duration_ms {
        // Receipt-style summary from the turn's actual work, not just a clock:
        // "Edited 2 files · +49 −12 · ran 3 commands · 12s". Interrupted or
        // failed turns lead with their end state — never a stale "Working…".
        let stats = turn_stats(children);
        let receipt = crate::terminal::render::agent_presentation::turn_receipt(
            stats.files_edited,
            stats.added,
            stats.removed,
            stats.commands,
            stats.reads,
            Some(ms),
        );
        match status_note {
            Some(note) => format!("{chevron} {note} · {receipt}"),
            None => format!("{chevron} {receipt}"),
        }
    } else {
        format!("{chevron} Working…")
    };

    let shimmer_handle = if is_streaming {
        Some(ctx.shimmer_state(&format!("turn-group-{block_id}")))
    } else {
        None
    };
    let header_text_clone = header_text.clone();
    let header_color = if is_streaming {
        theme.accent
    } else if status_note == Some("Failed") {
        theme.ui_red()
    } else {
        theme.sub_text()
    };

    let mouse_state = ctx.mouse_state(&format!("turn-group-hdr-{block_id}"));
    let header_el = Hoverable::new(mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0);

        if let Some(ref handle) = shimmer_handle {
            let spinner = ShimmeringTextElement::new(
                "●".to_string(),
                mono,
                crate::theme::text_size(11.0),
                theme.accent,
                ColorU::white(),
                ShimmerConfig {
                    period: Duration::from_millis(800),
                    shimmer_radius: 2,
                    padding: 0,
                },
                handle.clone(),
            )
            .finish();
            row = row.with_child(spinner);
        }

        row = row.with_child(
            Text::new(
                header_text_clone.clone(),
                mono,
                crate::theme::text_size(13.5),
            )
            .with_color(header_color)
            .with_style(Properties {
                weight: Weight::Medium,
                ..Default::default()
            })
            .finish(),
        );

        Container::new(row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
    })
    .with_cursor(warpui::platform::Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::ToggleBlockExpansion { block_id });
    });

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0)
        .with_child(Box::new(header_el));

    if expanded {
        for child in children {
            let el: Box<dyn Element> = match &child.kind {
                AgentBlockKind::ThinkingStep {
                    content,
                    complete,
                    expanded,
                } => build_thinking_step(child.id, content, *complete, *expanded, ctx, theme, mono),
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
                    child.id,
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
                // The agent's narration BETWEEN tool calls ("Let me check how
                // billing is wired…", "That confirms X, so…"). The controller
                // segments these into per-round AgentMessage children; dropping
                // them here made turns read as bare tool lists — the narrative
                // is the part that makes the work legible.
                AgentBlockKind::AgentMessage { text, is_streaming } => {
                    if text.trim().is_empty() {
                        Empty::new().finish() as Box<dyn Element>
                    } else {
                        Container::new(render_markdown_agent(
                            text,
                            child.id,
                            ctx,
                            theme,
                            mono,
                            !is_streaming,
                        ))
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .finish()
                    }
                }
                AgentBlockKind::ActivityGroup { blocks, expanded } => {
                    build_activity_group(child.id, blocks, *expanded, ctx, theme, mono)
                }
                // Live task list belongs to the turn's continuous response —
                // rendered inline where it happened, not as a separate card
                // pinned at the bottom.
                AgentBlockKind::TodoList { todos } => build_todo_list(todos, theme, mono),
                AgentBlockKind::Loading { message } => {
                    let fake_block = AgentBlock::new(
                        child.id,
                        child.conversation_id,
                        AgentBlockKind::Loading {
                            message: message.clone(),
                        },
                    );
                    build_loading(message, &fake_block, ctx, theme, mono)
                }
                _ => Empty::new().finish() as Box<dyn Element>,
            };
            col = col.with_child(Container::new(el).with_padding_left(16.0).finish());
        }
    }

    Container::new(col.finish())
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}
