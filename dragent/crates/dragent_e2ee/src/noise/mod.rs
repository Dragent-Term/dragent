//! Noise Protocol Framework (revision 34) wrappers on the §3.2 pins.
//!
//! | Use | Pattern |
//! |---|---|
//! | Live phone ↔ desktop channel | `Noise_KK_25519_ChaChaPoly_SHA256` ([`kk`]) |
//! | Pairing | `Noise_IKpsk1_25519_ChaChaPoly_SHA256` ([`pairing`]) |
//! | One-way seals (catalog key wrap, push) | `Noise_X_25519_ChaChaPoly_SHA256` ([`x`]) |
//!
//! Every handshake binds its §5 prologue. After a two-way handshake the keys
//! drive a [`Transport`] in `snow`'s stateless mode: the DATA counter is the
//! explicit nonce (§4.3), and counters are tracked by [`crate::channel`].

use std::fmt;

use snow::params::NoiseParams;
use snow::{Builder, HandshakeState, StatelessTransportState};
use zeroize::Zeroizing;

pub mod keys;
pub mod kk;
pub mod pairing;
pub mod prologue;
mod resolver;
pub mod x;

/// Noise vector runner: tests, and the opt-in `vectors` feature (never in
/// release builds; the phone module's simulator tests only).
#[cfg(any(test, feature = "vectors"))]
pub mod vectors;

pub use keys::{PublicKey, StaticKeypair};
pub use prologue::{CatalogPrologue, KkPrologue, PairingPrologue, PushPrologue};

pub(crate) const KK: &str = "Noise_KK_25519_ChaChaPoly_SHA256";
pub(crate) const IKPSK1: &str = "Noise_IKpsk1_25519_ChaChaPoly_SHA256";
pub(crate) const X: &str = "Noise_X_25519_ChaChaPoly_SHA256";

/// The largest Noise message (handshake or transport), in bytes.
pub const MAX_MESSAGE_LEN: usize = 65_535;
/// The AEAD tag length.
pub const TAG_LEN: usize = 16;
/// The largest transport plaintext: one §4.4 record of 65,519 bytes.
pub const MAX_TRANSPORT_PLAINTEXT: usize = MAX_MESSAGE_LEN - TAG_LEN;
/// Length of the handshake hash `h` (SHA-256).
pub const HASH_LEN: usize = 32;

/// A Noise operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseError {
    /// Authentication failed: a bad tag, a wrong key or prologue, or a DH
    /// check (low-order point). The message must not be acted on.
    Rejected,
    /// A message had the wrong length for its position, or a payload was too
    /// large to send.
    Malformed,
    /// The operating system random number generator failed.
    Random,
    /// An internal state error (a bug in the caller or in this crate).
    Internal,
}

impl fmt::Display for NoiseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NoiseError::Rejected => "message failed authentication",
            NoiseError::Malformed => "malformed or oversized message",
            NoiseError::Random => "random number generator failed",
            NoiseError::Internal => "internal Noise state error",
        })
    }
}

impl std::error::Error for NoiseError {}

impl From<snow::Error> for NoiseError {
    fn from(e: snow::Error) -> Self {
        match e {
            snow::Error::Decrypt | snow::Error::Dh => NoiseError::Rejected,
            snow::Error::Input => NoiseError::Malformed,
            snow::Error::Rng => NoiseError::Random,
            _ => NoiseError::Internal,
        }
    }
}

/// Constant-time byte-slice equality (lengths are not secret).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// Everything needed to build one side of a handshake. Internal: the public
/// wrappers fill it from typed prologues, and the vector tests from vectors.
pub(crate) struct Setup<'a> {
    pub pattern: &'static str,
    pub local_secret: Option<&'a [u8]>,
    pub remote_public: Option<&'a [u8]>,
    pub prologue: &'a [u8],
    /// (location, key)
    pub psk: Option<(u8, &'a [u8; 32])>,
    #[cfg(any(test, feature = "vectors"))]
    pub fixed_ephemeral: Option<&'a [u8]>,
}

impl<'a> Setup<'a> {
    pub(crate) fn new(pattern: &'static str, prologue: &'a [u8]) -> Self {
        Setup {
            pattern,
            local_secret: None,
            remote_public: None,
            prologue,
            psk: None,
            #[cfg(any(test, feature = "vectors"))]
            fixed_ephemeral: None,
        }
    }

    pub(crate) fn build(self, initiator: bool) -> Result<HandshakeState, NoiseError> {
        let params: NoiseParams = self.pattern.parse().map_err(|_| NoiseError::Internal)?;
        let mut b = Builder::with_resolver(params, Box::new(resolver::E2eeResolver))
            .prologue(self.prologue)?;
        if let Some(s) = self.local_secret {
            b = b.local_private_key(s)?;
        }
        if let Some(rs) = self.remote_public {
            b = b.remote_public_key(rs)?;
        }
        if let Some((loc, k)) = self.psk {
            b = b.psk(loc, k)?;
        }
        #[cfg(any(test, feature = "vectors"))]
        if let Some(e) = self.fixed_ephemeral {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        Ok(if initiator {
            b.build_initiator()?
        } else {
            b.build_responder()?
        })
    }
}

pub(crate) fn hs_write(hs: &mut HandshakeState, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
    let mut out = vec![0u8; MAX_MESSAGE_LEN];
    let n = hs.write_message(payload, &mut out)?;
    out.truncate(n);
    Ok(out)
}

pub(crate) fn hs_read(
    hs: &mut HandshakeState,
    message: &[u8],
) -> Result<Zeroizing<Vec<u8>>, NoiseError> {
    if message.len() > MAX_MESSAGE_LEN {
        return Err(NoiseError::Malformed);
    }
    let mut out = Zeroizing::new(vec![0u8; message.len()]);
    let n = hs.read_message(message, &mut out)?;
    out.truncate(n);
    Ok(out)
}

/// The transport keys of a finished two-way handshake, with explicit nonces.
///
/// Keys are wiped when this is dropped or [`Transport::zeroize`] is called.
/// Counter discipline (start at 0, +1 per frame, never reuse, gap = error) is
/// enforced by [`crate::channel::Channel`], not here.
pub struct Transport {
    state: Option<StatelessTransportState>,
    handshake_hash: [u8; HASH_LEN],
    remote_static: PublicKey,
}

impl Transport {
    pub(crate) fn from_handshake(hs: HandshakeState) -> Result<Self, NoiseError> {
        if !hs.is_handshake_finished() {
            return Err(NoiseError::Internal);
        }
        let mut handshake_hash = [0u8; HASH_LEN];
        let h = hs.get_handshake_hash();
        if h.len() != HASH_LEN {
            return Err(NoiseError::Internal);
        }
        handshake_hash.copy_from_slice(h);
        let remote_static = hs
            .get_remote_static()
            .and_then(PublicKey::from_slice)
            .ok_or(NoiseError::Internal)?;
        let state = hs.into_stateless_transport_mode()?;
        Ok(Transport {
            state: Some(state),
            handshake_hash,
            remote_static,
        })
    }

    /// The handshake hash `h` (used by pairing's `pair-confirm {h}`, §6.2).
    pub fn handshake_hash(&self) -> &[u8; HASH_LEN] {
        &self.handshake_hash
    }

    /// The peer's authenticated static key.
    pub fn remote_static(&self) -> PublicKey {
        self.remote_static
    }

    /// Whether the keys are still present (false after [`Transport::zeroize`]).
    pub fn has_keys(&self) -> bool {
        self.state.is_some()
    }

    /// Encrypt one transport message under counter `nonce`.
    pub fn seal(&self, nonce: u64, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let state = self.state.as_ref().ok_or(NoiseError::Internal)?;
        if plaintext.len() > MAX_TRANSPORT_PLAINTEXT {
            return Err(NoiseError::Malformed);
        }
        let mut out = vec![0u8; plaintext.len() + TAG_LEN];
        let n = state.write_message(nonce, plaintext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Decrypt one transport message under counter `nonce`.
    pub fn open(&self, nonce: u64, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, NoiseError> {
        let state = self.state.as_ref().ok_or(NoiseError::Internal)?;
        if ciphertext.len() < TAG_LEN || ciphertext.len() > MAX_MESSAGE_LEN {
            return Err(NoiseError::Rejected);
        }
        let mut out = Zeroizing::new(vec![0u8; ciphertext.len()]);
        let n = state.read_message(nonce, ciphertext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Wipe the transport keys now. Further seals and opens fail.
    pub fn zeroize(&mut self) {
        // Dropping the state drops our resolver's ciphers, which wipe their keys.
        self.state = None;
    }
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("has_keys", &self.has_keys())
            .field("remote_static", &self.remote_static)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_works() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
