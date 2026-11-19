//! First-run project entry surface.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::{EntityId, ViewHandle};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;

use super::ssh::SshDestination;
use super::{InputEditor, ProjectFolderIntent, WorkspaceAction};

/// A saved machine as the start screen shows it.
pub struct StartMachine {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub host: String,
    pub status: crate::settings::machines_store::MachineStatus,
}

/// Saved machines for the start screen, connected ones first.
pub fn start_machines(app: &warpui::AppContext) -> Vec<StartMachine> {
    use warpui::SingletonEntity;
    let store = crate::settings::machines_store::MachinesStore::handle(app);
    let store = store.as_ref(app);
    let mut machines: Vec<StartMachine> = store
        .registry
        .machines
        .iter()
        .map(|m| StartMachine {
            id: m.id.clone(),
            name: crate::settings::settings_tasks_hosts::display_name_for(&m.id, &m.name()),
            endpoint: crate::settings::machines_store::endpoint_label(m),
            host: m.connection.host.clone(),
            status: store.status(&m.id),
        })
        .collect();
    machines.sort_by_key(|m| {
        (
            m.status != crate::settings::machines_store::MachineStatus::Connected,
            m.name.to_lowercase(),
        )
    });
    machines
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectStartMode {
    Clone,
    Ssh,
}

pub struct ProjectStartProps<'a> {
    pub workspace_view_id: EntityId,
    pub theme: &'a Theme,
    pub ui_font: FamilyId,
    pub editor: &'a ViewHandle<InputEditor>,
    pub mode: Option<ProjectStartMode>,
    pub has_input: bool,
    pub busy: bool,
    pub error: Option<&'a str>,
    pub active_project: Option<&'a PathBuf>,
    pub recent_projects: &'a [PathBuf],
    pub recent_ssh_hosts: &'a [SshDestination],
    pub ssh_config_hosts: &'a [SshDestination],
    /// Saved remote machines, listed above unsaved SSH config hosts.
    pub machines: &'a [StartMachine],
    pub available_width: f32,
    pub mouse_states: &'a RefCell<HashMap<String, MouseStateHandle>>,
}

fn mouse_state(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: impl Into<String>,
) -> MouseStateHandle {
    states
        .borrow_mut()
        .entry(key.into())
        .or_insert_with(MouseStateHandle::default)
        .clone()
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

#[allow(clippy::too_many_arguments)]
fn action_card(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui_font: FamilyId,
    mouse: MouseStateHandle,
    icon: AppIcon,
    title: &'static str,
    detail: &'static str,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    Hoverable::new(mouse, move |state| {
        let hovered = state.is_hovered();
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.0)
                .with_child(render_icon(icon, theme.main_text(), 17.0))
                .with_child(
                    Text::new(title.to_string(), ui_font, crate::theme::text_size(14.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(detail.to_string(), ui_font, crate::theme::text_size(10.5))
                        .with_color(theme.sub_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(if hovered {
            with_alpha(theme.accent, 0.09)
        } else {
            theme.surface_2()
        })
        .with_border(Border::all(1.0).with_border_color(if hovered {
            theme.accent
        } else {
            theme.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
    })
    .finish()
}

fn recent_project_row(
    props: &ProjectStartProps<'_>,
    root: &PathBuf,
    index: usize,
) -> Box<dyn Element> {
    let workspace_view_id = props.workspace_view_id;
    let theme = props.theme;
    let ui_font = props.ui_font;
    let action_root = root.clone();
    let show_path = props.available_width >= 520.0;
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Project")
        .to_string();
    let path = root.to_string_lossy().into_owned();
    Hoverable::new(
        mouse_state(props.mouse_states, format!("recent-{index}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_spacing(10.0)
                    .with_child(render_icon(AppIcon::Folder, theme.accent, 13.0))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(name.clone(), ui_font, crate::theme::text_size(12.5))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(if show_path {
                        ConstrainedBox::new(
                            Text::new(path.clone(), ui_font, crate::theme::text_size(10.5))
                                .with_color(theme.disabled_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .with_width(250.0)
                        .finish()
                    } else {
                        Text::new(String::new(), ui_font, crate::theme::text_size(1.0)).finish()
                    })
                    .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenRecentProject {
                root: action_root.clone(),
            },
        );
    })
    .finish()
}

fn ssh_host_row(
    props: &ProjectStartProps<'_>,
    destination: &SshDestination,
    source: &'static str,
    index: usize,
) -> Box<dyn Element> {
    let workspace_view_id = props.workspace_view_id;
    let theme = props.theme;
    let ui_font = props.ui_font;
    let destination_for_action = destination.clone();
    let label = destination.display();
    Hoverable::new(
        mouse_state(props.mouse_states, format!("ssh-host-{source}-{index}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(10.0)
                    .with_child(render_icon(AppIcon::Server, theme.sub_text(), 13.0))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(label.clone(), ui_font, crate::theme::text_size(12.5))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Text::new(source.to_string(), ui_font, crate::theme::text_size(10.5))
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::ConnectSshDestination {
                destination: destination_for_action.clone(),
            },
        );
    })
    .finish()
}

fn action_rows(props: &ProjectStartProps<'_>) -> Box<dyn Element> {
    let id = props.workspace_view_id;
    let theme = props.theme;
    let ui = props.ui_font;
    let row = |left, right| {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(Expanded::new(1.0, left).finish())
            .with_child(Expanded::new(1.0, right).finish())
            .finish()
    };
    let (primary_title, primary_description, primary_action) =
        if let Some(root) = props.active_project {
            (
                "Open project terminal",
                "Start a shell in this project",
                WorkspaceAction::ProjectFolderSelected {
                    root: root.clone(),
                    intent: ProjectFolderIntent::OpenFolder,
                },
            )
        } else {
            (
                "Open project",
                "Choose a folder on this computer",
                WorkspaceAction::OpenFolder,
            )
        };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(row(
            action_card(
                id,
                theme,
                ui,
                mouse_state(props.mouse_states, "start-open"),
                AppIcon::FolderOpen,
                primary_title,
                primary_description,
                primary_action,
            ),
            action_card(
                id,
                theme,
                ui,
                mouse_state(props.mouse_states, "start-clone"),
                AppIcon::ArrowDown,
                "Clone repository",
                "HTTPS or Git SSH repository URL",
                WorkspaceAction::ShowProjectClone,
            ),
        ))
        .with_child(row(
            action_card(
                id,
                theme,
                ui,
                mouse_state(props.mouse_states, "start-terminal"),
                AppIcon::Terminal,
                "Open terminal",
                "Start a local shell without a project",
                WorkspaceAction::NewTab,
            ),
            action_card(
                id,
                theme,
                ui,
                mouse_state(props.mouse_states, "start-ssh"),
                AppIcon::Server,
                "Remote machine",
                "Open a project on a server over SSH",
                WorkspaceAction::ShowProjectSsh,
            ),
        ))
        .finish()
}

fn input_form(props: &ProjectStartProps<'_>, mode: ProjectStartMode) -> Box<dyn Element> {
    let id = props.workspace_view_id;
    let theme = props.theme;
    let ui = props.ui_font;
    let title = match mode {
        ProjectStartMode::Clone => "Repository URL",
        ProjectStartMode::Ssh => "SSH destination",
    };
    let button = if props.busy {
        "Working…"
    } else if mode == ProjectStartMode::Clone {
        "Choose destination"
    } else {
        "Connect"
    };
    let enabled = props.has_input && !props.busy;
    let submit = Hoverable::new(
        mouse_state(props.mouse_states, "start-submit"),
        move |state| {
            Container::new(
                Text::new(button.to_string(), ui, crate::theme::text_size(11.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_background_color(if state.is_hovered() && enabled {
                theme.accent
            } else {
                theme.surface_3()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        if enabled {
            event_ctx.dispatch_typed_action_for(id, WorkspaceAction::SubmitProjectStartForm);
        }
    })
    .finish();
    let close = Hoverable::new(
        mouse_state(props.mouse_states, "start-cancel"),
        move |state| {
            Container::new(render_icon(
                AppIcon::XClose,
                if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                12.0,
            ))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(id, WorkspaceAction::CloseProjectStartForm);
    })
    .finish();

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_child(
                        Text::new(title.to_string(), ui, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .with_child(close)
                    .finish(),
            )
            .with_child(
                Container::new(
                    ConstrainedBox::new(ChildView::new(props.editor).finish())
                        .with_height(38.0)
                        .finish(),
                )
                .with_border(Border::all(1.0).with_border_color(theme.accent))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .finish(),
            )
            .with_child(submit)
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(10.0)
    .with_padding_bottom(12.0)
    .finish()
}

fn section_label(props: &ProjectStartProps<'_>, label: &str) -> Box<dyn Element> {
    Container::new(
        Text::new(label.to_string(), props.ui_font, crate::theme::text_size(11.0))
            .with_color(props.theme.sub_text())
            .finish(),
    )
    .with_padding_bottom(4.0)
    .finish()
}

/// A saved machine: status dot, name, endpoint; opens Add project on it.
fn machine_row(props: &ProjectStartProps<'_>, machine: &StartMachine, index: usize) -> Box<dyn Element> {
    let workspace_view_id = props.workspace_view_id;
    let theme = props.theme;
    let ui_font = props.ui_font;
    let name = machine.name.clone();
    let endpoint = machine.endpoint.clone();
    let status = machine.status;
    let id = machine.id.clone();
    Hoverable::new(
        mouse_state(props.mouse_states, format!("start-machine-{index}")),
        move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(10.0)
                    .with_child(crate::machine_ui::status_icon(theme, status, 13.0))
                    .with_child(
                        Text::new(name.clone(), ui_font, crate::theme::text_size(12.5))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .finish(),
                    )
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(endpoint.clone(), ui_font, crate::theme::text_size(11.5))
                                .with_color(theme.disabled_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Text::new(
                            if state.is_hovered() { "Add project" } else { status_word(status) }.to_string(),
                            ui_font,
                            crate::theme::text_size(10.5),
                        )
                        .with_color(if state.is_hovered() { theme.accent } else { theme.disabled_text() })
                        .finish(),
                    )
                    .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme.sidebar_row_hover()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::AddProject(super::add_project::AddProjectAction::Open {
                machine: Some(id.clone()),
            }),
        );
    })
    .finish()
}

fn status_word(status: crate::settings::machines_store::MachineStatus) -> &'static str {
    use crate::settings::machines_store::MachineStatus;
    match status {
        MachineStatus::Connected => "Connected",
        MachineStatus::Connecting => "Connecting",
        MachineStatus::Error => "Unreachable",
        MachineStatus::Idle => "Not connected",
    }
}

pub fn render(props: ProjectStartProps<'_>) -> Box<dyn Element> {
    let (title, description) = if let Some(root) = props.active_project {
        let project = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project");
        (
            project.to_string(),
            "No terminal is open. Start one when you're ready; the project remains available."
                .to_string(),
        )
    } else {
        (
            "Start with a project".to_string(),
            "Open a project, clone a repository, or start a terminal when you're ready."
                .to_string(),
        )
    };
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0)
        .with_child(
            Text::new(title, props.ui_font, crate::theme::text_size(21.0))
                .with_color(props.theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(description, props.ui_font, crate::theme::text_size(12.0))
                .with_color(props.theme.sub_text())
                .finish(),
        )
        .with_child(action_rows(&props));

    if let Some(mode) = props.mode {
        body = body.with_child(input_form(&props, mode));
    }

    if !props.recent_projects.is_empty() {
        let mut recent = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0)
            .with_child(
                Text::new(
                    "Recent projects".to_string(),
                    props.ui_font,
                    crate::theme::text_size(11.0),
                )
                .with_color(props.theme.sub_text())
                .finish(),
            );
        for (index, root) in props.recent_projects.iter().take(5).enumerate() {
            recent = recent.with_child(recent_project_row(&props, root, index));
        }
        body = body.with_child(recent.finish());
    }
    if !props.machines.is_empty() {
        let mut machines = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0)
            .with_child(section_label(&props, "Remote machines"));
        for (index, machine) in props.machines.iter().take(6).enumerate() {
            machines = machines.with_child(machine_row(&props, machine, index));
        }
        body = body.with_child(machines.finish());
    }
    // SSH config hosts not saved as machines yet: picking one starts the
    // Add machine form for it.
    let mut ssh_hosts = props.recent_ssh_hosts.to_vec();
    for destination in props.ssh_config_hosts {
        if !ssh_hosts.contains(destination) {
            ssh_hosts.push(destination.clone());
        }
    }
    ssh_hosts.retain(|destination| {
        !props
            .machines
            .iter()
            .any(|m| m.host == destination.host || m.endpoint == destination.display())
    });
    if !ssh_hosts.is_empty() {
        let mut recent = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(2.0)
            .with_child(section_label(&props, "From your SSH config"));
        for (index, destination) in ssh_hosts.iter().take(6).enumerate() {
            let source = if props.recent_ssh_hosts.contains(destination) {
                "Recent"
            } else {
                "Add"
            };
            recent = recent.with_child(ssh_host_row(&props, destination, source, index));
        }
        body = body.with_child(recent.finish());
    }

    Align::new(
        ConstrainedBox::new(
            Container::new(body.finish())
                .with_padding_left(20.0)
                .with_padding_right(20.0)
                .with_padding_top(24.0)
                .with_padding_bottom(24.0)
                .finish(),
        )
        .with_width(props.available_width.clamp(320.0, 680.0))
        .finish(),
    )
    .finish()
}
