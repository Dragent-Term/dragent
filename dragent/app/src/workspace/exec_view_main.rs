//! Execution workspace main views: render entry, activity, summary.
//!
//! Moved verbatim; same behavior.

//! Coordinate workspace projected exclusively from canonical workflow state.
//!
//! No sample agents, tasks, test counts, artifacts, or timestamps belong in
//! this view. An empty canonical projection renders an honest onboarding state.

use super::exec_view_attention::*;
use super::exec_view_cards::*;
use super::exec_view_tasks::*;
use super::execution_workspace::{
    ACTIVITY_HEIGHT, CONTENT_BOTTOM_PADDING, MINIMUM_MAIN_PANE_WIDTH,
    MINIMUM_SCROLLING_CONTENT_HEIGHT, NARROW_CONTENT_WIDTH, NAV_WIDTH,
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

thread_local! {
    static COORDINATE_MOUSE_STATES: std::cell::RefCell<
        std::collections::HashMap<String, MouseStateHandle>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

thread_local! {
    /// Live per-attempt state labels ("working: read_file src/app.tsx",
    /// "waiting on the Principal", "needs your approval") set by the view
    /// from each mounted controller right before Coordinate renders.
    static LIVE_ATTEMPT_STATES: std::cell::RefCell<
        std::collections::HashMap<ExecutionAttemptId, String>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

pub(crate) fn set_live_attempt_states(
    states: std::collections::HashMap<ExecutionAttemptId, String>,
) {
    LIVE_ATTEMPT_STATES.with(|cell| *cell.borrow_mut() = states);
}

pub(crate) fn live_attempt_state(attempt_id: Option<ExecutionAttemptId>) -> Option<String> {
    let attempt_id = attempt_id?;
    LIVE_ATTEMPT_STATES.with(|cell| cell.borrow().get(&attempt_id).cloned())
}

pub(crate) fn stable_mouse(key: String) -> MouseStateHandle {
    COORDINATE_MOUSE_STATES.with(|states| {
        states
            .borrow_mut()
            .entry(key)
            .or_insert_with(MouseStateHandle::default)
            .clone()
    })
}

/// Narrow threshold shared by the layout branch and the scroll
/// arrangement: both must agree exactly, or a content-sized (Min) tree gets
/// pinned to a fixed canvas height (unreachable overflow) or a stretched
/// (Max/Expanded) tree gets measured unbounded (debug panic).
pub(crate) fn coordinate_is_narrow(viewport_width: f32) -> bool {
    (viewport_width - NAV_WIDTH).max(1.0) < NARROW_CONTENT_WIDTH
}

pub(crate) const EXECUTION_MAP_GUTTER: f32 = 14.0;
pub(crate) const EXECUTION_CARD_HORIZONTAL_PADDING: f32 = 12.0;
pub(crate) const EXECUTION_CARD_VERTICAL_PADDING: f32 = 10.0;

#[allow(clippy::too_many_arguments)]
pub fn render(
    theme: &Theme,
    font: FamilyId,
    viewport_width: f32,
    viewport_height: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    work_title: Option<&str>,
    work_status: WorkStatus,
    active_execution_count: usize,
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    attempts: &[ExecutionAttempt],
    messages: &[(AgentMessage, AgentMessageRouting)],
    evidence: &[Evidence],
    attention: &[AttentionItem],
    checkpoints: &[WorkspaceCheckpointSummary],
    checkpoint_notice: Option<&str>,
    execution_notice: Option<&str>,
    workspace_view_id: EntityId,
    activity_scroll: ClippedScrollStateHandle,
    activity_dock_state: ResizableStateHandle,
) -> Box<dyn Element> {
    // Live dock height from the drag state; the map viewport shrinks as the
    // dock grows, exactly like the editor yields to the docked terminal.
    let activity_height = activity_dock_state
        .lock()
        .ok()
        .map(|state| state.size())
        .unwrap_or(ACTIVITY_HEIGHT);
    let primary = primary_area(
        viewport_width,
        plan,
        roles,
        attempts,
        messages,
        evidence,
        attention,
        checkpoints,
        checkpoint_notice,
        execution_notice,
        theme,
        font,
        workspace_view_id,
    );
    // Narrow widths stack content taller than any viewport: size to
    // content with no fixed canvas height so the whole page stays
    // reachable. Wide keeps the stretched arrangement with a pinned canvas.
    let narrow = coordinate_is_narrow(viewport_width);
    let scrolling_content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(if narrow {
            MainAxisSize::Min
        } else {
            MainAxisSize::Max
        })
        .with_child(super::work_journey::render(
            dragent_workspace_model::BuiltInLayout::Coordinate,
            work_status,
            theme,
            font,
        ))
        .with_child(execution_summary(
            active_execution_count,
            plan,
            work_status,
            theme,
            font,
            workspace_view_id,
        ))
        .with_child(if narrow {
            primary
        } else {
            Expanded::new(1.0, primary).finish()
        })
        .with_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(CONTENT_BOTTOM_PADDING)
                .finish(),
        )
        .finish();
    let main_pane = super::work_canvas::render(
        theme,
        horizontal_scroll,
        vertical_scroll,
        (viewport_width - NAV_WIDTH).max(1.0),
        (viewport_height - activity_height).max(1.0),
        MINIMUM_MAIN_PANE_WIDTH,
        if narrow {
            None
        } else {
            Some(MINIMUM_SCROLLING_CONTENT_HEIGHT)
        },
        scrolling_content,
    );
    let workspace = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1.0, main_pane).finish())
        .with_child(render_activity_dock_panel(
            activity_dock(
                attempts,
                messages,
                evidence,
                workspace_view_id,
                activity_scroll,
                theme,
                font,
            ),
            &activity_dock_state,
            theme,
        ))
        .finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                ConstrainedBox::new(super::shape_workspace::work_navigation(
                    theme, font, work_title,
                ))
                .with_width(NAV_WIDTH)
                .finish(),
            )
            .with_child(Expanded::new(1.0, workspace).finish())
            .finish(),
    )
    .with_background_color(theme.pane_surface())
    .finish()
}

/// Coordinate content hosted inside a first-class work-item editor tab.
///
/// This intentionally reuses every canonical projection and action from the
/// full Coordinate arrangement, while omitting its duplicate work navigator
/// and journey header. The surrounding editor shell owns that navigation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_embedded(
    theme: &Theme,
    font: FamilyId,
    viewport_width: f32,
    viewport_height: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    work_status: WorkStatus,
    active_execution_count: usize,
    plan: Option<&Plan>,
    roles: &[ProposedRoleView],
    attempts: &[ExecutionAttempt],
    messages: &[(AgentMessage, AgentMessageRouting)],
    evidence: &[Evidence],
    attention: &[AttentionItem],
    checkpoints: &[WorkspaceCheckpointSummary],
    checkpoint_notice: Option<&str>,
    execution_notice: Option<&str>,
    workspace_view_id: EntityId,
    activity_scroll: ClippedScrollStateHandle,
    activity_dock_state: ResizableStateHandle,
) -> Box<dyn Element> {
    // Live dock height from the drag state; the map viewport shrinks as the
    // dock grows, exactly like the editor yields to the docked terminal.
    let activity_height = activity_dock_state
        .lock()
        .ok()
        .map(|state| state.size())
        .unwrap_or(ACTIVITY_HEIGHT);
    let primary = primary_area(
        viewport_width,
        plan,
        roles,
        attempts,
        messages,
        evidence,
        attention,
        checkpoints,
        checkpoint_notice,
        execution_notice,
        theme,
        font,
        workspace_view_id,
    );
    // Narrow widths stack content taller than any viewport: size to
    // content with no fixed canvas height so the whole page stays
    // reachable. Wide keeps the stretched arrangement with a pinned canvas.
    let narrow = coordinate_is_narrow(viewport_width);
    let scrolling_content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(if narrow {
            MainAxisSize::Min
        } else {
            MainAxisSize::Max
        })
        .with_child(execution_summary(
            active_execution_count,
            plan,
            work_status,
            theme,
            font,
            workspace_view_id,
        ))
        .with_child(if narrow {
            primary
        } else {
            Expanded::new(1.0, primary).finish()
        })
        .with_child(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(CONTENT_BOTTOM_PADDING)
                .finish(),
        )
        .finish();
    let main_pane = super::work_canvas::render(
        theme,
        horizontal_scroll,
        vertical_scroll,
        viewport_width.max(1.0),
        (viewport_height - activity_height).max(1.0),
        MINIMUM_MAIN_PANE_WIDTH,
        if narrow {
            None
        } else {
            Some(MINIMUM_SCROLLING_CONTENT_HEIGHT)
        },
        scrolling_content,
    );
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Expanded::new(1.0, main_pane).finish())
            .with_child(render_activity_dock_panel(
                activity_dock(
                    attempts,
                    messages,
                    evidence,
                    workspace_view_id,
                    activity_scroll,
                    theme,
                    font,
                ),
                &activity_dock_state,
                theme,
            ))
            .finish(),
    )
    .with_background_color(theme.pane_surface())
    .finish()
}

/// Draggable activity dock panel. Same recipe as the docked terminal: a
/// top dragbar in the accent color, bounded between a readable minimum and
/// 60% of the window, with resize repaints.
pub(crate) fn render_activity_dock_panel(
    dock: Box<dyn Element>,
    dock_state: &ResizableStateHandle,
    theme: &Theme,
) -> Box<dyn Element> {
    let dock = Container::new(dock)
        .with_border(Border::top(2.0).with_border_color(theme.accent))
        .finish();
    Resizable::new(dock_state.clone(), dock)
        .with_dragbar_side(DragBarSide::Top)
        .with_dragbar_color(theme.accent.into())
        .on_resize(|ctx, _| ctx.notify())
        .with_bounds_callback(Box::new(|window_size| {
            let min_height = 110.0;
            let max_height = (window_size.y() * 0.6).max(min_height);
            (min_height, max_height)
        }))
        .finish()
}

pub(crate) fn execution_summary(
    active_count: usize,
    plan: Option<&Plan>,
    status: WorkStatus,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let (completed, total) = plan.map_or((0, 0), |plan| {
        (
            plan.tasks
                .iter()
                .filter(|task| task.status == PlanTaskStatus::Completed)
                .count(),
            plan.tasks.len(),
        )
    });
    let progress = (total != 0)
        .then_some(completed as f32 / total as f32)
        .unwrap_or(0.0);
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(status_dot(theme.accent))
        .with_child(label(work_status_label(status), theme.accent, 11.5, font))
        .with_child(divider(theme))
        .with_child(label(
            &format!(
                "{active_count} active agent{}",
                if active_count == 1 { "" } else { "s" }
            ),
            theme.main_text(),
            11.0,
            font,
        ))
        .with_child(progress_bar(86.0, progress, theme.accent, theme))
        .with_child(label(
            &format!("{completed}/{total} tasks"),
            theme.sub_text(),
            10.5,
            font,
        ))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if matches!(status, WorkStatus::Executing | WorkStatus::Paused) {
        row = row
            .with_child(action_button(
                if status == WorkStatus::Paused {
                    "Resume"
                } else {
                    "Pause"
                },
                theme.sub_text(),
                WorkspaceAction::ToggleWorkspaceExecutionPause,
                *theme,
                font,
                view_id,
            ))
            .with_child(action_button(
                "Cancel",
                theme.ui_red(),
                WorkspaceAction::CancelWorkspaceExecution,
                *theme,
                font,
                view_id,
            ));
    }
    Container::new(row.finish())
        .with_padding_left(14.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
}
