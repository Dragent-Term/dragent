//! Bracketed paste (mode 2004) and clipboard normalization.

use dragent_terminal::TermMode;

/// Normalize clipboard text to PTY line endings (`\r`).
pub fn normalize_paste_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r")
}

/// Wrap paste bytes with bracketed-paste CSI when mode 2004 is active.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = normalize_paste_text(text);
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut bytes = Vec::with_capacity(normalized.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(normalized.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_wrap() {
        let bytes = encode_paste("hello\nworld", TermMode::BRACKETED_PASTE);
        assert_eq!(bytes, b"\x1b[200~hello\rworld\x1b[201~".as_slice());
    }

    #[test]
    fn plain_paste_without_mode() {
        let bytes = encode_paste("a\nb", TermMode::default());
        assert_eq!(bytes, b"a\rb".as_slice());
    }
}
