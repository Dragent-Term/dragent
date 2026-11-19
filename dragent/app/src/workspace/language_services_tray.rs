//! Compact multi-server health control and management tray.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisSize,
    ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::ide_chrome::{tooltip_above, tooltip_below};
use super::WorkspaceAction;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

type States = RefCell<HashMap<String, MouseStateHandle>>;

#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub server_id: String,
    pub label: String,
    pub detail: String,
    pub state: ServiceState,
    pub installable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Available,
    Downloading,
    Installed,
    Starting,
    Ready,
    Unavailable,
    Stopped,
}

impl ServiceState {
    fn color(self, theme: &Theme) -> warpui::color::ColorU {
        match self {
            Self::Available | Self::Stopped => theme.disabled_text(),
            Self::Downloading | Self::Starting => theme.accent,
            Self::Installed | Self::Ready => theme.ui_green(),
            Self::Unavailable => theme.ui_red(),
        }
    }
}

fn overall_state(services: &[ServiceSnapshot]) -> ServiceState {
    [
        ServiceState::Unavailable,
        ServiceState::Downloading,
        ServiceState::Starting,
        ServiceState::Ready,
    ]
    .into_iter()
    .find(|state| services.iter().any(|service| service.state == *state))
    .unwrap_or(ServiceState::Stopped)
}

pub fn render_button(
    services: &[ServiceSnapshot],
    open: bool,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    render_button_inner(services, open, theme, ui, id, states, false)
}

pub fn render_editor_button(
    services: &[ServiceSnapshot],
    open: bool,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    render_button_inner(services, open, theme, ui, id, states, true)
}

fn render_button_inner(
    services: &[ServiceSnapshot],
    open: bool,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
    editor_placement: bool,
) -> Box<dyn Element> {
    let state = states
        .borrow_mut()
        .entry("workspace_language_services_button".into())
        .or_default()
        .clone();
    let ready = services
        .iter()
        .filter(|service| service.state == ServiceState::Ready)
        .count();
    let total = services.len();
    let tooltip = format!("Language Servers · {ready}/{total} ready");
    let health = overall_state(services).color(theme);
    Box::new(
        Hoverable::new(state, move |state| {
            let dot = ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(health)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                    .finish(),
            )
            .with_width(6.0)
            .with_height(6.0)
            .finish();
            let mut content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(render_icon(AppIcon::Bolt, theme.sub_text(), 13.0));
            if editor_placement {
                content = content.with_child(
                    Text::new(format!("LSP {ready}/{total}"), ui, crate::theme::text_size(9.5))
                        .with_color(theme.main_text())
                        .finish(),
                );
            }
            let button = Container::new(content.with_child(dot).finish())
                .with_padding_left(7.0)
                .with_padding_right(7.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if open || state.is_hovered() {
                    theme.surface_2()
                } else {
                    theme.panel_background()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish();
            if editor_placement {
                tooltip_below(button, state.is_hovered(), tooltip.clone(), theme, ui)
            } else {
                tooltip_above(button, state.is_hovered(), tooltip.clone(), theme, ui)
            }
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, WorkspaceAction::ToggleLanguageServicesPanel)
        }),
    )
}

pub fn render_tray(
    services: &[ServiceSnapshot],
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let ready = services
        .iter()
        .filter(|service| service.state == ServiceState::Ready)
        .count();
    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(render_icon(AppIcon::Bolt, theme.sub_text(), 14.0))
        .with_child(
            Text::new("Language Servers", ui, crate::theme::text_size(11.5))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(
                format!("{ready}/{} running", services.len()),
                ui,
                crate::theme::text_size(9.5),
            )
            .with_color(theme.disabled_text())
            .finish(),
        )
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
        .with_child(icon_action(
            "language_services_close",
            AppIcon::XClose,
            "Close Language Servers",
            WorkspaceAction::ToggleLanguageServicesPanel,
            theme,
            ui,
            id,
            states,
        ))
        .finish();
    let mut rows = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(6.0);
    for service in services {
        rows = rows.with_child(service_row(service, theme, ui, mono, id, states));
    }
    let body = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(header)
        .with_child(rows.finish())
        .with_child(
            Flex::row()
                .with_spacing(6.0)
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(text_action(
                    "language_services_restart_all",
                    "Restart all",
                    WorkspaceAction::RestartLanguageServices,
                    theme,
                    ui,
                    id,
                    states,
                ))
                .with_child(text_action(
                    "language_services_stop_all",
                    "Stop all",
                    WorkspaceAction::StopLanguageServices,
                    theme,
                    ui,
                    id,
                    states,
                ))
                .finish(),
        )
        .finish();
    let card = ConstrainedBox::new(
        Container::new(body)
            .with_uniform_padding(11.0)
            .with_background_color(theme.solid_bg())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish(),
    )
    .with_width(410.0)
    .finish();
    card
}

fn service_row(
    service: &ServiceSnapshot,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let health = service.state.color(theme);
    let action = match service.state {
        ServiceState::Available => Some((
            "Enable",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Unavailable if service.installable => Some((
            "Download",
            WorkspaceAction::InstallLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Unavailable => None,
        ServiceState::Installed => Some((
            "Start",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Stopped => Some((
            "Start",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Downloading => Some((
            "Downloading…",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Starting => Some((
            "Starting…",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
        ServiceState::Ready => Some((
            "Restart",
            WorkspaceAction::RestartLanguageServer {
                server_id: service.server_id.clone(),
            },
        )),
    };
    let server_id = service.server_id.clone();
    let dot = ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(health)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish(),
    )
    .with_width(8.0)
    .with_height(8.0)
    .finish();
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(
            Flex::column()
                .with_spacing(2.0)
                .with_child(
                    Text::new(service.label.clone(), ui, crate::theme::text_size(10.5))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        format!("{} · {}", service.server_id, service.detail),
                        mono,
                        crate::theme::text_size(9.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .finish(),
        )
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if let Some((action_label, action)) = action {
        row = row.with_child(text_action(
            &format!("language_server_restart_{server_id}"),
            action_label,
            action,
            theme,
            ui,
            id,
            states,
        ));
    }
    row = row.with_child(icon_action(
        &format!("language_server_stop_{server_id}"),
        AppIcon::XClose,
        "Stop server",
        WorkspaceAction::StopLanguageServer { server_id },
        theme,
        ui,
        id,
        states,
    ));
    Container::new(row.finish())
        .with_uniform_padding(9.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish()
}

#[allow(clippy::too_many_arguments)]
fn icon_action(
    key: &str,
    icon: AppIcon,
    tooltip: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let state = states.borrow_mut().entry(key.into()).or_default().clone();
    let tooltip = tooltip.to_string();
    Box::new(
        Hoverable::new(state, move |state| {
            let button = Container::new(render_icon(icon, theme.sub_text(), 12.0))
                .with_uniform_padding(5.0)
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    theme.panel_background()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish();
            tooltip_above(button, state.is_hovered(), tooltip.clone(), theme, ui)
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
    )
}

#[allow(clippy::too_many_arguments)]
fn text_action(
    key: &str,
    label: &str,
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
                Text::new(label.clone(), ui, crate::theme::text_size(9.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
        })
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(id: &str, state: ServiceState) -> ServiceSnapshot {
        ServiceSnapshot {
            server_id: id.to_string(),
            label: id.to_string(),
            detail: String::new(),
            state,
            installable: false,
        }
    }

    #[test]
    fn catalog_entries_do_not_mask_a_confirmed_ready_server() {
        let services = vec![
            service("typescript-language-server", ServiceState::Ready),
            service("deno", ServiceState::Available),
        ];
        assert_eq!(overall_state(&services), ServiceState::Ready);
    }

    #[test]
    fn only_a_worker_confirmed_start_reports_starting() {
        let services = vec![
            service("typescript-language-server", ServiceState::Starting),
            service("deno", ServiceState::Available),
        ];
        assert_eq!(overall_state(&services), ServiceState::Starting);
    }
}
