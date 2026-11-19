//! The desktop's identity key and pin-store key in the OS keychain
//! (docs/design/e2ee.md §6.1).
//!
//! - One X25519 identity key per install, bound to `AppSettings::device_id`
//!   (the keychain account names carry the device id).
//! - The 32-byte state key that seals the pin store (`e2e_store`) and the
//!   store's monotonic generation number live next to it.
//! - **No secure store means fail closed**: if the keychain can't be read or
//!   written, phone access is off on this machine. There is no file fallback.
//! - Each item is read once per process (the caller caches the opened state;
//!   see `e2e_store::desktop`). Private keys are wiped on drop and are never
//!   logged: the `Debug` impls print public halves only.
//!
//! Keychain layout (service [`KEYCHAIN_SERVICE`]):
//!
//! | account | value |
//! |---|---|
//! | `e2ee/<device id>/keys.v1` | `v1.<64 hex identity secret>.<64 hex state key>` |
//! | `e2ee/<device id>/generation.v1` | the pin-store generation, decimal |
//!
//! The identity secret and the state key share one item: both are written
//! once, when the keys are created, so one read serves both. The generation
//! changes on every pin-store write, so it has its own item and a failed
//! bump can never damage the keys.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use dragent_e2ee::noise::{PublicKey, StaticKeypair};
use dragent_e2ee::seal::at_rest::SymmetricKey;
use dragent_e2ee::zeroize::Zeroizing;
use dragent_e2ee::Id;

/// The keychain service every Dragent secret lives under.
pub const KEYCHAIN_SERVICE: &str = "com.dragent.app";

/// The keychain could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStoreError {
    /// No secure store, access refused, or a platform failure. Phone access
    /// stays off (§6.1, §9).
    Unavailable(String),
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretStoreError::Unavailable(why) => write!(f, "secure key store unavailable: {why}"),
        }
    }
}

/// A secret store: the OS keychain in the app, an in-memory map in tests.
pub trait SecretStore: Send + Sync {
    /// Read an item. `Ok(None)` means the item does not exist.
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretStoreError>;
    /// Create or replace an item.
    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError>;
    /// Delete an item (deleting a missing item is not an error).
    fn delete(&self, account: &str) -> Result<(), SecretStoreError>;
}

/// The OS keychain through `keyring` (macOS Keychain, Windows Credential
/// Manager, Linux Secret Service).
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeychain;

#[cfg(not(target_family = "wasm"))]
impl SecretStore for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretStoreError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| SecretStoreError::Unavailable(keyring_error_class(&e)))?;
        match entry.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretStoreError::Unavailable(keyring_error_class(&e))),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .and_then(|e| e.set_password(value))
            .map_err(|e| SecretStoreError::Unavailable(keyring_error_class(&e)))
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .map_err(|e| SecretStoreError::Unavailable(keyring_error_class(&e)))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretStoreError::Unavailable(keyring_error_class(&e))),
        }
    }
}

/// A short error class with no item contents (errors can echo attributes).
#[cfg(not(target_family = "wasm"))]
fn keyring_error_class(e: &keyring::Error) -> String {
    match e {
        keyring::Error::PlatformFailure(_) => "platform failure",
        keyring::Error::NoStorageAccess(_) => "no storage access",
        keyring::Error::NoEntry => "no entry",
        keyring::Error::BadEncoding(_) => "bad encoding",
        keyring::Error::TooLong(..) => "too long",
        keyring::Error::Invalid(..) => "invalid",
        keyring::Error::Ambiguous(_) => "ambiguous",
        _ => "other",
    }
    .to_string()
}

/// An in-memory secret store for tests and harnesses. It can be switched to
/// "unavailable" to exercise the fail-closed paths.
#[derive(Default)]
pub struct MemorySecretStore {
    items: Mutex<HashMap<String, Zeroizing<String>>>,
    unavailable: AtomicBool,
    /// Fail only writes (e.g. a keychain that can be read but not updated).
    read_only: AtomicBool,
}

impl MemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make every call fail (`true`) or work again (`false`).
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// Make writes fail while reads still work.
    pub fn set_read_only(&self, read_only: bool) {
        self.read_only.store(read_only, Ordering::SeqCst);
    }

    /// Remove an item behind the app's back (a "lost keychain item").
    pub fn forget(&self, account: &str) {
        self.lock().remove(account);
    }

    /// The raw value of an item (tests only).
    pub fn peek(&self, account: &str) -> Option<String> {
        self.lock().get(account).map(|v| v.to_string())
    }

    /// Overwrite an item behind the app's back (tests only).
    pub fn poke(&self, account: &str, value: &str) {
        self.lock()
            .insert(account.to_string(), Zeroizing::new(value.to_string()));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Zeroizing<String>>> {
        self.items.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn check(&self) -> Result<(), SecretStoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(SecretStoreError::Unavailable("test store offline".into()));
        }
        Ok(())
    }

    fn check_write(&self) -> Result<(), SecretStoreError> {
        self.check()?;
        if self.read_only.load(Ordering::SeqCst) {
            return Err(SecretStoreError::Unavailable("test store read-only".into()));
        }
        Ok(())
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<String>>, SecretStoreError> {
        self.check()?;
        Ok(self.lock().get(account).cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretStoreError> {
        self.check_write()?;
        self.lock()
            .insert(account.to_string(), Zeroizing::new(value.to_string()));
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretStoreError> {
        self.check_write()?;
        self.lock().remove(account);
        Ok(())
    }
}

/// The keychain account holding the identity secret and state key.
pub fn keys_account(device: &Id) -> String {
    format!("e2ee/{device}/keys.v1")
}

/// The keychain account holding the pin-store generation.
pub fn generation_account(device: &Id) -> String {
    format!("e2ee/{device}/generation.v1")
}

/// The desktop's long-term secrets: the identity key pair and the pin-store
/// state key. Both are wiped on drop.
pub struct DesktopKeys {
    identity: StaticKeypair,
    state_key: SymmetricKey,
}

impl fmt::Debug for DesktopKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DesktopKeys")
            .field("identity", &self.identity.public())
            .finish_non_exhaustive()
    }
}

impl DesktopKeys {
    /// Fresh keys from the OS generator.
    pub fn generate() -> Result<Self, KeyError> {
        Ok(DesktopKeys {
            identity: StaticKeypair::generate().map_err(|_| KeyError::Random)?,
            state_key: SymmetricKey::generate().map_err(|_| KeyError::Random)?,
        })
    }

    /// The identity key pair.
    pub fn identity(&self) -> &StaticKeypair {
        &self.identity
    }

    /// The identity public key (what the pairing QR carries as `k`).
    pub fn public(&self) -> PublicKey {
        self.identity.public()
    }

    /// The pin-store state key.
    pub fn state_key(&self) -> &SymmetricKey {
        &self.state_key
    }

    /// Split into the identity key pair and the state key.
    pub fn into_parts(self) -> (StaticKeypair, SymmetricKey) {
        (self.identity, self.state_key)
    }

    fn encode(&self) -> Zeroizing<String> {
        let id = self.identity.secret_bytes();
        let mut s = Zeroizing::new(String::with_capacity(3 + 1 + 64 + 1 + 64));
        s.push_str("v1.");
        push_hex(&mut s, &id[..]);
        s.push('.');
        push_hex(&mut s, &self.state_key.bytes()[..]);
        s
    }

    fn decode(value: &str) -> Option<Self> {
        let rest = value.strip_prefix("v1.")?;
        let (id_hex, key_hex) = rest.split_once('.')?;
        let id = hex32(id_hex)?;
        let key = hex32(key_hex)?;
        Some(DesktopKeys {
            identity: StaticKeypair::from_secret_bytes(&id),
            state_key: SymmetricKey::from_bytes(key),
        })
    }
}

fn push_hex(out: &mut String, bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
}

fn hex32(s: &str) -> Option<Zeroizing<[u8; 32]>> {
    let b = s.as_bytes();
    if b.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = Zeroizing::new([0u8; 32]);
    for (i, pair) in b.chunks_exact(2).enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

/// Why the keys could not be loaded or created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// No usable secure store: phone access is off on this machine (§9).
    NoSecureStore(SecretStoreError),
    /// The OS random generator failed.
    Random,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::NoSecureStore(_) => f.write_str(NO_SECURE_STORE_MESSAGE),
            KeyError::Random => f.write_str("the system random number generator failed"),
        }
    }
}

impl std::error::Error for KeyError {}

/// §9 "No secure key store".
pub const NO_SECURE_STORE_MESSAGE: &str = "This computer has no secure key store, so phone access \
     is off. Install GNOME Keyring or KWallet and restart Dragent.";

/// What [`load`] found in the keychain.
#[derive(Debug)]
pub enum Stored {
    /// The keys.
    Keys(DesktopKeys),
    /// No item: a fresh install, or the item was lost.
    Missing,
    /// An item that doesn't parse. Treated like a lost item (new keys, every
    /// phone pairs again), never repaired.
    Malformed,
}

/// Read the identity secret and state key.
pub fn load(store: &dyn SecretStore, device: &Id) -> Result<Stored, KeyError> {
    match store.get(&keys_account(device)) {
        Ok(Some(v)) => Ok(DesktopKeys::decode(&v).map_or(Stored::Malformed, Stored::Keys)),
        Ok(None) => Ok(Stored::Missing),
        Err(e) => Err(KeyError::NoSecureStore(e)),
    }
}

/// Generate new keys, write them, and read them back: a store that accepts
/// writes but doesn't keep them (or returns something else) fails closed.
pub fn create(store: &dyn SecretStore, device: &Id) -> Result<DesktopKeys, KeyError> {
    let keys = DesktopKeys::generate()?;
    let account = keys_account(device);
    store
        .set(&account, &keys.encode())
        .map_err(KeyError::NoSecureStore)?;
    match store.get(&account) {
        Ok(Some(v)) if *v == *keys.encode() => Ok(keys),
        Ok(_) => Err(KeyError::NoSecureStore(SecretStoreError::Unavailable(
            "the keychain did not keep the new key".into(),
        ))),
        Err(e) => Err(KeyError::NoSecureStore(e)),
    }
}

/// The pin-store generation recorded in the keychain (0 when there is none).
/// A value that doesn't parse reads as 0, which makes any existing store fail
/// its generation check: fail closed.
pub fn read_generation(store: &dyn SecretStore, device: &Id) -> Result<u64, KeyError> {
    match store.get(&generation_account(device)) {
        Ok(Some(v)) => Ok(v.trim().parse::<u64>().unwrap_or(0)),
        Ok(None) => Ok(0),
        Err(e) => Err(KeyError::NoSecureStore(e)),
    }
}

/// Record the pin-store generation in the keychain.
pub fn write_generation(
    store: &dyn SecretStore,
    device: &Id,
    generation: u64,
) -> Result<(), SecretStoreError> {
    store.set(&generation_account(device), &generation.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Id {
        Id([9; 16])
    }

    #[test]
    fn create_then_load_round_trips() {
        let s = MemorySecretStore::new();
        assert!(matches!(load(&s, &device()).unwrap(), Stored::Missing));
        let k = create(&s, &device()).unwrap();
        let Stored::Keys(again) = load(&s, &device()).unwrap() else {
            panic!("keys not found");
        };
        assert_eq!(k.public(), again.public());
        assert_eq!(k.state_key().bytes(), again.state_key().bytes());
        // The item is bound to the device id.
        assert!(matches!(load(&s, &Id([8; 16])).unwrap(), Stored::Missing));
    }

    #[test]
    fn malformed_item_is_not_repaired() {
        let s = MemorySecretStore::new();
        s.poke(&keys_account(&device()), "v1.zz.zz");
        assert!(matches!(load(&s, &device()).unwrap(), Stored::Malformed));
        s.poke(&keys_account(&device()), &format!("v1.{}", "a".repeat(64)));
        assert!(matches!(load(&s, &device()).unwrap(), Stored::Malformed));
        s.poke(
            &keys_account(&device()),
            &format!("v1.{}.{}", "A".repeat(64), "a".repeat(64)),
        );
        assert!(matches!(load(&s, &device()).unwrap(), Stored::Malformed));
    }

    #[test]
    fn no_secure_store_fails_closed() {
        let s = MemorySecretStore::new();
        s.set_unavailable(true);
        assert!(matches!(
            load(&s, &device()),
            Err(KeyError::NoSecureStore(_))
        ));
        assert!(matches!(
            create(&s, &device()),
            Err(KeyError::NoSecureStore(_))
        ));
        assert!(read_generation(&s, &device()).is_err());
        assert!(
            KeyError::NoSecureStore(SecretStoreError::Unavailable("x".into()))
                .to_string()
                .contains("no secure key store")
        );
    }

    #[test]
    fn a_store_that_forgets_writes_fails_closed() {
        /// Accepts writes, keeps nothing (keyring's mock store behaves so).
        struct Forgetful;
        impl SecretStore for Forgetful {
            fn get(&self, _: &str) -> Result<Option<Zeroizing<String>>, SecretStoreError> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), SecretStoreError> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), SecretStoreError> {
                Ok(())
            }
        }
        assert!(matches!(
            create(&Forgetful, &device()),
            Err(KeyError::NoSecureStore(_))
        ));
    }

    #[test]
    fn generation_round_trips_and_garbage_reads_zero() {
        let s = MemorySecretStore::new();
        assert_eq!(read_generation(&s, &device()).unwrap(), 0);
        write_generation(&s, &device(), 41).unwrap();
        assert_eq!(read_generation(&s, &device()).unwrap(), 41);
        s.poke(&generation_account(&device()), "forty-two");
        assert_eq!(read_generation(&s, &device()).unwrap(), 0);
    }

    #[test]
    fn debug_never_prints_secrets() {
        let k = DesktopKeys::generate().unwrap();
        let printed = format!("{k:?}");
        let mut secret_hex = String::new();
        push_hex(&mut secret_hex, &k.identity().secret_bytes()[..]);
        assert!(!printed.contains(&secret_hex));
        let mut state_hex = String::new();
        push_hex(&mut state_hex, &k.state_key().bytes()[..]);
        assert!(!printed.contains(&state_hex));
    }
}
