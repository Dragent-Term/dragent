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
use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionStatus, TodoListItem, ToolStepStatus,
};
use crate::terminal::render::agent_block_element::agent_block_card;
use crate::terminal::render::agent_block_element::agent_block_header;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

/// Live task list (`todowrite` tool), opencode `# Todos` styling: `[✓]` /
/// `[•]` / `[ ]` markers, warning color while in progress, muted otherwise.
/// Shared by the terminal conversation and coordinator-style agent surfaces.
/// Keeping this presentation in one place prevents task updates from drifting
/// into a workspace-specific sticky-card treatment.
pub(crate) fn build_todo_list(
    todos: &[TodoListItem],
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0);
    if todos.is_empty() {
        col = col.with_child(
            Text::new(
                "No todos yet.".to_string(),
                mono,
                crate::theme::text_size(12.5),
            )
            .with_color(theme.disabled_text())
            .finish(),
        );
    }
    for todo in todos {
        let in_progress = todo.status == "in_progress";
        let marker = if todo.status == "completed" {
            "✓"
        } else if in_progress {
            "•"
        } else {
            " "
        };
        let color = if in_progress {
            theme.ui_yellow()
        } else {
            theme.sub_text()
        };
        col = col.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.0)
                .with_child(
                    Text::new(format!("[{marker}]"), mono, crate::theme::text_size(12.5))
                        .with_color(color)
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(todo.content.clone(), mono, crate::theme::text_size(12.5))
                            .with_color(color)
                            .with_style(Properties {
                                weight: Weight::Medium,
                                ..Default::default()
                            })
                            .soft_wrap(true)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        );
    }
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(agent_block_header(
                "Agent · Todos",
                theme.accent,
                theme,
                mono,
            ))
            .with_child(
                Container::new(col.finish())
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

/// A small filled dot used for the question-progress indicator.
pub(crate) fn dot(color: ColorU, size: f32) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(size)
            .with_height(size)
            .finish(),
    )
    .with_background_color(color)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(size / 2.0)))
    .finish()
}

pub(crate) fn build_action_result(
    command: &str,
    exit_code: Option<i32>,
    output_preview: &str,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let success = exit_code.map_or(true, |c| c == 0);
    let accent = if success {
        theme.ui_green()
    } else {
        theme.ui_red()
    };
    let header = agent_block_header("Agent · Result", accent, theme, mono);

    let exit_text = match exit_code {
        Some(0) => "exit 0".to_string(),
        Some(c) => format!("exit {c}"),
        None => "done".to_string(),
    };

    // Command line: `$ ` prompt in the accent, the command in main text, and a
    // colored exit pill pushed to the right — reads like a real terminal block.
    let cmd_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            crate::terminal::render::agent_presentation::command_text(
                "$ ",
                command,
                mono,
                crate::theme::text_size(13.0),
                &theme,
            )
            .finish(),
        )
        .with_child(Box::new(Expanded::new(1.0, Empty::new().finish())))
        .with_child(status_badge(&exit_text, accent, theme, mono))
        .finish();

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(cmd_row);

    if !output_preview.is_empty() {
        // Output in its own recessed monospace card. Keep a generous bounded
        // result so one command cannot grow the UI without limit.
        const MAX_RESULT_LINES: usize = 10_000;
        let shown = output_preview
            .lines()
            .take(MAX_RESULT_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        let mut out_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(0.0)
            .with_child(
                Text::new(shown, mono, crate::theme::text_size(12.5))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        let out_card = Container::new(out_col.finish())
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish();
        col = col.with_child(out_card);
    }

    let body = Container::new(col.finish())
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(10.0)
        .finish();

    agent_block_card(header, Some(body), accent)
}

pub(crate) fn build_tool_step(
    block_id: usize,
    tool_id: &str,
    expanded: bool,
    name: &str,
    icon: &str,
    args_summary: &str,
    status: &ToolStepStatus,
    result_summary: Option<&str>,
    duration_ms: Option<i64>,
    persisted_diff: Option<&crate::agent::edit_diff::EditDiff>,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    // A tool result can acknowledge a background job before its process exits.
    // Keep the row live and cancellable until the actual process finishes.
    let live_run = crate::agent::local_tools::running_agent_commands().into_iter().find(|run| run.id == tool_id);
    let running_status = ToolStepStatus::Running;
    let status = if live_run.is_some() { &running_status } else { status };
    let (accent, status_suffix) = match status {
        ToolStepStatus::Running => (theme.accent, " …"),
        ToolStepStatus::Done => (theme.main_text(), ""),
        ToolStepStatus::Error => (theme.ui_red(), " ✗"),
    };

    let _ = (icon, strip_emoji_str);
    let chevron = if expanded { "▾" } else { "▸" };

    // Semantic header: monochrome glyph + human verb, e.g. "◇ Read" / "◆ Edited"
    // / "▷ Ran". No emoji, no snake_case tool name, no raw arg blob.
    let pres = crate::terminal::render::agent_presentation::tool_presentation(name, args_summary);
    let verb = if matches!(status, ToolStepStatus::Running) && pres.verb == "Ran" { "Running" } else { &pres.verb };
    let header_text = format!("{chevron} {verb}{status_suffix}");

    let header_el = Text::new(header_text, crate::fonts::ui_role(mono), crate::theme::text_size(12.5))
        .with_color(accent)
        .with_style(warpui::fonts::Properties {
            weight: warpui::fonts::Weight::Medium,
            ..Default::default()
        })
        .finish();

    let mut header_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(header_el);

    // The object (file basename, query, command) in the main text weight so it
    // reads as "Read manager.rs" rather than a dimmed afterthought.
    if !pres.object.trim().is_empty() {
        let mut obj = pres.object.trim().to_string();
        if obj.chars().count() > 52 {
            obj = obj.chars().take(52).collect::<String>() + "…";
        }
        let presentation = crate::terminal::render::agent_presentation::object_kind(name);
        header_row = header_row.with_child(Box::new(Expanded::new(1.0,
            crate::terminal::render::agent_presentation::object_text(
                presentation, &obj, mono, crate::theme::text_size(13.0), &theme,
            )
            .finish(),
        )));
    }

    // Outcome detail (match/line count) in muted text, when the tool completed.
    if matches!(status, ToolStepStatus::Done) {
        if let Some(rs) = result_summary {
            let detail = crate::terminal::render::agent_presentation::result_detail(name, rs);
            if !detail.is_empty() {
                header_row = header_row.with_child(
                    Text::new(detail, mono, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                );
            }
        }
    }

    // Push the duration to the far right on completed rows.
    if let Some(ms) = duration_ms {
        let dur = if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        };
        header_row = header_row.with_child(
            Text::new(dur, mono, crate::theme::text_size(11.5))
                .with_color(theme.sub_text())
                .finish(),
        );
    }

    let mouse_state = ctx.mouse_state(&format!("tool-step-{}", block_id));
    let header_interactive = Hoverable::new(mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        };
        Container::new(header_row.finish())
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::ToggleToolStep { block_id });
    });

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(Box::new(header_interactive));

    // Live process row: while a run_command is executing, show a ticking
    // spinner, elapsed time, the newest output line, and a Cancel button —
    // the user must never wonder whether something is still running.
    if matches!(status, ToolStepStatus::Running) {
        if let Some(run) = live_run {
            // Fira Code has no braille (U+28xx), and the web build embeds ONLY Fira
            // Code with no fallback provider — the braille spinner rendered as a
            // row of tofu boxes there. These circle frames are in the font.
            const FRAMES: [&str; 4] = ["◐", "◓", "◑", "◒"];
            let frame = FRAMES[(run.elapsed_secs as usize) % FRAMES.len()];
            let where_label = if run.backgrounded {
                " · background"
            } else {
                ""
            };
            let mut live_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    Text::new(
                        format!("{frame} running — {}s{where_label}", run.elapsed_secs),
                        mono,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.accent)
                    .finish(),
                );
            // Deliberately NO output/last-line here: output lines carry file
            // paths and arbitrary text that distort the row (a long path once
            // rendered as a 1-char-wide column). This row only says "a command
            // is running"; the output lives in the block body.
            // Spacer pins the Cancel button to the far right edge of the row.
            live_row = live_row.with_child(Box::new(Expanded::new(1.0, Empty::new().finish())));
            let cancel_id = run.id.clone();
            let cancel_state = ctx.mouse_state(&format!("tool-cancel-{}", run.id));
            let cancel_btn: Box<dyn Element> = Box::new(
                Hoverable::new(cancel_state, move |state| {
                    let (color, border) = if state.is_hovered() {
                        (theme.ui_red(), theme.ui_red())
                    } else {
                        (theme.sub_text(), theme.outline())
                    };
                    {
                        let icon = Container::new(
                            warpui::elements::Align::new(Text::new("Stop", mono, crate::theme::text_size(11.0)).with_color(color).finish())
                            .finish(),
                        )
                        .with_border(Border::all(1.0).with_border_color(border))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                        .finish();
                        warpui::elements::ConstrainedBox::new(icon)
                            .with_width(48.0)
                            .with_height(24.0)
                            .finish()
                    }
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::CancelAgentCommand {
                        task_id: cancel_id.clone(),
                    });
                }),
            );
            live_row = live_row.with_child(cancel_btn);
            col = col.with_child(
                Container::new(live_row.finish())
                    .with_padding_left(12.0)
                    .finish(),
            );
        }
    }

    // Diff card: if this tool captured an edit, ALWAYS show the ±hunks (edits
    // are the thing users most want to see) — not gated behind expand, never
    // auto-collapsed. Persisted diff (survives restarts) first; the in-memory
    // registry covers the live moment before ToolEnd attaches it to the block.
    let diff = persisted_diff
        .cloned()
        .or_else(|| crate::agent::edit_diff::get(tool_id));
    if let Some(ref d) = diff {
        let expanded = crate::agent::edit_diff::is_expanded(tool_id);
        col = col.with_child(
            Container::new(crate::terminal::render::agent_presentation::diff_card(
                d, expanded, theme, mono,
            ))
            .with_padding_left(12.0)
            .finish(),
        );
        // Clickable "+N more lines / Show less" toggle when the diff is clipped.
        let hidden = crate::terminal::render::agent_presentation::diff_hidden_lines(d);
        if hidden > 0 {
            let label = if expanded {
                "▾  Show less".to_string()
            } else {
                format!(
                    "▸  +{hidden} more line{} — show all",
                    if hidden == 1 { "" } else { "s" }
                )
            };
            let toggle_state = ctx.mouse_state(&format!("diff-expand-{tool_id}"));
            let tool_id_owned = tool_id.to_string();
            let toggle: Box<dyn Element> = Box::new(
                Hoverable::new(toggle_state, move |state| {
                    let color = if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    };
                    Container::new(
                        Text::new(label.clone(), mono, crate::theme::text_size(11.5))
                            .with_color(color)
                            .finish(),
                    )
                    .with_padding_left(16.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(2.0)
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleDiffExpand {
                        tool_id: tool_id_owned.clone(),
                    });
                }),
            );
            col = col.with_child(toggle);
        }
    }

    // Commands stay in an independently scrollable log panel. A long build
    // cannot resize the whole conversation or move its composer off screen.
    let live_output = if matches!(status, ToolStepStatus::Running) {
        crate::agent::local_tools::agent_command_output(tool_id)
    } else { None };
    if (expanded || live_output.is_some()) && diff.is_none() {
        if expanded && (args_summary.chars().count() > 52 || args_summary.contains('\n')) {
            col = col.with_child(Text::new(args_summary.to_string(), mono, crate::theme::text_size(12.0)).with_color(theme.sub_text()).finish());
        }
        let result = live_output.as_deref().or(result_summary).unwrap_or_default();
        if !result.is_empty() {
            col = col.with_child(super::agent_transcript::output_panel(tool_id, result, ctx, theme, mono));
        }
    }

    Container::new(col.finish())
        .with_padding_left(8.0)
        .with_padding_top(1.0)
        .with_padding_bottom(1.0)
        .finish()
}

/// Extract a compact single-line preview from raw thinking text.
/// Takes the first non-empty line, strips leading markdown markers, and
/// truncates to 72 chars with an ellipsis if needed.
fn thinking_heading(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("…");
    let stripped = first.trim_start_matches(|c: char| matches!(c, '#' | '*' | '-' | '>' | ' '));
    let collapsed: String = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "…".to_string();
    }
    const MAX: usize = 72;
    if collapsed.chars().count() > MAX {
        let head: String = collapsed.chars().take(MAX).collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

pub(crate) fn build_thinking_step(
    block_id: usize,
    content: &str,
    complete: bool,
    expanded: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use std::time::Duration;
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};

    let header_color = if complete {
        theme.disabled_text()
    } else {
        theme.accent
    };
    let chevron = if expanded { "▾" } else { "▸" };
    let heading_text = thinking_heading(content);

    // Pre-build pieces that are Clone/Copy so they can be captured by the Hoverable closure,
    // which re-renders on each hover state change (Flex is not Clone so can't be built outside).
    let spinner_handle = if !complete {
        Some(ctx.shimmer_state(&format!("thinking-spinner-{}", block_id)))
    } else {
        None
    };
    let status_label = if complete {
        format!("{chevron} ✳ Thought")
    } else {
        format!("{chevron} ✳ Thinking")
    };
    let heading_preview = if !expanded && !heading_text.is_empty() && heading_text != "…" {
        Some(heading_text.clone())
    } else {
        None
    };

    let mouse_state = ctx.mouse_state(&format!("thinking-toggle-{}", block_id));
    let header = Hoverable::new(mouse_state, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            theme.background
        };

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0);

        if let Some(ref handle) = spinner_handle {
            let spinner_el = ShimmeringTextElement::new(
                "●".to_string(),
                mono,
                crate::theme::text_size(12.0),
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
            Text::new(status_label.clone(), mono, crate::theme::text_size(13.0))
                .with_color(header_color)
                .with_style(warpui::fonts::Properties {
                    weight: warpui::fonts::Weight::Medium,
                    ..Default::default()
                })
                .finish(),
        );

        if let Some(ref preview) = heading_preview {
            row = row.with_child(
                Text::new(preview.clone(), mono, crate::theme::text_size(12.5))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        }

        Container::new(row.finish())
            .with_background_color(bg)
            .with_padding_left(14.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish()
    })
    .with_cursor(warpui::platform::Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(
            crate::terminal::view::actions::TerminalAction::ToggleAgentThinking { block_id },
        );
    });

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(Box::new(header));

    if expanded {
        // Reasoning reads as a quiet aside: recessed panel with a thin accent
        // rail on the left (blockquote feel), clearly distinct from the
        // agent's actual replies.
        let inner = Container::new(render_markdown_agent(
            content, block_id, ctx, theme, mono, true,
        ))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_background_color(theme.surface_1())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
        let rail = Container::new(inner)
            .with_border(
                warpui_core::scene::Border::left(2.0).with_border_color(theme.accent_overlay()),
            )
            .finish();
        let content_el = Container::new(rail)
            .with_padding_left(26.0)
            .with_padding_top(2.0)
            .with_padding_bottom(4.0)
            .finish();
        col = col.with_child(content_el);
    }

    Container::new(col.finish())

        .finish()
}

pub(crate) fn build_loading(
    message: &str,
    block: &crate::agent::controller::block::AgentBlock,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use std::time::Duration;
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};

    let shimmer_handle = ctx.shimmer_state(&format!("loading-{}", block.id));
    let shimmer_el = ShimmeringTextElement::new(
        message.to_string(),
        mono,
        crate::theme::text_size(13.0),
        theme.sub_text(),
        theme.accent,
        ShimmerConfig {
            period: Duration::from_millis(1500),
            shimmer_radius: 4,
            padding: 6,
        },
        shimmer_handle,
    )
    .finish();

    Container::new(shimmer_el)
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}

pub(crate) fn build_error_card(
    message: &str,
    kind: &ErrorKind,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.ui_red();

    let label = match kind {
        ErrorKind::Auth => "Agent · Sign in Required",
        ErrorKind::InsufficientCredits => "Agent · Credits Exhausted",
        ErrorKind::Network => "Agent · Network Error",
        ErrorKind::Busy => "Agent · Busy",
        ErrorKind::Internal => "Agent · Error",
    };

    let header = agent_block_header(label, accent, theme, mono);

    let body_text = render_markdown(message, theme, mono);

    // Show a "Continue" button for retryable errors (network loss, server busy,
    // internal errors). Auth and credits errors need user action outside the
    // agent, so no automatic retry makes sense there.
    let can_retry = matches!(
        kind,
        ErrorKind::Network | ErrorKind::Busy | ErrorKind::Internal
    );

    let body: Box<dyn Element> = if can_retry {
        let mouse_state = ctx.mouse_state(&format!("error-continue-{block_id}"));
        let btn_label = Text::new(
            "Continue".to_string(),
            mono,
            crate::theme::text_size(13.0),
        )
        .with_color(accent)
        .with_style(Properties {
            weight: Weight::Medium,
            ..Default::default()
        })
        .finish();
        let btn_inner = Container::new(btn_label)
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(theme.surface_3())
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(accent))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(5.0),
            ))
            .finish();
        let btn = Box::new(
            Hoverable::new(mouse_state, move |_| btn_inner)
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::ContinueAgent {
                        error_block_id: block_id,
                    });
                }),
        ) as Box<dyn Element>;

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(10.0)
            .with_child(body_text)
            .with_child(btn)
            .finish() as Box<dyn Element>
    } else {
        body_text
    };

    let body_container = Container::new(body)
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(6.0)
        .with_padding_bottom(12.0)
        .finish();

    agent_block_card(header, Some(body_container), accent)
}
