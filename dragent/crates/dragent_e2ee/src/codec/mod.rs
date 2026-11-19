//! Wire codecs: the §4.1 frame header, §4.4 records, TERM, the §4.5 part
//! protocol, and the §6.5 catalog and §8 push blobs.
//!
//! The design fixes every security-relevant rule; where it leaves an encoding
//! choice open (CLOSE reason bytes, PAD_ONLY semantics, `msg_id` 0, the JSON
//! base64 alphabet and catalog padding, handshake payload limits), the choice
//! is documented on the item as "chosen here" so the Go relay (E2a), the
//! desktop (E3a–E3c, E9a) and the phone (E4, E5a) implement the same bytes.
//!
//! All decoders are total on untrusted input: arbitrary bytes yield `Err`, never
//! a panic. `fuzz/` covers every parser; `vectors/codec/*.json` pins the wire.

pub mod b64;
pub mod catalog_blob;
pub mod ctrl;
pub mod header;
pub mod hs;
pub mod part;
pub mod push_plain;
pub mod record;
pub mod term;

/// Codec vector runner: tests, and the opt-in `vectors` feature (never in
/// release builds; the phone module's simulator tests only).
#[cfg(any(test, feature = "vectors"))]
pub mod vectors;
