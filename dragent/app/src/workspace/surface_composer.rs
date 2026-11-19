//! Work-item composer and principal block views.
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

pub(crate) fn compact_composer(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    mouse_state: impl Fn(&'static str) -> MouseStateHandle + Copy,
) -> Box<dyn Element> {
    // Terminal agent parity: same InputEditor reuse, same bottom controls
    // ([+] attach, model picker, mic, salmon send/stop). Branch UI is
    // deliberately omitted — there is no branch chip or branch div here.
    let busy = props.composer_busy;
    let has_text = props.composer_has_text || !props.composer_attachments.is_empty();
    let mono = props.mono;

    // ── Top divider bar: identical to the terminal's chips_header — a full
    // bleed 1px top border cutting right-to-left above the input. The terminal
    // shows folder/branch chips here; principal omits branch entirely, so the
    // bar renders empty but the divider line stays in the exact same place.
    let top_divider = Container::new(Empty::new().finish())
        .with_background_color(theme.pane_surface())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish();

    // ── Attached files (above editor, like the terminal's attached blocks) ──
    let mut attached: Option<Box<dyn Element>> = None;
    if !props.composer_attachments.is_empty() {
        let mut attachments = Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_run_spacing(6.0);
        for (index, attachment) in props.composer_attachments.iter().enumerate() {
            let chip = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(render_icon(AppIcon::File, theme.accent, 12.0))
                    .with_child(
                        Text::new(
                            attachment.name().to_string(),
                            mono,
                            crate::theme::text_size(WI_BODY),
                        )
                        .with_color(theme.main_text())
                        .finish(),
                    )
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
        attached = Some(
            Container::new(attachments.finish())
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_border(
                    warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()),
                )
                .finish(),
        );
    }

    // ── Editor row: Expanded ChildView with NO fixed height, exactly like the
    // terminal's editor_row — the InputEditor sizes to content and grows up to
    // its max (336 for TerminalCommand), so cursor/placeholder placement,
    // growing and copy/paste all match.
    let editor_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(Box::new(Expanded::new(
            1.0,
            ChildView::new(&props.composer).finish(),
        )))
        .finish();

    let mut content = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    content = content.with_child(editor_row);
    // Turn/loading status lives in the conversation card while pending —
    // never inside the input. The input only shows idle notices (voice,
    // stopped confirmations).
    if !busy {
        if let Some(notice) = props.composer_notice {
            content = content.with_child(
                Text::new(notice.to_string(), mono, crate::theme::text_size(WI_BODY))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        }
        if let Some(activity) = props.principal_activity.as_deref() {
            content = content.with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Container::new(
                                ConstrainedBox::new(
                                    Container::new(Empty::new().finish())
                                        .with_background_color(theme.accent)
                                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                                            3.0,
                                        )))
                                        .finish(),
                                )
                                .with_width(6.0)
                                .with_height(6.0)
                                .finish(),
                            )
                            .with_padding_right(6.0)
                            .finish(),
                        )
                        .with_child(
                            Text::new(activity.to_string(), mono, crate::theme::text_size(WI_BODY))
                                .with_color(theme.sub_text())
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_top(4.0)
                .finish(),
            );
        }
    }

    // ── Model picker popover (global model, same source as terminal) ──
    if props.shape_model_menu_open && !props.shape_model_locked {
        const ROW_KEYS: [&str; 32] = [
            "shape-model-row-00",
            "shape-model-row-01",
            "shape-model-row-02",
            "shape-model-row-03",
            "shape-model-row-04",
            "shape-model-row-05",
            "shape-model-row-06",
            "shape-model-row-07",
            "shape-model-row-08",
            "shape-model-row-09",
            "shape-model-row-10",
            "shape-model-row-11",
            "shape-model-row-12",
            "shape-model-row-13",
            "shape-model-row-14",
            "shape-model-row-15",
            "shape-model-row-16",
            "shape-model-row-17",
            "shape-model-row-18",
            "shape-model-row-19",
            "shape-model-row-20",
            "shape-model-row-21",
            "shape-model-row-22",
            "shape-model-row-23",
            "shape-model-row-24",
            "shape-model-row-25",
            "shape-model-row-26",
            "shape-model-row-27",
            "shape-model-row-28",
            "shape-model-row-29",
            "shape-model-row-30",
            "shape-model-row-31",
        ];
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0);
        for (index, row) in props.shape_models.iter().enumerate() {
            let row_id = row.id.clone();
            let row_effort = row.effort_wire.clone();
            let row_label = row.label.clone();
            let selected = row.selected;
            let theme_owned = *theme;
            let key: &'static str = ROW_KEYS[index % ROW_KEYS.len()];
            let el = Hoverable::new(mouse_state(key), move |state| {
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
                            Text::new(row_label.clone(), mono, crate::theme::text_size(WI_SUBHEAD))
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
            let el = EventHandler::new(el)
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
                .finish();
            list = list.with_child(el);
        }
        let scrolling = ConstrainedBox::new(
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
        .finish();
        content = content.with_child(
            Container::new(scrolling)
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

    // ── Bottom controls (terminal layout: left attach+model, right mic+send) ──
    let mut controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(shape_input_icon_button(
            AppIcon::Plus,
            if props.composer_attachments.is_empty() {
                theme.sub_text()
            } else {
                theme.accent
            },
            WorkspaceAction::OpenShapeAttachmentPicker,
            workspace_view_id,
        ))
        .with_child(shape_model_trigger(
            props,
            theme,
            mono,
            workspace_view_id,
            mouse_state("shape_model_trigger"),
        ));
    controls = controls.with_child(Box::new(Expanded::new(1.0, Empty::new().finish())));
    if crate::voice::supported() {
        let (icon_color, border_color) = match props.voice_state {
            crate::voice::VoiceUiState::Idle => (theme.sub_text(), None),
            crate::voice::VoiceUiState::Connecting => (theme.accent, Some(theme.accent)),
            crate::voice::VoiceUiState::Recording => (theme.ui_red(), Some(theme.ui_red())),
        };
        let glyph = if props.voice_state == crate::voice::VoiceUiState::Connecting {
            crate::icons::render_spinner(props.voice_spin, icon_color, 14.0)
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

    // ── Send / Stop (terminal salmon pattern) ──
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
    let btn = Container::new(
        Container::new(inner)
            .with_padding_left(7.5)
            .with_padding_right(7.5)
            .with_padding_top(7.5)
            .with_padding_bottom(7.5)
            .finish(),
    )
    .with_background_color(bg)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .finish();
    controls = controls.with_child(
        Container::new(
            EventHandler::new(btn)
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

    // Terminal structure: full-bleed divider + attached strip on top, then the
    // padded editor card flush to the window bottom (bottom 4, no margin).
    let editor_container = Container::new(content.finish())
        .with_background_color(theme.pane_surface())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(4.0)
        .finish();
    let mut outer = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    outer = outer.with_child(top_divider);
    if let Some(strip) = attached {
        outer = outer.with_child(strip);
    }
    outer = outer.with_child(editor_container);
    let content = Container::new(outer.finish())
        .with_margin_left(0.0)
        .with_margin_right(0.0)
        .with_background_color(theme.pane_surface())
        .finish();
    super::composer_input::file_drop_target(content, workspace_view_id, |paths| {
        WorkspaceAction::AttachShapeFiles { paths }
    })
}

pub(crate) fn shape_input_icon_button(
    icon: AppIcon,
    color: ColorU,
    action: WorkspaceAction,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let inner = Container::new(render_icon(icon, color, 14.0))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish();
    EventHandler::new(inner)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

pub(crate) fn shape_model_trigger(
    props: &WorkItemSurfaceProps<'_>,
    theme: &Theme,
    font: FamilyId,
    workspace_view_id: EntityId,
    trigger_ms: MouseStateHandle,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    let open = props.shape_model_menu_open;
    let locked = props.shape_model_locked;
    let display = props.shape_selected_label.clone();
    let display: String = if display.chars().count() > 22 {
        let mut s: String = display.chars().take(21).collect();
        s.push('…');
        s
    } else {
        display
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

pub(crate) fn composer_action_button(
    icon: AppIcon,
    color: ColorU,
    action: WorkspaceAction,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let button = Container::new(render_icon(icon, color, 14.0))
        .with_uniform_padding(6.0)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
    EventHandler::new(button)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// Terminal agent card chrome — mirrors
/// `terminal::render::agent_block_element` exactly: 3px left accent rail,
/// 1px top outline divider, dot/avatar header row, markdown body.
pub(crate) const PRINCIPAL_ACCENT_WIDTH: f32 = 3.0;

pub(crate) fn principal_block_header(
    label: &str,
    accent: ColorU,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(accent)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();
    let label_el = Text::new(label.to_string(), mono, crate::theme::text_size(WI_HEADING))
        .with_color(accent)
        .with_style(Properties {
            weight: Weight::Semibold,
            ..Default::default()
        })
        .finish();
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(label_el)
        .finish();
    let header_body = Container::new(row)
        .with_padding_left(10.0)
        .with_padding_right(13.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .with_border(Border::left(PRINCIPAL_ACCENT_WIDTH).with_border_color(accent))
        .finish();
    Container::new(header_body)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}

pub(crate) fn principal_block_card(
    header: Box<dyn Element>,
    body: Option<Box<dyn Element>>,
    accent: ColorU,
) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header);
    if let Some(b) = body {
        let body_with_accent = Container::new(b)
            .with_border(
                Border::left(PRINCIPAL_ACCENT_WIDTH)
                    .with_border_color(ColorU::new(accent.r, accent.g, accent.b, 50)),
            )
            .finish();
        col = col.with_child(body_with_accent);
    }
    col.finish()
}

pub(crate) fn principal_copy_button(
    text: String,
    theme: &Theme,
    copy_ms: MouseStateHandle,
    workspace_view_id: EntityId,
) -> Box<dyn Element> {
    let theme_owned = *theme;
    Box::new(
        Hoverable::new(copy_ms, move |state| {
            let color = if state.is_hovered() {
                theme_owned.main_text()
            } else {
                theme_owned.sub_text()
            };
            crate::icons::render_icon(
                crate::icons::AppIcon::Copy,
                color,
                crate::icons::ICON_SIZE_SM,
            )
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _, _| {
            event_ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::CopyShapeMessageText { text: text.clone() },
            );
        }),
    )
}
