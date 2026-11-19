//! Data at rest and one-way seals.
//!
//! - [`at_rest`]: XChaCha20-Poly1305 with random 24-byte nonces (§3.1).
//! - [`pin_store`]: the sealed pin store (§6.1).
//! - [`catalog`]: the catalog body AEAD and the per-phone Noise X key wrap (§6.5).
//! - [`push`]: the fixed-size Noise X push seal (§8).

pub mod at_rest;
pub mod catalog;
pub mod pin_store;
pub mod push;
