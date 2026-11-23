//! OS-keychain credential ownership for provider keys (M2).
//!
//! Design: on native builds the keychain is the SOLE durable store. The
//! settings JSON file is a one-time migration source (pre-M2 records) and
//! never a destination — key fields are `serde(skip)` there, so `save()`
//! cannot write them back. In-memory maps (`ProviderKeys`,
//! `CustomModel.api_key`) stay fully populated, so every existing reader
//! resolves unchanged.
//!
//! Precedence per slot: file value (fresh paste / legacy record) wins for
//! the session and is pushed to the keychain best-effort; otherwise the
//! keychain value; otherwise nothing. A failed keychain write degrades to
//! session memory with a warning — never silent loss, never a crash.
//!
//! Wasm has no keychain backend: all of this compiles to pass-throughs and
//! the JSON file keeps behaving exactly as before.

use super::app_settings::{CustomModel, ProviderKeys};

/// Every settings key slot owned by the keychain. The drift test below
/// requires each entry to round-trip through `ProviderKeys::get/set` —
/// add the slot in both places or the build fails the test, not review.
pub const KEY_SLOTS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "xai",
    "groq",
    "cerebras",
    "openrouter",
    "meta",
    "deepseek",
    "qwen",
    "zhipu",
    "moonshot",
    "minimax",
    "mistral",
    "together",
    "deepinfra",
    "perplexity",
    "venice",
    "nvidia",
    "fireworks",
    "cohere",
];

/// Keychain account for a provider slot. The slot string is the credential
/// id the client sends instead of any secret (M3 grant/credential contract).
pub fn account_for_slot(slot: &str) -> String {
    format!("provider/{slot}")
}

/// Keychain account for a custom model's key, keyed by model id.
pub fn account_for_custom(model_id: &str) -> String {
    format!("custom/{}", model_id.trim())
}

// ── Keychain reads ───────────────────────────────────────────────────────────
//
// What this process has already read from the keychain, including items that
// weren't there or that the user declined. Settings load on hot paths such as
// every agent step, and on macOS a read the item doesn't already allow is a
// permission popup, so each item is read at most once per run.
//
// The read itself can block for as long as macOS shows that popup, so the
// desktop app never reads on its UI thread: `start_keychain_warmup` (called at
// launch, on the UI thread) hands every known account to one reader thread,
// and from then on a UI-thread load answers from the cache alone. An account
// the cache doesn't know yet reads as absent there and is queued for the
// reader; when a batch lands the global settings snapshot is reloaded and
// subscribers (`subscribe_keychain_reads`) are told, so open views can pick
// the keys up. Other threads wait for an in-flight read of the same account
// instead of starting a second one (no second popup), then read the cache.
// Processes that never start the warm-up (CLI, tests) keep reading inline.

/// The cache plus which accounts some thread is reading right now.
#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
struct KeychainReads {
    cache: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
    in_flight: std::sync::Mutex<std::collections::HashSet<String>>,
    landed: std::sync::Condvar,
}

#[cfg(not(target_family = "wasm"))]
impl KeychainReads {
    /// What the cache holds for `account`: `None` when it was never read.
    fn cached(&self, account: &str) -> Option<Option<String>> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account)
            .cloned()
    }

    /// Record a value this process knows to be current (a write it made).
    fn remember(&self, account: &str, value: Option<String>) {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account.to_string(), value);
    }

    /// Claim the accounts that are neither cached nor already being read;
    /// the caller must `land` each one it gets back.
    fn claim(&self, accounts: impl IntoIterator<Item = String>) -> Vec<String> {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let mut in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
        let mut claimed = Vec::new();
        for account in accounts {
            if !cache.contains_key(&account) && in_flight.insert(account.clone()) {
                claimed.push(account);
            }
        }
        claimed
    }

    /// Finish a claimed read. A value written while the read was in flight
    /// is newer, so it stays. With `cache_it` false the claim is given up
    /// without caching anything (the read never happened).
    fn land(&self, account: &str, value: Option<String>, cache_it: bool) {
        if cache_it {
            self.cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(account.to_string())
                .or_insert(value);
        }
        // Remove under the lock the waiters wait on, so none misses the wake.
        self.in_flight
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account);
        self.landed.notify_all();
    }

    /// Cache first; else wait out another thread's read of the same
    /// account; else read it here, once.
    fn load_blocking(
        &self,
        account: &str,
        read: impl FnOnce(&str) -> Option<String>,
    ) -> Option<String> {
        loop {
            if let Some(value) = self.cached(account) {
                return value;
            }
            let in_flight = self.in_flight.lock().unwrap_or_else(|p| p.into_inner());
            if in_flight.contains(account) {
                drop(
                    self.landed
                        .wait_while(in_flight, |f| f.contains(account))
                        .unwrap_or_else(|p| p.into_inner()),
                );
                continue;
            }
            drop(in_flight);
            if self.claim([account.to_string()]).is_empty() {
                continue; // cached or claimed in between: look again
            }
            let value = read(account);
            self.land(account, value.clone(), true);
            return self.cached(account).unwrap_or(value);
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn keychain_reads() -> &'static KeychainReads {
    static READS: std::sync::OnceLock<KeychainReads> = std::sync::OnceLock::new();
    READS.get_or_init(Default::default)
}

/// The thread that must never wait on the keychain; set by the warm-up.
#[cfg(not(target_family = "wasm"))]
static UI_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

#[cfg(not(target_family = "wasm"))]
fn on_ui_thread() -> bool {
    UI_THREAD.get() == Some(&std::thread::current().id())
}

#[cfg(not(target_family = "wasm"))]
thread_local! {
    /// Set on the reader thread. Its settings reload must not wait for an
    /// account it has been sent but hasn't reached yet (it would wait on
    /// itself), so it too answers from the cache and catches up next batch.
    static ON_READER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this thread must answer keychain loads from the cache alone.
#[cfg(not(target_family = "wasm"))]
fn cache_only() -> bool {
    on_ui_thread() || ON_READER_THREAD.with(std::cell::Cell::get)
}

/// The blocking keychain read itself.
#[cfg(not(target_family = "wasm"))]
fn read_keychain_item(account: &str) -> Option<String> {
    keyring::Entry::new(dragent_paths::keychain_service(), account)
        .ok()?
        .get_password()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(not(target_family = "wasm"))]
fn keychain_load(account: &str) -> Option<String> {
    let reads = keychain_reads();
    if cache_only() {
        return match reads.cached(account) {
            Some(value) => value,
            None => {
                queue_keychain_reads(vec![account.to_string()]);
                None
            }
        };
    }
    reads.load_blocking(account, read_keychain_item)
}

#[cfg(not(target_family = "wasm"))]
fn remember_keychain_value(account: &str, value: Option<String>) {
    keychain_reads().remember(account, value);
}

/// Hand accounts the cache doesn't know to the reader thread. Never blocks.
#[cfg(not(target_family = "wasm"))]
fn queue_keychain_reads(accounts: Vec<String>) {
    let claimed = keychain_reads().claim(accounts);
    if claimed.is_empty() {
        return;
    }
    if let Err(std::sync::mpsc::SendError(claimed)) = keychain_reader().send(claimed) {
        log::warn!("[credentials] keychain reader is gone; keys stay unread");
        for account in claimed {
            keychain_reads().land(&account, None, false);
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn keychain_reader() -> &'static std::sync::mpsc::Sender<Vec<String>> {
    static READER: std::sync::OnceLock<std::sync::mpsc::Sender<Vec<String>>> =
        std::sync::OnceLock::new();
    READER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
        let spawned = std::thread::Builder::new()
            .name("dragent-keychain-read".into())
            .spawn(move || {
                ON_READER_THREAD.with(|on| on.set(true));
                while let Ok(mut batch) = rx.recv() {
                    for more in rx.try_iter() {
                        batch.extend(more);
                    }
                    for account in batch {
                        let value = read_keychain_item(&account);
                        keychain_reads().land(&account, value, true);
                    }
                    // Every claimed account is cached now, so this load
                    // doesn't touch the keychain.
                    super::app_settings::AppSettings::reload_global();
                    notify_keychain_subscribers();
                }
            });
        if let Err(e) = spawned {
            log::warn!("[credentials] keychain reader thread failed to start: {e}");
        }
        tx
    })
}

#[cfg(not(target_family = "wasm"))]
type Subscribers = std::sync::Mutex<Vec<async_channel::Sender<()>>>;

#[cfg(not(target_family = "wasm"))]
fn keychain_subscribers() -> &'static Subscribers {
    static SUBSCRIBERS: std::sync::OnceLock<Subscribers> = std::sync::OnceLock::new();
    SUBSCRIBERS.get_or_init(Default::default)
}

#[cfg(not(target_family = "wasm"))]
fn notify_keychain_subscribers() {
    keychain_subscribers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|tx| tx.try_send(()).is_ok());
}

/// A message arrives each time background keychain reads land, so keys a
/// UI-thread load reported absent may now be there. Subscribe before the
/// load whose result you want refreshed, so a batch can't land in between.
pub fn subscribe_keychain_reads() -> async_channel::Receiver<()> {
    let (tx, rx) = async_channel::unbounded();
    #[cfg(not(target_family = "wasm"))]
    keychain_subscribers()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(tx);
    #[cfg(target_family = "wasm")]
    drop(tx);
    rx
}

/// Call once at launch, on the UI thread, before the first settings load:
/// from then on this thread never reads the keychain itself. Every provider
/// slot and every custom model named in the settings file is read in the
/// background, in one batch.
pub fn start_keychain_warmup() {
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = UI_THREAD.set(std::thread::current().id());
        let raw = std::fs::read_to_string(super::app_settings::AppSettings::config_path())
            .unwrap_or_default();
        queue_keychain_reads(warmup_accounts(&raw));
    }
}

/// Accounts to warm: all provider slots, then each custom model's.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn warmup_accounts(settings_json: &str) -> Vec<String> {
    let mut accounts: Vec<String> = KEY_SLOTS.iter().map(|s| account_for_slot(s)).collect();
    let value = serde_json::from_str::<serde_json::Value>(settings_json).unwrap_or_default();
    if let Some(models) = value.get("custom_models").and_then(|m| m.as_array()) {
        for model in models {
            if let Some(id) = model.get("model_id").and_then(|id| id.as_str()) {
                if !id.trim().is_empty() {
                    accounts.push(account_for_custom(id));
                }
            }
        }
    }
    accounts
}

/// Whether this process knows what the keychain holds for `account` (it
/// read or wrote it). Always true where there is no keychain.
pub fn keychain_known(account: &str) -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        keychain_reads().cached(account).is_some()
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = account;
        true
    }
}

/// Resolve one credential to its in-memory value: file value wins (and is
/// pushed to the keychain best-effort), else the keychain value, else
/// nothing. `load`/`save` are injected so the policy is unit-testable
/// without touching the real keychain.
pub fn resolve_key(
    file_value: Option<&str>,
    account: &str,
    load: impl Fn(&str) -> Option<String>,
    save: impl Fn(&str, &str) -> bool,
) -> Option<String> {
    let file_key = file_value.map(str::trim).filter(|s| !s.is_empty());
    match (file_key, load(account)) {
        (Some(fresh), _) => {
            if !save(account, fresh) {
                log::warn!("[credentials] {account}: keychain persist failed; session-only");
            }
            Some(fresh.to_string())
        }
        (None, Some(stored)) => Some(stored),
        (None, None) => None,
    }
}

/// The load-time write-through (migration of a key still in the settings
/// file), skipped when this process already knows the keychain holds exactly
/// this value. `AppSettings::load` runs on hot paths, often on the UI thread,
/// so the write is queued, never made here; the cache shows it at once.
#[cfg(not(target_family = "wasm"))]
fn keychain_save_if_changed(account: &str, key: &str) -> bool {
    if keychain_reads().cached(account).flatten().as_deref() == Some(key.trim()) {
        return true;
    }
    queue_keychain_write(account.to_string(), Some(key.to_string())).is_some()
}

/// Provider-slot flavor of [`resolve_key`] against the real keychain.
#[cfg(not(target_family = "wasm"))]
pub fn resolve_provider_slot(file_value: Option<&str>, slot: &str) -> Option<String> {
    resolve_key(
        file_value,
        &account_for_slot(slot),
        keychain_load,
        keychain_save_if_changed,
    )
}

/// Custom-model flavor of [`resolve_key`] against the real keychain.
#[cfg(not(target_family = "wasm"))]
pub fn resolve_custom_key(file_value: Option<&str>, model_id: &str) -> Option<String> {
    resolve_key(
        file_value,
        &account_for_custom(model_id),
        keychain_load,
        keychain_save_if_changed,
    )
}

// ── Queued keychain writes ───────────────────────────────────────────────────
//
// A keychain write can block for as long as macOS shows an access prompt, so
// the settings UI never writes on its own thread. Writes go to one worker
// thread, in order; a burst for the same account (one per keystroke while a
// key is typed) collapses to its last value. The process cache is updated at
// once, so every reader sees the new value before the write lands.

/// What finished a queued write: `Ok` once stored, `Err` with a sentence the
/// UI can show. A write overtaken by a newer one for the same account never
/// answers (its receiver closes), so only the latest outcome is reported.
pub type KeychainOutcome = Result<(), String>;

#[cfg(not(target_family = "wasm"))]
struct KeychainJob {
    account: String,
    /// `None` deletes the item.
    value: Option<String>,
    done: async_channel::Sender<KeychainOutcome>,
}

/// Keep only the last job per account, in the order those last jobs came.
/// Dropped jobs close their reply channel unanswered.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn latest_per_account<T>(jobs: Vec<T>, account: impl Fn(&T) -> &str) -> Vec<T> {
    let mut kept: Vec<T> = Vec::with_capacity(jobs.len());
    for job in jobs {
        kept.retain(|earlier| account(earlier) != account(&job));
        kept.push(job);
    }
    kept
}

#[cfg(not(target_family = "wasm"))]
fn keychain_writer() -> &'static std::sync::mpsc::Sender<KeychainJob> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<KeychainJob>> =
        std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<KeychainJob>();
        let spawned = std::thread::Builder::new()
            .name("dragent-keychain".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch = vec![first];
                    batch.extend(rx.try_iter());
                    for job in latest_per_account(batch, |job| job.account.as_str()) {
                        let outcome = write_keychain_item(&job.account, job.value.as_deref());
                        let _ = job.done.send_blocking(outcome);
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("[credentials] keychain writer thread failed to start: {e}");
        }
        tx
    })
}

/// The blocking keychain call itself; runs only on the writer thread.
#[cfg(not(target_family = "wasm"))]
fn write_keychain_item(account: &str, value: Option<&str>) -> KeychainOutcome {
    let entry = keyring::Entry::new(dragent_paths::keychain_service(), account).map_err(|e| {
        log::warn!("[credentials] keychain unavailable for {account}: {e}");
        format!("The Keychain is unavailable ({e}). The key works until Dragent quits.")
    })?;
    match value {
        Some(key) => entry.set_password(key).map_err(|e| {
            log::warn!("[credentials] keychain save failed for {account}: {e}");
            format!("Not saved to the Keychain ({e}). The key works until Dragent quits.")
        }),
        None => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => {
                log::warn!("[credentials] keychain delete failed for {account}: {e}");
                Err(format!("Couldn't remove the key from the Keychain ({e})."))
            }
        },
    }
}

/// Queue a keychain write (`None` deletes) without blocking the caller. The
/// in-process view of the keychain changes immediately. Returns where the
/// outcome arrives, or `None` when there is no keychain (wasm) or the writer
/// is gone.
pub fn queue_keychain_write(
    account: String,
    value: Option<String>,
) -> Option<async_channel::Receiver<KeychainOutcome>> {
    #[cfg(not(target_family = "wasm"))]
    {
        let value = value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        remember_keychain_value(&account, value.clone());
        let (done, outcome) = async_channel::bounded(1);
        let job = KeychainJob {
            account,
            value,
            done,
        };
        if keychain_writer().send(job).is_err() {
            log::warn!("[credentials] keychain writer is gone; key is session-only");
            return None;
        }
        Some(outcome)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = (account, value);
        None
    }
}

/// Write-through for the settings UI: the in-memory map changes now, the
/// keychain write (native) is queued. Empty values delete instead of storing
/// blanks. Returns the account and where its outcome arrives.
pub fn save_provider_key(
    keys: &mut ProviderKeys,
    slot: &str,
    value: String,
) -> Option<async_channel::Receiver<KeychainOutcome>> {
    if value.trim().is_empty() {
        return delete_provider_key(keys, slot);
    }
    let outcome = queue_keychain_write(account_for_slot(slot), Some(value.clone()));
    keys.set(slot, value);
    outcome
}

/// Delete from both stores (the keychain side is queued).
pub fn delete_provider_key(
    keys: &mut ProviderKeys,
    slot: &str,
) -> Option<async_channel::Receiver<KeychainOutcome>> {
    keys.clear(slot);
    queue_keychain_write(account_for_slot(slot), None)
}

/// Resolve a custom model's key: keychain first (native), legacy field as
/// fallback (wasm, plus pre-migration records mid-flight). Native reads come
/// from the process cache, which settings load has already filled.
pub fn custom_api_key(model: &CustomModel) -> Option<String> {
    #[cfg(not(target_family = "wasm"))]
    {
        if let Some(stored) = keychain_load(&account_for_custom(&model.model_id)) {
            return Some(stored);
        }
    }
    model
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Persist a custom model's key. Returns the in-memory value to keep on the
/// record (never written to disk on native: `save` scrubs it) and where the
/// queued keychain write's outcome arrives.
///
/// A blank key normally deletes the stored one. Not while the stored one is
/// still being read in the background, though: the edit form showed it blank
/// only because it hadn't arrived yet, so the stored key is left alone.
pub fn save_custom_key(
    model_id: &str,
    key: &str,
) -> (
    Option<String>,
    Option<async_channel::Receiver<KeychainOutcome>>,
) {
    let trimmed = key.trim();
    let in_memory = (!trimmed.is_empty()).then(|| trimmed.to_string());
    let account = account_for_custom(model_id);
    if in_memory.is_none() && !blank_may_delete(&account) {
        return (None, None);
    }
    let outcome = queue_keychain_write(account, in_memory.clone());
    (in_memory, outcome)
}

/// Whether a blank custom-model key may delete what the keychain holds: only
/// once this process knows what that is, when reads happen in the background.
fn blank_may_delete(account: &str) -> bool {
    #[cfg(not(target_family = "wasm"))]
    {
        !on_ui_thread() || keychain_known(account)
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = account;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_slot_round_trips_through_provider_keys() {
        let mut keys = ProviderKeys::default();
        for slot in KEY_SLOTS {
            assert_eq!(keys.get(slot), None);
            keys.set(slot, "k".to_string());
            assert_eq!(keys.get(slot), Some("k"));
            keys.clear(slot);
            assert_eq!(keys.get(slot), None);
        }
    }

    #[test]
    fn account_namespacing_is_stable() {
        assert_eq!(account_for_slot("anthropic"), "provider/anthropic");
        assert_eq!(account_for_custom("m"), "custom/m");
        assert_eq!(account_for_custom("  spaced  "), "custom/spaced");
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn each_keychain_item_is_read_once_even_when_missing() {
        let cache = KeychainReads::default();
        let reads = std::cell::Cell::new(0);
        let read = |value: Option<&str>| {
            reads.set(reads.get() + 1);
            value.map(str::to_string)
        };
        for _ in 0..3 {
            assert_eq!(
                cache.load_blocking("a", |_| read(Some("k"))).as_deref(),
                Some("k")
            );
            assert_eq!(cache.load_blocking("absent", |_| read(None)), None);
        }
        assert_eq!(reads.get(), 2);
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn an_unread_account_is_unknown_not_absent() {
        let cache = KeychainReads::default();
        assert_eq!(cache.cached("a"), None);
        cache.remember("a", None);
        assert_eq!(cache.cached("a"), Some(None));
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn claiming_skips_cached_and_in_flight_accounts() {
        let cache = KeychainReads::default();
        cache.remember("known", Some("k".into()));
        let first = cache.claim(["known".to_string(), "a".into(), "b".into()]);
        assert_eq!(first, vec!["a".to_string(), "b".into()]);
        // Already being read: nobody claims them twice (no second popup).
        assert!(cache.claim(["a".to_string(), "b".into()]).is_empty());
        cache.land("a", Some("ka".into()), true);
        cache.land("b", None, false); // given up: still unknown, claimable
        assert_eq!(cache.cached("a"), Some(Some("ka".into())));
        assert_eq!(cache.cached("b"), None);
        assert_eq!(
            cache.claim(["a".to_string(), "b".into()]),
            vec!["b".to_string()]
        );
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn a_write_during_a_read_beats_the_read() {
        let cache = KeychainReads::default();
        assert_eq!(cache.claim(["a".to_string()]), vec!["a".to_string()]);
        cache.remember("a", Some("typed".into()));
        cache.land("a", Some("stale".into()), true);
        assert_eq!(cache.cached("a"), Some(Some("typed".into())));
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn a_background_load_waits_for_the_read_in_flight() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let cache = Arc::new(KeychainReads::default());
        assert_eq!(cache.claim(["a".to_string()]), vec!["a".to_string()]);
        let own_reads = Arc::new(AtomicUsize::new(0));
        let waiter = {
            let (cache, own_reads) = (cache.clone(), own_reads.clone());
            std::thread::spawn(move || {
                cache.load_blocking("a", |_| {
                    own_reads.fetch_add(1, Ordering::SeqCst);
                    Some("second read".into())
                })
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!waiter.is_finished(), "must wait for the read in flight");
        cache.land("a", Some("warm".into()), true);
        assert_eq!(waiter.join().unwrap().as_deref(), Some("warm"));
        assert_eq!(own_reads.load(Ordering::SeqCst), 0);
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn a_given_up_read_lets_a_waiter_read_itself() {
        use std::sync::Arc;
        let cache = Arc::new(KeychainReads::default());
        cache.claim(["a".to_string()]);
        let waiter = {
            let cache = cache.clone();
            std::thread::spawn(move || cache.load_blocking("a", |_| Some("own".into())))
        };
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.land("a", None, false);
        assert_eq!(waiter.join().unwrap().as_deref(), Some("own"));
    }

    #[test]
    fn warmup_covers_every_slot_and_each_custom_model() {
        let accounts = warmup_accounts(
            r#"{"custom_models":[{"model_id":" m1 "},{"model_id":""},{"name":"no id"}]}"#,
        );
        assert_eq!(accounts.len(), KEY_SLOTS.len() + 1);
        assert!(accounts.contains(&"provider/anthropic".to_string()));
        assert_eq!(accounts.last().map(String::as_str), Some("custom/m1"));
        // First run: no settings file at all.
        assert_eq!(warmup_accounts("").len(), KEY_SLOTS.len());
    }

    #[test]
    fn resolve_prefers_file_then_keychain_then_none() {
        let save_log = std::sync::Mutex::new(Vec::new());
        let save = |a: &str, k: &str| {
            save_log
                .lock()
                .unwrap()
                .push((a.to_string(), k.to_string()));
            true
        };
        // File wins and is pushed to the keychain.
        let got = resolve_key(
            Some("file-key"),
            "provider/s",
            |_| Some("old".into()),
            &save,
        );
        assert_eq!(got.as_deref(), Some("file-key"));
        assert_eq!(save_log.lock().unwrap().len(), 1);
        // No file: keychain value surfaces, no write.
        let got = resolve_key(None, "provider/s", |_| Some("stored".into()), &save);
        assert_eq!(got.as_deref(), Some("stored"));
        assert_eq!(save_log.lock().unwrap().len(), 1);
        // Neither: none. Blanks treated as absent.
        let none = |_: &str| None;
        assert_eq!(resolve_key(None, "provider/s", none, &save), None);
        assert_eq!(resolve_key(Some("   "), "provider/s", none, &save), None);
    }

    #[test]
    fn resolve_survives_keychain_failure() {
        let fail = |_: &str, _: &str| false;
        let none = |_: &str| None;
        // Session still works from the file value even when persist fails.
        let got = resolve_key(Some("file-key"), "provider/s", none, fail);
        assert_eq!(got.as_deref(), Some("file-key"));
    }

    #[test]
    fn a_burst_of_writes_keeps_only_the_last_per_account() {
        let jobs = vec![
            ("provider/a", "1"),
            ("provider/b", "x"),
            ("provider/a", "12"),
            ("provider/a", "123"),
        ];
        let kept = latest_per_account(jobs, |job| job.0);
        assert_eq!(kept, vec![("provider/b", "x"), ("provider/a", "123")]);
    }

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn queued_writes_show_at_once_and_load_skips_known_values() {
        // The cache side only: no keychain call happens for a value this
        // process already holds.
        let account = "test-only/t6-cache-skip";
        remember_keychain_value(account, Some("known".into()));
        assert!(keychain_save_if_changed(account, " known "));
        assert_eq!(keychain_load(account).as_deref(), Some("known"));
    }
}
