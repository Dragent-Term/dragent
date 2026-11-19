//! Shape workspace plan surfaces.
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

pub(crate) fn shaping_surface(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    approve_mouse_state: MouseStateHandle,
    submit_mouse_state: MouseStateHandle,
    composer_editor: ViewHandle<InputEditor>,
    composer_has_text: bool,
    messages: &[ConversationMessage],
    proposal: Option<&ShapingProposal>,
    work_brief: Option<&WorkBrief>,
    records: Option<&ShapingRecords>,
    artifacts: &[ArtifactRef],
    plan: Option<&Plan>,
    proposed_roles: &[ProposedRoleView],
    edit_target: Option<&super::ShapeRecordEditTarget>,
    record_editor: ViewHandle<InputEditor>,
    record_edit_has_text: bool,
    record_edit_pending: bool,
    record_edit_error: Option<&str>,
    coordination_ready: bool,
    coordination_pending: bool,
    coordination_summary: &str,
    coordination_error: Option<&str>,
    principal_pending: bool,
    principal_errors: &[(usize, String)],
    has_active_work: bool,
    composer_attachments: &[crate::agent::attachments::LoadedAttachment],
    composer_notice: Option<&str>,
    voice_state: crate::voice::VoiceUiState,
    voice_spin: usize,
    model_menu_open: bool,
    models: &[super::work_item_surface::ShapeComposerModelRow],
    selected_model_label: &str,
    model_locked: bool,
    model_trigger_mouse: MouseStateHandle,
    model_row_mouse: impl Fn(&'static str) -> MouseStateHandle + Copy,
    shape_approved: bool,
    has_proposal: bool,
    shape_approve_mouse: MouseStateHandle,
    requirements_complete: bool,
    remove_armed: Option<&super::ShapeRecordEditTarget>,
    role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    role_action_error: Option<&str>,
    model_routes: &[ModelRouteOption],
    model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    model_picker_locked: bool,
) -> Box<dyn Element> {
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0);
    if has_active_work {
        for projected in messages {
            content = content.with_child(projected_message(projected, theme, font));
        }
        // Principal errors as conversation cards (terminal flow), never input text.
        // Every streamed error keeps its position: new turns and newer errors
        // append below, nothing is hidden or removed.
        for (_, error) in principal_errors {
            content = content.with_child(legacy_principal_error_card(error, theme, font));
        }
        if proposal.is_some() {
            content = content.with_child(render_editable_plan(
                theme,
                font,
                workspace_view_id,
                proposal,
                work_brief,
                records,
                artifacts,
                plan,
                proposed_roles,
                edit_target,
                record_editor,
                record_edit_has_text,
                record_edit_pending,
                record_edit_error,
                remove_armed,
                role_action_pending,
                role_action_error,
                model_routes,
                model_picker_contract,
                model_picker_locked,
            ));
        }
    } else {
        content = content.with_child(new_work_intro(theme, font));
    }
    let content = content
        .with_child(composer(
            theme,
            font,
            workspace_view_id,
            approve_mouse_state,
            submit_mouse_state,
            composer_editor,
            composer_has_text,
            coordination_ready,
            coordination_pending,
            coordination_summary,
            coordination_error,
            principal_pending,
            has_active_work,
            composer_attachments,
            composer_notice,
            voice_state,
            voice_spin,
            model_menu_open,
            models,
            selected_model_label,
            model_locked,
            model_trigger_mouse,
            model_row_mouse,
            shape_approved,
            has_proposal,
            shape_approve_mouse,
            requirements_complete,
        ))
        .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Expanded::new(
                    1.0,
                    Clipped::new(
                        Container::new(content)
                            .with_padding_left(28.0)
                            .with_padding_right(28.0)
                            .with_padding_top(14.0)
                            .with_padding_bottom(14.0)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_border(Border::right(1.0).with_border_color(theme.border()))
    .finish()
}

pub(crate) fn new_work_intro(theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.0)
            .with_child(label(
                "Shape a new outcome",
                theme.main_text(),
                17.0,
                font,
            ))
            .with_child(label(
                "Describe the result you want, the context that matters, and any constraints.",
                theme.sub_text(),
                11.0,
                font,
            ))
            .with_child(label(
                "Dragent will create one durable work item, then the Principal will turn the conversation into an editable brief and plan.",
                theme.disabled_text(),
                10.2,
                font,
            ))
            .finish(),
    )
    .with_padding_left(18.0)
    .with_padding_right(18.0)
    .with_padding_top(18.0)
    .with_padding_bottom(18.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish()
}

pub(crate) fn projected_message(
    message: &ConversationMessage,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let color = if message.principal {
        theme.accent
    } else {
        theme.link()
    };
    // Same terminal-agent markdown formatter so no raw markdown is shown.
    let body: Box<dyn Element> =
        crate::terminal::render::markdown::render_markdown(&message.body, *theme, font);
    let mut body_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(label(&message.actor, theme.main_text(), WI_SUBHEAD, font))
                .with_child(label(&message.time, theme.disabled_text(), WI_BODY, font))
                .finish(),
        )
        .with_child(body);
    // User rows show the signed-in user's avatar photo (initials fallback),
    // same as the terminal agent's user blocks.
    let leading: Box<dyn Element> = if message.principal {
        Container::new(render_icon(AppIcon::Agent, color, 15.0))
            .with_uniform_padding(7.0)
            .with_background_color(theme.accent_overlay())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(15.0)))
            .finish()
    } else {
        let name = message.display_name.trim();
        let name = if name.is_empty() {
            message.actor.as_str()
        } else {
            name
        };
        crate::workspace::share_sheet::avatar_sized(
            name,
            "",
            &message.avatar_url,
            theme,
            font,
            29.0,
        )
    };
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(11.0)
        .with_child(leading)
        .with_child(Expanded::new(1.0, body_col.finish()).finish())
        .finish()
}

pub(crate) fn wrap_message(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for source_line in text.lines() {
        let mut line = String::new();
        for word in source_line.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max_chars {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(crate) fn message(
    actor: &str,
    time: &str,
    lines: &[&str],
    principal: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let color = if principal {
        theme.accent
    } else {
        theme.link()
    };
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(9.0)
                .with_child(label(actor, theme.main_text(), WI_SUBHEAD, font))
                .with_child(label(time, theme.disabled_text(), WI_BODY, font))
                .finish(),
        );
    for line in lines {
        body = body.with_child(label(line, theme.sub_text(), WI_BODY, font));
    }
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(11.0)
        .with_child(
            Container::new(render_icon(
                if principal {
                    AppIcon::Agent
                } else {
                    AppIcon::User
                },
                color,
                15.0,
            ))
            .with_uniform_padding(7.0)
            .with_background_color(if principal {
                theme.accent_overlay()
            } else {
                theme.surface_2()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(15.0)))
            .finish(),
        )
        .with_child(Expanded::new(1.0, body.finish()).finish())
        .finish()
}

/// Canonical editable Work Brief and Plan projection shared by the Shape
/// arrangement and the editor-hosted work-item surface.
pub(super) fn render_editable_plan(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    proposal: Option<&ShapingProposal>,
    work_brief: Option<&WorkBrief>,
    records: Option<&ShapingRecords>,
    artifacts: &[ArtifactRef],
    plan: Option<&Plan>,
    proposed_roles: &[ProposedRoleView],
    edit_target: Option<&super::ShapeRecordEditTarget>,
    record_editor: ViewHandle<InputEditor>,
    record_edit_has_text: bool,
    record_edit_pending: bool,
    record_edit_error: Option<&str>,
    remove_armed: Option<&super::ShapeRecordEditTarget>,
    role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    role_action_error: Option<&str>,
    model_routes: &[ModelRouteOption],
    model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    model_picker_locked: bool,
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
    let Some(proposal) = proposal else {
        return Container::new(label(
            "Describe the outcome you want. The Principal will turn it into an editable plan.",
            theme.sub_text(),
            10.5,
            font,
        ))
        .with_uniform_padding(14.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish();
    };
    let (heading, goal, requirements, decisions, criteria, approach) = {
        let requirements = records.map_or_else(
            || owned_bullet_list(&proposal.requirements, theme, font),
            |records| {
                editable_record_list(
                    records.requirements.iter().map(|record| {
                        (
                            record.summary.clone(),
                            super::ShapeRecordEditTarget::Requirement(record.id),
                        )
                    }),
                    "•",
                    workspace_view_id,
                    ui,
                    theme,
                    font,
                )
            },
        );
        let decisions = records.map_or_else(
            || owned_decision_list(&proposal.open_decisions, theme, font),
            |records| {
                editable_decision_list(&records.decisions, workspace_view_id, ui, theme, font)
            },
        );
        let criteria = records.map_or_else(
            || owned_check_list(&proposal.acceptance_criteria, theme, font),
            |records| {
                editable_record_list(
                    records.acceptance_criteria.iter().map(|record| {
                        (
                            record.description.clone(),
                            super::ShapeRecordEditTarget::AcceptanceCriterion(record.id),
                        )
                    }),
                    "✓",
                    workspace_view_id,
                    ui,
                    theme,
                    font,
                )
            },
        );
        let goal_text = work_brief
            .map(|brief| brief.outcome.as_str())
            .unwrap_or(&proposal.outcome);
        let approach_text = work_brief
            .map(|brief| brief.summary.as_str())
            .unwrap_or(&proposal.plan_summary);
        (
            work_brief.map_or_else(
                || format!("Proposed plan · v{}", proposal.revision),
                |brief| format!("Work brief · v{}", brief.version),
            ),
            editable_record_list(
                [(
                    goal_text.to_string(),
                    super::ShapeRecordEditTarget::WorkBriefOutcome,
                )],
                "◆",
                workspace_view_id,
                ui,
                theme,
                font,
            ),
            requirements,
            decisions,
            criteria,
            Some(editable_record_list(
                [(
                    approach_text.to_string(),
                    super::ShapeRecordEditTarget::WorkBriefSummary,
                )],
                "◆",
                workspace_view_id,
                ui,
                theme,
                font,
            )),
        )
    };

    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(label(&heading, theme.main_text(), WI_SUBHEAD, font))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(label(
                        "Select an item to edit",
                        theme.disabled_text(),
                        WI_CAPTION,
                        font,
                    ))
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(8.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_border(Border::bottom(1.0).with_border_color(theme.border()))
            .finish(),
        )
        .with_child(plan_row("Goal", goal, theme, font));
    // NOTE: no shared top-placed editor panel — each editable row renders
    // its own panel inline (see editable_record_list / plan_task_row /
    // with_add_control / role_contract_field), exactly where clicked.
    rows = rows
        .with_child(plan_row(
            "Requirements",
            with_add_control(
                requirements,
                "+ Add requirement",
                super::ShapeRecordEditTarget::NewRequirement,
                workspace_view_id,
                ui,
                theme,
                font,
            ),
            theme,
            font,
        ))
        .with_child(plan_row(
            "Open decisions",
            with_add_control(
                decisions,
                "+ Add decision",
                super::ShapeRecordEditTarget::NewDecision,
                workspace_view_id,
                ui,
                theme,
                font,
            ),
            theme,
            font,
        ))
        .with_child(plan_row(
            "Acceptance criteria",
            with_add_control(
                criteria,
                "+ Add criterion",
                super::ShapeRecordEditTarget::NewAcceptanceCriterion,
                workspace_view_id,
                ui,
                theme,
                font,
            ),
            theme,
            font,
        ))
        .with_child(plan_row(
            "Repository context",
            with_add_control(
                editable_record_list(
                    artifacts.iter().map(|artifact| {
                        (
                            format!(
                                "{} · {:?}",
                                artifact.relative_path.to_string_lossy(),
                                artifact.kind
                            ),
                            super::ShapeRecordEditTarget::Artifact(artifact.id),
                        )
                    }),
                    "↗",
                    workspace_view_id,
                    ui,
                    theme,
                    font,
                ),
                "+ Add artifact",
                super::ShapeRecordEditTarget::NewArtifact,
                workspace_view_id,
                ui,
                theme,
                font,
            ),
            theme,
            font,
        ));
    if let Some(approach) = approach {
        rows = rows.with_child(plan_row("Approach", approach, theme, font));
    }
    // A role is the primary unit here, not a task: each role card owns its
    // mission, its one model picker, and its own nested tasks — never the
    // task showing (or editing) a model of its own.
    rows = rows.with_child(plan_row(
        &format!(
            "Team & tasks · {} role{}",
            proposed_roles.len(),
            if proposed_roles.len() == 1 { "" } else { "s" }
        ),
        plan_team_and_tasks_section(
            plan,
            proposed_roles,
            model_routes,
            model_picker_contract,
            model_picker_locked,
            role_action_pending,
            role_action_error,
            workspace_view_id,
            ui,
            theme,
            font,
        ),
        theme,
        font,
    ));

    Container::new(rows.finish())
        .with_background_color(theme.sidebar_background())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
}

pub(crate) fn owned_text_lines(
    lines: &[String],
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.0);
    for line in lines {
        body = body.with_child(label(line, theme.sub_text(), WI_BODY, font));
    }
    body.finish()
}

pub(crate) fn owned_bullet_list(
    lines: &[String],
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.0);
    for line in lines {
        list = list.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(label("•", theme.sub_text(), WI_BODY, font))
                .with_child(label(line, theme.sub_text(), WI_BODY, font))
                .finish(),
        );
    }
    list.finish()
}

pub(crate) fn owned_decision_list(
    lines: &[String],
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    if lines.is_empty() {
        return label("No open decisions", theme.disabled_text(), WI_BODY, font);
    }
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0);
    for (index, decision) in lines.iter().enumerate() {
        list = list.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(label(
                    &format!("{}.", index + 1),
                    theme.sub_text(),
                    10.0,
                    font,
                ))
                .with_child(label(decision, theme.sub_text(), WI_BODY, font))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(status_badge(
                    "Needs decision",
                    theme.ui_yellow(),
                    theme,
                    font,
                ))
                .finish(),
        );
    }
    list.finish()
}

pub(crate) fn owned_check_list(
    lines: &[String],
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(3.0);
    for line in lines {
        list = list.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(render_icon(AppIcon::Check, theme.ui_green(), 11.0))
                .with_child(label(line, theme.sub_text(), WI_BODY, font))
                .finish(),
        );
    }
    list.finish()
}

/// Team-and-tasks section for the Plan view: a role is the primary unit, not
/// a task. Every proposed role contract renders as a card — approval state
/// and inline Approve button, mission, the role's own model picker (one per
/// role, not duplicated per task) — with that role's own tasks nested
/// directly under it. Tasks with no role (or a stale/removed one) fall into
/// a trailing "Unassigned" group. Reuses the same ApproveProposedRole
/// authority path (editing a contract still resets it to Proposed — only
/// Approve approves).
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_team_and_tasks_section(
    plan: Option<&Plan>,
    proposed_roles: &[ProposedRoleView],
    model_routes: &[ModelRouteOption],
    model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    model_picker_locked: bool,
    role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    role_action_error: Option<&str>,
    workspace_view_id: EntityId,
    ui: &RecordEditUi<'_>,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    use dragent_workspace_model::WorkContractStatus;
    let total_tasks = plan.map_or(0, |plan| plan.tasks.len());
    let mut remaining: Vec<(usize, &dragent_workspace_model::PlanTask)> = plan
        .map(|plan| plan.tasks.iter().enumerate().collect())
        .unwrap_or_default();
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0);
    if proposed_roles.is_empty() {
        list = list.with_child(label(
            "No proposed team yet — ask the Principal to propose one.",
            theme.disabled_text(),
            10.1,
            font,
        ));
    }
    for role in proposed_roles {
        let contract_id = role.work_contract_id;
        let pending = role_action_pending == Some(contract_id);
        // The Approve button only exists when approval is actually possible:
        // a visible Approve button is always clickable (except while its own
        // approval is in flight). Every other status gets a truthful badge
        // and no button — never a dead disabled Approve.
        let approvable = matches!(
            role.status,
            WorkContractStatus::Draft | WorkContractStatus::Proposed
        );
        let (badge_text, badge_color) = match role.status {
            WorkContractStatus::Approved => ("Approved", theme.ui_green()),
            WorkContractStatus::Active => ("Active", theme.accent),
            WorkContractStatus::Completed => ("Completed", theme.ui_green()),
            WorkContractStatus::Revoked => ("Revoked", theme.ui_red()),
            _ if pending => ("Approving…", theme.ui_yellow()),
            _ => ("Needs approval", theme.ui_yellow()),
        };
        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(label(&role.name, theme.main_text(), WI_SUBHEAD, font))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(status_badge(badge_text, badge_color, theme, font));
        if approvable {
            header = header.with_child(record_edit_button(
                "Approve",
                WorkspaceAction::ApproveProposedRole {
                    work_contract_id: contract_id,
                },
                !pending,
                true,
                workspace_view_id,
                theme,
                font,
            ));
        }
        let (matching, rest) = remaining
            .into_iter()
            .partition::<Vec<_>, _>(|(_, task)| task.assigned_contract_id == Some(contract_id));
        remaining = rest;
        let mut card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(3.0)
            .with_child(header.finish())
            .with_child(label(&role.mission, theme.sub_text(), WI_BODY, font))
            .with_child(role_model_picker_field(
                role.preferred_model_route.as_deref(),
                model_routes,
                model_picker_contract == Some(contract_id),
                model_picker_locked,
                contract_id,
                workspace_view_id,
                theme,
                font,
            ));
        if !matching.is_empty() {
            let mut tasks = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(3.0);
            for (index, task) in matching {
                tasks = tasks.with_child(plan_task_row(
                    task,
                    index,
                    total_tasks,
                    workspace_view_id,
                    ui,
                    theme,
                    font,
                ));
            }
            card = card.with_child(
                Container::new(tasks.finish())
                    .with_padding_top(4.0)
                    .finish(),
            );
        }
        list = list.with_child(
            Container::new(card.finish())
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish(),
        );
    }
    if !remaining.is_empty() {
        let mut tasks = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(3.0)
            .with_child(label("Unassigned", theme.disabled_text(), WI_SMALL, font));
        for (index, task) in remaining {
            tasks = tasks.with_child(plan_task_row(
                task,
                index,
                total_tasks,
                workspace_view_id,
                ui,
                theme,
                font,
            ));
        }
        list = list.with_child(
            Container::new(tasks.finish())
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_background_color(theme.surface_1())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish(),
        );
    }
    if let Some(error) = role_action_error {
        list = list.with_child(label(error, theme.ui_red(), WI_SMALL, font));
    }
    if plan.is_some() {
        list = list.with_child(
            Container::new(record_edit_button(
                "+ Add task",
                WorkspaceAction::StartShapeRecordEdit {
                    target: super::ShapeRecordEditTarget::NewPlanTask,
                },
                !ui.pending,
                false,
                workspace_view_id,
                theme,
                font,
            ))
            .with_padding_top(5.0)
            .finish(),
        );
        // Inline editor for "+ Add task": opens right under its own button.
        if ui.target == Some(&super::ShapeRecordEditTarget::NewPlanTask) {
            list = list.with_child(record_edit_panel(
                super::ShapeRecordEditTarget::NewPlanTask,
                ui.editor.clone(),
                ui.has_text,
                ui.pending,
                ui.error,
                workspace_view_id,
                theme,
                font,
            ));
        }
    }
    list.finish()
}
