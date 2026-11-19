//! The Add machine / Edit machine dialog: a name, how to connect (from the
//! user's SSH config or entered manually), authentication, optional advanced
//! routing, and inline Test connection feedback.
use super::{MachineAction, MachineForm, Resolution, SettingsView, TestState};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Segment, Size, Tone, Variant};
use crate::settings::SettingsAction;
use crate::shared_input::{shared_input, SharedInputOpts};
use crate::theme::Theme;
use dragent_host::remote::{Authentication, ResolvedSsh};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element, EventHandler,
    Expanded, Flex, ParentElement,
};
use warpui_core::scene::{CornerRadius, Radius};

const FORM_WIDTH: f32 = 560.0;

fn act(action: MachineAction) -> SettingsAction {
    SettingsAction::Machine(action)
}

impl SettingsView {
    fn machine_input(&self, theme: &Theme, key: &str) -> Box<dyn Element> {
        shared_input(
            theme,
            &self.machines.fields[key],
            SharedInputOpts {
                height: Some(mui::CONTROL_HEIGHT),
                radius: mui::RADIUS_CONTROL,
                pad_left: 11.0,
                pad_right: 11.0,
                ..Default::default()
            },
        )
    }

    pub(super) fn render_machine_form(
        &self,
        theme: &Theme,
        form: &MachineForm,
        app: &warpui::AppContext,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let alias = self.machines.value("alias", app);
        let ready = form.ready(&alias);
        let editing = form.id.is_some();

        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(20.0)
            .with_child(mui::field(
                ui,
                theme,
                "Name",
                self.machine_input(theme, "name"),
                Some("Shown in the sidebar and the project location menu."),
            ))
            .with_child(mui::field(ui, theme, "Connect using", self.source_toggle(theme, form.config_mode), None));

        if form.config_mode {
            body = body.with_child(self.host_picker(theme, form, app));
            match &form.resolution {
                Resolution::Resolving => {
                    body = body.with_child(mui::notice(
                        theme,
                        ui,
                        Tone::Info,
                        "Reading your SSH config…",
                        None,
                        None,
                    ))
                }
                Resolution::Failed(error) => {
                    body = body.with_child(mui::notice(
                        theme,
                        ui,
                        Tone::Error,
                        "OpenSSH couldn't resolve this host",
                        Some(error.clone()),
                        None,
                    ))
                }
                Resolution::Resolved { host, .. } => body = body.with_child(self.connection_details(theme, host)),
                Resolution::Idle => {}
            }
        } else {
            body = body.with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(12.0)
                    .with_child(
                        Expanded::new(1.0, mui::field(ui, theme, "Host", self.machine_input(theme, "host"), None))
                            .finish(),
                    )
                    .with_child(
                        ConstrainedBox::new(mui::field(ui, theme, "Port", self.machine_input(theme, "port"), None))
                            .with_width(96.0)
                            .finish(),
                    )
                    .finish(),
            );
            body = body.with_child(mui::field(ui, theme, "Username", self.machine_input(theme, "user"), None));
        }

        if !form.config_mode || matches!(form.resolution, Resolution::Resolved { .. }) {
            body = body.with_child(self.authentication_fields(theme, form));
        }
        body = body.with_child(self.advanced_settings(theme, form));
        if let Some(feedback) = self.test_feedback(theme, form) {
            body = body.with_child(feedback);
        }
        if let Some(error) = &form.save_error {
            body = body.with_child(mui::notice(
                theme,
                ui,
                Tone::Error,
                "Couldn't save this machine",
                Some(error.clone()),
                None,
            ));
        }

        let primary_label = if form.saving {
            "Saving…"
        } else if !ready {
            match form.resolution {
                Resolution::Resolving => "Resolving…",
                Resolution::Failed(_) => "Try again",
                _ => "Continue",
            }
        } else if editing {
            "Save changes"
        } else {
            "Add machine"
        };
        let test = ready.then(|| {
            mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:test"),
                Some(AppIcon::Plug),
                if matches!(form.test, TestState::Testing) { "Testing…" } else { "Test connection" },
                Variant::Secondary,
                Size::Md,
                !matches!(form.test, TestState::Testing) && !form.saving,
                act(MachineAction::Test),
            )
        });
        let footer = mui::dialog_footer(
            test,
            vec![
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:cancel"),
                    None,
                    "Cancel",
                    Variant::Secondary,
                    Size::Md,
                    !form.saving,
                    act(MachineAction::CloseForm),
                ),
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:submit"),
                    None,
                    primary_label,
                    Variant::Primary,
                    Size::Md,
                    !form.saving && !matches!(form.resolution, Resolution::Resolving),
                    act(MachineAction::Submit),
                ),
            ],
        );

        let view_id = self.view_id;
        let body = EventHandler::new(body.finish())
            .on_keydown(move |ctx, _, keystroke| match keystroke.key.as_str() {
                "enter" => {
                    ctx.dispatch_typed_action_for(view_id, act(MachineAction::Submit));
                    DispatchEventResult::StopPropagation
                }
                "escape" => {
                    ctx.dispatch_typed_action_for(view_id, act(MachineAction::CloseForm));
                    DispatchEventResult::StopPropagation
                }
                _ => DispatchEventResult::PropagateToParent,
            })
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(view_id, act(MachineAction::CloseMenus));
                DispatchEventResult::PropagateToParent
            })
            .finish();

        mui::dialog(
            theme,
            ui,
            self.view_id,
            self.hover("machines:form-close"),
            &form.scroll,
            mui::dialog_title(
                theme,
                ui,
                AppIcon::Server,
                if editing { "Edit machine" } else { "Add machine" },
                Some("Connect a server over SSH to run projects and agents on it."),
            ),
            body,
            Some(footer),
            FORM_WIDTH,
            (!form.saving).then(|| act(MachineAction::CloseForm)),
        )
    }

    fn source_toggle(&self, theme: &Theme, config: bool) -> Box<dyn Element> {
        mui::segmented(
            theme,
            self.ui_font,
            self.view_id,
            vec![
                Segment {
                    hover: self.hover("machines:mode:true"),
                    icon: Some(AppIcon::File),
                    label: "SSH config".into(),
                    selected: config,
                    action: act(MachineAction::FormMode { config: true }),
                },
                Segment {
                    hover: self.hover("machines:mode:false"),
                    icon: Some(AppIcon::Edit),
                    label: "Enter manually".into(),
                    selected: !config,
                    action: act(MachineAction::FormMode { config: false }),
                },
            ],
            true,
        )
    }

    /// A combobox over the hosts in the user's SSH config.
    fn host_picker(&self, theme: &Theme, form: &MachineForm, app: &warpui::AppContext) -> Box<dyn Element> {
        let ui = self.ui_font;
        let input = shared_input(
            theme,
            &self.machines.fields["alias"],
            SharedInputOpts {
                leading_icon: Some(crate::shared_input::SharedInputIcon {
                    icon: AppIcon::Server,
                    size: 14.0,
                }),
                height: Some(mui::CONTROL_HEIGHT),
                radius: mui::RADIUS_CONTROL,
                pad_left: 11.0,
                pad_right: 3.0,
                trailing: Some(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:host-menu"),
                    Some(if form.host_menu_open { AppIcon::ChevronUp } else { AppIcon::ChevronDown }),
                    "",
                    Variant::Ghost,
                    Size::Xs,
                    true,
                    act(MachineAction::ToggleHostMenu),
                )),
                ..Default::default()
            },
        );
        let menu = form.host_menu_open.then(|| {
            let typed = self.machines.value("alias", app).to_lowercase();
            let hosts: Vec<String> = crate::workspace::ssh::ssh_config_hosts()
                .into_iter()
                .map(|h| h.display())
                .filter(|h| h.to_lowercase().contains(&typed))
                .take(12)
                .collect();
            let mut rows = vec![mui::menu_heading(ui, theme, "From your SSH config")];
            if hosts.is_empty() {
                rows.push(
                    Container::new(mui::text(
                        ui,
                        "No saved hosts match. Continue to resolve this hostname.",
                        12.0,
                        theme.sub_text(),
                    ))
                    .with_padding_left(10.0)
                    .with_padding_right(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(8.0)
                    .finish(),
                );
            }
            for host in hosts {
                rows.push(mui::menu_row(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:host:{host}")),
                    Some(render_icon(AppIcon::Server, theme.sub_text(), 14.0)),
                    &host,
                    None,
                    false,
                    false,
                    act(MachineAction::PickHost(host.clone())),
                ));
            }
            mui::menu_panel(theme, rows, FORM_WIDTH - 2.0 * mui::DIALOG_PAD)
        });
        mui::field(
            ui,
            theme,
            "Host",
            mui::with_menu(input, menu, false),
            Some("A Host entry from ~/.ssh/config, or any hostname OpenSSH can resolve."),
        )
    }

    /// What OpenSSH resolved for the chosen host.
    fn connection_details(&self, theme: &Theme, host: &ResolvedSsh) -> Box<dyn Element> {
        let ui = self.ui_font;
        let mut rows = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(9.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::CircleCheck, theme.ui_green(), 14.0))
                    .with_child(mui::medium(ui, "Resolved by OpenSSH", 12.5, theme.main_text()))
                    .finish(),
            )
            .with_child(mui::detail_row(ui, theme, "Host", host.hostname.clone()))
            .with_child(mui::detail_row(ui, theme, "Port", host.port.to_string()))
            .with_child(mui::detail_row(
                ui,
                theme,
                "Username",
                host.user.clone().unwrap_or_else(|| "Not specified".into()),
            ))
            .with_child(mui::detail_row(
                ui,
                theme,
                "Jump host",
                host.proxy_jump.clone().unwrap_or_else(|| "None".into()),
            ));
        if let Some(command) = &host.proxy_command {
            rows = rows.with_child(mui::detail_row(ui, theme, "Proxy command", command.clone()));
        }
        rows = rows.with_child(mui::detail_row(
            ui,
            theme,
            "Agent forwarding",
            if host.forward_agent { "On" } else { "Off" }.into(),
        ));
        mui::sunken(theme, rows.finish())
    }

    fn authentication_fields(&self, theme: &Theme, form: &MachineForm) -> Box<dyn Element> {
        let ui = self.ui_font;
        let choice = |auth: Authentication, icon: AppIcon, label: &str| Segment {
            hover: self.hover(&format!("machines:auth:{auth:?}")),
            icon: Some(icon),
            label: label.into(),
            selected: form.auth == auth,
            action: act(MachineAction::Auth(auth)),
        };
        let selector = mui::segmented(
            theme,
            ui,
            self.view_id,
            vec![
                choice(Authentication::Key, AppIcon::Key, "SSH key"),
                choice(Authentication::Agent, AppIcon::User, "SSH agent"),
                choice(Authentication::Password, AppIcon::Lock, "Password"),
            ],
            true,
        );
        let mut section = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(16.0)
            .with_child(mui::field(ui, theme, "Authentication", selector, None));
        section = match form.auth {
            Authentication::Key => section
                .with_child(mui::field(
                    ui,
                    theme,
                    "Private key",
                    self.machine_input(theme, "key"),
                    form.config_mode.then_some("Leave blank to use the key from your SSH config."),
                ))
                .with_child(mui::field(
                    ui,
                    theme,
                    "Passphrase",
                    self.machine_input(theme, "passphrase"),
                    Some(if form.id.is_some() {
                        "Optional. Leave blank to keep the saved passphrase."
                    } else {
                        "Optional. Stored in your system keychain."
                    }),
                )),
            Authentication::Password => section.with_child(mui::field(
                ui,
                theme,
                "Password",
                self.machine_input(theme, "password"),
                Some(if form.id.is_some() {
                    "Leave blank to keep the saved password."
                } else {
                    "Stored in your system keychain."
                }),
            )),
            Authentication::Agent => section.with_child(mui::text(
                ui,
                "Uses the keys loaded in your local SSH agent. Nothing is stored by Dragent.",
                12.0,
                theme.sub_text(),
            )),
        };
        section.finish()
    }

    /// Collapsible jump host, agent forwarding and SSH config file.
    fn advanced_settings(&self, theme: &Theme, form: &MachineForm) -> Box<dyn Element> {
        let ui = self.ui_font;
        let trigger = mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:advanced"),
            Some(if form.advanced { AppIcon::ChevronDown } else { AppIcon::ChevronRight }),
            "Advanced",
            Variant::Ghost,
            Size::Xs,
            true,
            act(MachineAction::ToggleAdvanced),
        );
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(16.0)
            .with_child(trigger);
        if form.advanced {
            let mut inner = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(16.0);
            if !form.config_mode {
                inner = inner
                    .with_child(mui::field(
                        ui,
                        theme,
                        "Jump host",
                        self.machine_input(theme, "jump"),
                        Some("Optional bastion to connect through, such as user@bastion:2222."),
                    ))
                    .with_child(mui::settings_row(
                        ui,
                        theme,
                        "Forward SSH agent",
                        Vec::new(),
                        Some(mui::text(
                            ui,
                            "Lets the machine use your local SSH keys. Enable only for hosts you trust.",
                            12.0,
                            theme.sub_text(),
                        )),
                        Some(mui::switch(
                            theme,
                            self.view_id,
                            self.hover("machines:forward"),
                            form.forward,
                            act(MachineAction::ToggleForward),
                        )),
                    ));
            }
            inner = inner.with_child(mui::field(
                ui,
                theme,
                "SSH config file",
                self.machine_input(theme, "config"),
                Some("Optional. Use a file other than ~/.ssh/config for this machine."),
            ));
            column = column.with_child(mui::sunken(theme, inner.finish()));
        }
        column.finish()
    }

    /// Test connection result as a banner, with the SSH log behind a toggle.
    fn test_feedback(&self, theme: &Theme, form: &MachineForm) -> Option<Box<dyn Element>> {
        let ui = self.ui_font;
        let banner = match &form.test {
            TestState::Idle => return None,
            TestState::Testing => mui::notice(theme, ui, Tone::Info, "Testing the connection…", None, None),
            TestState::Passed { latency_ms } => mui::notice(
                theme,
                ui,
                Tone::Success,
                "Connection works",
                Some(format!("Reached the machine in {latency_ms} ms.")),
                None,
            ),
            TestState::Failed { message, logs } => {
                let toggle = (!logs.is_empty()).then(|| {
                    mui::button(
                        theme,
                        ui,
                        self.view_id,
                        self.hover("machines:logs"),
                        Some(if form.show_logs { AppIcon::ChevronUp } else { AppIcon::ChevronDown }),
                        if form.show_logs { "Hide log" } else { "Show log" },
                        Variant::Ghost,
                        Size::Xs,
                        true,
                        act(MachineAction::ToggleTestLogs),
                    )
                });
                mui::notice(theme, ui, Tone::Error, "Couldn't connect", Some(message.clone()), toggle)
            }
            TestState::NewHostKey(key) => mui::host_key_notice(
                theme,
                ui,
                "this machine",
                key,
                mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:trust-key"),
                    None,
                    "Trust and test",
                    Variant::Primary,
                    Size::Xs,
                    true,
                    act(MachineAction::TrustTestedHostKey),
                ),
            ),
        };
        let logs = match &form.test {
            TestState::Failed { logs, .. } if form.show_logs && !logs.is_empty() => Some(logs.join("\n")),
            _ => None,
        };
        let Some(logs) = logs else {
            return Some(banner);
        };
        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(8.0)
                .with_child(banner)
                .with_child(
                    Container::new(mui::text(ui, logs, 11.5, theme.sub_text()))
                        .with_background_color(theme.surface_1())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                        .with_uniform_padding(12.0)
                        .finish(),
                )
                .finish(),
        )
    }
}
