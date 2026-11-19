//! End-to-end encryption core for the phone relay and push notifications.
//!
//! The specification is `docs/design/e2ee.md` (revision 9). This crate is the
//! one implementation used by the desktop app, the iOS and Android native
//! module and the iOS Notification Service Extension (§3.2). It contains no
//! cryptographic primitives of its own: Noise (revision 34) comes from `snow`,
//! X25519 from `x25519-dalek`, the AEAD from `chacha20poly1305`, and HKDF and
//! SHA-256 from `hkdf` and `sha2`, all at the exact pins in `Cargo.toml`.
//!
//! Layout:
//!
//! - [`noise`]: the three handshake patterns (KK, IKpsk1, X) on the §3.2 pins,
//!   with the §5 prologues, a zeroising crypto resolver, identity keys and the
//!   stateless (explicit-nonce) transport.
//! - [`channel`]: the §4.2 companion channel state machine, the §4.3 DATA
//!   receiver (any counter gap is a protocol error) and the §4.6 resume
//!   bookkeeping (last sent counter, old-channel drain, `input_lost`).
//! - [`seal`]: data at rest and one-way seals: the sealed pin store (§6.1),
//!   XChaCha20-Poly1305 bodies, the catalog key wrap (§6.5) and the push seal
//!   (§8).
//! - [`codec`]: wire codecs (header, record, TERM, parts, blobs); owned by E1b.
//!
//! Nothing here performs I/O except [`seal::pin_store::write_atomic`] and
//! [`seal::pin_store::read_file`]; everything else is pure state so that the
//! desktop (E3a/E3b) and the phone (E4/E5a) drive it from their own event loops.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod channel;
#[allow(missing_docs)]
pub mod codec;
mod ids;
pub mod noise;
mod rng;
pub mod seal;

pub use ids::{ChannelId, Id, IdParseError};
pub use rng::{fill_random, RandomError};
/// Re-exported so users of this crate's API (which takes and returns
/// `Zeroizing` buffers) use the exact §3.2 pin without their own dependency.
pub use zeroize;

/// Version byte of every binary format this crate defines (§4.1 header
/// `version`, §5 prologue prefix `dragent-e2ee/1`).
pub const PROTOCOL_VERSION: u8 = 1;
