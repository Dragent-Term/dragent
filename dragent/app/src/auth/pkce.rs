//! PKCE (RFC 7636, S256) for desktop sign-in.
//!
//! The desktop keeps a secret verifier and sends only its SHA-256 (the
//! challenge) when it opens the browser. The server then puts a one-time
//! code in the `dragent://` link instead of a refresh token, and the code is
//! only redeemable with the verifier. Another app that registers the scheme
//! and reads the link gets nothing it can use.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// A fresh verifier: 32 random bytes, base64url (43 characters).
pub fn new_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The S256 challenge for a verifier.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_7636_example() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifiers_are_long_and_fresh() {
        let a = new_verifier();
        assert_eq!(a.len(), 43);
        assert_ne!(a, new_verifier());
    }
}
