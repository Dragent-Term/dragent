//! Shape workspace navigation and activity views.
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

pub(crate) const NAV_WIDTH: f32 = 218.0;
pub(crate) const ACTIVITY_HEIGHT: f32 = 204.0;
pub(crate) const MINIMUM_MAIN_PANE_WIDTH: f32 = 1022.0;
// The conversation, complete proposed plan, composer, and team cards require a
// taller upper region than the original 760px prototype allowed. Keeping
// the full main canvas at this content-safe minimum makes the outer viewport
// scroll before either column can paint through the Activity dock.
pub(crate) const MINIMUM_SCROLLING_CONTENT_HEIGHT: f32 = 1180.0;

#[derive(Clone, Debug)]
pub(crate) struct ConversationMessage {
    pub actor: String,
    pub time: String,
    pub body: String,
    pub principal: bool,
    /// Signed-in user's display name for the avatar/initials fallback.
    pub display_name: String,
    /// Signed-in user's photo URL — rendered via `avatar_sized`, same as the
    /// terminal agent's user blocks (initials circle while loading/failing).
    pub avatar_url: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProposedRoleView {
    pub work_contract_id: dragent_workspace_model::WorkContractId,
    pub version: u64,
    pub kind: dragent_workspace_model::AgentRoleKind,
    pub name: String,
    pub mission: String,
    pub rationale: String,
    pub capabilities: Vec<String>,
    pub tools: Vec<String>,
    pub write_scope: Vec<String>,
    pub approval_required: bool,
    pub permissions: Vec<dragent_workspace_model::PermissionGrant>,
    pub execution_location: ExecutionLocation,
    pub preferred_model_route: Option<String>,
    pub status: dragent_workspace_model::WorkContractStatus,
    pub maximum_input_tokens: Option<u64>,
    pub maximum_output_tokens: Option<u64>,
    pub maximum_cost_micros: Option<u64>,
    pub maximum_duration_ms: Option<u64>,
    pub maximum_tool_calls: Option<u64>,
    pub expected_outputs: Vec<String>,
    pub input_artifacts: Vec<String>,
    pub success_criteria: Vec<String>,
    pub escalation_rules: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelRouteOption {
    pub route: String,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ActivityTone {
    User,
    Principal,
    Work,
    Team,
}

#[derive(Clone, Debug)]
pub(crate) struct ActivityItem {
    pub time: String,
    pub actor: String,
    pub summary: String,
    pub kind: String,
    pub tone: ActivityTone,
}

pub fn render(
    theme: &Theme,
    font: FamilyId,
    viewport_width: f32,
    viewport_height: f32,
    horizontal_scroll: ClippedScrollStateHandle,
    vertical_scroll: ClippedScrollStateHandle,
    workspace_view_id: EntityId,
    approve_mouse_state: MouseStateHandle,
    submit_mouse_state: MouseStateHandle,
    describe_role_mouse_state: MouseStateHandle,
    composer_editor: ViewHandle<InputEditor>,
    composer_has_text: bool,
    messages: &[ConversationMessage],
    proposal: Option<&ShapingProposal>,
    work_brief: Option<&WorkBrief>,
    records: Option<&ShapingRecords>,
    artifacts: &[ArtifactRef],
    plan: Option<&Plan>,
    proposed_roles: &[ProposedRoleView],
    model_routes: &[ModelRouteOption],
    model_picker_contract: Option<dragent_workspace_model::WorkContractId>,
    model_picker_locked: bool,
    activity: &[ActivityItem],
    selected_role: Option<dragent_workspace_model::WorkContractId>,
    role_action_pending: Option<dragent_workspace_model::WorkContractId>,
    role_action_error: Option<&str>,
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
    work_title: Option<&str>,
    work_status: dragent_workspace_model::WorkStatus,
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
) -> Box<dyn Element> {
    let has_active_work = work_title.is_some();
    let primary_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Expanded::new(
                1.0,
                shaping_surface(
                    &theme,
                    font,
                    workspace_view_id,
                    approve_mouse_state,
                    submit_mouse_state,
                    composer_editor,
                    composer_has_text,
                    messages,
                    proposal,
                    work_brief,
                    records,
                    artifacts,
                    plan,
                    proposed_roles,
                    edit_target,
                    record_editor.clone(),
                    record_edit_has_text,
                    record_edit_pending,
                    record_edit_error,
                    coordination_ready,
                    coordination_pending,
                    coordination_summary,
                    coordination_error,
                    principal_pending,
                    principal_errors,
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
                    remove_armed,
                    role_action_pending,
                    role_action_error,
                    model_routes,
                    model_picker_contract,
                    model_picker_locked,
                ),
            )
            .finish(),
        )
        // Preserve the visual's conversation/team relationship on the full
        // canvas. Narrow windows reach the remainder by scrolling.
        .with_child(
            Expanded::new(
                0.64,
                proposed_team(
                    theme,
                    font,
                    proposed_roles,
                    model_routes,
                    model_picker_contract,
                    model_picker_locked,
                    workspace_view_id,
                    describe_role_mouse_state,
                    selected_role,
                    role_action_pending,
                    role_action_error,
                    edit_target,
                    record_editor.clone(),
                    record_edit_has_text,
                    record_edit_pending,
                    record_edit_error,
                    remove_armed,
                ),
            )
            .finish(),
        );

    let scrolling_content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(super::work_journey::render(
            dragent_workspace_model::BuiltInLayout::Shape,
            work_status,
            theme,
            font,
        ))
        .with_child(Expanded::new(1.0, primary_row.finish()).finish())
        .finish();

    let main_pane = super::work_canvas::render(
        theme,
        horizontal_scroll,
        vertical_scroll,
        (viewport_width - NAV_WIDTH).max(1.0),
        (viewport_height - ACTIVITY_HEIGHT).max(1.0),
        MINIMUM_MAIN_PANE_WIDTH,
        Some(MINIMUM_SCROLLING_CONTENT_HEIGHT),
        scrolling_content,
    );

    // Activity is workspace chrome: it remains observable while the shaping
    // conversation and plan move beneath it in their own viewport.
    let workspace = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1.0, main_pane).finish())
        .with_child(
            ConstrainedBox::new(activity_dock(theme, font, activity))
                .with_height(ACTIVITY_HEIGHT)
                .finish(),
        )
        .finish();

    let root = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            ConstrainedBox::new(work_navigation(theme, font, work_title))
                .with_width(NAV_WIDTH)
                .finish(),
        )
        .with_child(Expanded::new(1.0, workspace).finish());

    Container::new(root.finish())
        .with_background_color(theme.pane_surface())
        .finish()
}

pub(super) fn work_navigation(
    theme: &Theme,
    font: FamilyId,
    work_title: Option<&str>,
) -> Box<dyn Element> {
    let mut work_items = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0)
        .with_child(nav_heading("WORK", theme, font))
        .with_child(nav_row("Work", AppIcon::Agent, true, theme, font));
    work_items = work_items.with_child(work_item_row(
        work_title.unwrap_or("New work item"),
        true,
        theme,
        font,
    ));
    let work_items = work_items.finish();

    let knowledge = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0)
        .with_child(nav_heading("KNOWLEDGE", theme, font))
        .with_child(nav_row("Documents", AppIcon::File, false, theme, font))
        .with_child(nav_row("Changes", AppIcon::Code, false, theme, font))
        .with_child(nav_row("Artifacts", AppIcon::Grid, false, theme, font))
        .with_child(nav_row("Memory", AppIcon::History, false, theme, font))
        .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(render_icon(AppIcon::Folder, theme.main_text(), 14.0))
                        .with_child(
                            Container::new(label(
                                "Acme Platform",
                                theme.main_text(),
                                WI_SUBHEAD,
                                font,
                            ))
                            .with_padding_left(8.0)
                            .finish(),
                        )
                        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                        .with_child(render_icon(
                            AppIcon::ChevronDown,
                            theme.disabled_text(),
                            11.0,
                        ))
                        .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(13.0)
                .with_padding_bottom(13.0)
                .with_border(Border::bottom(1.0).with_border_color(theme.border()))
                .finish(),
            )
            .with_child(
                Container::new(work_items)
                    .with_padding_left(9.0)
                    .with_padding_right(9.0)
                    .with_padding_top(12.0)
                    .finish(),
            )
            .with_child(
                Container::new(knowledge)
                    .with_padding_left(9.0)
                    .with_padding_right(9.0)
                    .with_padding_top(18.0)
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(9.0)
                        .with_child(render_icon(AppIcon::Settings, theme.sub_text(), 14.0))
                        .with_child(label("Settings", theme.sub_text(), WI_BODY, font))
                        .finish(),
                )
                .with_padding_left(13.0)
                .with_padding_top(11.0)
                .with_padding_bottom(11.0)
                .with_border(Border::top(1.0).with_border_color(theme.border()))
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .with_border(Border::right(1.0).with_border_color(theme.border()))
    .finish()
}

pub(crate) fn nav_heading(text: &str, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    Container::new(label(text, theme.disabled_text(), WI_CAPTION, font))
        .with_padding_left(7.0)
        .with_padding_bottom(4.0)
        .finish()
}

pub(crate) fn nav_row(
    text: &str,
    icon: AppIcon,
    active: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(
                icon,
                if active {
                    theme.accent
                } else {
                    theme.sub_text()
                },
                14.0,
            ))
            .with_child(label(
                text,
                if active {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                11.2,
                font,
            ))
            .finish(),
    )
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(7.0)
    .with_padding_bottom(7.0)
    .with_background_color(if active {
        theme.surface_2()
    } else {
        ColorU::transparent_black()
    })
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .finish()
}

pub(crate) fn work_item_row(
    text: &str,
    active: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(Empty::new().finish())
                    .with_uniform_padding(2.0)
                    .with_background_color(if active {
                        theme.accent
                    } else {
                        theme.disabled_text()
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                    .finish(),
            )
            .with_child(
                Container::new(label(
                    text,
                    if active {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    },
                    WI_BUTTON,
                    font,
                ))
                .with_padding_left(9.0)
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(26.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_background_color(if active {
        theme.accent_overlay()
    } else {
        ColorU::transparent_black()
    })
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .finish()
}

pub(crate) fn activity_dock(
    theme: &Theme,
    font: FamilyId,
    activity: &[ActivityItem],
) -> Box<dyn Element> {
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0);
    if activity.is_empty() {
        rows = rows.with_child(label(
            "Canonical work activity will appear here as the work is shaped.",
            theme.disabled_text(),
            10.2,
            font,
        ));
    }
    for item in activity {
        let color = match item.tone {
            ActivityTone::User => theme.link(),
            ActivityTone::Principal => theme.accent,
            ActivityTone::Work => theme.ansi_blue(),
            ActivityTone::Team => theme.ui_green(),
        };
        rows = rows.with_child(activity_row(
            &item.time,
            &item.actor,
            &item.summary,
            &item.kind,
            color,
            theme,
            font,
        ));
    }
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(24.0)
                        .with_child(activity_tab("Activity", true, theme, font))
                        .with_child(activity_tab("Terminal", false, theme, font))
                        .with_child(activity_tab("Tests", false, theme, font))
                        .with_child(activity_tab("Attention", false, theme, font))
                        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                        .with_child(label("Clear", theme.sub_text(), WI_BUTTON, font))
                        .with_child(outline_button("All", AppIcon::ChevronDown, theme, font))
                        .finish(),
                )
                .with_padding_left(18.0)
                .with_padding_right(12.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .with_border(Border::bottom(1.0).with_border_color(theme.border()))
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Clipped::new(
                        Container::new(rows.finish())
                            .with_padding_left(18.0)
                            .with_padding_right(18.0)
                            .with_padding_top(9.0)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .with_border(Border::top(1.0).with_border_color(theme.border()))
    .finish()
}

pub(crate) fn activity_tab(
    text: &str,
    active: bool,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(label(
        text,
        if active {
            theme.main_text()
        } else {
            theme.sub_text()
        },
        WI_BUTTON,
        font,
    ))
    .with_padding_bottom(5.0)
    .with_border(Border::bottom(if active { 2.0 } else { 0.0 }).with_border_color(theme.accent))
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn activity_row(
    time: &str,
    actor: &str,
    event: &str,
    kind: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(9.0)
        .with_child(
            ConstrainedBox::new(label(time, theme.disabled_text(), WI_SMALL, font))
                .with_width(64.0)
                .finish(),
        )
        .with_child(
            Container::new(Empty::new().finish())
                .with_uniform_padding(2.0)
                .with_background_color(theme.disabled_text())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                .finish(),
        )
        .with_child(render_icon(AppIcon::Agent, color, 11.0))
        .with_child(
            ConstrainedBox::new(label(actor, color, WI_BODY, font))
                .with_width(116.0)
                .finish(),
        )
        .with_child(label(event, theme.sub_text(), WI_BODY, font))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(status_badge(kind, color, theme, font))
        .with_child(render_icon(
            AppIcon::ChevronRight,
            theme.disabled_text(),
            10.0,
        ))
        .finish()
}

pub(crate) fn outline_button(
    text: &str,
    icon: AppIcon,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(icon, theme.sub_text(), 11.0))
            .with_child(label(text, theme.sub_text(), WI_BODY, font))
            .finish(),
    )
    .with_padding_left(7.0)
    .with_padding_right(7.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn action_outline_button(
    text: &str,
    icon: AppIcon,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    let theme = *theme;
    let text = text.to_string();
    Hoverable::new(mouse_state, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(icon, theme.sub_text(), 11.0))
                .with_child(label(&text, theme.sub_text(), WI_BODY, font))
                .finish(),
        )
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(if state.is_hovered() {
            theme.surface_2()
        } else {
            theme.surface_1()
        })
        .with_border(Border::all(1.0).with_border_color(if state.is_hovered() {
            theme.accent
        } else {
            theme.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

pub(crate) fn approve_and_coordinate_button(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    enabled: bool,
    pending: bool,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse_state, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(label(
                    if pending {
                        "Preparing…"
                    } else {
                        "Start Coordination"
                    },
                    if enabled {
                        theme.accent_text()
                    } else {
                        theme.disabled_text()
                    },
                    10.7,
                    font,
                ))
                .with_child(render_icon(
                    AppIcon::ArrowRight,
                    if enabled {
                        theme.accent_text()
                    } else {
                        theme.disabled_text()
                    },
                    12.0,
                ))
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(10.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(if enabled && state.is_hovered() {
            theme.link()
        } else if enabled {
            theme.accent
        } else {
            theme.surface_2()
        })
        .with_border(Border::all(1.0).with_border_color(if enabled {
            theme.accent
        } else {
            theme.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(if enabled {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |event_ctx, _, _| {
        if enabled {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::ApprovePlanAndCoordinate,
            );
        }
    })
    .finish()
}

pub(crate) fn status_badge(
    text: &str,
    color: ColorU,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    Container::new(label(text, color, WI_CAPTION, font))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(color))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
}

pub(crate) fn label(text: &str, color: ColorU, size: f32, font: FamilyId) -> Box<dyn Element> {
    Text::new(text.to_string(), font, crate::theme::text_size(size))
        .with_color(color)
        .finish()
}
