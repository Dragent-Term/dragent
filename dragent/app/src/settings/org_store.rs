//! OrgStore — the signed-in user's organisations and everything the
//! Organisation settings pages show for the active one: members, sent
//! invitations, teams, and organisation billing, plus the subscription-plan and
//! credit-pack catalogues and invitations addressed to the user.
//!
//! Same backend routes as the web dashboard; every request runs off the UI
//! thread and lands through a short poll, like `AccountStore`. Checkout and
//! billing-portal links are handed to the view (`OrgStoreEvent::OpenUrl`) to
//! open in the browser, since payment happens on the provider's page.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

#[derive(Clone, Debug)]
pub enum OrgStoreEvent {
    Updated,
    /// The server-side active organisation changed: account usage and
    /// credits are resolved against it, so dependants should refetch.
    ActiveChanged,
    /// A checkout or billing-portal link to open in the browser.
    OpenUrl(String),
}

// ── Response types (mirror the Go API) ───────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Organization {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    /// "owner" | "admin" | "member"; absent on bare organisation responses.
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Member {
    pub user_id: String,
    pub email: String,
    pub role: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Invitation {
    pub id: String,
    #[serde(default)]
    pub organization_id: String,
    pub email: String,
    pub role: String,
    pub status: String,
    #[serde(default)]
    pub expired: bool,
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub created_at: i64,
}

impl Invitation {
    pub fn is_open(&self) -> bool {
        self.status == "pending" && !self.expired
    }

    /// Status as shown to the user ("expired" for a lapsed pending invite).
    pub fn display_status(&self) -> &str {
        if self.status == "pending" && self.expired {
            "expired"
        } else {
            &self.status
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Team {
    pub id: String,
    #[serde(default)]
    pub organization_id: String,
    pub name: String,
    #[serde(default)]
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct OrgBilling {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub is_paid: bool,
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub monthly_allowance_mc: i64,
    #[serde(default)]
    pub subscription_status: String,
    /// RFC 3339.
    #[serde(default)]
    pub subscription_renews_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Plan {
    pub id: String,
    pub tier: String,
    /// "monthly" | "annual".
    pub billing_interval: String,
    pub price_cents: i64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub purchasable: bool,
}

impl Plan {
    pub fn is_annual(&self) -> bool {
        self.billing_interval == "annual"
    }

    /// Effective monthly price in cents (annual plans divided over 12, rounded).
    pub fn monthly_cents(&self) -> i64 {
        if self.is_annual() {
            (self.price_cents + 6) / 12
        } else {
            self.price_cents
        }
    }

    /// Percent saved against paying `monthly` for a year; 0 when not cheaper.
    pub fn annual_saving_percent(&self, monthly: &Plan) -> i64 {
        if !self.is_annual() || monthly.price_cents <= 0 {
            return 0;
        }
        let yearly = monthly.price_cents * 12;
        (((yearly - self.price_cents) as f64 / yearly as f64) * 100.0)
            .round()
            .max(0.0) as i64
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Pack {
    pub id: String,
    pub price_cents: i64,
    /// Credits delivered, bonus included (1 credit = 10,000 mc = 1 cent).
    pub credits_mc: i64,
    #[serde(default)]
    pub currency: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub purchasable: bool,
}

impl Pack {
    pub fn credits(&self) -> u32 {
        super::account_store::mc_to_credits(self.credits_mc)
    }

    /// Bonus over the price paid, in percent (credits are 1¢ each).
    pub fn bonus_percent(&self) -> i64 {
        if self.price_cents <= 0 {
            return 0;
        }
        let credits = self.credits_mc / super::account_store::MC_PER_CREDIT;
        (((credits - self.price_cents) as f64 / self.price_cents as f64) * 100.0)
            .round()
            .max(0.0) as i64
    }
}

/// Organisation role, ordered by privilege.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrgRole {
    Member,
    Admin,
    Owner,
}

impl OrgRole {
    pub fn parse(role: &str) -> Self {
        match role {
            "owner" => Self::Owner,
            "admin" => Self::Admin,
            _ => Self::Member,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Admin => "Admin",
            Self::Member => "Member",
        }
    }

    pub fn can_manage(self) -> bool {
        self >= Self::Admin
    }
}

/// URL-safe slug for a display name ("Acme Labs!" → "acme-labs").
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

// ── Mutations ─────────────────────────────────────────────────────────────────

/// One write request against the API.
#[derive(Clone, Debug)]
pub struct Mutation {
    pub method: &'static str,
    pub path: String,
    pub body: serde_json::Value,
    /// Open the `url` the server returns (checkout, billing portal).
    pub opens_url: bool,
    /// Shown once the request succeeds.
    pub notice: Option<String>,
    /// The request changes which organisation is active.
    pub switches_active: bool,
    /// Make the organisation this request created the active one.
    pub activates_created: bool,
}

impl Mutation {
    fn new(method: &'static str, path: String, body: serde_json::Value) -> Self {
        Self {
            method,
            path,
            body,
            opens_url: false,
            notice: None,
            switches_active: false,
            activates_created: false,
        }
    }

    pub fn post(path: impl Into<String>, body: serde_json::Value) -> Self {
        Self::new("POST", path.into(), body)
    }

    pub fn patch(path: impl Into<String>, body: serde_json::Value) -> Self {
        Self::new("PATCH", path.into(), body)
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self::new("DELETE", path.into(), serde_json::Value::Null)
    }

    pub fn get(path: impl Into<String>) -> Self {
        Self::new("GET", path.into(), serde_json::Value::Null)
    }

    pub fn with_notice(mut self, notice: impl Into<String>) -> Self {
        self.notice = Some(notice.into());
        self
    }

    pub fn opening_url(mut self) -> Self {
        self.opens_url = true;
        self
    }

    pub fn switching_active(mut self) -> Self {
        self.switches_active = true;
        self
    }

    pub fn activating_created(mut self) -> Self {
        self.activates_created = true;
        self
    }

    fn send(&self) -> Result<serde_json::Value, crate::api::ApiError> {
        let api = &crate::api::API_CLIENT;
        match self.method {
            "GET" => api.get(&self.path, 20),
            "PATCH" => api.patch(&self.path, &self.body, 20),
            "DELETE" => api.delete(&self.path, 20),
            _ => api.post(&self.path, &self.body, 20),
        }
    }
}

// ── Loading ───────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Snapshot {
    organizations: Option<Vec<Organization>>,
    active: Option<Organization>,
    my_invitations: Vec<Invitation>,
    plans: Vec<Plan>,
    packs: Vec<Pack>,
    billing: Option<OrgBilling>,
    members: Option<Vec<Member>>,
    invitations: Option<Vec<Invitation>>,
    teams: Option<Vec<Team>>,
    error: Option<String>,
}

fn get_as<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let value = crate::api::API_CLIENT
        .get(path, 20)
        .map_err(|e| e.user_message())?;
    serde_json::from_value(value).map_err(|e| format!("Unexpected response from {path}: {e}"))
}

/// List endpoints answer a bare array, or `{"<key>": [...]}` (plans, packs).
fn get_list<T: serde::de::DeserializeOwned>(path: &str, key: &str) -> Result<Vec<T>, String> {
    let value = crate::api::API_CLIENT
        .get(path, 20)
        .map_err(|e| e.user_message())?;
    let list = match value {
        serde_json::Value::Object(mut map) => map.remove(key).unwrap_or_default(),
        other => other,
    };
    if list.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(list).map_err(|e| format!("Unexpected response from {path}: {e}"))
}

/// Everything the Organisation pages show, fetched in parallel where the
/// requests are independent.
fn load_snapshot() -> Snapshot {
    let mut snap = Snapshot::default();
    let mut errors = Vec::new();
    std::thread::scope(|scope| {
        let orgs = scope.spawn(|| get_list::<Organization>("/api/v1/organizations", "organizations"));
        let active = scope.spawn(|| get_as::<Organization>("/api/v1/me/active-organization"));
        let mine = scope.spawn(|| get_list::<Invitation>("/api/v1/me/invitations", "invitations"));
        let plans = scope.spawn(|| get_list::<Plan>("/api/v1/billing/plans", "plans"));
        let packs = scope.spawn(|| get_list::<Pack>("/api/v1/billing/packs", "packs"));
        match orgs.join().unwrap_or_else(|_| Err("organisation request panicked".into())) {
            Ok(list) => snap.organizations = Some(list),
            Err(e) => errors.push(e),
        }
        // A missing active organisation is not an error: fall back to the
        // first membership (every account has its personal organisation).
        snap.active = active.join().ok().and_then(Result::ok);
        snap.my_invitations = mine.join().ok().and_then(Result::ok).unwrap_or_default();
        snap.plans = plans.join().ok().and_then(Result::ok).unwrap_or_default();
        snap.packs = packs.join().ok().and_then(Result::ok).unwrap_or_default();
    });
    if snap.active.as_ref().is_none_or(|a| a.id.is_empty()) {
        snap.active = snap.organizations.as_ref().and_then(|l| l.first().cloned());
    }
    // The active response may omit the role; the membership list has it.
    if let (Some(active), Some(list)) = (snap.active.as_mut(), snap.organizations.as_ref()) {
        if active.role.is_empty() {
            if let Some(member_of) = list.iter().find(|o| o.id == active.id) {
                active.role = member_of.role.clone();
            }
        }
    }
    if let Some(active) = snap.active.clone() {
        let org = active.id.clone();
        let can_manage = OrgRole::parse(&active.role).can_manage();
        std::thread::scope(|scope| {
            let billing = scope.spawn(|| get_as::<OrgBilling>(&format!("/api/v1/organizations/{org}/billing")));
            let members = scope.spawn(|| get_list::<Member>(&format!("/api/v1/organizations/{org}/members"), "members"));
            let sent = can_manage.then(|| {
                scope.spawn(|| get_list::<Invitation>(&format!("/api/v1/organizations/{org}/invitations"), "invitations"))
            });
            match billing.join().unwrap_or_else(|_| Err("billing request panicked".into())) {
                Ok(b) => snap.billing = Some(b),
                Err(e) => errors.push(e),
            }
            match members.join().unwrap_or_else(|_| Err("members request panicked".into())) {
                Ok(m) => snap.members = Some(m),
                Err(e) => errors.push(e),
            }
            if let Some(sent) = sent {
                match sent.join().unwrap_or_else(|_| Err("invitations request panicked".into())) {
                    Ok(i) => snap.invitations = Some(i),
                    Err(e) => errors.push(e),
                }
            }
        });
        // Teams are a paid-plan feature; a free organisation has none.
        if snap.billing.as_ref().is_some_and(|b| b.is_paid) {
            match get_list::<Team>(&format!("/api/v1/organizations/{org}/teams"), "teams") {
                Ok(t) => snap.teams = Some(t),
                Err(e) => errors.push(e),
            }
        } else {
            snap.teams = Some(Vec::new());
        }
    }
    snap.error = errors.into_iter().next();
    snap
}

// ── Entity ────────────────────────────────────────────────────────────────────

pub struct OrgStore {
    pub organizations: Option<Vec<Organization>>,
    pub active: Option<Organization>,
    pub members: Option<Vec<Member>>,
    /// Invitations the active organisation sent (admins only).
    pub invitations: Option<Vec<Invitation>>,
    pub teams: Option<Vec<Team>>,
    pub billing: Option<OrgBilling>,
    pub plans: Vec<Plan>,
    pub packs: Vec<Pack>,
    /// Invitations addressed to the signed-in user, from any organisation.
    pub my_invitations: Vec<Invitation>,
    pub loading: bool,
    /// Key of the mutation in flight; its control shows a busy state.
    pub busy: Option<String>,
    pub error: Option<String>,
    pub notice: Option<String>,
    loaded_at: Option<Instant>,
    /// Bumped by `clear()` so a response from before sign-out is dropped.
    generation: u64,
}

impl Entity for OrgStore {
    type Event = OrgStoreEvent;
}

impl SingletonEntity for OrgStore {}

type Reply<T> = mpsc::Receiver<T>;

impl OrgStore {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            organizations: None,
            active: None,
            members: None,
            invitations: None,
            teams: None,
            billing: None,
            plans: Vec::new(),
            packs: Vec::new(),
            my_invitations: Vec::new(),
            loading: false,
            busy: None,
            error: None,
            notice: None,
            loaded_at: None,
            generation: 0,
        }
    }

    pub fn active_role(&self) -> OrgRole {
        self.active
            .as_ref()
            .map(|a| OrgRole::parse(&a.role))
            .unwrap_or(OrgRole::Member)
    }

    pub fn is_paid(&self) -> bool {
        self.billing.as_ref().is_some_and(|b| b.is_paid)
    }

    pub fn is_busy(&self, key: &str) -> bool {
        self.busy.as_deref() == Some(key)
    }

    /// Drop everything (sign-out): nothing of the previous user may show.
    pub fn clear(&mut self, ctx: &mut ModelContext<Self>) {
        *self = Self {
            generation: self.generation + 1,
            ..Self::new(ctx)
        };
        ctx.emit(OrgStoreEvent::Updated);
        ctx.notify();
    }

    pub fn dismiss_messages(&mut self, ctx: &mut ModelContext<Self>) {
        self.error = None;
        self.notice = None;
        ctx.notify();
    }

    /// Refetch unless a load finished within `max_age`.
    pub fn fetch_if_stale(&mut self, max_age: Duration, ctx: &mut ModelContext<Self>) {
        if self.loaded_at.is_some_and(|t| t.elapsed() < max_age) {
            return;
        }
        self.fetch(ctx);
    }

    pub fn fetch(&mut self, ctx: &mut ModelContext<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        ctx.notify();
        let (tx, rx) = mpsc::channel();
        crate::bg::detach(move || {
            let _ = tx.send(load_snapshot());
        });
        let generation = self.generation;
        ctx.spawn(Timer::after(Duration::from_millis(20)), move |s, _, c| {
            s.poll_snapshot(rx, generation, c)
        });
    }

    fn poll_snapshot(&mut self, rx: Reply<Snapshot>, generation: u64, ctx: &mut ModelContext<Self>) {
        if generation != self.generation {
            return;
        }
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.spawn(Timer::after(Duration::from_millis(20)), move |s, _, c| {
                    s.poll_snapshot(rx, generation, c)
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = false;
                self.error = Some("Loading your organisation stopped unexpectedly.".into());
                ctx.emit(OrgStoreEvent::Updated);
                ctx.notify();
            }
            Ok(snap) => {
                self.loading = false;
                self.loaded_at = Some(Instant::now());
                if snap.organizations.is_some() {
                    self.organizations = snap.organizations;
                }
                self.active = snap.active;
                self.members = snap.members;
                self.invitations = snap.invitations;
                self.teams = snap.teams;
                self.billing = snap.billing;
                self.plans = snap.plans;
                self.packs = snap.packs;
                self.my_invitations = snap.my_invitations;
                if snap.error.is_some() {
                    self.error = snap.error;
                }
                ctx.emit(OrgStoreEvent::Updated);
                ctx.notify();
            }
        }
    }

    /// Make `id` the user's active organisation on the server. Once the
    /// refetch confirms it, `OrganisationContext` switches local data.
    pub fn switch_active(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            return;
        }
        let switch = Mutation::post(
            format!("/api/v1/organizations/{id}/set-active"),
            serde_json::json!({}),
        )
        .switching_active();
        self.perform("switch", switch, ctx);
    }

    /// Run one write request. `key` marks the control that started it; a
    /// second request while one is in flight is ignored.
    pub fn perform(&mut self, key: &str, mutation: Mutation, ctx: &mut ModelContext<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(key.to_string());
        self.error = None;
        self.notice = None;
        ctx.notify();
        let (tx, rx) = mpsc::channel();
        let request = mutation.clone();
        crate::bg::detach(move || {
            let _ = tx.send(request.send());
        });
        let generation = self.generation;
        ctx.spawn(Timer::after(Duration::from_millis(20)), move |s, _, c| {
            s.poll_mutation(rx, mutation, generation, c)
        });
    }

    fn poll_mutation(
        &mut self,
        rx: Reply<Result<serde_json::Value, crate::api::ApiError>>,
        mutation: Mutation,
        generation: u64,
        ctx: &mut ModelContext<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match rx.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                ctx.spawn(Timer::after(Duration::from_millis(20)), move |s, _, c| {
                    s.poll_mutation(rx, mutation, generation, c)
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.busy = None;
                self.error = Some("That request stopped unexpectedly. Try again.".into());
                ctx.notify();
            }
            Ok(Err(error)) => {
                self.busy = None;
                self.error = Some(error.user_message());
                ctx.notify();
            }
            Ok(Ok(value)) => {
                self.busy = None;
                self.notice = mutation.notice.clone();
                if mutation.opens_url {
                    match value.get("url").and_then(|u| u.as_str()) {
                        Some(url) if !url.is_empty() => {
                            ctx.emit(OrgStoreEvent::OpenUrl(url.to_string()))
                        }
                        _ => {
                            self.notice = None;
                            self.error = Some("Checkout isn't available right now.".into());
                        }
                    }
                }
                if mutation.switches_active {
                    ctx.emit(OrgStoreEvent::ActiveChanged);
                }
                if mutation.activates_created {
                    if let Some(id) = value.get("id").and_then(|i| i.as_str()) {
                        let activate = Mutation::post(
                            format!("/api/v1/organizations/{id}/set-active"),
                            serde_json::json!({}),
                        )
                        .switching_active();
                        let notice = self.notice.take();
                        self.perform("switch", activate, ctx);
                        self.notice = notice;
                        ctx.notify();
                        return;
                    }
                }
                // Checkout links don't change anything until payment lands.
                if !mutation.opens_url {
                    self.loaded_at = None;
                    self.fetch(ctx);
                }
                ctx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_shapes_parse() {
        let orgs: Vec<Organization> = serde_json::from_str(
            r#"[{"id":"o1","name":"Acme","slug":"acme","role":"owner"},{"id":"o2","name":"Personal","slug":"p"}]"#,
        )
        .unwrap();
        assert_eq!(orgs[0].role, "owner");
        assert_eq!(orgs[1].role, "");
        let billing: OrgBilling = serde_json::from_str(
            r#"{"enabled":true,"is_paid":true,"plan_id":"pro-m","tier":"pro","monthly_allowance_mc":20000000,"subscription_status":"active","subscription_renews_at":"2026-10-30T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(billing.is_paid && billing.subscription_renews_at.is_some());
        let inv: Invitation = serde_json::from_str(
            r#"{"id":"i","organization_id":"o1","email":"a@b.c","role":"member","status":"pending","expired":true,"expires_at":1,"created_at":1}"#,
        )
        .unwrap();
        assert!(!inv.is_open());
        assert_eq!(inv.display_status(), "expired");
    }

    #[test]
    fn prices_and_bonuses_match_the_dashboard() {
        let monthly = Plan {
            id: "pro-m".into(),
            tier: "pro".into(),
            billing_interval: "monthly".into(),
            price_cents: 2_000,
            ..Default::default()
        };
        let annual = Plan {
            id: "pro-y".into(),
            tier: "pro".into(),
            billing_interval: "annual".into(),
            price_cents: 19_200,
            ..Default::default()
        };
        assert_eq!(annual.monthly_cents(), 1_600);
        assert_eq!(annual.annual_saving_percent(&monthly), 20);
        assert_eq!(monthly.annual_saving_percent(&monthly), 0);
        // $50 pack with +15%: 5,750 credits.
        let pack = Pack {
            id: "pack-50".into(),
            price_cents: 5_000,
            credits_mc: 57_500_000,
            ..Default::default()
        };
        assert_eq!(pack.credits(), 5_750);
        assert_eq!(pack.bonus_percent(), 15);
    }

    #[test]
    fn roles_and_slugs() {
        assert!(OrgRole::parse("owner").can_manage());
        assert!(OrgRole::parse("admin").can_manage());
        assert!(!OrgRole::parse("member").can_manage());
        assert!(!OrgRole::parse("").can_manage());
        assert_eq!(slugify("  Acme Labs! "), "acme-labs");
        assert_eq!(slugify("Déjà vu 2"), "d-j-vu-2");
        assert_eq!(slugify("---"), "");
    }
}
