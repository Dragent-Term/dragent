//! §5 prologues, exact. `|` is literal; ids are lowercase canonical UUIDs; hex
//! is lowercase.
//!
//! ```text
//! KK       "dragent-e2ee/1|kk|"      account "|" phone   "|" desktop "|" hex(chan)
//! pairing  "dragent-e2ee/1|pair|"    account "|" phone   "|" desktop "|" hex(chan)
//! catalog  "dragent-e2ee/1|catalog|" account "|" desktop "|" phone   "|" version
//! push     "dragent-e2ee/1|push|"    account "|" phone
//! ```
//!
//! `version` is the catalog version in decimal with no leading zeros.

use crate::ids::{ChannelId, Id};

/// The live companion channel (Noise KK).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KkPrologue {
    /// The account both devices belong to.
    pub account: Id,
    /// The phone (always the KK initiator).
    pub phone: Id,
    /// The desktop (always the KK responder).
    pub desktop: Id,
    /// The channel id from the frame header.
    pub chan: ChannelId,
}

impl KkPrologue {
    /// The exact prologue bytes.
    pub fn bytes(&self) -> Vec<u8> {
        format!(
            "dragent-e2ee/1|kk|{}|{}|{}|{}",
            self.account,
            self.phone,
            self.desktop,
            self.chan.hex()
        )
        .into_bytes()
    }
}

/// Pairing (Noise IKpsk1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairingPrologue {
    /// The account both devices belong to.
    pub account: Id,
    /// The phone (pairing initiator).
    pub phone: Id,
    /// The desktop (pairing responder).
    pub desktop: Id,
    /// The channel id from the frame header.
    pub chan: ChannelId,
}

impl PairingPrologue {
    /// The exact prologue bytes.
    pub fn bytes(&self) -> Vec<u8> {
        format!(
            "dragent-e2ee/1|pair|{}|{}|{}|{}",
            self.account,
            self.phone,
            self.desktop,
            self.chan.hex()
        )
        .into_bytes()
    }
}

/// The catalog key wrap (Noise X), one per recipient phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogPrologue {
    /// The account.
    pub account: Id,
    /// The sealing desktop.
    pub desktop: Id,
    /// The recipient phone.
    pub phone: Id,
    /// The catalog version being sealed.
    pub version: u64,
}

impl CatalogPrologue {
    /// The exact prologue bytes.
    pub fn bytes(&self) -> Vec<u8> {
        format!(
            "dragent-e2ee/1|catalog|{}|{}|{}|{}",
            self.account, self.desktop, self.phone, self.version
        )
        .into_bytes()
    }
}

/// A push notification seal (Noise X). The sender desktop is deliberately not
/// in the prologue (§8): the recipient learns it from the Noise static key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushPrologue {
    /// The account.
    pub account: Id,
    /// The recipient phone.
    pub phone: Id,
}

impl PushPrologue {
    /// The exact prologue bytes.
    pub fn bytes(&self) -> Vec<u8> {
        format!("dragent-e2ee/1|push|{}|{}", self.account, self.phone).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Id {
        Id::parse(s).unwrap()
    }

    const ACCOUNT: &str = "11111111-2222-4333-8444-555555555555";
    const PHONE: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const DESKTOP: &str = "01234567-89ab-4cde-8f01-23456789abcd";

    #[test]
    fn prologues_are_exact() {
        let chan = ChannelId([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        let (a, p, d) = (id(ACCOUNT), id(PHONE), id(DESKTOP));
        assert_eq!(
            KkPrologue { account: a, phone: p, desktop: d, chan }.bytes(),
            b"dragent-e2ee/1|kk|11111111-2222-4333-8444-555555555555|aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee|01234567-89ab-4cde-8f01-23456789abcd|00112233445566778899aabbccddeeff"
        );
        assert_eq!(
            PairingPrologue { account: a, phone: p, desktop: d, chan }.bytes(),
            b"dragent-e2ee/1|pair|11111111-2222-4333-8444-555555555555|aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee|01234567-89ab-4cde-8f01-23456789abcd|00112233445566778899aabbccddeeff"
        );
        assert_eq!(
            CatalogPrologue { account: a, desktop: d, phone: p, version: 9 }.bytes(),
            b"dragent-e2ee/1|catalog|11111111-2222-4333-8444-555555555555|01234567-89ab-4cde-8f01-23456789abcd|aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee|9"
        );
        assert_eq!(
            PushPrologue { account: a, phone: p }.bytes(),
            b"dragent-e2ee/1|push|11111111-2222-4333-8444-555555555555|aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
    }
}
