//! Execution workspace task views: map, cards, threads, evidence.
//!
//! Moved verbatim; same behavior.

//! Coordinate workspace projected exclusively from canonical workflow state.
//!
//! No sample agents, tasks, test counts, artifacts, or timestamps belong in
//! this view. An empty canonical projection renders an honest onboarding state.

use super::exec_view_attention::*;
use super::exec_view_cards::*;
use super::exec_view_main::*;
use super::execution_workspace::{
    ACTIVITY_HEIGHT, CONTENT_BOTTOM_PADDING, EXECUTION_CARD_HORIZONTAL_PADDING,
    EXECUTION_CARD_VERTICAL_PADDING, EXECUTION_MAP_GUTTER, MAP_HEIGHT, MINIMUM_MAIN_PANE_WIDTH,
    MINIMUM_SCROLLING_CONTENT_HEIGHT, NARROW_CONTENT_WIDTH, NAV_WIDTH, TASK_CARD_MAX_COLUMNS,
    TASK_CARD_TARGET_WIDTH,
};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DragBarSide, Element, Empty, Expanded, Flex, MainAxisSize,
    ParentElement, Resizable, ResizableStateHandle, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{EntityId, ViewHandle};

use super::InputEditor;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::agent::workspace_execution::WorkspaceCheckpointSummary;
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::WorkspaceAction;
use dragent_workspace_model::{
    AgentMessage, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionKind,
    AttentionStatus, Evidence, EvidenceKind, ExecutionAttempt, ExecutionAttemptId,
    ExecutionAttemptStatus, ExecutionReviewResolution, Plan, PlanTask, PlanTaskId, PlanTaskStatus,
    WorkStatus,
};

use super::shape_workspace::ProposedRoleView;

/// Shared center + evidence-rail area for both Coordinate arrangements.
/// Below `NARROW_CONTENT_WIDTH` the layout reflows (wrapped task cards,
/// stacked detail columns, full-width rail) instead of squeezing content
/// into overlap; the finite canvas scroll remains only as a backstop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn primary_area(
    viewport_width: f32,
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    attempts: &[ExecutionAttempt],
    messages: &[(AgentMessage, AgentMessageRouting)],
    evidence: &[Evidence],
    attention: &[AttentionItem],
    checkpoints: &[WorkspaceCheckpointSummary],
    checkpoint_notice: Option<&str>,
    execution_notice: Option<&str>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let content_width = (viewport_width - NAV_WIDTH).max(1.0);
    let narrow = coordinate_is_narrow(viewport_width);
    let columns = ((content_width - EXECUTION_MAP_GUTTER * 2.0) / TASK_CARD_TARGET_WIDTH)
        .floor()
        .max(1.0) as usize;
    let columns = columns.min(TASK_CARD_MAX_COLUMNS);
    let map = organization_map(
        plan,
        roles,
        attempts,
        columns,
        workspace_view_id,
        theme,
        font,
    );
    // Narrow content lives in a vertical scroll (unbounded height): every
    // vertical flex must size to its content (Min, no Expanded). Wide content
    // keeps the previous stretched arrangement.
    // Rows size themselves; a fixed map height would clip multi-row maps.
    let map = ConstrainedBox::new(map)
        .with_min_height(MAP_HEIGHT)
        .finish();
    let center: Box<dyn Element> = if narrow {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(map)
            .with_child(selected_work(
                plan, roles, attempts, messages, narrow, theme, font,
            ))
            .finish()
    } else {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(map)
            .with_child(
                Expanded::new(
                    1.0,
                    selected_work(plan, roles, attempts, messages, narrow, theme, font),
                )
                .finish(),
            )
            .finish()
    };
    let rail = evidence_rail(
        attempts,
        evidence,
        attention,
        checkpoints,
        checkpoint_notice,
        theme,
        font,
        workspace_view_id,
    );
    // The error banner lives above the map/rail split — never inside the
    // rail — so a coordination failure stays visible even when narrow widths
    // stack the rail below the content.
    let mut top = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    if let Some(notice) = execution_notice {
        top = top.with_child(
            Container::new(notice_card(
                notice,
                theme.ui_red(),
                theme,
                font,
                Some((WorkspaceAction::DismissCoordinationError, workspace_view_id)),
            ))
            .with_padding_top(10.0)
            .with_padding_left(EXECUTION_MAP_GUTTER)
            .with_padding_right(EXECUTION_MAP_GUTTER)
            .with_padding_bottom(12.0)
            .finish(),
        );
    }
    if narrow {
        top.with_child(center).with_child(rail).finish()
    } else {
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Expanded::new(1.0, center).finish())
            .with_child(Expanded::new(0.34, rail).finish())
            .finish();
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(top.finish())
                .with_child(Expanded::new(1.0, row).finish())
                .finish(),
        )
        .finish()
    }
}

pub(crate) fn organization_map(
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    attempts: &[ExecutionAttempt],
    columns: usize,
    view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    // Content-sized (Min): the caller pins the visible height. A Max column
    // here panics when an ancestor measures with an unbounded height, as
    // startup sizing passes do.
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(2.0)
                    .with_child(label(
                        "Organization execution map",
                        theme.main_text(),
                        12.5,
                        font,
                    ))
                    .with_child(label(
                        "Canonical tasks, assigned roles, and current execution state",
                        theme.disabled_text(),
                        10.5,
                        font,
                    ))
                    .finish(),
            )
            .with_padding_left(EXECUTION_MAP_GUTTER)
            .with_padding_right(EXECUTION_MAP_GUTTER)
            .with_padding_top(10.0)
            .with_padding_bottom(9.0)
            .finish(),
        );
    if let Some(plan) = plan {
        // Flow cards into rows sized from the measured content width. Every
        // card gets tight constraints from its row, so text wraps at word
        // boundaries instead of overflowing (unbounded) or squeezing into
        // character breaks (over-packed).
        let columns = columns.max(1);
        let mut rows = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0);
        for chunk in plan.tasks.chunks(columns) {
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0);
            for task in chunk {
                let role = role_for_task(task, roles)
                    .map(|role| role.name.as_str())
                    .unwrap_or("Unassigned");
                let selected = attempts.iter().any(|attempt| {
                    attempt.task_id == task.id && attempt.status == ExecutionAttemptStatus::Running
                });
                let (progress, color) = task_style(task.status, theme);
                let open_attempt = latest_attempt_id(attempts, task.id);
                let live = if task.status == PlanTaskStatus::Running {
                    live_attempt_state(open_attempt)
                } else {
                    None
                };
                row = row.with_child(
                    Expanded::new(
                        1.0,
                        task_node(
                            role,
                            &task.title,
                            progress,
                            live.as_deref().unwrap_or(task_status_label(task.status)),
                            color,
                            selected,
                            task.id,
                            open_attempt,
                            view_id,
                            theme,
                            font,
                        ),
                    )
                    .finish(),
                );
            }
            rows = rows.with_child(row.finish());
        }
        content = content.with_child(
            Container::new(rows.finish())
                .with_padding_left(EXECUTION_MAP_GUTTER)
                .with_padding_right(EXECUTION_MAP_GUTTER)
                .with_padding_top(9.0)
                .with_padding_bottom(14.0)
                .finish(),
        );
    } else {
        content = content.with_child(
            Container::new(empty_card(
                "No approved plan",
                "Return to Shape, describe the outcome, then approve the Principal's plan.",
                theme,
                font,
            ))
            .with_padding_left(EXECUTION_MAP_GUTTER)
            .with_padding_right(EXECUTION_MAP_GUTTER)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        );
    }
    Container::new(content.finish())
        .with_background_color(theme.sidebar_background())
        .with_border(Border::right(1.0).with_border_color(theme.border()))
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
}

/// Latest canonical attempt for a task, if any. Drives both the inspectable
/// agent tab target and card clickability — never synthesized.
pub(crate) fn latest_attempt_id(
    attempts: &[ExecutionAttempt],
    task_id: PlanTaskId,
) -> Option<ExecutionAttemptId> {
    attempts
        .iter()
        .filter(|attempt| attempt.task_id == task_id)
        .max_by_key(|attempt| attempt.attempt_number)
        .map(|attempt| attempt.id)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn task_node(
    role: &str,
    task: &str,
    progress: f32,
    state: &str,
    color: ColorU,
    selected: bool,
    task_id: PlanTaskId,
    attempt_id: Option<ExecutionAttemptId>,
    view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    // Every card opens the inspectable runtime tab, including tasks with no
    // attempts yet — the tab then shows the assigned task and its waiting
    // state instead of history. Read-only navigation: the tab renders
    // durable state and never alters execution. The body rebuilds per hover
    // frame from owned values (elements are single-use).
    let role_owned = role.to_string();
    let task_owned = task.to_string();
    let state_owned = state.to_string();
    let theme_owned = *theme;
    let mouse = stable_mouse(format!("coordinate:task:{:?}:{:?}", task_id, attempt_id));
    Hoverable::new(mouse, move |hover| {
        let card = task_card_body(
            &role_owned,
            &task_owned,
            progress,
            &state_owned,
            color,
            selected,
            &theme_owned,
            font,
        );
        if hover.is_hovered() {
            Container::new(card)
                .with_background_color(theme_owned.accent_overlay())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish()
        } else {
            card
        }
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            view_id,
            WorkspaceAction::OpenWorkspaceAgentTimeline {
                task_id: Some(task_id),
                attempt_id,
            },
        )
    })
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn task_card_body(
    role: &str,
    task: &str,
    progress: f32,
    state: &str,
    color: ColorU,
    selected: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(7.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(
                        if progress >= 1.0 {
                            AppIcon::Check
                        } else {
                            AppIcon::Agent
                        },
                        color,
                        12.0,
                    ))
                    .with_child(label(role, theme.main_text(), 10.5, font))
                    .finish(),
            )
            .with_child(label(task, theme.sub_text(), 9.8, font))
            .with_child(label(state, color, 9.8, font))
            .with_child(progress_bar(84.0, progress, color, theme))
            .finish(),
    )
    .with_padding_left(EXECUTION_CARD_HORIZONTAL_PADDING)
    .with_padding_right(EXECUTION_CARD_HORIZONTAL_PADDING)
    .with_padding_top(EXECUTION_CARD_VERTICAL_PADDING)
    .with_padding_bottom(EXECUTION_CARD_VERTICAL_PADDING)
    .with_background_color(if selected {
        theme.link_surface()
    } else {
        theme.surface_1()
    })
    .with_border(Border::all(1.0).with_border_color(if selected { color } else { theme.outline() }))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish()
}

pub(crate) fn selected_work(
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    attempts: &[ExecutionAttempt],
    messages: &[(AgentMessage, AgentMessageRouting)],
    narrow: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let task = plan.and_then(|plan| {
        plan.tasks
            .iter()
            .find(|task| {
                matches!(
                    task.status,
                    PlanTaskStatus::Running
                        | PlanTaskStatus::Blocked
                        | PlanTaskStatus::Reviewing
                        | PlanTaskStatus::Verifying
                )
            })
            .or_else(|| {
                plan.tasks
                    .iter()
                    .find(|task| task.status != PlanTaskStatus::Completed)
            })
            .or_else(|| plan.tasks.last())
    });
    let Some(task) = task else {
        return empty_card(
            "Nothing is executing yet",
            "Approve a shaped plan to create tasks, role contracts, and isolated attempts.",
            theme,
            font,
        );
    };
    let role = role_for_task(task, roles);
    let attempt = attempts
        .iter()
        .rev()
        .find(|attempt| attempt.task_id == task.id);
    let contract = role.map_or_else(
        || {
            empty_card(
                "Unassigned task",
                "The Principal has not assigned a role contract.",
                theme,
                font,
            )
        },
        |role| {
            let mut details = vec![
                ("Mission", role.mission.clone()),
                (
                    "Model",
                    role.preferred_model_route
                        .clone()
                        .unwrap_or_else(|| "Automatic routing".into()),
                ),
                ("Location", format!("{:?}", role.execution_location)),
                ("Status", format!("{:?}", role.status)),
            ];
            if !role.write_scope.is_empty() {
                details.push(("Write scope", role.write_scope.join(", ")));
            }
            detail_column("Work contract", &details, theme, font)
        },
    );
    let role_name = role.map(|role| role.name.as_str()).unwrap_or("Unassigned");
    let header = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(9.0)
            .with_child(render_icon(AppIcon::Agent, theme.link(), 15.0))
            .with_child(label(role_name, theme.main_text(), 13.0, font))
            .with_child(divider(theme))
            .with_child(label(
                task_status_label(task.status),
                task_style(task.status, theme).1,
                11.0,
                font,
            ))
            .finish(),
    )
    .with_padding_left(13.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
    .finish();
    // Narrow content scrolls vertically (unbounded height): size to content
    // with no Expanded. Wide content stretches into its bounded pane.
    let sections: Box<dyn Element> = if narrow {
        // Stacked sections read naturally on small widths; the row layout
        // would squeeze each column to overlap.
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(contract)
            .with_child(current_task(task, attempt, theme, font))
            .with_child(agent_thread(messages, task, theme, font))
            .finish()
    } else {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Expanded::new(0.86, contract).finish())
            .with_child(Expanded::new(1.10, current_task(task, attempt, theme, font)).finish())
            .with_child(Expanded::new(1.0, agent_thread(messages, task, theme, font)).finish())
            .finish()
    };
    let card = Container::new(if narrow {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(header)
            .with_child(sections)
            .finish()
    } else {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(Expanded::new(1.0, sections).finish())
            .finish()
    })
    .with_border(Border::right(1.0).with_border_color(theme.border()))
    .finish();
    // Margin outside the border so the whole bordered card — not just its
    // text — sits inset like the organization execution map content.
    Container::new(card)
        .with_padding_left(EXECUTION_MAP_GUTTER)
        .with_padding_right(EXECUTION_MAP_GUTTER)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish()
}

pub(crate) fn detail_column(
    title: &str,
    rows: &[(&str, String)],
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label(title, theme.main_text(), 11.5, font));
    for (name, value) in rows {
        content = content.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_child(
                        ConstrainedBox::new(label(name, theme.disabled_text(), 9.8, font))
                            .with_width(82.0)
                            .finish(),
                    )
                    .with_child(
                        Expanded::new(1.0, label(value, theme.sub_text(), 10.0, font)).finish(),
                    )
                    .finish(),
            )
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish(),
        );
    }
    Container::new(content.finish())
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(9.0)
        .with_border(Border::right(1.0).with_border_color(theme.border()))
        .finish()
}

pub(crate) fn current_task(
    task: &PlanTask,
    attempt: Option<&ExecutionAttempt>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(label("Current task", theme.main_text(), 11.5, font))
        .with_child(label(&task.title, theme.main_text(), 11.0, font))
        .with_child(label(&task.description, theme.sub_text(), 9.8, font));
    if let Some(attempt) = attempt {
        content = content.with_child(label(
            &format!(
                "Attempt {} · {} · {:?}",
                attempt.attempt_number, attempt.model_route, attempt.status
            ),
            theme.disabled_text(),
            9.6,
            font,
        ));
        if let Some(summary) = &attempt.change_summary {
            for file in &summary.files {
                content = content.with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(5.0)
                        .with_child(render_icon(AppIcon::File, theme.disabled_text(), 9.0))
                        .with_child(label(
                            &file.relative_path.to_string_lossy(),
                            theme.sub_text(),
                            9.6,
                            font,
                        ))
                        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                        .with_child(label(
                            &format!(
                                "+{} -{}",
                                file.added_lines.unwrap_or(0),
                                file.deleted_lines.unwrap_or(0)
                            ),
                            theme.link(),
                            9.5,
                            font,
                        ))
                        .finish(),
                );
            }
        }
    } else {
        content = content.with_child(label(
            "Waiting for the scheduler to create an execution attempt.",
            theme.disabled_text(),
            9.8,
            font,
        ));
    }
    Container::new(content.finish())
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(9.0)
        .with_border(Border::right(1.0).with_border_color(theme.border()))
        .finish()
}

pub(crate) fn agent_thread(
    messages: &[(AgentMessage, AgentMessageRouting)],
    task: &PlanTask,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let relevant = messages
        .iter()
        .filter(|(_, routing)| routing.task_id.is_none() || routing.task_id == Some(task.id))
        .rev()
        .take(6)
        .collect::<Vec<_>>();
    let mut thread = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(label("Agent thread", theme.main_text(), 11.5, font));
    if relevant.is_empty() {
        thread = thread.with_child(label(
            "Handoffs, blockers, and review findings will appear here.",
            theme.disabled_text(),
            9.8,
            font,
        ));
    } else {
        for (message, _) in relevant.into_iter().rev() {
            thread = thread.with_child(evidence_card(
                message_kind_label(message.kind),
                &message.body,
                message_kind_color(message.kind, theme),
                theme,
                font,
            ));
        }
    }
    Container::new(thread.finish())
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(9.0)
        .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn evidence_rail(
    attempts: &[ExecutionAttempt],
    evidence: &[Evidence],
    attention: &[AttentionItem],
    checkpoints: &[WorkspaceCheckpointSummary],
    checkpoint_notice: Option<&str>,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let open_attention = attention
        .iter()
        .filter(|item| {
            matches!(
                item.status,
                AttentionStatus::Open | AttentionStatus::Acknowledged
            )
        })
        .collect::<Vec<_>>();
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0);
    content = content.with_child(section_title(
        "Attention",
        open_attention.len(),
        theme.ui_yellow(),
        theme,
        font,
    ));
    if open_attention.is_empty() {
        content = content.with_child(evidence_card(
            "No blockers",
            "Execution can continue without user input.",
            theme.ui_green(),
            theme,
            font,
        ));
    } else {
        // Actionable in place: a permission request approves/rejects the
        // parked tool call (the agent's own approval card resolves with it),
        // and any agent-raised item can open that agent's pane.
        for item in &open_attention {
            let mut card = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(7.0)
                .with_child(attention_card(item, theme, font, view_id));
            if item.execution_attempt_id.is_some() {
                card = card.with_child(
                    Flex::row()
                        .with_child(action_button(
                            "Open agent",
                            theme.link(),
                            WorkspaceAction::RouteToWorkspaceAttention {
                                attention_item_id: item.id,
                            },
                            *theme,
                            font,
                            view_id,
                        ))
                        .finish(),
                );
            }
            content = content.with_child(card.finish());
        }
    }
    content = content.with_child(section_title(
        "Evidence",
        evidence.len(),
        theme.link(),
        theme,
        font,
    ));
    if evidence.is_empty() {
        content = content.with_child(evidence_card(
            "No evidence yet",
            "Commands, tests, reviews, artifacts, and file changes appear as agents record them.",
            theme.sub_text(),
            theme,
            font,
        ));
    } else {
        for item in evidence.iter().rev().take(8).rev() {
            content = content.with_child(evidence_card(
                evidence_kind_label(item.kind),
                &format!("{} · {}", item.summary, item.source),
                evidence_color(item.kind, theme),
                theme,
                font,
            ));
        }
    }
    content = content.with_child(section_title(
        "Checkpoints",
        checkpoints.len(),
        theme.accent,
        theme,
        font,
    ));
    if checkpoints.is_empty() {
        content = content.with_child(evidence_card(
            "No restore points",
            "A checkpoint appears after an agent captures file changes.",
            theme.sub_text(),
            theme,
            font,
        ));
    } else {
        for checkpoint in checkpoints {
            content = content.with_child(checkpoint_card(checkpoint, theme, font, view_id));
        }
    }
    if let Some(notice) = checkpoint_notice {
        content = content.with_child(notice_card(notice, theme.ui_green(), theme, font, None));
    }
    let reviews = attempts
        .iter()
        .filter(|attempt| {
            attempt.status == ExecutionAttemptStatus::Succeeded
                && attempt.review_resolution.is_none()
        })
        .count();
    content = content.with_child(section_title(
        "Awaiting review",
        reviews,
        theme.ui_yellow(),
        theme,
        font,
    ));
    for attempt in attempts.iter().filter(|attempt| {
        attempt.status == ExecutionAttemptStatus::Succeeded && attempt.review_resolution.is_none()
    }) {
        content = content.with_child(review_card(attempt, theme, font, view_id));
    }
    Container::new(
        Clipped::new(
            Container::new(content.finish())
                .with_uniform_padding(10.0)
                .finish(),
        )
        .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .finish()
}

pub(crate) fn review_card(
    attempt: &ExecutionAttempt,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    // A "successful" attempt that captured zero files is a dead end if we
    // offer Integrate: there is nothing to integrate and the DAG stays stuck
    // with "No blockers". Make the single next step obvious instead.
    let empty_changes = attempt
        .change_summary
        .as_ref()
        .is_some_and(|summary| summary.files.is_empty());
    if empty_changes {
        return Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(6.0)
                .with_child(label(
                    "No file changes — action needed",
                    theme.ui_yellow(),
                    10.0,
                    font,
                ))
                .with_child(label(
                    "Worker reported success but changed nothing. Request revision to retry it, or Discard to cancel this task.",
                    theme.sub_text(),
                    9.5,
                    font,
                ))
                .with_child(label(&attempt.model_route, theme.sub_text(), 9.5, font))
                .with_child(
                    Flex::row()
                        .with_spacing(5.0)
                        .with_child(action_button(
                            "Discard",
                            theme.ui_red(),
                            WorkspaceAction::ResolveWorkspaceExecutionReview {
                                attempt_id: attempt.id,
                                resolution: ExecutionReviewResolution::Discarded,
                            },
                            *theme,
                            font,
                            view_id,
                        ))
                        .with_child(action_button(
                            "Request revision",
                            theme.ui_green(),
                            WorkspaceAction::ResolveWorkspaceExecutionReview {
                                attempt_id: attempt.id,
                                resolution: ExecutionReviewResolution::RevisionRequested,
                            },
                            *theme,
                            font,
                            view_id,
                        ))
                        .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(9.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.ui_yellow()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
    }
    let primary_resolution = if attempt.change_summary.is_some() {
        ExecutionReviewResolution::Integrated
    } else {
        ExecutionReviewResolution::Accepted
    };
    let primary_label = if attempt.change_summary.is_some() {
        "Integrate"
    } else {
        "Accept"
    };
    let details = attempt.change_summary.as_ref().map_or_else(
        || "Read-only result ready for approval".to_string(),
        |summary| {
            format!(
                "{} changed file{} ready for integration",
                summary.files.len(),
                if summary.files.len() == 1 { "" } else { "s" }
            )
        },
    );
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(label(&details, theme.main_text(), 10.0, font))
            .with_child(label(&attempt.model_route, theme.sub_text(), 9.5, font))
            .with_child(
                Flex::row()
                    .with_spacing(5.0)
                    .with_child(action_button(
                        "Discard",
                        theme.ui_red(),
                        WorkspaceAction::ResolveWorkspaceExecutionReview {
                            attempt_id: attempt.id,
                            resolution: ExecutionReviewResolution::Discarded,
                        },
                        *theme,
                        font,
                        view_id,
                    ))
                    .with_child(action_button(
                        "Request revision",
                        theme.ui_yellow(),
                        WorkspaceAction::ResolveWorkspaceExecutionReview {
                            attempt_id: attempt.id,
                            resolution: ExecutionReviewResolution::RevisionRequested,
                        },
                        *theme,
                        font,
                        view_id,
                    ))
                    .with_child(action_button(
                        primary_label,
                        theme.ui_green(),
                        WorkspaceAction::ResolveWorkspaceExecutionReview {
                            attempt_id: attempt.id,
                            resolution: primary_resolution,
                        },
                        *theme,
                        font,
                        view_id,
                    ))
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(9.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.ui_yellow()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}
