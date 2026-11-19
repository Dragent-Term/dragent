//! uniffi binding of `dragent_e2ee` for the phone (docs/design/e2ee.md §3.2,
//! §13 E4).
//!
//! This crate contains no cryptography of its own. Every operation is a thin
//! wrapper over `dragent_e2ee`, which holds the Noise patterns (revision 34),
//! the §4.2/§4.3 channel state machine, the seals and the codecs on the §3.2
//! pins. What lives here:
//!
//! - types that cross the FFI (records, enums and one error type);
//! - stateful objects (key pairs, handshakes, channels, reassemblers, part
//!   sessions, the pin store) behind a mutex, because uniffi objects are
//!   shared across threads;
//! - the push-pin list format (§8) shared by the app and the iOS Notification
//!   Service Extension / Android messaging service, and the one-call push open
//!   (decrypt, sender lookup by static key, `desktop_id` check, staleness).
//!
//! Private keys: [`Keypair`] objects are created and held by the native
//! module (Swift/Kotlin) from the keychain / Keystore. JavaScript never sees a
//! private key: it gets public keys and opaque handshake/channel objects.

#![deny(unsafe_op_in_unsafe_fn)]

use std::fmt;

uniffi::setup_scaffolding!();

mod bench;
mod catalog;
mod channel;
mod codec;
mod keys;
mod pin_store;
mod push;
#[cfg(feature = "vectors")]
mod selftest;
#[cfg(test)]
mod tests;

pub use bench::*;
pub use catalog::*;
pub use channel::*;
pub use codec::*;
pub use keys::*;
pub use pin_store::*;
pub use push::*;
#[cfg(feature = "vectors")]
pub use selftest::*;

use dragent_e2ee::{ChannelId, Id};

/// Every failure that crosses the FFI. Variants carry no secrets: details are
/// lengths, states and error kinds only (§2.3 applies to the phone too).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum E2eeError {
    /// A caller-supplied value is malformed (an id, a key length, a size).
    InvalidArgument {
        /// What was wrong.
        detail: String,
    },
    /// Authentication failed: a bad tag, wrong key or prologue. Never act on
    /// the message.
    Rejected {
        /// Which check.
        detail: String,
    },
    /// A message or blob had the wrong shape.
    Malformed {
        /// Which check.
        detail: String,
    },
    /// The OS random number generator failed.
    Random,
    /// §4.3: the channel closed with `protocol_error`; its keys are zeroised.
    /// Send CLOSE `protocol_error`, discard partial messages, fail pending
    /// requests, discard queued input, reconnect and re-seed.
    ProtocolError {
        /// The violation (counter values only).
        detail: String,
    },
    /// The channel is closed.
    ChannelClosed {
        /// Why.
        reason: String,
    },
    /// Not allowed in the channel's current state.
    NotReady,
    /// The send counter is exhausted (should have rekeyed long ago).
    CounterExhausted,
    /// A record or payload is too large.
    TooLarge,
    /// A sealed catalog or push came from a key that isn't pinned.
    WrongSender,
    /// Catalog version older than the high-water mark (rollback).
    CatalogRollback,
    /// Same catalog version, different body (equivocation): flag it.
    CatalogEquivocation,
    /// §6.1: the pin store failed its check. Fail closed: pair again.
    StoreLost {
        /// Which check (no contents).
        reason: String,
    },
    /// File system error (pin store).
    Io {
        /// The error kind.
        detail: String,
    },
    /// A handshake or session object was used after it was consumed.
    Consumed,
    /// An internal error (a bug).
    Internal {
        /// What.
        detail: String,
    },
}

impl fmt::Display for E2eeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())?;
        match self {
            E2eeError::InvalidArgument { detail }
            | E2eeError::Rejected { detail }
            | E2eeError::Malformed { detail }
            | E2eeError::ProtocolError { detail }
            | E2eeError::Io { detail }
            | E2eeError::Internal { detail } => write!(f, ": {detail}"),
            E2eeError::ChannelClosed { reason } | E2eeError::StoreLost { reason } => {
                write!(f, ": {reason}")
            }
            _ => Ok(()),
        }
    }
}

impl std::error::Error for E2eeError {}

impl E2eeError {
    /// Stable code for JS (`E2EE_*`), used by the Expo module's exceptions.
    pub fn code(&self) -> &'static str {
        match self {
            E2eeError::InvalidArgument { .. } => "E2EE_INVALID_ARGUMENT",
            E2eeError::Rejected { .. } => "E2EE_REJECTED",
            E2eeError::Malformed { .. } => "E2EE_MALFORMED",
            E2eeError::Random => "E2EE_RANDOM",
            E2eeError::ProtocolError { .. } => "E2EE_PROTOCOL_ERROR",
            E2eeError::ChannelClosed { .. } => "E2EE_CHANNEL_CLOSED",
            E2eeError::NotReady => "E2EE_NOT_READY",
            E2eeError::CounterExhausted => "E2EE_COUNTER_EXHAUSTED",
            E2eeError::TooLarge => "E2EE_TOO_LARGE",
            E2eeError::WrongSender => "E2EE_WRONG_SENDER",
            E2eeError::CatalogRollback => "E2EE_CATALOG_ROLLBACK",
            E2eeError::CatalogEquivocation => "E2EE_CATALOG_EQUIVOCATION",
            E2eeError::StoreLost { .. } => "E2EE_STORE_LOST",
            E2eeError::Io { .. } => "E2EE_IO",
            E2eeError::Consumed => "E2EE_CONSUMED",
            E2eeError::Internal { .. } => "E2EE_INTERNAL",
        }
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        E2eeError::InvalidArgument {
            detail: detail.into(),
        }
    }

    pub(crate) fn malformed(detail: impl fmt::Debug) -> Self {
        E2eeError::Malformed {
            detail: format!("{detail:?}"),
        }
    }
}

/// The stable error code of an [`E2eeError`] (for the native glue).
#[uniffi::export]
pub fn error_code(error: &E2eeError) -> String {
    error.code().to_string()
}

impl From<dragent_e2ee::noise::NoiseError> for E2eeError {
    fn from(e: dragent_e2ee::noise::NoiseError) -> Self {
        use dragent_e2ee::noise::NoiseError as N;
        match e {
            N::Rejected => E2eeError::Rejected {
                detail: "noise".into(),
            },
            N::Malformed => E2eeError::Malformed {
                detail: "noise message length".into(),
            },
            N::Random => E2eeError::Random,
            N::Internal => E2eeError::Internal {
                detail: "noise state".into(),
            },
        }
    }
}

impl From<dragent_e2ee::RandomError> for E2eeError {
    fn from(_: dragent_e2ee::RandomError) -> Self {
        E2eeError::Random
    }
}

/// Parse a canonical lowercase UUID (§5).
pub(crate) fn parse_id(what: &str, s: &str) -> Result<Id, E2eeError> {
    Id::parse(s)
        .map_err(|_| E2eeError::invalid(format!("{what} is not a lowercase canonical UUID")))
}

/// A 16-byte channel id.
pub(crate) fn parse_chan(b: &[u8]) -> Result<ChannelId, E2eeError> {
    <[u8; 16]>::try_from(b)
        .map(ChannelId)
        .map_err(|_| E2eeError::invalid("channel id must be 16 bytes"))
}

pub(crate) fn parse_key(what: &str, b: &[u8]) -> Result<dragent_e2ee::noise::PublicKey, E2eeError> {
    dragent_e2ee::noise::PublicKey::from_slice(b)
        .ok_or_else(|| E2eeError::invalid(format!("{what} must be 32 bytes")))
}

pub(crate) fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic while holding the lock leaves the guarded state as it was at
    // the panic; every method re-checks state before acting, so recover.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A fresh random channel id (§4.1: initiator-chosen).
#[uniffi::export]
pub fn random_channel_id() -> Result<Vec<u8>, E2eeError> {
    Ok(ChannelId::random()?.as_bytes().to_vec())
}

/// `n` random bytes from the OS generator (notification ids, nonces for
/// callers that need one). At most 1 KiB.
#[uniffi::export]
pub fn random_bytes(n: u32) -> Result<Vec<u8>, E2eeError> {
    if n > 1024 {
        return Err(E2eeError::invalid("at most 1024 random bytes"));
    }
    let mut out = vec![0u8; n as usize];
    dragent_e2ee::fill_random(&mut out)?;
    Ok(out)
}

/// SHA-256 (catalog body hashes for the high-water mark, NSE thread ids).
#[uniffi::export]
pub fn sha256(data: Vec<u8>) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(&data).to_vec()
}

/// The protocol version byte (§4.1) and binding version, for diagnostics.
#[uniffi::export]
pub fn binding_info() -> BindingInfo {
    BindingInfo {
        protocol_version: dragent_e2ee::PROTOCOL_VERSION,
        binding_version: env!("CARGO_PKG_VERSION").to_string(),
        vectors_compiled: cfg!(feature = "vectors"),
    }
}

/// See [`binding_info`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct BindingInfo {
    /// §4.1 header version.
    pub protocol_version: u8,
    /// This crate's version.
    pub binding_version: String,
    /// Whether the vector runners are compiled in (test builds only).
    pub vectors_compiled: bool,
}
