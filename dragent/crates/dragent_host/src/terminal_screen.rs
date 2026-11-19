//! Daemon-owned display state and query replies. Colour queries are answered
//! here too: apps send one followed by a device query as a marker, and treat an
//! answer that arrives after the marker's as no answer at all. The UI still
//! handles bells and images during migration.
use crate::screen_protocol::{
    ScreenColor, ScreenCursorShape, ScreenPen, ScreenRow, ScreenSpan, TerminalColors,
    TerminalScreen,
};
use dragent_terminal::{Color, CursorShape, Emulator, HookEvent, TermEvent, TermMode};

pub(crate) struct SessionScreen {
    emulator: Emulator,
    offset: u64,
    complete: bool,
    title: String,
    cwd: String,
    cursor_shape: ScreenCursorShape,
    replay: std::collections::VecDeque<crate::screen_protocol::OperationFrame>,
    replay_bytes: usize,
    shell_ready: bool,
    starting: Option<String>,
    replies: Vec<Vec<u8>>,
    command: Option<String>,
    colors: TerminalColors,
    /// What the running program was last told, for a UI attaching later.
    answered_colors: Option<TerminalColors>,
}

impl SessionScreen {
    pub(crate) fn mark_incomplete(&mut self) {
        self.complete = false;
    }
    pub(crate) fn new(rows: u16, cols: u16, cwd: String, offset: u64) -> Self {
        // Zero means unlimited; retain a bounded history for normal-screen reflow.
        let mut emulator = Emulator::with_scrollback(rows as usize, cols as usize, 2_000);
        emulator.set_audible_bell(false);
        Self {
            emulator,
            offset,
            complete: offset == 0,
            title: String::new(),
            cwd,
            cursor_shape: ScreenCursorShape::Default,
            replay: Default::default(),
            replay_bytes: 0,
            shell_ready: false,
            starting: None,
            replies: Vec::new(),
            command: None,
            colors: TerminalColors::default(),
            answered_colors: None,
        }
    }

    /// `shell` was spawned with integration and has not shown its first prompt.
    pub(crate) fn mark_starting(&mut self, shell: String) {
        if !self.shell_ready {
            self.starting = Some(shell);
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> crate::screen_protocol::OperationFrame {
        let operations = self.emulator.feed_recorded(bytes);
        let frame = crate::screen_protocol::OperationFrame {
            base: self.offset,
            resume: None,
            data: bytes.to_vec(),
            operations: std::sync::Arc::new(operations),
        };
        self.replay_bytes += frame.memory_size();
        self.replay.push_back(frame.clone());
        // Bound decoded memory too: a few raw bytes can finish a large OSC.
        while self.replay_bytes > 512 * 1024 && self.replay.len() > 1 {
            let old = self.replay.pop_front().unwrap();
            self.replay_bytes -= old.memory_size();
        }
        self.offset += bytes.len() as u64;
        // Device/cursor queries are answered even with no renderer attached.
        for event in self.emulator.take_events() {
            match event {
                TermEvent::Reply(bytes) => self.replies.push(bytes),
                TermEvent::ColorQuery { index } => {
                    let rgb = if index == 10 {
                        self.colors.foreground
                    } else {
                        self.colors.background
                    };
                    self.replies
                        .push(dragent_terminal::color_query_report(index, rgb));
                    self.answered_colors = Some(self.colors);
                }
                TermEvent::ColorSchemeQuery => self
                    .replies
                    .push(dragent_terminal::color_scheme_report(self.colors.is_dark)),
                TermEvent::TitleChanged(title) => self.title = title,
                TermEvent::Hook(HookEvent::Precmd { cwd, .. }) => {
                    self.cwd = cwd;
                    self.shell_ready = true;
                    self.starting = None;
                    self.command = None;
                    self.answered_colors = None;
                }
                TermEvent::Hook(HookEvent::Preexec { command }) => {
                    self.command = Some(command);
                    self.answered_colors = None;
                }
                TermEvent::CursorShape(shape) => {
                    self.cursor_shape = match shape {
                        CursorShape::Default => ScreenCursorShape::Default,
                        CursorShape::BlinkingBlock => ScreenCursorShape::BlinkingBlock,
                        CursorShape::SteadyBlock => ScreenCursorShape::SteadyBlock,
                        CursorShape::BlinkingUnderline => ScreenCursorShape::BlinkingUnderline,
                        CursorShape::SteadyUnderline => ScreenCursorShape::SteadyUnderline,
                        CursorShape::BlinkingBar => ScreenCursorShape::BlinkingBar,
                        CursorShape::SteadyBar => ScreenCursorShape::SteadyBar,
                    }
                }
                _ => {}
            }
        }
        frame
    }

    pub(crate) fn shell_ready(&self) -> bool {
        self.shell_ready
    }
    /// Adopt the terminal's colours. Returns the report to send the running app
    /// when the scheme flipped and it asked to hear about that.
    pub(crate) fn set_colors(&mut self, colors: TerminalColors) -> Option<Vec<u8>> {
        let flipped = self.colors.is_dark != colors.is_dark;
        self.colors = colors;
        let wanted = self
            .emulator
            .mode()
            .contains(TermMode::COLOR_SCHEME_UPDATES);
        (flipped && wanted).then(|| dragent_terminal::color_scheme_report(colors.is_dark))
    }

    pub(crate) fn take_replies(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.replies)
    }

    pub(crate) fn operation_checkpoint(&self) -> crate::screen_protocol::OperationCheckpoint {
        crate::screen_protocol::OperationCheckpoint {
            parser: self.emulator.parser_checkpoint(),
            frames: self.replay.iter().cloned().collect(),
            resume: self.resume().ok(),
        }
    }

    pub(crate) fn restore_operations(
        &mut self,
        checkpoint: crate::screen_protocol::OperationCheckpoint,
    ) -> Result<(), String> {
        let mut previous = None;
        let mut memory = 0usize;
        for frame in &checkpoint.frames {
            if frame.data.is_empty()
                || frame.data.len() > 4096
                || previous.is_some_and(|end| end != frame.base)
            {
                return Err("invalid terminal operation replay checkpoint".into());
            }
            previous = Some(frame.end());
            memory = memory.saturating_add(frame.memory_size());
        }
        if previous.is_some_and(|end| end != self.offset) || memory > 512 * 1024 {
            return Err("terminal operation checkpoint exceeds replay bounds".into());
        }
        self.emulator.restore_parser_checkpoint(checkpoint.parser)?;
        if let Some(resume) = checkpoint.resume {
            self.emulator.restore_display_checkpoint(&resume.display)?;
            self.shell_ready = resume.shell_ready || resume.command.is_some();
            // An adopted shell is treated as started: no startup queue
            // survives a daemon handoff, so input must reach it directly.
            self.starting = None;
            self.cwd = resume.cwd;
            self.command = resume.command;
            self.complete = true;
        }
        self.replay = checkpoint.frames.into();
        self.replay_bytes = memory;
        Ok(())
    }

    pub(crate) fn replay_since(&self, offset: u64) -> Result<Vec<crate::fanout::Delivery>, String> {
        if offset > self.offset {
            return Err("terminal operation offset is ahead of output".into());
        }
        if offset == 0 || self.replay.front().is_some_and(|frame| offset < frame.base) {
            // Capture and subscribe under the registry's output lock: no
            // output can slip between the checkpoint and live subscription.
            return Ok(vec![crate::fanout::Delivery::Operations(
                crate::screen_protocol::OperationFrame {
                    base: self.offset,
                    data: Vec::new(),
                    operations: Default::default(),
                    resume: Some(self.resume()?),
                },
            )]);
        }
        if self
            .replay
            .iter()
            .any(|f| f.base < offset && offset < f.end())
        {
            return Err("terminal operation offset is not a frame boundary".into());
        }
        Ok(self
            .replay
            .iter()
            .filter(|f| f.end() > offset)
            .cloned()
            .map(crate::fanout::Delivery::Operations)
            .collect())
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        if self.emulator.dimensions() != (rows as usize, cols as usize) {
            self.emulator.resize(rows as usize, cols as usize);
        }
    }

    fn resume(&self) -> Result<crate::screen_protocol::TerminalResume, String> {
        if !self.complete {
            return Err("terminal state unavailable after legacy daemon handoff; refusing incomplete reattachment".into());
        }
        Ok(crate::screen_protocol::TerminalResume {
            display: self
                .emulator
                .display_checkpoint()
                .map_err(|error| format!("terminal state unavailable: {error}"))?,
            cwd: self.cwd.clone(),
            command: self.command.clone(),
            shell_ready: self.shell_ready,
            starting: self.starting.clone().filter(|_| !self.shell_ready),
            answered_colors: self.answered_colors,
        })
    }

    pub(crate) fn snapshot(&self) -> Result<TerminalScreen, String> {
        let (rows, cols) = self.emulator.dimensions();
        // Bound lock hold time and JSON expansion for pathological dimensions.
        if rows.saturating_mul(cols) > 65_536 {
            return Err("terminal viewport exceeds snapshot cell limit".into());
        }
        let alternate_screen = self.emulator.alt_screen_active();
        let grid = if alternate_screen {
            self.emulator.alt_grid()
        } else {
            self.emulator.grid()
        };
        let lines = grid
            .screen_styled_lines()
            .into_iter()
            .map(|line| ScreenRow {
                wrapped: line.wrapped,
                spans: line
                    .spans
                    .into_iter()
                    .map(|span| ScreenSpan {
                        text: span.text,
                        foreground: color(span.style.fg),
                        background: color(span.style.bg),
                        flags: span.style.flags.bits(),
                        underline_color: span.style.underline_color.map(color),
                    })
                    .collect(),
            })
            .collect();
        let (cursor_row, cursor_col) = self.emulator.cursor_position();
        let mode = self.emulator.mode();
        Ok(TerminalScreen {
            revision: self.emulator.render_revision(),
            output_offset: self.offset,
            complete: self.complete,
            rows: rows as u16,
            cols: cols as u16,
            cursor_row: cursor_row as u16,
            cursor_col: cursor_col as u16,
            cursor_visible: self.emulator.cursor_visible(),
            cursor_shape: self.cursor_shape,
            alternate_screen,
            // Why separate booleans: the phone reads modes without decoding
            // the raw mode word, and old daemons omit them (serde default).
            mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            parser_ground: self.emulator.parser_is_ground(),
            parser_prefix: self.emulator.parser_pending_prefix(),
            scroll_region: Some((
                grid.scroll_region_top() as u16,
                grid.scroll_region_bottom() as u16,
            )),
            saved_cursor: grid.saved_cursor().map(|(r, c)| (r as u16, c as u16)),
            pen: {
                let style = self.emulator.current_pen();
                ScreenPen {
                    foreground: color(style.fg),
                    background: color(style.bg),
                    flags: style.flags.bits(),
                    underline_color: style.underline_color.map(color),
                }
            },
            pending_wrap: grid
                .pending_wrap()
                .then(|| {
                    let row = grid.screen_rows().get(cursor_row)?;
                    let mut col = cursor_col;
                    let spacer = |c: usize| {
                        row.cells.get(c).is_some_and(|cell| {
                            cell.flags
                                .contains(dragent_terminal::Flags::WIDE_CHAR_SPACER)
                        })
                    };
                    if col > 0 && spacer(col) {
                        col -= 1;
                    }
                    let cell = row.cells.get(col)?;
                    Some((
                        col as u16,
                        ScreenSpan {
                            // Combining marks cannot be re-attached at the edge.
                            text: cell.ch.to_string(),
                            foreground: color(cell.fg),
                            background: color(cell.bg),
                            flags: (cell.flags & dragent_terminal::Flags::SGR_MASK).bits(),
                            underline_color: cell
                                .extra()
                                .and_then(|e| e.underline_color)
                                .map(color),
                        },
                    ))
                })
                .flatten(),
            modes: mode.bits(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            lines,
        })
    }
}

fn color(color: Color) -> ScreenColor {
    match color {
        Color::Default => ScreenColor::Default,
        Color::Named(n) => ScreenColor::Indexed(n.index()),
        Color::Indexed(n) => ScreenColor::Indexed(n),
        Color::Rgb(r, g, b) => ScreenColor::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> TerminalColors {
        TerminalColors {
            foreground: (0, 0, 0),
            background: (250, 250, 250),
            is_dark: false,
        }
    }

    #[test]
    fn colour_queries_are_answered_in_order_with_the_marker_after_them() {
        let mut screen = SessionScreen::new(24, 80, String::new(), 0);
        screen.set_colors(light());
        screen.feed(b"\x1b]11;?\x1b\\\x1b[c");
        assert_eq!(
            screen.take_replies(),
            vec![
                b"\x1b]11;rgb:fafa/fafa/fafa\x1b\\".to_vec(),
                b"\x1b[?6c".to_vec(),
            ]
        );
        screen.feed(b"\x1b[?996n");
        assert_eq!(screen.take_replies(), vec![b"\x1b[?997;2n".to_vec()]);
    }

    #[test]
    fn what_the_running_program_was_told_survives_a_theme_change() {
        let mut screen = SessionScreen::new(24, 80, String::new(), 0);
        screen.feed(b"\x1b]11;?\x1b\\");
        screen.set_colors(light());
        assert_eq!(screen.answered_colors, Some(TerminalColors::default()));
        screen.feed(b"\x1b]11;?\x1b\\");
        assert_eq!(screen.answered_colors, Some(light()));
    }

    #[test]
    fn a_scheme_flip_is_announced_only_to_an_app_that_asked() {
        let mut screen = SessionScreen::new(24, 80, String::new(), 0);
        assert_eq!(screen.set_colors(light()), None);
        screen.feed(b"\x1b[?2031h");
        // A new colour in the same scheme is not a flip.
        assert_eq!(
            screen.set_colors(TerminalColors {
                background: (240, 240, 240),
                ..light()
            }),
            None
        );
        assert_eq!(
            screen.set_colors(TerminalColors::default()),
            Some(b"\x1b[?997;1n".to_vec())
        );
    }

    #[test]
    fn reconnect_restores_running_command_and_modes_after_replay_eviction() {
        let mut screen = SessionScreen::new(4, 40, "/repo".into(), 0);
        screen.feed(b"\x1b]7979;precmd;0;/repo\x07\x1b]7979;preexec;claude --resume abc\x07");
        screen.feed(b"primary\x1b[?1049h\x1b[?1006h\x1b[?1000h\x1b[?2004h\x1b[>1uagent waiting");
        for _ in 0..600 {
            screen.feed(&[0; 1024]);
        }
        assert!(screen.replay.front().unwrap().base > 0);
        let replay = screen.replay_since(0).unwrap();
        let crate::fanout::Delivery::Operations(frame) = &replay[0] else {
            panic!("missing checkpoint")
        };
        assert_eq!(frame.base, screen.offset);
        assert!(frame.data.is_empty());
        let resume = frame.resume.as_ref().unwrap();
        assert_eq!(resume.command.as_deref(), Some("claude --resume abc"));
        let mut restored = Emulator::new(1, 1);
        restored
            .restore_display_checkpoint(&resume.display)
            .unwrap();
        assert!(restored.alt_screen_active());
        assert_eq!(restored.mode(), screen.emulator.mode());
        assert_eq!(
            restored.keyboard_stack_depth(),
            screen.emulator.keyboard_stack_depth()
        );
        assert_eq!(
            restored.cursor_position(),
            screen.emulator.cursor_position()
        );
        assert!(
            restored.take_events().is_empty(),
            "checkpoint must not replay replies/hooks"
        );
        restored.feed(b"\x1b[?1049l");
        assert!(restored.grid().screen_styled_lines()[0]
            .plain_text()
            .contains("primary"));
        let checkpoint = screen.operation_checkpoint();
        let mut adopted = SessionScreen::new(4, 40, String::new(), screen.offset);
        adopted.restore_operations(checkpoint).unwrap();
        assert!(adopted.complete);
        assert_eq!(adopted.command, screen.command);
        assert!(adopted.emulator.alt_screen_active());
        adopted.feed(b"\x1b[?1049l\x1b]7979;precmd;0;/repo\x07");
        assert!(adopted.command.is_none());
    }

    fn text(screen: &TerminalScreen) -> String {
        screen
            .lines
            .iter()
            .flat_map(|row| &row.spans)
            .map(|span| span.text.as_str())
            .collect()
    }

    #[test]
    fn fragmented_output_preserves_unicode_style_cursor_and_modes() {
        let mut screen = SessionScreen::new(4, 40, "/tmp".into(), 0);
        let data = "\x1b]0;Agent\x07\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[38;2;1;2;3m日本 e\u{301}\x1b[3;7H\x1b[6 q";
        for byte in data.as_bytes() {
            screen.feed(&[*byte]);
        }
        let snapshot = screen.snapshot().unwrap();
        assert!(snapshot.complete);
        assert!(snapshot.alternate_screen);
        assert_eq!(snapshot.output_offset, data.len() as u64);
        assert_eq!(snapshot.title, "Agent");
        assert_eq!((snapshot.cursor_row, snapshot.cursor_col), (2, 6));
        assert_eq!(snapshot.cursor_shape, ScreenCursorShape::SteadyBar);
        assert_eq!(snapshot.lines.len(), 4);
        assert_eq!(
            snapshot.lines[0].spans[0].foreground,
            ScreenColor::Rgb(1, 2, 3)
        );
        assert!(text(&snapshot).contains("日本 e\u{301}"));
        assert_ne!(
            snapshot.modes & dragent_terminal::TermMode::BRACKETED_PASTE.bits(),
            0
        );
        // Phone reads modes without decoding the raw word.
        assert!(snapshot.mouse_reporting);
        assert!(snapshot.sgr_mouse);
        assert!(snapshot.bracketed_paste);
        let encoded = crate::protocol::encode_message(&crate::protocol::Response::TerminalScreen(
            snapshot.clone(),
        ))
        .unwrap();
        let decoded: crate::protocol::Response = crate::protocol::decode_message(&encoded).unwrap();
        assert!(
            matches!(decoded, crate::protocol::Response::TerminalScreen(back) if back == snapshot)
        );
    }

    #[test]
    fn screen_survives_output_beyond_raw_replay_capacity() {
        let mut screen = SessionScreen::new(4, 40, "/tmp".into(), 0);
        screen.feed(b"primary\x1b[?1049h\x1b[Halt");
        for _ in 0..600 {
            screen.feed(&[0; 1024]);
        }
        assert!(screen.snapshot().unwrap().output_offset > crate::terminals::REPLAY_CAP as u64);
        assert!(text(&screen.snapshot().unwrap()).contains("alt"));
        screen.feed(b"\x1b[?1049l");
        assert!(text(&screen.snapshot().unwrap()).contains("primary"));
        assert!(!screen.snapshot().unwrap().alternate_screen);
    }

    #[test]
    fn resize_and_cwd_are_reflected_without_more_output() {
        let mut screen = SessionScreen::new(4, 40, "/tmp".into(), 0);
        screen.feed(b"\x1b]7979;precmd;0;/repo\x07");
        let previous = screen.snapshot().unwrap();
        screen.resize(8, 60);
        let resized = screen.snapshot().unwrap();
        assert_eq!(
            (resized.rows, resized.cols, resized.lines.len()),
            (8, 60, 8)
        );
        assert_eq!(resized.cwd, "/repo");
        assert_eq!(resized.output_offset, previous.output_offset);
        assert!(resized.revision > previous.revision);
        screen.resize(8, 60);
        assert_eq!(screen.snapshot().unwrap().revision, resized.revision);
        screen.mark_incomplete();
        assert!(!screen.snapshot().unwrap().complete);
    }

    #[test]
    fn snapshot_reports_mouse_sgr_and_bracketed_modes() {
        let mut screen = SessionScreen::new(4, 40, "/tmp".into(), 0);
        let fresh = screen.snapshot().unwrap();
        assert!(!fresh.mouse_reporting);
        assert!(!fresh.sgr_mouse);
        assert!(!fresh.bracketed_paste);
        // Any tracking mode counts: 1002 (drag) also reports mouse on.
        screen.feed(b"\x1b[?1002h\x1b[?2004h");
        let on = screen.snapshot().unwrap();
        assert!(on.mouse_reporting);
        assert!(!on.sgr_mouse);
        assert!(on.bracketed_paste);
        screen.feed(b"\x1b[?1006h");
        assert!(screen.snapshot().unwrap().sgr_mouse);
        screen.feed(b"\x1b[?1002l\x1b[?2004l\x1b[?1006l");
        let off = screen.snapshot().unwrap();
        assert!(!off.mouse_reporting);
        assert!(!off.sgr_mouse);
        assert!(!off.bracketed_paste);
    }

    #[test]
    fn old_daemon_snapshot_without_modes_still_parses() {
        // Long-lived PTY daemons may omit the new booleans; they default off.
        let json = serde_json::json!({
            "revision": 1,
            "output_offset": 0,
            "complete": true,
            "rows": 2,
            "cols": 3,
            "cursor_row": 0,
            "cursor_col": 0,
            "cursor_visible": true,
            "cursor_shape": "Default",
            "alternate_screen": true,
            "modes": 0,
            "title": "",
            "cwd": "",
            "lines": []
        });
        let screen: TerminalScreen = serde_json::from_value(json).unwrap();
        assert!(screen.alternate_screen);
        assert!(!screen.mouse_reporting);
        assert!(!screen.sgr_mouse);
        assert!(!screen.bracketed_paste);
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;
    use crate::screen_protocol::terminal_seed;

    fn round_trip(bytes: &[u8], rows: u16, cols: u16) {
        let mut original = SessionScreen::new(rows, cols, String::new(), 0);
        original.feed(bytes);
        let expected = original.snapshot().unwrap();
        let seed = terminal_seed(&expected).unwrap();
        let mut restored = SessionScreen::new(rows, cols, String::new(), 0);
        restored.feed(&seed);
        let actual = restored.snapshot().unwrap();
        assert_eq!(actual.lines, expected.lines);
        assert_eq!(
            (actual.cursor_row, actual.cursor_col),
            (expected.cursor_row, expected.cursor_col)
        );
        assert_eq!(actual.cursor_visible, expected.cursor_visible);
        assert_eq!(actual.cursor_shape, expected.cursor_shape);
        assert_eq!(actual.modes, expected.modes);
        assert_eq!(actual.title, expected.title);
        assert_eq!(
            restored.emulator.mouse_state().reporting,
            original.emulator.mouse_state().reporting
        );
        assert_eq!(
            restored.emulator.mouse_state().encoding,
            original.emulator.mouse_state().encoding
        );
    }

    fn continuation(a: &[u8], b: &[u8]) {
        let mut desktop = SessionScreen::new(24, 80, String::new(), 0);
        desktop.feed(a);
        let snapshot = desktop.snapshot().unwrap();
        let seed = terminal_seed(&snapshot).unwrap();
        assert!(seed.len() <= 256 * 1024);
        assert_eq!(snapshot.output_offset, a.len() as u64);
        let mut phone = SessionScreen::new(24, 80, String::new(), 0);
        phone.feed(&seed);
        for bytes in [b"".as_slice(), b] {
            desktop.feed(bytes);
            phone.feed(bytes);
            let expected = desktop.snapshot().unwrap();
            let actual = phone.snapshot().unwrap();
            assert_eq!(actual.lines, expected.lines);
            assert_eq!(
                (actual.cursor_row, actual.cursor_col),
                (expected.cursor_row, expected.cursor_col)
            );
            assert_eq!(actual.cursor_visible, expected.cursor_visible);
            assert_eq!(actual.modes, expected.modes);
            assert_eq!(actual.scroll_region, expected.scroll_region);
            assert_eq!(actual.saved_cursor, expected.saved_cursor);
            assert_eq!(actual.pen, expected.pen);
            assert_eq!(actual.pending_wrap, expected.pending_wrap);
            assert_eq!(actual.parser_ground, expected.parser_ground);
            assert_eq!(actual.parser_prefix, expected.parser_prefix);
            assert_eq!(actual.title, expected.title);
        }
    }

    #[test]
    fn vt_seed_continues_scrolling_inside_margins() {
        let mut b = Vec::new();
        for i in 0..200 {
            b.extend_from_slice(format!("line {i:06}\r\n").as_bytes());
        }
        continuation(
            b"\x1b[?1049h\x1b[2J\x1b[1;1HHEADER READY\x1b[24;1HFOOTER\x1b[3;20r\x1b[3;1H",
            &b,
        );
    }

    #[test]
    fn vt_seed_continues_current_pen_and_saved_cursor() {
        continuation(
            b"\x1b[?1049h\x1b[2J\x1b[31mREADY\x1b7\x1b[4;1H",
            &b"x".repeat(200),
        );
        continuation(
            b"\x1b[?1049h\x1b[2JREADY\x1b7\x1b[4;1H",
            &b"\x1b8x\x1b7\x1b[4;1H".repeat(200),
        );
        continuation(
            b"\x1b[1;2;3;5;7;8;9;53;38;2;11;22;33;48;5;42;58;2;3;4;5m\x1b[4:3m",
            b"drawn with active attributes\x1b[2K\r\nnext",
        );
    }

    #[test]
    fn vt_seed_continues_origin_insert_and_disabled_wrap() {
        continuation(
            b"\x1b[?1049h\x1b[3;20r\x1b[?6h\x1b[2;3Hsaved\x1b7\x1b[?7l\x1b[4h\x1b[5;2H",
            b"abc\x1b8insert\r\nmore",
        );
        // The saved position can be outside the region when origin mode changes.
        continuation(
            b"\x1b[1;4H\x1b7\x1b[3;20r\x1b[?6h\x1b8",
            b"above margin\x1b[2;1Hinside",
        );
    }

    #[test]
    fn vt_seed_keeps_the_cursor_waiting_to_wrap_at_the_right_edge() {
        let full = "x".repeat(80);
        // A long line split exactly at the edge continues on the next row.
        continuation(
            format!("\x1b[?1049h\x1b[2;1H\x1b[32m{full}").as_bytes(),
            b"continued\r\nnext",
        );
        // A wide character ending at the edge, then text in insert mode.
        continuation(
            format!("\x1b[?1049h\x1b[4h\x1b[3;1H{}\u{4e2d}", "y".repeat(78)).as_bytes(),
            "z\u{4e2d}w".as_bytes(),
        );
        // The bottom row scrolls the region when the next character arrives.
        continuation(
            format!("\x1b[?1049h\x1b[2;24r\x1b[24;1H\x1b[44m{full}").as_bytes(),
            b"scrolled",
        );
        // With autowrap off the next character overwrites the edge.
        continuation(
            format!("\x1b[?1049h\x1b[?7l\x1b[5;1H{full}").as_bytes(),
            b"!\x1b[?7hwrapped",
        );
    }

    #[test]
    fn vt_seed_resumes_split_sequences_immediately_at_current_offset() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"\x1b[?1049h\x1b[2JREADY\x1b[31", b"mX"),
            (b"\x1b[?1049h\x1b[2JREADY\xe4\xb8", b"\xadx"),
            (b"READY\x1b[3\n1", b"mX"),
            (b"READY\x1b]2;pending title", b"\x07text"),
            (b"READY\x1b]2;pending title\x1b", b"\\text"),
            (b"READY\x1bPpayload", b"\x1b\\text"),
            (b"READY\x1bPpayload\x1b", b"\\text"),
            (b"READY\x1b_ignored", b"\x1b\\text"),
        ];
        for (a, b) in cases {
            continuation(a, b);
        }
    }

    #[test]
    fn vt_seed_rejects_unknown_parser_state_from_older_daemon() {
        let mut screen = SessionScreen::new(2, 3, String::new(), 0)
            .snapshot()
            .unwrap();
        screen.parser_ground = false;
        assert!(terminal_seed(&screen).is_err());
    }

    #[test]
    fn vt_seed_restores_styled_alt_screen_and_input_modes() {
        round_trip(
            concat!(
                "\x1b[?1049h\x1b[38;5;203;48;2;10;20;30m┌─界🙂─┐   ",
                "\x1b[2;1H\x1b[0;1;2;3;4;5;7;8;9;53mstyled",
                "\x1b[4:2m\x1b[58;2;99;88;77m double",
                "\x1b[3;1H\x1b[0m\x1b[4:3mcurly e\u{301}",
                "\x1b[4;1H\x1b[0;48;5;42m                    ",
                "\x1b[0m\x1b[?25l\x1b[?1003h\x1b[?1006h\x1b[?2004h",
                "\x1b[?1h\x1b[?1004h\x1b[6 q\x1b]2;terminal title\x1b\\\x1b[2;4H"
            )
            .as_bytes(),
            6,
            20,
        );
    }

    #[test]
    fn vt_seed_restores_wrapped_rows_and_disabled_wrap() {
        round_trip(b"abcdefghijklmnopqrstu\x1b[?7l\x1b[2;3H", 4, 10);
    }

    #[test]
    fn vt_seed_restores_cursor_shapes_and_drag_mouse() {
        for shape in 1..=6 {
            round_trip(
                format!(
                    "\x1b[?1002h\x1b[?1006h\x1b[?4h\x1b[?1h\x1b=\x1b[=3u\x1b[{} q",
                    shape
                )
                .as_bytes(),
                4,
                10,
            );
        }
    }

    #[test]
    fn vt_seed_rejects_incomplete_or_oversized_viewport() {
        let mut screen = SessionScreen::new(2, 3, String::new(), 0)
            .snapshot()
            .unwrap();
        screen.complete = false;
        assert!(terminal_seed(&screen).is_err());
        screen.complete = true;
        screen.title = "x".repeat(256 * 1024);
        assert!(terminal_seed(&screen).is_err());
    }
}
