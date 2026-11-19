//! §4.1 frame header: the only bytes the relay parses on the sockets.
//!
//! ```text
//! offset size field
//! 0      1    version = 0x01
//! 1      1    kind: 0x01 HS, 0x02 DATA, 0x03 CLOSE
//! 2      1    step: HS → 1..3; else 0
//! 3      1    purpose: 0x01 companion (KK), 0x02 pairing (IKpsk1); else 0
//! 4      16   chan: channel id (128-bit random, initiator-chosen)
//! 20     16   peer: HS step 1 only (initiator socket → TARGET device id;
//!                   responder socket → relay-written INITIATOR device id); else zero
//! 36     8    ctr: DATA only, u64; else 0
//! 44     ...  body: Noise handshake message | Noise transport ciphertext
//!                   (incl. 16-byte tag) | CLOSE reason byte
//! ```
//!
//! The header is 44 bytes; the body is at most 65,535 bytes, so a frame is at
//! most 65,579 bytes. The relay validates lengths and enum values; the rules
//! below are exactly that check, so the Go relay (E2a) and this codec agree.
//! Anything stateful (HS1 target, `chan` uniqueness, channel/byte caps) is the
//! relay's, not the codec's.
//!
//! CLOSE reason bytes chosen here (the design names only `unknown_peer` and
//! `protocol_error`): `0x01 protocol_error`, `0x02 unknown_peer`. Decoders
//! preserve any other single byte as [`CloseReason::Unknown`] so a new reason
//! never breaks parsing; validators must reject unknown reasons explicitly.

use crate::ids::{ChannelId, Id};

/// Header length in bytes (§4.1: the header is 44 bytes).
pub const HEADER_LEN: usize = 44;
/// Largest body the relay accepts (§4.1: at most 65,535 bytes).
pub const MAX_BODY_LEN: usize = 65_535;
/// Largest whole frame: 44 + 65,535 = 65,579 (§4.5 cites this number).
pub const MAX_FRAME_LEN: usize = HEADER_LEN + MAX_BODY_LEN;

/// Frame kinds (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `0x01`: Noise handshake message.
    Hs,
    /// `0x02`: Noise transport ciphertext.
    Data,
    /// `0x03`: unauthenticated close hint (a forged CLOSE only reconnects).
    Close,
}

impl Kind {
    fn byte(self) -> u8 {
        match self {
            Kind::Hs => 0x01,
            Kind::Data => 0x02,
            Kind::Close => 0x03,
        }
    }

    fn parse(b: u8) -> Option<Kind> {
        match b {
            0x01 => Some(Kind::Hs),
            0x02 => Some(Kind::Data),
            0x03 => Some(Kind::Close),
            _ => None,
        }
    }
}

/// Channel purposes (§4.1): the live companion channel (KK) and pairing
/// (IKpsk1). Anything else is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// `0x01`: companion channel (KK).
    Companion,
    /// `0x02`: pairing (IKpsk1).
    Pairing,
}

impl Purpose {
    fn byte(self) -> u8 {
        match self {
            Purpose::Companion => 0x01,
            Purpose::Pairing => 0x02,
        }
    }

    fn parse(b: u8) -> Option<Purpose> {
        match b {
            0x01 => Some(Purpose::Companion),
            0x02 => Some(Purpose::Pairing),
            _ => None,
        }
    }
}

/// CLOSE reason byte. Authenticated reasons travel as CTRL `bye`; this byte is
/// an unauthenticated hint only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// `0x01`: §4.3 protocol error.
    ProtocolError,
    /// `0x02`: §4.2 responder refusal (HS1 failed or the peer is unpinned).
    UnknownPeer,
    /// Any other reason byte, preserved verbatim.
    Unknown(u8),
}

impl CloseReason {
    /// The wire byte.
    pub fn byte(self) -> u8 {
        match self {
            CloseReason::ProtocolError => 0x01,
            CloseReason::UnknownPeer => 0x02,
            CloseReason::Unknown(b) => b,
        }
    }

    /// Parse a wire byte. Unknown bytes are preserved, not rejected: call
    /// [`CloseReason::known`] when the relay's enum check needs enforcing.
    pub fn parse(b: u8) -> CloseReason {
        match b {
            0x01 => CloseReason::ProtocolError,
            0x02 => CloseReason::UnknownPeer,
            other => CloseReason::Unknown(other),
        }
    }

    /// `true` for the reasons this design defines.
    pub fn known(self) -> bool {
        !matches!(self, CloseReason::Unknown(_))
    }
}

/// A decoded §4.1 frame (header plus a borrowed body).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    /// HS, DATA or CLOSE.
    pub kind: Kind,
    /// HS handshake step (1..=3); 0 for DATA and CLOSE.
    pub step: u8,
    /// Companion or pairing for HS; `None` (wire 0) for DATA and CLOSE.
    pub purpose: Option<Purpose>,
    /// Initiator-chosen channel id.
    pub chan: ChannelId,
    /// HS step 1 only: the TARGET (initiator socket) or INITIATOR
    /// (responder socket, relay-written) device id; else `None` (wire zero).
    pub peer: Option<Id>,
    /// DATA only: the sender-assigned counter (the explicit nonce).
    pub ctr: u64,
    /// The body: handshake message, transport ciphertext, or CLOSE reason.
    pub body: &'a [u8],
}

/// An owned frame under construction (for sending).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head<'a> {
    /// HS, DATA or CLOSE.
    pub kind: Kind,
    /// HS handshake step (1..=3); 0 for DATA and CLOSE.
    pub step: u8,
    /// Companion or pairing for HS; `None` (wire 0) for DATA and CLOSE.
    pub purpose: Option<Purpose>,
    /// Initiator-chosen channel id.
    pub chan: ChannelId,
    /// HS step 1 only; else `None`.
    pub peer: Option<Id>,
    /// DATA only; else 0.
    pub ctr: u64,
    /// The body.
    pub body: &'a [u8],
}

impl<'a> Head<'a> {
    /// An HS step 1 frame: `peer` (the TARGET device id) is required.
    pub fn hs1(purpose: Purpose, chan: ChannelId, peer: Id, body: &'a [u8]) -> Head<'a> {
        Head {
            kind: Kind::Hs,
            step: 1,
            purpose: Some(purpose),
            chan,
            peer: Some(peer),
            ctr: 0,
            body,
        }
    }

    /// Encode the 44-byte header plus body into `out`.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), HeaderError> {
        self.check()?;
        out.reserve(HEADER_LEN + self.body.len());
        out.push(crate::PROTOCOL_VERSION);
        out.push(self.kind.byte());
        out.push(self.step);
        out.push(self.purpose.map_or(0, Purpose::byte));
        out.extend_from_slice(self.chan.as_bytes());
        match self.peer {
            Some(id) => out.extend_from_slice(id.as_bytes()),
            None => out.extend_from_slice(&[0u8; 16]),
        }
        out.extend_from_slice(&self.ctr.to_be_bytes());
        out.extend_from_slice(self.body);
        Ok(())
    }

    /// The relay's length-and-enum check, before routing.
    fn check(&self) -> Result<(), HeaderError> {
        if self.body.len() > MAX_BODY_LEN {
            return Err(HeaderError::BodyTooLong {
                len: self.body.len(),
            });
        }
        match self.kind {
            Kind::Hs => {
                if !(1..=3).contains(&self.step) {
                    return Err(HeaderError::BadStep { step: self.step });
                }
                if self.purpose.is_none() {
                    return Err(HeaderError::BadPurpose { purpose: 0 });
                }
                if self.ctr != 0 {
                    return Err(HeaderError::NonZeroCtr { ctr: self.ctr });
                }
                if self.step == 1 && self.peer.is_none() {
                    return Err(HeaderError::MissingPeer);
                }
                if self.step != 1 && self.peer.is_some() {
                    return Err(HeaderError::UnexpectedPeer);
                }
            }
            Kind::Data => {
                if self.step != 0 {
                    return Err(HeaderError::BadStep { step: self.step });
                }
                if self.purpose.is_some() {
                    return Err(HeaderError::BadPurpose {
                        purpose: self.purpose.map_or(0, Purpose::byte),
                    });
                }
                if self.peer.is_some() {
                    return Err(HeaderError::UnexpectedPeer);
                }
            }
            Kind::Close => {
                if self.step != 0 {
                    return Err(HeaderError::BadStep { step: self.step });
                }
                if self.purpose.is_some() {
                    return Err(HeaderError::BadPurpose {
                        purpose: self.purpose.map_or(0, Purpose::byte),
                    });
                }
                if self.peer.is_some() {
                    return Err(HeaderError::UnexpectedPeer);
                }
                if self.ctr != 0 {
                    return Err(HeaderError::NonZeroCtr { ctr: self.ctr });
                }
                if self.body.len() != 1 {
                    return Err(HeaderError::BadCloseBody {
                        len: self.body.len(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Why a frame was rejected by the length-and-enum check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderError {
    /// Fewer than 44 bytes.
    Truncated {
        /// Bytes available.
        len: usize,
    },
    /// `version != 0x01`.
    BadVersion {
        /// The version byte seen.
        version: u8,
    },
    /// Unknown `kind`.
    BadKind {
        /// The kind byte seen.
        kind: u8,
    },
    /// `step` outside its range for the kind (HS: 1..=3; else 0).
    BadStep {
        /// The step byte seen.
        step: u8,
    },
    /// Unknown `purpose`, or a nonzero purpose outside HS.
    BadPurpose {
        /// The purpose byte seen.
        purpose: u8,
    },
    /// HS step 1 without a peer id.
    MissingPeer,
    /// A peer id where the wire must be zero (HS step 2/3, DATA, CLOSE).
    UnexpectedPeer,
    /// A nonzero counter outside DATA.
    NonZeroCtr {
        /// The counter seen.
        ctr: u64,
    },
    /// A CLOSE body that is not exactly one reason byte.
    BadCloseBody {
        /// The body length seen.
        len: usize,
    },
    /// The body is over 65,535 bytes.
    BodyTooLong {
        /// The body length seen.
        len: usize,
    },
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::Truncated { len } => write!(f, "frame truncated at {len} bytes"),
            HeaderError::BadVersion { version } => write!(f, "bad version {version:#04x}"),
            HeaderError::BadKind { kind } => write!(f, "bad kind {kind:#04x}"),
            HeaderError::BadStep { step } => write!(f, "bad step {step}"),
            HeaderError::BadPurpose { purpose } => write!(f, "bad purpose {purpose:#04x}"),
            HeaderError::MissingPeer => f.write_str("HS step 1 without a peer id"),
            HeaderError::UnexpectedPeer => f.write_str("peer id where zero is required"),
            HeaderError::NonZeroCtr { ctr } => write!(f, "nonzero counter {ctr} outside DATA"),
            HeaderError::BadCloseBody { len } => {
                write!(f, "CLOSE body is {len} bytes, want 1")
            }
            HeaderError::BodyTooLong { len } => write!(f, "body too long: {len} bytes"),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Decode one §4.1 frame: the 44-byte header plus the body slice that follows
/// it in `bytes`. Returns the frame and the total frame length (44 + body).
/// The caller splits streams: `bytes` must hold exactly one frame's header
/// plus at least the body's bytes; trailing bytes belong to the next frame.
pub fn decode(bytes: &[u8]) -> Result<(Frame<'_>, usize), HeaderError> {
    decode_with_body(bytes, None)
}

/// Decode a header when the body arrives separately (the relay's read path):
/// `header` is exactly the 44 header bytes; `body_len` is the body's length,
/// checked against the 65,535-byte cap without needing the body in hand.
pub fn decode_header(header: &[u8], body_len: usize) -> Result<Frame<'_>, HeaderError> {
    if header.len() != HEADER_LEN {
        return Err(HeaderError::Truncated { len: header.len() });
    }
    decode_with_body(header, Some(body_len)).map(|(frame, _)| frame)
}

fn decode_with_body(bytes: &[u8], body_len: Option<usize>) -> Result<(Frame<'_>, usize), HeaderError> {
    if bytes.len() < HEADER_LEN {
        return Err(HeaderError::Truncated { len: bytes.len() });
    }
    let (header, rest) = bytes.split_at(HEADER_LEN);
    if header[0] != crate::PROTOCOL_VERSION {
        return Err(HeaderError::BadVersion {
            version: header[0],
        });
    }
    let kind = Kind::parse(header[1]).ok_or(HeaderError::BadKind { kind: header[1] })?;
    let step = header[2];
    let purpose_byte = header[3];
    let mut chan = [0u8; 16];
    chan.copy_from_slice(&header[4..20]);
    let mut peer_raw = [0u8; 16];
    peer_raw.copy_from_slice(&header[20..36]);
    let mut ctr_raw = [0u8; 8];
    ctr_raw.copy_from_slice(&header[36..44]);
    let ctr = u64::from_be_bytes(ctr_raw);

    let purpose = if purpose_byte == 0 {
        None
    } else {
        Some(Purpose::parse(purpose_byte).ok_or(HeaderError::BadPurpose {
            purpose: purpose_byte,
        })?)
    };
    let peer = if peer_raw == [0u8; 16] {
        None
    } else {
        Some(Id(peer_raw))
    };

    // The length-and-enum check (§4.1: the relay validates these).
    match kind {
        Kind::Hs => {
            if !(1..=3).contains(&step) {
                return Err(HeaderError::BadStep { step });
            }
            if purpose.is_none() {
                return Err(HeaderError::BadPurpose { purpose: 0 });
            }
            if ctr != 0 {
                return Err(HeaderError::NonZeroCtr { ctr });
            }
            if step == 1 && peer.is_none() {
                return Err(HeaderError::MissingPeer);
            }
            if step != 1 && peer.is_some() {
                return Err(HeaderError::UnexpectedPeer);
            }
        }
        Kind::Data => {
            if step != 0 {
                return Err(HeaderError::BadStep { step });
            }
            if purpose.is_some() {
                return Err(HeaderError::BadPurpose {
                    purpose: purpose_byte,
                });
            }
            if peer.is_some() {
                return Err(HeaderError::UnexpectedPeer);
            }
        }
        Kind::Close => {
            if step != 0 {
                return Err(HeaderError::BadStep { step });
            }
            if purpose.is_some() {
                return Err(HeaderError::BadPurpose {
                    purpose: purpose_byte,
                });
            }
            if peer.is_some() {
                return Err(HeaderError::UnexpectedPeer);
            }
            if ctr != 0 {
                return Err(HeaderError::NonZeroCtr { ctr });
            }
        }
    }

    let body = match body_len {
        Some(len) => {
            if len > MAX_BODY_LEN {
                return Err(HeaderError::BodyTooLong { len });
            }
            if kind == Kind::Close && len != 1 {
                return Err(HeaderError::BadCloseBody { len });
            }
            &[][..]
        }
        None => {
            if rest.len() > MAX_BODY_LEN {
                return Err(HeaderError::BodyTooLong { len: rest.len() });
            }
            if kind == Kind::Close && rest.len() != 1 {
                return Err(HeaderError::BadCloseBody { len: rest.len() });
            }
            rest
        }
    };
    Ok((
        Frame {
            kind,
            step,
            purpose,
            chan: ChannelId(chan),
            peer,
            ctr,
            body,
        },
        HEADER_LEN + body.len(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chan() -> ChannelId {
        ChannelId([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ])
    }

    fn peer() -> Id {
        Id::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap()
    }

    #[test]
    fn hs1_round_trip() {
        let head = Head {
            kind: Kind::Hs,
            step: 1,
            purpose: Some(Purpose::Companion),
            chan: chan(),
            peer: Some(peer()),
            ctr: 0,
            body: b"handshake-bytes",
        };
        let mut buf = Vec::new();
        head.encode(&mut buf).unwrap();
        assert_eq!(buf.len(), HEADER_LEN + 15);
        assert_eq!(&buf[0..4], &[0x01, 0x01, 0x01, 0x01]);
        let (frame, len) = decode(&buf).unwrap();
        assert_eq!(len, buf.len());
        assert_eq!(frame.kind, Kind::Hs);
        assert_eq!(frame.step, 1);
        assert_eq!(frame.purpose, Some(Purpose::Companion));
        assert_eq!(frame.chan, chan());
        assert_eq!(frame.peer, Some(peer()));
        assert_eq!(frame.ctr, 0);
        assert_eq!(frame.body, b"handshake-bytes");
    }

    #[test]
    fn data_and_close_round_trip() {
        let data = Head {
            kind: Kind::Data,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 41,
            body: &[7u8; 100],
        };
        let mut buf = Vec::new();
        data.encode(&mut buf).unwrap();
        let (frame, _) = decode(&buf).unwrap();
        assert_eq!(frame.kind, Kind::Data);
        assert_eq!(frame.ctr, 41);
        assert_eq!(frame.peer, None);

        let close = Head {
            kind: Kind::Close,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 0,
            body: &[CloseReason::ProtocolError.byte()],
        };
        let mut buf = Vec::new();
        close.encode(&mut buf).unwrap();
        let (frame, len) = decode(&buf).unwrap();
        assert_eq!(len, HEADER_LEN + 1);
        assert_eq!(frame.kind, Kind::Close);
        assert_eq!(CloseReason::parse(frame.body[0]), CloseReason::ProtocolError);
    }

    #[test]
    fn enum_violations_rejected() {
        let mut good = Vec::new();
        Head {
            kind: Kind::Hs,
            step: 1,
            purpose: Some(Purpose::Pairing),
            chan: chan(),
            peer: Some(peer()),
            ctr: 0,
            body: b"x",
        }
        .encode(&mut good)
        .unwrap();
        // One mutated byte per rejected case.
        for (index, value) in [(0u8, 0x02u8), (1, 0x04), (2, 0x00), (2, 0x04), (3, 0x03), (3, 0x00)]
        {
            let mut bad = good.clone();
            bad[index as usize] = value;
            assert!(decode(&bad).is_err(), "byte {index} = {value:#04x}");
        }
        // HS step 2 with a peer id set.
        let mut bad = good.clone();
        bad[2] = 0x02;
        assert_eq!(
            decode(&bad).unwrap_err(),
            HeaderError::UnexpectedPeer,
            "HS2 must not carry a peer"
        );
        // DATA with a nonzero purpose, CLOSE with a 2-byte body.
        let mut data = Vec::new();
        Head {
            kind: Kind::Data,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 0,
            body: b"y",
        }
        .encode(&mut data)
        .unwrap();
        data[3] = 0x01;
        assert!(decode(&data).is_err());
        assert_eq!(
            Head {
                kind: Kind::Close,
                step: 0,
                purpose: None,
                chan: chan(),
                peer: None,
                ctr: 0,
                body: &[0x01, 0x02],
            }
            .encode(&mut Vec::new())
            .unwrap_err(),
            HeaderError::BadCloseBody { len: 2 }
        );
        // A CLOSE header followed by a 2-byte body fails the length check.
        let mut close = Vec::new();
        Head {
            kind: Kind::Close,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 0,
            body: &[0x01],
        }
        .encode(&mut close)
        .unwrap();
        close.push(0x02);
        assert_eq!(decode(&close).unwrap_err(), HeaderError::BadCloseBody { len: 2 });
    }

    #[test]
    fn truncated_and_close_reasons() {
        // The frame has no self-delimiting body: decode consumes the whole
        // buffer as one frame (the WebSocket message is the boundary), and a
        // second concatenated frame is body bytes, never silently dropped.
        let mut two = Vec::new();
        Head {
            kind: Kind::Data,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 0,
            body: b"one",
        }
        .encode(&mut two)
        .unwrap();
        Head {
            kind: Kind::Close,
            step: 0,
            purpose: None,
            chan: chan(),
            peer: None,
            ctr: 0,
            body: &[0x01],
        }
        .encode(&mut two)
        .unwrap();
        let (f, len) = decode(&two).unwrap();
        assert_eq!(len, two.len());
        assert_eq!(f.kind, Kind::Data);
        assert_eq!(f.body.len(), 3 + HEADER_LEN + 1, "nothing is skipped");
        assert_eq!(decode(&[0u8; 43]).unwrap_err(), HeaderError::Truncated { len: 43 });
        assert_eq!(CloseReason::parse(0x01), CloseReason::ProtocolError);
        assert_eq!(CloseReason::parse(0x02), CloseReason::UnknownPeer);
        assert_eq!(CloseReason::parse(0x7f), CloseReason::Unknown(0x7f));
        assert!(CloseReason::ProtocolError.known());
        assert!(!CloseReason::Unknown(0x7f).known());
    }
}
