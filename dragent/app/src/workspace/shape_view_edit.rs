//! Shape workspace record editing views.
//!
//! Moved verbatim from shape_workspace; same behavior.

//! Deterministic Milestone-1 projection for the Shape workspace.
//!
//! Work, conversation,
//! shaping records, and proposed roles are projected from canonical workspace
//! events. Draft role contracts remain distinct from runtime agents and cannot
//! launch execution from this surface.

use super::shape_view_composer::*;
use super::shape_view_nav::*;
use super::shape_view_plan::*;
use super::shape_view_roles::*;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex,
    MainAxisSize, ParentElement, ScrollbarWidth, Text,
};
use warpui::fonts::FamilyId;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};
use dragent_workspace_model::{
    ArtifactRef, Decision, DecisionStatus, ExecutionLocation, Plan, ShapingProposal,
    ShapingRecords, WorkBrief,
};

use super::{
    work_item_surface::{WI_BODY, WI_BUTTON, WI_CAPTION, WI_HEADING, WI_SMALL, WI_SUBHEAD},
    InputEditor, WorkspaceAction,
};

/// Shared editing affordances for one inline-editable plan surface: the
/// active edit target, the single shared record editor, and the armed
/// (confirm-stage) remove target. Threaded through every editable row so the
/// editor panel opens exactly where its edit button was clicked.
pub(super) struct RecordEditUi<'a> {
    pub target: Option<&'a super::ShapeRecordEditTarget>,
    pub editor: ViewHandle<InputEditor>,
    pub has_text: bool,
    pub pending: bool,
    pub error: Option<&'a str>,
    pub remove_armed: Option<&'a super::ShapeRecordEditTarget>,
}

pub(crate) fn editable_record_list(
    items: impl IntoIterator<Item = (String, super::ShapeRecordEditTarget)>,
    marker: &'static str,
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    for (text, target) in items {
        let active = ui.target == Some(&target);
        let theme = *theme;
        let edit = Hoverable::new(
            super::execution_workspace::stable_mouse(format!("shape:record:{target:?}")),
            move |state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(7.0)
                        .with_child(label(marker, theme.accent, WI_BODY, font))
                        .with_child(
                            Expanded::new(1.0, label(&text, theme.sub_text(), WI_BODY, font))
                                .finish(),
                        )
                        .with_child(render_icon(AppIcon::Edit, theme.disabled_text(), 10.0))
                        .finish(),
                )
                .with_padding_left(5.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if active || state.is_hovered() {
                    theme.accent_overlay()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |event_ctx, _, _| {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::StartShapeRecordEdit { target },
            );
        })
        .finish();
        // Two-step remove: the first press arms ("Confirm remove"), the
        // second executes. An armed row also offers Cancel to back out.
        let armed = ui.remove_armed == Some(&target);
        let mut actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, edit).finish())
            .with_child(record_edit_button(
                if armed { "Confirm remove" } else { "Remove" },
                WorkspaceAction::RemoveShapeRecord { target },
                !ui.pending,
                armed,
                workspace_view_id,
                &theme,
                font,
            ));
        if armed {
            actions = actions.with_child(
                Container::new(record_edit_button(
                    "Cancel",
                    WorkspaceAction::CancelRemoveConfirm,
                    !ui.pending,
                    false,
                    workspace_view_id,
                    &theme,
                    font,
                ))
                .with_padding_left(6.0)
                .finish(),
            );
        }
        let mut entry = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(actions.finish());
        // Inline editor: opens exactly where its edit button was clicked,
        // so no scroll-back is ever needed to type.
        if active {
            entry = entry.with_child(record_edit_panel(
                target,
                ui.editor.clone(),
                ui.has_text,
                ui.pending,
                ui.error,
                workspace_view_id,
                &theme,
                font,
            ));
        }
        list = list.with_child(entry.finish());
    }
    list.finish()
}

/// Open decisions with an inline Resolve affordance: every "needs decision"
/// item offers Resolve (records resolution text and flips status to
/// Resolved), reword-on-click, and the shared two-step Remove. Resolved items
/// show their resolution and stay editable.
pub(crate) fn editable_decision_list(
    decisions: &[Decision],
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0);
    if decisions.is_empty() {
        return Container::new(label(
            "No open decisions",
            theme.disabled_text(),
            WI_BODY,
            font,
        ))
        .finish();
    }
    for (index, record) in decisions.iter().enumerate() {
        let resolved = record.status == DecisionStatus::Resolved;
        let reword_target = super::ShapeRecordEditTarget::Decision(record.id);
        let resolve_target = super::ShapeRecordEditTarget::DecisionResolution(record.id);
        let reword_active = ui.target == Some(&reword_target);
        let resolve_active = ui.target == Some(&resolve_target);
        let theme = *theme;
        let question_text = format!("{}. {}", index + 1, record.question);
        let edit = Hoverable::new(
            super::execution_workspace::stable_mouse(format!("shape:decision:{reword_target:?}")),
            move |state| {
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(7.0)
                        .with_child(label("?", theme.accent, WI_BODY, font))
                        .with_child(
                            Expanded::new(
                                1.0,
                                label(&question_text, theme.sub_text(), WI_BODY, font),
                            )
                            .finish(),
                        )
                        .with_child(status_badge(
                            if resolved {
                                "Resolved"
                            } else {
                                "Needs decision"
                            },
                            if resolved {
                                theme.ui_green()
                            } else {
                                theme.ui_yellow()
                            },
                            &theme,
                            font,
                        ))
                        .with_child(render_icon(AppIcon::Edit, theme.disabled_text(), 10.0))
                        .finish(),
                )
                .with_padding_left(5.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if reword_active || state.is_hovered() {
                    theme.accent_overlay()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            },
        )
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down(move |event_ctx, _, _| {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::StartShapeRecordEdit {
                    target: reword_target,
                },
            );
        })
        .finish();
        let armed = ui.remove_armed == Some(&reword_target);
        let mut actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(Expanded::new(1.0, edit).finish());
        if !resolved {
            actions = actions.with_child(record_edit_button(
                "Resolve",
                WorkspaceAction::StartShapeRecordEdit {
                    target: resolve_target,
                },
                !ui.pending && !resolve_active,
                true,
                workspace_view_id,
                &theme,
                font,
            ));
        }
        actions = actions.with_child(record_edit_button(
            if armed { "Confirm remove" } else { "Remove" },
            WorkspaceAction::RemoveShapeRecord {
                target: reword_target,
            },
            !ui.pending,
            armed,
            workspace_view_id,
            &theme,
            font,
        ));
        if armed {
            actions = actions.with_child(
                Container::new(record_edit_button(
                    "Cancel",
                    WorkspaceAction::CancelRemoveConfirm,
                    !ui.pending,
                    false,
                    workspace_view_id,
                    &theme,
                    font,
                ))
                .with_padding_left(6.0)
                .finish(),
            );
        }
        let mut entry = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(actions.finish());
        if let Some(resolution) = record
            .resolution
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            entry = entry.with_child(
                Container::new(label(
                    &format!("→ {resolution}"),
                    theme.sub_text(),
                    10.0,
                    font,
                ))
                .with_padding_left(22.0)
                .finish(),
            );
        }
        if reword_active {
            entry = entry.with_child(record_edit_panel(
                reword_target,
                ui.editor.clone(),
                ui.has_text,
                ui.pending,
                ui.error,
                workspace_view_id,
                &theme,
                font,
            ));
        }
        if resolve_active {
            entry = entry.with_child(record_edit_panel(
                resolve_target,
                ui.editor.clone(),
                ui.has_text,
                ui.pending,
                ui.error,
                workspace_view_id,
                &theme,
                font,
            ));
        }
        list = list.with_child(entry.finish());
    }
    list.finish()
}

pub(crate) fn with_add_control(
    body: Box<dyn Element>,
    label_text: &'static str,
    target: super::ShapeRecordEditTarget,
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(body)
        .with_child(
            Container::new(record_edit_button(
                label_text,
                WorkspaceAction::StartShapeRecordEdit { target },
                !ui.pending,
                false,
                workspace_view_id,
                theme,
                font,
            ))
            .with_padding_top(5.0)
            .finish(),
        );
    // Inline editor for "+ Add …": opens right under its own button.
    if ui.target == Some(&target) {
        col = col.with_child(record_edit_panel(
            target,
            ui.editor.clone(),
            ui.has_text,
            ui.pending,
            ui.error,
            workspace_view_id,
            theme,
            font,
        ));
    }
    col.finish()
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
/// One task's editable row: title/description summary, edit-in-place, and
/// move-up/move-down/remove controls. `index`/`total_tasks` are the task's
/// position in the plan's own flat order (what `MoveShapePlanTask` actually
/// reorders) — independent of which role group it visually renders under.
pub(crate) fn plan_task_row(
    task: &dragent_workspace_model::PlanTask,
    index: usize,
    total_tasks: usize,
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let detail = if task.description.trim().is_empty() {
        format!(
            "{} dependencies · {} criteria",
            task.depends_on.len(),
            task.acceptance_criteria.len()
        )
    } else {
        format!(
            "{} · {} dependencies · {} criteria",
            task.description,
            task.depends_on.len(),
            task.acceptance_criteria.len()
        )
    };
    let target = super::ShapeRecordEditTarget::PlanTask(task.id);
    let active = ui.target == Some(&target);
    let pending = ui.pending;
    let theme_value = *theme;
    let title = task.title.clone();
    let edit = Hoverable::new(
        super::execution_workspace::stable_mouse(format!("shape:task:{target:?}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(7.0)
                    .with_child(label("→", theme_value.accent, WI_BODY, font))
                    .with_child(
                        Expanded::new(
                            1.0,
                            label(
                                &format!("{title}\n{detail}"),
                                theme_value.sub_text(),
                                10.2,
                                font,
                            ),
                        )
                        .finish(),
                    )
                    .with_child(render_icon(
                        AppIcon::Edit,
                        theme_value.disabled_text(),
                        10.0,
                    ))
                    .finish(),
            )
            .with_padding_left(5.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(if active || state.is_hovered() {
                theme_value.accent_overlay()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::StartShapeRecordEdit { target },
        );
    })
    .finish();

    let mut entry = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.0)
        .with_child({
            let mut actions = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Expanded::new(1.0, edit).finish())
                .with_child(record_edit_button(
                    "↑",
                    WorkspaceAction::MoveShapePlanTask {
                        task_id: task.id,
                        direction: super::ShapePlanTaskMove::Up,
                    },
                    index > 0 && !pending,
                    false,
                    workspace_view_id,
                    theme,
                    font,
                ))
                .with_child(record_edit_button(
                    "↓",
                    WorkspaceAction::MoveShapePlanTask {
                        task_id: task.id,
                        direction: super::ShapePlanTaskMove::Down,
                    },
                    index + 1 < total_tasks && !pending,
                    false,
                    workspace_view_id,
                    theme,
                    font,
                ))
                .with_child(record_edit_button(
                    if ui.remove_armed == Some(&target) {
                        "Confirm remove"
                    } else {
                        "Remove"
                    },
                    WorkspaceAction::RemoveShapePlanTask { task_id: task.id },
                    total_tasks > 1 && !pending,
                    ui.remove_armed == Some(&target),
                    workspace_view_id,
                    theme,
                    font,
                ));
            // Armed remove offers Cancel to back out without deleting.
            if ui.remove_armed == Some(&target) {
                actions = actions.with_child(
                    Container::new(record_edit_button(
                        "Cancel",
                        WorkspaceAction::CancelRemoveConfirm,
                        !pending,
                        false,
                        workspace_view_id,
                        theme,
                        font,
                    ))
                    .with_padding_left(6.0)
                    .finish(),
                );
            }
            actions.finish()
        });
    // Inline editor: opens under the task being edited.
    if active {
        entry = entry.with_child(record_edit_panel(
            target,
            ui.editor.clone(),
            ui.has_text,
            ui.pending,
            ui.error,
            workspace_view_id,
            theme,
            font,
        ));
    }
    entry.finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_edit_panel(
    target: super::ShapeRecordEditTarget,
    editor: ViewHandle<InputEditor>,
    has_text: bool,
    pending: bool,
    error: Option<&str>,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let title = match target {
        super::ShapeRecordEditTarget::WorkBriefOutcome => "Edit work brief goal",
        super::ShapeRecordEditTarget::WorkBriefSummary => "Edit work brief approach",
        super::ShapeRecordEditTarget::NewRequirement => "Add requirement",
        super::ShapeRecordEditTarget::Requirement(_) => "Edit requirement",
        super::ShapeRecordEditTarget::NewDecision => "Add open decision",
        super::ShapeRecordEditTarget::Decision(_) => "Edit open decision",
        super::ShapeRecordEditTarget::DecisionResolution(_) => {
            "Resolve decision — record what was decided"
        }
        super::ShapeRecordEditTarget::NewAcceptanceCriterion => "Add acceptance criterion",
        super::ShapeRecordEditTarget::AcceptanceCriterion(_) => "Edit acceptance criterion",
        super::ShapeRecordEditTarget::NewArtifact => "Add repository artifact (path=, kind=)",
        super::ShapeRecordEditTarget::Artifact(_) => "Edit repository artifact (path=, kind=)",
        super::ShapeRecordEditTarget::NewPlanTask => {
            "Add task (title, description, role, dependency, criterion)"
        }
        super::ShapeRecordEditTarget::PlanTask(_) => {
            "Revise task (title, description, role, dependency, criterion)"
        }
        super::ShapeRecordEditTarget::RoleMission(_) => "Edit role mission",
        super::ShapeRecordEditTarget::RoleCapabilities(_) => {
            "Edit capabilities (one per line or comma separated)"
        }
        super::ShapeRecordEditTarget::RoleTools(_) => {
            "Edit tools (one per line or comma separated)"
        }
        super::ShapeRecordEditTarget::RoleWriteScope(_) => "Edit project-relative write scopes",
        super::ShapeRecordEditTarget::RolePermissions(_) => {
            "Edit permissions (permission=ask or permission=allow). \
             command_execute=allow still asks live for a PR, a database write, \
             file deletion, or a destructive command (e.g. git push) — those \
             are never pre-approved."
        }
        super::ShapeRecordEditTarget::RoleExpectedOutputs(_) => {
            "Edit expected outputs (one per line or comma separated)"
        }
        super::ShapeRecordEditTarget::RoleInputArtifacts(_) => {
            "Assign repository artifacts (one exact path per line)"
        }
        super::ShapeRecordEditTarget::RoleSuccessCriteria(_) => {
            "Assign acceptance criteria (one exact criterion per line)"
        }
        super::ShapeRecordEditTarget::RoleEscalationRules(_) => {
            "Edit escalation rules (one per line or comma separated)"
        }
        super::ShapeRecordEditTarget::RoleExecutionLocation(_) => {
            "Edit execution location (Local, Cloud, or Hybrid)"
        }
        super::ShapeRecordEditTarget::RoleModelRoute(_) => {
            "Edit model route (Auto or an exact model ID)"
        }
        super::ShapeRecordEditTarget::RoleBudget(_) => {
            "Edit budget limits (name=value; use unset for no limit)"
        }
    };
    let mut panel = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(label(title, theme.main_text(), WI_SUBHEAD, font))
        .with_child(
            ConstrainedBox::new(
                Container::new(ChildView::new(&editor).finish())
                    .with_uniform_padding(7.0)
                    .with_background_color(theme.background)
                    .with_border(Border::all(1.0).with_border_color(if error.is_some() {
                        theme.ui_red()
                    } else {
                        theme.accent
                    }))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                    .finish(),
            )
            .with_height(
                if matches!(
                    target,
                    super::ShapeRecordEditTarget::NewPlanTask
                        | super::ShapeRecordEditTarget::PlanTask(_)
                ) {
                    156.0
                } else {
                    76.0
                },
            )
            .finish(),
        );
    if let Some(error) = error {
        panel = panel.with_child(label(error, theme.ui_red(), WI_SMALL, font));
    }
    panel = panel.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(label(
                if pending {
                    "Saving…"
                } else {
                    "Changes are saved to workspace history"
                },
                theme.disabled_text(),
                WI_SMALL,
                font,
            ))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(record_edit_button(
                "Cancel",
                WorkspaceAction::CancelShapeRecordEdit,
                !pending,
                false,
                workspace_view_id,
                theme,
                font,
            ))
            .with_child(
                Container::new(record_edit_button(
                    "Save",
                    WorkspaceAction::SaveShapeRecordEdit,
                    has_text && !pending,
                    true,
                    workspace_view_id,
                    theme,
                    font,
                ))
                .with_padding_left(6.0)
                .finish(),
            )
            .finish(),
    );
    Container::new(panel.finish())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
}

pub(crate) fn record_edit_button(
    text: &'static str,
    action: WorkspaceAction,
    enabled: bool,
    primary: bool,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(
        super::execution_workspace::stable_mouse(format!("shape:button:{text}:{action:?}")),
        move |state| {
            Container::new(label(
                text,
                if primary && enabled {
                    theme.accent_text()
                } else if enabled {
                    theme.main_text()
                } else {
                    theme.disabled_text()
                },
                WI_BUTTON,
                font,
            ))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if primary && enabled {
                if state.is_hovered() {
                    theme.link()
                } else {
                    theme.accent
                }
            } else if state.is_hovered() && enabled {
                theme.surface_2()
            } else {
                theme.background
            })
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        },
    )
    .with_cursor(if enabled {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_mouse_down(move |event_ctx, _, _| {
        if enabled {
            event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        }
    })
    .finish()
}

pub(crate) fn plan_row(
    title: &str,
    body: Box<dyn Element>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            ConstrainedBox::new(
                Container::new(label(title, theme.main_text(), WI_SUBHEAD, font))
                    .with_padding_left(12.0)
                    .with_padding_top(8.0)
                    .finish(),
            )
            .with_width(125.0)
            .finish(),
        )
        .with_child(
            Expanded::new(
                1.0,
                Container::new(body)
                    .with_padding_left(12.0)
                    .with_padding_right(10.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_border(Border::left(1.0).with_border_color(theme.border()))
                    .finish(),
            )
            .finish(),
        )
        .finish()
}
