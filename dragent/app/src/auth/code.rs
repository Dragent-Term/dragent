//! The short code shown while the app waits for a browser sign-in.
//!
//! The browser shows the same code (derived from the same state) before the
//! person approves handing the session to the app, so a match means they're
//! approving this app, not a sign-in someone else started and sent them.
//! Must match auth-portal/src/lib/signInCode.ts.

use sha2::{Digest, Sha256};

const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// `XXX-XXX` for a sign-in state.
pub fn sign_in_code(state: &str) -> String {
    let digest = Sha256::digest(state.as_bytes());
    let chars: String = digest[..6]
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..3], &chars[3..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_and_stability() {
        let code = sign_in_code("2f1c7a9e-5b8d-4c3e-9a10-7d6e5f4c3b2a");
        assert_eq!(code.len(), 7);
        assert_eq!(&code[3..4], "-");
        assert!(code
            .chars()
            .filter(|c| *c != '-')
            .all(|c| ALPHABET.contains(&(c as u8))));
        // The same value the portal computes (signInCode.ts) for this state.
        assert_eq!(code, "U9Z-7FX");
        assert_ne!(code, sign_in_code("another-state"));
    }
}
