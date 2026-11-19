//! Dragent terminal emulator core.
//!
//! Models a real VT/xterm-compatible terminal: a 2-D cell grid with cursor,
//! styles, scrollback, alternate screen, and an incremental escape-sequence
//! parser. Also extracts structured events for:
//!
//! - Dragent's native OSC 7979 shell hooks (preexec / precmd / clear)
//! - OSC 133 semantic shell-prompt zones (FinalTerm protocol)
//! - OSC 8 hyperlinks
//! - Kitty Graphics Protocol images (APC G sequences)
//! - Mouse reporting (DEC modes 1000/1002/1003 with SGR / URXVT encoding)
//! - Cursor shape changes (DECSCUSR)
//! - Focus tracking, bracketed paste, synchronized output
//! - Cross-platform audible bell

pub mod bell;
pub mod cell;
pub mod color;
mod emu_actor;
mod emu_events;
mod emu_interpreter;
mod emu_surface;
pub mod emulator;
pub mod flags;
pub mod grid;
pub mod ime;
pub mod kitty_graphics;
pub mod mouse;
pub mod parser;
pub mod operations;
pub mod scrollback;
pub mod shell_integration;
pub mod sixel;
pub mod tab_stops;
pub mod term_mode;

// ── Primary public re-exports ─────────────────────────────────────────────────

// Color — backward-compatible with existing callers (Color::Default / Indexed / Rgb).
pub use color::{Color, NamedColor};

// Cell and style primitives.
pub use cell::{Attrs, Cell, CellExtra, Style, StyledLine, StyledSpan};

// Flags (also accessible as `crate::flags::Flags`).
pub use flags::Flags;

// Grid types.
pub use grid::{Grid, Row};

// Parser types.
pub use parser::{Parser, VtActor};

// Emulator and event types.
pub use emu_surface::Emulator;
pub use emulator::{color_query_report, color_scheme_report, CursorShape, TermEvent, HOOK_OSC_NUMBER};
pub use shell_integration::HookEvent;

// Mouse types.
pub use mouse::{
    MouseButton, MouseEncoding, MouseEvent, MouseEventKind, MouseModifiers, MouseReportingMode,
    MouseState,
};

// Shell integration types.
pub use shell_integration::SemanticMarker;

// Kitty graphics types.
pub use kitty_graphics::{KittyAction, KittyCommand, KittyFormat, KittyImage, KittyImageStore};

// Scrollback B+ tree types.
pub use scrollback::{ByteOffset, RowCount, RowSummary, Scrollback, VisibleRowCount};

// Terminal mode bitset.
pub use term_mode::TermMode;

// Sixel graphics.
pub use sixel::{decode_dcs as decode_sixel_dcs, SixelFrame, SixelImage, SixelImageStore};

// IME / pre-edit buffer.
pub use ime::{ImeCandidate, ImeEvent, ImePhase, ImeState};
