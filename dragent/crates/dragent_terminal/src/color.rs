//! Terminal color representation.
//!
//! Supports the full xterm/VT color space:
//! - Default foreground / background (resolved by the renderer from the theme)
//! - 16 named ANSI colors (dark + bright variants)
//! - 256-color indexed palette (xterm256)
//! - 24-bit direct RGB (SGR 38;2;r;g;b / 48;2;r;g;b)

/// The 16 standard ANSI named colors plus terminal-default pseudo-colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum NamedColor {
    // Dark palette (indices 0–7)
    Black = 0,
    Red = 1,
    Green = 2,
    Yellow = 3,
    Blue = 4,
    Magenta = 5,
    Cyan = 6,
    White = 7,
    // Bright palette (indices 8–15)
    BrightBlack = 8,
    BrightRed = 9,
    BrightGreen = 10,
    BrightYellow = 11,
    BrightBlue = 12,
    BrightMagenta = 13,
    BrightCyan = 14,
    BrightWhite = 15,
}

impl NamedColor {
    /// The xterm 256-color palette index for this named color.
    #[inline]
    pub fn index(self) -> u8 {
        self as u8
    }

    /// Convert a raw 0–15 ANSI index into the corresponding `NamedColor`.
    pub fn from_index(idx: u8) -> Option<Self> {
        match idx {
            0 => Some(Self::Black),
            1 => Some(Self::Red),
            2 => Some(Self::Green),
            3 => Some(Self::Yellow),
            4 => Some(Self::Blue),
            5 => Some(Self::Magenta),
            6 => Some(Self::Cyan),
            7 => Some(Self::White),
            8 => Some(Self::BrightBlack),
            9 => Some(Self::BrightRed),
            10 => Some(Self::BrightGreen),
            11 => Some(Self::BrightYellow),
            12 => Some(Self::BrightBlue),
            13 => Some(Self::BrightMagenta),
            14 => Some(Self::BrightCyan),
            15 => Some(Self::BrightWhite),
            _ => None,
        }
    }

    /// Whether this is one of the bright (high-intensity) variants.
    #[inline]
    pub fn is_bright(self) -> bool {
        (self as u8) >= 8
    }
}

/// A terminal color value.
///
/// The variants are kept wire-compatible with the original `Color` enum so
/// that existing callers (`block_store.rs` etc.) require no changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Color {
    /// Use the renderer's default foreground / background. This is the value
    /// in an un-styled cell.
    #[default]
    Default,
    /// One of the 16 standard named ANSI colors.
    Named(NamedColor),
    /// xterm 256-color index (0–255; 0–15 overlap with named, 16–231 are the
    /// 6×6×6 cube, 232–255 are the grayscale ramp).
    Indexed(u8),
    /// 24-bit direct RGB.
    Rgb(u8, u8, u8),
}

impl Color {
    /// Convert a named color to `Color::Indexed` for renderers that only
    /// handle the indexed palette.
    #[inline]
    pub fn to_indexed(self) -> Self {
        match self {
            Self::Named(n) => Self::Indexed(n.index()),
            other => other,
        }
    }

    /// Return true when this color is the un-styled default.
    #[inline]
    pub fn is_default(self) -> bool {
        matches!(self, Self::Default)
    }
}
