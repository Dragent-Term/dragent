//! §6.5 catalog blob: the JSON layout and padding (the crypto half is
//! [`crate::seal::catalog`]).
//!
//! ```json
//! {"v":1,"desktop":"<id>","version":N,"recipients":[{"phone":"<id>","wrap":"<b64>"}],"nonce":"<b64>","body":"<b64>"}
//! ```
//!
//! The blob is padded to 4 KiB buckets. Choices made here:
//! - Padding is trailing ASCII spaces (`0x20`) to the next multiple of 4,096
//!   bytes (an exact multiple is left alone). Spaces are valid JSON whitespace,
//!   so any JSON parser reads the padded blob; the length check below pins the
//!   bucketing for the relay's size bucketing and the Go store (E2b).
//! - The encoded blob is at most 4 MiB (1,024 buckets); anything larger is a
//!   sender bug and is rejected rather than buffered.
//! - Field order is fixed as above (serde declaration order), so the padded
//!   bytes are byte-identical for identical inputs.
//! - A phone id appears at most once in `recipients` (duplicates rejected).
//!   Empty recipients are allowed: after revoking every phone the desktop
//!   still publishes a sealed, empty catalog.
//! - Every wrap must decode to exactly [`WRAP_LEN`] bytes (the Noise X seal of
//!   `K || version`); the nonce to exactly 24 bytes. Shorter/longer means a
//!   forged or corrupt blob, rejected before any crypto runs.
//! - Base64 is base64url without padding (see [`crate::codec::b64`]).
//!
//! [`WRAP_LEN`]: crate::seal::catalog::WRAP_LEN

use crate::codec::b64;
use crate::ids::Id;
use crate::seal::at_rest::NONCE_LEN;
use crate::seal::catalog::{MAX_VERSION, WRAP_LEN};

/// Padding bucket in bytes (§6.5: padded to 4 KiB buckets).
pub const PAD_BUCKET: usize = 4096;
/// Largest encoded blob: 1,024 buckets (chosen here; see module docs).
pub const MAX_BLOB_LEN: usize = 1024 * PAD_BUCKET;

/// One recipient's sealed key wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    /// The recipient phone.
    pub phone: Id,
    /// The Noise X seal of `K || version` ([`WRAP_LEN`] bytes).
    pub wrap: Vec<u8>,
}

/// A decoded catalog blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogBlob {
    /// The sealing desktop.
    pub desktop: Id,
    /// Monotonic version (1..= 2^53 − 1, never reused).
    pub version: u64,
    /// One wrap per paired phone.
    pub recipients: Vec<Recipient>,
    /// The body nonce (24 bytes).
    pub nonce: [u8; 24],
    /// The XChaCha20-Poly1305 catalog ciphertext.
    pub body: Vec<u8>,
}

/// Why a catalog blob was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogBlobError {
    /// The bytes are not the expected JSON shape (or not JSON at all).
    NotJson,
    /// `v` is not 1.
    BadVersion,
    /// `version` is 0 or above 2^53 − 1.
    BadCatalogVersion {
        /// The version seen.
        version: u64,
    },
    /// The encoded length is not a multiple of 4,096, or is over 4 MiB.
    BadPadding {
        /// The length seen.
        len: usize,
    },
    /// A `wrap` is not valid base64url or not [`WRAP_LEN`] bytes.
    BadWrap,
    /// The `nonce` is not valid base64url or not 24 bytes.
    BadNonce,
    /// The `body` is not valid base64url.
    BadBody,
    /// The same phone appears twice.
    DuplicateRecipient {
        /// The phone id.
        phone: Id,
    },
}

impl std::fmt::Display for CatalogBlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogBlobError::NotJson => f.write_str("catalog blob is not the expected JSON"),
            CatalogBlobError::BadVersion => f.write_str("catalog blob v is not 1"),
            CatalogBlobError::BadCatalogVersion { version } => {
                write!(f, "catalog version {version} outside 1..=2^53-1")
            }
            CatalogBlobError::BadPadding { len } => {
                write!(f, "catalog blob length {len} is not a 4 KiB bucket")
            }
            CatalogBlobError::BadWrap => f.write_str("catalog wrap is malformed"),
            CatalogBlobError::BadNonce => f.write_str("catalog nonce is malformed"),
            CatalogBlobError::BadBody => f.write_str("catalog body is malformed"),
            CatalogBlobError::DuplicateRecipient { phone } => {
                write!(f, "duplicate catalog recipient {phone}")
            }
        }
    }
}

impl std::error::Error for CatalogBlobError {}

fn check_version(version: u64) -> Result<(), CatalogBlobError> {
    if version == 0 || version > MAX_VERSION {
        return Err(CatalogBlobError::BadCatalogVersion { version });
    }
    Ok(())
}

impl CatalogBlob {
    /// Encode the blob: fixed field order, then trailing spaces to the next
    /// 4 KiB bucket.
    pub fn encode(&self) -> Result<Vec<u8>, CatalogBlobError> {
        check_version(self.version)?;
        let mut seen = std::collections::HashSet::new();
        for r in &self.recipients {
            if r.wrap.len() != WRAP_LEN {
                return Err(CatalogBlobError::BadWrap);
            }
            if !seen.insert(r.phone) {
                return Err(CatalogBlobError::DuplicateRecipient { phone: r.phone });
            }
        }
        let mut json = format!(
            "{{\"v\":1,\"desktop\":\"{}\",\"version\":{},\"recipients\":[",
            self.desktop, self.version
        );
        for (i, r) in self.recipients.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "{{\"phone\":\"{}\",\"wrap\":\"{}\"}}",
                r.phone,
                b64::encode(&r.wrap)
            ));
        }
        json.push_str(&format!(
            "],\"nonce\":\"{}\",\"body\":\"{}\"}}",
            b64::encode(&self.nonce),
            b64::encode(&self.body)
        ));
        let mut out = json.into_bytes();
        if out.len() > MAX_BLOB_LEN {
            return Err(CatalogBlobError::BadPadding { len: out.len() });
        }
        let padded = out.len().div_ceil(PAD_BUCKET) * PAD_BUCKET;
        out.resize(padded, b' ');
        debug_assert_eq!(out.len() % PAD_BUCKET, 0);
        Ok(out)
    }
}

/// Decode and validate a catalog blob (trailing space padding allowed).
pub fn decode(bytes: &[u8]) -> Result<CatalogBlob, CatalogBlobError> {
    if bytes.is_empty() || bytes.len() > MAX_BLOB_LEN || !bytes.len().is_multiple_of(PAD_BUCKET) {
        return Err(CatalogBlobError::BadPadding { len: bytes.len() });
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| CatalogBlobError::NotJson)?;
    let o = v.as_object().ok_or(CatalogBlobError::NotJson)?;
    if o.len() != 6
        || o.get("v").and_then(|v| v.as_u64()) != Some(1)
        || !o.contains_key("desktop")
        || !o.contains_key("version")
        || !o.contains_key("recipients")
        || !o.contains_key("nonce")
        || !o.contains_key("body")
    {
        if o.get("v").and_then(|v| v.as_u64()) != Some(1) {
            return Err(CatalogBlobError::BadVersion);
        }
        return Err(CatalogBlobError::NotJson);
    }
    let desktop: Id = serde_json::from_value(o["desktop"].clone()).map_err(|_| CatalogBlobError::NotJson)?;
    let version = o["version"].as_u64().ok_or(CatalogBlobError::NotJson)?;
    check_version(version)?;
    let recipients_json = o["recipients"].as_array().ok_or(CatalogBlobError::NotJson)?;
    let mut recipients = Vec::with_capacity(recipients_json.len());
    let mut seen = std::collections::HashSet::new();
    for r in recipients_json {
        let ro = r.as_object().ok_or(CatalogBlobError::NotJson)?;
        if ro.len() != 2 || !ro.contains_key("phone") || !ro.contains_key("wrap") {
            return Err(CatalogBlobError::NotJson);
        }
        let phone: Id =
            serde_json::from_value(ro["phone"].clone()).map_err(|_| CatalogBlobError::NotJson)?;
        let wrap_b64 = ro["wrap"].as_str().ok_or(CatalogBlobError::NotJson)?;
        let wrap = b64::decode(wrap_b64).map_err(|_| CatalogBlobError::BadWrap)?;
        if wrap.len() != WRAP_LEN {
            return Err(CatalogBlobError::BadWrap);
        }
        if !seen.insert(phone) {
            return Err(CatalogBlobError::DuplicateRecipient { phone });
        }
        recipients.push(Recipient { phone, wrap });
    }
    let nonce_b64 = o["nonce"].as_str().ok_or(CatalogBlobError::NotJson)?;
    let nonce = b64::decode(nonce_b64).map_err(|_| CatalogBlobError::BadNonce)?;
    if nonce.len() != NONCE_LEN {
        return Err(CatalogBlobError::BadNonce);
    }
    let mut nonce_arr = [0u8; NONCE_LEN];
    nonce_arr.copy_from_slice(&nonce);
    let body_b64 = o["body"].as_str().ok_or(CatalogBlobError::NotJson)?;
    let body = b64::decode(body_b64).map_err(|_| CatalogBlobError::BadBody)?;
    Ok(CatalogBlob { desktop, version, recipients, nonce: nonce_arr, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_fixture() -> CatalogBlob {
        CatalogBlob {
            desktop: Id::parse("01234567-89ab-4cde-8f01-23456789abcd").unwrap(),
            version: 9,
            recipients: vec![Recipient {
                phone: Id::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
                wrap: vec![3u8; WRAP_LEN],
            }],
            nonce: [5u8; 24],
            body: b"{\"projects\":[]}".to_vec(),
        }
    }

    #[test]
    fn round_trip_and_bucketing() {
        let enc = blob_fixture().encode().unwrap();
        assert_eq!(enc.len(), PAD_BUCKET);
        // Trailing spaces after the JSON `}`.
        let json_end = enc.iter().rposition(|&c| c != b' ').unwrap();
        assert_eq!(enc[json_end], b'}');
        assert!(enc[json_end + 1..].iter().all(|&c| c == b' '));
        // The JSON prefix is byte-identical for identical inputs.
        assert_eq!(enc, blob_fixture().encode().unwrap());
        let back = decode(&enc).unwrap();
        assert_eq!(back, blob_fixture());
        // Empty recipients still publish a padded blob.
        let mut empty = blob_fixture();
        empty.recipients.clear();
        let enc = empty.encode().unwrap();
        assert_eq!(enc.len(), PAD_BUCKET);
        assert_eq!(decode(&enc).unwrap(), empty);
    }

    #[test]
    fn rejections() {
        // Not a bucket multiple, or empty.
        assert_eq!(
            decode(b"{}").unwrap_err(),
            CatalogBlobError::BadPadding { len: 2 }
        );
        assert_eq!(decode(b"").unwrap_err(), CatalogBlobError::BadPadding { len: 0 });
        // Wrong v, bad version, duplicate recipient, bad wrap/nonce.
        let mut padded = vec![b' '; PAD_BUCKET];
        let raw = br#"{"v":2,"desktop":"01234567-89ab-4cde-8f01-23456789abcd","version":9,"recipients":[],"nonce":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","body":""}"#;
        padded[..raw.len()].copy_from_slice(raw);
        assert_eq!(decode(&padded).unwrap_err(), CatalogBlobError::BadVersion);
        let mut bad = blob_fixture();
        bad.version = 0;
        assert_eq!(
            bad.encode().unwrap_err(),
            CatalogBlobError::BadCatalogVersion { version: 0 }
        );
        let mut bad = blob_fixture();
        bad.recipients.push(bad.recipients[0].clone());
        assert!(matches!(
            bad.encode(),
            Err(CatalogBlobError::DuplicateRecipient { .. })
        ));
        let mut bad = blob_fixture();
        bad.recipients[0].wrap = vec![0u8; 10];
        assert_eq!(bad.encode().unwrap_err(), CatalogBlobError::BadWrap);
        // Trailing garbage inside the bucket is rejected, not ignored.
        let mut bad = blob_fixture().encode().unwrap();
        let last = bad.len() - 1;
        bad[last] = b'x';
        assert_eq!(decode(&bad).unwrap_err(), CatalogBlobError::NotJson);
        // Non-JSON in a bucket is rejected.
        let mut padded = vec![b' '; PAD_BUCKET];
        padded[..3].copy_from_slice(b"{{$");
        assert_eq!(decode(&padded).unwrap_err(), CatalogBlobError::NotJson);
        let mut bad = blob_fixture();
        bad.nonce = [0u8; 24];
        let mut enc = bad.encode().unwrap();
        // Corrupt the nonce's base64 with a `+` (standard alphabet, rejected).
        let tag = b"\"nonce\":\"";
        let nonce_at = enc
            .windows(tag.len())
            .position(|w| w == tag)
            .map(|p| p + tag.len())
            .unwrap();
        enc[nonce_at] = b'+';
        assert_eq!(decode(&enc).unwrap_err(), CatalogBlobError::BadNonce);
    }
}
