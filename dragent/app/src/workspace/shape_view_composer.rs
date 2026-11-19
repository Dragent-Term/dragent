//! Shape workspace composer views.
//!
//! Moved verbatim from shape_workspace; same behavior.

//! Deterministic Milestone-1 projection for the Shape workspace.
//!
//! Work, conversation,
//! shaping records, and proposed roles are projected from canonical workspace
//! events. Draft role contracts remain distinct from runtime agents and cannot
//! launch execution from this surface.

use super::shape_view_edit::*;
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

pub(crate) fn composer(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    approve_mouse_state: MouseStateHandle,
    _submit_mouse_state: MouseStateHandle,
    composer_editor: ViewHandle<InputEditor>,
    composer_has_text: bool,
    coordination_ready: bool,
    coordination_pending: bool,
    coordination_summary: &str,
    _coordination_error: Option<&str>,
    principal_pending: bool,
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
) -> Box<dyn Element> {
    // Terminal agent parity: same InputEditor, same bottom controls
    // ([+] attach, model picker, mic, salmon send/stop). No branch chip/div.
    let busy = principal_pending;
    let has_text = composer_has_text || !composer_attachments.is_empty();
    let top_divider = Container::new(Empty::new().finish())
        .with_background_color(theme.pane_surface())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish();
    let mut content = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    content = content.with_child(top_divider);

    if !composer_attachments.is_empty() {
        let mut attachments = warpui::elements::Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_run_spacing(6.0);
        for (index, attachment) in composer_attachments.iter().enumerate() {
            let chip = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(render_icon(AppIcon::File, theme.accent, 12.0))
                    .with_child(label(attachment.name(), theme.main_text(), WI_BODY, font))
                    .with_child(render_icon(AppIcon::XClose, theme.disabled_text(), 10.0))
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(7.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish();
            attachments = attachments.with_child(
                EventHandler::new(chip)
                    .on_left_mouse_down(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::RemoveShapeAttachment { index },
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
        }
        content = content.with_child(attachments.finish());
    }

    // Growing editor row — no fixed height, same as the terminal's editor_row.
    content = content.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(Box::new(Expanded::new(
                1.0,
                ChildView::new(&composer_editor).finish(),
            )))
            .finish(),
    );
    if busy {
        // Turn/loading status never echoes notice text inside the input.
        content = content.with_child(label(
            "Principal is updating the work plan…",
            theme.accent,
            10.0,
            font,
        ));
    } else if let Some(notice) = composer_notice {
        content = content.with_child(label(notice, theme.sub_text(), WI_BODY, font));
    }
    // Principal errors render as conversation cards (terminal flow), never here.
    content = content.with_child(label(
        coordination_summary,
        if coordination_ready {
            theme.ui_green()
        } else {
            theme.disabled_text()
        },
        9.7,
        font,
    ));

    if model_menu_open && !model_locked {
        const ROW_KEYS: [&str; 32] = [
            "shape-legacy-model-row-00",
            "shape-legacy-model-row-01",
            "shape-legacy-model-row-02",
            "shape-legacy-model-row-03",
            "shape-legacy-model-row-04",
            "shape-legacy-model-row-05",
            "shape-legacy-model-row-06",
            "shape-legacy-model-row-07",
            "shape-legacy-model-row-08",
            "shape-legacy-model-row-09",
            "shape-legacy-model-row-10",
            "shape-legacy-model-row-11",
            "shape-legacy-model-row-12",
            "shape-legacy-model-row-13",
            "shape-legacy-model-row-14",
            "shape-legacy-model-row-15",
            "shape-legacy-model-row-16",
            "shape-legacy-model-row-17",
            "shape-legacy-model-row-18",
            "shape-legacy-model-row-19",
            "shape-legacy-model-row-20",
            "shape-legacy-model-row-21",
            "shape-legacy-model-row-22",
            "shape-legacy-model-row-23",
            "shape-legacy-model-row-24",
            "shape-legacy-model-row-25",
            "shape-legacy-model-row-26",
            "shape-legacy-model-row-27",
            "shape-legacy-model-row-28",
            "shape-legacy-model-row-29",
            "shape-legacy-model-row-30",
            "shape-legacy-model-row-31",
        ];
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0);
        for (index, row) in models.iter().enumerate() {
            let row_id = row.id.clone();
            let row_effort = row.effort_wire.clone();
            let row_label = row.label.clone();
            let selected = row.selected;
            let theme_owned = *theme;
            let key: &'static str = ROW_KEYS[index % ROW_KEYS.len()];
            let el = Hoverable::new(model_row_mouse(key), move |state| {
                let hovered = state.is_hovered();
                let bg = if selected {
                    ColorU::new(
                        theme_owned.accent.r,
                        theme_owned.accent.g,
                        theme_owned.accent.b,
                        30,
                    )
                } else if hovered {
                    theme_owned.surface_3()
                } else {
                    theme_owned.surface_2()
                };
                let check: Box<dyn Element> = if selected {
                    render_icon(AppIcon::Check, theme_owned.accent, 12.0)
                } else {
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(12.0)
                        .with_height(12.0)
                        .finish()
                };
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(check)
                        .with_child(
                            Text::new(row_label.clone(), font, crate::theme::text_size(WI_SUBHEAD))
                                .with_color(if selected {
                                    theme_owned.accent
                                } else {
                                    theme_owned.main_text()
                                })
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .finish();
            list = list.with_child(
                EventHandler::new(el)
                    .on_left_mouse_down(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::SelectShapeModel {
                                model_id: row_id.clone(),
                                effort: row_effort.clone(),
                            },
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            );
        }
        content = content.with_child(
            Container::new(
                ConstrainedBox::new(
                    ClippedScrollable::vertical(
                        ClippedScrollStateHandle::new(),
                        list.finish(),
                        ScrollbarWidth::Auto,
                        theme.outline().into(),
                        theme.sub_text().into(),
                        theme.surface_1().into(),
                    )
                    .with_overlayed_scrollbar()
                    .finish(),
                )
                .with_max_height(240.0)
                .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish(),
        );
    }

    let mut controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(shape_legacy_icon_button(
            AppIcon::Plus,
            if composer_attachments.is_empty() {
                theme.sub_text()
            } else {
                theme.accent
            },
            WorkspaceAction::OpenShapeAttachmentPicker,
            workspace_view_id,
        ))
        .with_child(shape_legacy_model_trigger(
            theme,
            font,
            workspace_view_id,
            model_trigger_mouse,
            model_menu_open,
            model_locked,
            selected_model_label,
        ));
    controls = controls.with_child(Box::new(Expanded::new(1.0, Empty::new().finish())));
    if crate::voice::supported() {
        let (icon_color, border_color) = match voice_state {
            crate::voice::VoiceUiState::Idle => (theme.sub_text(), None),
            crate::voice::VoiceUiState::Connecting => (theme.accent, Some(theme.accent)),
            crate::voice::VoiceUiState::Recording => (theme.ui_red(), Some(theme.ui_red())),
        };
        let glyph = if voice_state == crate::voice::VoiceUiState::Connecting {
            crate::icons::render_spinner(voice_spin, icon_color, 14.0)
        } else {
            render_icon(AppIcon::Microphone, icon_color, 14.0)
        };
        let mut mic = Container::new(glyph)
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
        if let Some(bc) = border_color {
            mic = mic.with_border(Border::all(1.0).with_border_color(bc));
        }
        controls = controls.with_child(
            EventHandler::new(mic.finish())
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::ToggleShapeVoiceInput,
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish(),
        );
    }
    let salmon = theme.accent;
    let glyph_dark = theme.background;
    let (inner, action): (Box<dyn Element>, WorkspaceAction) = if busy {
        let square = Container::new(Empty::new().finish())
            .with_background_color(glyph_dark)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
            .finish();
        (
            ConstrainedBox::new(square)
                .with_width(12.0)
                .with_height(12.0)
                .finish(),
            WorkspaceAction::StopPrincipalTurn,
        )
    } else {
        (
            render_icon(AppIcon::ArrowUp, glyph_dark, 16.0),
            WorkspaceAction::SubmitShapeMessage,
        )
    };
    let bg = if busy || has_text {
        salmon
    } else {
        ColorU {
            r: salmon.r,
            g: salmon.g,
            b: salmon.b,
            a: 110,
        }
    };
    controls = controls.with_child(
        Container::new(
            EventHandler::new(
                Container::new(
                    Container::new(inner)
                        .with_padding_left(7.5)
                        .with_padding_right(7.5)
                        .with_padding_top(7.5)
                        .with_padding_bottom(7.5)
                        .finish(),
                )
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            )
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish(),
        )
        .with_padding_left(4.0)
        .finish(),
    );
    content = content.with_child(
        Container::new(controls.finish())
            .with_padding_top(6.0)
            .finish(),
    );
    content = content.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(approve_and_coordinate_button(
                theme,
                font,
                workspace_view_id,
                approve_mouse_state,
                coordination_ready && !coordination_pending,
                coordination_pending,
            ))
            .finish(),
    );
    // Work Item Flow step 3 gate: explicit Approve Shape consent. Shown
    // until approved or already shaped; hidden while a turn is running.
    if !shape_approved && !has_proposal && !busy {
        content = content.with_child(legacy_approve_shape_row(
            theme,
            font,
            workspace_view_id,
            shape_approve_mouse,
            requirements_complete,
        ));
    }
    let content = Container::new(content.finish())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(4.0)
        .with_background_color(theme.pane_surface())
        .finish();
    super::composer_input::file_drop_target(content, workspace_view_id, |paths| {
        WorkspaceAction::AttachShapeFiles { paths }
    })
}

/// Work Item Flow step 3 gate for the legacy Shape surface: same Approve
/// Shape consent as the work-item conversation. Disabled until the
/// conversation has something to shape from.
pub(crate) fn legacy_approve_shape_row(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    approve_ms: MouseStateHandle,
    requirements_complete: bool,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let enabled = requirements_complete;
    let button = Hoverable::new(approve_ms, move |state| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(label(
                    "Approve Shape",
                    if enabled {
                        theme_owned.accent_text()
                    } else {
                        theme_owned.disabled_text()
                    },
                    10.4,
                    font,
                ))
                .with_child(render_icon(
                    AppIcon::Check,
                    if enabled {
                        theme_owned.accent_text()
                    } else {
                        theme_owned.disabled_text()
                    },
                    11.0,
                ))
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
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
            event_ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ApproveShape);
        }
    })
    .finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(label(
                "Approve Shape unlocks on confirmation",
                theme.sub_text(),
                WI_SMALL,
                font,
            ))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(button)
            .finish(),
    )
    .with_padding_top(8.0)
    .finish()
}

/// Principal error as a terminal-style Agent error card for the legacy Shape
/// surface — same flow as the terminal agent, never input-area text.
pub(crate) fn legacy_principal_error_card(
    error: &str,
    theme: &Theme,
    font: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.ui_red();
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(accent)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();
    let header_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(
            Text::new("Agent · Error".to_string(), font, crate::theme::text_size(WI_HEADING))
                .with_color(accent)
                .finish(),
        )
        .finish();
    let header = Container::new(
        Container::new(header_row)
            .with_padding_left(10.0)
            .with_padding_right(13.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .with_border(Border::left(3.0).with_border_color(accent))
            .finish(),
    )
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .finish();
    let body = Container::new(crate::terminal::render::markdown::render_markdown(
        error, *theme, font,
    ))
    .with_padding_left(16.0)
    .with_padding_right(13.0)
    .with_padding_top(6.0)
    .with_padding_bottom(12.0)
    .finish();
    let body_rail = Container::new(body)
        .with_border(
            Border::left(3.0).with_border_color(ColorU::new(accent.r, accent.g, accent.b, 50)),
        )
        .finish();
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header)
        .with_child(body_rail)
        .finish()
}

pub(crate) fn shape_legacy_icon_button(
    icon: AppIcon,
    color: ColorU,
    action: WorkspaceAction,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    EventHandler::new(
        Container::new(render_icon(icon, color, 14.0))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish(),
    )
    .on_left_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        DispatchEventResult::StopPropagation
    })
    .finish()
}

pub(crate) fn shape_legacy_model_trigger(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    trigger_ms: MouseStateHandle,
    open: bool,
    locked: bool,
    selected_label: &str,
) -> Box<dyn Element> {
    let display: String = if selected_label.chars().count() > 22 {
        let mut s: String = selected_label.chars().take(21).collect();
        s.push('…');
        s
    } else {
        selected_label.to_string()
    };
    if locked {
        return Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(
                    Text::new(display, font, crate::theme::text_size(WI_BODY))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(render_icon(AppIcon::Info, theme.sub_text(), 11.0))
                .finish(),
        )
        .with_padding_left(4.0)
        .with_padding_right(4.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish();
    }
    let theme_owned = *theme;
    EventHandler::new(
        Hoverable::new(trigger_ms, move |state| {
            let bright = open || state.is_hovered();
            let color = if bright {
                theme_owned.main_text()
            } else {
                theme_owned.sub_text()
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(
                        Text::new(display.clone(), font, crate::theme::text_size(WI_BODY))
                            .with_color(color)
                            .finish(),
                    )
                    .with_child(render_icon(
                        if open {
                            AppIcon::ChevronDown
                        } else {
                            AppIcon::ChevronUp
                        },
                        color,
                        11.0,
                    ))
                    .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .finish(),
    )
    .on_left_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::ToggleShapeModelMenu);
        DispatchEventResult::StopPropagation
    })
    .finish()
}

pub(crate) fn submit_message_button(
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: MouseStateHandle,
    enabled: bool,
    has_active_work: bool,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse_state, move |state| {
        let foreground = if enabled {
            theme.accent_text()
        } else {
            theme.disabled_text()
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(label(
                    if has_active_work {
                        "Send"
                    } else {
                        "Create work"
                    },
                    foreground,
                    10.4,
                    font,
                ))
                .with_child(render_icon(AppIcon::ArrowUp, foreground, 11.0))
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(if enabled {
            if state.is_hovered() {
                theme.link()
            } else {
                theme.accent
            }
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
            event_ctx
                .dispatch_typed_action_for(workspace_view_id, WorkspaceAction::SubmitShapeMessage);
        }
    })
    .finish()
}
