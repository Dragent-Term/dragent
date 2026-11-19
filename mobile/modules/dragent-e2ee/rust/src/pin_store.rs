//! The phone pin store (§6.1): desktop pins, tombstones and catalog
//! high-water marks, sealed in the app's documents directory with the
//! `dragent_e2ee` `state.v1` format.
//!
//! The 32-byte state key and the generation live in the keychain / Keystore
//! (identity class, §6.7); the native module reads them and passes them to
//! [`open_pin_store`]. After [`PinStore::commit`] the native module writes the
//! returned generation back to the keychain. Any failed check is
//! [`E2eeError::StoreLost`]: fail closed and pair again, never "empty and
//! continue".

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use dragent_e2ee::channel::PinLookup;
use dragent_e2ee::seal::at_rest::SymmetricKey;
use dragent_e2ee::seal::pin_store::{
    self as ps, LostReason, PeerPin, PendingRotation, StoreError, StoredKey,
};
use zeroize::{Zeroize, Zeroizing};

use crate::catalog::{self, CatalogResult, OpenArgs};
use crate::{lock, parse_id, parse_key, E2eeError, Keypair};

impl From<StoreError> for E2eeError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Lost(r) => E2eeError::StoreLost {
                reason: match r {
                    LostReason::Malformed => "malformed".into(),
                    LostReason::Authentication => "authentication".into(),
                    LostReason::Generation { .. } => "generation".into(),
                    LostReason::Missing => "missing".into(),
                },
            },
            // The kind only: io::Error's text can carry the file path.
            StoreError::Io(e) => E2eeError::Io {
                detail: format!("{:?}", e.kind()),
            },
            StoreError::Random => E2eeError::Random,
            StoreError::VersionExhausted => E2eeError::Internal {
                detail: "catalog version exhausted".into(),
            },
        }
    }
}

/// A freshly generated 32-byte state key, for the keychain.
#[uniffi::export]
pub fn generate_state_key() -> Result<Vec<u8>, E2eeError> {
    Ok(SymmetricKey::generate()?.bytes().to_vec())
}

/// The opened store and whether the keychain generation must be bumped.
#[derive(uniffi::Record)]
pub struct PinStoreOpened {
    /// The store.
    pub store: Arc<PinStore>,
    /// `Some(g)`: the last write's keychain bump didn't happen; set the
    /// keychain generation to `g` **before any further commit**.
    pub bump_keychain_to: Option<u64>,
}

/// Open (or start, when generation 0 and no file) the store at `path`.
/// `state_key` is wiped on the Rust side.
#[uniffi::export]
pub fn open_pin_store(
    path: String,
    mut state_key: Vec<u8>,
    keychain_generation: u64,
) -> Result<PinStoreOpened, E2eeError> {
    let key = <[u8; 32]>::try_from(&state_key[..]).map(Zeroizing::new);
    state_key.zeroize();
    let key = SymmetricKey::from_bytes(
        key.map_err(|_| E2eeError::invalid("state key must be 32 bytes"))?,
    );
    let path = PathBuf::from(path);
    let file = ps::read_file(&path)?;
    let opened = ps::open(&key, keychain_generation, file.as_deref())?;
    Ok(PinStoreOpened {
        store: Arc::new(PinStore {
            inner: Mutex::new(Inner {
                path,
                key,
                generation: opened.generation,
                store: opened.store,
            }),
        }),
        bump_keychain_to: opened.bump_keychain_to,
    })
}

struct Inner {
    path: PathBuf,
    key: SymmetricKey,
    generation: u64,
    store: ps::PinStore,
}

/// The phone's pin store. Changes are in memory until [`PinStore::commit`].
#[derive(uniffi::Object)]
pub struct PinStore {
    inner: Mutex<Inner>,
}

/// A paired desktop.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PinnedDesktop {
    /// Device id.
    pub id: String,
    /// Its confirmed identity key.
    pub identity_key: Vec<u8>,
    /// Its name from `pair-done`.
    pub name: String,
    /// When it was pinned (ms since the Unix epoch).
    pub pinned_at_ms: u64,
    /// An announced, unconfirmed identity rotation (§6.6).
    pub rotation_key: Option<Vec<u8>>,
}

/// What the store knows about a device.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PinState {
    /// Pinned with this identity key.
    Pinned {
        /// The key.
        identity_key: Vec<u8>,
    },
    /// Never paired.
    Unpinned,
    /// Unpaired / revoked (§6.4).
    Tombstoned,
}

#[uniffi::export]
impl PinStore {
    /// The store's current (file) generation.
    pub fn generation(&self) -> u64 {
        lock(&self.inner).generation
    }

    /// Every paired desktop.
    pub fn desktops(&self) -> Vec<PinnedDesktop> {
        lock(&self.inner)
            .store
            .peers
            .iter()
            .map(|(id, p)| PinnedDesktop {
                id: id.to_string(),
                identity_key: p.identity.0.as_bytes().to_vec(),
                name: p.name.clone(),
                pinned_at_ms: p.pinned_at_ms,
                rotation_key: p.rotation.as_ref().map(|r| r.key.0.as_bytes().to_vec()),
            })
            .collect()
    }

    /// Pin state of one device.
    pub fn lookup(&self, device: String) -> Result<PinState, E2eeError> {
        let id = parse_id("device", &device)?;
        Ok(match lock(&self.inner).store.lookup(&id) {
            PinLookup::Pinned(k) => PinState::Pinned {
                identity_key: k.as_bytes().to_vec(),
            },
            PinLookup::Unpinned => PinState::Unpinned,
            PinLookup::Tombstoned => PinState::Tombstoned,
        })
    }

    /// Keys to try for the next KK handshake with this desktop, in order
    /// (§6.6: an announced new key first, then the confirmed one within the
    /// 7-day grace).
    pub fn candidate_keys(&self, device: String, now_ms: u64) -> Result<Vec<Vec<u8>>, E2eeError> {
        let id = parse_id("device", &device)?;
        Ok(lock(&self.inner)
            .store
            .peer(&id)
            .map(|p| {
                p.candidate_keys(now_ms)
                    .iter()
                    .map(|k| k.as_bytes().to_vec())
                    .collect()
            })
            .unwrap_or_default())
    }

    /// §6.2 step 8: commit a pairing on `pair-done`. Pins the desktop key
    /// **from the QR** (the one the IKpsk1 handshake authenticated) and sets
    /// the catalog floor to `catalog_version − 1` (L6).
    pub fn pin_desktop(
        &self,
        device: String,
        identity_key: Vec<u8>,
        name: String,
        catalog_version: u64,
        now_ms: u64,
    ) -> Result<(), E2eeError> {
        let id = parse_id("device", &device)?;
        let key = parse_key("identity key", &identity_key)?;
        let mut g = lock(&self.inner);
        g.store.pin(
            id,
            PeerPin {
                identity: StoredKey(key),
                push: None,
                name,
                pinned_at_ms: now_ms,
                rotation: None,
            },
        );
        g.store.catalog_set_floor(id, catalog_version);
        Ok(())
    }

    /// §6.6: the desktop announced `identity-rotate {new_pub}` inside a
    /// confirmed channel. The old key stays acceptable for 7 days.
    pub fn announce_rotation(
        &self,
        device: String,
        new_key: Vec<u8>,
        now_ms: u64,
    ) -> Result<(), E2eeError> {
        let id = parse_id("device", &device)?;
        let key = parse_key("new key", &new_key)?;
        let mut g = lock(&self.inner);
        let pin = g
            .store
            .peers
            .get_mut(&id)
            .ok_or_else(|| E2eeError::invalid("not paired"))?;
        pin.rotation = Some(PendingRotation {
            key: StoredKey(key),
            announced_at_ms: now_ms,
        });
        Ok(())
    }

    /// §6.6 step 2: a handshake with the announced key confirmed it: re-pin.
    pub fn confirm_rotation(
        &self,
        device: String,
        confirmed_key: Vec<u8>,
    ) -> Result<bool, E2eeError> {
        let id = parse_id("device", &device)?;
        let key = parse_key("confirmed key", &confirmed_key)?;
        let mut g = lock(&self.inner);
        let pin = g
            .store
            .peers
            .get_mut(&id)
            .ok_or_else(|| E2eeError::invalid("not paired"))?;
        match &pin.rotation {
            Some(r) if r.key.0.ct_eq(&key) => {
                pin.identity = r.key;
                pin.rotation = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// §6.4 step 1: remove the pin, tombstone its key(s) and drop the catalog
    /// mark. Returns whether it was pinned. Commit before anything else.
    pub fn revoke(&self, device: String, now_ms: u64) -> Result<bool, E2eeError> {
        let id = parse_id("device", &device)?;
        Ok(lock(&self.inner).store.revoke(&id, now_ms))
    }

    /// §6.5: verify and open a sealed catalog blob from `desktop` against this
    /// store's pin and high-water mark. On `Accepted` the mark advances in
    /// memory; [`PinStore::commit`] persists it.
    pub fn open_catalog(
        &self,
        identity: &Keypair,
        account: String,
        phone: String,
        desktop: String,
        blob: Vec<u8>,
    ) -> Result<CatalogResult, E2eeError> {
        let account = parse_id("account", &account)?;
        let phone = parse_id("phone", &phone)?;
        let desktop = parse_id("desktop", &desktop)?;
        let mut g = lock(&self.inner);
        let pin = match g.store.lookup(&desktop) {
            PinLookup::Pinned(k) => k,
            _ => return Err(E2eeError::WrongSender),
        };
        catalog::open(
            &mut g.store,
            OpenArgs {
                identity: &identity.inner,
                desktop_pin: &pin,
                account,
                phone,
                desktop,
                blob: &blob,
            },
        )
    }

    /// The catalog high-water mark for a desktop.
    pub fn catalog_high_water(&self, desktop: String) -> Result<Option<u64>, E2eeError> {
        let id = parse_id("desktop", &desktop)?;
        Ok(lock(&self.inner)
            .store
            .catalog_marks
            .get(&id)
            .map(|m| m.high_water))
    }

    /// Persist as the next generation (temp file, fsync, rename, fsync dir).
    /// Returns the new generation: write it to the keychain now.
    pub fn commit(&self) -> Result<u64, E2eeError> {
        let mut g = lock(&self.inner);
        let next = ps::commit(&g.path, &g.key, g.generation, &g.store)?;
        g.generation = next;
        Ok(next)
    }
}
