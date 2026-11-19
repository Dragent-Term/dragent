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

use crate::cell::{Cell, StyledLine};
use crate::emu_interpreter::Interpreter;
use crate::grid::Grid;
use crate::ime::{ImeEvent, ImeState};
use crate::kitty_graphics::KittyImageStore;
use crate::mouse::MouseState;
use crate::parser::{Parser, VtActor};
use crate::sixel::SixelImageStore;
use crate::term_mode::TermMode;
use crate::TermEvent;

/// Maximum depth of the Kitty keyboard enhancement stack.

/// A full terminal emulator (B+ tree scrollback + TermMode bitset).
pub struct Emulator {
    render_revision: u64,
    parser: Parser,
    interp: Interpreter,
}

impl Emulator {
    /// Full interpreter state, including both screens, scrollback, input modes,
    /// keyboard stacks and images. Parser state travels separately on handoff.
    /// No events are replayed: in particular, device replies must not be sent twice.
    pub fn display_checkpoint(&self) -> Result<Vec<u8>, String> {
        use bincode::Options;
        bincode::DefaultOptions::new()
            .with_varint_encoding()
            .with_limit(24 * 1024 * 1024)
            .serialize(&self.interp)
            .map_err(|error| error.to_string())
    }

    pub fn restore_display_checkpoint(&mut self, bytes: &[u8]) -> Result<(), String> {
        use bincode::Options;
        let mut interp: Interpreter = bincode::DefaultOptions::new()
            .with_varint_encoding()
            .with_limit(24 * 1024 * 1024)
            .reject_trailing_bytes()
            .deserialize(bytes)
            .map_err(|error| error.to_string())?;
        if !interp.screens.grid.valid_checkpoint()
            || !interp.screens.inactive_grid.valid_checkpoint()
            || interp.keyboard_stack.len() > super::emulator::KITTY_KEYBOARD_STACK_MAX
            || interp.inactive_keyboard_stack.len() > super::emulator::KITTY_KEYBOARD_STACK_MAX
        {
            return Err("invalid terminal display checkpoint".into());
        }
        interp.audible_bell = self.interp.audible_bell;
        self.interp = interp;
        self.render_revision = self.render_revision.wrapping_add(1);
        Ok(())
    }

    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            render_revision: 0,
            parser: Parser::new(),
            interp: Interpreter::new(rows, cols),
        }
    }

    /// Like [`Emulator::new`] with an explicit scrollback line limit
    /// (settings-driven; `0` = unlimited — eviction disabled).
    pub fn with_scrollback(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Self {
            render_revision: 0,
            parser: Parser::new(),
            interp: Interpreter::with_scrollback(rows, cols, max_scrollback),
        }
    }

    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.render_revision = self.render_revision.wrapping_add(1);
        }
        self.parser.feed(bytes, &mut self.interp);
    }

    pub fn take_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut self.interp.events)
    }

    /// Parse once in the daemon and retain the exact operations for clients.
    pub fn feed_recorded(&mut self, bytes: &[u8]) -> Vec<crate::operations::Operation> {
        if !bytes.is_empty() {
            self.render_revision = self.render_revision.wrapping_add(1);
        }
        let mut recorder = crate::operations::Recorder {
            actor: &mut self.interp,
            operations: Vec::new(),
        };
        self.parser.feed(bytes, &mut recorder);
        recorder.operations
    }

    /// Whether raw output can resume at this byte boundary on a fresh parser.
    pub fn parser_is_ground(&self) -> bool {
        self.parser.is_ground()
    }

    /// Bounded VT prefix for resuming an unfinished escape or UTF-8 sequence.
    pub fn parser_pending_prefix(&self) -> Vec<u8> {
        self.parser.pending_prefix()
    }

    /// Attributes applied to the next printed character.
    pub fn current_pen(&self) -> crate::Style {
        self.interp.style
    }

    /// Only the byte parser state, not the display grid/interpreter state.
    pub fn parser_checkpoint(&self) -> crate::parser::Parser {
        self.parser.clone()
    }

    pub fn restore_parser_checkpoint(
        &mut self,
        parser: crate::parser::Parser,
    ) -> Result<(), &'static str> {
        if !parser.valid_checkpoint() {
            return Err("invalid terminal parser checkpoint");
        }
        self.parser = parser;
        Ok(())
    }

    /// Update a display replica without invoking its byte parser.
    pub fn apply_operations(&mut self, operations: &[crate::operations::Operation]) {
        if !operations.is_empty() {
            self.render_revision = self.render_revision.wrapping_add(1);
        }
        for operation in operations {
            operation.apply(&mut self.interp);
        }
    }

    // ── Mode access ───────────────────────────────────────────────────────────

    /// The full terminal mode word.
    ///
    /// The host application uses this to:
    /// - Check `APP_CURSOR` when encoding arrow/function key input
    /// - Check `APP_KEYPAD` when encoding numpad input
    /// - Check `BRACKETED_PASTE` before pasting
    /// - Set `VI` / `URGENCY_HINTS` independently of VT sequences
    pub fn mode(&self) -> TermMode {
        self.interp.mode
    }

    /// Directly set or clear a mode flag (for host-driven modes like `VI`).
    pub fn set_mode(&mut self, flag: TermMode, on: bool) {
        self.interp.mode.set(flag, on);
    }

    /// Reset pointer protocols at a shell command boundary, preserving output.
    pub fn reset_pointer_modes(&mut self) {
        self.interp.mouse = MouseState::default();
        self.interp.mode.remove(
            TermMode::MOUSE_MODE
                | TermMode::SGR_MOUSE
                | TermMode::URXVT_MOUSE
                | TermMode::FOCUS_IN_OUT
                | TermMode::ALTERNATE_SCROLL
                | TermMode::COLOR_SCHEME_UPDATES,
        );
    }

    pub fn cursor_visible(&self) -> bool {
        self.interp.mode.contains(TermMode::SHOW_CURSOR)
    }

    pub fn alt_screen_active(&self) -> bool {
        self.interp.screens.alt_active
    }

    pub fn set_audible_bell(&mut self, enabled: bool) {
        self.interp.audible_bell = enabled;
    }

    // ── Grid access ───────────────────────────────────────────────────────────

    pub fn grid(&self) -> &Grid {
        if self.interp.screens.alt_active {
            &self.interp.screens.inactive_grid
        } else {
            &self.interp.screens.grid
        }
    }

    pub fn alt_grid(&self) -> &Grid {
        if self.interp.screens.alt_active {
            &self.interp.screens.grid
        } else {
            &self.interp.screens.inactive_grid
        }
    }

    pub fn cursor_abs_line(&self) -> usize {
        self.grid().cursor_abs_line()
    }

    pub fn styled_lines_in(&self, start: usize, end: usize) -> Vec<StyledLine> {
        self.grid().styled_lines_in(start, end)
    }

    pub fn alt_screen_lines(&self) -> Vec<StyledLine> {
        let mut lines = self.alt_grid().screen_styled_lines();
        while lines.last().is_some_and(StyledLine::is_blank) {
            lines.pop();
        }
        lines
    }

    /// All rows in the alt screen, including trailing blank rows.
    /// Use this for full-canvas rendering where row positions must be exact.
    pub fn alt_screen_all_lines(&self) -> Vec<StyledLine> {
        self.alt_grid().screen_styled_lines()
    }

    /// Current cursor position `(row, col)` on the active screen (primary or alt).
    pub fn cursor_position(&self) -> (usize, usize) {
        let g = &self.interp.screens.grid;
        (g.cursor_row, g.cursor_col)
    }

    /// Terminal grid dimensions `(rows, cols)`.
    pub fn dimensions(&self) -> (usize, usize) {
        let g = self.grid();
        (g.rows, g.cols)
    }

    pub fn cursor_line_text(&self) -> String {
        // Cursor is always on the visible screen.
        let grid = self.grid();
        grid.screen
            .get(grid.cursor_row)
            .map(|row| row.plain_text())
            .unwrap_or_default()
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.render_revision = self.render_revision.wrapping_add(1);
        self.interp.screens.grid.resize(rows, cols);
        self.interp.screens.inactive_grid.resize(rows, cols);
    }

    pub fn kitty_images(&self) -> &KittyImageStore {
        &self.interp.kitty_images
    }

    pub fn sixel_images(&self) -> &SixelImageStore {
        &self.interp.sixel_images
    }

    pub fn mouse_state(&self) -> &MouseState {
        &self.interp.mouse
    }

    // ── Kitty keyboard stack ──────────────────────────────────────────────────

    /// Current depth of the Kitty keyboard enhancement stack.
    pub fn keyboard_stack_depth(&self) -> usize {
        self.interp.keyboard_stack.len()
    }

    // ── IME public API ────────────────────────────────────────────────────────

    /// Inspect current IME composition state.
    pub fn ime_state(&self) -> &ImeState {
        &self.interp.ime
    }

    /// Begin an IME composition anchored at the current cursor position.
    pub fn ime_begin(&mut self) {
        let grid = self.grid();
        let (row, col) = (grid.cursor_row, grid.cursor_col);
        self.interp.ime.begin_at(row, col);
    }

    /// Feed an `ImeEvent` into the emulator.
    ///
    /// If the event is `ImeEvent::Commit`, the committed text is emitted as
    /// `TermEvent::ImeCommit` so the host can forward it to the PTY.
    pub fn ime_handle(&mut self, event: ImeEvent) {
        if let Some(text) = self.interp.ime.handle(event) {
            self.interp.events.push(TermEvent::ImeCommit(text));
        }
    }

    // ── Live cell edits (update_at via B+ tree) ───────────────────────────────

    /// Mutate a single scrollback cell in-place. O(log n).
    ///
    /// `abs_row` is 0-based within the scrollback buffer (0 = oldest row).
    /// Use this to apply async decorations (URL underlines, LSP squiggles,
    /// semantic highlights) without rebuilding the scrollback tree.
    pub fn update_scrollback_cell<F>(&mut self, abs_row: usize, col: usize, f: F) -> bool
    where
        F: FnOnce(&mut Cell),
    {
        let grid = if self.interp.screens.alt_active {
            &mut self.interp.screens.inactive_grid
        } else {
            &mut self.interp.screens.grid
        };
        grid.update_scrollback_cell(abs_row, col, f)
    }

    /// Mutate a span of cells in a scrollback row in-place. O(log n).
    pub fn update_scrollback_span<F>(
        &mut self,
        abs_row: usize,
        col_start: usize,
        col_end: usize,
        f: F,
    ) -> bool
    where
        F: Fn(&mut Cell),
    {
        let grid = if self.interp.screens.alt_active {
            &mut self.interp.screens.inactive_grid
        } else {
            &mut self.interp.screens.grid
        };
        grid.update_scrollback_span(abs_row, col_start, col_end, f)
    }

    // ── Block collapse / expand (FilterPath) ─────────────────────────────────

    /// Collapse scrollback rows `[abs_start, abs_end)` so they are hidden
    /// from `visible_styled_lines_in`.
    pub fn collapse_block(&mut self, abs_start: usize, abs_end: usize) {
        let grid = if self.interp.screens.alt_active {
            &mut self.interp.screens.inactive_grid
        } else {
            &mut self.interp.screens.grid
        };
        grid.collapse_block(abs_start, abs_end);
    }

    /// Re-expand previously collapsed rows.
    pub fn expand_block(&mut self, abs_start: usize, abs_end: usize) {
        let grid = if self.interp.screens.alt_active {
            &mut self.interp.screens.inactive_grid
        } else {
            &mut self.interp.screens.grid
        };
        grid.expand_block(abs_start, abs_end);
    }

    /// Number of currently visible scrollback rows (collapsed rows excluded).
    pub fn visible_scrollback_len(&self) -> usize {
        self.grid().visible_scrollback_len()
    }
}
