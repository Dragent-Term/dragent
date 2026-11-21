use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::setup::reset_for_onboarding;
use crate::workspace::default_agent_command_config;
use warpui::{UpdateView, ViewContext};
use warpui_core::clipboard::ClipboardContent;

impl SettingsView {
    fn sync_agent_command_editors(&mut self, ctx: &mut ViewContext<Self>) {
        let config = self.agent_configs[self.command_agent.command_key()].clone();
        self.agent_command_value = config.prompt_command.clone();
        for (editor, text) in [
            (&self.agent_base_editor, &config.command),
            (&self.agent_command_editor, &config.prompt_command),
            (&self.agent_suffix_editor, &config.prompt_suffix),
        ] {
            ctx.update_view(editor, |editor, ctx| editor.set_text(text, ctx));
        }
    }

    pub(crate) fn handle_misc(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::SelectCommandAgent(agent) => {
                self.command_agent = *agent;
                self.sync_agent_command_editors(ctx);
                self.agent_command_status = None;
                ctx.notify();
            }
            SettingsAction::SetAgentPromptStdin(stdin) => {
                if let Some(config) = self.agent_configs.get_mut(self.command_agent.command_key()) {
                    config.prompt_stdin = *stdin;
                }
                self.agent_command_status = Some("Unsaved changes".into());
            }
            SettingsAction::SaveAgentCommand => {
                let key = self.command_agent.command_key();
                if let Some(config) = self.agent_configs.get(key) {
                    if let Err(error) = config.validate() {
                        self.agent_command_status = Some(error.into());
                        return;
                    }
                    let mut settings = crate::setup::UiSettings::load();
                    settings.agent_commands.remove(key);
                    settings
                        .agent_command_configs
                        .insert(key.to_string(), config.clone());
                    self.agent_command_status = Some(match settings.save_checked() {
                        Ok(()) => "Changes saved. New launches will use these commands.".into(),
                        Err(error) => format!("Could not save: {error}"),
                    });
                }
            }
            SettingsAction::ResetAgentCommand => {
                self.agent_configs.insert(
                    self.command_agent.command_key().to_string(),
                    default_agent_command_config(self.command_agent),
                );
                self.sync_agent_command_editors(ctx);
                self.agent_command_status =
                    Some("Defaults restored. Select Save changes to apply.".into());
            }
            SettingsAction::OpenPrivacyPolicy => {
                ctx.dispatch_typed_action_deferred(
                    crate::workspace::WorkspaceAction::OpenExternalUrl {
                        url: "https://dragent.dev/privacy".into(),
                    },
                );
            }
            SettingsAction::StarOnGitHub => {
                ctx.dispatch_typed_action_deferred(crate::workspace::WorkspaceAction::StarOnGitHub);
            }
            SettingsAction::FollowOnX => {
                ctx.dispatch_typed_action_deferred(crate::workspace::WorkspaceAction::FollowOnX);
            }
            SettingsAction::OpenDocumentation => {
                ctx.dispatch_typed_action_deferred(
                    crate::workspace::WorkspaceAction::OpenExternalUrl {
                        url: "https://dragent.dev/docs".into(),
                    },
                );
            }
            SettingsAction::ResetOnboarding => {
                reset_for_onboarding();
                self.status_message =
                    Some("Onboarding reset — restart Dragent to see the setup wizard.".to_string());
            }
            SettingsAction::CheckForUpdates => {
                self.status_message = Some(format!(
                    "Dragent {} — manual update checks are not wired yet.",
                    env!("CARGO_PKG_VERSION")
                ));
            }
            SettingsAction::CopyVersion => {
                ctx.clipboard().write(ClipboardContent::plain_text(
                    env!("CARGO_PKG_VERSION").to_string(),
                ));
                self.status_message = Some("Version copied to clipboard.".to_string());
            }
            _ => {}
        }
    }
}
