//! Terminal mode flags (`TermMode`).
//!
//! A single `u32` bitset replaces the scattered `bool` fields that tracked
//! individual DEC private modes in the emulator. This matches common terminal emulator practice
//! and adds several modes that many emulators also support (`APP_CURSOR`, `APP_KEYPAD`,
//! `ORIGIN`, `INSERT`, `VI`, `URGENCY_HINTS`, `ALTERNATE_SCROLL`) plus the
//! full Kitty keyboard enhancement flags (5 bits).
//!
//! # Usage
//! ```rust,ignore
//! // Test a mode
//! if mode.contains(TermMode::APP_CURSOR) { ... }
//! // Toggle a mode
//! mode.set_flag(TermMode::BRACKETED_PASTE, true);
//! // Reset all modes to default
//! let mode = TermMode::DEFAULT;
//! ```

bitflags::bitflags! {
    /// Bitset of active DEC private modes and Dragent-specific terminal flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct TermMode: u32 {
        // ── Cursor ────────────────────────────────────────────────────────────
        /// DEC mode 25 (`?25h/l`) — cursor visible.
        const SHOW_CURSOR            = 1 << 0;

        /// DEC mode 1 (`?1h/l`) — application cursor keys.
        ///
        /// When active, arrow keys send `SS3 A/B/C/D` (`\x1bO…`) instead
        /// of `CSI A/B/C/D` (`\x1b[…`). The host encodes keyboard input
        /// differently based on this mode.
        const APP_CURSOR             = 1 << 1;

        // ── Keypad ────────────────────────────────────────────────────────────
        /// DEC mode (DECKPAM/DECKPNM via `ESC =` / `ESC >`) — application
        /// keypad mode. Numeric keys send escape sequences instead of digits.
        const APP_KEYPAD             = 1 << 2;

        // ── Line behaviour ────────────────────────────────────────────────────
        /// DEC mode 7 (`?7h/l`) — auto-wrap mode (DECAWM).
        ///
        /// When set (default) the cursor wraps to the next line after reaching
        /// the right margin. When clear the cursor stays at the last column.
        const LINE_WRAP              = 1 << 3;

        /// DEC mode 20 (`?20h/l`) — new-line / line-feed mode (LNM).
        ///
        /// When set, LF / VT / FF also perform a carriage return.
        const LINE_FEED_NEW_LINE     = 1 << 4;

        /// DEC mode 6 (`?6h/l`) — origin mode (DECOM).
        ///
        /// When set, cursor addresses are relative to the scroll-region top.
        const ORIGIN                 = 1 << 5;

        /// DEC mode 4 (`?4h/l`) — insert mode (IRM).
        ///
        /// When set, printed characters are inserted and existing characters
        /// are shifted right rather than being overwritten.
        const INSERT                 = 1 << 6;

        // ── Screen ────────────────────────────────────────────────────────────
        /// Alternate screen is currently active (modes 47 / 1047 / 1049).
        const ALT_SCREEN             = 1 << 7;

        /// DEC mode 1007 — alternate scroll (ESC [ ? 1007 h/l).
        ///
        /// In alt-screen mode, scroll-wheel events send arrow keys instead of
        /// scrolling the history.
        const ALTERNATE_SCROLL       = 1 << 8;

        // ── Input ─────────────────────────────────────────────────────────────
        /// DEC mode 2004 — bracketed paste.
        const BRACKETED_PASTE        = 1 << 9;

        /// DEC mode 1004 — focus tracking (FocusIn / FocusOut events).
        const FOCUS_IN_OUT           = 1 << 10;

        // ── Mouse ─────────────────────────────────────────────────────────────
        /// DEC mode 1000 — report mouse button press/release (Normal mode).
        const MOUSE_REPORT_CLICK     = 1 << 11;

        /// DEC mode 1002 — report mouse button events + motion while held.
        const MOUSE_DRAG             = 1 << 12;

        /// DEC mode 1003 — report all mouse motion (hover).
        const MOUSE_MOTION           = 1 << 13;

        /// DEC mode 1006 — SGR mouse encoding (unbounded coordinates).
        const SGR_MOUSE              = 1 << 14;

        /// DEC mode 1015 — URXVT decimal mouse encoding.
        const URXVT_MOUSE            = 1 << 15;

        /// Union of all mouse-mode bits (used to clear when switching modes).
        const MOUSE_MODE = Self::MOUSE_REPORT_CLICK.bits()
            | Self::MOUSE_DRAG.bits()
            | Self::MOUSE_MOTION.bits();

        // ── Extended protocols ────────────────────────────────────────────────
        /// DEC mode 2026 — synchronized output (batch rendering).
        const SYNCHRONIZED_OUTPUT    = 1 << 16;

        /// DEC mode 2048 — Kitty keyboard protocol (at least one flag active).
        const KITTY_KEYBOARD         = 1 << 17;

        // ── Kitty keyboard enhancement flags ──────────────────────────────────
        // https://sw.kovidgoyal.net/kitty/keyboard-protocol/#progressive-enhancement

        /// Flag 1: Disambiguate escape codes.
        const KEYBOARD_DISAMBIGUATE_ESCAPE    = 1 << 18;

        /// Flag 2: Report key press, repeat and release events.
        const KEYBOARD_REPORT_EVENT_TYPES     = 1 << 19;

        /// Flag 4: Report alternate (unshifted) keys alongside shifted keys.
        const KEYBOARD_REPORT_ALTERNATE_KEYS  = 1 << 20;

        /// Flag 8: Encode all keys as CSI u sequences (including printable chars).
        const KEYBOARD_REPORT_ALL_AS_ESCAPE   = 1 << 21;

        /// Flag 16: Include the produced text in the CSI u sequence.
        const KEYBOARD_REPORT_ASSOCIATED_TEXT = 1 << 22;

        /// Union of all Kitty keyboard enhancement flags.
        const KEYBOARD_PROTOCOL = Self::KEYBOARD_DISAMBIGUATE_ESCAPE.bits()
            | Self::KEYBOARD_REPORT_EVENT_TYPES.bits()
            | Self::KEYBOARD_REPORT_ALTERNATE_KEYS.bits()
            | Self::KEYBOARD_REPORT_ALL_AS_ESCAPE.bits()
            | Self::KEYBOARD_REPORT_ASSOCIATED_TEXT.bits();

        // ── Application-layer modes ────────────────────────────────────────────
        /// VI / buffer-navigation mode (enabled by the host application, not by
        /// a VT sequence). Signals the UI to apply vi-key bindings for scrollback
        /// navigation instead of forwarding keystrokes to the PTY.
        const VI                     = 1 << 23;

        /// Urgency hints: ring the system bell + raise the window urgency flag
        /// when a BEL is received while the window does not have focus.
        const URGENCY_HINTS          = 1 << 24;

        /// DEC mode 2031 — the app wants to be told when the terminal's colour
        /// scheme flips between dark and light (`CSI ? 997 ; 1|2 n`).
        const COLOR_SCHEME_UPDATES   = 1 << 25;

        // ── Default state ─────────────────────────────────────────────────────
        /// The mode word that a freshly-created terminal starts with.
        ///
        /// - `SHOW_CURSOR` on (cursor visible)
        /// - `LINE_WRAP` on (auto-wrap enabled, VT default)
        const DEFAULT = Self::SHOW_CURSOR.bits() | Self::LINE_WRAP.bits();
    }
}

impl TermMode {
    /// Set or clear a single flag.
    #[inline]
    pub fn set_flag(&mut self, flag: Self, on: bool) {
        if on {
            self.insert(flag);
        } else {
            self.remove(flag);
        }
    }

    /// Active Kitty keyboard enhancement flags as a `u8` bitmask suitable
    /// for encoding `CSI = <flags> ; <mode> u` responses.
    pub fn kitty_keyboard_flags(self) -> u8 {
        let mut f = 0u8;
        if self.contains(Self::KEYBOARD_DISAMBIGUATE_ESCAPE) {
            f |= 1;
        }
        if self.contains(Self::KEYBOARD_REPORT_EVENT_TYPES) {
            f |= 2;
        }
        if self.contains(Self::KEYBOARD_REPORT_ALTERNATE_KEYS) {
            f |= 4;
        }
        if self.contains(Self::KEYBOARD_REPORT_ALL_AS_ESCAPE) {
            f |= 8;
        }
        if self.contains(Self::KEYBOARD_REPORT_ASSOCIATED_TEXT) {
            f |= 16;
        }
        f
    }
}
