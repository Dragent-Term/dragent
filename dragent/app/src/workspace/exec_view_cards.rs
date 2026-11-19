//! Execution workspace shared cards, labels, and styles.
//!
//! Moved verbatim; same behavior.

//! Coordinate workspace projected exclusively from canonical workflow state.
//!
//! No sample agents, tasks, test counts, artifacts, or timestamps belong in
//! this view. An empty canonical projection renders an honest onboarding state.

use super::exec_view_attention::*;
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

pub(crate) fn role_for_task<'a>(
    task: &PlanTask,
    roles: &'a [ProposedRoleView],
) -> Option<&'a ProposedRoleView> {
    task.assigned_contract_id
        .and_then(|id| roles.iter().find(|role| role.work_contract_id == id))
}

pub(crate) fn task_style(status: PlanTaskStatus, theme: &Theme) -> (f32, ColorU) {
    match status {
        PlanTaskStatus::Completed => (1.0, theme.ui_green()),
        PlanTaskStatus::Running => (0.55, theme.link()),
        PlanTaskStatus::Reviewing | PlanTaskStatus::Verifying => (0.82, theme.ui_yellow()),
        PlanTaskStatus::Blocked | PlanTaskStatus::Failed => (0.35, theme.ui_red()),
        PlanTaskStatus::Ready => (0.08, theme.accent),
        PlanTaskStatus::Cancelled => (0.0, theme.disabled_text()),
        PlanTaskStatus::Proposed => (0.0, theme.sub_text()),
    }
}

pub(crate) fn task_status_label(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Proposed => "Proposed",
        PlanTaskStatus::Ready => "Ready",
        PlanTaskStatus::Running => "In progress",
        PlanTaskStatus::Blocked => "Blocked",
        PlanTaskStatus::Reviewing => "Reviewing",
        PlanTaskStatus::Verifying => "Verifying",
        PlanTaskStatus::Completed => "Completed",
        PlanTaskStatus::Failed => "Failed",
        PlanTaskStatus::Cancelled => "Cancelled",
    }
}

pub(crate) fn work_status_label(status: WorkStatus) -> &'static str {
    match status {
        WorkStatus::ReadyToExecute => "Ready to coordinate",
        WorkStatus::Executing => "Execution active",
        WorkStatus::Reviewing => "Review required",
        WorkStatus::Verifying => "Verifying",
        WorkStatus::Completed => "Complete",
        WorkStatus::Blocked => "Blocked",
        WorkStatus::Paused => "Paused",
        WorkStatus::Cancelled => "Cancelled",
        _ => "Coordinate",
    }
}

pub(crate) fn attention_label(kind: AttentionKind) -> &'static str {
    match kind {
        AttentionKind::Permission => "Permission required",
        AttentionKind::Approval => "Approval required",
        AttentionKind::Decision => "Decision required",
        AttentionKind::Conflict => "Conflict needs resolution",
        AttentionKind::Budget => "Budget needs attention",
        AttentionKind::Failure => "Execution failed",
        AttentionKind::Review => "Review required",
    }
}

pub(crate) fn message_kind_label(kind: AgentMessageKind) -> &'static str {
    match kind {
        AgentMessageKind::Handoff => "Handoff",
        AgentMessageKind::Blocker => "Blocker",
        AgentMessageKind::DecisionRequest => "Decision request",
        AgentMessageKind::ReviewFinding => "Review finding",
        AgentMessageKind::RevisionRequest => "Revision request",
        AgentMessageKind::Approval => "Approval",
        AgentMessageKind::StatusUpdate => "Status update",
        AgentMessageKind::Discussion => "Discussion",
    }
}

pub(crate) fn message_kind_color(kind: AgentMessageKind, theme: &Theme) -> ColorU {
    match kind {
        AgentMessageKind::Blocker => theme.ui_red(),
        AgentMessageKind::DecisionRequest
        | AgentMessageKind::ReviewFinding
        | AgentMessageKind::RevisionRequest => theme.ui_yellow(),
        AgentMessageKind::Approval => theme.ui_green(),
        AgentMessageKind::Handoff | AgentMessageKind::StatusUpdate => theme.link(),
        AgentMessageKind::Discussion => theme.sub_text(),
    }
}

pub(crate) fn attempt_color(status: ExecutionAttemptStatus, theme: &Theme) -> ColorU {
    match status {
        ExecutionAttemptStatus::Succeeded => theme.ui_green(),
        ExecutionAttemptStatus::Failed | ExecutionAttemptStatus::TimedOut => theme.ui_red(),
        ExecutionAttemptStatus::Running => theme.link(),
        ExecutionAttemptStatus::Paused => theme.ui_yellow(),
        ExecutionAttemptStatus::Cancelled => theme.disabled_text(),
    }
}

pub(crate) fn evidence_kind_label(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::FileChange => "File change",
        EvidenceKind::Command => "Command",
        EvidenceKind::Test => "Test",
        EvidenceKind::Review => "Review",
        EvidenceKind::Artifact => "Artifact",
        EvidenceKind::UserConfirmation => "User confirmation",
        EvidenceKind::ExternalReference => "External reference",
    }
}

pub(crate) fn evidence_color(kind: EvidenceKind, theme: &Theme) -> ColorU {
    match kind {
        EvidenceKind::Test | EvidenceKind::Review | EvidenceKind::UserConfirmation => {
            theme.ui_green()
        }
        EvidenceKind::FileChange | EvidenceKind::Command => theme.link(),
        EvidenceKind::Artifact | EvidenceKind::ExternalReference => theme.accent,
    }
}

pub(crate) fn format_time(timestamp_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(timestamp_ms)
        .map(|time| time.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "—".into())
}

pub(crate) fn section_title(
    title: &str,
    count: usize,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(label(title, theme.main_text(), 11.5, font))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(label(&count.to_string(), color, 10.0, font))
        .finish()
}

pub(crate) fn evidence_card(
    title: &str,
    detail: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(label(title, color, 10.0, font))
            .with_child(label(detail, theme.sub_text(), 10.0, font))
            .finish(),
    )
    .with_uniform_padding(9.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

/// Status notice with a tinted surface, a leading glyph, and an optional
/// dismiss control. Shared by the coordination-error banner (red, always
/// dismissible) and the checkpoint success notice (green, no dismiss action
/// wired yet) — `color` alone carries the severity.
pub(crate) fn notice_card(
    text: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
    dismiss: Option<(WorkspaceAction, EntityId)>,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(
            Container::new(render_icon(AppIcon::Info, color, 13.0))
                .with_padding_top(1.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, label(text, theme.main_text(), 10.2, font)).finish());
    if let Some((action, view_id)) = dismiss {
        row = row.with_child(
            Hoverable::new(
                stable_mouse(format!("coordinate:notice:{text}")),
                move |state| {
                    Container::new(render_icon(
                        AppIcon::XClose,
                        if state.is_hovered() {
                            theme_owned.main_text()
                        } else {
                            theme_owned.sub_text()
                        },
                        11.0,
                    ))
                    .finish()
                },
            )
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action_for(view_id, action.clone())
            })
            .finish(),
        );
    }
    Container::new(row.finish())
        .with_uniform_padding(10.0)
        .with_background_color(theme.tinted_surface(color))
        .with_border(Border::all(1.0).with_border_color(color))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

pub(crate) fn empty_card(
    title: &str,
    detail: &str,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(6.0)
            .with_child(label(title, theme.main_text(), 11.5, font))
            .with_child(label(detail, theme.sub_text(), 10.0, font))
            .finish(),
    )
    .with_uniform_padding(14.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish()
}

pub(crate) fn action_button(
    text: &'static str,
    color: ColorU,
    action: WorkspaceAction,
    theme: Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let mouse = stable_mouse(format!("coordinate:button:{text}:{action:?}"));
    Hoverable::new(mouse, move |state| {
        Container::new(label(text, color, 9.8, font))
            .with_padding_left(9.0)
            .with_padding_right(9.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if state.is_hovered() {
                theme.accent_overlay()
            } else {
                theme.surface_1()
            })
            .with_border(Border::all(1.0).with_border_color(if state.is_hovered() {
                color
            } else {
                theme.outline()
            }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| event_ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

pub(crate) fn progress_bar(
    width: f32,
    progress: f32,
    color: ColorU,
    theme: &Theme,
) -> Box<dyn Element> {
    let filled_width = width * progress.clamp(0.0, 1.0);
    let remaining_width = (width - filled_width).max(0.0);
    Container::new(
        Flex::row()
            .with_child(
                ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_background_color(color)
                        .finish(),
                )
                .with_width(filled_width)
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(remaining_width)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.outline())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .finish()
}

pub(crate) fn status_dot(color: ColorU) -> Box<dyn Element> {
    Container::new(Empty::new().finish())
        .with_uniform_padding(3.0)
        .with_background_color(color)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .finish()
}

pub(crate) fn divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(Empty::new().finish())
        .with_padding_left(0.5)
        .with_padding_right(0.5)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_background_color(theme.outline())
        .finish()
}

pub(crate) fn label(text: &str, color: ColorU, size: f32, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}

/// One open attention item with the context the bell dropdown shows
/// beside it.
pub struct AttentionOverlayEntry {
    pub item: AttentionItem,
    pub tool_name: Option<String>,
    pub task_title: Option<String>,
    pub work_title: Option<String>,
}

/// Full-width centered action with a colored identity (primary call to
/// action). Stronger than the quiet outline `action_button`: the border is
/// always colored and hover fills the accent wash.
pub(crate) fn action_button_strong(
    text: &'static str,
    color: ColorU,
    action: WorkspaceAction,
    theme: Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    Hoverable::new(
        stable_mouse(format!("coordinate:button-strong:{text}:{action:?}")),
        move |state| {
            Container::new(Align::new(label(text, color, 11.0, font)).finish())
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_background_color(if state.is_hovered() {
                    theme.accent_overlay()
                } else {
                    ColorU::transparent_black()
                })
                .with_border(Border::all(1.5).with_border_color(color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _, _| event_ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

pub(crate) fn attention_divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.0)
            .finish(),
    )
    .with_background_color(theme.outline())
    .finish()
}

pub(crate) fn attention_popover_card(
    entry: &AttentionOverlayEntry,
    total: usize,
    response_editor: &ViewHandle<InputEditor>,
    response_text: &str,
    view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let item = &entry.item;
    let urgent = matches!(
        item.kind,
        AttentionKind::Permission | AttentionKind::Failure | AttentionKind::Conflict
    );
    let kind_color = if urgent {
        theme.ui_red()
    } else {
        theme.ui_yellow()
    };
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(7.0)
            .with_height(7.0)
            .finish(),
    )
    .with_background_color(kind_color)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish();
    let mut location = entry.work_title.clone().unwrap_or_default();
    if let Some(task) = entry.task_title.as_deref() {
        if !location.is_empty() {
            location.push_str(" · ");
        }
        location.push_str(task);
    }
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(dot)
                .with_child(label(
                    attention_label(item.kind),
                    theme.main_text(),
                    12.5,
                    font,
                ))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(label(
                    &format!("1 of {total} · {}", format_time(item.created_at_unix_ms)),
                    theme.disabled_text(),
                    10.0,
                    font,
                ))
                .finish(),
        );
    if !location.is_empty() {
        content = content.with_child(label(&location, theme.disabled_text(), 10.5, font));
    }
    content = content.with_child(label(&item.summary, theme.main_text(), 11.5, font));
    if let Some(tool) = entry.tool_name.as_deref() {
        content = content.with_child(
            Container::new(label(
                &format!("Tool `{tool}` is waiting for approval"),
                theme.sub_text(),
                10.5,
                font,
            ))
            .with_uniform_padding(8.0)
            .with_background_color(theme.surface_1())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish(),
        );
    }
    content = content.with_child(attention_divider(theme));
    if item.kind == AttentionKind::Permission {
        // Only Approved/Rejected unpark a tool call, so permission offers
        // the three-way decision and no free-text path.
        content = content
            .with_child(action_button_strong(
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
            .with_child(action_button(
                "Always allow this tool",
                theme.ui_green(),
                WorkspaceAction::AlwaysAllowWorkspaceTool {
                    attention_item_id: item.id,
                },
                *theme,
                font,
                view_id,
            ))
            .with_child(action_button(
                "Reject",
                theme.ui_red(),
                WorkspaceAction::ResolveWorkspaceAttention {
                    attention_item_id: item.id,
                    approve: false,
                },
                *theme,
                font,
                view_id,
            ));
        if entry.tool_name.is_some() {
            content = content.with_child(label(
                "Always allow remembers this tool for the work item (this session).",
                theme.disabled_text(),
                9.5,
                font,
            ));
        }
    } else {
        let response_owned = response_text.to_string();
        let item_id = item.id;
        content = content
            .with_child(label("Your answer", theme.disabled_text(), 10.0, font))
            .with_child(
                Container::new(ChildView::new(response_editor).finish())
                    .with_uniform_padding(9.0)
                    .with_background_color(theme.pane_surface())
                    .with_border(Border::all(1.0).with_border_color(theme.accent))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                    .finish(),
            )
            .with_child(action_button_strong(
                "Send answer",
                theme.accent,
                WorkspaceAction::ResolveWorkspaceAttentionWithResponse {
                    attention_item_id: item_id,
                    approve: true,
                    response: response_owned.clone(),
                },
                *theme,
                font,
                view_id,
            ))
            .with_child(action_button(
                "Dismiss",
                theme.main_text(),
                WorkspaceAction::ResolveWorkspaceAttentionWithResponse {
                    attention_item_id: item_id,
                    approve: false,
                    response: response_owned,
                },
                *theme,
                font,
                view_id,
            ));
    }
    content = content.with_child(attention_divider(theme)).with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(action_button(
                "Back to list",
                theme.main_text(),
                WorkspaceAction::SelectWorkspaceAttentionItem {
                    attention_item_id: None,
                },
                *theme,
                font,
                view_id,
            ))
            .finish(),
    );
    Container::new(
        ConstrainedBox::new(content.finish())
            .with_width(380.0)
            .finish(),
    )
    .with_uniform_padding(16.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .finish()
}
