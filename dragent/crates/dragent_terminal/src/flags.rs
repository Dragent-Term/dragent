//! Cell attribute flags (SGR + wide-char layout metadata).
//!
//! Using a compact `u16` bitset instead of individual bools saves ~6 bytes per
//! cell and lets flag manipulation compile to single `AND`/`OR`/`XOR` machine
//! instructions. The `bitflags!` macro provides a type-safe wrapper.

bitflags::bitflags! {
    /// Attribute flags stored in every terminal [`Cell`](crate::cell::Cell).
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct Flags: u16 {
        // ── SGR attribute flags ─────────────────────────────────────────────
        /// SGR 1 — bold / increased intensity.
        const BOLD              = 1 << 0;
        /// SGR 2 — faint / decreased intensity.
        const DIM               = 1 << 1;
        /// SGR 3 — italic.
        const ITALIC            = 1 << 2;
        /// SGR 4 — single underline.
        const UNDERLINE         = 1 << 3;
        /// SGR 21 — doubly-underlined.
        const DOUBLE_UNDERLINE  = 1 << 4;
        /// SGR 4:3 (kitty extension) — wavy underline (undercurl).
        const UNDERCURL         = 1 << 5;
        /// SGR 7 — reverse video.
        const INVERSE           = 1 << 6;
        /// SGR 8 — concealed (hidden).
        const HIDDEN            = 1 << 7;
        /// SGR 9 — strikethrough.
        const STRIKEOUT         = 1 << 8;
        /// SGR 5 — slowly blinking text.
        const BLINKING          = 1 << 9;
        /// SGR 53 — overline.
        const OVERLINE          = 1 << 10;

        // ── Wide-char layout flags ───────────────────────────────────────────
        /// This cell holds a wide (fullwidth) Unicode character occupying two
        /// columns. The *next* cell must have `WIDE_CHAR_SPACER` set.
        const WIDE_CHAR         = 1 << 11;
        /// Placeholder cell that the preceding wide character spills into.
        /// The character value is a space; the style mirrors the wide cell.
        const WIDE_CHAR_SPACER  = 1 << 12;

        // ── Layout / reflow flag ─────────────────────────────────────────────
        /// The row ended by autowrap (not a hard newline). Used for line
        /// reflow when the terminal is resized.
        const WRAPLINE          = 1 << 13;

        // ── Extra data flag ──────────────────────────────────────────────────
        /// The `Cell.extra` box is populated (grapheme cluster / hyperlink /
        /// underline colour / prompt marker). Set automatically by `Cell`.
        const HAS_EXTRA         = 1 << 14;

        // bit 15 reserved for future use
    }
}

impl Flags {
    /// All SGR attribute flags (excludes layout / extra flags).
    pub const SGR_MASK: Self = Self::from_bits_truncate(
        Self::BOLD.bits()
            | Self::DIM.bits()
            | Self::ITALIC.bits()
            | Self::UNDERLINE.bits()
            | Self::DOUBLE_UNDERLINE.bits()
            | Self::UNDERCURL.bits()
            | Self::INVERSE.bits()
            | Self::HIDDEN.bits()
            | Self::STRIKEOUT.bits()
            | Self::BLINKING.bits()
            | Self::OVERLINE.bits(),
    );
}
