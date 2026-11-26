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

use crate::color::Color;
use crate::parser::VtActor;

/// Maximum depth of the Kitty keyboard enhancement stack.

/// Maximum depth of the Kitty keyboard enhancement stack.
pub(crate) const KITTY_KEYBOARD_STACK_MAX: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
// Public constants
// ─────────────────────────────────────────────────────────────────────────────

/// OSC number used by Dragent's native shell-hook protocol.
pub const HOOK_OSC_NUMBER: &str = "7979";

/// Background color the shell hooks paint on one prompt cell as the rendered
/// drain fence (see [`HookEvent::Fence`]). RGB(1,2,3) is visually black and
/// never produced by real programs; only scanned in SGR 48;2;… truecolor form.
pub const FENCE_BG_SENTINEL: Color = Color::Rgb(1, 2, 3);

/// The largest OSC 52 payload we accept, in base64 characters.
const OSC52_MAX_ENCODED_LEN: usize = 8 * 1024 * 1024;

/// The text an OSC 52 sequence (`<targets> ; <base64>`) copies to the clipboard.
///
/// Only writes to the clipboard target are honoured. Answering a read (`?`)
/// would let anything printed in the terminal, such as a file being `cat`ed,
/// read the user's clipboard.
pub(crate) fn parse_osc52_write(rest: &str) -> Option<String> {
    use base64::Engine;
    let (targets, data) = rest.split_once(';')?;
    if !(targets.is_empty() || targets.contains('c')) {
        return None;
    }
    if data.is_empty() || data == "?" || data.len() > OSC52_MAX_ENCODED_LEN {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    String::from_utf8(bytes).ok().filter(|text| !text.is_empty())
}

/// The answer to an OSC 10/11 colour query. Colour replies use 16 bits per
/// channel, which is what every parser expects, so each byte is doubled.
pub fn color_query_report(index: u8, (r, g, b): (u8, u8, u8)) -> Vec<u8> {
    format!("\x1b]{index};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\").into_bytes()
}

/// The report telling an app the terminal's colour scheme (`CSI ? 997 ; 1|2 n`).
pub fn color_scheme_report(is_dark: bool) -> Vec<u8> {
    format!("\x1b[?997;{}n", if is_dark { 1 } else { 2 }).into_bytes()
}

// Keep enough live history for long-running commands without allowing an
// unbounded process to grow forever. Completed blocks apply the same limit.
// There is no unlimited mode: see `scrollback::normalize_scrollback_lines`.
pub(crate) const MAX_SCROLLBACK: usize = crate::scrollback::SCROLLBACK_DEFAULT_LINES;

// ─────────────────────────────────────────────────────────────────────────────
// Public event types
// ─────────────────────────────────────────────────────────────────────────────

pub use super::emu_events::{CursorShape, TermEvent};
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ime::ImeEvent;
    use crate::{Emulator, Flags, HookEvent, SemanticMarker, StyledLine, TermMode};

    fn screen_text(emu: &Emulator) -> Vec<String> {
        emu.grid()
            .screen_styled_lines()
            .iter()
            .map(StyledLine::plain_text)
            .collect()
    }

    #[test]
    fn echoes_simple_output() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"hello\r\nworld\r\n");
        let text = screen_text(&emu);
        assert_eq!(text[0], "hello");
        assert_eq!(text[1], "world");
    }

    #[test]
    fn spinner_redraw() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"|\r/\r-\r\\\rdone");
        let text = screen_text(&emu);
        assert_eq!(text[0], "done");
    }

    #[test]
    fn sgr_colors() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[31mred\x1b[0m plain");
        let lines = emu.grid().screen_styled_lines();
        assert_eq!(lines[0].spans[0].style.fg, Color::Indexed(1));
        assert_eq!(lines[0].spans[1].style.fg, Color::Default);
    }

    #[test]
    fn truecolor_sgr() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[38;2;10;20;30mX");
        let lines = emu.grid().screen_styled_lines();
        assert_eq!(lines[0].spans[0].style.fg, Color::Rgb(10, 20, 30));
    }

    #[test]
    fn osc52_writes_reach_the_clipboard_but_reads_never_do() {
        let mut emu = Emulator::new(24, 80);
        emu.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert!(emu
            .take_events()
            .contains(&TermEvent::ClipboardWrite("hello".to_string())));
        for sequence in [
            &b"\x1b]52;c;?\x07"[..],
            b"\x1b]52;c;\x07",
            b"\x1b]52;p;aGVsbG8=\x07",
            b"\x1b]52;c;not base64!\x07",
        ] {
            emu.feed(sequence);
            assert!(!emu
                .take_events()
                .iter()
                .any(|e| matches!(e, TermEvent::ClipboardWrite(_))));
        }
    }

    #[test]
    fn color_scheme_mode_and_query() {
        let mut emu = Emulator::new(24, 80);
        emu.feed(b"\x1b[?2031h");
        assert!(emu.mode().contains(TermMode::COLOR_SCHEME_UPDATES));
        emu.feed(b"\x1b[?996n");
        assert!(emu.take_events().contains(&TermEvent::ColorSchemeQuery));
        // A crashed app must not leave the shell prompt receiving reports.
        emu.reset_pointer_modes();
        assert!(!emu.mode().contains(TermMode::COLOR_SCHEME_UPDATES));
        assert_eq!(color_scheme_report(true), b"\x1b[?997;1n");
        assert_eq!(color_scheme_report(false), b"\x1b[?997;2n");
    }

    #[test]
    fn alt_screen() {
        let mut emu = Emulator::new(3, 20);
        emu.feed(b"shell line\r\n");
        emu.feed(b"\x1b[?1049h\x1b[2J\x1b[Hvim contents");
        assert!(emu.alt_screen_active());
        assert!(emu.mode().contains(TermMode::ALT_SCREEN));
        assert_eq!(emu.alt_screen_lines()[0].plain_text(), "vim contents");
        emu.feed(b"\x1b[?1049l");
        assert!(!emu.alt_screen_active());
        assert_eq!(screen_text(&emu)[0], "shell line");
    }

    #[test]
    fn hook_events() {
        let mut emu = Emulator::new(4, 40);
        emu.feed(b"\x1b]7979;preexec;cargo build --release\x1b\\");
        emu.feed(b"output\r\n");
        emu.feed(b"\x1b]7979;precmd;0;/Users/me/dev;x\x07");
        let events = emu.take_events();
        assert_eq!(
            events[0],
            TermEvent::Hook(HookEvent::Preexec {
                command: "cargo build --release".into()
            })
        );
        assert_eq!(
            events[1],
            TermEvent::Hook(HookEvent::Precmd {
                exit_code: 0,
                cwd: "/Users/me/dev;x".into()
            })
        );
    }

    #[test]
    fn absolute_line_ranges() {
        let mut emu = Emulator::new(4, 40);
        emu.feed(b"prompt $ ls\r\n");
        let mark = emu.cursor_abs_line();
        emu.feed(b"file_a\r\nfile_b\r\n");
        let end = emu.cursor_abs_line();
        let lines = emu.styled_lines_in(mark, end);
        assert_eq!(lines[0].plain_text(), "file_a");
        assert_eq!(lines[1].plain_text(), "file_b");
    }

    #[test]
    fn cursor_positioning() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"abcdef\x1b[1;1Hxy");
        assert_eq!(screen_text(&emu)[0], "xycdef");
    }

    #[test]
    fn screen_cleared_event() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[2J");
        assert!(emu.take_events().contains(&TermEvent::ScreenCleared));
    }

    /// The exact query `gh auth login` (termenv) sends before drawing. With no
    /// reply it blocks forever — this was the Mac-only `gh auth login` hang.
    #[test]
    fn dsr_cursor_position_is_answered() {
        let mut emu = Emulator::new(10, 40);
        emu.feed(b"hi\r\n\x1b[6n");
        let ev = emu.take_events();
        // Row 2, col 1 (1-based) after one line of output.
        assert!(
            ev.contains(&TermEvent::Reply(b"\x1b[2;1R".to_vec())),
            "expected cursor position report, got {ev:?}"
        );
    }

    #[test]
    fn dsr_status_and_device_attributes_are_answered() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[5n\x1b[c");
        let ev = emu.take_events();
        assert!(ev.contains(&TermEvent::Reply(b"\x1b[0n".to_vec())), "DSR 5");
        assert!(
            ev.contains(&TermEvent::Reply(b"\x1b[?6c".to_vec())),
            "primary DA"
        );
    }

    /// `CSI ? 6 n` used to be swallowed by the private-mode early return.
    #[test]
    fn decxcpr_is_answered() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[?6n");
        assert!(emu
            .take_events()
            .contains(&TermEvent::Reply(b"\x1b[?1;1R".to_vec())));
    }

    /// Only the `?` form is a query; `OSC 11 ; <colour>` sets the colour and
    /// must not trigger a reply.
    #[test]
    fn osc_colour_query_only_on_question_mark() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b]11;?\x1b\\");
        assert!(emu
            .take_events()
            .contains(&TermEvent::ColorQuery { index: 11 }));

        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b]11;rgb:00/00/00\x1b\\");
        assert!(!emu
            .take_events()
            .iter()
            .any(|e| matches!(e, TermEvent::ColorQuery { .. })));
    }

    #[test]
    fn osc133_prompt_start() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b]133;A\x07");
        assert!(emu
            .take_events()
            .contains(&TermEvent::SemanticPrompt(SemanticMarker::PromptStart)));
    }

    #[test]
    fn osc8_hyperlinks() {
        let mut emu = Emulator::new(2, 40);
        emu.feed(b"\x1b]8;id=abc;https://example.com\x07");
        let ev = emu.take_events();
        assert!(ev.iter().any(
            |e| matches!(e, TermEvent::Hyperlink { uri, .. } if uri == "https://example.com")
        ));
    }

    #[test]
    fn bracketed_paste_mode() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[?2004h");
        assert!(emu.mode().contains(TermMode::BRACKETED_PASTE));
        assert!(emu.take_events().contains(&TermEvent::BracketedPaste(true)));
        emu.feed(b"\x1b[?2004l");
        assert!(!emu.mode().contains(TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn app_cursor_mode() {
        let mut emu = Emulator::new(2, 20);
        assert!(!emu.mode().contains(TermMode::APP_CURSOR));
        emu.feed(b"\x1b[?1h");
        assert!(emu.mode().contains(TermMode::APP_CURSOR));
        emu.feed(b"\x1b[?1l");
        assert!(!emu.mode().contains(TermMode::APP_CURSOR));
    }

    #[test]
    fn app_keypad_esc_equals() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b=");
        assert!(emu.mode().contains(TermMode::APP_KEYPAD));
        emu.feed(b"\x1b>");
        assert!(!emu.mode().contains(TermMode::APP_KEYPAD));
    }

    #[test]
    fn insert_mode() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"abcde");
        emu.feed(b"\x1b[?4h"); // enable IRM (insert mode)
        emu.feed(b"\x1b[1;1H"); // cursor to column 1
        emu.feed(b"X"); // insert 'X' — should push 'a' right
                        // With insert mode: row should start with "Xabcde..."
        let text = screen_text(&emu);
        assert_eq!(&text[0][..6], "Xabcde", "insert mode failed: {:?}", text[0]);
    }

    #[test]
    fn line_feed_new_line_mode() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[?20h"); // enable LNM
                                // Move to col 5.
        emu.feed(b"\x1b[1;6H");
        emu.feed(b"\x0a"); // LF should also CR
        assert_eq!(emu.grid().cursor_col, 0, "LNM: LF should reset column to 0");
    }

    #[test]
    fn origin_mode() {
        let mut emu = Emulator::new(10, 20);
        // Set scroll region rows 3–7 (1-based → 0-based: 2–6).
        emu.feed(b"\x1b[3;7r");
        // Enable DECOM.
        emu.feed(b"\x1b[?6h");
        // CSI H with DECOM → cursor goes to scroll-region top.
        assert_eq!(
            emu.grid().cursor_row,
            2,
            "DECOM: cursor should be at scroll_top=2"
        );
    }

    #[test]
    fn alternate_scroll_mode() {
        let mut emu = Emulator::new(4, 20);
        emu.feed(b"\x1b[?1007h");
        assert!(emu.mode().contains(TermMode::ALTERNATE_SCROLL));
    }

    #[test]
    fn kitty_keyboard_flags() {
        let mut emu = Emulator::new(2, 20);
        // CSI > 1 ; 1 u — set DISAMBIGUATE_ESCAPE flag.
        emu.feed(b"\x1b[>1;1u");
        assert!(emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE));
        assert!(emu.mode().contains(TermMode::KITTY_KEYBOARD));
        // CSI > 0 ; 1 u — clear all flags.
        emu.feed(b"\x1b[>0;1u");
        assert!(!emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE));
        assert!(!emu.mode().contains(TermMode::KITTY_KEYBOARD));
    }

    #[test]
    fn synchronized_output() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[?2026h");
        assert!(emu.mode().contains(TermMode::SYNCHRONIZED_OUTPUT));
        assert!(emu
            .take_events()
            .contains(&TermEvent::SynchronizedOutput(true)));
    }

    #[test]
    fn vi_mode_host_set() {
        let mut emu = Emulator::new(2, 20);
        assert!(!emu.mode().contains(TermMode::VI));
        emu.set_mode(TermMode::VI, true);
        assert!(emu.mode().contains(TermMode::VI));
    }

    #[test]
    fn wide_char_through_emulator() {
        let mut emu = Emulator::new(4, 20);
        emu.feed("日本語".as_bytes());
        assert_eq!(emu.grid().cursor_col, 6);
    }

    #[test]
    fn tab_operations() {
        let mut emu = Emulator::new(2, 40);
        emu.feed(b"\x1b[I"); // CHT 1
        assert_eq!(emu.grid().cursor_col, 8);
    }

    #[test]
    fn hts_sets_stop() {
        let mut emu = Emulator::new(2, 40);
        emu.feed(b"\x1b[1;6H\x1bH\x1b[1;1H\x09"); // set stop at col 5, tab there
        assert_eq!(emu.grid().cursor_col, 5);
    }

    #[test]
    fn sgr_bold_flag() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[1mX\x1b[0m");
        let lines = emu.grid().screen_styled_lines();
        assert!(lines[0].spans[0].style.flags.contains(Flags::BOLD));
    }

    #[test]
    fn ris_resets_modes() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[?1h\x1b="); // APP_CURSOR + APP_KEYPAD
        emu.feed(b"\x1bc"); // RIS
        assert!(!emu.mode().contains(TermMode::APP_CURSOR));
        assert!(!emu.mode().contains(TermMode::APP_KEYPAD));
    }

    #[test]
    fn scrollback_is_btree_backed() {
        let mut emu = Emulator::new(2, 5);
        // Fill enough lines to push some into scrollback.
        for _ in 0..10 {
            emu.feed(b"abcde\r\n");
        }
        let sb_len = emu.grid().scrollback_len();
        assert!(sb_len > 0, "expected non-empty scrollback, got {sb_len}");
        // Version should be non-zero.
        assert!(emu.grid().scrollback_version() > 0);
    }

    // ── Kitty keyboard push/pop stack ─────────────────────────────────────────

    #[test]
    fn kitty_keyboard_push_pop() {
        let mut emu = Emulator::new(2, 20);
        // Set DISAMBIGUATE flag (mode 1).
        emu.feed(b"\x1b[>1;1u");
        assert!(emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE));
        assert_eq!(emu.keyboard_stack_depth(), 0);

        // PUSH (mode 4): save current flags, then set ALL_AS_ESCAPE only.
        emu.feed(b"\x1b[>8;4u");
        assert_eq!(emu.keyboard_stack_depth(), 1, "stack should have 1 entry");
        assert!(
            !emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE),
            "should be cleared after push"
        );
        assert!(emu.mode().contains(TermMode::KEYBOARD_REPORT_ALL_AS_ESCAPE));

        // POP (mode 5): restore DISAMBIGUATE.
        emu.feed(b"\x1b[>0;5u");
        assert_eq!(
            emu.keyboard_stack_depth(),
            0,
            "stack should be empty after pop"
        );
        assert!(
            emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE),
            "should be restored after pop"
        );
        assert!(!emu.mode().contains(TermMode::KEYBOARD_REPORT_ALL_AS_ESCAPE));
    }

    #[test]
    fn kitty_keyboard_stack_overflow_is_bounded() {
        let mut emu = Emulator::new(2, 20);
        // Push more than KITTY_KEYBOARD_STACK_MAX times.
        for _ in 0..20 {
            emu.feed(b"\x1b[>1;4u"); // push
        }
        assert!(emu.keyboard_stack_depth() <= KITTY_KEYBOARD_STACK_MAX);
    }

    #[test]
    fn kitty_keyboard_pop_empty_stack_clears() {
        let mut emu = Emulator::new(2, 20);
        emu.feed(b"\x1b[>1;1u"); // set DISAMBIGUATE
        emu.feed(b"\x1b[>0;5u"); // pop on empty stack → should clear flags
        assert!(!emu.mode().contains(TermMode::KEYBOARD_DISAMBIGUATE_ESCAPE));
    }

    // ── Sixel DCS ──────────────────────────────────────────────────────────────

    #[test]
    fn sixel_dcs_produces_event() {
        let mut emu = Emulator::new(4, 40);
        // Minimal valid sixel: DCS 0;0;0 q <red pixel> ST
        // #1;2;100;0;0  = register 1 = red RGB
        // #1~           = paint 6 red pixels in column 0
        let sixel_dcs = b"\x1bP0;0;0q#1;2;100;0;0#1~\x1b\\";
        emu.feed(sixel_dcs);
        let events = emu.take_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TermEvent::SixelImage { .. })),
            "expected SixelImage event, got: {events:?}"
        );
    }

    #[test]
    fn sixel_image_stored_and_retrievable() {
        let mut emu = Emulator::new(4, 40);
        let sixel_dcs = b"\x1bP0;0;0q#1;2;100;0;0#1~\x1b\\";
        emu.feed(sixel_dcs);
        let events = emu.take_events();
        let id = events
            .iter()
            .find_map(|e| match e {
                TermEvent::SixelImage { image_id } => Some(*image_id),
                _ => None,
            })
            .expect("no SixelImage event");
        let img = emu.sixel_images().get(id).expect("image not stored");
        assert_eq!(img.frames.len(), 1);
        assert!(img.frames[0].width > 0);
    }

    // ── IME ───────────────────────────────────────────────────────────────────

    #[test]
    fn ime_commit_produces_event() {
        let mut emu = Emulator::new(2, 40);
        emu.ime_begin();
        emu.ime_handle(ImeEvent::Compose {
            text: "日".into(),
            cursor: 0..3,
        });
        assert!(emu.ime_state().is_active());
        emu.ime_handle(ImeEvent::Commit("日本語".into()));
        let events = emu.take_events();
        assert!(events.contains(&TermEvent::ImeCommit("日本語".into())));
        assert!(!emu.ime_state().is_active());
    }

    #[test]
    fn ime_cancel_clears_state() {
        let mut emu = Emulator::new(2, 40);
        emu.ime_begin();
        emu.ime_handle(ImeEvent::Compose {
            text: "te".into(),
            cursor: 0..2,
        });
        emu.ime_handle(ImeEvent::Cancel);
        assert!(!emu.ime_state().is_active());
        assert!(emu.ime_state().pre_edit.is_empty());
    }

    // ── Live cell edits (update_at) ───────────────────────────────────────────

    #[test]
    fn update_scrollback_cell_live_edit() {
        let mut emu = Emulator::new(2, 20);
        // Push content into scrollback.
        for _ in 0..5 {
            emu.feed(b"hello\r\n");
        }
        let sb_len = emu.grid().scrollback_len();
        assert!(sb_len > 0, "need scrollback rows for this test");
        // Apply an underline to cell (0,0).
        let ok = emu.update_scrollback_cell(0, 0, |cell| {
            cell.flags.insert(Flags::UNDERLINE);
        });
        assert!(ok, "update_scrollback_cell should return true");
        // Verify by reading back.
        let row = emu.grid().line(0).expect("row 0 must exist");
        assert!(row.cells[0].flags.contains(Flags::UNDERLINE));
    }

    // ── Block collapse / FilterPath ───────────────────────────────────────────

    #[test]
    fn collapse_expand_block() {
        let mut emu = Emulator::new(2, 20);
        for _ in 0..6 {
            emu.feed(b"line\r\n");
        }
        let total_sb = emu.grid().scrollback_len();
        let vis_before = emu.visible_scrollback_len();
        assert_eq!(vis_before, total_sb, "all rows visible before collapse");
        // Collapse 2 rows.
        emu.collapse_block(0, 2);
        let vis_after = emu.visible_scrollback_len();
        assert_eq!(vis_after, total_sb - 2, "2 rows hidden after collapse");
        // Re-expand.
        emu.expand_block(0, 2);
        assert_eq!(
            emu.visible_scrollback_len(),
            total_sb,
            "all visible after expand"
        );
    }
}
