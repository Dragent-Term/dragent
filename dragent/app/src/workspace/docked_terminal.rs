//! Chrome for the editor's docked terminal group.
//!
//! Terminal processes and layout state remain owned by `WorkspaceView`; this
//! module is intentionally presentation-only so the same group can later be
//! reused by the full engineering-workspace layout.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::elements::{
    Align, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, Element, EventHandler, Expanded, Flex, MainAxisSize,
    ParentElement, ScrollbarWidth, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::WorkspaceAction;

pub struct HeaderTab {
    pub id: usize,
    pub label: String,
    pub active: bool,
}

fn mouse_state(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: impl Into<String>,
) -> MouseStateHandle {
    states.borrow_mut().entry(key.into()).or_default().clone()
}

fn action_button(
    key: &'static str,
    icon: AppIcon,
    action: WorkspaceAction,
    view_id: EntityId,
    theme: Theme,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let state = mouse_state(states, format!("dock_terminal_action_{key}"));
    Box::new(
        Hoverable::new(state, move |mouse| {
            let background = if mouse.is_hovered() {
                theme.surface_3()
            } else {
                warpui::color::ColorU::new(0, 0, 0, 0)
            };
            let icon = ConstrainedBox::new(
                Align::new(render_icon(icon, theme.main_text(), 16.0)).finish(),
            )
            .with_width(32.0)
            .with_height(34.0)
            .finish();
            Container::new(icon)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone())),
    )
}

pub fn render_header(
    tabs: Vec<HeaderTab>,
    maximized: bool,
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    scroll_state: ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let mut tab_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(0.0);

    for tab in tabs {
        let tab_id = tab.id;
        let active = tab.active;
        let label = tab.label;
        let state = mouse_state(states, format!("dock_terminal_tab_{tab_id}"));
        let theme_for_tab = *theme;
        let tab_element: Box<dyn Element> = Box::new(
            Hoverable::new(state, move |mouse| {
                let hovered = mouse.is_hovered();
                let text_color = if active || hovered {
                    theme_for_tab.main_text()
                } else {
                    theme_for_tab.sub_text()
                };
                let close = if hovered {
                    let close = EventHandler::new(
                        ConstrainedBox::new(
                            Align::new(render_icon(AppIcon::XClose, text_color, 12.0)).finish(),
                        )
                        .with_width(20.0)
                        .with_height(20.0)
                        .finish(),
                    )
                    .on_left_mouse_down(move |ctx, _, _| {
                        ctx.dispatch_typed_action_for(
                            view_id,
                            WorkspaceAction::CloseDockedTerminal {
                                terminal_id: tab_id,
                            },
                        );
                        DispatchEventResult::StopPropagation
                    })
                    .finish();
                    close
                } else {
                    ConstrainedBox::new(warpui::elements::Empty::new().finish())
                        .with_width(20.0)
                        .with_height(20.0)
                        .finish()
                };
                let content = Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::TerminalPanel, text_color, 16.0))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Clipped::new(
                                Text::new(label.clone(), ui_font, crate::theme::text_size(13.0))
                                    .with_color(text_color)
                                    .soft_wrap(false)
                                    .with_clip(ClipConfig::ellipsis())
                                    .finish(),
                            )
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_child(close)
                    .finish();
                Container::new(content)
                    .with_padding_left(12.0)
                    .with_padding_right(7.0)
                    .with_padding_top(7.0)
                    .with_padding_bottom(7.0)
                    .with_background_color(if active {
                        theme_for_tab.surface_1()
                    } else if hovered {
                        theme_for_tab.surface_2()
                    } else {
                        warpui::color::ColorU::new(0, 0, 0, 0)
                    })
                    .with_border(Border::right(1.0).with_border_color(theme_for_tab.outline()))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    view_id,
                    WorkspaceAction::SelectDockedTerminal {
                        terminal_id: tab_id,
                    },
                )
            }),
        );
        tab_row = tab_row.with_child(ConstrainedBox::new(tab_element).with_width(225.0).finish());
    }

    let actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(1.0)
        .with_child(action_button(
            "new",
            AppIcon::Plus,
            WorkspaceAction::NewDockedTerminal,
            view_id,
            *theme,
            states,
        ))
        .with_child(action_button(
            "split",
            AppIcon::SplitVertical,
            WorkspaceAction::SplitDockedTerminal,
            view_id,
            *theme,
            states,
        ))
        .with_child(action_button(
            "maximize",
            if maximized {
                AppIcon::PanelRestore
            } else {
                AppIcon::PanelMaximize
            },
            WorkspaceAction::ToggleDockedTerminalMaximized,
            view_id,
            *theme,
            states,
        ))
        .finish();

    let actions = Container::new(actions)
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::left(1.0).with_border_color(theme.outline()))
        .finish();
    let actions = ConstrainedBox::new(actions).with_width(107.0).finish();

    let transparent = warpui::color::ColorU::new(0, 0, 0, 0);
    let tab_scroller = ClippedScrollable::horizontal(
        scroll_state,
        tab_row.finish(),
        ScrollbarWidth::None,
        transparent.into(),
        transparent.into(),
        transparent.into(),
    )
    .with_overlayed_scrollbar()
    .finish();

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_constrain_horizontal_bounds_to_parent(true)
            .with_child(Expanded::new(1.0, tab_scroller).finish())
            .with_child(actions)
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .finish()
}
