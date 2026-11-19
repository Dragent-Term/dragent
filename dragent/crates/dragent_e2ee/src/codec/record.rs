//! §4.4 records: fragmentation, padding and reassembly.
//!
//! Each DATA plaintext is one record of length `L` in
//! `{64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65519}`:
//!
//! ```text
//! 0   1   rflags: bit0 FIRST, bit1 LAST, bit2 PAD_ONLY; others 0
//! 1   1   mtype (FIRST only, else 0): 0x01 JSON, 0x02 TERM, 0x03 CTRL
//! 2   4   msg_id u32, per direction, starts at 1, increases per message
//! 6   4   total_len u32 (FIRST only, else 0) ≤ 1,048,576
//! 10  2   clen u16 ≤ L − 12
//! 12  clen content
//! …   zero padding to L (receiver verifies all-zero)
//! ```
//!
//! Choices made here (the design leaves them open):
//! - `msg_id` 0 is rejected: ids start at 1, and 0 would collide with the
//!   padding-only record below.
//! - A PAD_ONLY record carries no message: `msg_id`, `mtype`, `total_len` and
//!   `clen` must all be 0. It contributes no bytes to any message.
//! - Reserved `rflags` bits (3..=7) are rejected.
//! - A non-FIRST record must have `mtype` 0 and `total_len` 0; a FIRST record
//!   of a real message must have `mtype` ∈ {JSON, TERM, CTRL}.
//! - The sender picks `L`; [`Record::encode`] takes the content and a target
//!   `L` and pads with zeros. [`smallest_len`] returns the smallest `L` that
//!   fits.
//!
//! [`Reassembler`] puts messages back together: messages over 65,507 content
//! bytes span consecutive records with the same `msg_id` (interleaved with
//! other messages at record boundaries). Limits (§4.4, enforced at both ends):
//! at most 4 partial messages and 2 MiB buffered per direction; records of one
//! message arrive in order; the `clen` sum equals `total_len`. Any violation is
//! a protocol error — the caller closes the channel (see
//! [`crate::channel::ProtocolViolation`]), so the reassembler only reports
//! [`ReassembleError`] and holds no channel state.

/// Record lengths the sender may use (§4.4).
pub const RECORD_LENS: [usize; 11] = [
    64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65519,
];
/// Header bytes before content (§4.4: 12).
pub const RECORD_OVERHEAD: usize = 12;
/// Largest content one record carries: 65,519 − 12 = 65,507.
pub const MAX_RECORD_CONTENT: usize = 65519 - RECORD_OVERHEAD;
/// Largest message: `total_len` ≤ 1,048,576 (§4.4).
///
/// Named `MAX_TOTAL_LEN` (not `MAX_TOTAL_LEN`): the latter already means
/// the 65,535-byte Noise transport cap in [`crate::noise`].
pub const MAX_TOTAL_LEN: usize = 1_048_576;
/// Receiver limits: at most 4 partial messages, 2 MiB buffered (§4.4).
pub const MAX_PARTIALS: usize = 4;
/// Receiver limit on buffered bytes per direction (§4.4).
pub const MAX_BUFFERED: usize = 2 * 1024 * 1024;

/// Message types (§4.4): JSON envelopes, TERM output, CTRL control messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    /// `0x01`: `rpc-request`, `rpc-reply`, `rpc-subscribe`, `rpc-unsubscribe`,
    /// `rpc-stream` envelopes, answered by the desktop.
    Json,
    /// `0x02`: terminal output (§4.4 TERM layout, see [`crate::codec::term`]).
    Term,
    /// `0x03`: `hello`, `bye`, `pair-confirm`, `pair-done`,
    /// `identity-rotate`, `input_lost`, `input_lost_ack`, `part-begin`,
    /// `part`, `part-end`, `part-abort`.
    Ctrl,
}

impl MsgType {
    fn byte(self) -> u8 {
        match self {
            MsgType::Json => 0x01,
            MsgType::Term => 0x02,
            MsgType::Ctrl => 0x03,
        }
    }

    fn parse(b: u8) -> Option<MsgType> {
        match b {
            0x01 => Some(MsgType::Json),
            0x02 => Some(MsgType::Term),
            0x03 => Some(MsgType::Ctrl),
            _ => None,
        }
    }
}

/// Flag bits in `rflags` (§4.4).
pub mod flag {
    /// First record of a message.
    pub const FIRST: u8 = 0x01;
    /// Last record of a message.
    pub const LAST: u8 = 0x02;
    /// Padding-only record: carries no message bytes (all id fields zero).
    pub const PAD_ONLY: u8 = 0x04;
}

/// A decoded record with borrowed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// First record of its message.
    pub first: bool,
    /// Last record of its message.
    pub last: bool,
    /// Padding-only record (no message; `msg_id` is 0).
    pub pad_only: bool,
    /// Message type (FIRST of a real message only).
    pub mtype: Option<MsgType>,
    /// Per-direction message id (starts at 1; 0 only for PAD_ONLY).
    pub msg_id: u32,
    /// Total message content length (FIRST only).
    pub total_len: u32,
    /// This record's content.
    pub content: &'a [u8],
    /// The record length `L` (one of [`RECORD_LENS`]).
    pub len: usize,
}

/// Why a record was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Not one of the §4.4 lengths.
    BadLength {
        /// The length seen.
        len: usize,
    },
    /// Reserved `rflags` bits set.
    BadFlags {
        /// The flags byte seen.
        flags: u8,
    },
    /// PAD_ONLY combined with FIRST/LAST, or PAD_ONLY with nonzero fields.
    BadPadOnly,
    /// `msg_id` 0 on a non-PAD_ONLY record.
    BadMsgId,
    /// Unknown `mtype`, or a nonzero `mtype`/`total_len` where zero is required.
    BadMtype {
        /// The mtype byte seen.
        mtype: u8,
    },
    /// Nonzero `total_len` on a non-FIRST record.
    BadTotal {
        /// The total_len seen.
        total: u32,
    },
    /// `total_len` over 1,048,576.
    TotalTooLarge {
        /// The total_len seen.
        total: u32,
    },
    /// `clen` over `L − 12`, or past the end of the record.
    BadContentLen {
        /// The clen seen.
        clen: usize,
    },
    /// Nonzero padding bytes.
    BadPadding,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::BadLength { len } => write!(f, "bad record length {len}"),
            RecordError::BadFlags { flags } => write!(f, "bad record flags {flags:#04x}"),
            RecordError::BadPadOnly => f.write_str("bad padding-only record"),
            RecordError::BadMsgId => f.write_str("message id 0 outside a padding record"),
            RecordError::BadMtype { mtype } => write!(f, "bad message type {mtype:#04x}"),
            RecordError::BadTotal { total } => {
                write!(f, "nonzero total_len {total} outside FIRST")
            }
            RecordError::TotalTooLarge { total } => write!(f, "total_len {total} over 1 MiB"),
            RecordError::BadContentLen { clen } => write!(f, "bad content length {clen}"),
            RecordError::BadPadding => f.write_str("nonzero padding"),
        }
    }
}

impl std::error::Error for RecordError {}

/// The smallest [`RECORD_LENS`] entry that fits `content_len` content bytes.
/// Returns `None` when even 65,507 bytes do not fit (use the §4.5 part
/// protocol instead).
pub fn smallest_len(content_len: usize) -> Option<usize> {
    RECORD_LENS.iter().copied().find(|&l| content_len <= l - RECORD_OVERHEAD)
}

/// Encode one record: 12 header bytes, content, then zeros to `len`.
/// `mtype`/`total_len` apply to FIRST records; continuation records pass
/// `mtype = None`. PAD_ONLY records pass `pad_only = true` with empty content.
#[allow(clippy::too_many_arguments)]
pub fn encode(
    out: &mut Vec<u8>,
    len: usize,
    first: bool,
    last: bool,
    pad_only: bool,
    mtype: Option<MsgType>,
    msg_id: u32,
    total_len: u32,
    content: &[u8],
) -> Result<(), RecordError> {
    if !RECORD_LENS.contains(&len) {
        return Err(RecordError::BadLength { len });
    }
    if pad_only {
        if first || last {
            return Err(RecordError::BadPadOnly);
        }
        if mtype.is_some() || msg_id != 0 || total_len != 0 || !content.is_empty() {
            return Err(RecordError::BadPadOnly);
        }
    } else {
        if msg_id == 0 {
            return Err(RecordError::BadMsgId);
        }
        if total_len > MAX_TOTAL_LEN as u32 {
            return Err(RecordError::TotalTooLarge { total: total_len });
        }
        match (first, mtype) {
            (true, Some(_)) => {}
            (true, None) => return Err(RecordError::BadMtype { mtype: 0 }),
            (false, None) => {}
            (false, Some(m)) => return Err(RecordError::BadMtype { mtype: m.byte() }),
        }
        if !first && total_len != 0 {
            return Err(RecordError::BadTotal { total: total_len });
        }
    }
    if content.len() > len - RECORD_OVERHEAD {
        return Err(RecordError::BadContentLen { clen: content.len() });
    }
    let mut flags = 0u8;
    if first {
        flags |= flag::FIRST;
    }
    if last {
        flags |= flag::LAST;
    }
    if pad_only {
        flags |= flag::PAD_ONLY;
    }
    out.push(flags);
    out.push(mtype.map_or(0, MsgType::byte));
    out.extend_from_slice(&msg_id.to_be_bytes());
    out.extend_from_slice(&total_len.to_be_bytes());
    out.extend_from_slice(&(content.len() as u16).to_be_bytes());
    out.extend_from_slice(content);
    out.resize(out.len() + (len - RECORD_OVERHEAD - content.len()), 0);
    Ok(())
}

/// Decode and verify one record (`bytes` is exactly the `L`-byte plaintext).
/// Verifies the length is legal, reserved bits are clear, `clen` fits and the
/// padding is all zero.
pub fn decode(bytes: &[u8]) -> Result<Record<'_>, RecordError> {
    if !RECORD_LENS.contains(&bytes.len()) {
        return Err(RecordError::BadLength { len: bytes.len() });
    }
    let flags = bytes[0];
    if flags & !0x07 != 0 {
        return Err(RecordError::BadFlags { flags });
    }
    let first = flags & flag::FIRST != 0;
    let last = flags & flag::LAST != 0;
    let pad_only = flags & flag::PAD_ONLY != 0;
    let mtype_byte = bytes[1];
    let msg_id = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let total_len = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    let clen = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
    if clen > bytes.len() - RECORD_OVERHEAD {
        return Err(RecordError::BadContentLen { clen });
    }
    let content = &bytes[RECORD_OVERHEAD..RECORD_OVERHEAD + clen];
    if bytes[RECORD_OVERHEAD + clen..].iter().any(|&b| b != 0) {
        return Err(RecordError::BadPadding);
    }
    if pad_only {
        if first || last || mtype_byte != 0 || msg_id != 0 || total_len != 0 || clen != 0 {
            return Err(RecordError::BadPadOnly);
        }
        return Ok(Record {
            first: false,
            last: false,
            pad_only: true,
            mtype: None,
            msg_id: 0,
            total_len: 0,
            content: &[],
            len: bytes.len(),
        });
    }
    if msg_id == 0 {
        return Err(RecordError::BadMsgId);
    }
    let mtype = match (first, mtype_byte) {
        (true, b) => Some(MsgType::parse(b).ok_or(RecordError::BadMtype { mtype: b })?),
        (false, 0) => None,
        (false, b) => return Err(RecordError::BadMtype { mtype: b }),
    };
    if !first && total_len != 0 {
        return Err(RecordError::BadTotal { total: total_len });
    }
    if total_len > MAX_TOTAL_LEN as u32 {
        return Err(RecordError::TotalTooLarge { total: total_len });
    }
    Ok(Record {
        first,
        last,
        pad_only: false,
        mtype,
        msg_id,
        total_len,
        content,
        len: bytes.len(),
    })
}

/// A reassembled message: its type and full content bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The type from the FIRST record.
    pub mtype: MsgType,
    /// The concatenated content bytes (`total_len` long).
    pub content: Vec<u8>,
}

/// Why reassembly failed. Every variant is a §4.3-style protocol error: the
/// caller must close the channel, never process the partial bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReassembleError {
    /// A FIRST record for an id that already has a partial message.
    DuplicateFirst {
        /// The message id.
        msg_id: u32,
    },
    /// A continuation record with no open FIRST.
    OrphanContinuation {
        /// The message id.
        msg_id: u32,
    },
    /// A record that would push a message past its `total_len`, or a LAST
    /// record whose sum disagrees with `total_len`.
    LengthMismatch {
        /// The message id.
        msg_id: u32,
    },
    /// More than 4 partial messages would be open at once.
    TooManyPartials,
    /// More than 2 MiB would be buffered.
    BufferFull,
    /// A message completed twice (LAST after completion is an orphan).
    AlreadyComplete {
        /// The message id.
        msg_id: u32,
    },
}

impl std::fmt::Display for ReassembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReassembleError::DuplicateFirst { msg_id } => {
                write!(f, "second FIRST for message {msg_id}")
            }
            ReassembleError::OrphanContinuation { msg_id } => {
                write!(f, "continuation without FIRST for message {msg_id}")
            }
            ReassembleError::LengthMismatch { msg_id } => {
                write!(f, "length mismatch in message {msg_id}")
            }
            ReassembleError::TooManyPartials => f.write_str("too many partial messages"),
            ReassembleError::BufferFull => f.write_str("reassembly buffer full"),
            ReassembleError::AlreadyComplete { msg_id } => {
                write!(f, "message {msg_id} completed twice")
            }
        }
    }
}

impl std::error::Error for ReassembleError {}

#[derive(Debug)]
struct Partial {
    mtype: MsgType,
    total: usize,
    buf: Vec<u8>,
}

/// One direction's reassembly state (§4.4 receiver limits).
#[derive(Debug, Default)]
pub struct Reassembler {
    partials: Vec<(u32, Partial)>,
    buffered: usize,
}

impl Reassembler {
    /// An empty reassembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open partial messages (for tests and flow-control accounting).
    pub fn partial_count(&self) -> usize {
        self.partials.len()
    }

    /// Bytes currently buffered (for tests and accounting).
    pub fn buffered(&self) -> usize {
        self.buffered
    }

    /// Feed one decoded record. PAD_ONLY records are ignored. Returns
    /// `Some(message)` when a LAST record completes a message; single-record
    /// (FIRST|LAST) messages complete immediately without buffering.
    pub fn feed(&mut self, record: &Record<'_>) -> Result<Option<Message>, ReassembleError> {
        if record.pad_only {
            return Ok(None);
        }
        if record.first {
            if self.partials.iter().any(|(id, _)| *id == record.msg_id) {
                return Err(ReassembleError::DuplicateFirst { msg_id: record.msg_id });
            }
            let total = record.total_len as usize;
            if record.content.len() > total {
                return Err(ReassembleError::LengthMismatch { msg_id: record.msg_id });
            }
            if record.last {
                if record.content.len() != total {
                    return Err(ReassembleError::LengthMismatch { msg_id: record.msg_id });
                }
                return Ok(Some(Message {
                    mtype: record.mtype.expect("FIRST of a message has a type"),
                    content: record.content.to_vec(),
                }));
            }
            if self.partials.len() >= MAX_PARTIALS {
                return Err(ReassembleError::TooManyPartials);
            }
            if self.buffered + record.content.len() > MAX_BUFFERED {
                return Err(ReassembleError::BufferFull);
            }
            self.buffered += record.content.len();
            self.partials.push((
                record.msg_id,
                Partial {
                    mtype: record.mtype.expect("FIRST of a message has a type"),
                    total,
                    buf: record.content.to_vec(),
                },
            ));
            return Ok(None);
        }
        let slot = self
            .partials
            .iter_mut()
            .find(|(id, _)| *id == record.msg_id)
            .ok_or(ReassembleError::OrphanContinuation { msg_id: record.msg_id })?;
        if slot.1.buf.len() + record.content.len() > slot.1.total {
            return Err(ReassembleError::LengthMismatch { msg_id: record.msg_id });
        }
        if self.buffered + record.content.len() > MAX_BUFFERED {
            return Err(ReassembleError::BufferFull);
        }
        slot.1.buf.extend_from_slice(record.content);
        self.buffered += record.content.len();
        if record.last {
            if slot.1.buf.len() != slot.1.total {
                return Err(ReassembleError::LengthMismatch { msg_id: record.msg_id });
            }
            let pos = self.partials.iter().position(|(id, _)| *id == record.msg_id).expect(
                "continuation has an open partial",
            );
            let (_, partial) = self.partials.remove(pos);
            self.buffered -= partial.buf.len();
            return Ok(Some(Message {
                mtype: partial.mtype,
                content: partial.buf,
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(
        len: usize,
        first: bool,
        last: bool,
        mtype: Option<MsgType>,
        msg_id: u32,
        total: u32,
        content: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        encode(&mut out, len, first, last, false, mtype, msg_id, total, content).unwrap();
        assert_eq!(out.len(), len);
        out
    }

    #[test]
    fn single_record_round_trip() {
        let bytes = rec(64, true, true, Some(MsgType::Json), 1, 3, b"{}x");
        let r = decode(&bytes).unwrap();
        assert!(r.first && r.last && !r.pad_only);
        assert_eq!(r.mtype, Some(MsgType::Json));
        assert_eq!(r.msg_id, 1);
        assert_eq!(r.total_len, 3);
        assert_eq!(r.content, b"{}x");
        assert_eq!(r.len, 64);
    }

    #[test]
    fn smallest_len_table() {
        assert_eq!(smallest_len(0), Some(64));
        assert_eq!(smallest_len(52), Some(64));
        assert_eq!(smallest_len(53), Some(128));
        assert_eq!(smallest_len(65507), Some(65519));
        assert_eq!(smallest_len(65508), None);
    }

    #[test]
    fn rejections() {
        // Bad length: only an exact legal L parses.
        assert_eq!(decode(&[0u8; 63]).unwrap_err(), RecordError::BadLength { len: 63 });
        // One byte past a legal L: trailing bytes are rejected, not parsed.
        let mut plus_one = rec(64, true, true, Some(MsgType::Json), 1, 3, b"{}x");
        plus_one.push(0);
        assert_eq!(decode(&plus_one).unwrap_err(), RecordError::BadLength { len: 65 });
        // Reserved flag bit.
        let mut b = rec(64, true, true, Some(MsgType::Ctrl), 1, 1, b"h");
        b[0] |= 0x08;
        assert_eq!(decode(&b).unwrap_err(), RecordError::BadFlags { flags: 0x0b });
        // Nonzero padding.
        let mut b = rec(64, true, true, Some(MsgType::Ctrl), 1, 1, b"h");
        b[63] = 1;
        assert_eq!(decode(&b).unwrap_err(), RecordError::BadPadding);
        // Zero msg id, unknown mtype, clen past the end.
        let mut out = Vec::new();
        assert_eq!(
            encode(&mut out, 64, true, true, false, Some(MsgType::Json), 0, 1, b"a")
                .unwrap_err(),
            RecordError::BadMsgId
        );
        let mut b = rec(64, true, true, Some(MsgType::Json), 1, 1, b"a");
        b[1] = 0x09;
        assert_eq!(decode(&b).unwrap_err(), RecordError::BadMtype { mtype: 0x09 });
        let mut b = rec(64, false, true, None, 1, 0, b"a");
        b[10] = 0xff;
        b[11] = 0xff;
        assert!(matches!(decode(&b), Err(RecordError::BadContentLen { .. })));
        // total_len over 1 MiB.
        let mut out = Vec::new();
        assert_eq!(
            encode(&mut out, 64, true, true, false, Some(MsgType::Json), 1, 1_048_577, b"")
                .unwrap_err(),
            RecordError::TotalTooLarge { total: 1_048_577 }
        );
        // Continuation with nonzero mtype/total.
        let b = rec(64, false, true, None, 1, 0, b"a");
        // (valid) — now corrupt:
        let mut bad = b.clone();
        bad[1] = 0x01;
        assert_eq!(decode(&bad).unwrap_err(), RecordError::BadMtype { mtype: 0x01 });
        let mut bad = b.clone();
        bad[6] = 0x01;
        assert_eq!(decode(&bad).unwrap_err(), RecordError::BadTotal { total: 0x0100_0000 });
        // FIRST without a type.
        let mut out = Vec::new();
        assert_eq!(
            encode(&mut out, 64, true, true, false, None, 1, 0, b"").unwrap_err(),
            RecordError::BadMtype { mtype: 0 }
        );
    }

    #[test]
    fn pad_only_records() {
        let mut out = Vec::new();
        encode(&mut out, 128, false, false, true, None, 0, 0, b"").unwrap();
        let r = decode(&out).unwrap();
        assert!(r.pad_only && r.msg_id == 0 && r.content.is_empty());
        let mut r2 = Reassembler::new();
        assert_eq!(r2.feed(&r).unwrap(), None);
        assert_eq!(r2.partial_count(), 0);
        // PAD_ONLY with content or flags is rejected.
        let mut out = Vec::new();
        assert_eq!(
            encode(&mut out, 128, true, false, true, None, 0, 0, b"").unwrap_err(),
            RecordError::BadPadOnly
        );
    }

    #[test]
    fn fragmented_message_reassembles() {
        let mut r = Reassembler::new();
        // Two interleaved two-record messages.
        let a1 = rec(64, true, false, Some(MsgType::Term), 1, 60, &[1u8; 52]);
        let b1 = rec(64, true, false, Some(MsgType::Json), 2, 10, b"0123456789");
        let a2 = rec(64, false, true, None, 1, 0, &[2u8; 8]);
        let b2 = rec(64, false, true, None, 2, 0, b"");
        // Decodes borrow from these buffers; feed one at a time.
        for bytes in [&a1, &b1] {
            let d = decode(bytes).unwrap();
            assert_eq!(r.feed(&d).unwrap(), None);
        }
        assert_eq!(r.partial_count(), 2);
        let d = decode(&a2).unwrap();
        let done: Option<Message> = r.feed(&d).unwrap();
        let done = done.unwrap();
        assert_eq!(done.mtype, MsgType::Term);
        assert_eq!(done.content.len(), 60);
        assert_eq!(r.partial_count(), 1);
        let d = decode(&b2).unwrap();
        let done = r.feed(&d).unwrap().unwrap();
        assert_eq!(done.mtype, MsgType::Json);
        assert_eq!(done.content, b"0123456789");
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn reassembly_violations() {
        let mut r = Reassembler::new();
        let f1 = rec(64, true, false, Some(MsgType::Json), 1, 60, &[0u8; 52]);
        let f1dup = rec(64, true, false, Some(MsgType::Json), 1, 60, &[0u8; 52]);
        let orphan = rec(64, false, true, None, 9, 0, b"x");
        let over = rec(64, false, true, None, 1, 0, &[0u8; 52]);
        let short = rec(64, false, true, None, 1, 0, b"");
        let d = decode(&f1).unwrap();
        r.feed(&d).unwrap();
        let d = decode(&f1dup).unwrap();
        assert_eq!(r.feed(&d), Err(ReassembleError::DuplicateFirst { msg_id: 1 }));
        let d = decode(&orphan).unwrap();
        assert_eq!(r.feed(&d), Err(ReassembleError::OrphanContinuation { msg_id: 9 }));
        // 52 buffered + 52 more > total 60.
        let d = decode(&over).unwrap();
        assert_eq!(r.feed(&d), Err(ReassembleError::LengthMismatch { msg_id: 1 }));
        // LAST with only 52 of 60 bytes.
        let d = decode(&short).unwrap();
        assert_eq!(r.feed(&d), Err(ReassembleError::LengthMismatch { msg_id: 1 }));
        // Too many partials.
        let mut r = Reassembler::new();
        for id in 1..=4u32 {
            let b = rec(64, true, false, Some(MsgType::Json), id, 52, &[0u8; 52]);
            let d = decode(&b).unwrap();
            r.feed(&d).unwrap();
        }
        let b = rec(64, true, false, Some(MsgType::Json), 5, 52, &[0u8; 52]);
        let d = decode(&b).unwrap();
        assert_eq!(r.feed(&d), Err(ReassembleError::TooManyPartials));
    }
}
