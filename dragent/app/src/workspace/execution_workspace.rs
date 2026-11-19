//! Coordinate workspace projected exclusively from canonical workflow state.
//!
//! No sample agents, tasks, test counts, artifacts, or timestamps belong in
//! this view. An empty canonical projection renders an honest onboarding state.

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

pub(crate) const NAV_WIDTH: f32 = 218.0;
pub(crate) const MAP_HEIGHT: f32 = 252.0;
pub(crate) const ACTIVITY_HEIGHT: f32 = 190.0;
/// Breathing room after the last content so scrolled-to-bottom rows never
/// sit flush against the activity dock.
pub(crate) const CONTENT_BOTTOM_PADDING: f32 = 40.0;
/// Backstop horizontal scroll width. Content reflows below
/// `NARROW_CONTENT_WIDTH`; only viewports narrower than this scroll.
pub(crate) const MINIMUM_MAIN_PANE_WIDTH: f32 = 680.0;
pub(crate) const MINIMUM_SCROLLING_CONTENT_HEIGHT: f32 = 720.0;
/// Below this content width the Coordinate layout reflows: the role/task/
/// thread columns stack and the evidence rail moves below.
pub(crate) const NARROW_CONTENT_WIDTH: f32 = 960.0;
/// Task cards flow into rows of this target width. Rows are computed from
/// the measured content width so cards always receive tight constraints and
/// their text wraps instead of overflowing (unbounded measurement) or
/// squeezing into character-level breaks.
pub(crate) const TASK_CARD_TARGET_WIDTH: f32 = 270.0;
/// Maximum cards per map row even on very wide screens.
pub(crate) const TASK_CARD_MAX_COLUMNS: usize = 4;
// Stable mouse-state handles for Coordinate's clickable elements. A
// `Hoverable` only fires `on_click` when the SAME handle saw the mouse-down
// and the mouse-up — and the down itself repaints the view, so a handle
// created fresh per render never completes a click. Keyed by a stable
// identity, like the terminal's `mouse_states` maps.

/// Re-exported area modules; existing `super::execution_workspace::…` paths
/// keep working. Canonical homes are the exec_view_* modules.
pub(crate) use super::exec_view_attention::*;
pub(crate) use super::exec_view_cards::*;
pub(crate) use super::exec_view_main::*;
pub(crate) use super::exec_view_tasks::*;
