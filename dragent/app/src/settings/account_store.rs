//! AccountStore — singleton that holds the fetched account data from GET /api/v1/account.
//!
//! Lifecycle:
//!   1. `AccountStore::handle(ctx).update(ctx, |s,c| s.fetch(c))` triggers a background fetch.
//!   2. On success, `data` is populated and `AccountStoreEvent::DataUpdated` is emitted.
//!   3. Subscribers (e.g. SettingsView) call `ctx.notify()` in response so the account
//!      page re-renders with real data.

use std::sync::mpsc;
use std::time::Duration;

use serde::Deserialize;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum AccountStoreEvent {
    DataUpdated,
    FetchFailed(String),
}

// ── Response types (mirror server JSON) ──────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AccountData {
    pub user: UserInfo,
    pub usage: UsageInfo,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub display_name: String,
    /// `#[serde(default)]` so an older server without the field still parses.
    #[serde(default)]
    pub avatar_url: String,
    pub tier: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UsageInfo {
    /// Monthly subscription allowance (paid tiers) — one pool, no windows.
    #[serde(default)]
    pub paid: PaidUsage,
    /// Monthly free allowance every user gets — one pool, no windows.
    #[serde(default)]
    pub free: FreeUsage,
    /// Durable purchased-credit balance (top-ups). `#[serde(default)]` so an
    /// older server without the field still parses.
    #[serde(default)]
    pub deposit_balance_mc: i64,
    /// User opt-in for spending that balance. When false the server refuses a
    /// request that outruns the plan rather than billing purchased credits.
    #[serde(default)]
    pub allow_credit_usage: bool,
    /// True when the user has no premium-capable funds (no paid allowance and no
    /// consented credits) and is therefore restricted to the free-tier model.
    /// The input-bar model selector locks when this is set. `#[serde(default)]`
    /// so an older server without the field parses as "not locked".
    #[serde(default)]
    pub model_locked_to_free: bool,
}

/// Micro-cents per displayed credit.
///
/// **1 credit = 1 cent.** Balances are stored in micro-cents (mc) where
/// $1 = 1,000,000 mc, so a cent is 10,000 mc. Every surface must agree: the web
/// dashboard divides by 10,000, and the credit packs are priced on it
/// (`pack-10` is 10,000,000 mc sold for 1,000 cents = 1,000 credits for $10).
///
/// These helpers previously divided by 1,000, making a desktop "credit" worth a
/// tenth of a dashboard one — the same balance would have read 10x larger in the
/// app. Route every mc→credit conversion through `mc_to_credits` so the two can
/// never drift apart again.
pub const MC_PER_CREDIT: i64 = 10_000;

/// Convert a micro-cent amount to whole displayed credits (truncating, so a
/// balance is never shown as more than the user can actually spend).
pub fn mc_to_credits(mc: i64) -> u32 {
    (mc / MC_PER_CREDIT).max(0) as u32
}

/// Monthly subscription allowance — a single pool, usable at any pace.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PaidUsage {
    #[serde(default)]
    pub is_paid: bool,
    #[serde(default)]
    pub cap_mc: i64,
    #[serde(default)]
    pub used_mc: i64,
    #[serde(default)]
    pub balance_mc: i64,
    /// Unix seconds; monthly renewal date.
    #[serde(default)]
    pub resets_at: i64,
}

/// Monthly free allowance — a single pool, usable at any pace.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FreeUsage {
    #[serde(default)]
    pub cap_mc: i64,
    #[serde(default)]
    pub used_mc: i64,
    /// Unix seconds; monthly refresh date.
    #[serde(default)]
    pub resets_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub context_window: i64,
    pub min_tier: String,
    pub is_active: bool,
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct AccountStore {
    pub data: Option<AccountData>,
    pub loading: bool,
    pub error: Option<String>,
    /// Bumped by `clear()`. A fetch records the generation it started under and
    /// its result is dropped if a clear happened in between — otherwise a
    /// response in flight at sign-out would repopulate the store with the
    /// previous user's data.
    generation: u64,
}

impl Entity for AccountStore {
    type Event = AccountStoreEvent;
}

impl SingletonEntity for AccountStore {}

impl AccountStore {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            data: None,
            loading: false,
            error: None,
            generation: 0,
        }
    }

    /// Drop all cached account data. Called on sign-out so the account page
    /// never shows the previous user's name/usage — without this, stale data
    /// lingered until the next successful fetch (i.e. until the next login's
    /// API call), and a different user logging in briefly saw the old account.
    pub fn clear(&mut self, ctx: &mut ModelContext<Self>) {
        self.data = None;
        self.loading = false;
        self.error = None;
        self.generation += 1;
        ctx.emit(AccountStoreEvent::DataUpdated);
        ctx.notify();
    }

    /// Kick off a background fetch of /api/v1/account. No-ops when already in flight.
    pub fn fetch(&mut self, ctx: &mut ModelContext<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        ctx.notify();

        let (tx, rx) = mpsc::channel();
        crate::bg::detach(move || {
            let result = crate::api::API_CLIENT.get("/api/v1/account", 15);
            let _ = tx.send(result);
        });

        let gen = self.generation;
        ctx.spawn(
            Timer::after(Duration::from_millis(20)),
            move |this, _, mctx| this.poll_fetch(rx, gen, mctx),
        );
    }

    /// Non-blocking wait for the preference PATCH result.
    ///
    /// Mirrors `poll_fetch` rather than calling `rx.recv()`: this runs on the
    /// UI context, so a blocking receive freezes the whole app for as long as
    /// the request takes — up to the full 15s timeout on a hung network, for
    /// what is a single settings toggle.
    fn poll_preference_save(
        &mut self,
        rx: mpsc::Receiver<Result<serde_json::Value, crate::api::ApiError>>,
        ctx: &mut ModelContext<Self>,
    ) {
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |store, _, mctx| {
                        store.poll_preference_save(rx, mctx);
                    },
                );
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // The request thread died without sending. Surface it: silence
                // would leave the toggle showing a value the server never took.
                log::warn!("[account] credit preference save channel disconnected");
                self.error = Some("Could not save that setting. Try again.".to_string());
                ctx.notify();
            }
            Ok(Ok(_)) => {
                // Refetch rather than trusting an optimistic local flip — this
                // preference decides whether the user is charged.
                self.fetch(ctx);
            }
            Ok(Err(e)) => {
                log::warn!("[account] could not save credit preference: {e}");
                self.error = Some("Could not save that setting. Try again.".to_string());
                ctx.notify();
            }
        }
    }

    fn poll_fetch(
        &mut self,
        rx: mpsc::Receiver<Result<serde_json::Value, crate::api::ApiError>>,
        gen: u64,
        ctx: &mut ModelContext<Self>,
    ) {
        // A clear() (sign-out) happened after this fetch started — its result
        // belongs to the previous session. Drop it.
        if gen != self.generation {
            return;
        }
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.spawn(
                    Timer::after(Duration::from_millis(20)),
                    move |this, _, mctx| this.poll_fetch(rx, gen, mctx),
                );
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = false;
                self.error = Some("account fetch channel disconnected".to_string());
                ctx.emit(AccountStoreEvent::FetchFailed(
                    self.error.clone().unwrap_or_default(),
                ));
                ctx.notify();
            }
            Ok(Err(e)) => {
                self.loading = false;
                self.error = Some(e.to_string());
                ctx.emit(AccountStoreEvent::FetchFailed(e.to_string()));
                ctx.notify();
            }
            Ok(Ok(json)) => {
                self.loading = false;
                self.error = None;
                self.data = serde_json::from_value(json).ok();
                ctx.emit(AccountStoreEvent::DataUpdated);
                ctx.notify();
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

impl PaidUsage {
    /// Monthly allowance in display credits (1 credit = 1 cent).
    pub fn used_credits(&self) -> u32 {
        mc_to_credits(self.used_mc)
    }
    pub fn cap_credits(&self) -> u32 {
        mc_to_credits(self.cap_mc).max(1)
    }
    pub fn balance_credits(&self) -> u32 {
        mc_to_credits(self.balance_mc)
    }
}

impl FreeUsage {
    pub fn used_credits(&self) -> u32 {
        mc_to_credits(self.used_mc)
    }
    pub fn cap_credits(&self) -> u32 {
        mc_to_credits(self.cap_mc).max(1)
    }
}

/// Persist the credit-usage opt-in, then refresh so the UI reflects what the
/// server actually stored rather than an optimistic guess — this preference
/// decides whether the user gets charged, so it must not drift.
pub fn set_allow_credit_usage(allow: bool, ctx: &mut ModelContext<AccountStore>) {
    let (tx, rx) = std::sync::mpsc::channel();
    crate::bg::detach(move || {
        let body = serde_json::json!({ "allow_credit_usage": allow });
        let _ = tx.send(crate::api::API_CLIENT.patch("/api/v1/account/preferences", &body, 15));
    });
    ctx.spawn(
        Timer::after(Duration::from_millis(20)),
        move |store, _, mctx| {
            store.poll_preference_save(rx, mctx);
        },
    );
}

#[cfg(test)]
mod credit_unit_tests {
    use super::*;

    /// 1 credit == 1 cent, and the desktop must agree with the web dashboard
    /// and with how the credit packs are priced.
    #[test]
    fn one_credit_is_one_cent() {
        // $1 = 1,000,000 mc = 100 cents = 100 credits.
        assert_eq!(mc_to_credits(1_000_000), 100);
        // A cent is exactly one credit.
        assert_eq!(mc_to_credits(MC_PER_CREDIT), 1);
        // The $10 pack: 10,000,000 mc sold for 1,000 cents.
        assert_eq!(mc_to_credits(10_000_000), 1_000);
        // The $20 pack carries a +10% bonus: 22,000,000 mc = 2,200 credits.
        assert_eq!(mc_to_credits(22_000_000), 2_200);
    }

    #[test]
    fn sub_credit_amounts_truncate_and_never_go_negative() {
        // Less than a cent shows as 0 rather than rounding up to a credit the
        // user does not have.
        assert_eq!(mc_to_credits(MC_PER_CREDIT - 1), 0);
        assert_eq!(mc_to_credits(0), 0);
        assert_eq!(mc_to_credits(-5_000), 0);
    }

    #[test]
    fn usage_helpers_use_the_shared_unit() {
        let p = PaidUsage {
            cap_mc: 20_000_000, // $20 plan
            used_mc: 5_000_000, // $5 spent
            ..Default::default()
        };
        assert_eq!(p.cap_credits(), 2_000); // $20 = 2,000 credits
        assert_eq!(p.used_credits(), 500); // $5  =   500 credits
        let f = FreeUsage {
            cap_mc: 375_000,
            used_mc: 0,
            ..Default::default()
        };
        assert_eq!(f.cap_credits(), 37); // $0.375 = 37.5 -> 37
    }
}
