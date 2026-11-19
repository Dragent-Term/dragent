//! Sub-header above the terminal pane — active session title and navigation hints.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult,
    Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentElement, PositionedElementAnchor, PositionedElementOffsetBounds,
    SavePosition, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::action::WorkspaceAction;
use super::share_sheet::{avatar, ShareParticipant};
use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::theme::{scaled, Theme, CONTENT_HEADER_HEIGHT, UI_FONT_SIZE};

type States = RefCell<HashMap<String, MouseStateHandle>>;

const TRANSPARENT: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};

/// `SavePosition` tag on the ⋮ button, so the share popover can anchor under it.
pub const SHARE_MENU_TAG: &str = "content_header_share_menu";
/// `SavePosition` tag on the presence stack, so the participants popover anchors under it.
pub const PARTICIPANTS_TAG: &str = "content_header_participants";
/// Per-participant access-chip tag prefix, so the access sub-menu anchors under it.
fn participant_chip_tag(id: &str) -> String {
    format!("hdr_part_{id}")
}

pub fn render_content_header(
    ui_font: FamilyId,
    theme: &Theme,
    session_title: &str,
    // When this pane is being shared by the host, `sharing` is true and the
    // header shows an avatar presence stack + a clickable "Live — Stop sharing"
    // chip (host-only). `participants` is the host + joined viewers.
    sharing: bool,
    participants: &[ShareParticipant],
    _right_sidebar_available: bool,
    _right_sidebar_visible: bool,
    document_open: bool,
    document_terminal_visible: bool,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    active_tab_id: Option<usize>,
) -> Box<dyn Element> {
    let accent_corner = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(theme.accent)
    .with_corner_radius(CornerRadius::with_top_left(Radius::Pixels(3.0)))
    .finish();

    let title_group = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.0)
        .with_child(render_icon(
            AppIcon::Terminal,
            theme.main_text(),
            ICON_SIZE_MD,
        ))
        .with_child(
            Expanded::new(
                1.0,
                Text::new(
                    session_title.to_string(),
                    ui_font,
                    crate::theme::text_size(UI_FONT_SIZE),
                )
                .with_color(theme.main_text())
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
            )
            .finish(),
        )
        .finish();

    let left_group = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(10.0)
        .with_child(
            Container::new(accent_corner)
                .with_padding_right(4.0)
                .finish(),
        )
        .with_child(Expanded::new(1.0, title_group).finish())
        .finish();

    // Split button in the split-pane header style: icon-only, transparent
    // until hovered. Does what Cmd+D does (side-by-side split of this tab).
    let split_hover = states
        .borrow_mut()
        .entry("content_header_split".to_string())
        .or_default()
        .clone();
    let split_btn = Hoverable::new(split_hover, move |st| {
        let bg = if st.is_hovered() {
            theme.surface_2()
        } else {
            TRANSPARENT
        };
        Container::new(render_icon(
            AppIcon::SplitHorizontal,
            if st.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            ICON_SIZE_MD,
        ))
        .with_uniform_padding(4.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(warpui_core::platform::Cursor::PointingHand)
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::SplitActiveTabHorizontal);
    })
    .finish();

    // Clickable ⋮ that opens the share popover, tagged so the popover (rendered
    // at the workspace root) can anchor beneath it.
    let overflow_hover = states
        .borrow_mut()
        .entry("content_header_overflow".to_string())
        .or_default()
        .clone();
    let sub = theme.sub_text();
    let overflow_hover_bg = theme.surface_1();
    let overflow_btn = Hoverable::new(overflow_hover, move |st| {
        let bg = if st.is_hovered() {
            overflow_hover_bg
        } else {
            TRANSPARENT
        };
        Container::new(render_icon(AppIcon::DotsVertical, sub, ICON_SIZE_MD))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
    })
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleContentHeaderMenu)
    });
    let overflow_btn = SavePosition::new(Box::new(overflow_btn), SHARE_MENU_TAG).finish();

    // Close button in the split-pane header style (X after the ⋮ menu).
    // CloseTab itself guards running foreground processes with a confirm
    // modal, so wiring it here is safe.
    let close_btn: Option<Box<dyn Element>> = active_tab_id.map(|tab_id| {
        let close_hover = states
            .borrow_mut()
            .entry("content_header_close".to_string())
            .or_default()
            .clone();
        Hoverable::new(close_hover, move |st| {
            let bg = if st.is_hovered() {
                theme.surface_2()
            } else {
                TRANSPARENT
            };
            Container::new(render_icon(
                AppIcon::XClose,
                if st.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                ICON_SIZE_MD,
            ))
            .with_uniform_padding(4.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        })
        .with_cursor(warpui_core::platform::Cursor::PointingHand)
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseTab { tab_id });
        })
        .finish()
    });

    let mut right_group = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0);
    if sharing {
        if !participants.is_empty() {
            right_group =
                right_group.with_child(presence_stack(participants, theme, ui_font, view_id));
        }
        right_group = right_group.with_child(sharing_chip(theme, ui_font, view_id, states));
    }
    right_group = right_group.with_child(split_btn).with_child(overflow_btn);
    if let Some(close_btn) = close_btn {
        right_group = right_group.with_child(close_btn);
    }
    if document_open {
        let terminal_hover = states
            .borrow_mut()
            .entry("content_header_document_terminal".to_string())
            .or_default()
            .clone();
        let terminal_color = if document_terminal_visible {
            theme.accent
        } else {
            theme.sub_text()
        };
        let terminal_bg = theme.surface_1();
        let terminal_button = Hoverable::new(terminal_hover, move |st| {
            Container::new(render_icon(
                AppIcon::LayoutBottom,
                terminal_color,
                ICON_SIZE_MD,
            ))
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .with_background_color(if st.is_hovered() || document_terminal_visible {
                terminal_bg
            } else {
                TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .with_cursor(warpui_core::platform::Cursor::PointingHand)
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleDocumentTerminal);
        })
        .finish();
        right_group = right_group.with_child(terminal_button);
    }

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(Shrinkable::new(1.0, left_group).finish())
        .with_child(right_group.finish())
        .finish();

    ConstrainedBox::new(
        Container::new(
            Container::new(row)
                .with_padding_left(10.0)
                .with_padding_right(12.0)
                // Leave room for the 30px presence avatars inside the 36px
                // content header instead of shrinking them to 24px.
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish(),
        )
        .with_background_color(theme.background)
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish(),
    )
    .with_height(CONTENT_HEADER_HEIGHT)
    .finish()
}

/// Build the ⋮ share popover: `(menu, dismiss_catcher, offset)`. When not
/// sharing, a single "Share session" row; when sharing, "View link" (re-open
/// the share sheet) plus "Stop session". The catcher is a full-screen
/// transparent layer that closes the popover on any click. The offset anchors
/// the menu under the ⋮ button via [`SHARE_MENU_TAG`].
pub fn share_menu_overlay(
    sharing: bool,
    active_tab_id: usize,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
    anchor_tag: &str,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let mut rows = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    // Artifact sharing gate (Settings → Skills → Sharing): when off, the
    // share entry is inert and explains itself; the backend blocks the call
    // too (see handle_sharing_action), so this is display, not security.
    let sharing_allowed = crate::settings::AppSettings::global()
        .sharing
        .artifact_sharing_enabled;
    if sharing {
        rows = rows
            .with_child(share_menu_row(
                "View link",
                AppIcon::Share,
                WorkspaceAction::OpenShareSheet,
                theme,
                ui_font,
                view_id,
                states,
            ))
            .with_child(share_menu_row(
                "Stop session",
                AppIcon::XClose,
                WorkspaceAction::RequestStopSharing,
                theme,
                ui_font,
                view_id,
                states,
            ));
    } else if sharing_allowed {
        rows = rows.with_child(share_menu_row(
            "Share session",
            AppIcon::Share,
            WorkspaceAction::ShareSession {
                tab_id: active_tab_id,
            },
            theme,
            ui_font,
            view_id,
            states,
        ));
    } else {
        rows = rows.with_child(share_disabled_row(
            "Sharing disabled",
            "Enable in Settings → Skills → Sharing",
            theme,
            ui_font,
        ));
    }
    let menu = ConstrainedBox::new(
        Container::new(rows.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_max_width(200.0)
    .finish();

    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(TRANSPARENT)
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseContentHeaderMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();

    let offset = OffsetPositioning::offset_from_save_position_element(
        anchor_tag,
        vec2f(0.0, 4.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}

/// Inert share-menu row shown when artifact sharing is disabled.
fn share_disabled_row(
    label: &str,
    hint: &str,
    theme: &Theme,
    ui_font: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(2.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(render_icon(AppIcon::Share, theme.disabled_text(), 14.0))
                    .with_child(
                        Text::new(label.to_string(), ui_font, crate::theme::text_size(13.0))
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_child(
                Text::new(hint.to_string(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(9.0)
    .with_padding_right(16.0)
    .with_padding_top(7.0)
    .with_padding_bottom(7.0)
    .finish()
}

fn share_menu_row(
    label: &str,
    icon: AppIcon,
    action: WorkspaceAction,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("content_header_menu_{label}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let hover_bg = theme.surface_3();
    let fg = theme.main_text();
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() {
                hover_bg
            } else {
                TRANSPARENT
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(render_icon(icon, fg, 14.0))
                    .with_child(
                        Text::new(label.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(fg)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_right(16.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone())),
    )
}

/// An overlapping avatar stack of everyone in the shared session (host first),
/// shown next to the Live chip so the host sees who's watching. Up to 4 +
/// overflow. Clickable (host): opens the participants popover to manage access.
fn presence_stack(
    participants: &[ShareParticipant],
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        // Stack the circular avatars while keeping the square paint bounds
        // and 50% mask from the shared avatar renderer intact.
        .with_spacing(-8.0);
    for p in participants.iter().take(4) {
        row = row.with_child(avatar(&p.name, &p.email, &p.avatar_url, theme, ui_font));
    }
    let extra = participants.len().saturating_sub(4);
    if extra > 0 {
        row = row.with_child(overflow_badge(extra, theme, ui_font));
    }
    let clickable = EventHandler::new(
        Container::new(row.finish())
            .with_padding_left(2.0)
            .with_padding_right(2.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(16.0)))
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleParticipantsMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();
    SavePosition::new(clickable, PARTICIPANTS_TAG).finish()
}

fn overflow_badge(extra: usize, theme: &Theme, ui_font: FamilyId) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(format!("+{extra}"), ui_font, crate::theme::text_size(10.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(theme.surface_3())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(15.0)))
        .finish(),
    )
    .with_width(30.0)
    .with_height(30.0)
    .finish()
}

/// Level-1 participants popover: everyone in the session, host first. Each
/// non-host row has a clickable access chip that opens the access sub-menu.
pub fn participants_menu_overlay(
    participants: &[ShareParticipant],
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(1.0)
        .with_child(
            Container::new(
                Text::new("In this session".to_string(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_top(5.0)
            .with_padding_bottom(4.0)
            .finish(),
        );
    for p in participants {
        col = col.with_child(participant_row(p, theme, ui_font, view_id, states));
    }
    let menu = ConstrainedBox::new(
        Container::new(col.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_max_width(280.0)
    .finish();

    let catcher = dismiss_catcher(view_id, WorkspaceAction::CloseParticipantsMenu);
    let offset = OffsetPositioning::offset_from_save_position_element(
        PARTICIPANTS_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}

fn participant_row(
    p: &ShareParticipant,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let name = if p.name.is_empty() {
        p.email.clone()
    } else {
        p.name.clone()
    };
    let access: Box<dyn Element> = if p.is_host {
        Container::new(
            Text::new("Host".to_string(), ui_font, crate::theme::text_size(12.0))
                .with_color(theme.disabled_text())
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(4.0)
        .finish()
    } else {
        access_chip(p, theme, ui_font, view_id, states)
    };
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(9.0)
            .with_child(avatar(&p.name, &p.email, &p.avatar_url, theme, ui_font))
            .with_child(Box::new(Expanded::new(
                1.0,
                Text::new(name, ui_font, crate::theme::text_size(13.0))
                    .with_color(theme.main_text())
                    .finish(),
            )))
            .with_child(access)
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(5.0)
    .with_padding_bottom(5.0)
    .finish()
}

/// The "Can view ⌄" / "Can edit ⌄" chip that opens a participant's access menu.
fn access_chip(
    p: &ShareParticipant,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("hdr_access_{}", p.id))
        .or_default()
        .clone();
    let (label, icon) = if p.control {
        ("Can edit", AppIcon::Edit)
    } else {
        ("Can view", AppIcon::Eye)
    };
    let color = theme.sub_text();
    let hover_bg = theme.surface_3();
    let pid = p.id.clone();
    let content = Hoverable::new(hover, move |st| {
        let bg = if st.is_hovered() {
            hover_bg
        } else {
            TRANSPARENT
        };
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(icon, color, 13.0))
                .with_child(
                    Text::new(label.to_string(), ui_font, crate::theme::text_size(12.0))
                        .with_color(color)
                        .finish(),
                )
                .with_child(render_icon(AppIcon::ChevronDown, color, 11.0))
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(6.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(
            view_id,
            WorkspaceAction::OpenParticipantAccess {
                participant: pid.clone(),
            },
        )
    });
    SavePosition::new(Box::new(content), &participant_chip_tag(&p.id)).finish()
}

/// Level-2 access sub-menu for one participant: Can view / Can edit / Remove.
pub fn participant_access_overlay(
    participant: &ShareParticipant,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let pid = participant.id.clone();
    let col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(access_row(
            AppIcon::Eye,
            "Can view",
            !participant.control,
            theme.main_text(),
            WorkspaceAction::SetParticipantAccess {
                participant: pid.clone(),
                control: false,
            },
            theme,
            ui_font,
            view_id,
            states,
        ))
        .with_child(access_row(
            AppIcon::Edit,
            "Can edit",
            participant.control,
            theme.main_text(),
            WorkspaceAction::SetParticipantAccess {
                participant: pid.clone(),
                control: true,
            },
            theme,
            ui_font,
            view_id,
            states,
        ))
        .with_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(theme.outline())
                    .finish(),
            )
            .with_height(1.0)
            .finish(),
        )
        .with_child(access_row(
            AppIcon::XClose,
            "Remove from session",
            false,
            theme.ui_red(),
            WorkspaceAction::RemoveParticipant {
                participant: pid.clone(),
            },
            theme,
            ui_font,
            view_id,
            states,
        ));
    let menu = ConstrainedBox::new(
        Container::new(col.finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(4.0)
            .with_padding_bottom(4.0)
            .finish(),
    )
    .with_max_width(210.0)
    .finish();

    let catcher = dismiss_catcher(view_id, WorkspaceAction::CloseParticipantAccess);
    let offset = OffsetPositioning::offset_from_save_position_element(
        &participant_chip_tag(&participant.id),
        vec2f(0.0, 4.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}

#[allow(clippy::too_many_arguments)]
fn access_row(
    icon: AppIcon,
    label: &str,
    selected: bool,
    fg: ColorU,
    action: WorkspaceAction,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("hdr_accrow_{label}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let sel_bg = theme.accent;
    let hover_bg = theme.surface_3();
    let white = ColorU {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    Box::new(
        Hoverable::new(hover, move |st| {
            let (bg, color) = if selected {
                (sel_bg, white)
            } else if st.is_hovered() {
                (hover_bg, fg)
            } else {
                (TRANSPARENT, fg)
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(render_icon(icon, color, 14.0))
                    .with_child(
                        Text::new(label.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(9.0)
            .with_padding_right(16.0)
            .with_padding_top(7.0)
            .with_padding_bottom(7.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone())),
    )
}

/// A full-screen transparent catcher that dispatches `action` on any click.
fn dismiss_catcher(view_id: EntityId, action: WorkspaceAction) -> Box<dyn Element> {
    EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(TRANSPARENT)
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, action.clone());
        DispatchEventResult::StopPropagation
    })
    .finish()
}

/// A clickable "● Live — Stop sharing" chip. Host-only; dispatches
/// [`WorkspaceAction::RequestStopSharing`]. Only rendered when this pane is shared.
fn sharing_chip(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry("content_header_stop_sharing".to_string())
        .or_default()
        .clone();
    let green = theme.ui_green();
    let idle_bg = ColorU {
        r: green.r,
        g: green.g,
        b: green.b,
        a: 28,
    };
    let hover_bg = ColorU {
        r: green.r,
        g: green.g,
        b: green.b,
        a: 48,
    };

    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hover_bg } else { idle_bg };
            let dot = Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(green)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish();
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(dot)
                    .with_child(Box::new(
                        Text::new("Live".to_string(), ui_font, crate::theme::text_size(11.0)).with_color(green),
                    ))
                    .with_child(render_icon(AppIcon::XClose, green, 11.0))
                    .with_child(Box::new(
                        Text::new("Stop sharing".to_string(), ui_font, crate::theme::text_size(11.0))
                            .with_color(green),
                    ))
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(view_id, WorkspaceAction::RequestStopSharing)
        }),
    )
}
