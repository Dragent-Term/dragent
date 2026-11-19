//! Background worker publishing the desktop relay catalog.
//!
//! The UI thread only ever *notes* snapshots (cheap: one mutex, no network,
//! no disk). This worker owns the rest: per-account ownership with a
//! generation counter, latest-only sends, a `<=15s` heartbeat, and the
//! authenticated `PUT` via the shared `API_CLIENT` (whose own refresh logic
//! is reused verbatim — nothing here duplicates OAuth/token handling).
//!
//! Account binding: a snapshot carries the account its view belonged to at
//! collect time (`AccountKey`). The worker's `current` owner is adopted
//! **only** from incoming notes (and explicit retire signals). It is never
//! re-read from auth state at send time, and old contributions are discarded
//! — never relabeled — when the account changes. A final pre-send check
//! (`put_for_user`) refuses to transmit when the stored owner is no longer
//! current, e.g. a sign-out that landed mid-`PUT`.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use super::desktop_catalog::{
    aggregate, build_body, catalog_hash, publisher_enabled, should_send, validate_for_publish,
    AccountKey, CatalogPayload, IndexedContribution, MachineMeta as WireMachine,
    WindowContribution, HEARTBEAT_MS, RELAY_CATALOG_PATH,
};
#[cfg(feature = "e2ee")]
use super::desktop_catalog::{seal_catalog, sealed_put_body};

pub const PUT_TIMEOUT_SECS: u64 = 10;
// Retention rule: a noted contribution lives until explicit retirement
// (`note_removed` on window close, `note_signed_out` on sign-out or org
// switch). There is deliberately no render-based expiry: an idle open
// desktop may legitimately go a long time without re-rendering, and the
// heartbeat must keep publishing its hierarchy — never an empty catalog —
// while it is still open.

/// Machine metadata attached to the catalog (no claimed identity).
#[derive(Clone, Debug)]
pub struct MachineMeta {
    pub name: String,
    pub platform: String,
    pub version: Option<String>,
}

impl MachineMeta {
    pub fn current() -> Self {
        Self {
            name: system_hostname().unwrap_or_else(|| "desktop".to_string()),
            platform: std::env::consts::OS.to_string(),
            version: None,
        }
    }

    fn wire(&self) -> WireMachine {
        WireMachine {
            name: self.name.clone(),
            platform: self.platform.clone(),
            version: self.version.clone(),
        }
    }
}

// `libc` has no `gethostname` on Windows; the machine name is in the environment.
#[cfg(windows)]
fn system_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|name| !name.is_empty())
}

#[cfg(not(windows))]
fn system_hostname() -> Option<String> {
    let mut buf = [0 as libc::c_char; 256];
    // Zero return with a NUL inside means success; anything else → unknown.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len()) } == 0;
    if !ok {
        return None;
    }
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).ok().filter(|name| !name.is_empty())
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct State {
    /// Owning account confirmed by collector notes. `None` = signed out (or
    /// never signed in): nothing is sent.
    current: Option<AccountKey>,
    /// Bumped whenever the owning account changes; in-flight packages from a
    /// previous generation are never recorded as sent.
    generation: u64,
    /// Stable per-contributor wire indices (assigned at note time).
    indices: HashMap<String, u32>,
    next_index: u32,
    contributions: HashMap<String, WindowContribution>,
    pending: bool,
    last_hash: Option<u64>,
    last_sent_ms: Option<u64>,
    last_try_ms: Option<u64>,
    last_try_hash: Option<u64>,
    /// Last sealed blob sent (e2ee only): heartbeat resends these exact bytes
    /// under the same version instead of reserving a new one.
    #[cfg(feature = "e2ee")]
    sealed: Option<SealedSent>,
    /// The phones the catalog is sealed for changed (a pairing committed,
    /// a phone was revoked, keys were reset): seal and publish now rather
    /// than at the next heartbeat (§6.2 step 8, §6.4 step 3).
    #[cfg(feature = "e2ee")]
    reseal: bool,
}

/// Last sealed publish (e2ee only).
#[cfg(feature = "e2ee")]
#[derive(Debug, Clone)]
struct SealedSent {
    /// Plaintext catalog hash this seal covers (change detection).
    hash: u64,
    /// Persisted catalog version sealed under.
    version: u64,
    /// Sorted canonical recipient phone ids sealed for.
    recipients: Vec<String>,
    /// The relay PUT body (routing + ciphertext, no plaintext fields).
    body: serde_json::Value,
}

/// A publishable body plus the owner it was built for.
pub struct SendPackage {
    pub generation: u64,
    pub expected: AccountKey,
    pub body: serde_json::Value,
    pub hash: u64,
    pub empty: bool,
}

impl State {
    fn index_for(&mut self, contributor: &str) -> u32 {
        if let Some(index) = self.indices.get(contributor) {
            return *index;
        }
        let index = self.next_index;
        self.next_index += 1;
        self.indices.insert(contributor.to_string(), index);
        index
    }

    fn note_snapshot(&mut self, snap: WindowContribution) {
        if self.current.as_ref() != Some(&snap.account) {
            // A new owning account (sign-in, account switch, organisation
            // switch): the old account's snapshots are discarded, never
            // relabeled under the new owner.
            self.current = Some(snap.account.clone());
            self.generation += 1;
            self.contributions.clear();
        }
        self.index_for(&snap.contributor);
        self.contributions.insert(snap.contributor.clone(), snap);
        self.pending = true;
    }

    fn note_removed(&mut self, contributor: &str) {
        if self.contributions.remove(contributor).is_some() {
            // The survivor set changed (window closed): re-evaluate, which
            // publishes an empty catalog when nothing remains while signed in.
            self.pending = true;
        }
    }

    fn note_signed_out(&mut self, contributor: &str) {
        self.contributions.remove(contributor);
        if self.contributions.is_empty() {
            // Signed out: clear pending snapshots and ownership. No PUT is
            // attempted without credentials; the relay marks the machine
            // offline via `last_seen` expiry and keeps last-known state.
            self.current = None;
            self.pending = false;
            self.last_hash = None;
            self.last_sent_ms = None;
            self.last_try_ms = None;
            self.last_try_hash = None;
        } else {
            self.pending = true;
        }
    }

    /// Decide whether to send now. Pure apart from the clock value passed in,
    /// so tests drive it with a fake clock.
    /// The aggregate of every noted contribution, in index order.
    fn aggregate_current(&self) -> CatalogPayload {
        let mut indexed: Vec<(u32, &WindowContribution)> = self
            .contributions
            .values()
            .map(|contribution| {
                (
                    *self.indices.get(&contribution.contributor).unwrap_or(&0),
                    contribution,
                )
            })
            .collect();
        indexed.sort_by_key(|(index, _)| *index);
        let items: Vec<IndexedContribution> = indexed
            .into_iter()
            .map(|(index, contribution)| IndexedContribution { index, contribution })
            .collect();
        aggregate(&items)
    }

    /// Whether a recipient change forces a send now.
    fn reseal_due(&self) -> bool {
        #[cfg(feature = "e2ee")]
        {
            self.reseal
        }
        #[cfg(not(feature = "e2ee"))]
        {
            false
        }
    }

    fn flush_package(
        &mut self,
        device_id: &str,
        machine: Option<WireMachine>,
        now_ms: u64,
    ) -> Option<SendPackage> {
        let expected = self.current.clone()?;
        if !self.pending {
            match self.last_sent_ms {
                // Idle and heartbeat not due: nothing to do.
                Some(last) if now_ms.saturating_sub(last) < HEARTBEAT_MS => return None,
                Some(_) => {}
                // Never sent: fall through so an (empty) baseline goes out.
                None => {}
            }
        }
        let catalog: CatalogPayload = self.aggregate_current();
        if let Err(error) = validate_for_publish(&catalog) {
            log::warn!("[catalog] dropping invalid aggregate: {error}");
            self.pending = false;
            self.last_try_ms = Some(now_ms);
            return None;
        }
        let hash = catalog_hash(&catalog);
        let reseal = self.reseal_due();
        let changed = self.last_hash != Some(hash) || reseal;
        if !should_send(changed, now_ms, self.last_sent_ms) {
            self.pending = false;
            return None;
        }
        // Do not hammer a failing relay faster than the heartbeat unless the
        // content (or the recipient set) changed since the failed attempt.
        if self.last_try_hash == Some(hash) && !reseal {
            if let Some(last_try) = self.last_try_ms {
                if now_ms.saturating_sub(last_try) < HEARTBEAT_MS {
                    return None;
                }
            }
        }
        let empty = catalog.projects.is_empty()
            && catalog.workspaces.is_empty()
            && catalog.tabs.is_empty();
        Some(SendPackage {
            generation: self.generation,
            expected,
            body: build_body(device_id, machine, catalog),
            hash,
            empty,
        })
    }

    /// Record a completed send — only when the package's generation and owner
    /// are still current. A stale package (account changed mid-`PUT`) is
    /// ignored so the new owner's fresh data, not the old body, goes out.
    fn record_sent(&mut self, package: &SendPackage, now_ms: u64) {
        if self.generation != package.generation || self.current.as_ref() != Some(&package.expected) {
            return;
        }
        self.pending = false;
        self.last_hash = Some(package.hash);
        self.last_sent_ms = Some(now_ms);
        self.last_try_ms = Some(now_ms);
        self.last_try_hash = Some(package.hash);
        #[cfg(feature = "e2ee")]
        {
            self.reseal = false;
        }
    }

    fn record_failed(&mut self, auth_gone: bool, hash: u64, now_ms: u64) {
        self.last_try_ms = Some(now_ms);
        self.last_try_hash = Some(hash);
        if auth_gone {
            // The credential is dead; the auth layer will broadcast expiry and
            // the UI will retire its contributions. Stop retrying meanwhile.
            self.pending = false;
        }
    }

    /// Cached sealed PUT body when neither the catalog content nor the
    /// recipient set changed (e2ee heartbeat resend under the same version).
    #[cfg(feature = "e2ee")]
    fn sealed_hit(&self, hash: u64, recipients: &[String]) -> Option<serde_json::Value> {
        let sent = self.sealed.as_ref()?;
        if sent.hash == hash && sent.recipients == recipients {
            return Some(sent.body.clone());
        }
        None
    }

    /// Remember a sealed publish that was just acknowledged (call after
    /// [`Self::record_sent`], while the generation check still holds).
    #[cfg(feature = "e2ee")]
    fn record_sealed(
        &mut self,
        package: &SendPackage,
        version: u64,
        recipients: Vec<String>,
        body: serde_json::Value,
    ) {
        if self.generation != package.generation
            || self.current.as_ref() != Some(&package.expected)
        {
            return;
        }
        self.sealed = Some(SealedSent {
            hash: package.hash,
            version,
            recipients,
            body,
        });
    }
}

// ── Worker ───────────────────────────────────────────────────────────────────

pub(crate) struct Core {
    state: Mutex<State>,
    wake: Condvar,
}

static CORE: OnceLock<Arc<Core>> = OnceLock::new();

fn core() -> Arc<Core> {
    CORE.get_or_init(|| {
        let core = Arc::new(Core {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker = core.clone();
        let _ = std::thread::Builder::new()
            .name("relay-catalog".into())
            .spawn(move || run(worker));
        core
    })
    .clone()
}

/// Queue a window's snapshot. Latest-only and bounded: replaces any earlier
/// snapshot from the same contributor; never blocks the UI thread.
pub(crate) fn note_snapshot(snap: WindowContribution) {
    let core = core();
    core.state.lock().unwrap().note_snapshot(snap);
    core.wake.notify_one();
}

/// Resolve the exact index already used by the catalog aggregate. The terminal
/// bridge must never allocate a second namespace or accept a retired window.
pub(crate) fn wire_tab_id_for(account: &AccountKey, contributor: &str, local_id: u64) -> Option<u64> {
    let core = core();
    let state = core.state.lock().unwrap();
    if state.current.as_ref() != Some(account) { return None; }
    let contribution = state.contributions.get(contributor)?;
    if &contribution.account != account || !contribution.tabs.iter().any(|tab| tab.local_id == local_id) { return None; }
    super::desktop_catalog::tab_wire_id(*state.indices.get(contributor)?, local_id)
}

/// The catalog index `contributor` publishes under, while it is noted for
/// `account`: what namespaces its workspace and tab wire ids.
pub(crate) fn contributor_index(account: &AccountKey, contributor: &str) -> Option<u32> {
    let core = core();
    let state = core.state.lock().unwrap();
    if state.current.as_ref() != Some(account) { return None; }
    if state.contributions.get(contributor).is_none_or(|c| &c.account != account) { return None; }
    state.indices.get(contributor).copied()
}

/// Retire a contributor (window closed / view dropped). Its workspaces and
/// tabs leave the aggregate; survivors are untouched.
pub(crate) fn note_removed(contributor: &str) {
    super::companion_host::note_removed(contributor);
    let core = core();
    core.state.lock().unwrap().note_removed(contributor);
    core.wake.notify_one();
}

/// Retire a contributor because its view is signed out (or stale after an
/// organisation switch). When nothing remains, pending snapshots and
/// ownership are cleared; no PUT is attempted without credentials.
pub(crate) fn note_signed_out(contributor: &str) {
    super::companion_host::note_signed_out(contributor);
    let core = core();
    core.state.lock().unwrap().note_signed_out(contributor);
    core.wake.notify_one();
}

/// The phones the catalog is sealed for changed (pairing committed, phone
/// revoked, keys reset): seal for the new set and publish now (e2ee only).
/// Never blocks the caller beyond one mutex.
#[cfg(feature = "e2ee")]
pub(crate) fn note_recipients_changed() {
    let core = core();
    {
        let mut state = core.state.lock().unwrap();
        state.reseal = true;
        state.pending = true;
    }
    core.wake.notify_one();
}

/// The live catalog for `account` as the phone's `catalog` stream carries
/// it: `{catalog, machine}` (e2ee only; served inside the channel, §2.1 F4).
/// `None` when this desktop has noted nothing for that account.
#[cfg(feature = "e2ee")]
pub(crate) fn live_snapshot(account: &AccountKey) -> Option<serde_json::Value> {
    static IDENTITY: OnceLock<(String, MachineMeta)> = OnceLock::new();
    let (device_id, machine) = IDENTITY.get_or_init(|| {
        (
            crate::settings::AppSettings::device_id(),
            MachineMeta::current(),
        )
    });
    let catalog = {
        let core = core();
        let state = core.state.lock().unwrap();
        if state.current.as_ref() != Some(account) {
            return None;
        }
        state.aggregate_current()
    };
    validate_for_publish(&catalog).ok()?;
    let body = build_body(device_id, Some(machine.wire()), catalog);
    Some(serde_json::json!({
        "catalog": body,
        "machine": {
            "device_id": device_id,
            "name": machine.name,
            "platform": machine.platform,
            "version": machine.version,
            "last_seen_at": epoch_ms(),
            "online": true,
        },
    }))
}

/// Plaintext worker (pre-cut-over behaviour). With the `e2ee` feature this
/// function is not compiled: the sealed worker below is the only publisher.
#[cfg(not(feature = "e2ee"))]
fn run(core: Arc<Core>) {
    // Disk on first call (settings load) — this is the worker thread, never
    // the UI thread.
    let device_id = crate::settings::AppSettings::device_id();
    let machine = MachineMeta::current().wire();
    loop {
        let pkg = {
            let state = core.state.lock().unwrap();
            let (mut state, _) = core
                .wake
                .wait_timeout(state, Duration::from_millis(super::desktop_catalog::COALESCE_MS))
                .unwrap();
            state.flush_package(&device_id, Some(machine.clone()), epoch_ms())
        };
        let Some(pkg) = pkg else { continue };
        let outcome = put_for_user(&core, &pkg.expected, &pkg.body);
        let now_ms = epoch_ms();
        let mut state = core.state.lock().unwrap();
        match outcome {
            PutOutcome::Sent => state.record_sent(&pkg, now_ms),
            PutOutcome::AuthGone => state.record_failed(true, pkg.hash, now_ms),
            PutOutcome::Transient(_) => state.record_failed(false, pkg.hash, now_ms),
            PutOutcome::Disabled | PutOutcome::Stale => {
                // Disabled (no backend/credential): drop the attempt, keep the
                // data for when publishing is possible again. Stale (owner
                // changed mid-PUT): leave pending so fresh data sends.
                if matches!(outcome, PutOutcome::Disabled) {
                    state.pending = false;
                    state.last_try_ms = Some(now_ms);
                    state.last_try_hash = Some(pkg.hash);
                }
            }
        }
    }
}

/// Sealed worker (§6.5, e2ee only). The only publisher compiled with the
/// feature: the plaintext body built for change detection is sealed in
/// memory and never sent. Fail closed throughout — an unparsable account or
/// device id, a missing keychain, or a seal failure sends nothing.
#[cfg(feature = "e2ee")]
fn run(core: Arc<Core>) {
    let device_id = crate::settings::AppSettings::device_id();
    let machine = MachineMeta::current().wire();
    loop {
        let pkg = {
            let state = core.state.lock().unwrap();
            let (mut state, _) = core
                .wake
                .wait_timeout(state, Duration::from_millis(super::desktop_catalog::COALESCE_MS))
                .unwrap();
            state.flush_package(&device_id, Some(machine.clone()), epoch_ms())
        };
        let Some(pkg) = pkg else { continue };
        let outcome = sealed_publish(&core, &pkg, &device_id);
        let now_ms = epoch_ms();
        let mut state = core.state.lock().unwrap();
        match outcome {
            SealedOutcome::Sent {
                version,
                recipients,
                body,
            } => {
                state.record_sent(&pkg, now_ms);
                state.record_sealed(&pkg, version, recipients, body);
            }
            SealedOutcome::Put(put) => match put {
                PutOutcome::Sent => {
                    // Heartbeat resend of the cached blob: same version.
                    state.record_sent(&pkg, now_ms);
                }
                PutOutcome::AuthGone => state.record_failed(true, pkg.hash, now_ms),
                PutOutcome::Transient(_) => state.record_failed(false, pkg.hash, now_ms),
                PutOutcome::Disabled | PutOutcome::Stale => {
                    if matches!(put, PutOutcome::Disabled) {
                        state.pending = false;
                        state.last_try_ms = Some(now_ms);
                        state.last_try_hash = Some(pkg.hash);
                    }
                }
            },
        }
    }
}

/// Result of the sealed send path.
#[cfg(feature = "e2ee")]
enum SealedOutcome {
    /// A fresh seal was published (new version).
    Sent {
        version: u64,
        recipients: Vec<String>,
        body: serde_json::Value,
    },
    /// A cached heartbeat blob was (re)sent, or the send was skipped.
    Put(PutOutcome),
}

/// Seal `pkg` for the currently paired phones and PUT the sealed blob.
#[cfg(feature = "e2ee")]
fn sealed_publish(core: &Core, pkg: &SendPackage, device_id: &str) -> SealedOutcome {
    use super::e2e_store::desktop as open_desktop;

    let account = match dragent_e2ee::Id::parse(&pkg.expected.user_id) {
        Ok(id) => id,
        Err(_) => {
            log::warn!("[catalog] account id is not a UUID: sending nothing");
            return SealedOutcome::Put(PutOutcome::Transient("account id invalid".into()));
        }
    };
    let desktop = match dragent_e2ee::Id::parse(device_id) {
        Ok(id) => id,
        Err(_) => {
            log::warn!("[catalog] device id is not a UUID: sending nothing");
            return SealedOutcome::Put(PutOutcome::Transient("device id invalid".into()));
        }
    };
    let desktop_state = match open_desktop() {
        Ok(state) => state,
        Err(e) => {
            // No secure key store (or lost keys): phone access stays off.
            // Never fall back to plaintext.
            log::warn!("[catalog] e2ee unavailable, not publishing: {e}");
            return SealedOutcome::Put(PutOutcome::Disabled);
        }
    };
    let mut phones: Vec<(dragent_e2ee::Id, dragent_e2ee::noise::PublicKey)> = {
        let store = desktop_state.store();
        let mut paired = store.paired_phones();
        paired.sort_by(|a, b| a.id.as_bytes().cmp(b.id.as_bytes()));
        paired
            .into_iter()
            .map(|p| (p.id, p.identity))
            .collect()
    };
    phones.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let recipients: Vec<String> = phones.iter().map(|(id, _)| id.to_string()).collect();
    // Heartbeat with unchanged content and recipients: resend the exact
    // sealed bytes under the same version (the phone reports Unchanged).
    // The lookup is its own statement so the state lock is released before
    // `put_for_user` takes it again: inside `if let`, the guard would live
    // to the end of the block and this thread would wait on itself (and
    // the next window draw, which records a snapshot, behind it).
    let cached = core.state.lock().unwrap().sealed_hit(pkg.hash, &recipients);
    if let Some(cached) = cached {
        return SealedOutcome::Put(put_for_user(core, &pkg.expected, &cached));
    }
    // Reserve the version first: it is persisted before sealing, so it is
    // never reused even if sealing or sending fails below.
    let (version, recipients, body) = match seal_new_version(
        &desktop_state,
        &account,
        &desktop,
        &pkg.body,
        phones,
    ) {
        Ok(sealed) => sealed,
        Err(msg) => {
            log::warn!("[catalog] cannot seal catalog: {msg}");
            return SealedOutcome::Put(PutOutcome::Transient(msg));
        }
    };
    match put_for_user(core, &pkg.expected, &body) {
        PutOutcome::Sent => SealedOutcome::Sent {
            version,
            recipients,
            body,
        },
        put => SealedOutcome::Put(put),
    }
}

/// Reserve a fresh catalog version (persisted first, never reused) and seal
/// `catalog_body` for `phones`. Returns the version, the sorted canonical
/// recipient ids, and the relay PUT body (routing + ciphertext only).
#[cfg(feature = "e2ee")]
fn seal_new_version(
    desktop_state: &std::sync::Arc<super::e2e_store::E2eDesktop>,
    account: &dragent_e2ee::Id,
    desktop: &dragent_e2ee::Id,
    catalog_body: &serde_json::Value,
    mut phones: Vec<(dragent_e2ee::Id, dragent_e2ee::noise::PublicKey)>,
) -> Result<(u64, Vec<String>, serde_json::Value), String> {
    phones.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let recipients: Vec<String> = phones.iter().map(|(id, _)| id.to_string()).collect();
    let version = desktop_state
        .store()
        .next_catalog_version()
        .map_err(|e| e.to_string())?;
    let catalog_json = serde_json::to_vec(catalog_body).map_err(|e| e.to_string())?;
    let identity = desktop_state.identity();
    let blob = seal_catalog(account, desktop, version, &catalog_json, &phones, &identity)
        .map_err(|e| e.to_string())?;
    Ok((
        version,
        recipients,
        sealed_put_body(&desktop.to_string(), &blob),
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum PutOutcome {
    Sent,
    /// Credential rejected or absent: no retry until auth state changes.
    AuthGone,
    Transient(String),
    /// No backend configured or no credential held: no network attempted.
    Disabled,
    /// The stored owner is no longer current: never send a stale account's body.
    Stale,
}

/// Send an already-built body for its stored owner.
///
/// `expected` comes from the collector's stamp, confirmed still-current
/// against worker state — identity is never re-read from auth here. The
/// JWT inside `API_CLIENT` remains authoritative server-side.
fn put_for_user(core: &Core, expected: &AccountKey, body: &serde_json::Value) -> PutOutcome {
    {
        let state = core.state.lock().unwrap();
        if state.current.as_ref() != Some(expected) {
            return PutOutcome::Stale;
        }
    }
    if crate::api::backend_url().is_empty() {
        return PutOutcome::Disabled;
    }
    match crate::api::API_CLIENT.put_for_user(
        RELAY_CATALOG_PATH,
        body,
        &expected.user_id,
        PUT_TIMEOUT_SECS,
    ) {
        Ok(_) => PutOutcome::Sent,
        Err(error) => match error {
            crate::api::ApiError::NotAuthenticated => PutOutcome::AuthGone,
            other => PutOutcome::Transient(other.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::desktop_catalog::{
        AccountKey, AgentState, LocalSurface, LocalTab, LocalWorkspace, TabKind,
        WindowContribution,
    };
    use super::*;

    fn account(user: &str) -> AccountKey {
        AccountKey::new(user, "org-1")
    }

    fn contribution(contributor: &str, user: &str, root: &str) -> WindowContribution {
        WindowContribution {
            contributor: contributor.to_string(),
            account: account(user),
            workspaces: vec![LocalWorkspace {
                team: None,
                workspace_id: 1,
                title: "main".to_string(),
                root: root.to_string(),
                branch: Some("main".to_string()),
                project_path: root.to_string(),
                project_name: "repo".to_string(),
            }],
            surfaces: vec![LocalSurface {
                surface_id: 1,
                workspace_id: 1,
                active_tab_id: 1,
                visible_tab_ids: vec![1],
            }],
            tabs: vec![LocalTab {
                local_id: 1,
                surface_id: 1,
                kind: TabKind::Terminal,
                title: "shell".to_string(),
                agent_kind: None,
                agent_state: None,
                attention: None,
                agent_work: None,
                agent_step: None,
                agent_activity: None,
            }],
            standalone_projects: Vec::new(),
        }
    }

    fn tabs_of(body: &serde_json::Value) -> Vec<serde_json::Value> {
        body["tabs"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn account_switch_retires_old_snapshots_without_relabeling() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        let first = state
            .flush_package("dev-1", None, 1_000)
            .expect("first account sends");
        assert_eq!(first.expected.user_id, "alice");
        assert_eq!(first.generation, 1);

        // Organisation/account switch: a new view notes under the new owner.
        state.note_snapshot(contribution("win-b", "bob", "/repo"));
        assert_eq!(state.generation, 2);
        let second = state
            .flush_package("dev-1", None, 2_000)
            .expect("new account sends");
        assert_eq!(second.expected.user_id, "bob");
        let tabs = tabs_of(&second.body);
        assert_eq!(tabs.len(), 1);
        // Bob's window got contributor index 1: alice's tab 1 is gone, not
        // relabeled under bob.
        assert_eq!(tabs[0]["id"], (1u64 << 32) + 1);
        assert_eq!(tabs[0]["workspace_id"], "w1w1");
    }

    #[test]
    fn stale_package_is_never_recorded_as_sent() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        let stale = state.flush_package("dev-1", None, 1_000).unwrap();
        state.note_snapshot(contribution("win-b", "bob", "/repo"));
        state.record_sent(&stale, 2_000);
        // The stale send must not mark bob's data as published.
        assert_eq!(state.generation, 2);
        assert!(state.last_hash.is_none());
        assert!(state.pending);
        let fresh = state.flush_package("dev-1", None, 3_000).unwrap();
        assert_eq!(fresh.expected.user_id, "bob");
    }

    #[test]
    fn signout_clears_pending_snapshots_and_ownership() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        assert!(state.flush_package("dev-1", None, 1_000).is_some());
        state.note_signed_out("win-a");
        assert!(state.current.is_none());
        assert!(!state.pending);
        assert!(state.contributions.is_empty());
        assert!(state.flush_package("dev-1", None, 2_000).is_none());
    }

    #[test]
    fn last_window_removal_publishes_empty_while_signed_in() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        let first = state.flush_package("dev-1", None, 1_000).unwrap();
        state.record_sent(&first, 1_000);
        // Window closed (view dropped): survivors none, but the account is
        // still signed in, so an empty catalog goes out to clear the relay.
        state.note_removed("win-a");
        let clear = state
            .flush_package("dev-1", None, 2_000)
            .expect("empty state must publish while signed in");
        assert!(clear.empty);
        assert_eq!(clear.expected.user_id, "alice");
        assert_eq!(tabs_of(&clear.body).len(), 0);
        let value: CatalogPayload = serde_json::from_value(clear.body).unwrap();
        assert!(super::super::desktop_catalog::validate_for_publish(&value).is_ok());
    }

    #[test]
    fn heartbeat_resends_without_changes() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        let first = state.flush_package("dev-1", None, 0).unwrap();
        state.record_sent(&first, 0);
        assert!(state.flush_package("dev-1", None, 1_000).is_none());
        let beat = state
            .flush_package("dev-1", None, HEARTBEAT_MS)
            .expect("heartbeat must resend");
        assert_eq!(beat.hash, first.hash);
    }

    #[test]
    fn transient_failure_backs_off_until_heartbeat_or_change() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/repo"));
        let pkg = state.flush_package("dev-1", None, 0).unwrap();
        state.record_failed(false, pkg.hash, 0);
        assert!(state.flush_package("dev-1", None, 1_000).is_none());
        assert!(state
            .flush_package("dev-1", None, HEARTBEAT_MS)
            .is_some());
    }

    #[test]
    fn put_refuses_stale_owner_without_network() {
        let core = Core {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        };
        core.state
            .lock()
            .unwrap()
            .note_snapshot(contribution("win-a", "alice", "/repo"));
        // Bob was never the confirmed owner: no network is attempted.
        let outcome = put_for_user(&core, &account("mallory"), &serde_json::Value::Null);
        assert_eq!(outcome, PutOutcome::Stale);
    }

    #[test]
    fn agent_tab_carries_explicit_identity_only() {
        let mut snap = contribution("win-a", "alice", "/repo");
        snap.tabs[0].kind = TabKind::Agent;
        snap.tabs[0].agent_kind = Some("claude".to_string());
        snap.tabs[0].agent_state = Some(AgentState::Working);
        let mut state = State::default();
        state.note_snapshot(snap);
        let pkg = state.flush_package("dev-1", None, 0).unwrap();
        let tabs = tabs_of(&pkg.body);
        assert_eq!(tabs[0]["kind"], "agent");
        assert_eq!(tabs[0]["agent_kind"], "claude");
        assert_eq!(tabs[0]["agent_state"], "working");
    }

    #[test]
    fn idle_open_contribution_survives_long_silence() {
        // An idle open desktop may go a long time without re-rendering. Its
        // contribution is retained until explicit close/sign-out: heartbeats
        // keep publishing the same hierarchy, never an empty catalog.
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/a"));
        let first = state.flush_package("dev-1", None, 0).unwrap();
        state.record_sent(&first, 0);
        for now in [60_000, 300_000, 3_600_000, 86_400_000] {
            let beat = state
                .flush_package("dev-1", None, now)
                .expect("heartbeat must publish while the window is open");
            assert_eq!(beat.hash, first.hash);
            assert!(!beat.empty);
            let tabs = tabs_of(&beat.body);
            assert_eq!(tabs.len(), 1);
            assert_eq!(tabs[0]["workspace_id"], "w0w1");
            let value: CatalogPayload = serde_json::from_value(beat.body.clone()).unwrap();
            assert_eq!(value.projects.len(), 1);
            assert_eq!(value.projects[0].path, "/a");
        }
    }

    #[test]
    fn organisation_switch_retires_stale_view_data() {
        let mut state = State::default();
        state.note_snapshot(contribution("win-a", "alice", "/a"));
        // Fresh view under the new organisation notes under the new account;
        // the stale old-org snapshot under the same window is discarded,
        // never relabeled.
        let mut snap = contribution("win-a", "alice", "/a");
        snap.account = AccountKey::new("alice", "org-2");
        state.note_snapshot(snap);
        assert_eq!(state.generation, 2);
        let pkg = state.flush_package("dev-1", None, 2_000).unwrap();
        assert_eq!(pkg.expected.org, "org-2");
        assert_eq!(pkg.expected.user_id, "alice");
    }

    #[test]
    fn machine_metadata_reports_real_platform() {
        let meta = MachineMeta::current();
        assert_eq!(meta.platform, std::env::consts::OS);
        assert!(!meta.name.is_empty());
        assert!(meta.version.is_none());
    }
}

/// §6.5 publisher tests (e2ee only): the sealed PUT body carries no
/// plaintext catalog field, heartbeats resend the cached version, and a
/// revoked phone is excluded from the next seal. The phone side is scripted
/// with the real `dragent_e2ee` unwrap/open/high-water checks.
#[cfg(all(test, feature = "e2ee"))]
mod e2ee_tests {
    use super::super::desktop_catalog::{
        AccountKey, LocalSurface, LocalTab, LocalWorkspace, TabKind, WindowContribution,
    };
    use super::*;
    use crate::realtime::e2e_keys::MemorySecretStore;
    use crate::realtime::e2e_store::tests::{device, TempHome};
    use crate::realtime::e2e_store::E2eDesktop;
    use dragent_e2ee::codec::catalog_blob;
    use dragent_e2ee::noise::{CatalogPrologue, StaticKeypair};
    use dragent_e2ee::seal::catalog as seal;
    use dragent_e2ee::seal::pin_store::{CatalogCheck, PinStore};

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        h.finalize().into()
    }

    fn open_as_phone(
        phone_keys: &StaticKeypair,
        pinned_desktop: &dragent_e2ee::noise::PublicKey,
        account: &dragent_e2ee::Id,
        desktop: &dragent_e2ee::Id,
        blob: &catalog_blob::CatalogBlob,
        phone: &dragent_e2ee::Id,
    ) -> Vec<u8> {
        let wrap = blob
            .recipients
            .iter()
            .find(|r| &r.phone == phone)
            .expect("wrap for phone")
            .wrap
            .clone();
        let prologue = CatalogPrologue {
            account: *account,
            desktop: *desktop,
            phone: *phone,
            version: blob.version,
        };
        let k = seal::unwrap_key(phone_keys, pinned_desktop, &prologue, &wrap).unwrap();
        seal::open_body(&k, account, desktop, blob.version, &blob.nonce, &blob.body)
            .unwrap()
            .to_vec()
    }

    fn desktop_with_phone(
        tag: &str,
        phone_id: dragent_e2ee::Id,
        phone_pub: dragent_e2ee::noise::PublicKey,
    ) -> (TempHome, std::sync::Arc<E2eDesktop>) {
        let home = TempHome::new(tag);
        let secrets = std::sync::Arc::new(MemorySecretStore::new());
        let desktop = std::sync::Arc::new(
            E2eDesktop::open(&home.0, device(), secrets, 1_000).unwrap(),
        );
        desktop
            .store()
            .pin_phone(phone_id, phone_pub, phone_pub, "Pixel".into(), 5)
            .unwrap();
        (home, desktop)
    }

    /// Decode a sealed PUT body back to its blob (the envelope adds
    /// `device_id`; the blob bytes are re-padded for the codec check).
    fn put_to_blob(body: &serde_json::Value) -> catalog_blob::CatalogBlob {
        let mut v = body.clone();
        v.as_object_mut().unwrap().remove("device_id");
        let bytes = serde_json::to_vec(&v).unwrap();
        let len =
            bytes.len().div_ceil(catalog_blob::PAD_BUCKET) * catalog_blob::PAD_BUCKET;
        let mut padded = bytes;
        padded.resize(len, b' ');
        catalog_blob::decode(&padded).unwrap()
    }

    fn canary_contribution() -> WindowContribution {
        WindowContribution {
            contributor: "win-a".to_string(),
            account: AccountKey::new(
                "11111111-2222-4333-8444-555555555555",
                "org-1",
            ),
            workspaces: vec![LocalWorkspace {
                team: None,
                workspace_id: 1,
                title: "canary-workspace-title-7q".to_string(),
                root: "/Users/person/canary-project-path-7q".to_string(),
                branch: Some("canary-branch-name-7q".to_string()),
                project_path: "/Users/person/canary-project-path-7q".to_string(),
                project_name: "canary-project-7q".to_string(),
            }],
            surfaces: vec![LocalSurface {
                surface_id: 1,
                workspace_id: 1,
                active_tab_id: 1,
                visible_tab_ids: vec![1],
            }],
            tabs: vec![LocalTab {
                local_id: 1,
                surface_id: 1,
                kind: TabKind::Terminal,
                title: "canary-tab-title-7q".to_string(),
                agent_kind: None,
                agent_state: None,
                attention: None,
                agent_work: None,
                agent_step: None,
                agent_activity: None,
            }],
            standalone_projects: Vec::new(),
        }
    }

    #[test]
    fn sealed_put_body_carries_no_plaintext_canary() {
        let phone_id = dragent_e2ee::Id([0xaa; 16]);
        let phone_keys = StaticKeypair::generate().unwrap();
        let (_home, desktop) = desktop_with_phone("noleak", phone_id, phone_keys.public());
        let account = dragent_e2ee::Id::parse("11111111-2222-4333-8444-555555555555").unwrap();
        let desktop_id = device();

        let mut state = State::default();
        state.note_snapshot(canary_contribution());
        let machine = WireMachine {
            name: "canary-machine-name-7q".to_string(),
            platform: "macos".to_string(),
            version: None,
        };
        let pkg = state
            .flush_package(&desktop_id.to_string(), Some(machine), 1_000)
            .expect("package");
        // The plaintext change-detection body does contain the fields (it is
        // sealed in memory and never sent).
        let plain = serde_json::to_string(&pkg.body).unwrap();
        assert!(plain.contains("canary-tab-title-7q"));

        let phones = vec![(phone_id, phone_keys.public())];
        let (version, recipients, body) =
            seal_new_version(&desktop, &account, &desktop_id, &pkg.body, phones).unwrap();
        assert_eq!(recipients, vec![phone_id.to_string()]);
        let put_bytes = serde_json::to_vec(&body).unwrap();
        for canary in [
            "canary-machine-name-7q",
            "canary-project-path-7q",
            "canary-branch-name-7q",
            "canary-tab-title-7q",
            "canary-workspace-title-7q",
        ] {
            assert!(
                !put_bytes.windows(canary.len()).any(|w| w == canary.as_bytes()),
                "plaintext canary {canary:?} in PUT body"
            );
        }
        // ... nor any plaintext field name outside the seal.
        for key in ["projects", "workspaces", "tabs", "machine"] {
            let quoted = format!("\"{key}\"");
            assert!(
                !put_bytes
                    .windows(quoted.len())
                    .any(|w| w == quoted.as_bytes()),
                "plaintext key {key:?} outside seal"
            );
        }
        // The phone still reads everything back.
        let blob = put_to_blob(&body);
        assert_eq!(blob.version, version);
        let pt = open_as_phone(
            &phone_keys,
            &desktop.public(),
            &account,
            &desktop_id,
            &blob,
            &phone_id,
        );
        let v: serde_json::Value = serde_json::from_slice(&pt).unwrap();
        assert_eq!(v["tabs"][0]["title"].as_str().unwrap(), "canary-tab-title-7q");
    }

    #[test]
    fn phone_rejects_rollback_and_equivocation_through_publisher_seals() {
        let phone_id = dragent_e2ee::Id([0xbb; 16]);
        let phone_keys = StaticKeypair::generate().unwrap();
        let (_home, desktop) = desktop_with_phone("rollback", phone_id, phone_keys.public());
        let account = dragent_e2ee::Id::parse("11111111-2222-4333-8444-555555555555").unwrap();
        let desktop_id = device();
        let phones = vec![(phone_id, phone_keys.public())];

        let mut marks = PinStore::default();
        marks.catalog_set_floor(desktop_id, 1);

        let (_, _, body5) = seal_new_version(
            &desktop,
            &account,
            &desktop_id,
            &serde_json::json!({"projects":[]}),
            phones.clone(),
        )
        .unwrap();
        let blob5 = put_to_blob(&body5);
        let pt5 = open_as_phone(
            &phone_keys,
            &desktop.public(),
            &account,
            &desktop_id,
            &blob5,
            &phone_id,
        );
        assert_eq!(
            marks.catalog_check(&desktop_id, blob5.version, &sha256(&pt5)),
            CatalogCheck::Accept
        );
        marks.catalog_accept(desktop_id, blob5.version, &sha256(&pt5));

        // A newer seal is accepted...
        let (_, _, body9) = seal_new_version(
            &desktop,
            &account,
            &desktop_id,
            &serde_json::json!({"projects":[{"path":"/new"}]}),
            phones.clone(),
        )
        .unwrap();
        let blob9 = put_to_blob(&body9);
        let pt9 = open_as_phone(
            &phone_keys,
            &desktop.public(),
            &account,
            &desktop_id,
            &blob9,
            &phone_id,
        );
        assert_eq!(
            marks.catalog_check(&desktop_id, blob9.version, &sha256(&pt9)),
            CatalogCheck::Accept
        );
        marks.catalog_accept(desktop_id, blob9.version, &sha256(&pt9));
        // ...then the old blob is a rollback.
        assert_eq!(
            marks.catalog_check(&desktop_id, blob5.version, &sha256(&pt5)),
            CatalogCheck::Rollback
        );
        // Same version, different body: equivocation (via a direct seal at
        // the same version to simulate a forked body).
        let same_version_blob = {
            use crate::realtime::desktop_catalog::seal_catalog;
            let json = serde_json::to_vec(&serde_json::json!({"projects":[{"path":"/evil"}]}))
                .unwrap();
            let identity = desktop.identity();
            let blob = seal_catalog(
                &account,
                &desktop_id,
                blob9.version,
                &json,
                &phones,
                &identity,
            )
            .unwrap();
            blob
        };
        let pt_evil = open_as_phone(
            &phone_keys,
            &desktop.public(),
            &account,
            &desktop_id,
            &same_version_blob,
            &phone_id,
        );
        assert_ne!(pt9, pt_evil);
        assert_eq!(
            marks.catalog_check(&desktop_id, same_version_blob.version, &sha256(&pt_evil)),
            CatalogCheck::Equivocation
        );
    }

    #[test]
    fn heartbeat_resends_cached_version_and_revocation_excludes_phone() {
        let phone_id = dragent_e2ee::Id([0xcc; 16]);
        let other_id = dragent_e2ee::Id([0xdd; 16]);
        let phone_keys = StaticKeypair::generate().unwrap();
        let other_keys = StaticKeypair::generate().unwrap();
        let (_home, desktop) = desktop_with_phone("cache", phone_id, phone_keys.public());
        let account = dragent_e2ee::Id::parse("11111111-2222-4333-8444-555555555555").unwrap();
        let desktop_id = device();

        let mut state = State::default();
        let mut snap = canary_contribution();
        snap.account = AccountKey::new(
            "11111111-2222-4333-8444-555555555555",
            "org-1",
        );
        state.note_snapshot(snap);
        let pkg = state
            .flush_package(&desktop_id.to_string(), None, 1_000)
            .unwrap();
        let phones = vec![(phone_id, phone_keys.public())];
        let (v1, recipients, body) =
            seal_new_version(&desktop, &account, &desktop_id, &pkg.body, phones).unwrap();
        state.record_sent(&pkg, 1_000);
        state.record_sealed(&pkg, v1, recipients.clone(), body.clone());
        // Unchanged content + recipients: cache hit, no new version.
        let hit = state.sealed_hit(pkg.hash, &recipients).expect("cache hit");
        assert_eq!(hit, body);

        // Revoke the phone and pair another: the next seal excludes it.
        desktop.store().revoke(&phone_id, 2_000).unwrap();
        desktop
            .store()
            .pin_phone(other_id, other_keys.public(), other_keys.public(), "P2".into(), 3_000)
            .unwrap();
        let (v2, recipients2, body2) = seal_new_version(
            &desktop,
            &account,
            &desktop_id,
            &pkg.body,
            vec![(other_id, other_keys.public())],
        )
        .unwrap();
        assert!(v2 > v1);
        assert_eq!(recipients2, vec![other_id.to_string()]);
        assert_ne!(recipients2, recipients);
        let put2 = serde_json::to_string(&body2).unwrap();
        assert!(!put2.contains(&phone_id.to_string()));
        assert!(put2.contains(&other_id.to_string()));
        // Stale packages never poison the cache.
        let mut stale_snap = canary_contribution();
        stale_snap.account = AccountKey::new(
            "11111111-2222-4333-8444-555555555555",
            "org-2",
        );
        state.note_snapshot(stale_snap);
        // Heartbeat is due by now, so the org-2 snapshot flushes.
        let stale_pkg = state
            .flush_package(&desktop_id.to_string(), None, 1_000 + HEARTBEAT_MS)
            .unwrap();
        state.record_sealed(&pkg, 9999, vec![], body.clone());
        assert_eq!(state.sealed.as_ref().unwrap().version, v1);
        let _ = stale_pkg;
    }
}
