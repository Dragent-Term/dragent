//! Incremental VT escape-sequence parser (ECMA-48 / xterm / kitty extensions).
//!
//! Processes the byte stream from a PTY master one byte at a time and
//! dispatches decoded actions to a [`VtActor`].
//!
//! # Extensions over the original parser
//! - **APC** (`ESC _ … ST`): parsed and forwarded via `VtActor::apc`.
//!   Used by the Kitty Graphics Protocol (`ESC _ G … ST`).
//! - **DCS** (`ESC P … ST`): parsed and forwarded via `VtActor::dcs`.
//!   Used by sixel graphics and tmux passthrough.
//! - **PM / SOS** (`ESC ^ … ST`, `ESC X … ST`): still discarded (rarely used).
//! - The `>` private marker (CSI `>` …) is now handled alongside `?`, `<`,
//!   and `=`. Used by the kitty keyboard query protocol.

/// Receiver for parsed terminal actions.
pub trait VtActor {
    /// A printable character in the ground state (may be multi-byte Unicode).
    fn print(&mut self, ch: char);
    /// A C0 control byte (BS, HT, LF, CR, BEL, …).
    fn control(&mut self, byte: u8);
    /// A completed CSI sequence.
    ///
    /// - `params`       — numeric parameters (0 = omitted / default)
    /// - `private`      — optional private marker byte (`?`, `>`, `<`, `=`)
    /// - `intermediates`— intermediate bytes before the final byte
    /// - `action`       — the final byte (`m`, `H`, `h`, `l`, …)
    fn csi(&mut self, params: &[u16], private: Option<u8>, intermediates: &[u8], action: u8);
    /// A completed non-CSI escape sequence (`ESC <intermediate> <final>`).
    fn esc(&mut self, intermediates: &[u8], byte: u8);
    /// A completed OSC string (payload between `ESC ]` and BEL / ST).
    fn osc(&mut self, payload: &[u8]);
    /// A completed APC string (`ESC _ … ST`). Default: discard.
    fn apc(&mut self, _payload: &[u8]) {}
    /// A completed DCS string (`ESC P … ST`). Default: discard.
    fn dcs(&mut self, _payload: &[u8]) {}
}

// ─────────────────────────────────────────────────────────────────────────────
// State machine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    CsiIgnore,
    /// Accumulating an OSC string payload (keep = true).
    Osc,
    /// Accumulating an APC string payload (keep = true).
    Apc,
    /// Accumulating a DCS string payload (keep = true).
    Dcs,
    /// Collecting a string and just saw ESC (possible ST = ESC \).
    StringEsc,
    /// Consuming a PM/SOS string (discard).
    StringIgnore,
}

/// Which type of string is being accumulated (so `finish_string` dispatches
/// correctly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum StringKind {
    Osc,
    Apc,
    Dcs,
    Ignore,
}

const MAX_PARAMS: usize = 32;
const MAX_STRING_LEN: usize = 1024 * 64; // 64 KiB for APC image chunks
const MAX_INTERMEDIATES: usize = 4;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Parser {
    state: State,
    string_kind: StringKind,
    params: Vec<u16>,
    param_started: bool,
    private: Option<u8>,
    intermediates: Vec<u8>,
    string_buf: Vec<u8>,
    utf8_buf: Vec<u8>,
    utf8_remaining: usize,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// True at a byte boundary that a fresh parser can resume from.
    pub fn is_ground(&self) -> bool {
        self.state == State::Ground && self.utf8_remaining == 0 && self.utf8_buf.is_empty()
    }

    /// VT prefix that recreates the pending parser state on a fresh parser.
    /// Already executed C0 controls are deliberately absent. Replaying the raw
    /// unfinished sequence would execute them twice after a display seed.
    /// The prefix is bounded by the parser's existing parameter/string limits.
    pub fn pending_prefix(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self.state {
            State::Ground => out.extend_from_slice(&self.utf8_buf),
            State::Escape => out.push(0x1b),
            State::EscapeIntermediate => {
                out.push(0x1b);
                out.extend_from_slice(&self.intermediates);
            }
            State::Csi => {
                out.extend_from_slice(b"\x1b[");
                if let Some(private) = self.private {
                    out.push(private);
                }
                for (i, param) in self.params.iter().enumerate() {
                    if i > 0 {
                        out.push(b';');
                    }
                    out.extend_from_slice(param.to_string().as_bytes());
                }
                if !self.param_started && !self.params.is_empty() {
                    out.push(b';');
                }
                out.extend_from_slice(&self.intermediates);
            }
            State::CsiIgnore => out.extend_from_slice(b"\x1b[??"),
            State::Osc | State::Apc | State::Dcs | State::StringEsc | State::StringIgnore => {
                out.extend_from_slice(match self.string_kind {
                    StringKind::Osc => b"\x1b]",
                    StringKind::Apc => b"\x1b_",
                    StringKind::Dcs => b"\x1bP",
                    StringKind::Ignore => b"\x1b^",
                });
                out.extend_from_slice(&self.string_buf);
                if self.state == State::StringEsc {
                    out.push(0x1b);
                }
            }
        }
        out
    }

    /// Validate a checkpoint received during a daemon handoff before use.
    pub fn valid_checkpoint(&self) -> bool {
        self.params.len() <= MAX_PARAMS
            && self.intermediates.len() <= MAX_INTERMEDIATES
            && self.string_buf.len() <= MAX_STRING_LEN
            && self.utf8_buf.len() <= 4
            && self.utf8_remaining <= 3
            && self.utf8_buf.len() + self.utf8_remaining <= 4
    }

    pub fn new() -> Self {
        Self {
            state: State::Ground,
            string_kind: StringKind::Ignore,
            params: Vec::new(),
            param_started: false,
            private: None,
            intermediates: Vec::new(),
            string_buf: Vec::new(),
            utf8_buf: Vec::new(),
            utf8_remaining: 0,
        }
    }

    pub fn feed(&mut self, bytes: &[u8], actor: &mut impl VtActor) {
        for &byte in bytes {
            self.step(byte, actor);
        }
    }

    fn step(&mut self, byte: u8, actor: &mut impl VtActor) {
        match self.state {
            State::Ground => self.ground(byte, actor),
            State::Escape => self.escape(byte, actor),
            State::EscapeIntermediate => self.escape_intermediate(byte, actor),
            State::Csi => self.csi(byte, actor),
            State::CsiIgnore => self.csi_ignore(byte),
            State::Osc => self.string_body(byte, actor, true),
            State::Apc => self.string_body(byte, actor, true),
            State::Dcs => self.string_body(byte, actor, true),
            State::StringIgnore => self.string_body(byte, actor, false),
            State::StringEsc => self.string_esc(byte, actor),
        }
    }

    // ── Ground state ─────────────────────────────────────────────────────────

    fn ground(&mut self, byte: u8, actor: &mut impl VtActor) {
        // Complete a multi-byte UTF-8 sequence.
        if self.utf8_remaining > 0 {
            if byte & 0xc0 == 0x80 {
                self.utf8_buf.push(byte);
                self.utf8_remaining -= 1;
                if self.utf8_remaining == 0 {
                    let ch = std::str::from_utf8(&self.utf8_buf)
                        .ok()
                        .and_then(|s| s.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    actor.print(ch);
                    self.utf8_buf.clear();
                }
                return;
            }
            // Bad continuation: emit replacement, reprocess byte below.
            self.utf8_buf.clear();
            self.utf8_remaining = 0;
            actor.print(char::REPLACEMENT_CHARACTER);
        }

        match byte {
            0x1b => self.enter_escape(),
            0x00..=0x1f | 0x7f => actor.control(byte),
            0x20..=0x7e => actor.print(byte as char),
            0xc2..=0xdf => self.start_utf8(byte, 1),
            0xe0..=0xef => self.start_utf8(byte, 2),
            0xf0..=0xf4 => self.start_utf8(byte, 3),
            _ => actor.print(char::REPLACEMENT_CHARACTER),
        }
    }

    fn start_utf8(&mut self, byte: u8, continuation: usize) {
        self.utf8_buf.clear();
        self.utf8_buf.push(byte);
        self.utf8_remaining = continuation;
    }

    fn enter_escape(&mut self) {
        self.state = State::Escape;
        self.intermediates.clear();
    }

    // ── Escape state ─────────────────────────────────────────────────────────

    fn escape(&mut self, byte: u8, actor: &mut impl VtActor) {
        match byte {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.param_started = false;
                self.private = None;
                self.intermediates.clear();
            }
            b']' => self.enter_string(State::Osc, StringKind::Osc),
            b'_' => self.enter_string(State::Apc, StringKind::Apc),
            b'P' => self.enter_string(State::Dcs, StringKind::Dcs),
            // SOS / PM: discard.
            b'X' | b'^' => self.enter_string(State::StringIgnore, StringKind::Ignore),
            0x20..=0x2f => {
                self.intermediates.push(byte);
                self.state = State::EscapeIntermediate;
            }
            0x1b => self.enter_escape(),
            0x00..=0x1a | 0x1c..=0x1f => actor.control(byte),
            _ => {
                self.state = State::Ground;
                actor.esc(&[], byte);
            }
        }
    }

    fn enter_string(&mut self, state: State, kind: StringKind) {
        self.state = state;
        self.string_kind = kind;
        self.string_buf.clear();
    }

    // ── EscapeIntermediate state ──────────────────────────────────────────────

    fn escape_intermediate(&mut self, byte: u8, actor: &mut impl VtActor) {
        match byte {
            0x20..=0x2f => {
                if self.intermediates.len() < MAX_INTERMEDIATES {
                    self.intermediates.push(byte);
                }
            }
            0x1b => self.enter_escape(),
            0x30..=0x7e => {
                self.state = State::Ground;
                let intermediates = std::mem::take(&mut self.intermediates);
                actor.esc(&intermediates, byte);
            }
            _ => {
                self.state = State::Ground;
            }
        }
    }

    // ── CSI state ────────────────────────────────────────────────────────────

    fn csi(&mut self, byte: u8, actor: &mut impl VtActor) {
        match byte {
            b'0'..=b'9' => {
                if !self.param_started {
                    if self.params.len() >= MAX_PARAMS {
                        self.state = State::CsiIgnore;
                        return;
                    }
                    self.params.push(0);
                    self.param_started = true;
                }
                if let Some(last) = self.params.last_mut() {
                    *last = last.saturating_mul(10).saturating_add((byte - b'0') as u16);
                }
            }
            b';' | b':' => {
                if !self.param_started {
                    self.params.push(0);
                }
                self.param_started = false;
                if self.params.len() >= MAX_PARAMS {
                    self.state = State::CsiIgnore;
                }
            }
            b'?' | b'>' | b'<' | b'=' => {
                if self.params.is_empty() && self.private.is_none() {
                    self.private = Some(byte);
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x20..=0x2f => {
                if self.intermediates.len() < MAX_INTERMEDIATES {
                    self.intermediates.push(byte);
                } else {
                    self.state = State::CsiIgnore;
                }
            }
            0x40..=0x7e => {
                self.state = State::Ground;
                let params = std::mem::take(&mut self.params);
                let intermediates = std::mem::take(&mut self.intermediates);
                actor.csi(&params, self.private, &intermediates, byte);
            }
            0x1b => self.enter_escape(),
            0x00..=0x1a | 0x1c..=0x1f => actor.control(byte),
            _ => self.state = State::CsiIgnore,
        }
    }

    fn csi_ignore(&mut self, byte: u8) {
        match byte {
            0x40..=0x7e => self.state = State::Ground,
            0x1b => self.enter_escape(),
            _ => {}
        }
    }

    // ── String body (OSC / APC / DCS / Ignore) ────────────────────────────────

    fn string_body(&mut self, byte: u8, actor: &mut impl VtActor, keep: bool) {
        match byte {
            // BEL terminates OSC strings (xterm extension — APC/DCS use ST only).
            0x07 if matches!(self.string_kind, StringKind::Osc) => {
                self.finish_string(actor);
            }
            0x1b => {
                self.state = State::StringEsc;
            }
            // CAN / SUB abort strings.
            0x18 | 0x1a => {
                self.string_buf.clear();
                self.state = State::Ground;
            }
            // Other C0 controls inside strings: ignore (do NOT execute).
            0x00..=0x1f => {}
            _ => {
                if keep && self.string_buf.len() < MAX_STRING_LEN {
                    self.string_buf.push(byte);
                }
            }
        }
    }

    fn string_esc(&mut self, byte: u8, actor: &mut impl VtActor) {
        if byte == b'\\' {
            // ESC \ = ST.
            self.finish_string(actor);
        } else {
            // Not an ST: abort string and reprocess as an escape.
            self.string_buf.clear();
            self.enter_escape();
            self.escape(byte, actor);
        }
    }

    fn finish_string(&mut self, actor: &mut impl VtActor) {
        let payload = std::mem::take(&mut self.string_buf);
        match self.string_kind {
            StringKind::Osc => actor.osc(&payload),
            StringKind::Apc => actor.apc(&payload),
            StringKind::Dcs => actor.dcs(&payload),
            StringKind::Ignore => {}
        }
        self.state = State::Ground;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Recorder {
        printed: String,
        controls: Vec<u8>,
        csis: Vec<(Vec<u16>, Option<u8>, Vec<u8>, u8)>,
        escs: Vec<(Vec<u8>, u8)>,
        oscs: Vec<Vec<u8>>,
        apcs: Vec<Vec<u8>>,
        dcss: Vec<Vec<u8>>,
    }

    #[test]
    fn pending_prefix_resumes_every_split_without_replaying_controls() {
        let cases: &[&[u8]] = &[
            b"\x1b[31mred",
            b"\x1b[;31;1mred",
            b"\x1b[4:3mcurly",
            b"\x1b[38;2;999999999;1;2mtext",
            b"\x1b[3\n1mtext",
            b"\x1b\r[31mtext",
            b"\x1b[?1003h",
            b"\x1b[1 q",
            b"\x1b(0",
            b"\x1b[??123Xtail",
            b"\x1b]2;some title\x07tail",
            b"\x1b]2;other title\x1b\\tail",
            b"\x1bPqdata\x1b\\tail",
            b"\x1b_Gdata\x1b\\tail",
            b"\x1b^ignored\x1b\\tail",
            b"\x1b]aborted\x1b[31mtext",
            "中🙂e\u{301}".as_bytes(),
        ];
        for bytes in cases {
            for split in 0..=bytes.len() {
                let mut original = Parser::new();
                original.feed(&bytes[..split], &mut Recorder::default());
                let prefix = original.pending_prefix();
                assert_eq!(original.is_ground(), prefix.is_empty());
                let mut restored = Parser::new();
                let mut replayed = Recorder::default();
                restored.feed(&prefix, &mut replayed);
                assert_eq!(replayed, Recorder::default(), "prefix performed actions");
                assert_eq!(restored.is_ground(), original.is_ground());
                let mut expected = Recorder::default();
                original.feed(&bytes[split..], &mut expected);
                restored.feed(&bytes[split..], &mut replayed);
                assert_eq!(replayed, expected, "{bytes:?} split at {split}");
                assert!(restored.is_ground());
            }
        }
    }

    #[test]
    fn pending_prefix_is_bounded_for_oversized_sequences() {
        for start in [b"\x1bP".as_slice(), b"\x1b]", b"\x1b[", b"\x1b("] {
            let mut parser = Parser::new();
            let mut actor = Recorder::default();
            parser.feed(start, &mut actor);
            parser.feed(&vec![b'1'; MAX_STRING_LEN * 2], &mut actor);
            assert!(parser.pending_prefix().len() <= MAX_STRING_LEN + 3);
        }
    }

    impl VtActor for Recorder {
        fn print(&mut self, ch: char) {
            self.printed.push(ch);
        }
        fn control(&mut self, b: u8) {
            self.controls.push(b);
        }
        fn csi(&mut self, p: &[u16], priv_: Option<u8>, inter: &[u8], act: u8) {
            self.csis.push((p.to_vec(), priv_, inter.to_vec(), act));
        }
        fn esc(&mut self, inter: &[u8], b: u8) {
            self.escs.push((inter.to_vec(), b));
        }
        fn osc(&mut self, payload: &[u8]) {
            self.oscs.push(payload.to_vec());
        }
        fn apc(&mut self, payload: &[u8]) {
            self.apcs.push(payload.to_vec());
        }
        fn dcs(&mut self, payload: &[u8]) {
            self.dcss.push(payload.to_vec());
        }
    }

    #[test]
    fn prints_utf8() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed("héllo ✓".as_bytes(), &mut r);
        assert_eq!(r.printed, "héllo ✓");
    }

    #[test]
    fn parses_sgr_csi() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"\x1b[1;32mok\x1b[0m", &mut r);
        assert_eq!(r.printed, "ok");
        assert_eq!(r.csis.len(), 2);
        assert_eq!(r.csis[0], (vec![1, 32], None, vec![], b'm'));
    }

    #[test]
    fn parses_private_mode_csi() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"\x1b[?1049h", &mut r);
        assert_eq!(r.csis[0], (vec![1049], Some(b'?'), vec![], b'h'));
    }

    #[test]
    fn parses_osc_bel_and_st() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"\x1b]0;title\x07after\x1b]2;t2\x1b\\", &mut r);
        assert_eq!(r.oscs, vec![b"0;title".to_vec(), b"2;t2".to_vec()]);
        assert_eq!(r.printed, "after");
    }

    #[test]
    fn parses_apc_kitty_graphics() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        // ESC _ G a=q,i=1 ; <data> ST
        p.feed(b"\x1b_Ga=q,i=1;aGVsbG8=\x1b\\", &mut r);
        assert_eq!(r.apcs.len(), 1);
        assert_eq!(r.apcs[0], b"Ga=q,i=1;aGVsbG8=".to_vec());
    }

    #[test]
    fn parses_dcs() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"\x1bPsome dcs\x1b\\visible", &mut r);
        assert_eq!(r.dcss.len(), 1);
        assert_eq!(r.printed, "visible");
    }

    #[test]
    fn handles_split_feeds() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"\x1b[3", &mut r);
        p.feed(b"8;5;1", &mut r);
        p.feed(b"2mX", &mut r);
        assert_eq!(r.csis[0], (vec![38, 5, 12], None, vec![], b'm'));
        assert_eq!(r.printed, "X");
    }

    #[test]
    fn controls_in_ground() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        p.feed(b"a\r\nb\x08", &mut r);
        assert_eq!(r.printed, "ab");
        assert_eq!(r.controls, vec![0x0d, 0x0a, 0x08]);
    }

    #[test]
    fn csi_gt_kitty_keyboard() {
        let mut p = Parser::new();
        let mut r = Recorder::default();
        // CSI > 0 u — kitty keyboard query
        p.feed(b"\x1b[>0u", &mut r);
        assert_eq!(r.csis[0], (vec![0], Some(b'>'), vec![], b'u'));
    }
}
