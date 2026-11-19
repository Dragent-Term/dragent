use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::SettingsEvent;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_agent(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::ToggleAgenticWorkspace => {
                self.app_settings.agent.agentic_workspace_enabled =
                    !self.app_settings.agent.agentic_workspace_enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectApprovalMode(mode) => {
                self.app_settings.agent.set_approval_mode(*mode);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectIdleSleepMinutes(minutes) => {
                self.app_settings.agent.idle_sleep_minutes = *minutes;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleAutoRunSafeCommands => {
                self.app_settings.agent.auto_run_safe_commands =
                    !self.app_settings.agent.auto_run_safe_commands;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleProactiveErrorScanning => {
                self.app_settings.agent.proactive_error_scanning =
                    !self.app_settings.agent.proactive_error_scanning;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleAutoFixOnFailure => {
                self.app_settings.agent.auto_fix_on_failure =
                    !self.app_settings.agent.auto_fix_on_failure;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectContextScope(scope) => {
                self.app_settings.agent.context_scope = *scope;
                self.open_picker = None;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            // ── Agents settings ─────────────────────────────
            SettingsAction::SelectDefaultAgent(agent) => {
                self.app_settings.agent.default_agent = agent.clone();
                self.open_picker = None;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleDisabledAgent(agent) => {
                self.app_settings.agent.toggle_disabled_agent(agent);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleWorkspaceTrust => {
                self.app_settings.agent.workspace_trust_enabled =
                    !self.app_settings.agent.workspace_trust_enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleConfirmCloseIdle => {
                self.app_settings.agent.confirm_close_idle_tab =
                    !self.app_settings.agent.confirm_close_idle_tab;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::ToggleServerIsolation => {
                self.app_settings.agent.terminal_server_isolation =
                    !self.app_settings.agent.terminal_server_isolation;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            SettingsAction::SelectDefaultShell(shell) => {
                self.editor_settings.terminal.default_shell = shell.clone();
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::ToggleAgentSkill => {
                let outcome = crate::cli::agent_skill::set_enabled(!crate::cli::agent_skill::enabled());
                let shown = |path: &std::path::Path| {
                    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(|rest| rest.to_path_buf())) {
                        Some(rest) => format!("~/{}", rest.display()),
                        None => path.display().to_string(),
                    }
                };
                self.agent_skill_note = outcome.kept.first().map(|path| {
                    format!("Your own skill at {} is kept as it is.", shown(path))
                });
            }
            #[cfg(not(target_family = "wasm"))]
            SettingsAction::InstallCliInShell => {
                self.cli_setup_error = match crate::cli::shell_setup::Setup::current() {
                    Some(setup) => setup.install(&crate::cli::user_bin_dir()).err(),
                    None => Some("Your shell isn't one Dragent can set up.".into()),
                };
            }
            _ => {}
        }
    }
}
