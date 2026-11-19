//! Small work-item text primitives.
//!
//! Moved verbatim; same behavior.

//! Editor-hosted presentation for one stable work item.
//!
//! Canonical work state remains owned by `WorkspaceState`; this module owns no
//! workflow truth. It is the first migration step away from app-wide Shape and
//! Coordinate screens toward one continuous editor-tab experience.

use super::execution_workspace::{
    action_button, attempt_color, attention_card, evidence_color, evidence_kind_label, format_time,
    message_kind_color, message_kind_label,
};
use super::work_item_surface::*;
use dragent_workspace_model::{
    AgentMessage, AgentMessageKind, AgentMessageRouting, ArtifactRef, AttentionItem,
    AttentionStatus, CommunicationThreadSummary, CompletionReport, Evidence, EvidenceKind,
    ExecutionAttempt, ExecutionAttemptStatus, ExecutionFileChangeKind, Plan, ShapingProposal,
    ShapingRecords, WorkBrief, WorkItemId, WorkStatus,
};
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisSize, ParentElement, ResizableStateHandle, ScrollbarWidth, SelectableArea,
    SelectionUpdateArgs, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::shimmering_text::{
    ShimmerConfig, ShimmeringTextElement, ShimmeringTextStateHandle,
};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::agent::controller::block::TodoListItem;
use crate::agent::workspace_execution::WorkspaceCheckpointSummary;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::shape_workspace::ProposedRoleView;
use super::{InputEditor, ShapeRecordEditTarget, WorkItemView, WorkspaceAction};

pub(crate) fn empty_conversation(theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(7.0)
            .with_child(heading("Explore the work with your Principal", theme, font))
            .with_child(body_text(
                "Discuss ideas, compare approaches, and gather requirements. Nothing is scheduled until you explicitly ask to shape and approve a concrete work item.",
                theme.sub_text(),
                font,
            ))
            .finish(),
    )
    .with_uniform_padding(18.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish()
}

pub(crate) fn heading(text: &str, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(WI_HEADING))
        .with_color(theme.main_text())
        .finish()
}

pub(crate) fn body_text(text: &str, color: ColorU, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(WI_BODY))
        .with_color(color)
        .soft_wrap(true)
        .finish()
}

pub(crate) fn status_pill(
    text: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Text::new(text.to_string(), font, crate::theme::text_size(WI_CAPTION))
            .with_color(color)
            .finish(),
    )
    .with_padding_left(7.0)
    .with_padding_right(7.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(color))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .finish()
}
