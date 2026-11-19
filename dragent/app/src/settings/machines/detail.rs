//! One machine's page: breadcrumb, an identity header with the primary
//! connect action and a ⋯ menu, labelled tabs, and per-tab sections —
//! Overview (connection, runtime, resources, dependencies), Projects,
//! Sessions and Agents.
use super::super::machines_store::{
    endpoint_label, format_bytes, MachineConnection, MachineStatus, MachinesStore, RuntimeCommand,
    RuntimeState, SshState,
};
use super::{Confirmation, MachineAction, MachineTab, SettingsView};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::settings::SettingsAction;
use crate::settings::tool_catalog::{self, Os};
use crate::theme::Theme;
use dragent_host::development::{Elevation, ToolStatus};
use dragent_host::host_tools;
use dragent_host::machine_registry::Machine;
use dragent_host::remote::Authentication;
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element, EventHandler,
    Expanded, Flex, MainAxisSize, ParentElement,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::Border;

fn act(action: MachineAction) -> SettingsAction {
    SettingsAction::Machine(action)
}

/// Tools Dragent needs on every machine; the rest are recommended.
const REQUIRED_TOOLS: &[&str] = &["git"];
/// Built-in agents detected from the machine's tools.
/// Shown when a runtime predates agent configurations.
const KNOWN_AGENTS: &[(&str, &str, AppIcon)] = &[
    ("claude", "Claude Code", AppIcon::ClaudeLogo),
    ("codex", "Codex", AppIcon::ChatgptLogo),
];

impl SettingsView {
    pub(super) fn render_machine_detail(
        &self,
        theme: &Theme,
        store: &MachinesStore,
        id: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let Some(machine) = store.machine(id) else {
            return mui::spacer(0.0);
        };
        let connection = store.connection(id);
        let status = store.status(id);

        let mut page = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.breadcrumb(theme, &machine.name()))
            .with_child(mui::spacer(18.0))
            .with_child(self.entity_header(theme, machine, connection, status))
            .with_child(mui::spacer(24.0));
        if let Some(notice) = &self.machines.notice {
            page = page
                .with_child(mui::notice(theme, ui, Tone::Error, "Something went wrong", Some(notice.clone()), None))
                .with_child(mui::spacer(16.0));
        }
        if let Some(problem) = self.connection_problem(theme, machine, connection) {
            page = page.with_child(problem).with_child(mui::spacer(16.0));
        }
        page = page
            .with_child(self.machine_tabs(theme, store, machine, connection))
            .with_child(mui::spacer(24.0));
        let section = match self.machines.tab {
            MachineTab::System => self.system_tab(theme, machine, connection),
            MachineTab::Workspaces => self.workspaces_tab(theme, store, machine, connection),
            MachineTab::Sessions => self.sessions_tab(theme, store, machine, connection),
            MachineTab::Agents => self.agents_tab(theme, machine, connection),
        };
        page.with_child(section).with_child(mui::spacer(32.0)).finish()
    }

    fn breadcrumb(&self, theme: &Theme, name: &str) -> Box<dyn Element> {
        let ui = self.ui_font;
        let theme_c = *theme;
        let view_id = self.view_id;
        let machines = Hoverable::new(self.hover("machines:crumb"), move |state| {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::ChevronLeft, theme_c.sub_text(), 12.0))
                .with_child(mui::text(
                    ui,
                    "Machines",
                    12.5,
                    if state.is_hovered() { theme_c.main_text() } else { theme_c.sub_text() },
                ))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, act(MachineAction::ShowList)))
        .finish();
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(machines)
            .with_child(mui::text(ui, "/", 12.5, theme.disabled_text()))
            .with_child(mui::line(ui, name, 12.5, theme.main_text()))
            .finish()
    }

    /// Avatar, name, one line of facts, and the actions: the primary
    /// connect/disconnect button and a ⋯ menu.
    fn entity_header(
        &self,
        theme: &Theme,
        machine: &Machine,
        connection: Option<&MachineConnection>,
        status: MachineStatus,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let title: Box<dyn Element> = if self.machines.renaming {
            let view_id = self.view_id;
            EventHandler::new(
                ConstrainedBox::new(crate::shared_input::shared_input(
                    theme,
                    &self.machines.fields["rename"],
                    crate::shared_input::SharedInputOpts {
                        height: Some(36.0),
                        radius: mui::RADIUS_CONTROL,
                        ..Default::default()
                    },
                ))
                .with_max_width(360.0)
                .finish(),
            )
            .on_keydown(move |ctx, _, keystroke| match keystroke.key.as_str() {
                "enter" => {
                    ctx.dispatch_typed_action_for(view_id, act(MachineAction::CommitRename));
                    DispatchEventResult::StopPropagation
                }
                "escape" => {
                    ctx.dispatch_typed_action_for(view_id, act(MachineAction::CancelRename));
                    DispatchEventResult::StopPropagation
                }
                _ => DispatchEventResult::PropagateToParent,
            })
            .finish()
        } else {
            mui::line(ui, machine.name(), 20.0, theme.main_text())
        };
        let mut facts = vec![endpoint_label(machine)];
        if let Some(env) = connection.and_then(|c| c.environment.as_ref()) {
            facts.push(mui::platform_label(&env.os, &env.arch));
        }
        let subtitle = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(mui::status_pill(theme, ui, status))
            .with_child(Expanded::new(1.0, mui::line(ui, facts.join("  ·  "), 12.5, theme.sub_text())).finish())
            .finish();

        let ssh = connection.map(|c| c.ssh).unwrap_or(SshState::Disconnected);
        let mut actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0);
        if matches!(ssh, SshState::Connected | SshState::Connecting) {
            actions = actions.with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:disconnect"),
                Some(AppIcon::Power),
                if ssh == SshState::Connecting { "Cancel" } else { "Disconnect" },
                Variant::Secondary,
                Size::Md,
                true,
                act(MachineAction::Disconnect(machine.id.clone())),
            ));
        } else {
            actions = actions.with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:connect"),
                Some(AppIcon::Plug),
                if status == MachineStatus::Error { "Try again" } else { "Connect" },
                Variant::Primary,
                Size::Md,
                true,
                act(MachineAction::Connect(machine.id.clone())),
            ));
        }
        let more = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:more"),
            Some(AppIcon::Ellipsis),
            "",
            Variant::Secondary,
            Size::Md,
            true,
            act(MachineAction::ToggleMenu),
        );
        let menu = self.machines.menu_open.then(|| {
            mui::menu_panel(
                theme,
                vec![
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:menu:rename"),
                        Some(render_icon(AppIcon::Edit, theme.sub_text(), 14.0)),
                        "Rename",
                        None,
                        false,
                        false,
                        act(MachineAction::StartRename),
                    ),
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:menu:edit"),
                        Some(render_icon(AppIcon::Settings, theme.sub_text(), 14.0)),
                        "Edit connection",
                        None,
                        false,
                        false,
                        act(MachineAction::Edit(machine.id.clone())),
                    ),
                    mui::menu_separator(theme),
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:menu:delete"),
                        Some(render_icon(AppIcon::Trash, theme.ui_red(), 14.0)),
                        "Delete machine",
                        None,
                        false,
                        true,
                        act(MachineAction::RequestDelete),
                    ),
                ],
                200.0,
            )
        });
        actions = actions.with_child(mui::with_menu(more, menu, true));

        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_child(mui::machine_avatar(theme, status, 52.0))
            .with_child(
                Expanded::new(
                    1.0,
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_spacing(8.0)
                        .with_child(title)
                        .with_child(subtitle)
                        .finish(),
                )
                .finish(),
            )
            .with_child(actions.finish())
            .finish()
    }

    /// A failed connection, explained once above the tabs with its fix.
    fn connection_problem(
        &self,
        theme: &Theme,
        machine: &Machine,
        connection: Option<&MachineConnection>,
    ) -> Option<Box<dyn Element>> {
        let connection = connection?;
        let ui = self.ui_font;
        if let Some(key) = &connection.host_key {
            return Some(mui::host_key_notice(
                theme,
                ui,
                &machine.name(),
                key,
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:trust-key"),
                    None,
                    "Trust and connect",
                    Variant::Primary,
                    Size::Xs,
                    true,
                    act(MachineAction::TrustHostKey(machine.id.clone())),
                ),
            ));
        }
        let error = connection.error.as_ref()?;
        let (title, action) = match (connection.ssh, connection.runtime) {
            (SshState::Failed, _) => (
                format!("Couldn't reach {}", machine.name()),
                Some(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:problem-edit"),
                    Some(AppIcon::Settings),
                    "Edit connection",
                    Variant::Secondary,
                    Size::Xs,
                    true,
                    act(MachineAction::Edit(machine.id.clone())),
                )),
            ),
            (SshState::Connected, RuntimeState::Failed | RuntimeState::Stopped) => {
                ("The workspace runtime isn't running".to_string(), None)
            }
            _ => return None,
        };
        Some(mui::notice(theme, ui, Tone::Error, &title, Some(error.clone()), action))
    }

    /// Labelled tabs with an accent underline on the active one.
    fn machine_tabs(
        &self,
        theme: &Theme,
        store: &MachinesStore,
        machine: &Machine,
        connection: Option<&MachineConnection>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let projects = store.projects_for(&machine.id).len();
        let sessions = connection
            .map(|c| {
                c.sessions
                    .iter()
                    .filter(|s| matches!(s.state, dragent_host::TerminalState::Running))
                    .count()
            })
            .unwrap_or(0);
        let tabs = [
            (MachineTab::System, AppIcon::Activity, "Overview", None),
            (MachineTab::Workspaces, AppIcon::FolderGit, "Projects", Some(projects)),
            (MachineTab::Sessions, AppIcon::Terminal, "Sessions", Some(sessions)),
            (MachineTab::Agents, AppIcon::Agent, "Agents", None),
        ]
        .into_iter()
        .map(|(tab, icon, label, count)| {
            mui::tab(
                theme,
                ui,
                self.view_id,
                self.hover(&format!("machines:tab:{label}")),
                icon,
                label,
                count,
                self.machines.tab == tab,
                act(MachineAction::Tab(tab)),
            )
        })
        .collect();
        mui::tab_strip(theme, tabs)
    }

    // ── Overview ──────────────────────────────────────────────────────────

    fn system_tab(
        &self,
        theme: &Theme,
        machine: &Machine,
        connection: Option<&MachineConnection>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let running = connection.is_some_and(|c| c.ready());
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.group_title_at(theme, "Connection", None, 0.0))
            .with_child(mui::list_card(
                theme,
                vec![self.ssh_row(theme, machine, connection), self.runtime_row(theme, connection)],
            ))
            .with_child(mui::spacer(8.0))
            .with_child(super::super::settings_tasks_hosts::render_host_override_card(
                self,
                theme,
                &machine.id,
            ));
        if running {
            column = column
                .with_child(self.group_title(theme, "Resources", None))
                .with_child(self.resource_tiles(theme, connection))
                .with_child(self.dependencies(theme, connection));
        } else {
            column = column.with_child(mui::spacer(20.0)).with_child(mui::notice(
                theme,
                ui,
                Tone::Neutral,
                "Connect to see resources and tools",
                Some("CPU, memory, disk and installed tools appear once the machine is connected.".into()),
                None,
            ));
        }
        column.finish()
    }

    /// An eyebrow heading with an optional right-aligned note.
    fn group_title(&self, theme: &Theme, label: &str, note: Option<String>) -> Box<dyn Element> {
        self.group_title_at(theme, label, note, 28.0)
    }

    fn group_title_at(&self, theme: &Theme, label: &str, note: Option<String>, top: f32) -> Box<dyn Element> {
        let ui = self.ui_font;
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, mui::eyebrow(ui, theme, label)).finish());
        if let Some(note) = note {
            row = row.with_child(mui::text(ui, note, 12.0, theme.sub_text()));
        }
        Container::new(row.finish())
            .with_padding_left(2.0)
            .with_padding_top(top)
            .with_padding_bottom(10.0)
            .finish()
    }

    fn ssh_row(&self, theme: &Theme, machine: &Machine, connection: Option<&MachineConnection>) -> Box<dyn Element> {
        let ui = self.ui_font;
        let ssh = connection.map(|c| c.ssh).unwrap_or(SshState::Disconnected);
        let badge = match ssh {
            SshState::Connected => mui::pill(theme, ui, "Connected", Tone::Success),
            SshState::Connecting => mui::pill(theme, ui, "Connecting", Tone::Info),
            SshState::Failed => mui::pill(theme, ui, "Failed", Tone::Error),
            SshState::Disconnected => mui::pill(theme, ui, "Disconnected", Tone::Neutral),
        };
        let host = &machine.connection;
        let auth = match host.authentication {
            Authentication::Key => match &host.identity_file {
                Some(key) => format!(
                    "Key {}",
                    key.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                ),
                None => "SSH key".into(),
            },
            Authentication::Agent => "SSH agent".into(),
            Authentication::Password => "Password".into(),
        };
        let mut facts = vec![endpoint_label(machine), auth];
        if let Some(jump) = &host.proxy_jump {
            facts.push(format!("via {jump}"));
        }
        self.overview_row(
            theme,
            AppIcon::Plug,
            "SSH",
            badge,
            facts.join("  ·  "),
            Some(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:edit"),
                Some(AppIcon::Settings),
                "Edit",
                Variant::Secondary,
                Size::Xs,
                true,
                act(MachineAction::Edit(machine.id.clone())),
            )),
        )
    }

    fn runtime_row(&self, theme: &Theme, connection: Option<&MachineConnection>) -> Box<dyn Element> {
        let ui = self.ui_font;
        let connected = connection.is_some_and(|c| c.ssh == SshState::Connected);
        let runtime = connection.map(|c| c.runtime).unwrap_or(RuntimeState::Unknown);
        let pending = connection.and_then(|c| c.pending);
        let badge = if !connected {
            mui::pill(theme, ui, "Offline", Tone::Neutral)
        } else {
            match (runtime, pending) {
                (_, Some(RuntimeCommand::Shutdown)) => mui::pill(theme, ui, "Stopping", Tone::Info),
                (_, Some(RuntimeCommand::Update)) => mui::pill(theme, ui, "Updating", Tone::Info),
                (_, Some(_)) => mui::pill(theme, ui, "Restarting", Tone::Info),
                (RuntimeState::Running, _) => mui::pill(theme, ui, "Healthy", Tone::Success),
                (RuntimeState::Starting, _) => mui::pill(theme, ui, "Starting", Tone::Info),
                (RuntimeState::Stopped, _) => mui::pill(theme, ui, "Stopped", Tone::Neutral),
                (RuntimeState::Failed, _) => mui::pill(theme, ui, "Error", Tone::Error),
                (RuntimeState::Unknown, _) => mui::pill(theme, ui, "Checking", Tone::Neutral),
            }
        };
        let detail = if !connected {
            "Runs projects, terminals and agents on the machine. Starts when you connect.".to_string()
        } else {
            match runtime {
                RuntimeState::Starting => "Installing and starting the runtime…".into(),
                RuntimeState::Stopped => "Stopped. Start it to use projects, terminals and agents.".into(),
                RuntimeState::Unknown => "Checking the runtime…".into(),
                _ => {
                    let mut parts = Vec::new();
                    if let Some(version) = connection.and_then(|c| c.version) {
                        parts.push(format!("Version {version}"));
                    }
                    if let Some(started) = connection.and_then(|c| c.started_at_unix_ms) {
                        parts.push(format!("started {}", started_ago(started)));
                    }
                    parts.join("  ·  ")
                }
            }
        };
        let control: Option<Box<dyn Element>> = if !connected || pending.is_some() {
            None
        } else if runtime == RuntimeState::Stopped {
            Some(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:start"),
                Some(AppIcon::Play),
                "Start",
                Variant::Primary,
                Size::Xs,
                true,
                act(MachineAction::Runtime(RuntimeCommand::Start)),
            ))
        } else if matches!(runtime, RuntimeState::Running | RuntimeState::Failed) {
            Some(self.runtime_split_button(theme))
        } else {
            None
        };
        self.overview_row(theme, AppIcon::Activity, "Workspace runtime", badge, detail, control)
    }

    /// An icon tile, title + badge over a detail line, and a trailing control.
    fn overview_row(
        &self,
        theme: &Theme,
        icon: AppIcon,
        title: &str,
        badge: Box<dyn Element>,
        detail: String,
        control: Option<Box<dyn Element>>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let title = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(mui::medium(ui, title, 13.5, theme.main_text()))
            .with_child(badge)
            .finish();
        mui::list_row(
            ui,
            theme,
            Some(mui::icon_tile(theme, icon, 32.0, None)),
            title,
            Some(detail),
            control.into_iter().collect(),
        )
    }

    /// Restart, with Update and Shut down behind a caret.
    fn runtime_split_button(&self, theme: &Theme) -> Box<dyn Element> {
        let ui = self.ui_font;
        let main = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:restart"),
            Some(AppIcon::RotateCcw),
            "Restart",
            Variant::Secondary,
            Size::Xs,
            true,
            act(MachineAction::Runtime(RuntimeCommand::Restart)),
        );
        let caret = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:runtime-menu"),
            Some(AppIcon::ChevronDown),
            "",
            Variant::Secondary,
            Size::Xs,
            true,
            act(MachineAction::ToggleRuntimeMenu),
        );
        let menu = self.machines.runtime_menu_open.then(|| {
            mui::menu_panel(
                theme,
                vec![
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:runtime:restart"),
                        Some(render_icon(AppIcon::RotateCcw, theme.sub_text(), 14.0)),
                        "Restart",
                        Some("Stops running terminals and agents"),
                        false,
                        false,
                        act(MachineAction::Runtime(RuntimeCommand::Restart)),
                    ),
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:runtime:update"),
                        Some(render_icon(AppIcon::Download, theme.sub_text(), 14.0)),
                        "Update",
                        Some("Install this app's runtime version"),
                        false,
                        false,
                        act(MachineAction::Runtime(RuntimeCommand::Update)),
                    ),
                    mui::menu_separator(theme),
                    mui::menu_row(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:runtime:shutdown"),
                        Some(render_icon(AppIcon::Power, theme.ui_red(), 14.0)),
                        "Shut down",
                        Some("Ends every session on this machine"),
                        false,
                        true,
                        act(MachineAction::Runtime(RuntimeCommand::Shutdown)),
                    ),
                ],
                260.0,
            )
        });
        mui::with_menu(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(4.0)
                .with_child(main)
                .with_child(caret)
                .finish(),
            menu,
            true,
        )
    }

    fn resource_tiles(&self, theme: &Theme, connection: Option<&MachineConnection>) -> Box<dyn Element> {
        let ui = self.ui_font;
        let metrics = connection.and_then(|c| c.metrics);
        let tile = |icon: AppIcon, label: &str, fraction: Option<f32>, detail: String| {
            let value = fraction
                .map(|f| format!("{}%", (f * 100.0).round()))
                .unwrap_or_else(|| "—".into());
            let color = fraction.map(|f| mui::usage_color(theme, f)).unwrap_or(theme.surface_3());
            Expanded::new(
                1.0,
                mui::card(
                    theme,
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(10.0)
                        .with_child(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(8.0)
                                .with_child(render_icon(icon, theme.sub_text(), 14.0))
                                .with_child(Expanded::new(1.0, mui::medium(ui, label, 12.5, theme.sub_text())).finish())
                                .with_child(mui::semibold(ui, value, 15.0, theme.main_text()))
                                .finish(),
                        )
                        .with_child(mui::meter(theme, fraction.unwrap_or(0.0), color))
                        .with_child(mui::line(ui, detail, 11.5, theme.disabled_text()))
                        .finish(),
                ),
            )
            .finish()
        };
        let ratio = |used: u64, total: u64| (total > 0).then(|| used as f32 / total as f32);
        let (cpu, memory, disk) = match metrics {
            Some(m) => (
                (Some(m.cpu_percent as f32 / 100.0), "Across all cores".to_string()),
                (
                    ratio(m.memory_used, m.memory_total),
                    format!("{} of {}", format_bytes(m.memory_used), format_bytes(m.memory_total)),
                ),
                (
                    ratio(m.disk_used, m.disk_total),
                    if m.disk_total == 0 {
                        "Unavailable".to_string()
                    } else {
                        format!("{} of {}", format_bytes(m.disk_used), format_bytes(m.disk_total))
                    },
                ),
            ),
            None => (
                (None, "Measuring…".to_string()),
                (None, "Measuring…".to_string()),
                (None, "Measuring…".to_string()),
            ),
        };
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(12.0)
            .with_child(tile(AppIcon::Cpu, "CPU", cpu.0, cpu.1))
            .with_child(tile(AppIcon::MemoryStick, "Memory", memory.0, memory.1))
            .with_child(tile(AppIcon::HardDrive, "Disk", disk.0, disk.1))
            .finish()
    }

    /// System tools as a grid of chips: version (or what's wrong) for
    /// installed ones, Install for missing ones. A chip's name opens the
    /// tool's sheet: copies, install methods, update and remove.
    fn dependencies(&self, theme: &Theme, connection: Option<&MachineConnection>) -> Box<dyn Element> {
        let ui = self.ui_font;
        let os = connection.map(|c| c.os()).unwrap_or(Os::Linux);
        // Agents have their own tab; this grid is the machine's system tools.
        let mut tools: Vec<(String, bool)> = connection
            .and_then(|c| c.environment.as_ref())
            .map(|e| e.tools.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|(tool, _)| tool_catalog::SYSTEM_TOOLS.contains(&tool.as_str()))
            .collect();
        // Required first, then missing (actionable), then alphabetical.
        tools.sort_by_key(|(tool, installed)| {
            (!REQUIRED_TOOLS.contains(&tool.as_str()), *installed, tool.clone())
        });
        let installed = tools.iter().filter(|(_, ok)| *ok).count();
        let installable = tools
            .iter()
            .filter(|(tool, ok)| !ok && tool_catalog::can_install(tool, os))
            .count();
        const COLUMNS: usize = 4;
        let mut chips: Vec<Box<dyn Element>> = Vec::new();
        for (tool, ok) in &tools {
            let report = connection.and_then(|c| c.reports.get(tool));
            let broken = report.is_some_and(|r| r.status == ToolStatus::Broken);
            let (icon, color) = if broken {
                (AppIcon::TriangleAlert, theme.ui_yellow())
            } else if *ok {
                (AppIcon::CircleCheck, theme.ui_green())
            } else {
                (AppIcon::CircleX, theme.disabled_text())
            };
            let note = if broken {
                Some("Not working".to_string())
            } else {
                report.and_then(|r| r.version.clone())
            };
            let mut names = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(1.0)
                .with_child(mui::line(ui, tool.clone(), 13.0, if *ok { theme.main_text() } else { theme.sub_text() }));
            if let Some(note) = note {
                names = names.with_child(mui::line(
                    ui,
                    note,
                    11.0,
                    if broken { theme.ui_yellow() } else { theme.disabled_text() },
                ));
            }
            let summary = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(render_icon(icon, color, 14.0))
                .with_child(Expanded::new(1.0, names.finish()).finish())
                .finish();
            let view_id = self.view_id;
            let open = tool.clone();
            let summary = warpui_core::elements::Hoverable::new(self.hover(&format!("machines:tool:{tool}")), |_| summary)
                .with_cursor(warpui_core::platform::Cursor::PointingHand)
                .on_click(move |ectx, _, _| {
                    ectx.dispatch_typed_action_for(view_id, act(MachineAction::ShowTool(open.clone())))
                })
                .finish();
            let mut chip = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(Expanded::new(1.0, summary).finish());
            if REQUIRED_TOOLS.contains(&tool.as_str()) && *ok {
                chip = chip.with_child(mui::text(ui, "Required", 11.0, theme.disabled_text()));
            }
            if report.is_some_and(|r| r.copies.len() > 1) {
                let copies = report.map(|r| r.copies.len()).unwrap_or_default();
                chip = chip.with_child(mui::text(ui, format!("{copies} copies"), 11.0, theme.disabled_text()));
            }
            if !ok && tool_catalog::can_install(tool, os) {
                chip = chip.with_child(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:install:{tool}")),
                    Some(AppIcon::Download),
                    "Install",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::InstallTool(tool.clone())),
                ));
            } else if !ok && tool_catalog::docs(tool).is_some() {
                chip = chip.with_child(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:docs:{tool}")),
                    None,
                    "Install guide",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::OpenToolDocs(tool.clone())),
                ));
            }
            chips.push(
                Container::new(
                    ConstrainedBox::new(chip.finish())
                        .with_min_height(mui::CONTROL_HEIGHT_SM)
                        .finish(),
                )
                .with_border(Border::all(1.0).with_border_color(if broken { theme.ui_yellow() } else { theme.outline() }))
                .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                    warpui_core::scene::Radius::Pixels(mui::RADIUS_CONTROL + 2.0),
                ))
                .with_padding_left(12.0)
                .with_padding_right(6.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
            );
        }
        // Equal columns that share the page width; the last row is padded
        // with empty cells so its chips keep the same width.
        let mut grid = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0);
        let mut chips = chips.into_iter().peekable();
        while chips.peek().is_some() {
            let mut row = Flex::row().with_spacing(8.0);
            for _ in 0..COLUMNS {
                row = row.with_child(Expanded::new(1.0, chips.next().unwrap_or_else(|| mui::spacer(0.0))).finish());
            }
            grid = grid.with_child(row.finish());
        }
        let mut header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(Expanded::new(1.0, mui::eyebrow(ui, theme, "System tools")).finish())
            .with_child(mui::text(ui, format!("{installed} of {} installed", tools.len()), 12.0, theme.sub_text()));
        if installable > 0 {
            header = header.with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:install-all"),
                Some(AppIcon::Download),
                if installable == 1 { "Install missing" } else { "Install all missing" },
                Variant::Secondary,
                Size::Xs,
                true,
                act(MachineAction::InstallAllTools),
            ));
        }
        let elevation = match connection.and_then(|c| c.elevation) {
            Some(Elevation::Root) => "Installs run as root on this machine.",
            Some(Elevation::PasswordlessSudo) => "Installs that need administrator rights use sudo without a password.",
            Some(Elevation::SudoWithPassword) => {
                "Installs that need administrator rights ask for your sudo password in the install window."
            }
            Some(Elevation::Unavailable) => {
                "No administrator access was found for this account: installs that need it (system packages) will fail."
            }
            None => "Installs run in a terminal on the machine, so you can follow along and answer prompts.",
        };
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(header.finish())
                    .with_padding_left(2.0)
                    .with_padding_top(28.0)
                    .with_padding_bottom(10.0)
                    .finish(),
            )
            .with_child(grid.finish())
            .with_child(mui::spacer(10.0))
            .with_child(mui::text(ui, elevation, 12.0, theme.disabled_text()))
            .finish()
    }

    // ── Projects ──────────────────────────────────────────────────────────

    fn workspaces_tab(
        &self,
        theme: &Theme,
        store: &MachinesStore,
        machine: &Machine,
        connection: Option<&MachineConnection>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let projects = store.projects_for(&machine.id);
        let sessions = connection.map(|c| c.sessions.as_slice()).unwrap_or(&[]);
        let add = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:add-project"),
            Some(AppIcon::Plus),
            "Add project",
            Variant::Primary,
            Size::Md,
            true,
            act(MachineAction::AddProject(machine.id.clone())),
        );
        if projects.is_empty() {
            return mui::empty_state(
                theme,
                ui,
                AppIcon::FolderGit,
                "No projects on this machine",
                &format!("Open a folder or clone a repository on {}. Its files stay on the machine.", machine.name()),
                Some(add),
            );
        }
        let rows = projects
            .iter()
            .map(|project| {
                let count = sessions
                    .iter()
                    .filter(|s| path_within(&s.cwd, &project.root) && matches!(s.state, dragent_host::TerminalState::Running))
                    .count();
                let mut trailing = Vec::new();
                if count > 0 {
                    trailing.push(mui::pill(theme, ui, &mui::count(count, "session", "sessions"), Tone::Success));
                }
                trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:open-project:{}", project.id)),
                    None,
                    "Open",
                    Variant::Secondary,
                    Size::Xs,
                    true,
                    act(MachineAction::OpenProject(project.id.clone())),
                ));
                trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:remove-project:{}", project.id)),
                    Some(AppIcon::Trash),
                    "",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::RemoveProject(project.id.clone())),
                ));
                mui::list_row(
                    ui,
                    theme,
                    Some(mui::icon_tile(theme, AppIcon::FolderGit, 32.0, None)),
                    mui::medium(ui, project.name(), 13.5, theme.main_text()),
                    Some(project.root.clone()),
                    trailing,
                )
            })
            .collect();
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(mui::section_header(
                ui,
                theme,
                "Projects",
                Some("Folders on this machine opened in Dragent. Files stay on the machine."),
                Some(add),
            ))
            .with_child(mui::spacer(16.0))
            .with_child(mui::list_card(theme, rows))
            .finish()
    }

    // ── Sessions ──────────────────────────────────────────────────────────

    fn sessions_tab(
        &self,
        theme: &Theme,
        store: &MachinesStore,
        machine: &Machine,
        connection: Option<&MachineConnection>,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let Some(connection) = connection.filter(|c| c.ready()) else {
            return mui::empty_state(
                theme,
                ui,
                AppIcon::Terminal,
                "Connect to see sessions",
                "Terminals and agents keep running on the machine while you're away.",
                Some(self.connect_button(theme, machine)),
            );
        };
        if connection.sessions.is_empty() {
            return mui::empty_state(
                theme,
                ui,
                AppIcon::Terminal,
                "No sessions running",
                "Open one of this machine's projects to start a terminal or an agent.",
                None,
            );
        }
        let projects = store.projects_for(&machine.id);
        let rows = connection
            .sessions
            .iter()
            .map(|session| {
                let project = projects
                    .iter()
                    .filter(|p| path_within(&session.cwd, &p.root))
                    .max_by_key(|p| p.root.len());
                let running = matches!(session.state, dragent_host::TerminalState::Running);
                let mut trailing = vec![if running {
                    mui::pill(theme, ui, "Running", Tone::Success)
                } else {
                    mui::pill(theme, ui, "Exited", Tone::Neutral)
                }];
                if running && project.is_some() {
                    trailing.push(mui::button(
                        theme,
                        ui,
                        self.view_id,
                        self.hover(&format!("machines:attach:{}", session.id)),
                        None,
                        "Open",
                        Variant::Secondary,
                        Size::Xs,
                        true,
                        act(MachineAction::AttachSession(session.id.clone())),
                    ));
                }
                trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:close-session:{}", session.id)),
                    Some(AppIcon::XClose),
                    "",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::CloseSession(session.id.clone())),
                ));
                mui::list_row(
                    ui,
                    theme,
                    Some(mui::icon_tile(theme, AppIcon::Terminal, 32.0, None)),
                    mui::medium(
                        ui,
                        project.map(|p| p.name()).unwrap_or_else(|| "Terminal".into()),
                        13.5,
                        theme.main_text(),
                    ),
                    Some(format!("{}  ·  started {}", session.cwd, started_ago(session.created_at_unix_ms))),
                    trailing,
                )
            })
            .collect();
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(mui::section_header(
                ui,
                theme,
                "Sessions",
                Some("Terminals and agents running on this machine. They keep running while you're away."),
                None,
            ))
            .with_child(mui::spacer(16.0))
            .with_child(mui::list_card(theme, rows))
            .finish()
    }

    // ── Agents ────────────────────────────────────────────────────────────

    fn agents_tab(&self, theme: &Theme, machine: &Machine, connection: Option<&MachineConnection>) -> Box<dyn Element> {
        let ui = self.ui_font;
        let Some(connection) = connection.filter(|c| c.ready()) else {
            return mui::empty_state(
                theme,
                ui,
                AppIcon::Agent,
                "Connect to detect agents",
                "Dragent checks which coding agents are installed on the machine.",
                Some(self.connect_button(theme, machine)),
            );
        };
        let tools = connection
            .environment
            .as_ref()
            .map(|e| e.tools.clone())
            .unwrap_or_default();
        // One row per agent the runtime knows (its presets plus the user's
        // own); install state comes from the tools detected on the machine.
        let mut agents: Vec<(String, String, bool)> = connection
            .agents
            .iter()
            .map(|a| {
                let command = std::iter::once(a.command.clone()).chain(a.args.iter().cloned()).collect::<Vec<_>>().join(" ");
                (a.label.clone(), command, a.preset_id.is_none())
            })
            .collect();
        if agents.is_empty() {
            agents = KNOWN_AGENTS
                .iter()
                .map(|(cli, label, _)| (label.to_string(), cli.to_string(), false))
                .collect();
        }
        let os = connection.os();
        let mut rows = Vec::new();
        for (index, (label, command, custom)) in agents.iter().enumerate() {
            let executable = command.split_whitespace().next().unwrap_or_default().to_string();
            let detected = tools.iter().find(|(tool, _)| *tool == executable).map(|(_, found)| *found);
            let report = connection.reports.get(&executable);
            let mut trailing = Vec::new();
            let mut detail = command.clone();
            match (detected, report.map(|r| r.status)) {
                (Some(true), Some(ToolStatus::Broken)) => {
                    trailing.push(mui::pill(theme, ui, "Not working", Tone::Error));
                }
                (Some(true), _) => {
                    trailing.push(mui::pill(theme, ui, "Installed", Tone::Success));
                    if let Some(report) = report {
                        let source = report
                            .copies
                            .iter()
                            .find(|copy| Some(&copy.path) == report.resolved.as_ref())
                            .map(|copy| copy.source.clone());
                        let facts: Vec<String> = report
                            .version
                            .iter()
                            .map(|version| format!("v{version}"))
                            .chain(source)
                            .collect();
                        if !facts.is_empty() {
                            detail = format!("{command}  ·  {}", facts.join("  ·  "));
                        }
                    }
                }
                (Some(false), _) if tool_catalog::can_install(&executable, os) => trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:agent-install:{index}:{executable}")),
                    Some(AppIcon::Download),
                    "Install",
                    Variant::Secondary,
                    Size::Xs,
                    true,
                    act(MachineAction::InstallTool(executable.clone())),
                )),
                (Some(false), _) => trailing.push(mui::pill(theme, ui, "Not installed", Tone::Neutral)),
                (None, _) => {}
            }
            if *custom {
                trailing.push(mui::pill(theme, ui, "Custom", Tone::Neutral));
            }
            if detected.is_some() && host_tools::valid_name(&executable) {
                trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:agent-manage:{index}:{executable}")),
                    None,
                    "Manage",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::ShowTool(executable.clone())),
                ));
            }
            let leading = match agent_icon(&executable) {
                Some(icon) => agent_tile(theme, icon),
                None => mui::icon_tile(theme, AppIcon::Terminal, 32.0, None),
            };
            rows.push(mui::list_row(
                ui,
                theme,
                Some(leading),
                mui::medium(ui, label.clone(), 13.5, theme.main_text()),
                Some(detail),
                trailing,
            ));
        }
        let input = |key: &str| {
            crate::shared_input::shared_input(
                theme,
                &self.machines.fields[key],
                crate::shared_input::SharedInputOpts {
                    height: Some(mui::CONTROL_HEIGHT),
                    radius: mui::RADIUS_CONTROL,
                    pad_left: 10.0,
                    ..Default::default()
                },
            )
        };
        let add = mui::card(
            theme,
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(16.0)
                .with_child(mui::section_header(
                    ui,
                    theme,
                    "Add a custom agent",
                    Some("Any command-line agent installed on this machine."),
                    None,
                ))
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::End)
                        .with_spacing(12.0)
                        .with_child(Expanded::new(1.0, mui::field(ui, theme, "Name", input("agent_name"), None)).finish())
                        .with_child(Expanded::new(1.0, mui::field(ui, theme, "Command", input("agent_command"), None)).finish())
                        .with_child(mui::button(
                            theme,
                            ui,
                            self.view_id,
                            self.hover("machines:add-agent"),
                            Some(AppIcon::Plus),
                            "Add agent",
                            Variant::Secondary,
                            Size::Md,
                            true,
                            act(MachineAction::AddAgent),
                        ))
                        .finish(),
                )
                .finish(),
        );
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(mui::section_header(
                ui,
                theme,
                "Agents",
                Some("Coding agents you can start in this machine's projects."),
                None,
            ))
            .with_child(mui::spacer(16.0))
            .with_child(mui::list_card(theme, rows))
            .with_child(mui::spacer(20.0))
            .with_child(add)
            .finish()
    }

    fn connect_button(&self, theme: &Theme, machine: &Machine) -> Box<dyn Element> {
        mui::button(
            theme,
            self.ui_font,
            self.view_id,
            self.hover("machines:empty-connect"),
            Some(AppIcon::Plug),
            "Connect",
            Variant::Primary,
            Size::Md,
            true,
            act(MachineAction::Connect(machine.id.clone())),
        )
    }

    pub(super) fn render_machine_confirm(
        &self,
        theme: &Theme,
        store: &MachinesStore,
        confirm: &Confirmation,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let (icon, title, body, destructive) = match confirm {
            Confirmation::DeleteBlocked => (
                AppIcon::FolderGit,
                "This machine still has projects",
                "Remove its projects from the Projects tab first, then delete the machine.".to_string(),
                false,
            ),
            Confirmation::Delete(id) => (
                AppIcon::Trash,
                "Delete this machine?",
                format!(
                    "\"{}\" and its saved credentials are removed from this device. Nothing on the machine is deleted.",
                    store.machine(id).map(|m| m.name()).unwrap_or_default()
                ),
                true,
            ),
        };
        let actions = if destructive {
            vec![
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:confirm-cancel"),
                    None,
                    "Cancel",
                    Variant::Secondary,
                    Size::Md,
                    true,
                    act(MachineAction::CancelConfirm),
                ),
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:confirm-delete"),
                    Some(AppIcon::Trash),
                    "Delete machine",
                    Variant::Destructive,
                    Size::Md,
                    true,
                    act(MachineAction::ConfirmDelete),
                ),
            ]
        } else {
            vec![mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:confirm-close"),
                None,
                "Got it",
                Variant::Primary,
                Size::Md,
                true,
                act(MachineAction::CancelConfirm),
            )]
        };
        mui::dialog(
            theme,
            ui,
            self.view_id,
            self.hover("machines:confirm-x"),
            &self.machines.confirm_scroll,
            mui::dialog_title(theme, ui, icon, title, None),
            mui::text(ui, body, 13.0, theme.sub_text()),
            Some(mui::dialog_footer(None, actions)),
            460.0,
            Some(act(MachineAction::CancelConfirm)),
        )
    }
}

/// The brand mark for a known agent executable.
fn agent_icon(executable: &str) -> Option<AppIcon> {
    Some(match executable {
        "claude" => AppIcon::ClaudeLogo,
        "codex" => AppIcon::ChatgptLogo,
        "gemini" => AppIcon::GeminiLogo,
        "opencode" => AppIcon::OpenCodeLogo,
        "amp" => AppIcon::AmpLogo,
        "copilot" => AppIcon::CopilotLogo,
        _ => return None,
    })
}

/// Agent brand marks on a quiet tile.
fn agent_tile(theme: &Theme, icon: AppIcon) -> Box<dyn Element> {
    Container::new(mui::sized(render_icon(icon, theme.main_text(), 16.0), 32.0, 32.0))
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(warpui_core::scene::Radius::Pixels(9.0)))
        .finish()
}

/// Session cwd lies inside a project root (string paths on the remote host).
pub(crate) fn path_within(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

fn started_ago(unix_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(unix_ms);
    let seconds = now.saturating_sub(unix_ms) / 1000;
    match seconds {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::path_within;

    #[test]
    fn session_belongs_to_project_root_only_on_path_boundaries() {
        assert!(path_within("/home/dev/app", "/home/dev/app"));
        assert!(path_within("/home/dev/app/src", "/home/dev/app/"));
        assert!(!path_within("/home/dev/app2", "/home/dev/app"));
    }
}
