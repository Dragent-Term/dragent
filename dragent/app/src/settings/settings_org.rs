//! Organisation pages: actions for the switcher, members, invitations, teams,
//! and billing & usage. Server state lives in `OrgStore`; this file turns
//! clicks and form input into its requests.

use std::time::Duration;

use serde_json::json;
use warpui::r#async::Timer;
use warpui::{ReadModel, SingletonEntity, UpdateModel, UpdateView, ViewContext};

use super::action::{SettingsAction, SettingsSection};
use super::org_store::{slugify, Mutation, OrgStore};
use super::settings_view::{OrgEditors, SettingsView};
use crate::settings::AccountStore;
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode};

/// Refreshes after a checkout opens: every 5 s for three minutes.
const PURCHASE_WATCH_TICKS: u32 = 36;
const PURCHASE_WATCH_INTERVAL: Duration = Duration::from_secs(5);

impl SettingsView {
    /// Opening Members, Teams, or Billing & Usage: create the form fields and
    /// load fresh organisation and usage data.
    pub(crate) fn enter_org_page(&mut self, ctx: &mut ViewContext<Self>) {
        self.ensure_org_editors(ctx);
        self.org.confirm = None;
        let store = OrgStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.fetch_if_stale(Duration::from_secs(30), c));
        let account = AccountStore::handle(ctx);
        ctx.update_model(&account, |s, c| s.fetch(c));
    }

    /// Members page with the new-organisation form open and focused.
    pub(crate) fn open_create_organisation(&mut self, ctx: &mut ViewContext<Self>) {
        self.show_section(SettingsSection::Members, ctx);
        self.org.switcher_open = true;
        self.org.create_open = true;
        if let Some(editors) = &self.org.editors {
            let editor = editors.new_org_name.clone();
            ctx.focus(&editor);
        }
        ctx.notify();
    }

    fn ensure_org_editors(&mut self, ctx: &mut ViewContext<Self>) {
        if self.org.editors.is_some() {
            return;
        }
        let theme = Theme::from_id(&self.theme_id);
        let ui_font = self.ui_font;
        let mut field = |placeholder: &'static str, key: &'static str| {
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new(
                    placeholder,
                    ui_font,
                    crate::theme::text_size(12.5),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings(key.to_string()),
                    inner.focus_handle(),
                )
            })
        };
        self.org.editors = Some(OrgEditors {
            invite_email: field("teammate@company.com", "org_invite_email"),
            team_name: field("Team name", "org_team_name"),
            new_org_name: field("Organisation name", "org_new_name"),
        });
    }

    fn clear_org_field(&mut self, which: fn(&OrgEditors) -> &warpui::ViewHandle<InputEditor>, ctx: &mut ViewContext<Self>) {
        if let Some(editors) = &self.org.editors {
            ctx.update_view(which(editors), |e, ectx| e.set_text("", ectx));
        }
    }

    /// First click arms a destructive action; the second performs it.
    fn confirmed(&mut self, key: &str) -> bool {
        if self.org.confirm.as_deref() == Some(key) {
            self.org.confirm = None;
            true
        } else {
            self.org.confirm = Some(key.to_string());
            false
        }
    }

    pub(crate) fn handle_org(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        let store = OrgStore::handle(ctx);
        let org_id = ctx.read_model(&store, |s, _| s.active.as_ref().map(|a| a.id.clone()));
        let org_path = |rest: &str| -> Option<String> {
            org_id
                .as_ref()
                .map(|id| format!("/api/v1/organizations/{id}{rest}"))
        };
        let mut run = |key: String, mutation: Mutation, ctx: &mut ViewContext<Self>| {
            ctx.update_model(&store, |s, c| s.perform(&key, mutation, c));
        };
        // Any other action disarms a pending confirmation.
        let keeps_confirm = matches!(
            action,
            SettingsAction::OrgRemoveMember(_)
                | SettingsAction::OrgLeave
                | SettingsAction::TeamDelete(_)
        );
        if !keeps_confirm && self.org.confirm.is_some() && is_org_action(action) {
            self.org.confirm = None;
        }

        match action {
            SettingsAction::OrgRefresh => {
                ctx.update_model(&store, |s, c| s.fetch(c));
                let account = AccountStore::handle(ctx);
                ctx.update_model(&account, |s, c| s.fetch(c));
            }
            SettingsAction::OrgDismissMessage => {
                ctx.update_model(&store, |s, c| s.dismiss_messages(c));
            }
            SettingsAction::OrgToggleSwitcher => {
                self.org.switcher_open = !self.org.switcher_open;
                if !self.org.switcher_open {
                    self.org.create_open = false;
                }
            }
            SettingsAction::OrgToggleCreate => {
                self.org.create_open = !self.org.create_open;
            }
            SettingsAction::OrgSwitch(id) => {
                self.org.switcher_open = false;
                self.org.create_open = false;
                let id = id.clone();
                ctx.update_model(&store, |s, c| s.switch_active(&id, c));
            }
            SettingsAction::OrgCreate => {
                let name = self.org.new_org_name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let slug = slugify(&name);
                if slug.is_empty() {
                    let store = OrgStore::handle(ctx);
                    ctx.update_model(&store, |s, c| {
                        s.error = Some("Use at least one letter or number in the name.".into());
                        c.notify();
                    });
                    return;
                }
                run(
                    "org_create".into(),
                    Mutation::post("/api/v1/organizations", json!({ "name": name, "slug": slug }))
                        .with_notice(format!("Created {name}."))
                        .activating_created(),
                    ctx,
                );
                self.org.new_org_name.clear();
                self.clear_org_field(|e| &e.new_org_name, ctx);
                self.org.create_open = false;
                self.org.switcher_open = false;
            }
            SettingsAction::OrgLeave => {
                if !self.confirmed("leave") {
                    return;
                }
                if let Some(path) = org_path("/leave") {
                    run(
                        "leave".into(),
                        Mutation::post(path, json!({}))
                            .with_notice("You left the organisation.")
                            .switching_active(),
                        ctx,
                    );
                }
            }
            SettingsAction::OrgSetInviteRole(role) => {
                self.org.invite_role = role.clone();
            }
            SettingsAction::OrgInvite => {
                let email = self.org.invite_email.trim().to_string();
                if email.is_empty() {
                    return;
                }
                if !looks_like_email(&email) {
                    ctx.update_model(&store, |s, c| {
                        s.error = Some(format!("\u{201c}{email}\u{201d} isn't a valid email address."));
                        c.notify();
                    });
                    return;
                }
                if let Some(path) = org_path("/invitations") {
                    run(
                        "invite".into(),
                        Mutation::post(path, json!({ "email": email, "role": self.org.invite_role }))
                            .with_notice(format!("Invitation sent to {email}.")),
                        ctx,
                    );
                    self.org.invite_email.clear();
                    self.clear_org_field(|e| &e.invite_email, ctx);
                }
            }
            SettingsAction::OrgCancelInvitation(id) => {
                if let Some(path) = org_path(&format!("/invitations/{id}/cancel")) {
                    run(
                        format!("cancel:{id}"),
                        Mutation::post(path, json!({})).with_notice("Invitation cancelled."),
                        ctx,
                    );
                }
            }
            SettingsAction::OrgSetMemberRole { user_id, role } => {
                let unchanged = ctx.read_model(&store, |s, _| {
                    s.members
                        .iter()
                        .flatten()
                        .any(|m| &m.user_id == user_id && &m.role == role)
                });
                if unchanged {
                    return;
                }
                if let Some(path) = org_path(&format!("/members/{user_id}")) {
                    run(
                        format!("role:{user_id}"),
                        Mutation::patch(path, json!({ "role": role })),
                        ctx,
                    );
                }
            }
            SettingsAction::OrgRemoveMember(user_id) => {
                if !self.confirmed(&format!("remove:{user_id}")) {
                    return;
                }
                if let Some(path) = org_path(&format!("/members/{user_id}")) {
                    run(
                        format!("remove:{user_id}"),
                        Mutation::delete(path).with_notice("Member removed."),
                        ctx,
                    );
                }
            }
            SettingsAction::OrgRespondInvitation { id, accept } => {
                let verb = if *accept { "accept" } else { "reject" };
                let mut mutation =
                    Mutation::post(format!("/api/v1/invitations/{id}/{verb}"), json!({}))
                        .with_notice(if *accept {
                            "Invitation accepted. Use Switch organisation to open it."
                        } else {
                            "Invitation declined."
                        });
                if *accept {
                    mutation = mutation.switching_active();
                }
                run(format!("respond:{id}"), mutation, ctx);
            }
            SettingsAction::TeamCreate => {
                let name = self.org.team_name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if let Some(path) = org_path("/teams") {
                    run(
                        "team_create".into(),
                        Mutation::post(path, json!({ "name": name, "slug": slugify(&name) }))
                            .with_notice(format!("Created team {name}.")),
                        ctx,
                    );
                    self.org.team_name.clear();
                    self.clear_org_field(|e| &e.team_name, ctx);
                }
            }
            SettingsAction::TeamDelete(id) => {
                if !self.confirmed(&format!("team:{id}")) {
                    return;
                }
                if let Some(path) = org_path(&format!("/teams/{id}")) {
                    run(
                        format!("team:{id}"),
                        Mutation::delete(path).with_notice("Team deleted."),
                        ctx,
                    );
                }
            }
            SettingsAction::BillingShowAnnual(annual) => {
                self.org.show_annual = *annual;
            }
            SettingsAction::BillingSubscribe(plan_id) => {
                if let Some(path) = org_path("/billing/subscribe") {
                    run(
                        format!("subscribe:{plan_id}"),
                        Mutation::post(path, json!({ "plan_id": plan_id }))
                            .opening_url()
                            .with_notice("Finish checkout in your browser. Your plan updates here when it completes."),
                        ctx,
                    );
                }
            }
            SettingsAction::BillingOpenPortal => {
                if let Some(path) = org_path("/billing/portal") {
                    run("portal".into(), Mutation::get(path).opening_url(), ctx);
                }
            }
            SettingsAction::BuyCredits(pack_id) => {
                run(
                    format!("buy:{pack_id}"),
                    Mutation::post("/api/v1/billing/topup", json!({ "pack_id": pack_id }))
                        .opening_url()
                        .with_notice("Finish the purchase in your browser. Your balance updates here when it goes through."),
                    ctx,
                );
            }
            _ => {}
        }
    }

    /// A checkout opened in the browser: refresh balance and plan every 5 s
    /// until either changes (or three minutes pass), so the page reflects the
    /// purchase without a manual refresh.
    pub(crate) fn arm_purchase_watch(&mut self, ctx: &mut ViewContext<Self>) {
        let already_running = self.org.purchase_watch > 0;
        self.org.purchase_watch = PURCHASE_WATCH_TICKS;
        self.org.purchase_baseline = Some(self.purchase_state(ctx));
        if !already_running {
            ctx.spawn(Timer::after(PURCHASE_WATCH_INTERVAL), |view, _, ctx| {
                view.purchase_watch_tick(ctx)
            });
        }
    }

    fn purchase_state(&self, ctx: &ViewContext<Self>) -> (i64, Option<String>) {
        let account = AccountStore::handle(ctx);
        let credits = ctx.read_model(&account, |s, _| {
            s.data.as_ref().map(|d| d.usage.deposit_balance_mc).unwrap_or(0)
        });
        let store = OrgStore::handle(ctx);
        let plan = ctx.read_model(&store, |s, _| {
            s.billing
                .as_ref()
                .filter(|b| b.is_paid)
                .map(|b| b.plan_id.clone())
        });
        (credits, plan)
    }

    fn purchase_watch_tick(&mut self, ctx: &mut ViewContext<Self>) {
        if self.org.purchase_watch == 0 {
            return;
        }
        let now = self.purchase_state(ctx);
        if self.org.purchase_baseline.as_ref().is_some_and(|base| *base != now) {
            self.org.purchase_watch = 0;
            self.org.purchase_baseline = None;
            let store = OrgStore::handle(ctx);
            ctx.update_model(&store, |s, c| {
                s.error = None;
                s.notice = Some("Payment received. Your account is updated.".into());
                c.notify();
            });
            ctx.notify();
            return;
        }
        self.org.purchase_watch -= 1;
        let account = AccountStore::handle(ctx);
        ctx.update_model(&account, |s, c| s.fetch(c));
        let store = OrgStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.fetch(c));
        if self.org.purchase_watch > 0 {
            ctx.spawn(Timer::after(PURCHASE_WATCH_INTERVAL), |view, _, ctx| {
                view.purchase_watch_tick(ctx)
            });
        }
    }
}

fn is_org_action(action: &SettingsAction) -> bool {
    matches!(
        action,
        SettingsAction::OrgRefresh
            | SettingsAction::OrgDismissMessage
            | SettingsAction::OrgToggleSwitcher
            | SettingsAction::OrgSwitch(_)
            | SettingsAction::OrgToggleCreate
            | SettingsAction::OrgCreate
            | SettingsAction::OrgLeave
            | SettingsAction::OrgSetInviteRole(_)
            | SettingsAction::OrgInvite
            | SettingsAction::OrgCancelInvitation(_)
            | SettingsAction::OrgSetMemberRole { .. }
            | SettingsAction::OrgRemoveMember(_)
            | SettingsAction::OrgRespondInvitation { .. }
            | SettingsAction::TeamCreate
            | SettingsAction::TeamDelete(_)
            | SettingsAction::BillingShowAnnual(_)
            | SettingsAction::BillingSubscribe(_)
            | SettingsAction::BillingOpenPortal
            | SettingsAction::BuyCredits(_)
            | SettingsAction::SelectSection(_)
    )
}

/// Enough to catch typos before a round trip; the server validates fully.
fn looks_like_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !email.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::looks_like_email;

    #[test]
    fn email_sanity_check() {
        assert!(looks_like_email("a@b.co"));
        assert!(looks_like_email("first.last+tag@company.io"));
        for bad in ["", "a", "a@", "@b.co", "a@b", "a@.co", "a@b.", "a b@c.co"] {
            assert!(!looks_like_email(bad), "{bad}");
        }
    }
}
