//! X25519 static keys: device identity keys and the phone's push key (§6.1).

use std::fmt;

use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::rng::{fill_random, RandomError};

/// An X25519 public key: a pin, a remote static, or our own public half.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// The raw 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// From a slice that must be exactly 32 bytes.
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        <[u8; 32]>::try_from(b).ok().map(PublicKey)
    }

    /// Constant-time equality, for comparing a remote static against a pin.
    pub fn ct_eq(&self, other: &PublicKey) -> bool {
        crate::noise::ct_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short prefix only: full fingerprints never go to logs (§2.3).
        write!(f, "PublicKey({}…)", crate::ids::hex_lower(&self.0[..4]))
    }
}

/// An X25519 static key pair. The secret is wiped when this is dropped, and it
/// is never printed.
pub struct StaticKeypair {
    secret: StaticSecret,
    public: PublicKey,
}

impl StaticKeypair {
    /// A fresh key pair from the operating system generator.
    pub fn generate() -> Result<Self, RandomError> {
        let mut bytes = Zeroizing::new([0u8; 32]);
        fill_random(&mut bytes[..])?;
        Ok(Self::from_secret_bytes(&bytes))
    }

    /// Rebuild a key pair from its stored 32-byte secret (keychain or secure
    /// store). The caller keeps `bytes` in a zeroising buffer.
    pub fn from_secret_bytes(bytes: &Zeroizing<[u8; 32]>) -> Self {
        let secret = StaticSecret::from(**bytes);
        let public = PublicKey(XPublicKey::from(&secret).to_bytes());
        Self { secret, public }
    }

    /// The 32-byte secret, for writing to the keychain or secure store only.
    pub fn secret_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.secret.to_bytes())
    }

    /// The public half.
    pub fn public(&self) -> PublicKey {
        self.public
    }

    pub(crate) fn secret_slice(&self) -> &[u8] {
        self.secret.as_bytes()
    }
}

impl fmt::Debug for StaticKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaticKeypair")
            .field("public", &self.public)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_round_trips() {
        let k = StaticKeypair::generate().unwrap();
        let again = StaticKeypair::from_secret_bytes(&k.secret_bytes());
        assert_eq!(k.public(), again.public());
        assert_ne!(k.public(), StaticKeypair::generate().unwrap().public());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let k = StaticKeypair::generate().unwrap();
        let s = format!("{k:?}");
        assert!(!s.contains(&crate::ids::hex_lower(&k.secret_bytes()[..])));
        assert!(s.len() < 80);
    }
}
