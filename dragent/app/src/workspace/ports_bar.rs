//! Workspace status rail and device-scoped port-forwarding tray.
//!
//! The rail stays compact and stable while contextual surfaces open above it.
//! Port forwarding remains deliberately separate from session sharing.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::vec2f;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::terminal::view::actions::TerminalAction;
use crate::theme::{scaled, Theme};
use warpui::color::ColorU;

use super::ide_chrome::tooltip_above;
use super::language_services_tray::{self, ServiceSnapshot};
use super::{InputEditor, WorkspaceAction};

type States = RefCell<HashMap<String, MouseStateHandle>>;

pub const PORTS_BUTTON_TAG: &str = "workspace_ports_button";
pub const LANGUAGE_SERVICES_BUTTON_TAG: &str = "workspace_language_services_button_anchor";

#[derive(Clone, Debug, Default)]
pub struct PortRow {
    pub id: String,
    pub port: u16,
    pub url: String,
}

#[derive(Clone, Debug, Default)]
pub struct PortsSnapshot {
    pub forwards: Vec<PortRow>,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub git: Option<GitSnapshot>,
    pub language_services: Vec<ServiceSnapshot>,
}

#[derive(Clone, Debug, Default)]
pub struct GitSnapshot {
    pub branch: String,
    pub changed_files: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub ahead: u32,
    pub behind: u32,
    pub added: u32,
    pub deleted: u32,
    pub operation: crate::git_status::GitOperation,
}

#[allow(clippy::too_many_arguments)]
pub fn render_ports_bar(
    snap: &PortsSnapshot,
    ports_open: bool,
    language_services_open: bool,
    _git_tab_active: bool,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let ports_state = states
        .borrow_mut()
        .entry("workspace_status_ports".into())
        .or_default()
        .clone();
    let count = snap.forwards.len();
    let ports_button = Hoverable::new(ports_state, move |state| {
        let active = ports_open || state.is_hovered();
        let button = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::Server, theme.sub_text(), 12.0))
                .with_child(
                    Text::new("Ports", ui, crate::theme::text_size(10.5))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(count_badge(count, snap.busy, theme, mono))
                .with_child(render_icon(
                    if ports_open {
                        AppIcon::ChevronDown
                    } else {
                        AppIcon::ChevronUp
                    },
                    theme.disabled_text(),
                    10.0,
                ))
                .finish(),
        )
        .with_padding_left(9.0)
        .with_padding_right(8.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(if active {
            theme.surface_2()
        } else {
            theme.surface_1()
        })
        .with_border(Border::all(1.0).with_border_color(if ports_open {
            theme.accent
        } else {
            theme.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
        tooltip_above(
            button,
            state.is_hovered(),
            if ports_open {
                "Hide Port Forwarding"
            } else {
                "Open Port Forwarding"
            },
            theme,
            ui,
        )
    })
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, WorkspaceAction::TogglePortsPanel))
    .finish();
    let ports_button = SavePosition::new(ports_button, PORTS_BUTTON_TAG).finish();

    let language_services_button = SavePosition::new(
        language_services_tray::render_button(
            &snap.language_services,
            language_services_open,
            theme,
            ui,
            id,
            states,
        ),
        LANGUAGE_SERVICES_BUTTON_TAG,
    )
    .finish();

    let mut rail = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0);
    // Pull/push only appear when there's something to sync. The branch and
    // working-tree chips were removed: Source Control in the sidebar shows
    // both.
    if let Some(git) = snap.git.as_ref() {
        let ahead = git.ahead;
        let behind = git.behind;
        let pulling = git.operation == crate::git_status::GitOperation::Pulling;
        let pushing = git.operation == crate::git_status::GitOperation::Pushing;
        if behind > 0 || pulling {
            rail = rail.with_child(git_sync_chip(
                if pulling {
                    "↓ pulling…".to_string()
                } else {
                    format!("↓{behind}")
                },
                pulling,
                WorkspaceAction::PullActiveGitBranch,
                "Pull from origin",
                theme,
                mono,
                id,
                states,
                "workspace_status_pull",
            ));
        }
        if ahead > 0 || pushing {
            rail = rail.with_child(git_sync_chip(
                if pushing {
                    "↑ pushing…".to_string()
                } else {
                    format!("↑{ahead}")
                },
                pushing,
                WorkspaceAction::PushActiveGitBranch,
                "Push to origin",
                theme,
                mono,
                id,
                states,
                "workspace_status_push",
            ));
        }
    }

    rail = rail
        .with_child(Expanded::new(1.0, warpui::elements::Empty::new().finish()).finish())
        .with_child(language_services_button)
        .with_child(ports_button);

    let rail = Container::new(rail.finish())
        .with_padding_left(10.0)
        .with_padding_right(8.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(theme.panel_background())
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish();

    rail
}

#[allow(clippy::too_many_arguments)]
fn git_sync_chip(
    label: String,
    busy: bool,
    action: WorkspaceAction,
    tooltip: &'static str,
    theme: &Theme,
    mono: FamilyId,
    id: EntityId,
    states: &States,
    state_key: &'static str,
) -> Box<dyn Element> {
    let mouse = states
        .borrow_mut()
        .entry(state_key.into())
        .or_default()
        .clone();
    Hoverable::new(mouse, move |state| {
        let chip = Container::new(
            Text::new(label.clone(), mono, crate::theme::text_size(10.0))
                .with_color(if busy {
                    theme.accent
                } else {
                    theme.main_text()
                })
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
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
        tooltip_above(chip, state.is_hovered(), tooltip, theme, mono)
    })
    .with_cursor(if busy {
        Cursor::Arrow
    } else {
        Cursor::PointingHand
    })
    .on_click(move |ctx, _, _| {
        if !busy {
            ctx.dispatch_typed_action_for(id, action.clone());
        }
    })
    .finish()
}

/// Ports and language-server management expand upward from their bottom-bar
/// controls. They float above the editor rather than becoming another row in
/// its flex layout, which keeps the document viewport from jumping.
#[allow(clippy::too_many_arguments)]
pub fn render_workspace_sheet(
    snap: &PortsSnapshot,
    ports_open: bool,
    language_services_open: bool,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    port_editor: &ViewHandle<InputEditor>,
    id: EntityId,
    states: &States,
) -> Option<(Box<dyn Element>, OffsetPositioning)> {
    let (panel, anchor) = if ports_open {
        (
            ConstrainedBox::new(render_panel(snap, theme, ui, mono, port_editor, id, states))
                .with_max_width(820.0)
                .finish(),
            PORTS_BUTTON_TAG,
        )
    } else if language_services_open {
        (
            language_services_tray::render_tray(
                &snap.language_services,
                theme,
                ui,
                mono,
                id,
                states,
            ),
            LANGUAGE_SERVICES_BUTTON_TAG,
        )
    } else {
        return None;
    };

    let offset = OffsetPositioning::offset_from_save_position_element(
        anchor,
        vec2f(0.0, -4.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::TopRight,
        ChildAnchor::BottomRight,
    );
    Some((panel, offset))
}

fn render_panel(
    snap: &PortsSnapshot,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    port_editor: &ViewHandle<InputEditor>,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let input = crate::shared_input::shared_input(
        theme,
        port_editor,
        crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 7.0,
            // Match the terminal composer: transparent over a background-image
            // theme, and the exact theme background for an opaque theme.
            background: Some(theme.pane_surface()),
            border: Some(theme.outline()),
            pad_left: 10.0,
            pad_right: 10.0,
            pad_top: 6.0,
            pad_bottom: 6.0,
            ..Default::default()
        },
    );
    let close_state = states
        .borrow_mut()
        .entry("ports_tray_close".into())
        .or_default()
        .clone();
    let close = Hoverable::new(close_state, move |state| {
        let button = Container::new(render_icon(AppIcon::XClose, theme.sub_text(), 12.0))
            .with_uniform_padding(6.0)
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                theme.pane_surface()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish();
        tooltip_above(button, state.is_hovered(), "Close Ports tray", theme, ui)
    })
    .on_click(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, WorkspaceAction::TogglePortsPanel)
    });

    let tray_header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(9.0)
        .with_child(
            Container::new(render_icon(AppIcon::Server, theme.accent, 14.0))
                .with_uniform_padding(7.0)
                .with_background_color(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
        )
        .with_child(
            Flex::column()
                .with_spacing(1.0)
                .with_child(
                    Text::new("Port forwarding", ui, crate::theme::text_size(12.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        "Expose local services through secure workspace URLs",
                        ui,
                        crate::theme::text_size(10.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .finish(),
        )
        .with_child(Expanded::new(1.0, warpui::elements::Empty::new().finish()).finish())
        .with_child(Box::new(close))
        .finish();

    let mut panel = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(tray_header)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(Expanded::new(1.0, input).finish())
                .with_child(action_button(
                    AppIcon::Plus,
                    if snap.busy {
                        "Adding…"
                    } else {
                        "Forward a Port"
                    },
                    "ports_add",
                    WorkspaceAction::CreatePortForward,
                    theme,
                    ui,
                    id,
                    states,
                ))
                .finish(),
        );

    if let Some(message) = snap.error.as_deref().or(snap.notice.as_deref()) {
        panel = panel.with_child(
            Text::new(message.to_string(), ui, crate::theme::text_size(10.5))
                .with_color(if snap.error.is_some() {
                    theme.ui_red()
                } else {
                    theme.sub_text()
                })
                .finish(),
        );
    }

    panel = panel.with_child(table_header(theme, ui));
    if snap.forwards.is_empty() {
        panel = panel.with_child(
            Container::new(
                Text::new(
                    "No forwarded ports. Enter a local port above to expose it securely.",
                    ui,
                    crate::theme::text_size(11.0),
                )
                .with_color(theme.disabled_text())
                .finish(),
            )
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        );
    } else {
        for (index, forward) in snap.forwards.iter().enumerate() {
            panel = panel.with_child(port_row(forward, index, theme, ui, mono, id, states));
        }
    }

    Container::new(panel.finish())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .with_background_color(theme.solid_bg())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .finish()
}

fn count_badge(count: usize, busy: bool, theme: &Theme, mono: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(
            if busy {
                "…".into()
            } else {
                count.to_string()
            },
            mono,
            crate::theme::text_size(9.5),
        )
        .with_color(if count > 0 {
            theme.main_text()
        } else {
            theme.disabled_text()
        })
        .finish(),
    )
    .with_padding_left(5.0)
    .with_padding_right(5.0)
    .with_padding_top(1.0)
    .with_padding_bottom(1.0)
    .with_background_color(theme.surface_2())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish()
}

fn status_item(
    icon: AppIcon,
    label: &str,
    badge: Option<String>,
    tooltip: String,
    color: warpui::color::ColorU,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    states: &States,
) -> Box<dyn Element> {
    let state = states
        .borrow_mut()
        .entry("workspace_status_git".into())
        .or_default()
        .clone();
    let label = truncate(label, 24);
    Box::new(Hoverable::new(state, move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(icon, color, 11.0))
            .with_child(
                Text::new(label.clone(), ui, crate::theme::text_size(10.0))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        if let Some(badge) = badge.as_ref() {
            row = row.with_child(
                Text::new(badge.clone(), mono, crate::theme::text_size(9.5))
                    .with_color(theme.accent)
                    .finish(),
            );
        }
        let item = Container::new(row.finish())
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish();
        tooltip_above(item, state.is_hovered(), tooltip.clone(), theme, ui)
    }))
}

fn table_header(theme: &Theme, ui: FamilyId) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(width_text("Port", 64.0, theme.disabled_text(), ui))
        .with_child(
            Expanded::new(
                1.0,
                Text::new("Forwarded Address", ui, crate::theme::text_size(10.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .finish(),
        )
        .with_child(width_text("Status", 68.0, theme.disabled_text(), ui))
        .with_child(width_text("Actions", 150.0, theme.disabled_text(), ui))
        .finish()
}

fn port_row(
    forward: &PortRow,
    index: usize,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let url = forward.url.clone();
    let open_url = url.clone();
    let close_id = forward.id.clone();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                ConstrainedBox::new(
                    Text::new(forward.port.to_string(), mono, crate::theme::text_size(11.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_width(64.0)
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(truncate(&url, 72), mono, crate::theme::text_size(10.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    Text::new("Forwarded", ui, crate::theme::text_size(10.5))
                        .with_color(theme.ui_green())
                        .finish(),
                )
                .with_width(68.0)
                .finish(),
            )
            .with_child(action_button(
                AppIcon::ChevronRight,
                "Open",
                &format!("ports_open_{index}"),
                WorkspaceAction::OpenPortForward { url: open_url },
                theme,
                ui,
                id,
                states,
            ))
            .with_child(action_button(
                AppIcon::Copy,
                "Copy",
                &format!("ports_copy_{index}"),
                WorkspaceAction::CopyPortForwardValue {
                    port: forward.port,
                    value: url,
                },
                theme,
                ui,
                id,
                states,
            ))
            .with_child(action_button(
                AppIcon::XClose,
                "Stop",
                &format!("ports_stop_{index}"),
                WorkspaceAction::ClosePortForward {
                    forward_id: close_id,
                },
                theme,
                ui,
                id,
                states,
            ))
            .finish(),
    )
    .with_padding_top(5.0)
    .with_padding_bottom(5.0)
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .finish()
}

fn action_button(
    icon: AppIcon,
    label: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let state = states.borrow_mut().entry(key.into()).or_default().clone();
    let label = label.to_string();
    Box::new(
        Hoverable::new(state, move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(render_icon(icon, theme.sub_text(), 11.0))
                    .with_child(
                        Text::new(label.clone(), ui, crate::theme::text_size(10.5))
                            .with_color(theme.main_text())
                            .finish(),
                    )
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
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

fn width_text(
    label: &str,
    width: f32,
    color: warpui::color::ColorU,
    ui: FamilyId,
) -> Box<dyn Element> {
    ConstrainedBox::new(
        Text::new(label.to_string(), ui, crate::theme::text_size(10.0))
            .with_color(color)
            .finish(),
    )
    .with_width(width)
    .finish()
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}
