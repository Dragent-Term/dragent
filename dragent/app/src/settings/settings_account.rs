use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::auth::AuthManager;
use crate::settings::AccountStore;
use warpui::SingletonEntity;
use warpui::UpdateModel;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_account(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::RefreshAccount => {
                let account = AccountStore::handle(ctx);
                ctx.update_model(&account, |store, mctx| store.fetch(mctx));
            }
            SettingsAction::SignIn => {
                self.auth_status_message = None;
                let auth = AuthManager::handle(ctx);
                let url =
                    ctx.update_model(&auth, |auth, auth_ctx| auth.begin_browser_sign_in(auth_ctx));
                ctx.open_url(&url);
            }
            SettingsAction::SignOut => {
                // Confirm first (with a shared-session warning), unless the user
                // asked not to be asked again.
                if crate::settings::AppSettings::skip_logout_confirm() {
                    let auth = AuthManager::handle(ctx);
                    ctx.update_model(&auth, |auth, auth_ctx| auth.log_out(auth_ctx));
                } else {
                    self.logout_dont_ask = false;
                    self.logout_confirm_open = true;
                    ctx.notify();
                }
            }
            SettingsAction::ConfirmSignOut => {
                if self.logout_dont_ask {
                    crate::settings::AppSettings::set_skip_logout_confirm(true);
                }
                self.logout_confirm_open = false;
                let auth = AuthManager::handle(ctx);
                ctx.update_model(&auth, |auth, auth_ctx| auth.log_out(auth_ctx));
                ctx.notify();
            }
            SettingsAction::CancelSignOut => {
                self.logout_confirm_open = false;
                ctx.notify();
            }
            SettingsAction::ToggleLogoutDontAsk => {
                self.logout_dont_ask = !self.logout_dont_ask;
                ctx.notify();
            }
            _ => {}
        }
    }
}
