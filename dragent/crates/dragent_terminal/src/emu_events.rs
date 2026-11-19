// Adapted from alacritty_terminal/src/term/mod.rs under the Apache License 2.0.
// See crates/LICENSE-ALACRITTY.

//! Terminal emulator: parser ↔ interpreter ↔ grid.
//!
//! # Feature summary
//! - `TermMode: u32` bitset — 25 DEC private modes in one word
//! - B+ tree scrollback via `dragent_sumtree` — O(log n) seeks + eviction
//! - Full Kitty keyboard enhancement protocol including **push/pop stack**
//!   (up to `KITTY_KEYBOARD_STACK_MAX` levels)
//! - **Sixel graphics** — full DCS decoder with 65 536 registers + RGBA
//! - **IME pre-edit buffer** — structured candidate list + cursor tracking
//! - **`update_scrollback_cell`** — O(log n) live cell edits via `BTree::update_at`
//! - **`collapse_block` / `expand_block`** — `FilterPath`-powered block folding

use crate::grid::Grid;
use crate::shell_integration::{HookEvent, SemanticMarker};

/// Maximum depth of the Kitty keyboard enhancement stack.

/// Events surfaced to the embedding application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Hook(HookEvent),
    TitleChanged(String),
    Bell,
    ScreenCleared,
    FocusIn,
    FocusOut,
    BracketedPaste(bool),
    SemanticPrompt(SemanticMarker),
    Hyperlink {
        id: String,
        uri: String,
    },
    KittyImage {
        image_id: u32,
    },
    /// A sixel image was received and stored.  `image_id` keys into
    /// `Emulator::sixel_images()`.
    SixelImage {
        image_id: u32,
    },
    DcsReceived(Vec<u8>),
    CursorShape(CursorShape),
    SynchronizedOutput(bool),
    /// IME composition text was committed — forward to PTY.
    ImeCommit(String),
    /// A ready-to-send answer to a terminal query (DSR, DA) — write verbatim
    /// to the PTY. Programs that ask block until they get one: `gh auth login`
    /// (and anything else built on Charm's termenv) sends `CSI 6n` before
    /// drawing and will sit there forever if the terminal stays silent.
    Reply(Vec<u8>),
    /// An OSC 10/11 colour query (foreground/background). The emulator has no
    /// theme, so the host answers this one.
    ColorQuery {
        index: u8,
    },
    /// `CSI ? 996 n` — is the terminal dark or light? Theme-dependent like
    /// [`ColorQuery`](Self::ColorQuery), so the host answers it too.
    ColorSchemeQuery,
    /// OSC 52 — the running app copied this text to the clipboard.
    ClipboardWrite(String),
}

/// Terminal cursor shape (DECSCUSR / `CSI n SP q`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

impl CursorShape {
    pub(crate) fn from_param(p: u16) -> Self {
        match p {
            0 | 1 => Self::BlinkingBlock,
            2 => Self::SteadyBlock,
            3 => Self::BlinkingUnderline,
            4 => Self::SteadyUnderline,
            5 => Self::BlinkingBar,
            6 => Self::SteadyBar,
            _ => Self::Default,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal structures
// ─────────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Screens {
    pub(crate) grid: Grid,
    pub(crate) inactive_grid: Grid,
    pub(crate) alt_active: bool,
}

impl Screens {
    pub(crate) fn active(&mut self) -> &mut Grid {
        &mut self.grid
    }
}
