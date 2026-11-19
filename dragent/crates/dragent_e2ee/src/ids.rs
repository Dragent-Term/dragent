//! Account, device and channel identifiers.
//!
//! §5: "ids are lowercase canonical UUIDs; hex is lowercase". An [`Id`] is the
//! 16 raw bytes of a UUID (accounts, phones, desktops); its text form is always
//! the canonical lowercase hyphenated form, and only that form is accepted when
//! parsing, so one id has exactly one prologue spelling. A [`ChannelId`] is a
//! 128-bit random value chosen by the initiator (§4.1), written as 32 lowercase
//! hex digits in prologues.

use std::fmt;
use std::str::FromStr;

use crate::rng::{fill_random, RandomError};

/// A UUID-shaped identifier: account, phone or desktop id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub [u8; 16]);

/// A string was not a canonical lowercase hyphenated UUID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdParseError;

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a canonical lowercase UUID")
    }
}

impl std::error::Error for IdParseError {}

const HYPHENS: [usize; 4] = [8, 13, 18, 23];

impl Id {
    /// The raw bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parse the canonical form `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
    /// (lowercase hex only).
    pub fn parse(s: &str) -> Result<Self, IdParseError> {
        let b = s.as_bytes();
        if b.len() != 36 {
            return Err(IdParseError);
        }
        let mut out = [0u8; 16];
        let mut nibbles = 0usize;
        for (i, &c) in b.iter().enumerate() {
            if HYPHENS.contains(&i) {
                if c != b'-' {
                    return Err(IdParseError);
                }
                continue;
            }
            let v = lower_hex_value(c).ok_or(IdParseError)?;
            out[nibbles / 2] |= if nibbles.is_multiple_of(2) { v << 4 } else { v };
            nibbles += 1;
        }
        Ok(Id(out))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let h = hex_lower(&self.0);
        write!(
            f,
            "{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        )
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({self})")
    }
}

impl FromStr for Id {
    type Err = IdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Id::parse(s)
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Id::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// A channel id: 128 random bits chosen by the initiator (§4.1 `chan`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub [u8; 16]);

impl ChannelId {
    /// A fresh random channel id.
    pub fn random() -> Result<Self, RandomError> {
        let mut b = [0u8; 16];
        fill_random(&mut b)?;
        Ok(ChannelId(b))
    }

    /// The raw bytes, as carried in the frame header.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// 32 lowercase hex digits, as used in prologues (§5).
    pub fn hex(&self) -> String {
        hex_lower(&self.0)
    }
}

impl fmt::Debug for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChannelId({})", self.hex())
    }
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn hex_decode_lower(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(2) {
        return None;
    }
    b.chunks(2)
        .map(|p| Some((lower_hex_value(p[0])? << 4) | lower_hex_value(p[1])?))
        .collect()
}

fn lower_hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trips_canonical_form() {
        let s = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let id = Id::parse(s).unwrap();
        assert_eq!(id.to_string(), s);
        assert_eq!(id.0[0], 0x0f);
        assert_eq!(id.0[15], 0x0e);
    }

    #[test]
    fn id_rejects_non_canonical_forms() {
        for bad in [
            "0F8FAD5B-D9CB-469F-A165-70867728950E",
            "0f8fad5bd9cb469fa16570867728950e",
            "{0f8fad5b-d9cb-469f-a165-70867728950e}",
            "0f8fad5b-d9cb-469f-a165-70867728950",
            "0f8fad5b-d9cb-469f-a165_70867728950e",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
        ] {
            assert_eq!(Id::parse(bad), Err(IdParseError), "{bad}");
        }
    }

    #[test]
    fn id_serde_is_the_canonical_string() {
        let id = Id::parse("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
        let j = serde_json::to_string(&id).unwrap();
        assert_eq!(j, "\"0f8fad5b-d9cb-469f-a165-70867728950e\"");
        assert_eq!(serde_json::from_str::<Id>(&j).unwrap(), id);
        assert!(serde_json::from_str::<Id>("\"0F8FAD5B-D9CB-469F-A165-70867728950E\"").is_err());
    }

    #[test]
    fn channel_hex_is_lowercase_32_digits() {
        let c = ChannelId([0xAB; 16]);
        assert_eq!(c.hex(), "ab".repeat(16));
        assert_ne!(ChannelId::random().unwrap(), ChannelId::random().unwrap());
    }

    #[test]
    fn hex_decode_rejects_uppercase_and_odd() {
        assert_eq!(hex_decode_lower("00ff"), Some(vec![0, 255]));
        assert_eq!(hex_decode_lower("00FF"), None);
        assert_eq!(hex_decode_lower("0"), None);
    }
}
