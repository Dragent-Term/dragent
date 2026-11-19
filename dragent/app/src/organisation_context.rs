//! Which organisation's local data the app shows.
//!
//! The source of truth is the signed-in user's server record: their active
//! organisation, as `OrgStore` reports it. The last one seen is cached in
//! `~/.dragent/organisation.json` only so launch opens the right local context
//! before the network answers; the server wins as soon as it does.
//!
//! Local data is scoped per organisation (`dragent_paths::organisation_dir`).
//! Everything created before organisations existed lives in the `local`
//! context. The personal organisation of the first account to sign in on this
//! machine adopts it, so signing in never hides what a user already had; other
//! accounts' personal organisations get a context of their own. Signed out,
//! the app shows `local` only while no account has adopted it; after that it
//! shows an empty signed-out context, so signing out never leaves one account's
//! tabs, projects, or worktrees on screen for the next person to sign in.
//!
//! A switch is two-phase. `WillSwitch` asks every window to save and detach
//! its workspace while the old organisation is still selected; on the next
//! tick the writer is flushed, the new organisation is selected, and
//! `Switched` asks the windows to restore from it. Terminals keep running in
//! the previous organisation's host and reattach when the user switches back.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity, UpdateModel};

use crate::auth::{AuthManager, AuthManagerEvent, AuthStatus};
use crate::settings::{OrgStore, OrgStoreEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrganisationContextEvent {
    /// Save and detach workspaces: the selection changes on the next tick.
    WillSwitch,
    /// The new organisation is selected; restore workspaces from it.
    Switched,
}

/// The user's active organisation as last confirmed by the server.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveOrganisation {
    pub user_id: String,
    pub organisation_id: String,
    pub name: String,
    /// Local storage context: `local`, or the organisation id.
    pub context: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Cache {
    #[serde(default)]
    active: Option<ActiveOrganisation>,
    /// Account whose personal organisation owns the `local` context.
    #[serde(default)]
    local_owner: Option<String>,
}

fn cache_path() -> PathBuf {
    dragent_paths::home().join("organisation.json")
}

fn load_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_cache(cache: &Cache) {
    let path = cache_path();
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(cache)?)?;
        std::fs::rename(&temp, &path)
    })();
    if let Err(error) = result {
        log::warn!("[organisation] could not save {}: {error}", path.display());
    }
}

/// Local context used while signed out once an account owns `local`. Server
/// organisation ids are UUIDs, so this can never name a real organisation.
const SIGNED_OUT_CONTEXT: &str = "signed-out";

/// What a signed-out app shows: the pre-organisation data until an account
/// adopts it, then an empty context of its own.
fn signed_out_context(cache: &Cache) -> String {
    match cache.local_owner {
        Some(_) => SIGNED_OUT_CONTEXT.to_string(),
        None => dragent_paths::local_organisation_id(),
    }
}

/// Select the cached organisation before anything opens local storage. With
/// no signed-in organisation cached, open the signed-out context directly
/// rather than flashing the previous account's workspaces.
pub fn restore_at_launch() {
    let cache = load_cache();
    match &cache.active {
        Some(active) => dragent_paths::select_organisation(&active.context),
        None => dragent_paths::select_organisation(&signed_out_context(&cache)),
    }
}

/// The active organisation as last cached by the app (for the command
/// line, which runs without signing in).
pub fn cached_active() -> Option<ActiveOrganisation> {
    load_cache().active
}

/// Personal organisations are created with a slug derived from the owner's
/// id: its first eight hex digits followed by `-team`.
pub fn is_personal(slug: &str, user_id: &str) -> bool {
    let hex: String = user_id.chars().filter(|c| *c != '-').take(8).collect();
    hex.len() == 8 && slug.eq_ignore_ascii_case(&format!("{hex}-team"))
}

/// The local context for `organisation_id`. The first account whose personal
/// organisation is resolved here claims the pre-organisation `local` data.
fn context_for(cache: &mut Cache, user_id: &str, organisation_id: &str, personal: bool) -> String {
    if personal {
        match cache.local_owner.as_deref() {
            None => {
                cache.local_owner = Some(user_id.to_string());
                return dragent_paths::local_organisation_id();
            }
            Some(owner) if owner == user_id => return dragent_paths::local_organisation_id(),
            Some(_) => {}
        }
    }
    organisation_id.to_string()
}

pub struct OrganisationContext {
    cache: Cache,
    /// Target of a switch whose selection flips on the next tick.
    pending: Option<String>,
}

impl Entity for OrganisationContext {
    type Event = OrganisationContextEvent;
}

impl SingletonEntity for OrganisationContext {}

impl OrganisationContext {
    pub(crate) fn preview(_ctx: &mut ModelContext<Self>) -> Self {
        Self { cache: Cache::default(), pending: None }
    }

    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let org_store = OrgStore::handle(ctx);
        ctx.subscribe_to_model(&org_store, |this, event, ctx| {
            if matches!(event, OrgStoreEvent::Updated) {
                this.sync_from_server(ctx);
            }
        });
        let auth = AuthManager::handle(ctx);
        ctx.subscribe_to_model(&auth, |this, event, ctx| match event {
            AuthManagerEvent::AuthComplete => {
                // A new login can belong to a different account. Retire old
                // requests and hide its workspace before resolving membership.
                this.signed_out(ctx);
                this.fetch_identity(ctx);
            }
            AuthManagerEvent::SessionRestored => {
                let user = match AuthManager::as_ref(ctx).status() {
                    AuthStatus::LoggedIn(user) => user.user_id.as_deref(),
                    _ => None,
                };
                if this
                    .cache
                    .active
                    .as_ref()
                    .is_some_and(|active| user.is_some_and(|user| user != active.user_id))
                {
                    this.signed_out(ctx);
                }
                this.fetch_identity(ctx);
            }
            AuthManagerEvent::LoggedOut | AuthManagerEvent::SessionExpired => this.signed_out(ctx),
            AuthManagerEvent::AuthFailed { .. } => {}
        });
        let account = crate::settings::AccountStore::handle(ctx);
        ctx.subscribe_to_model(&account, |this, event, ctx| {
            if matches!(event, crate::settings::AccountStoreEvent::DataUpdated) {
                this.sync_from_server(ctx);
            }
        });
        match AuthManager::as_ref(ctx).status() {
            // No stored session: a cached organisation from an earlier sign-in
            // must not stay visible.
            AuthStatus::LoggedOut => {
                ctx.spawn(Timer::after(Duration::ZERO), |this, _, ctx| {
                    this.signed_out(ctx)
                });
            }
            AuthStatus::LoggedIn(_) => {
                let store = OrgStore::handle(ctx);
                ctx.update_model(&store, |s, c| s.fetch(c));
            }
            AuthStatus::Restoring | AuthStatus::SigningIn | AuthStatus::NeedsReauth => {}
        }
        Self {
            cache: load_cache(),
            pending: None,
        }
    }

    /// A coordinator without the auth and server subscriptions, driven
    /// directly by integration tests (no credentials are touched).
    #[doc(hidden)]
    pub fn new_detached(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            cache: load_cache(),
            pending: None,
        }
    }

    /// Test hook: the switch a resolved server organisation triggers.
    #[doc(hidden)]
    pub fn switch_for_tests(&mut self, context: &str, ctx: &mut ModelContext<Self>) {
        self.switch_to(context.to_string(), ctx);
    }

    /// Test hook: what signing out (or a session expiring) does.
    #[doc(hidden)]
    pub fn sign_out_for_tests(&mut self, ctx: &mut ModelContext<Self>) {
        self.signed_out(ctx);
    }

    /// The active organisation as last confirmed (possibly from a previous run).
    pub fn active(&self) -> Option<&ActiveOrganisation> {
        self.cache.active.as_ref()
    }

    pub fn is_switching(&self) -> bool {
        self.pending.is_some()
    }

    fn fetch_identity(&mut self, ctx: &mut ModelContext<Self>) {
        let store = OrgStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.fetch(c));
        let account = crate::settings::AccountStore::handle(ctx);
        ctx.update_model(&account, |s, c| s.fetch(c));
    }

    fn sync_from_server(&mut self, ctx: &mut ModelContext<Self>) {
        let user_id = match AuthManager::as_ref(ctx).status() {
            AuthStatus::LoggedIn(user) => user.user_id.clone(),
            _ => return,
        }
        .or_else(|| {
            crate::settings::AccountStore::as_ref(ctx)
                .data
                .as_ref()
                .map(|d| d.user.id.clone())
        })
        .filter(|id| !id.is_empty());
        let Some(user_id) = user_id else {
            return;
        };
        let Some(org) = OrgStore::as_ref(ctx).active.clone() else {
            return;
        };
        let mut cache = self.cache.clone();
        let personal = is_personal(&org.slug, &user_id);
        let context = context_for(&mut cache, &user_id, &org.id, personal);
        cache.active = Some(ActiveOrganisation {
            user_id,
            organisation_id: org.id,
            name: org.name,
            context: context.clone(),
        });
        if cache != self.cache {
            self.cache = cache;
            save_cache(&self.cache);
            ctx.notify();
        }
        self.switch_to(context, ctx);
    }

    fn signed_out(&mut self, ctx: &mut ModelContext<Self>) {
        if self.cache.active.take().is_some() {
            save_cache(&self.cache);
            ctx.notify();
        }
        // Nothing of the previous account may feed the next sign-in: its
        // organisations and usage go before any other account resolves.
        let store = OrgStore::handle(ctx);
        ctx.update_model(&store, |s, c| s.clear(c));
        let account = crate::settings::AccountStore::handle(ctx);
        ctx.update_model(&account, |s, c| s.clear(c));
        let context = signed_out_context(&self.cache);
        self.switch_to(context, ctx);
    }

    fn switch_to(&mut self, context: String, ctx: &mut ModelContext<Self>) {
        let current = self
            .pending
            .clone()
            .unwrap_or_else(dragent_paths::organisation_id);
        if current == context {
            return;
        }
        log::info!("[organisation] switching local context {current} -> {context}");
        let already_pending = self.pending.replace(context).is_some();
        ctx.emit(OrganisationContextEvent::WillSwitch);
        ctx.notify();
        if !already_pending {
            ctx.spawn(Timer::after(Duration::ZERO), |this, _, ctx| {
                this.finish_switch(ctx)
            });
        }
    }

    fn finish_switch(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(context) = self.pending.take() else {
            return;
        };
        // Workspaces saved during `WillSwitch`; land those writes in the
        // organisation they belong to before its paths stop resolving.
        dragent_persistence::flush_session_on_close();
        dragent_paths::select_organisation(&context);
        dragent_persistence::warm_in_background();
        ctx.emit(OrganisationContextEvent::Switched);
        ctx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_slug_matches_the_server_scheme() {
        let user = "5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d";
        assert!(is_personal("5b0c1d2e-team", user));
        assert!(!is_personal("acme", user));
        assert!(!is_personal("5b0c1d2f-team", user));
        assert!(!is_personal("-team", ""));
    }

    #[test]
    fn signed_out_hides_adopted_local_data() {
        let mut cache = Cache::default();
        // Nobody has signed in on this machine: signed-out use is `local`.
        assert_eq!(signed_out_context(&cache), "local");
        assert_eq!(
            context_for(&mut cache, "alice", "alice-personal", true),
            "local"
        );
        // Alice owns `local` now; signing out must not keep it on screen.
        assert_eq!(signed_out_context(&cache), SIGNED_OUT_CONTEXT);
        assert_ne!(SIGNED_OUT_CONTEXT, "local");
    }

    #[test]
    fn first_personal_organisation_adopts_local_data() {
        let mut cache = Cache::default();
        assert_eq!(context_for(&mut cache, "alice", "org-a", false), "org-a");
        assert_eq!(cache.local_owner, None);
        assert_eq!(
            context_for(&mut cache, "alice", "alice-personal", true),
            "local"
        );
        assert_eq!(cache.local_owner.as_deref(), Some("alice"));
        // Same account again: still the adopted context.
        assert_eq!(
            context_for(&mut cache, "alice", "alice-personal", true),
            "local"
        );
        // Another account on this machine never sees the first one's data.
        assert_eq!(
            context_for(&mut cache, "bob", "bob-personal", true),
            "bob-personal"
        );
    }
}
