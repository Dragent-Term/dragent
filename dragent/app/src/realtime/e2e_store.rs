//! The desktop's pin store and opened E2EE state (docs/design/e2ee.md §6.1,
//! §6.4, §6.6).
//!
//! The store file is `DRAGENT_HOME/e2ee/state.v1`, in the sealed format of
//! [`dragent_e2ee::seal::pin_store`]: XChaCha20-Poly1305 under the state key,
//! with the generation number authenticated and mirrored in the keychain
//! ([`super::e2e_keys`]). Writes go to a temp file, then fsync, rename, and
//! then the keychain generation is bumped.
//!
//! **Fail closed.** If the store fails authentication or its generation check,
//! or the keychain item holding the keys is lost, every pin is dropped and the
//! person is told to pair again ([`ResetNotice`]); the store is never treated
//! as "empty and continue" silently. If the keychain can't be used at all,
//! [`E2eDesktop::open`] fails and phone access stays off.
//!
//! [`E2eDesktop`] is the shared, process-wide state that the channel endpoint
//! (`e2e_channel`), the sealed catalog (E3c) and push sealing (E9a) all read:
//! the identity key and the paired phones.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use dragent_e2ee::channel::PinLookup;
use dragent_e2ee::noise::{PublicKey, StaticKeypair};
use dragent_e2ee::seal::at_rest::SymmetricKey;
use dragent_e2ee::seal::pin_store::{
    self, LostReason, PeerPin, PendingRotation, PinStore, StoreError, StoredKey,
};
use dragent_e2ee::Id;

use super::e2e_keys::{self, KeyError, SecretStore, Stored};

/// The store file, relative to `DRAGENT_HOME`.
pub const STATE_FILE: &str = "e2ee/state.v1";

/// The largest catalog version (§6.5: ≤ 2^53 − 1).
const MAX_CATALOG_VERSION: u64 = (1 << 53) - 1;

/// Why paired phones were reset when the state was opened (§9). Show the
/// message once; every phone must pair again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetNotice {
    /// The keychain item holding the identity key was missing or unreadable
    /// while a pin store existed. New keys were generated.
    KeychainItemLost,
    /// The pin store failed authentication or its generation check.
    RecordsUnverified(LostReason),
    /// The person chose "Reset security keys" (§6.6).
    KeysReset,
}

impl ResetNotice {
    /// The §9 text.
    pub fn message(&self) -> &'static str {
        match self {
            ResetNotice::KeychainItemLost => {
                "Phone access was reset because this computer's security key was lost. \
                 Pair your phones again."
            }
            ResetNotice::RecordsUnverified(_) => {
                "Dragent's security records on this device couldn't be verified, so paired \
                 devices were reset. Pair again."
            }
            ResetNotice::KeysReset => {
                "Pair each phone again now. Until you do, someone holding the old key could \
                 pretend to be this computer to them."
            }
        }
    }
}

/// Opening the E2EE state failed: phone access stays off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// No usable keychain (§9 "No secure key store").
    Keys(KeyError),
    /// The store file couldn't be read or written (I/O). Not a reset: the
    /// file is left as it is and opening is retried on the next launch.
    Io(String),
    /// `AppSettings::device_id` is not a canonical UUID.
    BadDeviceId,
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Keys(e) => write!(f, "{e}"),
            OpenError::Io(e) => write!(f, "couldn't read Dragent's phone security records: {e}"),
            OpenError::BadDeviceId => f.write_str("this install's device id is not valid"),
        }
    }
}

impl std::error::Error for OpenError {}

/// A pin-store write failed. The in-memory state is unchanged unless the
/// method says otherwise (revocation stays in force in memory: fail closed).
#[derive(Debug)]
pub enum StoreFailure {
    /// Sealing or writing the file failed.
    Store(StoreError),
    /// The keychain generation couldn't be advanced, so no further write is
    /// safe until it can be.
    Keychain(String),
}

impl fmt::Display for StoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreFailure::Store(e) => write!(f, "{e}"),
            StoreFailure::Keychain(e) => write!(f, "keychain: {e}"),
        }
    }
}

impl std::error::Error for StoreFailure {}

impl From<StoreError> for StoreFailure {
    fn from(e: StoreError) -> Self {
        StoreFailure::Store(e)
    }
}

/// A paired phone as the desktop knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedPhone {
    /// The phone's device id.
    pub id: Id,
    /// Its confirmed identity key.
    pub identity: PublicKey,
    /// Its push key (§8). Push sealing uses this.
    pub push: Option<PublicKey>,
    /// Its name, from the encrypted pairing payload.
    pub name: String,
    /// When it was pinned (wall-clock ms).
    pub pinned_at_ms: u64,
}

impl PairedPhone {
    fn from_pin(id: Id, pin: &PeerPin) -> Self {
        PairedPhone {
            id,
            identity: pin.identity.0,
            push: pin.push.map(|k| k.0),
            name: pin.name.clone(),
            pinned_at_ms: pin.pinned_at_ms,
        }
    }
}

/// The sealed pin store with its keychain generation.
pub struct E2eStore {
    path: PathBuf,
    device: Id,
    secrets: Arc<dyn SecretStore>,
    state_key: SymmetricKey,
    store: PinStore,
    /// The generation of the file on disk.
    file_generation: u64,
    /// The generation recorded in the keychain (may lag the file by one after
    /// a failed bump; the next write retries the bump first).
    keychain_generation: u64,
}

impl fmt::Debug for E2eStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eStore")
            .field("peers", &self.store.peers.len())
            .field("tombstones", &self.store.tombstones.len())
            .field("file_generation", &self.file_generation)
            .finish_non_exhaustive()
    }
}

impl E2eStore {
    /// §4.2 `listen`: what is known about this phone.
    pub fn lookup(&self, phone: &Id) -> PinLookup {
        self.store.lookup(phone)
    }

    /// Keys to try for this phone's KK handshake, in order (an announced
    /// rotation first, §6.6). Empty when the phone is not pinned. Keys that
    /// belong to a removed peer are never returned.
    pub fn candidate_keys(&self, phone: &Id, now_wall_ms: u64) -> Vec<PublicKey> {
        self.store
            .peer(phone)
            .map(|p| p.candidate_keys(now_wall_ms))
            .unwrap_or_default()
            .into_iter()
            .filter(|k| !self.store.is_tombstoned_key(k))
            .collect()
    }

    /// A paired phone.
    pub fn phone(&self, id: &Id) -> Option<PairedPhone> {
        self.store.peer(id).map(|p| PairedPhone::from_pin(*id, p))
    }

    /// Every paired phone (catalog recipients, push recipients, Settings).
    pub fn paired_phones(&self) -> Vec<PairedPhone> {
        self.store
            .peers
            .iter()
            .map(|(id, p)| PairedPhone::from_pin(*id, p))
            .collect()
    }

    /// Whether this phone was removed (§6.4).
    pub fn is_tombstoned(&self, phone: &Id) -> bool {
        self.store.tombstones.iter().any(|t| t.device == *phone)
    }

    /// The last catalog version used (0: none yet).
    pub fn catalog_version(&self) -> u64 {
        self.store.catalog_version
    }

    /// Reserve the next catalog version (§6.5 step 1). It is persisted before
    /// it is returned, so it is never reused.
    pub fn next_catalog_version(&mut self) -> Result<u64, StoreFailure> {
        let mut next = self.store.clone();
        let v = next.next_catalog_version()?;
        self.commit(next)?;
        Ok(v)
    }

    /// §6.2 step 8: pin a phone after a confirmed pairing and reserve the
    /// catalog version `pair-done` announces, in one write.
    pub fn pin_phone(
        &mut self,
        phone: Id,
        identity: PublicKey,
        push: PublicKey,
        name: String,
        now_wall_ms: u64,
    ) -> Result<u64, StoreFailure> {
        let mut next = self.store.clone();
        next.pin(
            phone,
            PeerPin {
                identity: StoredKey(identity),
                push: Some(StoredKey(push)),
                name,
                pinned_at_ms: now_wall_ms,
                rotation: None,
            },
        );
        let v = next.next_catalog_version()?;
        self.commit(next)?;
        Ok(v)
    }

    /// §6.4 step 1: remove the pin and write a tombstone (fsync). The removal
    /// takes effect in memory even if the write fails, so the phone is
    /// refused from now on either way; the error tells the caller the
    /// tombstone isn't durable yet. Returns whether the phone was pinned.
    pub fn revoke(&mut self, phone: &Id, now_wall_ms: u64) -> Result<bool, StoreFailure> {
        let mut next = self.store.clone();
        if !next.revoke(phone, now_wall_ms) {
            return Ok(false);
        }
        let result = self.commit(next.clone());
        if result.is_err() {
            self.store = next;
        }
        result.map(|()| true)
    }

    /// §6.6: a phone announced `identity-rotate {new_pub, new_push_pub?}`
    /// inside a confirmed channel. The new identity key is accepted for KK
    /// from now on (and the old one for at most 7 days); the push key is
    /// re-pinned at once.
    pub fn record_rotation(
        &mut self,
        phone: &Id,
        new_identity: PublicKey,
        new_push: Option<PublicKey>,
        now_wall_ms: u64,
    ) -> Result<(), StoreFailure> {
        let mut next = self.store.clone();
        let Some(pin) = next.peers.get_mut(phone) else {
            return Ok(());
        };
        if pin.identity.0.ct_eq(&new_identity) {
            pin.rotation = None;
        } else {
            pin.rotation = Some(PendingRotation {
                key: StoredKey(new_identity),
                announced_at_ms: now_wall_ms,
            });
        }
        if let Some(push) = new_push {
            pin.push = Some(StoredKey(push));
        }
        self.commit(next)
    }

    /// §6.6 step 2: a handshake with `key` was confirmed. If it is the
    /// phone's announced new key, re-pin it and drop the old one.
    pub fn confirm_key(&mut self, phone: &Id, key: &PublicKey) -> Result<(), StoreFailure> {
        let promote = self
            .store
            .peer(phone)
            .and_then(|p| p.rotation.as_ref())
            .is_some_and(|r| r.key.0.ct_eq(key));
        if !promote {
            return Ok(());
        }
        let mut next = self.store.clone();
        if let Some(pin) = next.peers.get_mut(phone) {
            pin.identity = StoredKey(*key);
            pin.rotation = None;
        }
        self.commit(next)
    }

    /// Persist `next` as the new state: seal, temp file, fsync, rename, then
    /// bump the keychain generation. On error the in-memory state is unchanged.
    fn commit(&mut self, next: PinStore) -> Result<(), StoreFailure> {
        if self.keychain_generation != self.file_generation {
            // A previous bump failed: the file is one ahead. Writing another
            // generation now would leave it two ahead, which reads as a
            // rollback on the next launch, so the bump must succeed first.
            e2e_keys::write_generation(&*self.secrets, &self.device, self.file_generation)
                .map_err(|e| StoreFailure::Keychain(e.to_string()))?;
            self.keychain_generation = self.file_generation;
        }
        let g = pin_store::commit(&self.path, &self.state_key, self.keychain_generation, &next)?;
        self.file_generation = g;
        self.store = next;
        match e2e_keys::write_generation(&*self.secrets, &self.device, g) {
            Ok(()) => self.keychain_generation = g,
            // The file is durable and one ahead of the keychain, which the
            // next open accepts (and finishes the bump). Retry before the next
            // write.
            Err(_) => log::warn!("[e2ee] keychain generation bump deferred"),
        }
        Ok(())
    }
}

/// The opened E2EE state of this desktop: identity key and pin store. Shared
/// by the channel endpoint, the sealed catalog and push sealing.
pub struct E2eDesktop {
    device: Id,
    identity: RwLock<Arc<StaticKeypair>>,
    store: Mutex<E2eStore>,
    notice: Mutex<Option<ResetNotice>>,
}

impl fmt::Debug for E2eDesktop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("E2eDesktop")
            .field("device", &self.device)
            .field("identity", &self.public())
            .finish_non_exhaustive()
    }
}

impl E2eDesktop {
    /// Open (or create) this desktop's keys and pin store under `home`
    /// (`DRAGENT_HOME`). `now_wall_ms` is wall-clock ms since the Unix epoch.
    ///
    /// - Keychain unusable → `Err(OpenError::Keys)`: phone access off.
    /// - Keys item lost while a store exists → new keys, empty store,
    ///   [`ResetNotice::KeychainItemLost`].
    /// - Store fails its check → empty store under the same keys,
    ///   [`ResetNotice::RecordsUnverified`].
    pub fn open(
        home: &Path,
        device: Id,
        secrets: Arc<dyn SecretStore>,
        now_wall_ms: u64,
    ) -> Result<E2eDesktop, OpenError> {
        let path = home.join(STATE_FILE);
        // An oversized file (`Lost`) is a failed check, not an I/O problem.
        let (file, oversized) = match pin_store::read_file(&path) {
            Ok(f) => (f, false),
            Err(StoreError::Lost(_)) => (None, true),
            Err(e) => return Err(OpenError::Io(e.to_string())),
        };

        let (keys, mut notice) =
            match e2e_keys::load(&*secrets, &device).map_err(OpenError::Keys)? {
                Stored::Keys(k) => (k, None),
                Stored::Missing | Stored::Malformed => {
                    let had_state = file.is_some() || oversized;
                    let keys = e2e_keys::create(&*secrets, &device).map_err(OpenError::Keys)?;
                    e2e_keys::write_generation(&*secrets, &device, 0)
                        .map_err(|e| OpenError::Keys(KeyError::NoSecureStore(e)))?;
                    let notice = had_state.then_some(ResetNotice::KeychainItemLost);
                    (keys, notice)
                }
            };
        let (identity, state_key) = keys.into_parts();
        let keychain_generation =
            e2e_keys::read_generation(&*secrets, &device).map_err(OpenError::Keys)?;

        let fresh = || PinStore {
            // A new store's catalog versions start at the wall clock so they
            // stay above any version a lost store may have used (§6.5:
            // versions are never reused).
            catalog_version: now_wall_ms.min(MAX_CATALOG_VERSION - 1),
            ..PinStore::default()
        };

        let mut store = E2eStore {
            path,
            device,
            secrets: secrets.clone(),
            state_key,
            store: PinStore::default(),
            file_generation: keychain_generation,
            keychain_generation,
        };

        let opened = if notice.is_some() {
            None
        } else if oversized {
            notice = Some(ResetNotice::RecordsUnverified(LostReason::Malformed));
            None
        } else {
            match pin_store::open(&store.state_key, keychain_generation, file.as_deref()) {
                Ok(o) => Some(o),
                Err(StoreError::Lost(reason)) => {
                    notice = Some(ResetNotice::RecordsUnverified(reason));
                    None
                }
                Err(e) => return Err(OpenError::Io(e.to_string())),
            }
        };

        match opened {
            Some(o) => {
                store.store = o.store;
                store.file_generation = o.generation;
                if let Some(g) = o.bump_keychain_to {
                    // Finish an interrupted write; if the keychain still
                    // refuses, the next write retries before writing.
                    if e2e_keys::write_generation(&*secrets, &device, g).is_ok() {
                        store.keychain_generation = g;
                    }
                }
                if file.is_none() {
                    // First run: write the initial store so a later missing
                    // file reads as lost, not as a fresh install.
                    store
                        .commit(fresh())
                        .map_err(|e| OpenError::Io(e.to_string()))?;
                }
            }
            None => {
                if notice.is_some() {
                    log::warn!("[e2ee] pin store reset; every phone must pair again");
                }
                store
                    .commit(fresh())
                    .map_err(|e| OpenError::Io(e.to_string()))?;
            }
        }

        Ok(E2eDesktop {
            device,
            identity: RwLock::new(Arc::new(identity)),
            store: Mutex::new(store),
            notice: Mutex::new(notice),
        })
    }

    /// This desktop's device id.
    pub fn device(&self) -> Id {
        self.device
    }

    /// The current identity key pair (for handshakes and Noise X seals).
    pub fn identity(&self) -> Arc<StaticKeypair> {
        self.identity
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The identity public key (the pairing QR's `k`).
    pub fn public(&self) -> PublicKey {
        self.identity().public()
    }

    /// The pin store. Hold the lock briefly; never across I/O to the network.
    pub fn store(&self) -> MutexGuard<'_, E2eStore> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Why phones were reset when this state was opened, if they were. Taking
    /// it clears it, so the message is shown once.
    pub fn take_notice(&self) -> Option<ResetNotice> {
        self.notice.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    /// The pending notice without clearing it.
    pub fn notice(&self) -> Option<ResetNotice> {
        *self.notice.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// §6.6 compromise recovery ("Reset security keys"): new identity and
    /// state keys now, every phone pin deleted and tombstoned. The caller (the
    /// channel endpoint, [`super::e2e_channel::E2eEndpoint::reset_keys`])
    /// closes and zeroises every channel.
    ///
    /// Returns the phones that were removed.
    pub(crate) fn reset_keys(&self, now_wall_ms: u64) -> Result<Vec<Id>, StoreFailure> {
        let mut store = self.store();
        let new_keys = e2e_keys::create(&*store.secrets, &self.device)
            .map_err(|e| StoreFailure::Keychain(e.to_string()))?;
        let (identity, state_key) = new_keys.into_parts();
        // From here the old keys are gone from the keychain: switch in memory
        // even if the store write below fails (a store sealed under the old
        // key then reads as lost on the next launch, which also resets).
        *self.identity.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(identity);
        store.state_key = state_key;
        let mut next = store.store.clone();
        let removed: Vec<Id> = next.peers.keys().copied().collect();
        for phone in &removed {
            next.revoke(phone, now_wall_ms);
        }
        // Sealed under a new key: the generation sequence continues.
        let result = store.commit(next.clone());
        if result.is_err() {
            store.store = next;
        }
        *self.notice.lock().unwrap_or_else(|p| p.into_inner()) = Some(ResetNotice::KeysReset);
        result.map(|()| removed)
    }
}

/// Wall-clock milliseconds since the Unix epoch.
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// This process's E2EE desktop state, opened once on first use from the OS
/// keychain, `DRAGENT_HOME` and `AppSettings::device_id`. The result,
/// including a failure, is cached for the process: the keychain is read once
/// per process (§6.1), and a machine without a secure store stays off until
/// restart.
#[cfg(not(target_family = "wasm"))]
pub fn desktop() -> Result<Arc<E2eDesktop>, OpenError> {
    static DESKTOP: std::sync::OnceLock<Result<Arc<E2eDesktop>, OpenError>> =
        std::sync::OnceLock::new();
    DESKTOP
        .get_or_init(|| {
            let device = Id::parse(&crate::settings::AppSettings::device_id())
                .map_err(|_| OpenError::BadDeviceId)?;
            E2eDesktop::open(
                &dragent_paths::home(),
                device,
                Arc::new(e2e_keys::OsKeychain),
                wall_ms(),
            )
            .map(Arc::new)
        })
        .clone()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::realtime::e2e_keys::MemorySecretStore;

    pub(crate) struct TempHome(pub PathBuf);

    impl TempHome {
        pub(crate) fn new(tag: &str) -> Self {
            let mut b = [0u8; 8];
            dragent_e2ee::fill_random(&mut b).unwrap();
            let suffix: String = b.iter().map(|x| format!("{x:02x}")).collect();
            TempHome(std::env::temp_dir().join(format!("dragent_e3a_{tag}_{suffix}")))
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn device() -> Id {
        Id([0xd5; 16])
    }

    fn open(home: &TempHome, secrets: &Arc<MemorySecretStore>) -> Result<E2eDesktop, OpenError> {
        E2eDesktop::open(&home.0, device(), secrets.clone(), 1_000)
    }

    fn key(b: u8) -> PublicKey {
        PublicKey([b; 32])
    }

    #[test]
    fn first_open_creates_keys_and_store() {
        let home = TempHome::new("first");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.notice(), None);
        assert!(home.0.join(STATE_FILE).exists());
        assert_eq!(
            e2e_keys::read_generation(&*secrets, &device()).unwrap(),
            1,
            "initial store written and generation bumped"
        );
        let public = d.public();
        drop(d);
        let again = open(&home, &secrets).unwrap();
        assert_eq!(again.public(), public, "same identity on the next launch");
        assert_eq!(again.notice(), None);
    }

    #[test]
    fn pins_persist_and_revocation_tombstones() {
        let home = TempHome::new("pins");
        let secrets = Arc::new(MemorySecretStore::new());
        let phone = Id([2; 16]);
        {
            let d = open(&home, &secrets).unwrap();
            let v1 = d
                .store()
                .pin_phone(phone, key(7), key(8), "Pixel".into(), 5)
                .unwrap();
            let v2 = d.store().next_catalog_version().unwrap();
            assert_eq!(v2, v1 + 1);
        }
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.store().lookup(&phone), PinLookup::Pinned(key(7)));
        let p = d.store().phone(&phone).unwrap();
        assert_eq!((p.push, p.name.as_str()), (Some(key(8)), "Pixel"));
        assert!(d.store().revoke(&phone, 9).unwrap());
        drop(d);
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.store().lookup(&phone), PinLookup::Tombstoned);
        assert!(d.store().candidate_keys(&phone, 10).is_empty());
        assert!(d.store().paired_phones().is_empty());
    }

    #[test]
    fn catalog_versions_never_repeat_across_launches() {
        let home = TempHome::new("versions");
        let secrets = Arc::new(MemorySecretStore::new());
        let a = open(&home, &secrets)
            .unwrap()
            .store()
            .next_catalog_version()
            .unwrap();
        let b = open(&home, &secrets)
            .unwrap()
            .store()
            .next_catalog_version()
            .unwrap();
        assert!(b > a);
    }

    #[test]
    fn rolled_back_store_resets_pins_with_notice() {
        let home = TempHome::new("rollback");
        let secrets = Arc::new(MemorySecretStore::new());
        let phone = Id([2; 16]);
        let d = open(&home, &secrets).unwrap();
        d.store()
            .pin_phone(phone, key(7), key(8), "Pixel".into(), 5)
            .unwrap();
        let snapshot = std::fs::read(home.0.join(STATE_FILE)).unwrap();
        d.store().revoke(&phone, 6).unwrap();
        let v_before = d.store().catalog_version();
        drop(d);
        // A malicious or buggy writer restores the older file (phone pinned).
        std::fs::write(home.0.join(STATE_FILE), &snapshot).unwrap();
        let d = open(&home, &secrets).unwrap();
        assert!(matches!(
            d.notice(),
            Some(ResetNotice::RecordsUnverified(
                LostReason::Generation { .. }
            ))
        ));
        assert_eq!(
            d.store().lookup(&phone),
            PinLookup::Unpinned,
            "never re-admitted"
        );
        assert!(d.store().catalog_version() >= v_before.min(1_000));
        assert!(d.take_notice().is_some());
        assert!(d.take_notice().is_none(), "shown once");
    }

    #[test]
    fn tampered_or_missing_store_resets() {
        let home = TempHome::new("tamper");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        d.store()
            .pin_phone(Id([2; 16]), key(7), key(8), "P".into(), 5)
            .unwrap();
        drop(d);
        let path = home.0.join(STATE_FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        let d = open(&home, &secrets).unwrap();
        assert_eq!(
            d.notice(),
            Some(ResetNotice::RecordsUnverified(LostReason::Authentication))
        );
        assert!(d.store().paired_phones().is_empty());
        drop(d);
        std::fs::remove_file(&path).unwrap();
        let d = open(&home, &secrets).unwrap();
        assert_eq!(
            d.notice(),
            Some(ResetNotice::RecordsUnverified(LostReason::Missing))
        );
    }

    #[test]
    fn lost_keychain_item_means_new_keys_and_reset() {
        let home = TempHome::new("lostkey");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        let old = d.public();
        d.store()
            .pin_phone(Id([2; 16]), key(7), key(8), "P".into(), 5)
            .unwrap();
        drop(d);
        secrets.forget(&e2e_keys::keys_account(&device()));
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.notice(), Some(ResetNotice::KeychainItemLost));
        assert_ne!(d.public(), old);
        assert!(d.store().paired_phones().is_empty());
        // And it opens cleanly afterwards.
        drop(d);
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.notice(), None);
    }

    #[test]
    fn no_keychain_fails_closed_and_leaves_the_file() {
        let home = TempHome::new("nokeychain");
        let secrets = Arc::new(MemorySecretStore::new());
        drop(open(&home, &secrets).unwrap());
        let before = std::fs::read(home.0.join(STATE_FILE)).unwrap();
        secrets.set_unavailable(true);
        assert!(matches!(open(&home, &secrets), Err(OpenError::Keys(_))));
        assert_eq!(std::fs::read(home.0.join(STATE_FILE)).unwrap(), before);
        secrets.set_unavailable(false);
        assert_eq!(open(&home, &secrets).unwrap().notice(), None);
    }

    #[test]
    fn deferred_generation_bump_is_recovered_never_skipped() {
        let home = TempHome::new("bump");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        secrets.set_read_only(true);
        // The file write succeeds; the keychain bump is deferred.
        d.store()
            .pin_phone(Id([2; 16]), key(7), key(8), "P".into(), 5)
            .unwrap();
        // A second write must not run ahead of the keychain.
        assert!(matches!(
            d.store().next_catalog_version(),
            Err(StoreFailure::Keychain(_))
        ));
        drop(d);
        // A relaunch accepts the file one ahead and finishes the bump.
        secrets.set_read_only(false);
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.notice(), None);
        assert!(d.store().phone(&Id([2; 16])).is_some());
        d.store().next_catalog_version().unwrap();
        drop(d);
        assert_eq!(open(&home, &secrets).unwrap().notice(), None);
    }

    #[test]
    fn revoke_holds_in_memory_even_if_the_write_fails() {
        let home = TempHome::new("revokefail");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        let phone = Id([2; 16]);
        d.store()
            .pin_phone(phone, key(7), key(8), "P".into(), 5)
            .unwrap();
        // Make the file unwritable: replace the e2ee dir with a file.
        let dir = home.0.join("e2ee");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, b"x").unwrap();
        assert!(d.store().revoke(&phone, 6).is_err());
        assert_eq!(d.store().lookup(&phone), PinLookup::Tombstoned);
    }

    #[test]
    fn rotation_is_announced_then_confirmed() {
        let home = TempHome::new("rotate");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        let phone = Id([2; 16]);
        d.store()
            .pin_phone(phone, key(7), key(8), "P".into(), 5)
            .unwrap();
        d.store()
            .record_rotation(&phone, key(9), Some(key(10)), 100)
            .unwrap();
        assert_eq!(d.store().candidate_keys(&phone, 200), vec![key(9), key(7)]);
        assert_eq!(d.store().phone(&phone).unwrap().push, Some(key(10)));
        // The old key's grace ends after 7 days.
        let after = 100 + pin_store::ROTATION_GRACE_MS;
        assert_eq!(d.store().candidate_keys(&phone, after), vec![key(9)]);
        d.store().confirm_key(&phone, &key(9)).unwrap();
        assert_eq!(d.store().candidate_keys(&phone, 300), vec![key(9)]);
        assert_eq!(d.store().lookup(&phone), PinLookup::Pinned(key(9)));
    }

    #[test]
    fn reset_keys_replaces_identity_and_removes_every_phone() {
        let home = TempHome::new("reset");
        let secrets = Arc::new(MemorySecretStore::new());
        let d = open(&home, &secrets).unwrap();
        let old = d.public();
        d.store()
            .pin_phone(Id([2; 16]), key(7), key(8), "P".into(), 5)
            .unwrap();
        let removed = d.reset_keys(10).unwrap();
        assert_eq!(removed, vec![Id([2; 16])]);
        assert_ne!(d.public(), old);
        assert_eq!(d.store().lookup(&Id([2; 16])), PinLookup::Tombstoned);
        assert_eq!(d.notice(), Some(ResetNotice::KeysReset));
        drop(d);
        let d = open(&home, &secrets).unwrap();
        assert_eq!(d.notice(), None, "the new store opens under the new key");
        assert_ne!(d.public(), old);
        assert_eq!(d.store().lookup(&Id([2; 16])), PinLookup::Tombstoned);
    }
}
