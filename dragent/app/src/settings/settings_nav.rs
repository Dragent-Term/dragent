use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::AccountStore;
use crate::settings::SettingsEvent;
use crate::settings::SettingsSection;
use warpui::SingletonEntity;
use warpui::UpdateModel;
use warpui::UpdateView;
use warpui::ViewContext;

impl SettingsView {
    /// Navigate to `section` as if its nav item were clicked (used when
    /// another surface opens Settings on a specific page).
    pub fn show_section(&mut self, section: SettingsSection, ctx: &mut ViewContext<Self>) {
        self.handle_nav(&SettingsAction::SelectSection(section), ctx);
        ctx.notify();
    }

    pub(crate) fn handle_nav(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::ClearSettingsSearch => {
                self.search_query.clear();
                ctx.update_view(&self.search_editor, |editor, ctx| {
                    editor.set_text("", ctx);
                });
                ctx.notify();
            }
            SettingsAction::ClearAgentMenuSearch => {
                self.agent_menu_search.clear();
                ctx.update_view(&self.agent_menu_search_editor, |editor, ctx| {
                    editor.set_text("", ctx);
                });
                ctx.notify();
            }
            SettingsAction::ClearCommandAgentSearch => {
                self.command_agent_search.clear();
                ctx.update_view(&self.command_agent_search_editor, |editor, ctx| {
                    editor.set_text("", ctx);
                });
                ctx.notify();
            }
            SettingsAction::Close => {
                ctx.emit(SettingsEvent::Close);
            }
            SettingsAction::SelectSection(section) => {
                self.section = *section;
                #[cfg(not(target_family = "wasm"))]
                if *section == SettingsSection::ComputerUse {
                    self.refresh_computer_permissions(ctx);
                }
                if *section == SettingsSection::AgentAccounts {
                    self.handle_agent_account(&super::agent_accounts::AccountAction::Refresh, ctx);
                }
                if *section == SettingsSection::Usage {
                    self.refresh_usage(ctx);
                }
                if *section == SettingsSection::Projects {
                    ctx.emit(SettingsEvent::Project(super::ProjectSettingsAction::Refresh));
                }
                if *section == SettingsSection::Machines { self.refresh_machines(ctx); }
                if *section == SettingsSection::Integrations { self.refresh_integrations(ctx); }
                if *section == SettingsSection::Mobile {
                    self.refresh_mobile_devices(ctx);
                    if self.mobile.pairing.is_some() {
                        self.arm_mobile_tick(ctx);
                        self.arm_mobile_poll(ctx);
                    }
                }
                if section.is_agent() {
                    self.agent_nav_open = true;
                }
                self.mcp_status_message = None;
                self.open_picker = None;
                // Refresh account data whenever the user opens the Account page,
                // and start the live session-countdown ticker.
                if *section == SettingsSection::Account {
                    let account = AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                    self.arm_account_poll(ctx);
                }
                if section.is_org_page() {
                    self.enter_org_page(ctx);
                }
                if *section == SettingsSection::CodeIndex {
                    self.ensure_ignore_editor(ctx);
                    self.arm_index_poll(ctx);
                }
                if *section == SettingsSection::TerminalInput
                    || *section == SettingsSection::Editor
                {
                    self.ensure_font_discovery(ctx);
                }
                if *section == SettingsSection::General
                    || *section == SettingsSection::VersionControl
                {
                    self.ensure_git_editors(ctx);
                }
            }
            SettingsAction::ToggleAgentNav => {
                self.agent_nav_open = !self.agent_nav_open;
            }
            SettingsAction::ToggleUmbrella(name) => {
                if let Some(pos) = self.collapsed_umbrellas.iter().position(|u| *u == *name) {
                    self.collapsed_umbrellas.remove(pos);
                } else {
                    self.collapsed_umbrellas.push(*name);
                }
                ctx.notify();
            }
            SettingsAction::TogglePicker(key) => {
                self.open_picker = if self.open_picker.as_deref() == Some(key.as_str()) {
                    None
                } else {
                    Some(key.clone())
                };
            }
            _ => {}
        }
    }
}
