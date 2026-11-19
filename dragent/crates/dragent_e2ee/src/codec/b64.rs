//! Base64url without padding, hand-rolled so the codec needs no new
//! dependencies (`Cargo.toml`/`Cargo.lock` are owned by E1a and frozen).
//!
//! Convention chosen here (the design writes `"<b64>"` without naming an
//! alphabet): every E2EE JSON field — catalog blob parts (§6.5), part chunks
//! (§4.5) and handshake payload keys (§6.2) — uses base64url, no padding, and
//! decoders reject anything else, including `+`, `/`, `=` and whitespace. The
//! sealed push blob (§8: "1,494 base64url characters" for 1,120 bytes, which
//! only holds without padding) uses the same alphabet.

/// Encode bytes as base64url without padding.
pub fn encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let mut n: u32 = 0;
        for (i, &b) in chunk.iter().enumerate() {
            n |= (b as u32) << (16 - 8 * i);
        }
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 63) as usize] as char);
        }
    }
    out
}

/// What made a base64url string invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Wrong length (a trailing group of 1 character can never occur).
    BadLength,
    /// A byte outside the base64url alphabet (includes `+`, `/`, `=`).
    BadChar,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadLength => f.write_str("bad base64url length"),
            DecodeError::BadChar => f.write_str("bad base64url character"),
        }
    }
}

impl std::error::Error for DecodeError {}

fn value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// Decode base64url without padding. Rejects padded, standard-alphabet and
/// non-canonical inputs (a trailing group encodes exactly the bits it needs;
///
/// extra nonzero trailing bits are rejected).
pub fn decode(s: &str) -> Result<Vec<u8>, DecodeError> {
    let b = s.as_bytes();
    if b.len() % 4 == 1 {
        return Err(DecodeError::BadLength);
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3 + 3);
    for chunk in b.chunks(4) {
        let mut n: u32 = 0;
        for &c in chunk {
            n = (n << 6) | value(c).ok_or(DecodeError::BadChar)?;
        }
        match chunk.len() {
            4 => {
                out.push((n >> 16) as u8);
                out.push((n >> 8) as u8);
                out.push(n as u8);
            }
            3 => {
                if n & 3 != 0 {
                    return Err(DecodeError::BadChar);
                }
                out.push((n >> 10) as u8);
                out.push((n >> 2) as u8);
            }
            2 => {
                if n & 15 != 0 {
                    return Err(DecodeError::BadChar);
                }
                out.push((n >> 4) as u8);
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // RFC 4648 §10 test vectors, unpadded base64url form.
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(raw.as_bytes()), enc, "{raw:?}");
            assert_eq!(decode(enc).unwrap(), raw.as_bytes(), "{enc:?}");
        }
        // Bytes that differ between standard and URL alphabets.
        assert_eq!(encode(&[0xfb, 0xff]), "-_8");
        assert_eq!(decode("-_8").unwrap(), vec![0xfb, 0xff]);
    }

    #[test]
    fn rejects_non_canonical_inputs() {
        for bad in ["Zg==", "Zm9v\n", "Zm9v ", "+/==", "Zm+9", "Zm/9", "Zg=", "Z", "Zh=="] {
            assert!(decode(bad).is_err(), "{bad:?}");
        }
        // Nonzero trailing bits: "Zh" is 0x66 0x21; low 4 bits must be zero.
        assert!(decode("Zh").is_err());
        assert_eq!(decode("Zg").unwrap(), b"f");
    }

    #[test]
    fn round_trip_all_lengths() {
        for len in 0..70usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&data)).unwrap(), data, "len {len}");
            // No padding emitted, and the sealed-push size holds:
            // 1120 bytes -> 1494 chars.
            assert!(!encode(&data).contains('='));
        }
        assert_eq!(encode(&vec![0u8; 1120]).len(), 1494);
    }
}
