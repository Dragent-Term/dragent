//! Execution workspace attention and activity views.
//!
//! Moved verbatim; same behavior.

//! Coordinate workspace projected exclusively from canonical workflow state.
//!
//! No sample agents, tasks, test counts, artifacts, or timestamps belong in
//! this view. An empty canonical projection renders an honest onboarding state.

use super::exec_view_cards::*;
use super::exec_view_main::*;
use super::exec_view_tasks::*;
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

pub(crate) fn attention_card(
    item: &AttentionItem,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(label(
            attention_label(item.kind),
            theme.main_text(),
            9.8,
            font,
        ))
        .with_child(label(&item.summary, theme.sub_text(), 9.8, font));
    if item.kind == AttentionKind::Permission {
        content = content.with_child(
            Flex::row()
                .with_spacing(7.0)
                .with_child(action_button(
                    "Reject",
                    theme.main_text(),
                    WorkspaceAction::ResolveWorkspaceAttention {
                        attention_item_id: item.id,
                        approve: false,
                    },
                    *theme,
                    font,
                    view_id,
                ))
                .with_child(action_button(
                    "Approve once",
                    theme.accent,
                    WorkspaceAction::ResolveWorkspaceAttention {
                        attention_item_id: item.id,
                        approve: true,
                    },
                    *theme,
                    font,
                    view_id,
                ))
                .finish(),
        );
    } else {
        // Every non-permission attention carries a concrete next step in its
        // summary. The human can always acknowledge it as handled or dismiss
        // it; the controller keeps driving retries underneath regardless.
        // A Failure item is special: resolving it is literally what
        // authorizes the scheduler's one extra attempt (see
        // `retry_authorized` in `workspace_execution.rs`), so it gets its
        // own verb instead of the generic acknowledgement label — otherwise
        // "retry a failed agent" has no discoverable affordance at all.
        let resolve_label = if item.kind == AttentionKind::Failure {
            "Retry"
        } else {
            "Mark resolved"
        };
        content = content.with_child(
            Flex::row()
                .with_spacing(7.0)
                .with_child(action_button(
                    "Dismiss",
                    theme.main_text(),
                    WorkspaceAction::ResolveWorkspaceAttention {
                        attention_item_id: item.id,
                        approve: false,
                    },
                    *theme,
                    font,
                    view_id,
                ))
                .with_child(action_button(
                    resolve_label,
                    theme.accent,
                    WorkspaceAction::ResolveWorkspaceAttention {
                        attention_item_id: item.id,
                        approve: true,
                    },
                    *theme,
                    font,
                    view_id,
                ))
                .finish(),
        );
    }
    Container::new(content.finish())
        .with_uniform_padding(9.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.ui_yellow()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
}

pub(crate) fn checkpoint_card(
    checkpoint: &WorkspaceCheckpointSummary,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(label("Agent checkpoint", theme.main_text(), 10.5, font))
            .with_child(label(
                &format!(
                    "{} file{} · +{} -{}",
                    checkpoint.changed_files,
                    if checkpoint.changed_files == 1 {
                        ""
                    } else {
                        "s"
                    },
                    checkpoint.added_lines,
                    checkpoint.deleted_lines
                ),
                theme.sub_text(),
                9.6,
                font,
            ))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(action_button(
                        "Restore",
                        theme.accent,
                        WorkspaceAction::RestoreWorkspaceCheckpoint {
                            attempt_id: checkpoint.attempt_id,
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
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

/// Opaque dock background using the docked terminal's paint recipe: solid
/// base, optional theme image, translucent theme tint, then content. Image
/// themes keep their artwork while rows stay readable.
pub(crate) fn activity_dock_background(
    content: Box<dyn Element>,
    theme: &Theme,
) -> Box<dyn Element> {
    match Theme::bg_image_for(theme.id) {
        Some(bg) => {
            let image = warpui_core::elements::Clipped::new(
                warpui::elements::Image::new(
                    bg.asset_source(),
                    warpui::elements::CacheOption::Original,
                )
                .cover()
                .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                .finish(),
            )
            .finish();
            let tinted = Container::new(content)
                .with_background_color(theme.background)
                .finish();
            Container::new(Box::new(Stack::new().with_child(image).with_child(tinted)))
                .with_background_color(theme.solid_background())
                .finish()
        }
        None => Container::new(content)
            .with_background_color(theme.solid_background())
            .finish(),
    }
}

pub(crate) fn activity_dock(
    attempts: &[ExecutionAttempt],
    messages: &[(AgentMessage, AgentMessageRouting)],
    evidence: &[Evidence],
    view_id: EntityId,
    scroll: ClippedScrollStateHandle,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let open_attempt = |task_id: PlanTaskId, attempt_id: Option<ExecutionAttemptId>| {
        attempt_id
            .or_else(|| latest_attempt_id(attempts, task_id))
            .map(|id| WorkspaceAction::OpenWorkspaceAgentTimeline {
                task_id: Some(task_id),
                attempt_id: Some(id),
            })
    };
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(5.0);
    // No row caps: the dock scrolls, so every attempt, message, and
    // evidence item stays reachable.
    for attempt in attempts.iter().rev() {
        rows = rows.with_child(activity_row(
            &format_time(attempt.updated_at_unix_ms),
            "Agent",
            &format!("Attempt {} · {:?}", attempt.attempt_number, attempt.status),
            &attempt.model_route,
            attempt_color(attempt.status, theme),
            Some(WorkspaceAction::OpenWorkspaceAgentTimeline {
                task_id: Some(attempt.task_id),
                attempt_id: Some(attempt.id),
            }),
            view_id,
            theme,
            font,
        ));
    }
    for (message, routing) in messages.iter().rev() {
        // Message rows resolve to their routed attempt, else the latest
        // attempt of the routed task. Unrouted messages stay static.
        let open = match (routing.task_id, routing.execution_attempt_id) {
            (None, None) => None,
            (task_id, attempt_id) => {
                let resolved = attempt_id
                    .or_else(|| task_id.and_then(|task| latest_attempt_id(attempts, task)));
                resolved.map(|id| WorkspaceAction::OpenWorkspaceAgentTimeline {
                    task_id,
                    attempt_id: Some(id),
                })
            }
        };
        rows = rows.with_child(activity_row(
            &format_time(message.created_at_unix_ms),
            message_kind_label(message.kind),
            &message.body,
            "Agent communication",
            message_kind_color(message.kind, theme),
            open,
            view_id,
            theme,
            font,
        ));
    }
    for item in evidence.iter().rev() {
        let open = item.task_id.and_then(|task_id| open_attempt(task_id, None));
        rows = rows.with_child(activity_row(
            &format_time(item.created_at_unix_ms),
            "Evidence",
            &item.summary,
            &item.source,
            evidence_color(item.kind, theme),
            open,
            view_id,
            theme,
            font,
        ));
    }
    if attempts.is_empty() && messages.is_empty() && evidence.is_empty() {
        rows = rows.with_child(label(
            "Execution activity will appear here as the Principal dispatches real work.",
            theme.disabled_text(),
            10.0,
            font,
        ));
    }
    let dock = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Container::new(label("Activity", theme.main_text(), 11.5, font))
                    .with_padding_left(16.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    ClippedScrollable::vertical(
                        scroll,
                        Container::new(rows.finish())
                            .with_uniform_padding(12.0)
                            .finish(),
                        ScrollbarWidth::Auto,
                        theme.disabled_text().into(),
                        theme.main_text().into(),
                        theme.surface_2().into(),
                    )
                    .with_overlayed_scrollbar()
                    .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .finish();
    activity_dock_background(dock, theme)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn activity_row(
    time: &str,
    actor: &str,
    action: &str,
    detail: &str,
    color: ColorU,
    open: Option<WorkspaceAction>,
    view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    // Rows backed by an attempt open the inspectable runtime tab; the rest
    // stay static. Navigation only — execution is never touched. The row
    // rebuilds per hover frame from owned values (elements are single-use).
    let Some(open) = open else {
        return activity_row_body(time, actor, action, detail, color, theme, font);
    };
    let time_owned = time.to_string();
    let actor_owned = actor.to_string();
    let action_owned = action.to_string();
    let detail_owned = detail.to_string();
    let theme_owned = *theme;
    Hoverable::new(
        stable_mouse(format!(
            "coordinate:activity:{time}:{actor}:{action}:{detail}"
        )),
        move |hover| {
            let row = activity_row_body(
                &time_owned,
                &actor_owned,
                &action_owned,
                &detail_owned,
                color,
                &theme_owned,
                font,
            );
            if hover.is_hovered() {
                Container::new(row)
                    .with_background_color(theme_owned.accent_overlay())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .finish()
            } else {
                row
            }
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| event_ctx.dispatch_typed_action_for(view_id, open.clone()))
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn activity_row_body(
    time: &str,
    actor: &str,
    action: &str,
    detail: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            ConstrainedBox::new(label(time, theme.disabled_text(), 9.5, font))
                .with_width(58.0)
                .finish(),
        )
        .with_child(status_dot(color))
        .with_child(
            ConstrainedBox::new(label(actor, color, 9.8, font))
                .with_width(106.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, label(action, theme.main_text(), 9.8, font)).finish())
        .with_child(label(detail, theme.sub_text(), 9.5, font))
        .finish()
}
