//! §4.4 TERM record payload: one terminal output slice.
//!
//! ```text
//! handle:u32 | seq:u64 | base:u64 | total:u64 | flags:u8 (alive, reset, has_exit, has_size)
//! | [exit_code:i32] | [rows:u16 cols:u16] | data
//! ```
//!
//! All integers are big-endian. `seq`, `base` and `total` must be ≤ 2^53 − 1
//! (they travel through JSON elsewhere). Flag bits chosen here: bit 0 `alive`,
//! bit 1 `reset`, bit 2 `has_exit`, bit 3 `has_size`; bits 4..=7 are reserved
//! and rejected. A TERM payload fits one record (the desktop splits reads at
//! 65,000 bytes); encode rejects content over 65,507 bytes.
//!
//! The phone rebuilds today's `terminal.output` object from this, so the field
//! order matches what the renderer already consumes.

/// Largest u53 value (JSON-safe counters).
pub const MAX_COUNTER: u64 = (1 << 53) - 1;
/// Largest TERM payload: one record's content (§4.4: 65,519 − 12).
pub const MAX_TERM_LEN: usize = 65519 - 12;

/// Flag bits in the TERM flags byte (chosen here; see module docs).
pub mod flag {
    /// The PTY is alive (else this is a final snapshot).
    pub const ALIVE: u8 = 0x01;
    /// The stream reset (the phone discards earlier state).
    pub const RESET: u8 = 0x02;
    /// An `exit_code` follows the flags.
    pub const HAS_EXIT: u8 = 0x04;
    /// `rows` and `cols` follow (after `exit_code` when present).
    pub const HAS_SIZE: u8 = 0x08;
}

/// A decoded TERM payload with borrowed screen bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term<'a> {
    /// The desktop-minted handle (§4.6: bound to phone key and lease).
    pub handle: u32,
    /// Stream sequence number.
    pub seq: u64,
    /// First byte the phone still holds (continuity check at rekey).
    pub base: u64,
    /// Total bytes the desktop has ever written on this stream.
    pub total: u64,
    /// `true` while the PTY is alive.
    pub alive: bool,
    /// The stream reset; the phone discards earlier state.
    pub reset: bool,
    /// Process exit code, when the stream ended with one.
    pub exit_code: Option<i32>,
    /// Terminal size in rows and columns, when reported.
    pub size: Option<(u16, u16)>,
    /// Raw VT output bytes.
    pub data: &'a [u8],
}

/// Builder for [`Term`] (for sending).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermHead<'a> {
    /// The desktop-minted handle.
    pub handle: u32,
    /// Stream sequence number (≤ 2^53 − 1).
    pub seq: u64,
    /// First byte the phone still holds (≤ 2^53 − 1).
    pub base: u64,
    /// Total bytes written (≤ 2^53 − 1).
    pub total: u64,
    /// `true` while the PTY is alive.
    pub alive: bool,
    /// The stream reset.
    pub reset: bool,
    /// Process exit code, when the stream ended with one.
    pub exit_code: Option<i32>,
    /// Terminal size, when reported.
    pub size: Option<(u16, u16)>,
    /// Raw VT output bytes (≤ 65,507 bytes total payload).
    pub data: &'a [u8],
}

/// Why a TERM payload was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermError {
    /// Fewer than the 29 fixed bytes.
    Truncated,
    /// Reserved flag bits set.
    BadFlags {
        /// The flags byte seen.
        flags: u8,
    },
    /// `seq`, `base` or `total` above 2^53 − 1.
    CounterTooLarge {
        /// The counter seen.
        value: u64,
    },
    /// The payload would not fit one record.
    TooLong {
        /// The payload length it would have.
        len: usize,
    },
}

impl std::fmt::Display for TermError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TermError::Truncated => f.write_str("TERM payload truncated"),
            TermError::BadFlags { flags } => write!(f, "bad TERM flags {flags:#04x}"),
            TermError::CounterTooLarge { value } => {
                write!(f, "TERM counter {value} above 2^53 - 1")
            }
            TermError::TooLong { len } => write!(f, "TERM payload too long: {len} bytes"),
        }
    }
}

impl std::error::Error for TermError {}

fn fixed_len(exit: bool, size: bool) -> usize {
    4 + 8 + 8 + 8 + 1 + if exit { 4 } else { 0 } + if size { 4 } else { 0 }
}

impl<'a> TermHead<'a> {
    /// Encode the payload. Counters above 2^53 − 1 and payloads over one
    /// record's content are rejected.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), TermError> {
        for &v in &[self.seq, self.base, self.total] {
            if v > MAX_COUNTER {
                return Err(TermError::CounterTooLarge { value: v });
            }
        }
        let len = fixed_len(self.exit_code.is_some(), self.size.is_some()) + self.data.len();
        if len > MAX_TERM_LEN {
            return Err(TermError::TooLong { len });
        }
        out.reserve(len);
        out.extend_from_slice(&self.handle.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.base.to_be_bytes());
        out.extend_from_slice(&self.total.to_be_bytes());
        let mut flags = 0u8;
        if self.alive {
            flags |= flag::ALIVE;
        }
        if self.reset {
            flags |= flag::RESET;
        }
        if self.exit_code.is_some() {
            flags |= flag::HAS_EXIT;
        }
        if self.size.is_some() {
            flags |= flag::HAS_SIZE;
        }
        out.push(flags);
        if let Some(code) = self.exit_code {
            out.extend_from_slice(&code.to_be_bytes());
        }
        if let Some((rows, cols)) = self.size {
            out.extend_from_slice(&rows.to_be_bytes());
            out.extend_from_slice(&cols.to_be_bytes());
        }
        out.extend_from_slice(self.data);
        Ok(())
    }
}

/// Decode a TERM payload. The fixed fields must all be present; anything after
/// them is `data` (possibly empty).
pub fn decode(bytes: &[u8]) -> Result<Term<'_>, TermError> {
    if bytes.len() < fixed_len(false, false) {
        return Err(TermError::Truncated);
    }
    let handle = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let seq = u64::from_be_bytes(bytes[4..12].try_into().expect("TERM seq"));
    let base = u64::from_be_bytes(bytes[12..20].try_into().expect("TERM base"));
    let total = u64::from_be_bytes(bytes[20..28].try_into().expect("TERM total"));
    for &v in &[seq, base, total] {
        if v > MAX_COUNTER {
            return Err(TermError::CounterTooLarge { value: v });
        }
    }
    let flags = bytes[28];
    if flags & !0x0f != 0 {
        return Err(TermError::BadFlags { flags });
    }
    let mut at = 29;
    let exit_code = if flags & flag::HAS_EXIT != 0 {
        if bytes.len() < at + 4 {
            return Err(TermError::Truncated);
        }
        let code = i32::from_be_bytes(bytes[at..at + 4].try_into().expect("exit code"));
        at += 4;
        Some(code)
    } else {
        None
    };
    let size = if flags & flag::HAS_SIZE != 0 {
        if bytes.len() < at + 4 {
            return Err(TermError::Truncated);
        }
        let rows = u16::from_be_bytes([bytes[at], bytes[at + 1]]);
        let cols = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]);
        at += 4;
        Some((rows, cols))
    } else {
        None
    };
    Ok(Term {
        handle,
        seq,
        base,
        total,
        alive: flags & flag::ALIVE != 0,
        reset: flags & flag::RESET != 0,
        exit_code,
        size,
        data: &bytes[at..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_shape() {
        for (exit, size) in
            [(None, None), (Some(0), None), (None, Some((24, 80))), (Some(-1), Some((51, 123)))]
        {
            let head = TermHead {
                handle: 7,
                seq: 100,
                base: 90,
                total: 200,
                alive: true,
                reset: false,
                exit_code: exit,
                size,
                data: b"\x1b[1mhi",
            };
            let mut out = Vec::new();
            head.encode(&mut out).unwrap();
            let t = decode(&out).unwrap();
            assert_eq!(t.handle, 7);
            assert_eq!((t.seq, t.base, t.total), (100, 90, 200));
            assert!(t.alive && !t.reset);
            assert_eq!(t.exit_code, exit);
            assert_eq!(t.size, size);
            assert_eq!(t.data, b"\x1b[1mhi");
        }
    }

    #[test]
    fn rejections() {
        assert_eq!(decode(&[0u8; 28]).unwrap_err(), TermError::Truncated);
        // Trailing bytes are not rejected: a TERM payload fills its record, so
        // anything after the fixed fields is screen data by construction.
        let bare = TermHead {
            handle: 1,
            seq: 0,
            base: 0,
            total: 3,
            alive: true,
            reset: false,
            exit_code: None,
            size: None,
            data: b"",
        };
        let mut out = Vec::new();
        bare.encode(&mut out).unwrap();
        out.extend_from_slice(b"abc");
        assert_eq!(decode(&out).unwrap().data, b"abc");
        let mut head = TermHead {
            handle: 1,
            seq: 0,
            base: 0,
            total: 0,
            alive: false,
            reset: true,
            exit_code: Some(3),
            size: None,
            data: b"",
        };
        let mut out = Vec::new();
        head.encode(&mut out).unwrap();
        assert_eq!(decode(&out[..30]).unwrap_err(), TermError::Truncated);
        // Reserved flag bit.
        let mut bad = out.clone();
        bad[28] |= 0x10;
        assert_eq!(decode(&bad).unwrap_err(), TermError::BadFlags { flags: 0x16 });
        // Counter above 2^53 - 1.
        head.seq = 1 << 53;
        assert_eq!(
            head.encode(&mut Vec::new()).unwrap_err(),
            TermError::CounterTooLarge { value: 1 << 53 }
        );
        let mut big = Vec::new();
        head.seq = 0;
        let big_data = vec![0u8; MAX_TERM_LEN];
        head.data = &big_data;
        assert_eq!(
            head.encode(&mut big).unwrap_err(),
            TermError::TooLong {
                len: fixed_len(true, false) + MAX_TERM_LEN
            }
        );
    }
}
