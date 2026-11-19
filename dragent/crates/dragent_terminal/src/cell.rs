//! Cell, style and span primitives for the terminal grid.
//!
//! # Design
//! Each grid cell is 24 bytes:
//!   - `ch`    : 4 B  (Unicode scalar value)
//!   - `fg/bg` : 4 B each (Color enum)
//!   - `flags` : 2 B  (Flags bitset)
//!   - `_pad`  : 2 B
//!   - `extra` : 8 B  (fat pointer — None in the common case)
//!
//! Grapheme clusters, OSC-8 hyperlinks, underline color, and shell-integration
//! markers are heap-allocated via `CellExtra` and only present when needed.

use crate::color::Color;
use crate::flags::Flags;
use crate::shell_integration::SemanticMarker;

// ─────────────────────────────────────────────────────────────────────────────
// CellExtra — heap extension for uncommon attributes
// ─────────────────────────────────────────────────────────────────────────────

/// Heap-allocated extension data for a [`Cell`].
///
/// Allocated only when any of the fields is non-empty/non-None, so the common
/// case (plain ASCII) pays no allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CellExtra {
    /// Zero-width combining characters that visually modify `Cell::ch`.
    /// e.g. a combining tilde turns 'n' into 'ñ' without increasing width.
    pub grapheme_cluster: smallvec::SmallVec<[char; 4]>,
    /// 1-based ID into the emulator's hyperlink table (OSC 8).  0 = no link.
    pub hyperlink_id: u32,
    /// Underline color set via SGR 58 (kitty extension).
    pub underline_color: Option<Color>,
    /// Shell-integration prompt boundary marker (OSC 133 / OSC 7979).
    pub prompt_marker: Option<SemanticMarker>,
}

impl Default for CellExtra {
    fn default() -> Self {
        Self {
            grapheme_cluster: smallvec::SmallVec::new(),
            hyperlink_id: 0,
            underline_color: None,
            prompt_marker: None,
        }
    }
}

impl CellExtra {
    fn is_empty(&self) -> bool {
        self.grapheme_cluster.is_empty()
            && self.hyperlink_id == 0
            && self.underline_color.is_none()
            && self.prompt_marker.is_none()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cell
// ─────────────────────────────────────────────────────────────────────────────

/// One character cell in the terminal grid.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Cell {
    /// The Unicode codepoint displayed in this cell (space for blank / spacer
    /// cells).
    pub ch: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// SGR and layout attribute flags.
    pub flags: Flags,
    extra: Option<Box<CellExtra>>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::empty(),
            extra: None,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Self) -> bool {
        self.ch == other.ch
            && self.fg == other.fg
            && self.bg == other.bg
            && self.flags == other.flags
            && self.extra == other.extra
    }
}
impl Eq for Cell {}

impl Cell {
    /// A blank cell with the given foreground/background colors.
    #[inline]
    pub fn blank(fg: Color, bg: Color) -> Self {
        Self {
            ch: ' ',
            fg,
            bg,
            flags: Flags::empty(),
            extra: None,
        }
    }

    /// Construct from a character and a [`Style`].
    #[inline]
    pub fn from_char_style(ch: char, style: Style) -> Self {
        let mut cell = Self {
            ch,
            fg: style.fg,
            bg: style.bg,
            flags: style.flags,
            extra: None,
        };
        if let Some(uc) = style.underline_color {
            cell.extra_mut().underline_color = Some(uc);
        }
        cell
    }

    // ── Wide-char helpers ────────────────────────────────────────────────────

    #[inline]
    pub fn is_wide(&self) -> bool {
        self.flags.contains(Flags::WIDE_CHAR)
    }

    #[inline]
    pub fn is_wide_spacer(&self) -> bool {
        self.flags.contains(Flags::WIDE_CHAR_SPACER)
    }

    /// Display width: 2 for wide chars, 1 otherwise.
    #[inline]
    pub fn display_width(&self) -> usize {
        if self.is_wide() {
            2
        } else {
            1
        }
    }

    // ── Extra data accessors ─────────────────────────────────────────────────

    /// Borrow the extra data, if any.
    #[inline]
    pub fn extra(&self) -> Option<&CellExtra> {
        self.extra.as_deref()
    }

    /// Mutably borrow (or create) the extra data box.
    pub fn extra_mut(&mut self) -> &mut CellExtra {
        if self.extra.is_none() {
            self.extra = Some(Box::default());
            self.flags.insert(Flags::HAS_EXTRA);
        }
        self.extra.as_mut().unwrap()
    }

    /// Drop the extra box if it is now empty (avoids needless heap retention).
    pub fn compact(&mut self) {
        if self.extra.as_deref().is_some_and(CellExtra::is_empty) {
            self.extra = None;
            self.flags.remove(Flags::HAS_EXTRA);
        }
    }

    /// Push a combining codepoint onto the grapheme cluster.
    pub fn push_combining(&mut self, c: char) {
        self.extra_mut().grapheme_cluster.push(c);
    }

    /// The OSC-8 hyperlink ID (0 = no link).
    #[inline]
    pub fn hyperlink_id(&self) -> u32 {
        self.extra.as_deref().map_or(0, |e| e.hyperlink_id)
    }

    pub fn set_hyperlink_id(&mut self, id: u32) {
        if id == 0 {
            if let Some(e) = self.extra.as_mut() {
                e.hyperlink_id = 0;
            }
        } else {
            self.extra_mut().hyperlink_id = id;
        }
    }

    pub fn set_prompt_marker(&mut self, marker: SemanticMarker) {
        self.extra_mut().prompt_marker = Some(marker);
        self.flags.insert(Flags::HAS_EXTRA);
    }

    /// The full grapheme as a `String` (base char + combining chars).
    pub fn grapheme(&self) -> String {
        let mut s = self.ch.to_string();
        if let Some(e) = self.extra.as_deref() {
            for &c in &e.grapheme_cluster {
                s.push(c);
            }
        }
        s
    }

    /// Reset this cell to a blank with the given style (preserves allocation).
    pub fn reset(&mut self, style: Style) {
        self.ch = ' ';
        self.fg = style.fg;
        self.bg = style.bg;
        self.flags = style.flags & !Flags::WIDE_CHAR & !Flags::WIDE_CHAR_SPACER & !Flags::WRAPLINE;
        if let Some(e) = self.extra.as_mut() {
            *e = Box::default();
        }
        if let Some(uc) = style.underline_color {
            self.extra_mut().underline_color = Some(uc);
        }
        self.compact();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Style — the "cursor pen" carried by the emulator
// ─────────────────────────────────────────────────────────────────────────────

/// The current SGR visual style: colors, attributes and optional extras.
///
/// `Style` is the *pen* the emulator cursor writes with. It is `Copy` so the
/// interpreter can snapshot it cheaply per-character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Style {
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
    /// SGR attribute flags (does **not** include layout flags like WIDE_CHAR).
    pub flags: Flags,
    /// Optional underline color (SGR 58 / reset with SGR 59).
    pub underline_color: Option<Color>,
}

impl Style {
    /// A blank style with all attributes cleared.
    pub fn reset() -> Self {
        Self::default()
    }

    /// Convert the SGR attribute flags to a legacy `Attrs` struct for callers
    /// that haven't yet migrated to `Flags`.
    pub fn attrs(&self) -> Attrs {
        Attrs {
            bold: self.flags.contains(Flags::BOLD),
            dim: self.flags.contains(Flags::DIM),
            italic: self.flags.contains(Flags::ITALIC),
            underline: self.flags.contains(Flags::UNDERLINE),
            inverse: self.flags.contains(Flags::INVERSE),
            strikethrough: self.flags.contains(Flags::STRIKEOUT),
            hidden: self.flags.contains(Flags::HIDDEN),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Attrs — legacy compatibility shim
// ─────────────────────────────────────────────────────────────────────────────

/// Legacy attribute struct kept for API compatibility.
///
/// New code should use [`Flags`] and [`Style::flags`] directly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub hidden: bool,
}

impl Attrs {
    /// Convert to a `Flags` bitset.
    pub fn to_flags(self) -> Flags {
        let mut f = Flags::empty();
        if self.bold {
            f.insert(Flags::BOLD);
        }
        if self.dim {
            f.insert(Flags::DIM);
        }
        if self.italic {
            f.insert(Flags::ITALIC);
        }
        if self.underline {
            f.insert(Flags::UNDERLINE);
        }
        if self.inverse {
            f.insert(Flags::INVERSE);
        }
        if self.strikethrough {
            f.insert(Flags::STRIKEOUT);
        }
        if self.hidden {
            f.insert(Flags::HIDDEN);
        }
        f
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StyledSpan / StyledLine — rendering-friendly output
// ─────────────────────────────────────────────────────────────────────────────

/// A run of consecutive characters sharing the same style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
}

/// A display line decomposed into [`StyledSpan`]s (trailing blanks trimmed).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledLine {
    pub spans: Vec<StyledSpan>,
    /// The row ended by auto-wrap: its text continues on the next line with
    /// no real newline between them (a long path at the terminal's width).
    pub wrapped: bool,
}

impl StyledLine {
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }
}
