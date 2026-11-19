//! Execution grant verification (M3, shared core).
//!
//! Grants are JWTs (EdDSA/Ed25519) issued by `POST /v1/execution/authorize`.
//! The local engine verifies them OFFLINE against a cached public key: no
//! database access from local processes, no per-turn authorization round
//! trips. Revocation is via short TTL (24h), not live checks.
//!
//! Everything here is pure crypto + serde (no HTTP, no storage) so both the
//! desktop and the future host reuse it unchanged. Fetching, caching, and
//! enforcement policy live app-side.

use serde::{Deserialize, Serialize};

/// Grant lifetime issued by the server. Clients must tolerate skew, but a
/// grant older than this plus the grace window is dead.
pub const GRANT_TTL_SECS: i64 = 24 * 3600;

/// Grace window after expiry during which a cached grant still authorizes
/// execution while refresh is retried. Bounded authorization window in
/// exchange for local availability during cloud outages.
pub const GRANT_GRACE_SECS: i64 = 12 * 3600;

/// Capabilities a grant may carry. Mirrors the server constants.
pub mod capability {
    pub const AGENT: &str = "agent";
    pub const BYOK: &str = "byok";
    pub const MANAGED: &str = "managed";
}

/// Decoded grant claims (verified).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrantClaims {
    pub sub: String,
    pub exp: i64,
    pub iat: i64,
    pub plan: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub grant_id: String,
}

impl GrantClaims {
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

/// Grant verification failure modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantError {
    /// Malformed token (structure, encoding, JSON).
    Malformed(String),
    /// Signature does not verify against the known public key.
    BadSignature,
    /// Wrong algorithm (only EdDSA accepted — algorithm confusion rejected).
    BadAlgorithm(String),
    /// Expired beyond the grace window (carries seconds past expiry for logs).
    Expired(i64),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(s) => write!(f, "malformed grant: {s}"),
            Self::BadSignature => write!(f, "grant signature invalid"),
            Self::BadAlgorithm(a) => write!(f, "grant algorithm rejected: {a}"),
            Self::Expired(s) => write!(f, "grant expired {s}s ago"),
        }
    }
}

fn b64url_decode(input: &str) -> Result<Vec<u8>, GrantError> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| GrantError::Malformed(format!("base64: {e}")))
}

/// Verify a compact JWT grant against a raw 32-byte Ed25519 public key.
/// Returns the claims on success. `now_secs` is injected (Unix seconds) so
/// expiry is unit-testable without clock access.
pub fn verify_grant(
    token: &str,
    public_key: &[u8; 32],
    now_secs: i64,
) -> Result<GrantClaims, GrantError> {
    let mut parts = token.split('.');
    let (header_b64, payload_b64, sig_b64) =
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(h), Some(p), Some(s), None) => (h, p, s),
            _ => return Err(GrantError::Malformed("expected 3 segments".into())),
        };
    let header: serde_json::Value = serde_json::from_slice(&b64url_decode(header_b64)?)
        .map_err(|e| GrantError::Malformed(format!("header: {e}")))?;
    match header.get("alg").and_then(|v| v.as_str()) {
        Some("EdDSA") => {}
        other => {
            return Err(GrantError::BadAlgorithm(
                other.unwrap_or("(missing)").to_string(),
            ));
        }
    }
    let payload = b64url_decode(payload_b64)?;
    let claims: GrantClaims = serde_json::from_slice(&payload)
        .map_err(|e| GrantError::Malformed(format!("claims: {e}")))?;
    let signature = b64url_decode(sig_b64)?;
    let signature: &[u8; 64] = signature
        .as_slice()
        .try_into()
        .map_err(|_| GrantError::Malformed("signature must be 64 bytes".into()))?;
    let message = format!("{header_b64}.{payload_b64}");
    let verifying = ed25519_dalek::VerifyingKey::from_bytes(public_key)
        .map_err(|e| GrantError::Malformed(format!("public key: {e}")))?;
    verifying
        .verify_strict(
            message.as_bytes(),
            &ed25519_dalek::Signature::from_bytes(signature),
        )
        .map_err(|_| GrantError::BadSignature)?;
    // Expiry with grace: fully dead only past exp + grace.
    let age_past_expiry = now_secs - claims.exp;
    if age_past_expiry > GRANT_GRACE_SECS {
        return Err(GrantError::Expired(age_past_expiry));
    }
    Ok(claims)
}

/// True when the grant needs a refresh attempt (expired, or expiring within
/// a quarter of its TTL). Separate from validity: callers refresh
/// opportunistically but keep running on the cached grant inside grace.
pub fn needs_refresh(claims: &GrantClaims, now_secs: i64) -> bool {
    now_secs >= claims.exp - GRANT_TTL_SECS / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-only signer (deterministic key, no RNG needed).
    fn test_keypair() -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let secret = [7u8; 32];
        let signing = ed25519_dalek::SigningKey::from_bytes(&secret);
        let verifying = signing.verifying_key();
        (signing, verifying.to_bytes())
    }

    fn mint(signing: &ed25519_dalek::SigningKey, exp: i64, capabilities: &[&str]) -> String {
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"EdDSA","typ":"JWT"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "sub": "user-1",
                "exp": exp,
                "iat": exp - GRANT_TTL_SECS,
                "plan": "pro",
                "capabilities": capabilities,
                "device": "dev-1",
                "grant_id": "g-1",
            })
            .to_string(),
        );
        let message = format!("{header}.{payload}");
        let sig = signing.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes());
        format!("{message}.{sig_b64}")
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn valid_grant_verifies_with_capabilities() {
        let (signing, pubkey) = test_keypair();
        let token = mint(&signing, NOW + GRANT_TTL_SECS, &["agent", "byok"]);
        let claims = verify_grant(&token, &pubkey, NOW).expect("valid");
        assert_eq!(claims.sub, "user-1");
        assert!(claims.has_capability("agent"));
        assert!(claims.has_capability("byok"));
        assert!(!claims.has_capability("managed"));
        assert!(!needs_refresh(&claims, NOW));
    }

    #[test]
    fn tampered_payload_rejected() {
        use base64::Engine as _;
        let (signing, pubkey) = test_keypair();
        let token = mint(&signing, NOW + GRANT_TTL_SECS, &["agent"]);
        // Same shape, different subject: parses fine, must fail signature.
        let forged_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({
                "sub": "intruder",
                "exp": NOW + GRANT_TTL_SECS,
                "iat": NOW,
                "plan": "pro",
                "capabilities": ["agent", "byok", "managed"],
                "device": "dev-1",
                "grant_id": "g-1",
            })
            .to_string(),
        );
        let mut parts: Vec<&str> = token.split('.').collect();
        parts[1] = &forged_payload;
        let tampered = parts.join(".");
        assert_eq!(
            verify_grant(&tampered, &pubkey, NOW),
            Err(GrantError::BadSignature)
        );
    }

    #[test]
    fn wrong_algorithm_rejected() {
        let (signing, pubkey) = test_keypair();
        let token = mint(&signing, NOW + 3600, &["agent"]);
        // Rewrite header alg to HS256, re-sign so only the alg check trips.
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = token.split('.').nth(1).unwrap().to_string();
        let message = format!("{header}.{payload}");
        let sig = signing.sign(message.as_bytes());
        let forged = format!(
            "{message}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig.to_bytes())
        );
        assert_eq!(
            verify_grant(&forged, &pubkey, NOW),
            Err(GrantError::BadAlgorithm("HS256".into()))
        );
    }

    #[test]
    fn expiry_grace_and_refresh() {
        let (signing, pubkey) = test_keypair();
        // Fresh: no refresh needed.
        let fresh = mint(&signing, NOW + GRANT_TTL_SECS, &["agent"]);
        let claims = verify_grant(&fresh, &pubkey, NOW).unwrap();
        assert!(!needs_refresh(&claims, NOW));
        // Near expiry: refresh opportunistic, still valid.
        let claims = verify_grant(&fresh, &pubkey, NOW + GRANT_TTL_SECS - 10).unwrap();
        assert!(needs_refresh(&claims, NOW + GRANT_TTL_SECS - 10));
        // Just expired: inside grace, still authorizes.
        let claims = verify_grant(&fresh, &pubkey, NOW + GRANT_TTL_SECS + 100).unwrap();
        assert!(needs_refresh(&claims, NOW + GRANT_TTL_SECS + 100));
        // Past grace: dead.
        assert_eq!(
            verify_grant(&fresh, &pubkey, NOW + GRANT_TTL_SECS + GRANT_GRACE_SECS + 1),
            Err(GrantError::Expired(GRANT_GRACE_SECS + 1))
        );
    }

    #[test]
    fn malformed_inputs_rejected() {
        let (_, pubkey) = test_keypair();
        assert!(matches!(
            verify_grant("not-a-jwt", &pubkey, NOW),
            Err(GrantError::Malformed(_))
        ));
        assert!(matches!(
            verify_grant("a.b", &pubkey, NOW),
            Err(GrantError::Malformed(_))
        ));
        assert!(matches!(
            verify_grant("a.b.c.d", &pubkey, NOW),
            Err(GrantError::Malformed(_))
        ));
    }
}
