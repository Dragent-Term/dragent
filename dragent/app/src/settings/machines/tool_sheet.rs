//! The tool sheet (Settings → Machines → a tool or agent): whether it works
//! and which version, the copy Dragent uses (any copy found, or a command),
//! and installing, updating or removing it.
use super::{MachineAction, SettingsView};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::settings::machines_store::MachineConnection;
use crate::settings::tool_catalog;
use crate::settings::SettingsAction;
use crate::shared_input::{shared_input, SharedInputOpts};
use crate::theme::Theme;
use dragent_host::development::{ToolPin, ToolReport, ToolStatus};
use warpui::elements::{CrossAxisAlignment, Element, Expanded, Flex, ParentElement};

const SHEET_WIDTH: f32 = 600.0;

fn act(action: MachineAction) -> SettingsAction {
    SettingsAction::Machine(action)
}

impl SettingsView {
    pub(super) fn render_tool_sheet(
        &self,
        theme: &Theme,
        connection: &MachineConnection,
        tool: &str,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let os = connection.os();
        let label = tool_catalog::label(tool);
        let report = connection.reports.get(tool);
        let installed = connection
            .environment
            .as_ref()
            .and_then(|e| e.tools.iter().find(|(name, _)| name == tool))
            .is_some_and(|(_, found)| *found);

        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(20.0)
            .with_child(self.tool_status(theme, tool, report, installed));
        if let Some(error) = &connection.tool_error {
            body = body.with_child(mui::notice(theme, ui, Tone::Error, "The machine didn't accept that", Some(error.clone()), None));
        }
        if let Some(report) = report.filter(|r| !r.copies.is_empty() || r.pin.is_some()) {
            body = body.with_child(self.tool_copies(theme, tool, report));
        }
        body = body.with_child(self.tool_maintenance(theme, tool, report, installed, os));

        let mut left = Flex::row().with_spacing(8.0).with_child(mui::button(
            theme,
            ui,
            self.view_id,
            self.hover("machines:tool-check"),
            Some(AppIcon::RefreshCw),
            "Check again",
            Variant::Secondary,
            Size::Md,
            true,
            act(MachineAction::CheckTools),
        ));
        if tool_catalog::docs(tool).is_some() {
            left = left.with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:tool-docs"),
                None,
                "Install guide",
                Variant::Ghost,
                Size::Md,
                true,
                act(MachineAction::OpenToolDocs(tool.to_string())),
            ));
        }
        let footer = mui::dialog_footer(
            Some(left.finish()),
            vec![mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:tool-close"),
                None,
                "Done",
                Variant::Primary,
                Size::Md,
                true,
                act(MachineAction::CloseTool),
            )],
        );
        let kind = if tool_catalog::is_agent(tool) { "Agent" } else { "System tool" };
        mui::dialog(
            theme,
            ui,
            self.view_id,
            self.hover("machines:tool-x"),
            &self.machines.tool_sheet_scroll,
            mui::dialog_title(
                theme,
                ui,
                if tool_catalog::is_agent(tool) { AppIcon::Agent } else { AppIcon::Terminal },
                &label,
                Some(&format!("{kind} · {tool}")),
            ),
            body.finish(),
            Some(footer),
            SHEET_WIDTH,
            Some(act(MachineAction::CloseTool)),
        )
    }

    /// Works / not working / missing, with version, source and path.
    fn tool_status(&self, theme: &Theme, tool: &str, report: Option<&ToolReport>, installed: bool) -> Box<dyn Element> {
        let ui = self.ui_font;
        let status = report.map(|r| r.status).unwrap_or(if installed { ToolStatus::Ready } else { ToolStatus::Missing });
        let (icon, color, title) = match status {
            ToolStatus::Ready => (AppIcon::CircleCheck, theme.ui_green(), "Working"),
            ToolStatus::Broken => (AppIcon::TriangleAlert, theme.ui_yellow(), "Installed, but not working"),
            ToolStatus::Missing => (AppIcon::CircleX, theme.disabled_text(), "Not installed"),
        };
        let mut facts: Vec<String> = Vec::new();
        if let Some(report) = report {
            if let Some(version) = &report.version {
                facts.push(format!("Version {version}"));
            }
            let source = report
                .copies
                .iter()
                .find(|copy| Some(&copy.path) == report.resolved.as_ref())
                .map(|copy| copy.source.clone());
            if let Some(source) = source {
                facts.push(format!("installed with {source}"));
            }
            if matches!(report.pin, Some(ToolPin::Command(_))) {
                facts.push("runs your command".into());
            }
        } else if installed {
            facts.push("Update this machine's workspace runtime to see versions and copies.".into());
        }
        let mut text = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(3.0)
            .with_child(mui::medium(ui, title, 13.5, theme.main_text()));
        if !facts.is_empty() {
            text = text.with_child(mui::text(ui, facts.join(" · "), 12.0, theme.sub_text()));
        }
        if let Some(resolved) = report.and_then(|r| r.resolved.clone()) {
            text = text.with_child(mui::line(ui, resolved, 11.5, theme.disabled_text()));
        }
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(10.0)
                    .with_child(render_icon(icon, color, 18.0))
                    .with_child(Expanded::new(1.0, text.finish()).finish())
                    .finish(),
            );
        if let Some(detail) = report.and_then(|r| r.detail.clone()) {
            let tone = if status == ToolStatus::Broken { Tone::Error } else { Tone::Warning };
            let heading = if status == ToolStatus::Broken {
                format!("{} can't start", tool_catalog::label(tool))
            } else {
                "The chosen copy is missing".to_string()
            };
            column = column.with_child(mui::notice(theme, ui, tone, &heading, Some(detail), None));
        }
        mui::card(theme, column.finish())
    }

    /// The copy Dragent uses: the first on the PATH, another copy, or a
    /// command typed here.
    fn tool_copies(&self, theme: &Theme, tool: &str, report: &ToolReport) -> Box<dyn Element> {
        let ui = self.ui_font;
        let pinned_path = match &report.pin {
            Some(ToolPin::Path(path)) => Some(path.as_str()),
            _ => None,
        };
        let command_pin = matches!(report.pin, Some(ToolPin::Command(_)));
        let mut rows: Vec<Box<dyn Element>> = Vec::new();
        let first = report.copies.first().map(|copy| copy.path.clone());
        rows.push(mui::menu_row(
            theme,
            ui,
            self.view_id,
            self.hover(&format!("machines:copy-default:{tool}")),
            None,
            "The first one on the PATH",
            first.as_deref().or(Some("None found")),
            report.pin.is_none(),
            false,
            act(MachineAction::UseCopy { tool: tool.to_string(), path: None }),
        ));
        for (index, copy) in report.copies.iter().enumerate() {
            let detail = if copy.real_path != copy.path {
                format!("{} · {}", copy.source, copy.real_path)
            } else {
                copy.source.clone()
            };
            rows.push(mui::menu_row(
                theme,
                ui,
                self.view_id,
                self.hover(&format!("machines:copy:{tool}:{index}")),
                None,
                &copy.path,
                Some(&detail),
                pinned_path == Some(copy.path.as_str()),
                false,
                act(MachineAction::UseCopy {
                    tool: tool.to_string(),
                    path: Some(copy.path.clone()),
                }),
            ));
        }
        let command = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Expanded::new(
                    1.0,
                    shared_input(
                        theme,
                        &self.machines.fields["tool_command"],
                        SharedInputOpts {
                            height: Some(mui::CONTROL_HEIGHT),
                            radius: mui::RADIUS_CONTROL,
                            pad_left: 10.0,
                            ..Default::default()
                        },
                    ),
                )
                .finish(),
            )
            .with_child(mui::button(
                theme,
                ui,
                self.view_id,
                self.hover("machines:tool-command"),
                None,
                if command_pin { "Update command" } else { "Use command" },
                Variant::Secondary,
                Size::Md,
                true,
                act(MachineAction::UseToolCommand(tool.to_string())),
            ))
            .finish();
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(mui::section_header(
                ui,
                theme,
                "Copy Dragent uses",
                Some("Agents Dragent starts on this machine run this copy."),
                None,
            ))
            .with_child(mui::list_card(theme, rows))
            .with_child(mui::field(
                ui,
                theme,
                if command_pin { "Command (in use)" } else { "Or run a command instead" },
                command,
                Some("Its words replace the tool's name when Dragent starts it. Leave empty and choose a copy above to stop using it."),
            ))
            .finish()
    }

    /// Install methods when missing or broken; Update and Uninstall when
    /// installed.
    fn tool_maintenance(
        &self,
        theme: &Theme,
        tool: &str,
        report: Option<&ToolReport>,
        installed: bool,
        os: tool_catalog::Os,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let label = tool_catalog::label(tool);
        let methods = tool_catalog::methods(tool, os);
        let broken = report.is_some_and(|r| r.status == ToolStatus::Broken);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0);
        if installed {
            let update = report.and_then(|r| tool_catalog::update_command(tool, r, os));
            let uninstall = report.and_then(|r| tool_catalog::uninstall_command(tool, r, os));
            let mut buttons = Flex::row().with_spacing(8.0);
            if update.is_some() {
                buttons = buttons.with_child(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:tool-update"),
                    Some(AppIcon::Download),
                    "Update",
                    Variant::Secondary,
                    Size::Md,
                    true,
                    act(MachineAction::UpdateTool(tool.to_string())),
                ));
            }
            if uninstall.is_some() {
                buttons = buttons.with_child(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover("machines:tool-uninstall"),
                    Some(AppIcon::Trash),
                    "Uninstall",
                    Variant::Destructive,
                    Size::Md,
                    true,
                    act(MachineAction::UninstallTool(tool.to_string())),
                ));
            }
            let note = match (&update, &uninstall, report) {
                (None, None, Some(_)) => Some(format!(
                    "This copy wasn't installed in a way Dragent manages. Update or remove {label} the way it was installed."
                )),
                (None, None, None) => Some("Update this machine's workspace runtime to update or remove tools here.".to_string()),
                (Some(_), None, _) => Some(format!(
                    "Remove {label} the way it was installed; Dragent doesn't uninstall this kind of install."
                )),
                _ => None,
            };
            column = column
                .with_child(mui::section_header(ui, theme, "Maintain", Some("Runs in a terminal on the machine."), None))
                .with_child(buttons.finish());
            if let Some(note) = note {
                column = column.with_child(mui::text(ui, note, 12.0, theme.disabled_text()));
            }
        }
        if !installed || broken {
            column = column.with_child(mui::section_header(
                ui,
                theme,
                if broken { "Reinstall" } else { "Install" },
                Some("Runs in a terminal on the machine, so you can follow along and answer prompts."),
                None,
            ));
            if methods.is_empty() {
                column = column.with_child(mui::text(
                    ui,
                    format!("Dragent can't install {label} automatically. Follow its install guide, then choose Check again."),
                    12.0,
                    theme.disabled_text(),
                ));
            }
            let mut rows: Vec<Box<dyn Element>> = Vec::new();
            for (index, method) in methods.iter().enumerate() {
                let mut trailing = Vec::new();
                if method.recommended {
                    trailing.push(mui::pill(theme, ui, "Recommended", Tone::Info));
                }
                trailing.push(mui::button(
                    theme,
                    ui,
                    self.view_id,
                    self.hover(&format!("machines:tool-method:{index}")),
                    Some(AppIcon::Download),
                    "Install",
                    if method.recommended { Variant::Primary } else { Variant::Secondary },
                    Size::Xs,
                    true,
                    act(MachineAction::InstallWith {
                        tool: tool.to_string(),
                        method: index,
                    }),
                ));
                rows.push(mui::list_row(
                    ui,
                    theme,
                    None,
                    mui::medium(ui, method.label, 13.0, theme.main_text()),
                    Some(command_preview(&method.install)),
                    trailing,
                ));
            }
            if !rows.is_empty() {
                column = column.with_child(mui::list_card(theme, rows));
            }
        }
        column.finish()
    }
}

/// A one-line hint of what an install runs.
fn command_preview(command: &str) -> String {
    let inner = command
        .strip_prefix("sh -c '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(command);
    // The Linux script picks whichever package manager the machine has.
    if inner.contains("elif command -v dnf") {
        if let Some(rest) = inner.split("apt-get install -y ").nth(1) {
            let packages = rest.split(';').next().unwrap_or_default().trim();
            return format!("{packages}, with the machine's package manager (apt, dnf, zypper, pacman or apk)");
        }
    }
    // Homebrew runs through the path the script found.
    if let Some(at) = inner.rfind("\"$brew\" ") {
        let rest = &inner[at + "\"$brew\" ".len()..];
        return format!("brew {}", rest.split(';').next().unwrap_or(rest).trim());
    }
    let key = ["npm install -g", "brew install", "rustup", "curl"]
        .iter()
        .filter_map(|needle| inner.find(needle))
        .min()
        .map(|at| &inner[at..])
        .unwrap_or(inner);
    key.split([';', '\n']).next().unwrap_or(key).trim().chars().take(90).collect()
}

#[cfg(test)]
mod tests {
    use super::command_preview;

    #[test]
    fn previews_show_the_install_itself() {
        let npm = crate::settings::tool_catalog::methods("codex", crate::settings::tool_catalog::Os::Linux)
            .remove(0)
            .install;
        assert_eq!(command_preview(&npm), "npm install -g @openai/codex");
        assert_eq!(
            command_preview("curl -fsSL https://claude.ai/install.sh | bash"),
            "curl -fsSL https://claude.ai/install.sh | bash"
        );
        let rg = crate::settings::tool_catalog::methods("rg", crate::settings::tool_catalog::Os::Linux)
            .remove(0)
            .install;
        assert!(command_preview(&rg).starts_with("ripgrep, with the machine's package manager"), "{}", command_preview(&rg));
        let mac = crate::settings::tool_catalog::methods("rg", crate::settings::tool_catalog::Os::Mac)
            .remove(0)
            .install;
        assert_eq!(command_preview(&mac), "brew install ripgrep");
    }
}
