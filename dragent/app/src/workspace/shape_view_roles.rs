//! Shape workspace role views.
//!
//! Moved verbatim from shape_workspace; same behavior.

//! Deterministic Milestone-1 projection for the Shape workspace.
//!
//! Work, conversation,
//! shaping records, and proposed roles are projected from canonical workspace
//! events. Draft role contracts remain distinct from runtime agents and cannot
//! launch execution from this surface.

use super::shape_view_composer::*;
use super::shape_view_edit::*;
use super::shape_view_nav::*;
use super::shape_view_plan::*;
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

pub(crate) fn proposed_team(
    theme: &Theme,
    font: FamilyId,
    proposed_roles: &[ProposedRoleView],
    model_routes: &[ModelRouteOption],
    model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    model_picker_locked: bool,
    workspace_view_id: EntityId,
    describe_role_mouse_state: MouseStateHandle,
    selected_role: Option<dragent_workspace_model::WorkContractId>,
    role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    role_action_error: Option<&str>,
    edit_target: Option<&super::ShapeRecordEditTarget>,
    record_editor: ViewHandle<InputEditor>,
    record_edit_has_text: bool,
    record_edit_pending: bool,
    record_edit_error: Option<&str>,
    remove_armed: Option<&super::ShapeRecordEditTarget>,
) -> Box<dyn Element> {
    if proposed_roles.is_empty() {
        return Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    Container::new(label("Proposed team", theme.main_text(), WI_SUBHEAD, font))
                        .with_padding_left(14.0)
                        .with_padding_top(10.0)
                        .with_padding_bottom(10.0)
                        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
                        .finish(),
                )
                .with_child(
                    Container::new(label(
                        "Draft role contracts will appear after the Principal proposes a team.",
                        theme.sub_text(),
                        WI_BODY,
                        font,
                    ))
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .with_padding_top(16.0)
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(theme.sidebar_background())
        .finish();
    }
    let mut roles = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(role_card(
            "Principal Agent",
            "Shapes the work and coordinates decisions.",
            "Current shaping actor",
            "Shaping",
            theme.accent,
            true,
            theme,
            font,
        ));
    for role in proposed_roles {
        let location = match role.execution_location {
            ExecutionLocation::Local => "Local",
            ExecutionLocation::Cloud => "Cloud",
            ExecutionLocation::Hybrid => "Hybrid",
        };
        let scope = if role.write_scope.is_empty() {
            "Read-only".to_string()
        } else {
            role.write_scope.join(", ")
        };
        let approval = if role.approval_required {
            "approval required"
        } else {
            "no approval gate"
        };
        let detail = format!(
            "{} capabilities • {} tools • {location} • {approval} • {scope}",
            role.capabilities.len(),
            role.tools.len(),
        );
        let selected = selected_role == Some(role.work_contract_id);
        roles = roles.with_child(selectable_role_card(
            role,
            format!("{}  —  {}", role.rationale, detail),
            selected,
            workspace_view_id,
            theme,
            font,
        ));
        if selected {
            roles = roles.with_child(role_detail_panel(
                role,
                model_routes,
                model_picker_contract == Some(role.work_contract_id),
                model_picker_locked,
                edit_target,
                record_editor.clone(),
                record_edit_has_text,
                record_edit_pending,
                record_edit_error,
                remove_armed,
                role_action_pending == Some(role.work_contract_id),
                role_action_error,
                workspace_view_id,
                theme,
                font,
            ));
        }
    }
    let summary = format!(
        "{} proposed role{}  •  No runtime agents created",
        proposed_roles.len(),
        if proposed_roles.len() == 1 { "" } else { "s" },
    );
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(label("Proposed team", theme.main_text(), WI_SUBHEAD, font))
                        .with_child(
                            Container::new(render_icon(AppIcon::Info, theme.disabled_text(), 12.0))
                                .with_padding_left(7.0)
                                .finish(),
                        )
                        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                        .with_child(render_icon(AppIcon::ChevronUp, theme.disabled_text(), 12.0))
                        .with_child(
                            Container::new(render_icon(
                                AppIcon::DotsVertical,
                                theme.disabled_text(),
                                12.0,
                            ))
                            .with_padding_left(10.0)
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(14.0)
                .with_padding_right(12.0)
                .with_padding_top(10.0)
                .with_padding_bottom(7.0)
                .finish(),
            )
            .with_child(
                Container::new(label(&summary, theme.sub_text(), WI_BODY, font))
                    .with_padding_left(14.0)
                    .with_padding_bottom(10.0)
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Clipped::new(
                        Container::new(roles.finish())
                            .with_padding_left(34.0)
                            .with_padding_right(34.0)
                            .with_padding_bottom(8.0)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .with_child(
                Container::new(action_outline_button(
                    "Describe a role",
                    AppIcon::Plus,
                    theme,
                    font,
                    workspace_view_id,
                    describe_role_mouse_state,
                    WorkspaceAction::BeginDescribeRole,
                ))
                .with_padding_left(34.0)
                .with_padding_right(34.0)
                .with_padding_bottom(12.0)
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn role_card(
    name: &str,
    mission: &str,
    detail: &str,
    state: &str,
    color: ColorU,
    principal: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(
                        Container::new(render_icon(AppIcon::Agent, color, 13.0))
                            .with_uniform_padding(5.0)
                            .with_background_color(theme.surface_2())
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                            .finish(),
                    )
                    .with_child(label(name, theme.main_text(), WI_SUBHEAD, font))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(status_badge(state, color, theme, font))
                    .finish(),
            )
            .with_child(
                Container::new(label(mission, theme.sub_text(), WI_BODY, font))
                    .with_padding_left(29.0)
                    .finish(),
            )
            .with_child(
                Container::new(label(detail, theme.disabled_text(), WI_SMALL, font))
                    .with_padding_left(29.0)
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(9.0)
    .with_padding_right(9.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .with_background_color(if principal {
        theme.accent_overlay()
    } else {
        theme.surface_1()
    })
    .with_border(Border::all(1.0).with_border_color(if principal {
        theme.accent
    } else {
        theme.outline()
    }))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
    .finish()
}

pub(crate) fn selectable_role_card(
    role: &ProposedRoleView,
    detail: String,
    selected: bool,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let theme = *theme;
    let name = role.name.clone();
    let mission = role.mission.clone();
    let status = format!(
        "{} · {:?}",
        match role.kind {
            dragent_workspace_model::AgentRoleKind::Worker => "Worker",
            dragent_workspace_model::AgentRoleKind::Reviewer => "Reviewer",
        },
        role.status
    );
    let work_contract_id = role.work_contract_id;
    Hoverable::new(
        super::execution_workspace::stable_mouse(format!("shape:role:{work_contract_id:?}")),
        move |state| {
            Container::new(role_card(
                &name,
                &mission,
                &detail,
                &status,
                theme.link(),
                false,
                &theme,
                font,
            ))
            .with_border(
                Border::all(1.0).with_border_color(if selected || state.is_hovered() {
                    theme.accent
                } else {
                    ColorU::transparent_black()
                }),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::SelectProposedRole { work_contract_id },
        );
    })
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn role_detail_panel(
    role: &ProposedRoleView,
    model_routes: &[ModelRouteOption],
    model_picker_open: bool,
    model_picker_locked: bool,
    edit_target: Option<&super::ShapeRecordEditTarget>,
    record_editor: ViewHandle<InputEditor>,
    record_edit_has_text: bool,
    record_edit_pending: bool,
    record_edit_error: Option<&str>,
    remove_armed: Option<&super::ShapeRecordEditTarget>,
    role_action_pending: bool,
    role_action_error: Option<&str>,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let ui = RecordEditUi {
        target: edit_target,
        editor: record_editor.clone(),
        has_text: record_edit_has_text,
        pending: record_edit_pending,
        error: record_edit_error,
        remove_armed,
    };
    let ui = &ui;
    let contract_id = role.work_contract_id;
    let capabilities = if role.capabilities.is_empty() {
        "No capabilities requested".to_string()
    } else {
        role.capabilities.join(", ")
    };
    let tools = if role.tools.is_empty() {
        "No tools requested".to_string()
    } else {
        role.tools.join(", ")
    };
    let write_scope = if role.write_scope.is_empty() {
        "Read-only".to_string()
    } else {
        role.write_scope.join(", ")
    };
    let expected_outputs = if role.expected_outputs.is_empty() {
        "No expected outputs specified".to_string()
    } else {
        role.expected_outputs.join(", ")
    };
    let success_criteria = if role.success_criteria.is_empty() {
        "No acceptance criteria assigned".to_string()
    } else {
        role.success_criteria.join("; ")
    };
    let input_artifacts = if role.input_artifacts.is_empty() {
        "No repository artifacts assigned".to_string()
    } else {
        role.input_artifacts.join(", ")
    };
    let escalation_rules = if role.escalation_rules.is_empty() {
        "No escalation rules specified".to_string()
    } else {
        role.escalation_rules.join(", ")
    };
    let location = match role.execution_location {
        ExecutionLocation::Local => "Local",
        ExecutionLocation::Cloud => "Cloud",
        ExecutionLocation::Hybrid => "Hybrid",
    };
    let budget = format!(
        "Input {} • Output {} • Cost µ{} • Duration {}ms • Tools {}",
        role.maximum_input_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".into()),
        role.maximum_output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".into()),
        role.maximum_cost_micros
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".into()),
        role.maximum_duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".into()),
        role.maximum_tool_calls
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unset".into()),
    );
    let mut panel = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(label(
                    "Draft work contract",
                    theme.main_text(),
                    WI_SUBHEAD,
                    font,
                ))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(status_badge(
                    &format!("{:?} · v{}", role.status, role.version),
                    theme.ui_yellow(),
                    theme,
                    font,
                ))
                .with_child(
                    Container::new(action_outline_button(
                        "Close",
                        AppIcon::XClose,
                        theme,
                        font,
                        workspace_view_id,
                        MouseStateHandle::default(),
                        WorkspaceAction::CloseProposedRole,
                    ))
                    .with_padding_left(6.0)
                    .finish(),
                )
                .finish(),
        )
        .with_child(role_contract_field(
            "Mission",
            &role.mission,
            super::ShapeRecordEditTarget::RoleMission(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Capabilities",
            &capabilities,
            super::ShapeRecordEditTarget::RoleCapabilities(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Tools",
            &tools,
            super::ShapeRecordEditTarget::RoleTools(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Write scope",
            &write_scope,
            super::ShapeRecordEditTarget::RoleWriteScope(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Expected outputs",
            &expected_outputs,
            super::ShapeRecordEditTarget::RoleExpectedOutputs(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Input artifacts",
            &input_artifacts,
            super::ShapeRecordEditTarget::RoleInputArtifacts(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Success criteria",
            &success_criteria,
            super::ShapeRecordEditTarget::RoleSuccessCriteria(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Escalation rules",
            &escalation_rules,
            super::ShapeRecordEditTarget::RoleEscalationRules(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Permission",
            &role
                .permissions
                .iter()
                .map(|permission| {
                    format!(
                        "{}={}",
                        permission.permission,
                        if permission.requires_approval {
                            "ask"
                        } else {
                            "allow"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
            super::ShapeRecordEditTarget::RolePermissions(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_model_picker_field(
            role.preferred_model_route.as_deref(),
            model_routes,
            model_picker_open,
            model_picker_locked,
            contract_id,
            workspace_view_id,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Execution",
            location,
            super::ShapeRecordEditTarget::RoleExecutionLocation(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ))
        .with_child(role_contract_field(
            "Budget",
            &budget,
            super::ShapeRecordEditTarget::RoleBudget(contract_id),
            workspace_view_id,
            ui,
            theme,
            font,
        ));
    if let Some(error) = role_action_error {
        panel = panel.with_child(label(error, theme.ui_red(), WI_SMALL, font));
    }
    // The Approve action only exists when approval is actually possible — a
    // visible Approve button is always clickable (except while its own
    // approval is in flight). Every other status gets a truthful status line
    // instead of a dead disabled button.
    let approvable = matches!(
        role.status,
        dragent_workspace_model::WorkContractStatus::Draft
            | dragent_workspace_model::WorkContractStatus::Proposed
    );
    let status_line = if role_action_pending {
        "Approving…"
    } else if role.status == dragent_workspace_model::WorkContractStatus::Approved {
        "Approved for coordination; still not running"
    } else if approvable {
        "Approval does not launch an agent"
    } else {
        "This contract is no longer awaiting approval"
    };
    let mut action_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(label(status_line, theme.disabled_text(), WI_SMALL, font))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if approvable {
        action_row = action_row.with_child(record_edit_button(
            "Approve role",
            WorkspaceAction::ApproveProposedRole {
                work_contract_id: contract_id,
            },
            !role_action_pending,
            true,
            workspace_view_id,
            theme,
            font,
        ));
    }
    panel = panel.with_child(action_row.finish());
    if let Some(target) = edit_target.copied().filter(|target| {
        matches!(
            target,
            super::ShapeRecordEditTarget::RoleMission(id)
                | super::ShapeRecordEditTarget::RoleCapabilities(id)
                | super::ShapeRecordEditTarget::RoleTools(id)
                | super::ShapeRecordEditTarget::RoleWriteScope(id)
                | super::ShapeRecordEditTarget::RolePermissions(id)
                | super::ShapeRecordEditTarget::RoleExpectedOutputs(id)
                | super::ShapeRecordEditTarget::RoleInputArtifacts(id)
                | super::ShapeRecordEditTarget::RoleSuccessCriteria(id)
                | super::ShapeRecordEditTarget::RoleEscalationRules(id)
                | super::ShapeRecordEditTarget::RoleExecutionLocation(id)
                | super::ShapeRecordEditTarget::RoleModelRoute(id)
                | super::ShapeRecordEditTarget::RoleBudget(id)
                if *id == contract_id
        )
    }) {
        let can_save_empty = matches!(
            target,
            super::ShapeRecordEditTarget::RoleCapabilities(_)
                | super::ShapeRecordEditTarget::RoleTools(_)
                | super::ShapeRecordEditTarget::RoleWriteScope(_)
                | super::ShapeRecordEditTarget::RolePermissions(_)
                | super::ShapeRecordEditTarget::RoleExpectedOutputs(_)
                | super::ShapeRecordEditTarget::RoleInputArtifacts(_)
                | super::ShapeRecordEditTarget::RoleSuccessCriteria(_)
                | super::ShapeRecordEditTarget::RoleEscalationRules(_)
                | super::ShapeRecordEditTarget::RoleModelRoute(_)
                | super::ShapeRecordEditTarget::RoleBudget(_)
        );
        panel = panel.with_child(record_edit_panel(
            target,
            record_editor,
            record_edit_has_text || can_save_empty,
            record_edit_pending,
            record_edit_error,
            workspace_view_id,
            theme,
            font,
        ));
    }
    Container::new(panel.finish())
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .with_background_color(theme.background)
        .with_border(Border::all(1.0).with_border_color(theme.accent))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

pub(crate) fn role_contract_field(
    title: &str,
    value: &str,
    target: super::ShapeRecordEditTarget,
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let theme = *theme;
    let title = title.to_string();
    let value = value.to_string();
    let field = Hoverable::new(
        super::execution_workspace::stable_mouse(format!("shape:field:{title}")),
        move |state| {
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(2.0)
                    .with_child(
                        Flex::row()
                            .with_child(label(&title, theme.disabled_text(), WI_CAPTION, font))
                            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                            .with_child(render_icon(AppIcon::Edit, theme.disabled_text(), 9.5))
                            .finish(),
                    )
                    .with_child(label(&value, theme.sub_text(), WI_BODY, font))
                    .finish(),
            )
            .with_uniform_padding(6.0)
            .with_background_color(if state.is_hovered() {
                theme.accent_overlay()
            } else {
                theme.surface_1()
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
    // Inline editor: opens right under the field being edited.
    if ui.target == Some(&target) {
        return Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(field)
            .with_child(record_edit_panel(
                target,
                ui.editor.clone(),
                ui.has_text,
                ui.pending,
                ui.error,
                workspace_view_id,
                &theme,
                font,
            ))
            .finish();
    }
    field
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn role_model_picker_field(
    selected_route: Option<&str>,
    model_routes: &[ModelRouteOption],
    open: bool,
    locked: bool,
    work_contract_id: dragent_workspace_model::WorkContractId,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    // Locked accounts keep models on Auto; agent CLIs stay selectable.
    let selected_route = if locked {
        selected_route.filter(|route| dragent_workspace_model::route_agent(route).is_some())
    } else {
        selected_route
    };
    let openable = !locked || !model_routes.is_empty();
    let selected_label = selected_route
        .and_then(|route| {
            model_routes
                .iter()
                .find(|model| model.route == route)
                .map(|model| model.label.as_str())
        })
        .or(selected_route)
        .unwrap_or("Auto")
        .to_string();
    let theme_copy = *theme;
    let trigger = Hoverable::new(
        super::execution_workspace::stable_mouse("shape:model-picker".to_string()),
        move |state| {
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(2.0)
                    .with_child(
                        Flex::row()
                            .with_child(label(
                                "Model",
                                theme_copy.disabled_text(),
                                WI_CAPTION,
                                font,
                            ))
                            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                            .with_child(render_icon(
                                if open {
                                    AppIcon::ChevronUp
                                } else {
                                    AppIcon::ChevronDown
                                },
                                theme_copy.disabled_text(),
                                9.5,
                            ))
                            .finish(),
                    )
                    .with_child(label(&selected_label, theme_copy.sub_text(), WI_BODY, font))
                    .finish(),
            )
            .with_uniform_padding(6.0)
            .with_background_color(if state.is_hovered() && openable {
                theme_copy.accent_overlay()
            } else {
                theme_copy.surface_1()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        },
    )
    .with_cursor(if openable {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_mouse_down(move |event_ctx, _, _| {
        if openable {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::ToggleShapeRoleModelPicker { work_contract_id },
            );
        }
    })
    .finish();

    let mut field = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0)
        .with_child(trigger);
    if locked && !open {
        field = field.with_child(label(
            if model_routes.is_empty() {
                "Auto — the server resolves the route for your account tier"
            } else {
                "Auto — the server resolves the model for your tier, or pick an agent CLI"
            },
            theme.disabled_text(),
            WI_CAPTION,
            font,
        ));
    } else if open && openable {
        field = field.with_child(model_route_row(
            "Auto",
            "Choose by capability, account policy, and availability",
            selected_route.is_none(),
            None,
            work_contract_id,
            workspace_view_id,
            theme,
            font,
        ));
        for model in model_routes {
            field = field.with_child(model_route_row(
                &model.label,
                &model.detail,
                selected_route == Some(model.route.as_str()),
                Some(model.route.clone()),
                work_contract_id,
                workspace_view_id,
                theme,
                font,
            ));
        }
    }
    field.finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn model_route_row(
    title: &str,
    detail: &str,
    selected: bool,
    model_route: Option<String>,
    work_contract_id: dragent_workspace_model::WorkContractId,
    workspace_view_id: EntityId,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let theme = *theme;
    let title = title.to_string();
    let detail = detail.to_string();
    Hoverable::new(
        super::execution_workspace::stable_mouse(format!(
            "shape:route:{work_contract_id:?}:{title}"
        )),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(
                        AppIcon::Check,
                        if selected {
                            theme.accent
                        } else {
                            ColorU::transparent_black()
                        },
                        10.0,
                    ))
                    .with_child(
                        Flex::column()
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_child(label(&title, theme.main_text(), WI_CAPTION, font))
                            .with_child(label(&detail, theme.disabled_text(), WI_CAPTION, font))
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if state.is_hovered() || selected {
                theme.accent_overlay()
            } else {
                theme.surface_1()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::SelectShapeRoleModel {
                work_contract_id,
                model_route: model_route.clone(),
            },
        );
    })
    .finish()
}
