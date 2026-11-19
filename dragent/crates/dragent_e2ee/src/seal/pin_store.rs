//! The sealed pin store (§6.1): peer pins and tombstones, plus catalog
//! versions and high-water marks. On the desktop it lives at
//! `DRAGENT_HOME/e2ee/state.v1`; on the phone, in the app's documents
//! directory. The same format serves both.
//!
//! # File format (`state.v1`)
//!
//! ```text
//! offset size  field
//! 0      8     magic  = "DRGPINS1"
//! 8      8     generation, u64 big-endian
//! 16     24    nonce (random)
//! 40     ...   XChaCha20-Poly1305(state_key, nonce,
//!                                 AD = magic || generation,
//!                                 plaintext = JSON of [`PinStore`]) incl. 16-byte tag
//! ```
//!
//! The 32-byte state key and the current generation live in the OS keychain
//! or secure store, never in the file.
//!
//! # Writing ([`commit`])
//!
//! New generation `g + 1`: seal, write to a temp file, fsync, rename, fsync
//! the directory; then the caller bumps the keychain generation to `g + 1`.
//!
//! # Reading ([`open`])
//!
//! The file's generation is authenticated (it is in the AD). It must equal the
//! keychain generation `g`, or `g + 1`: a crash between the rename and the
//! keychain bump leaves the file one ahead, and only we can have written that
//! file, so it is accepted and [`Opened::bump_keychain_to`] tells the caller
//! to finish the bump. Anything else (an older file rolled back, a failed
//! tag, a missing file when `g > 0`, a malformed file) is
//! [`StoreError::Lost`]: fail closed and pair again, never "empty and
//! continue".

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::at_rest::{self, SymmetricKey, NONCE_LEN, TAG_LEN};
use super::catalog::MAX_VERSION;
use crate::channel::PinLookup;
use crate::ids::{hex_decode_lower, hex_lower, Id};
use crate::noise::PublicKey;

/// File magic, including the format version.
pub const MAGIC: &[u8; 8] = b"DRGPINS1";
const HEADER_LEN: usize = 8 + 8 + NONCE_LEN;
/// Refuse to read pin-store files larger than this.
pub const MAX_FILE_LEN: u64 = 16 * 1024 * 1024;
/// An announced identity rotation is honoured with the old key for at most
/// this long (§6.6).
pub const ROTATION_GRACE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Why the pin store couldn't be used.
#[derive(Debug)]
pub enum StoreError {
    /// Treat as lost keys: fail closed, reset, and pair again (§6.1, §9).
    Lost(LostReason),
    /// Reading or writing the file failed.
    Io(io::Error),
    /// The OS RNG failed.
    Random,
    /// The catalog version counter is exhausted (2^53 − 1).
    VersionExhausted,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Lost(r) => write!(f, "security records couldn't be verified ({r:?})"),
            StoreError::Io(e) => write!(f, "pin store I/O: {e}"),
            StoreError::Random => f.write_str("random number generator failed"),
            StoreError::VersionExhausted => f.write_str("catalog version exhausted"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// Which check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostReason {
    /// The file is too short, has the wrong magic, or its JSON is invalid.
    Malformed,
    /// The tag didn't verify (wrong key or tampering).
    Authentication,
    /// The authenticated generation is neither `g` nor `g + 1`.
    Generation {
        /// In the file.
        file: u64,
        /// In the keychain.
        keychain: u64,
    },
    /// The keychain says a store was written (`g > 0`) but there is no file.
    Missing,
}

/// A 32-byte public key stored as lowercase hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredKey(pub PublicKey);

impl Serialize for StoredKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex_lower(self.0.as_bytes()))
    }
}

impl<'de> Deserialize<'de> for StoredKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        hex_decode_lower(&s)
            .and_then(|b| PublicKey::from_slice(&b))
            .map(StoredKey)
            .ok_or_else(|| serde::de::Error::custom("expected 64 lowercase hex digits"))
    }
}

/// An identity rotation announced by `identity-rotate` but not yet confirmed
/// by a handshake with the new key (§6.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingRotation {
    /// The announced new identity key.
    pub key: StoredKey,
    /// When it was announced (wall-clock ms since the Unix epoch).
    pub announced_at_ms: u64,
}

/// A paired peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerPin {
    /// The peer's confirmed identity key.
    pub identity: StoredKey,
    /// The phone's push key (desktop side; `None` for a desktop peer).
    pub push: Option<StoredKey>,
    /// The peer's name, from the encrypted pairing payload or `pair-done`.
    pub name: String,
    /// When it was pinned (wall-clock ms since the Unix epoch).
    pub pinned_at_ms: u64,
    /// An announced, unconfirmed identity rotation.
    #[serde(default)]
    pub rotation: Option<PendingRotation>,
}

impl PeerPin {
    /// Keys to try for this peer's next handshake, in order: an announced new
    /// key first, then the confirmed key (dropped once the 7-day grace after
    /// an announcement has passed).
    pub fn candidate_keys(&self, now_ms: u64) -> Vec<PublicKey> {
        match &self.rotation {
            None => vec![self.identity.0],
            Some(r) if now_ms.saturating_sub(r.announced_at_ms) < ROTATION_GRACE_MS => {
                vec![r.key.0, self.identity.0]
            }
            Some(r) => vec![r.key.0],
        }
    }
}

/// A removed peer (§6.4): its key is refused for KK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tombstone {
    /// The removed device.
    pub device: Id,
    /// Its identity key at removal.
    pub identity: StoredKey,
    /// When it was removed (wall-clock ms since the Unix epoch).
    pub removed_at_ms: u64,
}

/// Phone side: the catalog high-water mark for one desktop (§6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogMark {
    /// The highest version accepted (or the `pair-done` floor).
    pub high_water: u64,
    /// SHA-256 of the body accepted at `high_water`, lowercase hex; `None`
    /// for a floor set by `pair-done` (no blob accepted at it).
    pub body_sha256: Option<String>,
}

/// Result of checking a catalog blob's version against the high-water mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogCheck {
    /// Newer: accept it and call [`PinStore::catalog_accept`].
    Accept,
    /// The blob already accepted at the high-water mark: nothing new.
    Unchanged,
    /// Older than the mark: reject (rollback).
    Rollback,
    /// Same version, different body: reject and flag (equivocation).
    Equivocation,
}

/// The pin store's contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinStore {
    /// Paired peers by device id.
    pub peers: BTreeMap<Id, PeerPin>,
    /// Removed peers.
    pub tombstones: Vec<Tombstone>,
    /// Phone side: catalog high-water marks by desktop id.
    pub catalog_marks: BTreeMap<Id, CatalogMark>,
    /// Desktop side: the last catalog version used (0 = none yet). Versions
    /// are never reused.
    pub catalog_version: u64,
}

impl PinStore {
    /// §4.2 `listen`: what is known about this peer.
    pub fn lookup(&self, device: &Id) -> PinLookup {
        if let Some(p) = self.peers.get(device) {
            return PinLookup::Pinned(p.identity.0);
        }
        if self.tombstones.iter().any(|t| t.device == *device) {
            return PinLookup::Tombstoned;
        }
        PinLookup::Unpinned
    }

    /// Whether `key` belongs to a removed peer.
    ///
    /// A key held by a current pin is never tombstoned: revocation deletes
    /// the pin, so a pin with the same key can only come from a later,
    /// committed pairing (QR secret + confirmed code). A phone keeps its
    /// identity key across pairings, and pairing again mints a new device
    /// id, so the old device's tombstone must not lock the key out.
    pub fn is_tombstoned_key(&self, key: &PublicKey) -> bool {
        self.tombstones.iter().any(|t| t.identity.0.ct_eq(key))
            && !self.peers.values().any(|p| {
                p.identity.0.ct_eq(key) || p.rotation.as_ref().is_some_and(|r| r.key.0.ct_eq(key))
            })
    }

    /// The pin for a peer.
    pub fn peer(&self, device: &Id) -> Option<&PeerPin> {
        self.peers.get(device)
    }

    /// Pin a peer after a committed pairing. A fresh pairing (which proved the
    /// QR secret) supersedes an earlier tombstone for the same device.
    pub fn pin(&mut self, device: Id, pin: PeerPin) {
        // Same device, or the same key under a new device id (pairing again
        // mints a new id; the phone keeps its identity key).
        self.tombstones
            .retain(|t| t.device != device && !t.identity.0.ct_eq(&pin.identity.0));
        self.peers.insert(device, pin);
    }

    /// Remove a peer and tombstone its key (§6.4 step 1). Returns whether it
    /// was pinned.
    pub fn revoke(&mut self, device: &Id, now_ms: u64) -> bool {
        let Some(pin) = self.peers.remove(device) else {
            return false;
        };
        let mut keys = vec![pin.identity];
        if let Some(r) = pin.rotation {
            keys.push(r.key);
        }
        for identity in keys {
            self.tombstones.push(Tombstone {
                device: *device,
                identity,
                removed_at_ms: now_ms,
            });
        }
        self.catalog_marks.remove(device);
        true
    }

    /// Desktop: the next catalog version to seal, recorded here. Persist the
    /// store ([`commit`]) **before** using the version, so it is never reused.
    pub fn next_catalog_version(&mut self) -> Result<u64, StoreError> {
        if self.catalog_version >= MAX_VERSION {
            return Err(StoreError::VersionExhausted);
        }
        self.catalog_version += 1;
        Ok(self.catalog_version)
    }

    /// Phone: check a blob's version (and body hash) against the mark.
    pub fn catalog_check(
        &self,
        desktop: &Id,
        version: u64,
        body_sha256: &[u8; 32],
    ) -> CatalogCheck {
        let Some(mark) = self.catalog_marks.get(desktop) else {
            return CatalogCheck::Accept;
        };
        if version > mark.high_water {
            return CatalogCheck::Accept;
        }
        if version < mark.high_water {
            return CatalogCheck::Rollback;
        }
        match &mark.body_sha256 {
            None => CatalogCheck::Rollback,
            Some(h) if *h == hex_lower(body_sha256) => CatalogCheck::Unchanged,
            Some(_) => CatalogCheck::Equivocation,
        }
    }

    /// Phone: record an accepted blob as the new high-water mark.
    pub fn catalog_accept(&mut self, desktop: Id, version: u64, body_sha256: &[u8; 32]) {
        self.catalog_marks.insert(
            desktop,
            CatalogMark {
                high_water: version,
                body_sha256: Some(hex_lower(body_sha256)),
            },
        );
    }

    /// Phone, on `pair-done {catalog_version}`: high-water = version − 1, so
    /// the fresh blob is accepted and no older one is, even from an earlier
    /// pairing (L6).
    pub fn catalog_set_floor(&mut self, desktop: Id, pair_done_version: u64) {
        self.catalog_marks.insert(
            desktop,
            CatalogMark {
                high_water: pair_done_version.saturating_sub(1),
                body_sha256: None,
            },
        );
    }
}

/// Successfully opened store.
#[derive(Debug)]
pub struct Opened {
    /// The contents.
    pub store: PinStore,
    /// The file's (authenticated) generation.
    pub generation: u64,
    /// `Some(g + 1)`: the last write's keychain bump didn't happen; set the
    /// keychain generation to this now.
    pub bump_keychain_to: Option<u64>,
}

fn ad(generation: u64) -> [u8; 16] {
    let mut ad = [0u8; 16];
    ad[..8].copy_from_slice(MAGIC);
    ad[8..].copy_from_slice(&generation.to_be_bytes());
    ad
}

/// Seal `store` as generation `generation`.
pub fn seal(
    state_key: &SymmetricKey,
    generation: u64,
    store: &PinStore,
) -> Result<Vec<u8>, StoreError> {
    let json = Zeroizing::new(
        serde_json::to_vec(store).map_err(|_| StoreError::Lost(LostReason::Malformed))?,
    );
    let (nonce, ct) =
        at_rest::seal(state_key, &ad(generation), &json).map_err(|_| StoreError::Random)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&generation.to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open the store file's bytes (`None` if there is no file) against the
/// keychain's state key and generation.
pub fn open(
    state_key: &SymmetricKey,
    keychain_generation: u64,
    file: Option<&[u8]>,
) -> Result<Opened, StoreError> {
    let Some(bytes) = file else {
        if keychain_generation == 0 {
            return Ok(Opened {
                store: PinStore::default(),
                generation: 0,
                bump_keychain_to: None,
            });
        }
        return Err(StoreError::Lost(LostReason::Missing));
    };
    if bytes.len() < HEADER_LEN + TAG_LEN || &bytes[..8] != MAGIC {
        return Err(StoreError::Lost(LostReason::Malformed));
    }
    let generation = u64::from_be_bytes(bytes[8..16].try_into().expect("8 bytes"));
    let nonce: [u8; NONCE_LEN] = bytes[16..HEADER_LEN].try_into().expect("24 bytes");
    let json = at_rest::open(state_key, &nonce, &ad(generation), &bytes[HEADER_LEN..])
        .map_err(|_| StoreError::Lost(LostReason::Authentication))?;
    let bump_keychain_to = if generation == keychain_generation {
        None
    } else if Some(generation) == keychain_generation.checked_add(1) {
        Some(generation)
    } else {
        return Err(StoreError::Lost(LostReason::Generation {
            file: generation,
            keychain: keychain_generation,
        }));
    };
    let store: PinStore =
        serde_json::from_slice(&json).map_err(|_| StoreError::Lost(LostReason::Malformed))?;
    Ok(Opened {
        store,
        generation,
        bump_keychain_to,
    })
}

/// Read the store file, if present.
pub fn read_file(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    f.take(MAX_FILE_LEN + 1).read_to_end(&mut out)?;
    if out.len() as u64 > MAX_FILE_LEN {
        return Err(StoreError::Lost(LostReason::Malformed));
    }
    Ok(Some(out))
}

/// Write `bytes` to `path` atomically: temp file, fsync, rename, fsync the
/// directory. Creates the parent directory if needed (mode 0700 on Unix; the
/// file is 0600).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Persist `store` as generation `keychain_generation + 1`. On success, set
/// the keychain generation to the returned value.
pub fn commit(
    path: &Path,
    state_key: &SymmetricKey,
    keychain_generation: u64,
    store: &PinStore,
) -> Result<u64, StoreError> {
    let next = keychain_generation
        .checked_add(1)
        .ok_or(StoreError::Lost(LostReason::Malformed))?;
    let bytes = seal(state_key, next, store)?;
    write_atomic(path, &bytes)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> StoredKey {
        StoredKey(PublicKey([b; 32]))
    }

    fn sample() -> PinStore {
        let mut s = PinStore::default();
        s.pin(
            Id([2; 16]),
            PeerPin {
                identity: key(7),
                push: Some(key(8)),
                name: "Pixel".into(),
                pinned_at_ms: 1,
                rotation: None,
            },
        );
        s.catalog_version = 4;
        s
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let mut b = [0u8; 8];
        crate::fill_random(&mut b).unwrap();
        std::env::temp_dir().join(format!("dragent_e2ee_{tag}_{}", hex_lower(&b)))
    }

    #[test]
    fn seal_open_round_trip() {
        let k = SymmetricKey::generate().unwrap();
        let bytes = seal(&k, 5, &sample()).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        let o = open(&k, 5, Some(&bytes)).unwrap();
        assert_eq!(o.store, sample());
        assert_eq!(o.bump_keychain_to, None);
    }

    #[test]
    fn crash_before_keychain_bump_is_recovered() {
        let k = SymmetricKey::generate().unwrap();
        let bytes = seal(&k, 6, &sample()).unwrap();
        let o = open(&k, 5, Some(&bytes)).unwrap();
        assert_eq!(o.bump_keychain_to, Some(6));
    }

    #[test]
    fn rollback_and_future_generations_are_lost() {
        let k = SymmetricKey::generate().unwrap();
        let old = seal(&k, 4, &sample()).unwrap();
        assert!(matches!(
            open(&k, 5, Some(&old)),
            Err(StoreError::Lost(LostReason::Generation {
                file: 4,
                keychain: 5
            }))
        ));
        let ahead = seal(&k, 7, &sample()).unwrap();
        assert!(matches!(
            open(&k, 5, Some(&ahead)),
            Err(StoreError::Lost(LostReason::Generation { .. }))
        ));
    }

    #[test]
    fn generation_is_authenticated() {
        let k = SymmetricKey::generate().unwrap();
        let mut bytes = seal(&k, 4, &sample()).unwrap();
        bytes[15] = 5; // claim generation 5
        assert!(matches!(
            open(&k, 5, Some(&bytes)),
            Err(StoreError::Lost(LostReason::Authentication))
        ));
    }

    #[test]
    fn wrong_key_tamper_truncation_and_missing_file_are_lost() {
        let k = SymmetricKey::generate().unwrap();
        let bytes = seal(&k, 1, &sample()).unwrap();
        let other = SymmetricKey::generate().unwrap();
        assert!(matches!(
            open(&other, 1, Some(&bytes)),
            Err(StoreError::Lost(LostReason::Authentication))
        ));
        let mut t = bytes.clone();
        let last = t.len() - 1;
        t[last] ^= 1;
        assert!(matches!(
            open(&k, 1, Some(&t)),
            Err(StoreError::Lost(LostReason::Authentication))
        ));
        assert!(matches!(
            open(&k, 1, Some(&bytes[..30])),
            Err(StoreError::Lost(LostReason::Malformed))
        ));
        let mut m = bytes.clone();
        m[0] = b'X';
        assert!(matches!(
            open(&k, 1, Some(&m)),
            Err(StoreError::Lost(LostReason::Malformed))
        ));
        assert!(matches!(
            open(&k, 1, None),
            Err(StoreError::Lost(LostReason::Missing))
        ));
        assert_eq!(open(&k, 0, None).unwrap().store, PinStore::default());
    }

    #[test]
    fn commit_and_read_back_through_the_file_system() {
        let dir = tmpdir("store");
        let path = dir.join("e2ee").join("state.v1");
        let k = SymmetricKey::generate().unwrap();
        assert!(read_file(&path).unwrap().is_none());
        let g1 = commit(&path, &k, 0, &sample()).unwrap();
        assert_eq!(g1, 1);
        let mut s = sample();
        s.revoke(&Id([2; 16]), 9);
        let g2 = commit(&path, &k, g1, &s).unwrap();
        let o = open(&k, g2, read_file(&path).unwrap().as_deref()).unwrap();
        assert_eq!(o.store, s);
        assert!(!path.with_file_name("state.v1.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pins_tombstones_and_lookup() {
        let mut s = sample();
        let phone = Id([2; 16]);
        assert_eq!(s.lookup(&phone), PinLookup::Pinned(PublicKey([7; 32])));
        assert_eq!(s.lookup(&Id([9; 16])), PinLookup::Unpinned);
        assert!(s.revoke(&phone, 10));
        assert!(!s.revoke(&phone, 11));
        assert_eq!(s.lookup(&phone), PinLookup::Tombstoned);
        assert!(s.is_tombstoned_key(&PublicKey([7; 32])));
        // Pairing again (proved by P) supersedes the tombstone.
        s.pin(
            phone,
            PeerPin {
                identity: key(9),
                push: None,
                name: "x".into(),
                pinned_at_ms: 12,
                rotation: None,
            },
        );
        assert_eq!(s.lookup(&phone), PinLookup::Pinned(PublicKey([9; 32])));
    }

    #[test]
    fn rotation_candidates() {
        let mut p = sample().peers[&Id([2; 16])].clone();
        assert_eq!(p.candidate_keys(0), vec![PublicKey([7; 32])]);
        p.rotation = Some(PendingRotation {
            key: key(5),
            announced_at_ms: 100,
        });
        assert_eq!(
            p.candidate_keys(100),
            vec![PublicKey([5; 32]), PublicKey([7; 32])]
        );
        assert_eq!(
            p.candidate_keys(100 + ROTATION_GRACE_MS),
            vec![PublicKey([5; 32])]
        );
    }

    #[test]
    fn catalog_version_counter() {
        let mut s = PinStore::default();
        assert_eq!(s.next_catalog_version().unwrap(), 1);
        assert_eq!(s.next_catalog_version().unwrap(), 2);
        s.catalog_version = MAX_VERSION;
        assert!(matches!(
            s.next_catalog_version(),
            Err(StoreError::VersionExhausted)
        ));
    }

    /// §12.3 L6: pair, receive version 5, unpair, pair again (`pair-done`
    /// says 9); the old version-5 blob is rejected.
    #[test]
    fn catalog_rollback_after_repairing() {
        let mut s = PinStore::default();
        let d = Id([3; 16]);
        let (h5, h9, h9b) = ([5u8; 32], [9u8; 32], [10u8; 32]);
        s.catalog_set_floor(d, 5);
        assert_eq!(s.catalog_check(&d, 4, &h5), CatalogCheck::Rollback);
        assert_eq!(s.catalog_check(&d, 5, &h5), CatalogCheck::Accept);
        s.catalog_accept(d, 5, &h5);
        assert_eq!(s.catalog_check(&d, 5, &h5), CatalogCheck::Unchanged);
        assert_eq!(s.catalog_check(&d, 5, &h9), CatalogCheck::Equivocation);
        // Unpair (mark deleted), re-pair with pair-done version 9.
        s.catalog_marks.remove(&d);
        s.catalog_set_floor(d, 9);
        assert_eq!(s.catalog_check(&d, 5, &h5), CatalogCheck::Rollback);
        assert_eq!(s.catalog_check(&d, 8, &h5), CatalogCheck::Rollback);
        assert_eq!(s.catalog_check(&d, 9, &h9), CatalogCheck::Accept);
        s.catalog_accept(d, 9, &h9);
        assert_eq!(s.catalog_check(&d, 9, &h9b), CatalogCheck::Equivocation);
    }

    #[test]
    fn unknown_fields_are_refused() {
        let k = SymmetricKey::generate().unwrap();
        let json =
            br#"{"peers":{},"tombstones":[],"catalog_marks":{},"catalog_version":0,"extra":1}"#;
        let (nonce, ct) = at_rest::seal(&k, &ad(1), json).unwrap();
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&1u64.to_be_bytes());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ct);
        assert!(matches!(
            open(&k, 1, Some(&bytes)),
            Err(StoreError::Lost(LostReason::Malformed))
        ));
    }

    fn phone_pin(identity: u8) -> PeerPin {
        PeerPin {
            identity: key(identity),
            push: None,
            name: "iPhone".into(),
            pinned_at_ms: 2,
            rotation: None,
        }
    }

    #[test]
    fn pairing_again_under_a_new_device_id_lifts_the_key_tombstone() {
        // A phone keeps its identity key; pairing again mints a new device id.
        let mut s = PinStore::default();
        s.pin(Id([1; 16]), phone_pin(9));
        assert!(s.revoke(&Id([1; 16]), 10));
        assert!(s.is_tombstoned_key(&key(9).0), "removed and not paired again");
        s.pin(Id([2; 16]), phone_pin(9));
        assert!(!s.is_tombstoned_key(&key(9).0), "paired again: the key is usable");
        assert!(s.tombstones.iter().all(|t| !t.identity.0.ct_eq(&key(9).0)));
        // Removing the new pairing blocks the key again.
        assert!(s.revoke(&Id([2; 16]), 20));
        assert!(s.is_tombstoned_key(&key(9).0));
    }

    #[test]
    fn a_store_saved_before_the_fix_is_not_locked_out() {
        // State written by the old pin(): the old device's tombstone and the
        // new pairing of the same key side by side.
        let mut s = PinStore::default();
        s.peers.insert(Id([2; 16]), phone_pin(9));
        s.tombstones.push(Tombstone { device: Id([1; 16]), identity: key(9), removed_at_ms: 1 });
        assert!(!s.is_tombstoned_key(&key(9).0));
        // Another removed key stays refused.
        s.tombstones.push(Tombstone { device: Id([3; 16]), identity: key(5), removed_at_ms: 1 });
        assert!(s.is_tombstoned_key(&key(5).0));
    }
}
