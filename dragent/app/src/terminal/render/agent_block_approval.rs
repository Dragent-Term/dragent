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

use super::agent_block_chrome::status_badge;
use super::agent_block_steps::dot;
use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionStatus, TodoListItem, ToolStepStatus,
};
use crate::terminal::render::agent_block_element::agent_block_header;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

pub(crate) fn build_approval_request(
    action_id: Uuid,
    command: &str,
    reason: &str,
    status: &ApprovalStatus,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.accent;
    let header = agent_block_header("Agent · Action Required", accent, theme, mono);

    let reason_el = Text::new(reason.to_string(), mono, crate::theme::text_size(13.5))
        .with_color(theme.disabled_text())
        .finish();

    // Copy command button
    let copy_btn_inner = crate::icons::render_icon(
        crate::icons::AppIcon::Copy,
        theme.sub_text(),
        crate::icons::ICON_SIZE_SM,
    );
    let copy_key = format!("copy-cmd-{}", block_id);
    let copied = ctx.is_copied(&copy_key);
    let copy_mouse_state = ctx.mouse_state(&copy_key);
    let copy_btn = Hoverable::new(copy_mouse_state, move |state| {
        let color = if state.is_hovered() {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let (icon, color) =
            crate::terminal::view::copy_confirmation::copy_glyph(copied, color, theme);
        crate::icons::render_icon(icon, color, crate::icons::ICON_SIZE_SM)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click({
        let command = command.to_string();
        move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::CopyWithConfirmation {
                key: copy_key.clone(),
                text: command.clone(),
            });
        }
    });

    let file_change = command.starts_with("write ")
        || command.starts_with("edit ")
        || command.starts_with("batch edit")
        || command.starts_with("patch ")
        || command.starts_with("apply_patch");
    let cmd_text = if file_change {
        Text::new(command.to_string(), mono, crate::theme::text_size(13.5))
            .with_color(theme.main_text())
    } else {
        crate::terminal::render::agent_presentation::command_text(
            "$ ",
            command,
            mono,
            crate::theme::text_size(13.5),
            &theme,
        )
    };

    let cmd_el = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Box::new(Expanded::new(
                1.0,
                cmd_text
                    .with_style(Properties {
                        weight: Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            )))
            .with_child(Box::new(copy_btn))
            .finish(),
    )
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_background_color(theme.surface_3())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish();

    let body_inner: Box<dyn Element> = match status {
        ApprovalStatus::Pending => {
            let approve_label = if reason.starts_with("Agent wants to read outside") {
                "Allow once"
            } else {
                "Approve"
            };
            let run_btn =
                approval_button(approve_label, true, action_id, block_id, ctx, theme, mono);
            let always_btn = allow_always_button(action_id, block_id, ctx, theme, mono);

            let reject_btn = {
                let mouse_state = ctx.mouse_state(&format!("agent-{block_id}-Reject"));
                let feedback_editor = ctx.feedback_editor.cloned();

                Box::new(
                    Hoverable::new(mouse_state, move |state| {
                        let bg = if state.is_hovered() {
                            theme.surface_2()
                        } else {
                            theme.surface_3()
                        };
                        Container::new(
                            Text::new("Reject".to_string(), mono, crate::theme::text_size(13.5))
                                .with_color(theme.main_text())
                                .with_style(Properties {
                                    weight: Weight::Medium,
                                    ..Default::default()
                                })
                                .finish(),
                        )
                        .with_padding_left(10.0)
                        .with_padding_right(10.0)
                        .with_padding_top(4.0)
                        .with_padding_bottom(4.0)
                        .with_background_color(bg)
                        .with_border(Border::all(1.0).with_border_color(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                        .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |event_ctx, app, _pos| {
                        let feedback = feedback_editor
                            .as_ref()
                            .map(|fe| fe.read(app, |editor, _| editor.get_text().to_string()));
                        let feedback_opt = feedback.filter(|f| !f.trim().is_empty());
                        event_ctx.dispatch_typed_action(TerminalAction::RejectAgentAction {
                            action_id,
                            feedback: feedback_opt,
                        });
                    }),
                ) as Box<dyn Element>
            };

            let mut col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(10.0)
                .with_child(reason_el)
                .with_child(cmd_el);

            if let Some(fe) = ctx.feedback_editor {
                let fe_child = ChildView::new(fe).finish();
                let fe_box = Container::new(fe_child)
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .with_background_color(theme.surface_2())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .finish();
                col = col.with_child(fe_box);
            }

            let buttons = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(run_btn)
                .with_child(always_btn)
                .with_child(reject_btn)
                .finish();

            col.with_child(buttons).finish()
        }
        ApprovalStatus::Approved | ApprovalStatus::ApprovedAlways => {
            let label = if matches!(status, ApprovalStatus::ApprovedAlways) {
                "Always allowed"
            } else {
                "Approved"
            };
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(10.0)
                .with_child(cmd_el)
                .with_child(status_badge(label, theme.ui_green(), theme, mono))
                .finish()
        }
        ApprovalStatus::Rejected => Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(10.0)
            .with_child(cmd_el)
            .with_child(status_badge("Rejected", theme.ui_red(), theme, mono))
            .finish(),
    };

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(
                Container::new(body_inner)
                    .with_padding_left(16.0)
                    .with_padding_right(13.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(12.0)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_user_question(
    action_id: Uuid,
    questions: &[QuestionItem],
    current: usize,
    answers: &[Vec<String>],
    status: &QuestionStatus,
    cursor: usize,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.accent;
    let header = agent_block_header("Agent · Question", accent, theme, mono);

    let body_inner: Box<dyn Element> = match status {
        QuestionStatus::Answered | QuestionStatus::Dismissed => {
            question_summary(questions, answers, status, theme, mono)
        }
        QuestionStatus::Pending => match questions.get(current) {
            Some(q) => build_pending_question(
                action_id,
                q,
                current,
                questions.len(),
                answers.get(current).map(|v| v.as_slice()).unwrap_or(&[]),
                cursor,
                block_id,
                ctx,
                theme,
                mono,
            ),
            None => Empty::new().finish(),
        },
    };

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header)
            .with_child(
                Container::new(body_inner)
                    .with_padding_left(16.0)
                    .with_padding_right(13.0)
                    .with_padding_top(10.0)
                    .with_padding_bottom(12.0)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

/// The live, answerable question: progress, header chip, question, options.
#[allow(clippy::too_many_arguments)]
fn build_pending_question(
    action_id: Uuid,
    q: &QuestionItem,
    current: usize,
    total: usize,
    selected: &[String],
    cursor: usize,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0);

    // Progress ("Question 2 of 3" + dots) — only when there is more than one.
    if total > 1 {
        let mut dots = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(
                Text::new(
                    format!("Question {} of {}", current + 1, total),
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        for i in 0..total {
            let color = if i < current {
                theme.ui_green()
            } else if i == current {
                theme.accent
            } else {
                theme.outline()
            };
            dots = dots.with_child(dot(color, 6.0));
        }
        col = col.with_child(dots.finish());
    }

    // Header chip. Wrapped in a start-aligned row so the pill hugs its text
    // instead of stretching to the column's full width.
    if !q.header.is_empty() {
        col = col.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Text::new(
                            q.header.to_uppercase(),
                            mono,
                            crate::theme::text_size(10.0),
                        )
                        .with_color(theme.accent)
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
                    .with_background_color(ColorU::new(
                        theme.accent.r,
                        theme.accent.g,
                        theme.accent.b,
                        28,
                    ))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
                )
                .finish(),
        );
    }

    // Question text.
    col = col.with_child(
        Text::new(q.question.clone(), mono, crate::theme::text_size(14.5))
            .with_color(theme.main_text())
            .with_style(Properties {
                weight: Weight::Medium,
                ..Default::default()
            })
            .finish(),
    );

    // Options.
    let mut opts = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0);
    for (i, opt) in q.options.iter().enumerate() {
        let is_selected = selected.iter().any(|s| s == &opt.label);
        opts = opts.with_child(option_row(
            action_id,
            &opt.label,
            &opt.description,
            q.multi_select,
            is_selected,
            i == cursor,
            block_id,
            current,
            i,
            ctx,
            theme,
            mono,
        ));
    }
    // Custom (typed) answers that aren't one of the provided options render as
    // selected rows so they're visible and can be toggled off. Only multi-select
    // keeps them on-screen — single-select advances the moment one is entered.
    let opt_count = q.options.len();
    for (j, custom) in selected
        .iter()
        .filter(|s| !q.options.iter().any(|o| &o.label == *s))
        .enumerate()
    {
        opts = opts.with_child(option_row(
            action_id,
            custom,
            "Your answer",
            true,
            true,
            false,
            block_id,
            current,
            opt_count + j,
            ctx,
            theme,
            mono,
        ));
    }
    col = col.with_child(opts.finish());

    // "Other" free-text field — available unless the model disabled custom
    // answers for a strict choice. Enter or the send button submits it.
    if q.custom {
        if let Some(editor) = ctx.question_editor {
            let send = send_answer_button(action_id, block_id, current, ctx, theme, mono);
            let field = Container::new(ChildView::new(editor).finish())
                .with_padding_left(10.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish();
            col = col.with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(Box::new(Expanded::new(1.0, field)))
                    .with_child(send)
                    .finish(),
            );
        }
    }

    // Multi-select needs an explicit Continue; single-select advances on click.
    if q.multi_select {
        let is_last = current + 1 >= total;
        let enabled = !selected.is_empty();
        let label = if is_last { "Done" } else { "Continue" };
        col = col.with_child(continue_button(
            action_id, label, enabled, block_id, current, ctx, theme, mono,
        ));
    }

    // Keyboard hints (Alt-gated so typing, history, and the PTY are untouched).
    col = col.with_child(
        Text::new(
            "Alt+↑↓ navigate · Alt+1–9 pick · Alt+Enter select".to_string(),
            mono,
            crate::theme::text_size(11.0),
        )
        .with_color(theme.disabled_text())
        .finish(),
    );

    col.finish()
}

/// Small send button for the free-text answer field.
fn send_answer_button(
    action_id: Uuid,
    block_id: usize,
    current: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let ms = ctx.mouse_state(&format!("q-send-{block_id}-{current}"));
    Box::new(
        Hoverable::new(ms, move |state| {
            let hovered = state.is_hovered();
            let color = if hovered {
                theme.accent
            } else {
                theme.sub_text()
            };
            Container::new(
                Text::new("Send".to_string(), mono, crate::theme::text_size(12.5))
                    .with_color(color)
                    .with_style(Properties {
                        weight: Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_border(Border::all(1.0).with_border_color(if hovered {
                theme.accent
            } else {
                theme.outline()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::SubmitAgentQuestionText { action_id });
        }),
    )
}

/// One clickable option row. Single-select highlights on hover; multi-select
/// shows a checkbox reflecting `is_selected`.
#[allow(clippy::too_many_arguments)]
fn option_row(
    action_id: Uuid,
    label: &str,
    description: &str,
    multi: bool,
    is_selected: bool,
    is_cursor: bool,
    block_id: usize,
    current: usize,
    idx: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let ms = ctx.mouse_state(&format!("q-{block_id}-{current}-{idx}"));
    let label_render = label.to_string();
    let desc_render = description.to_string();
    let label_click = label.to_string();

    Box::new(
        Hoverable::new(ms, move |state| {
            let hovered = state.is_hovered();
            // Keyboard cursor shares the hover treatment so Alt+Up/Down
            // movement is visible without touching the mouse.
            let highlighted = hovered || is_cursor;
            let bg = if is_selected {
                ColorU::new(theme.accent.r, theme.accent.g, theme.accent.b, 30)
            } else if highlighted {
                theme.surface_3()
            } else {
                theme.surface_2()
            };
            let border = if is_selected || highlighted {
                theme.accent
            } else {
                theme.outline()
            };

            // Left indicator: checkbox (multi) or radio dot (single).
            let indicator: Box<dyn Element> = {
                let inner: Box<dyn Element> = if is_selected {
                    crate::icons::render_icon(
                        crate::icons::AppIcon::Check,
                        theme.accent_text(),
                        12.0,
                    )
                } else {
                    Empty::new().finish()
                };
                let radius = if multi { 3.0 } else { 7.0 };
                Container::new(
                    ConstrainedBox::new(inner)
                        .with_width(14.0)
                        .with_height(14.0)
                        .finish(),
                )
                .with_background_color(if is_selected {
                    theme.accent
                } else {
                    theme.surface_1()
                })
                .with_border(Border::all(1.0).with_border_color(if is_selected {
                    theme.accent
                } else {
                    theme.outline()
                }))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(radius)))
                .finish()
            };

            let mut text_col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(2.0)
                .with_child(
                    Text::new(label_render.clone(), mono, crate::theme::text_size(13.5))
                        .with_color(theme.main_text())
                        .with_style(Properties {
                            weight: Weight::Medium,
                            ..Default::default()
                        })
                        .finish(),
                );
            if !desc_render.is_empty() {
                text_col = text_col.with_child(
                    Text::new(desc_render.clone(), mono, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                );
            }

            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(10.0)
                    .with_child(indicator)
                    .with_child(Box::new(Expanded::new(1.0, text_col.finish())))
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::PickAgentQuestionOption {
                action_id,
                option: label_click.clone(),
            });
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn continue_button(
    action_id: Uuid,
    label: &str,
    enabled: bool,
    block_id: usize,
    current: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let ms = ctx.mouse_state(&format!("q-continue-{block_id}-{current}"));
    let label = label.to_string();
    Box::new(
        Hoverable::new(ms, move |state| {
            let bg = if !enabled {
                theme.surface_2()
            } else if state.is_hovered() {
                ColorU::new(
                    theme.accent.r.saturating_add(30),
                    theme.accent.g.saturating_add(30),
                    theme.accent.b.saturating_add(30),
                    theme.accent.a,
                )
            } else {
                theme.accent
            };
            let text_color = if enabled {
                ColorU::white()
            } else {
                theme.disabled_text()
            };
            // Hug content: wrap the button in a start-aligned row.
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(
                        Text::new(label.clone(), mono, crate::theme::text_size(13.0))
                            .with_color(text_color)
                            .with_style(Properties {
                                weight: Weight::Semibold,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(5.0)
                    .with_padding_bottom(5.0)
                    .with_background_color(bg)
                    .with_border(Border::all(1.0).with_border_color(if enabled {
                        theme.accent
                    } else {
                        theme.outline()
                    }))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .finish(),
                )
                .finish()
        })
        .with_cursor(if enabled {
            Cursor::PointingHand
        } else {
            Cursor::Arrow
        })
        .on_click(move |event_ctx, _app, _pos| {
            if enabled {
                event_ctx.dispatch_typed_action(TerminalAction::ConfirmAgentQuestion { action_id });
            }
        }),
    )
}

/// Compact recap shown once the questions are answered (or dismissed) so the
/// conversation history reads cleanly.
fn question_summary(
    questions: &[QuestionItem],
    answers: &[Vec<String>],
    status: &QuestionStatus,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let (badge_label, badge_color) = match status {
        QuestionStatus::Dismissed => ("Dismissed", theme.sub_text()),
        _ => ("Answered", theme.ui_green()),
    };

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(status_badge(badge_label, badge_color, theme, mono));

    for (i, q) in questions.iter().enumerate() {
        let picked = answers.get(i).cloned().unwrap_or_default();
        let answer_text = if picked.is_empty() {
            "—".to_string()
        } else {
            picked.join(", ")
        };
        col = col.with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(2.0)
                .with_child(
                    Text::new(q.question.clone(), mono, crate::theme::text_size(12.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(
                    Text::new(answer_text, mono, crate::theme::text_size(12.5))
                        .with_color(theme.main_text())
                        .with_style(Properties {
                            weight: Weight::Medium,
                            ..Default::default()
                        })
                        .finish(),
                )
                .finish(),
        );
    }
    col.finish()
}

fn approval_button(
    label: &str,
    is_run: bool,
    action_id: Uuid,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let action: TerminalAction = if is_run {
        TerminalAction::ApproveAgentAction { action_id }
    } else {
        TerminalAction::RejectAgentAction {
            action_id,
            feedback: None,
        }
    };

    let mouse_state = ctx.mouse_state(&format!("agent-{block_id}-{label}"));
    Box::new(
        Hoverable::new(mouse_state, move |state| {
            let base_bg = if is_run {
                if state.is_hovered() {
                    ColorU::new(
                        theme.accent.r.saturating_add(30),
                        theme.accent.g.saturating_add(30),
                        theme.accent.b.saturating_add(30),
                        theme.accent.a,
                    )
                } else {
                    theme.accent
                }
            } else {
                if state.is_hovered() {
                    theme.surface_2()
                } else {
                    theme.surface_3()
                }
            };
            let text_color = if is_run {
                ColorU::white()
            } else {
                theme.main_text()
            };
            let border_color = if is_run {
                theme.accent
            } else {
                theme.outline()
            };

            Container::new(
                Text::new(label.to_string(), mono, crate::theme::text_size(13.5))
                    .with_color(text_color)
                    .with_style(Properties {
                        weight: Weight::Medium,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(base_bg)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(action.clone());
        }),
    )
}

fn allow_always_button(
    action_id: Uuid,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let action = TerminalAction::AllowToolAlways { action_id };
    let mouse_state = ctx.mouse_state(&format!("agent-{block_id}-allow-always"));
    Box::new(
        Hoverable::new(mouse_state, move |state| {
            let bg = if state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_3()
            };
            Container::new(
                Text::new(
                    "Allow always".to_string(),
                    mono,
                    crate::theme::text_size(13.5),
                )
                .with_color(theme.main_text())
                .with_style(Properties {
                    weight: Weight::Medium,
                    ..Default::default()
                })
                .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(theme.accent))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(action.clone());
        }),
    )
}
