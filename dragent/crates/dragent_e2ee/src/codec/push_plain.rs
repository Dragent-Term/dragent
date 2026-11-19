//! §8 push plaintext: the fixed 1,024-byte layout sealed with Noise X.
//!
//! ```text
//! v:u8=1 | desktop_id:16 | id:16 random | issued_at_ms:u64 | kind:u8
//! | dismiss:u8 (0 = show, 1 = remove ids; Android only, step 6)
//! | title_len:u8  | title (UTF-8, ≤ 240 bytes, cut on a grapheme boundary within that)
//! | body_len:u16  | body  (UTF-8, ≤ 600 bytes, cut on a grapheme boundary within that)
//! | workspace_id:16 | tab_id:16
//! | dismiss_count:u8 (≤ 58) | dismiss ids: dismiss_count × 16
//! | zero padding to exactly 1,024 bytes
//! ```
//!
//! All integers are big-endian. The fixed fields take 79 bytes: a notification
//! (`dismiss = 0`, `dismiss_count = 0`) is at most 79 + 240 + 600 = 919 bytes;
//! a dismissal (`dismiss = 1`, empty title and body) is at most
//! 79 + 58 × 16 = 1,007 bytes. Over-limit plaintexts are rejected, never
//! truncated silently: truncation is the caller's explicit choice through
//! [`truncate_to`] (char boundary) or a grapheme-aware cut of its own before
//! calling the constructors — either way the bytes that arrive fit, so the
//! receiver accepts any valid UTF-8 within the byte limits.
//!
//! Rejected (each is a failure: the phone shows the placeholder): a version
//! other than 1, `dismiss` outside {0, 1}, `title_len` over 240, `body_len`
//! over 600, `dismiss_count` over 58, `dismiss = 0` with any dismiss ids,
//! `dismiss = 1` with any title or body text, invalid UTF-8, or nonzero
//! padding.

use crate::ids::Id;

/// Fixed plaintext size (§8: exactly 1,024 bytes): the seal's size
/// ([`crate::seal::push::PLAINTEXT_LEN`]), re-exported so the codec and the
/// crypto agree by construction.
pub use crate::seal::push::PLAINTEXT_LEN;
/// Title cap in bytes (§8).
pub const MAX_TITLE_LEN: usize = 240;
/// Body cap in bytes (§8).
pub const MAX_BODY_LEN: usize = 600;
/// Dismiss-id cap (§8).
pub const MAX_DISMISS: usize = 58;
/// Fixed fields: 1+16+16+8+1+1+1+2+16+16+1 = 79 bytes (§8).
pub const FIXED_LEN: usize = 79;

/// A validated push plaintext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPlain {
    /// The sealing desktop (checked against the push-pin list by the phone).
    pub desktop_id: Id,
    /// Random notification id (also the APNs collapse id / Android tag).
    pub id: [u8; 16],
    /// When the desktop built the notification (stale check: 24 h).
    pub issued_at_ms: u64,
    /// Notification kind (opaque to the codec).
    pub kind: u8,
    /// `false` = show, `true` = remove ids (Android only).
    pub dismiss: bool,
    /// Title text (UTF-8, ≤ 240 bytes; empty for dismissals).
    pub title: String,
    /// Body text (UTF-8, ≤ 600 bytes; empty for dismissals).
    pub body: String,
    /// Workspace the notification opens.
    pub workspace_id: Id,
    /// Tab the notification opens.
    pub tab_id: Id,
    /// Ids to remove (dismissals only, ≤ 58).
    pub dismiss_ids: Vec<[u8; 16]>,
}

/// Why a push plaintext was rejected. Encoding and decoding share the checks,
/// so anything `encode` accepts, `decode` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// Not exactly 1,024 bytes.
    BadLength {
        /// The length seen.
        len: usize,
    },
    /// Version byte other than 1.
    BadVersion {
        /// The version seen.
        version: u8,
    },
    /// `dismiss` outside {0, 1}.
    BadDismiss {
        /// The byte seen.
        dismiss: u8,
    },
    /// Title over 240 bytes.
    TitleTooLong {
        /// The length seen.
        len: usize,
    },
    /// Body over 600 bytes.
    BodyTooLong {
        /// The length seen.
        len: usize,
    },
    /// More than 58 dismiss ids.
    TooManyDismiss {
        /// The count seen.
        count: usize,
    },
    /// `dismiss = 0` with dismiss ids, or `dismiss = 1` with title/body text.
    BadDismissShape,
    /// Title or body is not valid UTF-8.
    BadUtf8,
    /// Nonzero padding bytes.
    BadPadding,
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::BadLength { len } => write!(f, "push plaintext is {len} bytes, want 1024"),
            PushError::BadVersion { version } => write!(f, "bad push version {version}"),
            PushError::BadDismiss { dismiss } => write!(f, "bad push dismiss {dismiss}"),
            PushError::TitleTooLong { len } => write!(f, "push title too long: {len} bytes"),
            PushError::BodyTooLong { len } => write!(f, "push body too long: {len} bytes"),
            PushError::TooManyDismiss { count } => write!(f, "{count} dismiss ids, max 58"),
            PushError::BadDismissShape => f.write_str("push dismiss shape mismatch"),
            PushError::BadUtf8 => f.write_str("push title/body is not UTF-8"),
            PushError::BadPadding => f.write_str("push padding is nonzero"),
        }
    }
}

impl std::error::Error for PushError {}

impl PushPlain {
    /// A notification to show. Titles and bodies over the byte caps are
    /// rejected; cut them first with [`truncate_to`] or a grapheme-aware cut.
    #[allow(clippy::too_many_arguments)]
    pub fn notification(
        desktop_id: Id,
        id: [u8; 16],
        issued_at_ms: u64,
        kind: u8,
        title: &str,
        body: &str,
        workspace_id: Id,
        tab_id: Id,
    ) -> Result<Self, PushError> {
        if title.len() > MAX_TITLE_LEN {
            return Err(PushError::TitleTooLong { len: title.len() });
        }
        if body.len() > MAX_BODY_LEN {
            return Err(PushError::BodyTooLong { len: body.len() });
        }
        Ok(PushPlain {
            desktop_id,
            id,
            issued_at_ms,
            kind,
            dismiss: false,
            title: title.to_string(),
            body: body.to_string(),
            workspace_id,
            tab_id,
            dismiss_ids: Vec::new(),
        })
    }

    /// A dismissal: removes up to 58 notification ids. Title and body are
    /// always empty; anything else is a different message.
    pub fn dismissal(
        desktop_id: Id,
        id: [u8; 16],
        issued_at_ms: u64,
        kind: u8,
        workspace_id: Id,
        tab_id: Id,
        dismiss_ids: Vec<[u8; 16]>,
    ) -> Result<Self, PushError> {
        if dismiss_ids.len() > MAX_DISMISS {
            return Err(PushError::TooManyDismiss { count: dismiss_ids.len() });
        }
        Ok(PushPlain {
            desktop_id,
            id,
            issued_at_ms,
            kind,
            dismiss: true,
            title: String::new(),
            body: String::new(),
            workspace_id,
            tab_id,
            dismiss_ids,
        })
    }

    /// Encode the fixed 1,024-byte layout (big-endian, zero-padded).
    pub fn encode(&self) -> Result<[u8; PLAINTEXT_LEN], PushError> {
        if self.title.len() > MAX_TITLE_LEN {
            return Err(PushError::TitleTooLong { len: self.title.len() });
        }
        if self.body.len() > MAX_BODY_LEN {
            return Err(PushError::BodyTooLong { len: self.body.len() });
        }
        if self.dismiss_ids.len() > MAX_DISMISS {
            return Err(PushError::TooManyDismiss { count: self.dismiss_ids.len() });
        }
        if self.dismiss {
            if !self.title.is_empty() || !self.body.is_empty() {
                return Err(PushError::BadDismissShape);
            }
        } else if !self.dismiss_ids.is_empty() {
            return Err(PushError::BadDismissShape);
        }
        let mut out = [0u8; PLAINTEXT_LEN];
        out[0] = 1;
        out[1..17].copy_from_slice(self.desktop_id.as_bytes());
        out[17..33].copy_from_slice(&self.id);
        out[33..41].copy_from_slice(&self.issued_at_ms.to_be_bytes());
        out[41] = self.kind;
        out[42] = u8::from(self.dismiss);
        out[43] = self.title.len() as u8;
        let mut at = 44;
        out[at..at + self.title.len()].copy_from_slice(self.title.as_bytes());
        at += self.title.len();
        out[at..at + 2].copy_from_slice(&(self.body.len() as u16).to_be_bytes());
        at += 2;
        out[at..at + self.body.len()].copy_from_slice(self.body.as_bytes());
        at += self.body.len();
        out[at..at + 16].copy_from_slice(self.workspace_id.as_bytes());
        at += 16;
        out[at..at + 16].copy_from_slice(self.tab_id.as_bytes());
        at += 16;
        out[at] = self.dismiss_ids.len() as u8;
        at += 1;
        for id in &self.dismiss_ids {
            out[at..at + 16].copy_from_slice(id);
            at += 16;
        }
        debug_assert_eq!(at, FIXED_LEN + self.title.len() + self.body.len() + self.dismiss_ids.len() * 16);
        Ok(out)
    }
}

/// Decode and validate a 1,024-byte push plaintext.
pub fn decode(bytes: &[u8]) -> Result<PushPlain, PushError> {
    if bytes.len() != PLAINTEXT_LEN {
        return Err(PushError::BadLength { len: bytes.len() });
    }
    if bytes[0] != 1 {
        return Err(PushError::BadVersion { version: bytes[0] });
    }
    let mut desktop = [0u8; 16];
    desktop.copy_from_slice(&bytes[1..17]);
    let mut id = [0u8; 16];
    id.copy_from_slice(&bytes[17..33]);
    let mut issued = [0u8; 8];
    issued.copy_from_slice(&bytes[33..41]);
    let kind = bytes[41];
    let dismiss = match bytes[42] {
        0 => false,
        1 => true,
        b => return Err(PushError::BadDismiss { dismiss: b }),
    };
    let title_len = bytes[43] as usize;
    if title_len > MAX_TITLE_LEN {
        return Err(PushError::TitleTooLong { len: title_len });
    }
    let mut at = 44 + title_len;
    if at + 2 > PLAINTEXT_LEN {
        return Err(PushError::BadLength { len: bytes.len() });
    }
    let body_len = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
    if body_len > MAX_BODY_LEN {
        return Err(PushError::BodyTooLong { len: body_len });
    }
    at += 2;
    let title = std::str::from_utf8(&bytes[44..44 + title_len]).map_err(|_| PushError::BadUtf8)?;
    if at + body_len > PLAINTEXT_LEN {
        return Err(PushError::BadLength { len: bytes.len() });
    }
    let body = std::str::from_utf8(&bytes[at..at + body_len]).map_err(|_| PushError::BadUtf8)?;
    at += body_len;
    if at + 33 > PLAINTEXT_LEN {
        return Err(PushError::BadLength { len: bytes.len() });
    }
    let mut workspace = [0u8; 16];
    workspace.copy_from_slice(&bytes[at..at + 16]);
    at += 16;
    let mut tab = [0u8; 16];
    tab.copy_from_slice(&bytes[at..at + 16]);
    at += 16;
    let dismiss_count = bytes[at] as usize;
    at += 1;
    if dismiss_count > MAX_DISMISS {
        return Err(PushError::TooManyDismiss { count: dismiss_count });
    }
    if !dismiss && dismiss_count != 0 {
        return Err(PushError::BadDismissShape);
    }
    if dismiss && (!title.is_empty() || !body.is_empty()) {
        return Err(PushError::BadDismissShape);
    }
    if at + dismiss_count * 16 > PLAINTEXT_LEN {
        return Err(PushError::BadLength { len: bytes.len() });
    }
    let mut dismiss_ids = Vec::with_capacity(dismiss_count);
    for i in 0..dismiss_count {
        let mut did = [0u8; 16];
        did.copy_from_slice(&bytes[at + i * 16..at + (i + 1) * 16]);
        dismiss_ids.push(did);
    }
    at += dismiss_count * 16;
    if bytes[at..].iter().any(|&b| b != 0) {
        return Err(PushError::BadPadding);
    }
    Ok(PushPlain {
        desktop_id: Id(desktop),
        id,
        issued_at_ms: u64::from_be_bytes(issued),
        kind,
        dismiss,
        title: title.to_string(),
        body: body.to_string(),
        workspace_id: Id(workspace),
        tab_id: Id(tab),
        dismiss_ids,
    })
}

/// Cut `s` to at most `max_bytes` on a char boundary. The caller that must cut
/// on a grapheme boundary (desktop sealing, §8) does its own cut first; this is
/// the byte-limit backstop both use.
pub fn truncate_to(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Id, Id, Id) {
        (
            Id::parse("01234567-89ab-4cde-8f01-23456789abcd").unwrap(),
            Id::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
            Id::parse("11111111-2222-4333-8444-555555555555").unwrap(),
        )
    }

    #[test]
    fn fixed_layout_spots() {
        let (d, w, t) = ids();
        let p = PushPlain::notification(d, [7u8; 16], 1_729_000_000_000, 3, "Hi", "Body", w, t)
            .unwrap();
        let enc = p.encode().unwrap();
        assert_eq!(enc.len(), PLAINTEXT_LEN);
        assert_eq!(enc[0], 1);
        assert_eq!(&enc[1..17], d.as_bytes());
        assert_eq!(&enc[17..33], &[7u8; 16]);
        assert_eq!(u64::from_be_bytes(enc[33..41].try_into().unwrap()), 1_729_000_000_000);
        assert_eq!(enc[41], 3);
        assert_eq!(enc[42], 0);
        assert_eq!(enc[43], 2);
        assert_eq!(&enc[44..46], b"Hi");
        assert_eq!(u16::from_be_bytes([enc[46], enc[47]]), 4);
        assert_eq!(&enc[48..52], b"Body");
        // Rest: workspace, tab, zero dismiss count, zero padding.
        assert_eq!(&enc[52..68], w.as_bytes());
        assert_eq!(&enc[68..84], t.as_bytes());
        assert_eq!(enc[84], 0);
        assert!(enc[85..].iter().all(|&b| b == 0));
        assert_eq!(decode(&enc).unwrap(), p);
    }

    #[test]
    fn max_sizes_fit_exactly() {
        let (d, w, t) = ids();
        let title = "a".repeat(MAX_TITLE_LEN);
        let body = "b".repeat(MAX_BODY_LEN);
        let p =
            PushPlain::notification(d, [1u8; 16], 0, 0, &title, &body, w, t).unwrap();
        let enc = p.encode().unwrap();
        assert_eq!(FIXED_LEN + title.len() + body.len(), 919);
        assert_eq!(decode(&enc).unwrap(), p);
        // Maximum dismissal: 58 ids, empty text.
        let ids58: Vec<[u8; 16]> = (0..58u8).map(|i| [i; 16]).collect();
        let p = PushPlain::dismissal(d, [2u8; 16], 0, 0, w, t, ids58.clone()).unwrap();
        let enc = p.encode().unwrap();
        assert_eq!(FIXED_LEN + 58 * 16, 1007);
        let back = decode(&enc).unwrap();
        assert!(back.dismiss && back.title.is_empty() && back.body.is_empty());
        assert_eq!(back.dismiss_ids, ids58);
    }

    #[test]
    fn over_limits_rejected() {
        let (d, w, t) = ids();
        assert_eq!(
            PushPlain::notification(d, [0u8; 16], 0, 0, &"a".repeat(241), "", w, t).unwrap_err(),
            PushError::TitleTooLong { len: 241 }
        );
        assert_eq!(
            PushPlain::notification(d, [0u8; 16], 0, 0, "", &"b".repeat(601), w, t).unwrap_err(),
            PushError::BodyTooLong { len: 601 }
        );
        assert_eq!(
            PushPlain::dismissal(d, [0u8; 16], 0, 0, w, t, vec![[0u8; 16]; 59]).unwrap_err(),
            PushError::TooManyDismiss { count: 59 }
        );
        // dismiss = 0 with ids; dismiss = 1 with text; bad version/dismiss.
        let mut enc = PushPlain::notification(d, [0u8; 16], 0, 0, "T", "B", w, t)
            .unwrap()
            .encode()
            .unwrap();
        enc[80] = 1;
        enc[81..97].fill(9);
        assert_eq!(decode(&enc).unwrap_err(), PushError::BadDismissShape);
        // dismiss = 1 with title text.
        let mut enc_show = PushPlain::notification(d, [0u8; 16], 0, 0, "X", "", w, t)
            .unwrap()
            .encode()
            .unwrap();
        enc_show[42] = 1;
        assert_eq!(decode(&enc_show).unwrap_err(), PushError::BadDismissShape);
        let mut enc = PushPlain::notification(d, [0u8; 16], 0, 0, "", "", w, t)
            .unwrap()
            .encode()
            .unwrap();
        enc[0] = 2;
        assert_eq!(decode(&enc).unwrap_err(), PushError::BadVersion { version: 2 });
        enc[0] = 1;
        enc[42] = 7;
        assert_eq!(decode(&enc).unwrap_err(), PushError::BadDismiss { dismiss: 7 });
        enc[42] = 0;
        enc[1023] = 1;
        assert_eq!(decode(&enc).unwrap_err(), PushError::BadPadding);
        assert_eq!(decode(&enc[..100]).unwrap_err(), PushError::BadLength { len: 100 });
        // One byte past the fixed size is rejected: no trailing bytes.
        let mut long = enc.to_vec();
        long.push(0);
        assert_eq!(decode(&long).unwrap_err(), PushError::BadLength { len: 1025 });
        // Invalid UTF-8 in the title (body_len zeroed at its shifted spot).
        enc[1023] = 0;
        enc[43] = 2;
        enc[44] = 0xff;
        enc[45] = 0xfe;
        enc[46] = 0;
        enc[47] = 0;
        assert_eq!(decode(&enc).unwrap_err(), PushError::BadUtf8);
    }

    #[test]
    fn truncate_to_cuts_on_char_boundaries() {
        assert_eq!(truncate_to("hello", 10), "hello");
        // "é" is 2 bytes; a 3-byte cut keeps one "é", not one and a half.
        assert_eq!(truncate_to("éé", 3), "é");
        assert_eq!(truncate_to("éé", 4), "éé");
        // Emoji (4 bytes) never splits.
        assert_eq!(truncate_to("a🎉b", 5), "a🎉");
    }
}
