//! Work-item surface header: render entry, knowledge, header, routes.
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

pub(crate) fn render(
    props: WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    let header = work_header(&props, theme, font, workspace_view_id, mouse_state);
    let content = match props.active_view {
        WorkItemView::Conversation => {
            conversation_view(&props, theme, font, workspace_view_id, mouse_state)
        }
        WorkItemView::Plan => plan_view(&props, theme, font, workspace_view_id, mouse_state),
        WorkItemView::Coordinate => {
            // Pre-coordination statuses never reach the execution map: the
            // gate explains what's missing and offers Start Coordination.
            if matches!(
                props.status,
                WorkStatus::Draft
                    | WorkStatus::Understanding
                    | WorkStatus::WaitingForDecision
                    | WorkStatus::ReadyToPlan
                    | WorkStatus::Planning
            ) {
                coordinate_gate(&props, theme, font, workspace_view_id, mouse_state)
            } else {
                super::execution_workspace::render_embedded(
                    theme,
                    font,
                    props.coordinate_viewport_width,
                    props.coordinate_viewport_height,
                    props.coordinate_horizontal_scroll.clone(),
                    props.coordinate_vertical_scroll.clone(),
                    props.status,
                    props.active_execution_count,
                    props.plan,
                    props.proposed_roles,
                    props.attempts,
                    props.agent_messages,
                    props.evidence,
                    props.attention,
                    props.checkpoints,
                    props.checkpoint_notice,
                    props.execution_notice,
                    workspace_view_id,
                    props.activity_scroll.clone(),
                    props.activity_dock_state.clone(),
                )
            }
        }
        WorkItemView::Changes => changes_view(&props, theme, font, workspace_view_id),
        WorkItemView::Review => super::review_workspace::render_embedded(
            theme,
            font,
            props.coordinate_viewport_width,
            props.coordinate_viewport_height,
            props.review_horizontal_scroll.clone(),
            props.review_vertical_scroll.clone(),
            props.completion_report,
            props.evidence,
            workspace_view_id,
        ),
        WorkItemView::Knowledge => knowledge_view(&props, theme, font),
        WorkItemView::Agent => agent_timeline_view(&props, theme, font, workspace_view_id),
    };

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(Expanded::new(1.0, Clipped::new(content).finish()).finish())
            .finish(),
    )
    .with_background_color(theme.pane_surface())
    .finish()
}

pub(crate) fn knowledge_view(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(heading("Shared knowledge ledger", theme, font))
        .with_child(body_text(
            "What the Principal and the worker agents have recorded for each other on this work item: decisions and their reasons, discovered behavior and limitations, root causes, handoffs, answered questions. Every agent reads this before it researches.",
            theme.sub_text(),
            font,
        ));
    if props.knowledge.is_empty() {
        content = content.with_child(
            Container::new(body_text(
                "Nothing recorded yet. Notes appear here as agents call `record_knowledge` or the Principal finishes a turn.",
                theme.sub_text(),
                font,
            ))
            .with_uniform_padding(16.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    } else {
        let mut ledger = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0);
        for note in props.knowledge.iter().rev() {
            let kind = format!("{:?}", note.kind).to_ascii_lowercase();
            let color = match note.kind {
                dragent_workspace_model::ShapingMemoryKind::Decision => theme.accent,
                dragent_workspace_model::ShapingMemoryKind::Approval => theme.ui_green(),
                dragent_workspace_model::ShapingMemoryKind::Rejected => theme.ui_red(),
                dragent_workspace_model::ShapingMemoryKind::Change => theme.ui_yellow(),
                _ => theme.sub_text(),
            };
            ledger = ledger.with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(10.0)
                        .with_child(status_pill(&kind, color, theme, font))
                        .with_child(
                            Expanded::new(1.0, body_text(&note.text, theme.main_text(), font))
                                .finish(),
                        )
                        .with_child(body_text(
                            &super::execution_workspace::format_time(note.recorded_at_unix_ms),
                            theme.disabled_text(),
                            font,
                        ))
                        .finish(),
                )
                .with_uniform_padding(10.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            );
        }
        content = content.with_child(ledger.finish());
    }
    content = content
        .with_child(heading("Project memory", theme, font))
        .with_child(body_text(
            &format!(
                "Structured outcomes and resolved decisions shared by work items in {}. Private conversations and tool traces stay inside their work item.",
                props.project_scope
            ),
            theme.sub_text(),
            font,
        ));

    if props.project_memory.is_empty() {
        content = content.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(6.0)
                    .with_child(heading("No shared project memory yet", theme, font))
                    .with_child(body_text(
                        "As work items produce structured briefs and resolved decisions, relevant context will become available to other work in this folder.",
                        theme.sub_text(),
                        font,
                    ))
                    .finish(),
            )
            .with_uniform_padding(16.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    } else {
        for memory in props.project_memory {
            let mut card = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(7.0)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(heading(&memory.title, theme, font))
                        .with_child(status_pill(
                            &format!("{:?}", memory.status),
                            theme.accent,
                            theme,
                            font,
                        ))
                        .finish(),
                );
            if let Some(summary) = memory
                .summary
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                card = card.with_child(body_text(summary, theme.main_text(), font));
            }
            if let Some(outcome) = memory
                .outcome
                .as_deref()
                .filter(|text| !text.trim().is_empty())
            {
                card = card.with_child(body_text(outcome, theme.sub_text(), font));
            }
            for (question, resolution, rationale) in &memory.resolved_decisions {
                let text = rationale
                    .as_deref()
                    .filter(|text| !text.trim().is_empty())
                    .map_or_else(
                        || format!("{question}: {resolution}"),
                        |rationale| format!("{question}: {resolution} — {rationale}"),
                    );
                card = card.with_child(body_text(&text, theme.sub_text(), font));
            }
            content = content.with_child(
                Container::new(card.finish())
                    .with_uniform_padding(16.0)
                    .with_background_color(theme.surface_1())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                    .finish(),
            );
        }
    }

    ClippedScrollable::vertical(
        props.vertical_scroll.clone(),
        Container::new(content.finish())
            .with_uniform_padding(24.0)
            .finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.main_text().into(),
        theme.surface_2().into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}

pub(crate) fn work_header(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    const VIEWS: [(WorkItemView, &'static str); 6] = [
        (WorkItemView::Conversation, "work_view_conversation"),
        (WorkItemView::Plan, "work_view_plan"),
        (WorkItemView::Coordinate, "work_view_coordinate"),
        (WorkItemView::Changes, "work_view_changes"),
        (WorkItemView::Knowledge, "work_view_knowledge"),
        (WorkItemView::Review, "work_view_review"),
    ];
    let mut routes = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(3.0);
    for (view, key) in VIEWS {
        routes = routes.with_child(route_button(
            view,
            view == props.active_view,
            theme,
            font,
            workspace_view_id,
            mouse_state(key),
        ));
    }

    let status_color = match props.status {
        WorkStatus::Completed => theme.ui_green(),
        WorkStatus::Blocked | WorkStatus::Cancelled => theme.ui_red(),
        WorkStatus::WaitingForDecision | WorkStatus::NeedsUserApproval => theme.ui_yellow(),
        WorkStatus::Draft => theme.sub_text(),
        _ => theme.accent,
    };
    let status = format!("{:?}", props.status);
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(9.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(9.0)
                    .with_child(render_icon(AppIcon::Agent, theme.accent, 15.0))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(props.title.to_string(), font, crate::theme::text_size(WI_TITLE))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(status_pill(&status, status_color, theme, font))
                    .finish(),
            )
            .with_child({
                // Six fixed-width routes: in a pane dragged narrower than
                // their total, a bare row would size the header to its own
                // intrinsic width and paint past the pane edge. Scroll it
                // horizontally instead — the same treatment the editor tab
                // strip gets. Transparent everything: the header container
                // already owns the background and bottom border.
                let transparent = ColorU::new(0, 0, 0, 0);
                ClippedScrollable::horizontal(
                    props.view_tabs_scroll.clone(),
                    routes.finish(),
                    ScrollbarWidth::None,
                    transparent.into(),
                    transparent.into(),
                    transparent.into(),
                )
                .with_padding_start(0.0)
                .with_padding_end(0.0)
                .finish()
            })
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(11.0)
    .with_padding_bottom(8.0)
    .with_background_color(theme.panel_background())
    .with_border(Border::bottom(1.0).with_border_color(theme.border()))
    .finish()
}

pub(crate) fn route_button(
    view: WorkItemView,
    active: bool,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse_state, move |state| {
        Container::new(
            Text::new(view.label().to_string(), font, crate::theme::text_size(WI_SMALL))
                .with_color(if active {
                    theme.main_text()
                } else {
                    theme.sub_text()
                })
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(if active {
            theme.accent_overlay()
        } else if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .with_border(Border::bottom(1.5).with_border_color(if active {
            theme.accent
        } else {
            ColorU::transparent_black()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::SelectWorkItemView(view));
    })
    .finish()
}
