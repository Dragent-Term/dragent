//! X25519 static key pairs (§6.1): the phone's identity key and push key.
//!
//! The native module generates a pair here, writes [`Keypair::secret_bytes`]
//! to the keychain / Keystore with the §6.7 class, and afterwards rebuilds it
//! with [`Keypair::from_secret`]. The pair object itself stays native; JS only
//! ever sees [`Keypair::public_key`].

use std::sync::Arc;

use dragent_e2ee::noise::StaticKeypair;
use zeroize::{Zeroize, Zeroizing};

use crate::E2eeError;

/// A static X25519 key pair. The secret is wiped when the object is dropped.
#[derive(uniffi::Object)]
pub struct Keypair {
    pub(crate) inner: StaticKeypair,
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

#[uniffi::export]
impl Keypair {
    /// A fresh pair from the OS generator.
    #[uniffi::constructor]
    pub fn generate() -> Result<Arc<Self>, E2eeError> {
        Ok(Arc::new(Keypair {
            inner: StaticKeypair::generate()?,
        }))
    }

    /// Rebuild a pair from its stored 32-byte secret. The input buffer is
    /// wiped on the Rust side; the caller wipes its own copy.
    #[uniffi::constructor]
    pub fn from_secret(mut secret: Vec<u8>) -> Result<Arc<Self>, E2eeError> {
        let result = <[u8; 32]>::try_from(&secret[..])
            .map(|b| {
                let z = Zeroizing::new(b);
                Arc::new(Keypair {
                    inner: StaticKeypair::from_secret_bytes(&z),
                })
            })
            .map_err(|_| E2eeError::invalid("secret key must be 32 bytes"));
        secret.zeroize();
        result
    }

    /// The 32-byte public half.
    pub fn public_key(&self) -> Vec<u8> {
        self.inner.public().as_bytes().to_vec()
    }

    /// The 32-byte secret, for writing to the keychain / Keystore only.
    ///
    /// The returned buffer is copied across the FFI; the native caller wipes
    /// it after storing (P2 audit item: FFI copies are not zeroised by uniffi).
    pub fn secret_bytes(&self) -> Vec<u8> {
        self.inner.secret_bytes().to_vec()
    }
}
