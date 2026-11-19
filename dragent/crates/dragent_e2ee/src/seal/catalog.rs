//! §6.5 catalog at rest: the cryptographic half. (The blob's JSON layout and
//! padding are the codec's; sealing and publishing are the desktop's.)
//!
//! 1. The desktop increments its persisted `version` (≤ 2^53 − 1, never
//!    reused; [`super::pin_store::PinStore::next_catalog_version`]).
//! 2. It encrypts the catalog JSON once with a random `K` using
//!    XChaCha20-Poly1305 with `AD = "dragent catalog v1" || account || desktop
//!    || version` ([`seal_body`]).
//! 3. It wraps `K || version` for each paired phone with Noise X under the
//!    catalog prologue ([`wrap_key`]).
//!
//! The phone checks that the X sender is the pinned desktop key and that the
//! versions agree ([`unwrap_key`], [`open_body`]); the high-water check is
//! [`super::pin_store::PinStore::catalog_check`].
//!
//! AD encoding: the 18 ASCII bytes `dragent catalog v1`, then the account and
//! desktop ids as 16 raw bytes each, then `version` as a big-endian u64
//! (58 bytes, fixed length, so the concatenation is unambiguous).

use zeroize::Zeroizing;

use super::at_rest::{self, SymmetricKey, NONCE_LEN};
use crate::ids::Id;
use crate::noise::{x, CatalogPrologue, NoiseError, PublicKey, StaticKeypair};
use crate::rng::RandomError;

/// Catalog versions are at most 2^53 − 1 (they travel through JSON).
pub const MAX_VERSION: u64 = (1 << 53) - 1;
/// AD label.
pub const AD_LABEL: &[u8] = b"dragent catalog v1";
/// Length of a key wrap: X overhead + 32-byte key + 8-byte version.
pub const WRAP_LEN: usize = x::OVERHEAD + 32 + 8;

/// The catalog body AD.
pub fn body_ad(account: &Id, desktop: &Id, version: u64) -> Vec<u8> {
    let mut ad = Vec::with_capacity(AD_LABEL.len() + 16 + 16 + 8);
    ad.extend_from_slice(AD_LABEL);
    ad.extend_from_slice(account.as_bytes());
    ad.extend_from_slice(desktop.as_bytes());
    ad.extend_from_slice(&version.to_be_bytes());
    ad
}

/// A catalog sealing or opening failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// Version is 0 or above 2^53 − 1.
    BadVersion,
    /// The wrap or body failed authentication (wrong key, tampering, wrong prologue/AD).
    Rejected,
    /// The wrap was sealed by a key other than the pinned desktop key.
    WrongSender,
    /// The version inside the wrap disagrees with the blob's.
    VersionMismatch,
    /// The wrap is not the expected length.
    Malformed,
    /// The OS RNG failed.
    Random,
}

impl From<RandomError> for CatalogError {
    fn from(_: RandomError) -> Self {
        CatalogError::Random
    }
}

impl From<NoiseError> for CatalogError {
    fn from(e: NoiseError) -> Self {
        match e {
            NoiseError::Random => CatalogError::Random,
            NoiseError::Malformed => CatalogError::Malformed,
            _ => CatalogError::Rejected,
        }
    }
}

fn check_version(version: u64) -> Result<(), CatalogError> {
    if version == 0 || version > MAX_VERSION {
        return Err(CatalogError::BadVersion);
    }
    Ok(())
}

/// Step 2: encrypt the catalog JSON under a fresh `K`. Returns `K` (to wrap
/// for each phone), the nonce and the body ciphertext.
pub fn seal_body(
    account: &Id,
    desktop: &Id,
    version: u64,
    catalog_json: &[u8],
) -> Result<(SymmetricKey, [u8; NONCE_LEN], Vec<u8>), CatalogError> {
    check_version(version)?;
    let k = SymmetricKey::generate()?;
    let (nonce, body) = at_rest::seal(&k, &body_ad(account, desktop, version), catalog_json)?;
    Ok((k, nonce, body))
}

/// Step 3: wrap `K || version` to one phone.
pub fn wrap_key(
    desktop_identity: &StaticKeypair,
    phone_key: &PublicKey,
    prologue: &CatalogPrologue,
    k: &SymmetricKey,
) -> Result<Vec<u8>, CatalogError> {
    check_version(prologue.version)?;
    let mut pt = Zeroizing::new([0u8; 40]);
    pt[..32].copy_from_slice(k.bytes());
    pt[32..].copy_from_slice(&prologue.version.to_be_bytes());
    Ok(x::seal(
        desktop_identity,
        phone_key,
        &prologue.bytes(),
        &pt[..],
    )?)
}

/// Phone: open this phone's wrap. `prologue.version` is the blob's `version`;
/// the sender must be `pinned_desktop`, and the wrapped version must match.
pub fn unwrap_key(
    phone_identity: &StaticKeypair,
    pinned_desktop: &PublicKey,
    prologue: &CatalogPrologue,
    wrap: &[u8],
) -> Result<SymmetricKey, CatalogError> {
    check_version(prologue.version)?;
    if wrap.len() != WRAP_LEN {
        return Err(CatalogError::Malformed);
    }
    let (sender, pt) = x::open(phone_identity, &prologue.bytes(), wrap)?;
    if !sender.ct_eq(pinned_desktop) {
        return Err(CatalogError::WrongSender);
    }
    if pt.len() != 40 {
        return Err(CatalogError::Malformed);
    }
    let mut version = [0u8; 8];
    version.copy_from_slice(&pt[32..]);
    if u64::from_be_bytes(version) != prologue.version {
        return Err(CatalogError::VersionMismatch);
    }
    let mut k = Zeroizing::new([0u8; 32]);
    k.copy_from_slice(&pt[..32]);
    Ok(SymmetricKey::from_bytes(k))
}

/// Phone: decrypt the body with the unwrapped `K`.
pub fn open_body(
    k: &SymmetricKey,
    account: &Id,
    desktop: &Id,
    version: u64,
    nonce: &[u8; NONCE_LEN],
    body: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CatalogError> {
    check_version(version)?;
    at_rest::open(k, nonce, &body_ad(account, desktop, version), body)
        .map_err(|_| CatalogError::Rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Id = Id([1; 16]);
    const D: Id = Id([3; 16]);
    const P: Id = Id([2; 16]);

    fn pro(version: u64) -> CatalogPrologue {
        CatalogPrologue {
            account: A,
            desktop: D,
            phone: P,
            version,
        }
    }

    #[test]
    fn ad_layout() {
        let ad = body_ad(&A, &D, 0x0102);
        assert_eq!(ad.len(), 58);
        assert_eq!(&ad[..18], b"dragent catalog v1");
        assert_eq!(&ad[18..34], &[1u8; 16]);
        assert_eq!(&ad[34..50], &[3u8; 16]);
        assert_eq!(&ad[50..], &[0, 0, 0, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn seal_wrap_unwrap_open() {
        let desktop = StaticKeypair::generate().unwrap();
        let phone = StaticKeypair::generate().unwrap();
        let (k, nonce, body) = seal_body(&A, &D, 7, br#"{"projects":[]}"#).unwrap();
        let wrap = wrap_key(&desktop, &phone.public(), &pro(7), &k).unwrap();
        assert_eq!(wrap.len(), WRAP_LEN);
        let k2 = unwrap_key(&phone, &desktop.public(), &pro(7), &wrap).unwrap();
        let json = open_body(&k2, &A, &D, 7, &nonce, &body).unwrap();
        assert_eq!(&json[..], br#"{"projects":[]}"#);
    }

    #[test]
    fn wrong_sender_version_or_ad_is_rejected() {
        let desktop = StaticKeypair::generate().unwrap();
        let relay = StaticKeypair::generate().unwrap();
        let phone = StaticKeypair::generate().unwrap();
        let (k, nonce, body) = seal_body(&A, &D, 7, b"{}").unwrap();
        // Sealed by someone else's key.
        let forged = wrap_key(&relay, &phone.public(), &pro(7), &k).unwrap();
        assert_eq!(
            unwrap_key(&phone, &desktop.public(), &pro(7), &forged).unwrap_err(),
            CatalogError::WrongSender
        );
        // Wrap for version 7 presented as version 8.
        let wrap = wrap_key(&desktop, &phone.public(), &pro(7), &k).unwrap();
        assert_eq!(
            unwrap_key(&phone, &desktop.public(), &pro(8), &wrap).unwrap_err(),
            CatalogError::Rejected
        );
        // Body presented under another version or desktop.
        assert!(open_body(&k, &A, &D, 8, &nonce, &body).is_err());
        assert!(open_body(&k, &A, &P, 7, &nonce, &body).is_err());
        assert_eq!(
            seal_body(&A, &D, 0, b"").unwrap_err(),
            CatalogError::BadVersion
        );
        assert_eq!(
            seal_body(&A, &D, MAX_VERSION + 1, b"").unwrap_err(),
            CatalogError::BadVersion
        );
    }
}
