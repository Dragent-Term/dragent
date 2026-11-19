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

use super::emu_events::Screens;
use super::emulator::{parse_osc52_write, HOOK_OSC_NUMBER};
use super::emulator::KITTY_KEYBOARD_STACK_MAX;
use super::emulator::MAX_SCROLLBACK;
use crate::cell::Style;
use crate::color::Color;
use crate::emulator::FENCE_BG_SENTINEL;
use crate::flags::Flags;
use crate::grid::Grid;
use crate::ime::ImeState;
use crate::kitty_graphics::{parse_kitty_apc, KittyImageStore};
use crate::mouse::MouseState;
use crate::shell_integration::{parse_hook_payload, parse_osc133, HookEvent};
use crate::sixel::{is_sixel_dcs, SixelImageStore};
use crate::term_mode::TermMode;
use crate::TermEvent;

/// Maximum depth of the Kitty keyboard enhancement stack.

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Interpreter {
    pub(crate) screens: Screens,
    pub(crate) style: Style,
    #[serde(skip)]
    pub(crate) events: Vec<TermEvent>,
    /// All active terminal modes in one 32-bit word.
    pub(crate) mode: TermMode,
    pub(crate) mouse: MouseState,
    pub(crate) kitty_images: KittyImageStore,
    pub(crate) sixel_images: SixelImageStore,
    pub(crate) next_hyperlink_id: u32,
    pub(crate) active_hyperlink_id: u32,
    pub(crate) audible_bell: bool,
    /// Kitty keyboard enhancement stack (push/pop via CSI > flags ; 4/5 u).
    /// Each entry is a saved 5-bit enhancement flag byte.
    pub(crate) keyboard_stack: Vec<u8>,
    pub(crate) inactive_keyboard_stack: Vec<u8>,
    /// IME composition state.
    #[serde(skip)]
    pub(crate) ime: ImeState,
    /// Configured scrollback line limit (settings-driven; 0 = unlimited).
    pub(crate) max_scrollback: usize,
}

impl Interpreter {
    pub(crate) fn new(rows: usize, cols: usize) -> Self {
        Self::with_scrollback(rows, cols, MAX_SCROLLBACK)
    }

    /// Cursor position as DSR reports it: 1-based, row then column. Reads the
    /// *active* grid, so the answer stays correct on the alt screen.
    pub(crate) fn cursor_position_1based(&self) -> (usize, usize) {
        let g = &self.screens.grid;
        (g.cursor_row + 1, g.cursor_col + 1)
    }

    pub(crate) fn with_scrollback(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Self {
            max_scrollback,
            screens: Screens {
                grid: Grid::new(rows, cols, max_scrollback),
                inactive_grid: Grid::new(rows, cols, 0),
                alt_active: false,
            },
            style: Style::default(),
            events: Vec::new(),
            mode: TermMode::DEFAULT,
            mouse: MouseState::default(),
            kitty_images: KittyImageStore::new(),
            sixel_images: SixelImageStore::new(),
            next_hyperlink_id: 1,
            active_hyperlink_id: 0,
            audible_bell: true,
            keyboard_stack: Vec::new(),
            inactive_keyboard_stack: Vec::new(),
            ime: ImeState::new(),
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn swap_alt(&mut self, save_restore_cursor: bool) {
        if !self.screens.alt_active {
            // Enter alt screen
            if save_restore_cursor {
                self.screens.grid.save_cursor();
            }
            // Copy primary cursor position and pending wrap to inactive_grid
            self.screens.inactive_grid.cursor_row = self.screens.grid.cursor_row;
            self.screens.inactive_grid.cursor_col = self.screens.grid.cursor_col;
            self.screens.inactive_grid.pending_wrap = self.screens.grid.pending_wrap;
            // Clear alternate screen contents in-place
            self.screens.inactive_grid.clear();
        }

        std::mem::swap(&mut self.keyboard_stack, &mut self.inactive_keyboard_stack);
        std::mem::swap(&mut self.screens.grid, &mut self.screens.inactive_grid);
        self.screens.alt_active = !self.screens.alt_active;
        self.mode
            .set_flag(TermMode::ALT_SCREEN, self.screens.alt_active);

        if !self.screens.alt_active {
            // Exit alt screen
            if save_restore_cursor {
                self.screens.grid.restore_cursor();
            }
        }
    }

    pub(crate) fn param(params: &[u16], idx: usize, default: u16) -> u16 {
        match params.get(idx) {
            Some(0) | None => default,
            Some(&v) => v,
        }
    }

    // ── SGR ──────────────────────────────────────────────────────────────────

    pub(crate) fn apply_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.style = Style::default();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.style = Style::default(),
                1 => self.style.flags.insert(Flags::BOLD),
                2 => self.style.flags.insert(Flags::DIM),
                3 => self.style.flags.insert(Flags::ITALIC),
                4 => match params.get(i + 1) {
                    Some(0) => {
                        self.style
                            .flags
                            .remove(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL);
                        i += 1;
                    }
                    Some(2) => {
                        self.style.flags.insert(Flags::DOUBLE_UNDERLINE);
                        i += 1;
                    }
                    Some(3) => {
                        self.style.flags.insert(Flags::UNDERCURL);
                        i += 1;
                    }
                    _ => self.style.flags.insert(Flags::UNDERLINE),
                },
                5 => self.style.flags.insert(Flags::BLINKING),
                7 => self.style.flags.insert(Flags::INVERSE),
                8 => self.style.flags.insert(Flags::HIDDEN),
                9 => self.style.flags.insert(Flags::STRIKEOUT),
                21 => {
                    self.style.flags.remove(Flags::BOLD | Flags::DIM);
                    self.style.flags.insert(Flags::DOUBLE_UNDERLINE);
                }
                22 => {
                    self.style.flags.remove(Flags::BOLD | Flags::DIM);
                }
                23 => self.style.flags.remove(Flags::ITALIC),
                24 => self
                    .style
                    .flags
                    .remove(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL),
                25 => self.style.flags.remove(Flags::BLINKING),
                27 => self.style.flags.remove(Flags::INVERSE),
                28 => self.style.flags.remove(Flags::HIDDEN),
                29 => self.style.flags.remove(Flags::STRIKEOUT),
                53 => self.style.flags.insert(Flags::OVERLINE),
                55 => self.style.flags.remove(Flags::OVERLINE),
                30..=37 => self.style.fg = Color::Indexed((params[i] - 30) as u8),
                39 => self.style.fg = Color::Default,
                40..=47 => self.style.bg = Color::Indexed((params[i] - 40) as u8),
                49 => self.style.bg = Color::Default,
                90..=97 => self.style.fg = Color::Indexed((params[i] - 90 + 8) as u8),
                100..=107 => self.style.bg = Color::Indexed((params[i] - 100 + 8) as u8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    let color = match params.get(i + 1) {
                        Some(5) => {
                            let c = params.get(i + 2).copied().unwrap_or(0) as u8;
                            i += 2;
                            Some(Color::Indexed(c))
                        }
                        Some(2) => {
                            let r = params.get(i + 2).copied().unwrap_or(0) as u8;
                            let g = params.get(i + 3).copied().unwrap_or(0) as u8;
                            let b = params.get(i + 4).copied().unwrap_or(0) as u8;
                            i += 4;
                            Some(Color::Rgb(r, g, b))
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if is_fg {
                            self.style.fg = c;
                        } else if c == FENCE_BG_SENTINEL {
                            // Rendered drain fence (see HookEvent::Fence): the
                            // shell hooks paint one prompt cell with this exact
                            // background. Attributes travel in order with cell
                            // text through ConPTY, so seeing it guarantees the
                            // preceding command output has fully flushed.
                            // Repaints may re-emit it; consumers treat repeats
                            // as no-ops. The color itself is never stored — the
                            // fence cell must stay invisible wherever it lands.
                            self.events.push(TermEvent::Hook(HookEvent::Fence));
                            self.style.bg = Color::Default;
                        } else {
                            self.style.bg = c;
                        }
                    }
                }
                58 => {
                    let color = match params.get(i + 1) {
                        Some(5) => {
                            let c = params.get(i + 2).copied().unwrap_or(0) as u8;
                            i += 2;
                            Some(Color::Indexed(c))
                        }
                        Some(2) => {
                            let r = params.get(i + 2).copied().unwrap_or(0) as u8;
                            let g = params.get(i + 3).copied().unwrap_or(0) as u8;
                            let b = params.get(i + 4).copied().unwrap_or(0) as u8;
                            i += 4;
                            Some(Color::Rgb(r, g, b))
                        }
                        _ => None,
                    };
                    self.style.underline_color = color;
                }
                59 => self.style.underline_color = None,
                _ => {}
            }
            i += 1;
        }
    }

    // ── DEC private mode dispatch ─────────────────────────────────────────────

    pub(crate) fn set_private_mode(&mut self, params: &[u16], enable: bool) {
        for &mode in params {
            match mode {
                // ── Cursor ────────────────────────────────────────────────────
                25 => self.mode.set_flag(TermMode::SHOW_CURSOR, enable),

                // ── Application keys ──────────────────────────────────────────
                1 => self.mode.set_flag(TermMode::APP_CURSOR, enable),

                // ── Line behaviour ────────────────────────────────────────────
                7 => {
                    self.mode.set_flag(TermMode::LINE_WRAP, enable);
                    self.screens.active().auto_wrap = enable;
                }
                20 => self.mode.set_flag(TermMode::LINE_FEED_NEW_LINE, enable),
                6 => {
                    self.mode.set_flag(TermMode::ORIGIN, enable);
                    if enable {
                        // Move cursor to top-left of scroll region.
                        let top = self.screens.active().scroll_region_top();
                        self.screens.active().move_to(top, 0);
                    } else {
                        self.screens.active().move_to(0, 0);
                    }
                }
                4 => self.mode.set_flag(TermMode::INSERT, enable),

                // ── Alternate screen ──────────────────────────────────────────
                47 | 1047 => {
                    if enable && !self.screens.alt_active {
                        self.swap_alt(false);
                    } else if !enable && self.screens.alt_active {
                        self.swap_alt(false);
                    }
                }
                1049 => {
                    if enable && !self.screens.alt_active {
                        self.swap_alt(true);
                    } else if !enable && self.screens.alt_active {
                        self.swap_alt(true);
                    }
                }

                // ── Alternate scroll ──────────────────────────────────────────
                1007 => self.mode.set_flag(TermMode::ALTERNATE_SCROLL, enable),

                // ── Mouse ─────────────────────────────────────────────────────
                1000 => {
                    self.mode.set_flag(TermMode::MOUSE_REPORT_CLICK, enable);
                    self.mouse.set_mode(1000, enable);
                }
                1002 => {
                    self.mode.set_flag(TermMode::MOUSE_DRAG, enable);
                    self.mouse.set_mode(1002, enable);
                }
                1003 => {
                    self.mode.set_flag(TermMode::MOUSE_MOTION, enable);
                    self.mouse.set_mode(1003, enable);
                }
                1004 => {
                    self.mode.set_flag(TermMode::FOCUS_IN_OUT, enable);
                    self.mouse.set_mode(1004, enable);
                }
                1006 => {
                    self.mode.set_flag(TermMode::SGR_MOUSE, enable);
                    self.mouse.set_mode(1006, enable);
                }
                1015 => {
                    self.mode.set_flag(TermMode::URXVT_MOUSE, enable);
                    self.mouse.set_mode(1015, enable);
                }

                // ── Extended protocols ────────────────────────────────────────
                2004 => {
                    self.mode.set_flag(TermMode::BRACKETED_PASTE, enable);
                    self.events.push(TermEvent::BracketedPaste(enable));
                }
                2026 => {
                    self.mode.set_flag(TermMode::SYNCHRONIZED_OUTPUT, enable);
                    self.events.push(TermEvent::SynchronizedOutput(enable));
                }
                2031 => self.mode.set_flag(TermMode::COLOR_SCHEME_UPDATES, enable),
                2048 => self.mode.set_flag(TermMode::KITTY_KEYBOARD, enable),

                // ── Kitty keyboard enhancement flags ──────────────────────────
                // Query: CSI ? 2048 $ p  → not handled here (host responsibility)
                _ => {}
            }
        }
    }

    // ── Kitty keyboard flag CSI > <flags> ; <mode> u ─────────────────────────
    //
    // Mode semantics (Kitty keyboard protocol spec):
    // Kitty keyboard CSI u sub-params:
    //   1 = set flags to <flags>
    //   2 = OR flags into current flags
    //   3 = AND-NOT: clear indicated flags
    //   4 = PUSH current flags, then set
    //   5 = POP and restore saved flags
    //
    // We implement all 5 modes with an 8-level stack.
    pub(crate) fn set_kitty_keyboard_flags(&mut self, params: &[u16]) {
        let flags = Self::param(params, 0, 0);
        let mode_param = Self::param(params, 1, 1);
        match mode_param {
            1 => self.apply_kitty_flags_absolute(flags),
            2 => {
                // OR in
                if flags & 1 != 0 {
                    self.mode.insert(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE);
                }
                if flags & 2 != 0 {
                    self.mode.insert(TermMode::KEYBOARD_REPORT_EVENT_TYPES);
                }
                if flags & 4 != 0 {
                    self.mode.insert(TermMode::KEYBOARD_REPORT_ALTERNATE_KEYS);
                }
                if flags & 8 != 0 {
                    self.mode.insert(TermMode::KEYBOARD_REPORT_ALL_AS_ESCAPE);
                }
                if flags & 16 != 0 {
                    self.mode.insert(TermMode::KEYBOARD_REPORT_ASSOCIATED_TEXT);
                }
                if self.mode.intersects(TermMode::KEYBOARD_PROTOCOL) {
                    self.mode.insert(TermMode::KITTY_KEYBOARD);
                }
            }
            3 => {
                // AND-NOT (clear flags)
                if flags & 1 != 0 {
                    self.mode.remove(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE);
                }
                if flags & 2 != 0 {
                    self.mode.remove(TermMode::KEYBOARD_REPORT_EVENT_TYPES);
                }
                if flags & 4 != 0 {
                    self.mode.remove(TermMode::KEYBOARD_REPORT_ALTERNATE_KEYS);
                }
                if flags & 8 != 0 {
                    self.mode.remove(TermMode::KEYBOARD_REPORT_ALL_AS_ESCAPE);
                }
                if flags & 16 != 0 {
                    self.mode.remove(TermMode::KEYBOARD_REPORT_ASSOCIATED_TEXT);
                }
                if !self.mode.intersects(TermMode::KEYBOARD_PROTOCOL) {
                    self.mode.remove(TermMode::KITTY_KEYBOARD);
                }
            }
            4 => {
                // PUSH current, then set new flags
                if self.keyboard_stack.len() < KITTY_KEYBOARD_STACK_MAX {
                    self.keyboard_stack.push(self.mode.kitty_keyboard_flags());
                }
                self.apply_kitty_flags_absolute(flags);
            }
            5 => {
                // POP — restore saved flags
                let saved = self.keyboard_stack.pop().unwrap_or(0);
                self.apply_kitty_flags_absolute(saved.into());
            }
            _ => {} // unknown mode
        }
    }

    fn apply_kitty_flags_absolute(&mut self, flags: u16) {
        self.mode.remove(TermMode::KEYBOARD_PROTOCOL);
        if flags & 1 != 0 {
            self.mode.insert(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE);
        }
        if flags & 2 != 0 {
            self.mode.insert(TermMode::KEYBOARD_REPORT_EVENT_TYPES);
        }
        if flags & 4 != 0 {
            self.mode.insert(TermMode::KEYBOARD_REPORT_ALTERNATE_KEYS);
        }
        if flags & 8 != 0 {
            self.mode.insert(TermMode::KEYBOARD_REPORT_ALL_AS_ESCAPE);
        }
        if flags & 16 != 0 {
            self.mode.insert(TermMode::KEYBOARD_REPORT_ASSOCIATED_TEXT);
        }
        self.mode.set(TermMode::KITTY_KEYBOARD, flags != 0);
    }

    // ── OSC dispatcher ────────────────────────────────────────────────────────

    pub(crate) fn handle_osc(&mut self, payload: &[u8]) {
        let text = String::from_utf8_lossy(payload);
        let mut parts = text.splitn(2, ';');
        let number = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();

        match number {
            "0" | "2" => self.events.push(TermEvent::TitleChanged(rest.to_string())),
            HOOK_OSC_NUMBER => {
                if let Some(ev) = parse_hook_payload(rest) {
                    if matches!(ev, HookEvent::Precmd { .. }) {
                        // The shell owns input again even if a crashed TUI did
                        // not disable its mouse/alternate-screen protocols.
                        // Do this at the hook operation, before later output
                        // in the same read can start another command.
                        if self.screens.alt_active {
                            self.swap_alt(true);
                        }
                        self.mouse = MouseState::default();
                        self.mode.remove(
                            TermMode::MOUSE_MODE
                                | TermMode::SGR_MOUSE
                                | TermMode::URXVT_MOUSE
                                | TermMode::FOCUS_IN_OUT
                                | TermMode::ALTERNATE_SCROLL
                                | TermMode::COLOR_SCHEME_UPDATES,
                        );
                    }
                    self.events.push(TermEvent::Hook(ev));
                }
            }
            "133" => {
                if let Some(marker) = parse_osc133(rest) {
                    self.events.push(TermEvent::SemanticPrompt(marker));
                }
            }
            "8" => {
                let mut p2 = rest.splitn(2, ';');
                let params_str = p2.next().unwrap_or_default();
                let uri = p2.next().unwrap_or_default();
                if uri.is_empty() {
                    self.active_hyperlink_id = 0;
                    self.events.push(TermEvent::Hyperlink {
                        id: String::new(),
                        uri: String::new(),
                    });
                } else {
                    let id_str = params_str
                        .split(':')
                        .find(|p| p.starts_with("id="))
                        .map(|p| p[3..].to_string())
                        .unwrap_or_default();
                    let link_id = self.next_hyperlink_id;
                    self.next_hyperlink_id += 1;
                    self.active_hyperlink_id = link_id;
                    self.events.push(TermEvent::Hyperlink {
                        id: id_str,
                        uri: uri.to_string(),
                    });
                }
            }
            // Foreground/background colour queries. termenv sends `OSC 11 ; ? ST`
            // to decide whether to use a light or dark palette, and blocks on
            // the answer. Only the `?` (query) form needs a reply — a bare
            // `OSC 11 ; <colour>` is the program *setting* the colour.
            n @ ("10" | "11") if rest.starts_with('?') => {
                let index = if n == "10" { 10 } else { 11 };
                self.events.push(TermEvent::ColorQuery { index });
            }
            "52" => {
                if let Some(text) = parse_osc52_write(rest) {
                    self.events.push(TermEvent::ClipboardWrite(text));
                }
            }
            "1337" => self.events.push(TermEvent::DcsReceived(payload.to_vec())),
            _ => {}
        }
    }

    pub(crate) fn handle_apc(&mut self, payload: &[u8]) {
        if payload.first() == Some(&b'G') {
            if let Some(cmd) = parse_kitty_apc(&payload[1..]) {
                let (response, stored_id) = self.kitty_images.process(cmd);
                if let Some(resp) = response {
                    self.events.push(TermEvent::DcsReceived(resp));
                }
                if let Some(id) = stored_id {
                    self.events.push(TermEvent::KittyImage { image_id: id });
                }
            }
        }
    }

    pub(crate) fn handle_dcs(&mut self, payload: &[u8]) {
        if is_sixel_dcs(payload) {
            if let Some(id) = self.sixel_images.process_dcs(payload) {
                self.events.push(TermEvent::SixelImage { image_id: id });
                return;
            }
        }
        // Other DCS sequences (e.g. tmux passthrough, DECRQSS responses).
        self.events.push(TermEvent::DcsReceived(payload.to_vec()));
    }
}
