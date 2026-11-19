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

use crate::bell;
use crate::cell::Style;
use crate::emu_interpreter::Interpreter;
use crate::grid::Grid;
use crate::parser::VtActor;
use crate::term_mode::TermMode;
use crate::CursorShape;
use crate::TermEvent;

/// Maximum depth of the Kitty keyboard enhancement stack.

impl VtActor for Interpreter {
    fn print(&mut self, ch: char) {
        let style = self.style;
        let hl_id = self.active_hyperlink_id;

        // INSERT mode: make room before writing.
        if self.mode.contains(TermMode::INSERT) {
            self.screens.active().insert_chars(1, style);
        }

        let grid = self.screens.active();
        grid.put_char(ch, style);

        if hl_id != 0 {
            let row = grid.cursor_row;
            let col = if grid.cursor_col > 0 {
                grid.cursor_col - 1
            } else {
                0
            };
            grid.set_cell_hyperlink(row, col, hl_id);
        }
    }

    fn control(&mut self, byte: u8) {
        match byte {
            0x07 => {
                if self.audible_bell {
                    bell::ring_bell();
                }
                self.events.push(TermEvent::Bell);
            }
            0x08 => self.screens.active().backspace(),
            0x09 => self.screens.active().tab(),
            0x0a | 0x0b | 0x0c => {
                self.screens.active().linefeed();
                // LNM — line-feed / new-line mode.
                if self.mode.contains(TermMode::LINE_FEED_NEW_LINE) {
                    self.screens.active().carriage_return();
                }
            }
            0x0d => self.screens.active().carriage_return(),
            _ => {}
        }
    }

    fn csi(&mut self, params: &[u16], private: Option<u8>, intermediates: &[u8], action: u8) {
        // ── DEC private modes ─────────────────────────────────────────────────
        if private == Some(b'?') {
            match action {
                b'h' => self.set_private_mode(params, true),
                b'l' => self.set_private_mode(params, false),
                // DECXCPR (CSI ? 6 n) — extended cursor position report. Same
                // answer as DSR 6 but with the private marker echoed back.
                b'n' if Self::param(params, 0, 0) == 6 => {
                    let (row, col) = self.cursor_position_1based();
                    self.events
                        .push(TermEvent::Reply(format!("\x1b[?{row};{col}R").into_bytes()));
                }
                b'n' if Self::param(params, 0, 0) == 996 => {
                    self.events.push(TermEvent::ColorSchemeQuery);
                }
                _ => {}
            }
            return;
        }

        // ── Kitty keyboard flags (CSI > <flags> ; <mode> u) ───────────────────
        if private == Some(b'>') && action == b'u' {
            self.set_kitty_keyboard_flags(params);
            return;
        }

        if private.is_some() {
            return;
        }

        // ── DECSCUSR cursor shape: CSI n SP q ────────────────────────────────
        if intermediates == [b' '] && action == b'q' {
            let shape = CursorShape::from_param(Self::param(params, 0, 0));
            self.events.push(TermEvent::CursorShape(shape));
            return;
        }

        let style = self.style;

        // ── Origin-mode cursor offset ─────────────────────────────────────────
        let origin_row = if self.mode.contains(TermMode::ORIGIN) {
            self.screens.active().scroll_region_top()
        } else {
            0
        };

        match action {
            b'm' => self.apply_sgr(params),

            // Cursor movement.
            b'A' => self
                .screens
                .active()
                .move_rows(-(Self::param(params, 0, 1) as isize)),
            b'B' | b'e' => self
                .screens
                .active()
                .move_rows(Self::param(params, 0, 1) as isize),
            b'C' | b'a' => self
                .screens
                .active()
                .move_cols(Self::param(params, 0, 1) as isize),
            b'D' => self
                .screens
                .active()
                .move_cols(-(Self::param(params, 0, 1) as isize)),
            b'E' => {
                let n = Self::param(params, 0, 1) as isize;
                self.screens.active().move_rows(n);
                self.screens.active().carriage_return();
            }
            b'F' => {
                let n = Self::param(params, 0, 1) as isize;
                self.screens.active().move_rows(-n);
                self.screens.active().carriage_return();
            }
            b'G' | b'`' => {
                let col = Self::param(params, 0, 1) as usize;
                let g = self.screens.active();
                let row = g.cursor_row;
                g.move_to(row, col.saturating_sub(1));
            }
            b'd' => {
                let row = Self::param(params, 0, 1) as usize;
                let g = self.screens.active();
                let col = g.cursor_col;
                g.move_to(origin_row + row.saturating_sub(1), col);
            }
            b'H' | b'f' => {
                let row = Self::param(params, 0, 1) as usize;
                let col = Self::param(params, 1, 1) as usize;
                self.screens
                    .active()
                    .move_to(origin_row + row.saturating_sub(1), col.saturating_sub(1));
            }

            // Erase.
            b'J' => {
                let mode = params.first().copied().unwrap_or(0);
                self.screens.active().erase_display(mode, style);
                if !self.screens.alt_active && matches!(mode, 2 | 3) {
                    self.events.push(TermEvent::ScreenCleared);
                }
            }
            b'K' => {
                let mode = params.first().copied().unwrap_or(0);
                self.screens.active().erase_line(mode, style);
            }

            // Line insert / delete.
            b'L' => self
                .screens
                .active()
                .insert_lines(Self::param(params, 0, 1) as usize),
            b'M' => self
                .screens
                .active()
                .delete_lines(Self::param(params, 0, 1) as usize),

            // Character insert / delete / erase.
            b'@' => self
                .screens
                .active()
                .insert_chars(Self::param(params, 0, 1) as usize, style),
            b'P' => self
                .screens
                .active()
                .delete_chars(Self::param(params, 0, 1) as usize, style),
            b'X' => self
                .screens
                .active()
                .erase_chars(Self::param(params, 0, 1) as usize, style),

            // Scroll.
            b'S' => self
                .screens
                .active()
                .scroll_up(Self::param(params, 0, 1) as usize),
            b'T' => self
                .screens
                .active()
                .scroll_down(Self::param(params, 0, 1) as usize),

            // Scroll region.
            b'r' => {
                let g = self.screens.active();
                let top = Self::param(params, 0, 1) as usize;
                let bottom = Self::param(params, 1, g.rows as u16) as usize;
                g.set_scroll_region(top.saturating_sub(1), bottom.saturating_sub(1));
                if self.mode.contains(TermMode::ORIGIN) {
                    g.move_to(g.scroll_region_top(), 0);
                }
            }

            // Cursor save / restore.
            b's' => self.screens.active().save_cursor(),
            b'u' => self.screens.active().restore_cursor(),

            // Tab operations.
            b'I' => self
                .screens
                .active()
                .tab_forward(Self::param(params, 0, 1) as usize),
            b'Z' => self
                .screens
                .active()
                .tab_backward(Self::param(params, 0, 1) as usize),
            b'g' => match params.first().copied().unwrap_or(0) {
                0 => self.screens.active().clear_tab_stop(),
                3 => self.screens.active().clear_all_tab_stops(),
                _ => {}
            },

            b't' => {} // Window manipulation — ignore.

            // DSR — Device Status Report. Silence here hangs any TUI that
            // probes the terminal before drawing.
            b'n' => match Self::param(params, 0, 0) {
                5 => self.events.push(TermEvent::Reply(b"\x1b[0n".to_vec())), // "terminal OK"
                6 => {
                    let (row, col) = self.cursor_position_1based();
                    self.events
                        .push(TermEvent::Reply(format!("\x1b[{row};{col}R").into_bytes()));
                }
                _ => {}
            },

            // Primary DA — "what are you?". VT102, matching Alacritty.
            b'c' => self.events.push(TermEvent::Reply(b"\x1b[?6c".to_vec())),

            _ => {}
        }
    }

    fn esc(&mut self, _intermediates: &[u8], byte: u8) {
        match byte {
            b'7' => self.screens.active().save_cursor(),
            b'8' => self.screens.active().restore_cursor(),
            b'M' => self.screens.active().reverse_index(),
            b'H' => self.screens.active().set_tab_stop(),
            // Focus events.
            b'I' if self.mode.contains(TermMode::FOCUS_IN_OUT) => {
                self.events.push(TermEvent::FocusIn)
            }
            b'O' if self.mode.contains(TermMode::FOCUS_IN_OUT) => {
                self.events.push(TermEvent::FocusOut)
            }
            // APP_KEYPAD: ESC = on, ESC > off.
            b'=' => self.mode.insert(TermMode::APP_KEYPAD),
            b'>' => self.mode.remove(TermMode::APP_KEYPAD),
            // Full terminal reset (RIS).
            b'c' => {
                let (rows, cols) = (self.screens.grid.rows, self.screens.grid.cols);
                self.screens.grid = Grid::new(rows, cols, self.max_scrollback);
                self.screens.inactive_grid = Grid::new(rows, cols, 0);
                self.screens.alt_active = false;
                self.style = Style::default();
                self.mode = TermMode::DEFAULT;
                self.active_hyperlink_id = 0;
                self.keyboard_stack.clear();
                self.inactive_keyboard_stack.clear();
                self.ime.reset();
            }
            _ => {}
        }
    }

    fn osc(&mut self, payload: &[u8]) {
        self.handle_osc(payload);
    }
    fn apc(&mut self, payload: &[u8]) {
        self.handle_apc(payload);
    }
    fn dcs(&mut self, payload: &[u8]) {
        self.handle_dcs(payload);
    }
}
