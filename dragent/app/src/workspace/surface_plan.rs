//! Work-item plan, approval, readiness, and coordination views.
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

pub(crate) fn conversation_view(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    // The conversation IS the terminal agent's block list: the work item's
    // `AgentConversationView` renders its `AgentController` with the same
    // cards (user query, streaming reply, thinking, tool steps, ask_user,
    // TodoList, receipts, errors) and owns its own scroll/follow-bottom.
    let messages: Box<dyn Element> = match props.agent_view.as_ref() {
        Some(agent) => ChildView::new(agent).finish(),
        None => empty_conversation(theme, font),
    };
    // Worker ↔ Principal coordination is not summarized into cards here:
    // each running agent has its own pane (click its role in Coordinate),
    // Principal replies land in this conversation as turns, and open
    // attention is resolved from the work header's attention popover.
    let composer = compact_composer(props, theme, font, workspace_view_id, mouse_state);
    let mut lower = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1.0, messages).finish());
    // Work Item Flow step 3: explicit Approve Shape consent. Shown until the
    // item is approved or already shaped; hidden while a turn is running.
    if !props.shape_approved && !props.has_proposal && !props.principal_pending {
        lower = lower.with_child(shape_approval_row(
            props,
            theme,
            font,
            workspace_view_id,
            mouse_state("shape_approve_button"),
        ));
    }
    Container::new(
        // Full-bleed like the terminal input bar: no side/bottom inset so
        // the action row sits ~4px above the window bottom edge. The top
        // divider inside the composer cuts across the full width.
        lower.with_child(composer).finish(),
    )
    .with_background_color(theme.pane_surface())
    .finish()
}

pub(crate) fn shape_approval_row(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    approve_ms: MouseStateHandle,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let enabled = props.requirements_complete;
    let button = Hoverable::new(approve_ms, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    Text::new("Approve Shape".to_string(), font, crate::theme::text_size(WI_BODY))
                        .with_color(if enabled {
                            theme_owned.accent_text()
                        } else {
                            theme_owned.disabled_text()
                        })
                        .finish(),
                )
                .with_child(render_icon(
                    AppIcon::Check,
                    if enabled {
                        theme_owned.accent_text()
                    } else {
                        theme_owned.disabled_text()
                    },
                    12.0,
                ))
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_background_color(if enabled {
            if state.is_hovered() {
                theme_owned.link()
            } else {
                theme_owned.accent
            }
        } else {
            theme_owned.surface_2()
        })
        .with_border(Border::all(1.0).with_border_color(if enabled {
            theme_owned.accent
        } else {
            theme_owned.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .with_cursor(if enabled {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |event_ctx, _, _| {
        if enabled {
            event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ApproveShape);
        }
    })
    .finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(body_text(
                "Approve Shape unlocks on confirmation",
                theme.sub_text(),
                font,
            ))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(button)
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .with_background_color(theme.pane_surface())
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .finish()
}

pub(crate) fn plan_view(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    let approve_mouse = mouse_state("work_item_approve");
    let Some(proposal) = props.proposal else {
        return Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.0)
                .with_child(heading("No plan yet", theme, font))
                .with_child(body_text(
                    "Continue the Principal conversation, then explicitly ask it to shape the work item.",
                    theme.sub_text(),
                    font,
                ))
                .finish(),
        )
        .with_uniform_padding(28.0)
        .finish();
    };

    let editable_plan = super::shape_workspace::render_editable_plan(
        theme,
        font,
        workspace_view_id,
        Some(proposal),
        props.work_brief,
        props.records,
        props.artifacts,
        props.plan,
        props.proposed_roles,
        props.record_edit_target,
        props.record_editor.clone(),
        props.record_edit_has_text,
        props.record_edit_pending,
        props.record_edit_error,
        props.remove_armed.as_ref(),
        props.role_action_pending,
        props.role_action_error,
        props.task_model_routes,
        props.task_model_picker_contract,
        props.task_model_picker_locked,
    );
    let theme_owned = *theme;
    let coordination_ready = props.coordination_ready;
    let approve = start_coordination_button(
        coordination_ready,
        props.coordination_pending,
        theme,
        font,
        workspace_view_id,
        approve_mouse,
    );

    let content = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(editable_plan)
            .with_child(readiness_card(props, theme, font))
            .finish(),
    )
    .with_uniform_padding(24.0)
    .finish();
    let scrolled = ClippedScrollable::vertical(
        props.vertical_scroll.clone(),
        content,
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.pane_surface().into(),
    )
    .with_overlayed_scrollbar()
    .finish();
    // Sticky footer like Approve Shape: the notice and action stay visible
    // while the plan scrolls. The streamed record lives in Conversation only
    // — no duplicated Agent activity here.
    let footer = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(coordination_notice_row(
                props,
                theme,
                font,
                workspace_view_id,
            ))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(approve)
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(24.0)
    .with_padding_right(24.0)
    .with_padding_top(10.0)
    .with_padding_bottom(14.0)
    .with_background_color(theme.pane_surface())
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .finish();
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1.0, scrolled).finish())
        .with_child(footer)
        .finish()
}

/// Shared Start Coordination button: enabled state comes from the caller,
/// a disabled click is a no-op because the notice row beside it already
/// names the blocker. While a request is in flight the button renders its
/// Working state (disabled) so a tap never looks silently ignored.
pub(crate) fn start_coordination_button(
    enabled: bool,
    pending: bool,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let label = if pending {
        "Working…"
    } else {
        "Start Coordination"
    };
    let clickable = enabled && !pending;
    Hoverable::new(mouse, move |state| {
        Container::new(
            Text::new(label.to_string(), font, crate::theme::text_size(WI_BODY))
                .with_color(if clickable {
                    theme_owned.accent_text()
                } else {
                    theme_owned.disabled_text()
                })
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_background_color(if clickable {
            if state.is_hovered() {
                theme_owned.link()
            } else {
                theme_owned.accent
            }
        } else {
            theme_owned.surface_2()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .with_cursor(if clickable {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |ctx, _, _| {
        if clickable {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::ApprovePlanAndCoordinate,
            );
        }
    })
    .finish()
}

pub(crate) fn readiness_card(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let all_done = props.readiness_checks.iter().all(|check| check.done);
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(heading(
            if all_done {
                "Ready for coordination"
            } else {
                "Before Start Coordination"
            },
            theme,
            font,
        ));
    for check in props.readiness_checks {
        let (marker, color) = if check.done {
            ("✓", theme.ui_green())
        } else {
            ("○", theme.disabled_text())
        };
        col = col.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.0)
                .with_child(
                    Text::new(marker.to_string(), font, crate::theme::text_size(WI_BODY))
                        .with_color(color)
                        .finish(),
                )
                .with_child(body_text(
                    &check.label,
                    if check.done {
                        theme.sub_text()
                    } else {
                        theme.main_text()
                    },
                    font,
                ))
                .finish(),
        );
    }
    Container::new(col.finish())
        .with_uniform_padding(14.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

/// Pure content selection for the coordination notice: a failure always
/// wins, then the current blocker for a disabled button (first undone check,
/// else the lifecycle stage), then any startup recovery note. A ready button
/// shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoordinationNoticeTone {
    Error,
    Waiting,
    Recovery,
}

pub(crate) fn coordination_notice_content(
    coordination_error: Option<&str>,
    coordination_ready: bool,
    work_status: WorkStatus,
    readiness_checks: &[ReadinessCheck],
    recovery_notice: Option<&str>,
) -> Option<(&'static str, String, CoordinationNoticeTone)> {
    if let Some(error) = coordination_error {
        return Some((
            "Coordination failed",
            error.to_string(),
            CoordinationNoticeTone::Error,
        ));
    }
    if !coordination_ready {
        if let Some(blocker) = readiness_checks.iter().find(|check| !check.done) {
            return Some((
                "Waiting to coordinate",
                format!("Next: {}", blocker.label),
                CoordinationNoticeTone::Waiting,
            ));
        }
        if !work_status.has_started_coordination() {
            let stage = match work_status {
                WorkStatus::Draft => Some("still a draft"),
                WorkStatus::Understanding => Some("still being understood"),
                WorkStatus::WaitingForDecision | WorkStatus::ReadyToPlan => {
                    Some("not yet in Planning")
                }
                _ => None,
            };
            if let Some(stage) = stage {
                return Some((
                    "Waiting to coordinate",
                    format!("Work is {stage} — finish shaping to unlock coordination"),
                    CoordinationNoticeTone::Waiting,
                ));
            }
        }
    }
    if work_status.has_started_coordination() && work_status != WorkStatus::ReadyToExecute {
        return Some((
            "Coordination is underway",
            "Open the Coordinate view to follow progress.".to_string(),
            CoordinationNoticeTone::Waiting,
        ));
    }
    recovery_notice.map(|notice| {
        (
            "Previous session recovered",
            notice.to_string(),
            CoordinationNoticeTone::Recovery,
        )
    })
}

/// Inline notice directly under the Start Coordination button, right-aligned
/// beside it: a coordination failure renders as a red error card, and a
/// disabled button always shows its first blocking reason. Plan-view
/// coordination failures are never silent — the conversation surface is not
/// consulted, and clicking the disabled button needs no extra explanation
/// because the reason is already visible.
pub(crate) fn coordination_notice_row(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let (title, body, tone) = match coordination_notice_content(
        props.coordination_error,
        props.coordination_ready,
        props.status,
        props.readiness_checks,
        props.recovery_notice,
    ) {
        Some(content) => content,
        None => return Empty::new().finish(),
    };
    let accent = match tone {
        CoordinationNoticeTone::Error => theme.ui_red(),
        CoordinationNoticeTone::Waiting => theme.disabled_text(),
        CoordinationNoticeTone::Recovery => theme.ui_yellow(),
    };
    // Errors render compact and dismissible: a title row with a Dismiss
    // action clears view-local error state without touching canonical work.
    // Waiting/recovery notes stay text-only.
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(3.0);
    if tone == CoordinationNoticeTone::Error {
        column = column.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(title.to_string(), font, crate::theme::text_size(WI_SMALL))
                            .with_color(accent)
                            .with_style(Properties {
                                weight: Weight::Semibold,
                                ..Default::default()
                            })
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(action_button(
                    "Dismiss",
                    theme.sub_text(),
                    WorkspaceAction::DismissCoordinationError,
                    *theme,
                    font,
                    workspace_view_id,
                ))
                .finish(),
        );
    } else {
        column = column.with_child(
            Text::new(title.to_string(), font, crate::theme::text_size(WI_SMALL))
                .with_color(accent)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Default::default()
                })
                .finish(),
        );
    }
    let card = Container::new(
        column
            .with_child(body_text(&body, theme.main_text(), font))
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(accent))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish();
    Flex::row()
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(ConstrainedBox::new(card).with_max_width(480.0).finish())
        .finish()
}

/// Gate in front of the execution map: before coordination starts, the
/// Coordinate tab explains what's missing instead of showing an empty map.
/// The tab stays selectable so the blocker is discoverable — but no
/// execution content is reachable without going through Start Coordination.
pub(crate) fn coordinate_gate(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    let content = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(heading("Coordinate", theme, font))
            .with_child(body_text(
                "Coordination unlocks after the work is shaped, resolved, approved, and started. Nothing runs before then — complete the checklist below or start coordination to proceed.",
                theme.sub_text(),
                font,
            ))
            .with_child(readiness_card(props, theme, font))
            .with_child(coordination_notice_row(props, theme, font, workspace_view_id))
            .with_child(
                Flex::row()
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(start_coordination_button(
                        props.coordination_ready,
                        props.coordination_pending,
                        theme,
                        font,
                        workspace_view_id,
                        mouse_state("work_item_coordinate_gate"),
                    ))
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(24.0)
    .finish();
    ClippedScrollable::vertical(
        props.coordinate_vertical_scroll.clone(),
        content,
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.pane_surface().into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}
