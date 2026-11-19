//! Viewport and continuation state at an atomic output offset. Images and
//! scrollback are not transported. Pending parser state uses a bounded VT prefix.
use serde::{Deserialize, Serialize};

/// The colours a terminal shows, for the queries apps use to pick their own
/// palette. Each channel is 8-bit RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColors {
    pub foreground: (u8, u8, u8),
    pub background: (u8, u8, u8),
    pub is_dark: bool,
}

impl Default for TerminalColors {
    fn default() -> Self {
        Self {
            foreground: (255, 255, 255),
            background: (0, 0, 0),
            is_dark: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenSpan {
    pub text: String,
    pub foreground: ScreenColor,
    pub background: ScreenColor,
    pub flags: u16,
    pub underline_color: Option<ScreenColor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRow {
    pub spans: Vec<ScreenSpan>,
    pub wrapped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenCursorShape {
    Default,
    BlinkingBlock,
    SteadyBlock,
    BlinkingUnderline,
    SteadyUnderline,
    BlinkingBar,
    SteadyBar,
}

/// Attributes for the next printable character, independent of the grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenPen {
    pub foreground: ScreenColor,
    pub background: ScreenColor,
    pub flags: u16,
    pub underline_color: Option<ScreenColor>,
}

impl Default for ScreenPen {
    fn default() -> Self {
        Self {
            foreground: ScreenColor::Default,
            background: ScreenColor::Default,
            flags: 0,
            underline_color: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScreen {
    pub revision: u64,
    /// Absolute output offset incorporated in this snapshot.
    pub output_offset: u64,
    /// False after a legacy daemon handoff with a truncated raw replay ring.
    /// Such a snapshot cannot be used as a complete rendering baseline.
    pub complete: bool,
    pub rows: u16,
    pub cols: u16,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
    pub cursor_shape: ScreenCursorShape,
    pub alternate_screen: bool,
    /// Any mouse tracking mode is on (1000/1002/1003). Omitted by older
    /// daemons; defaults to false so old snapshots still parse.
    #[serde(default)]
    pub mouse_reporting: bool,
    /// SGR mouse encoding (1006) is on. Omitted by older daemons.
    #[serde(default)]
    pub sgr_mouse: bool,
    /// Bracketed paste (2004) is on. Omitted by older daemons.
    #[serde(default)]
    pub bracketed_paste: bool,
    /// Absent on old daemons: never assume their parser boundary is safe.
    #[serde(default)]
    pub parser_ground: bool,
    /// Canonical unfinished sequence, appended after the redraw so raw bytes
    /// resume at output_offset. Excludes controls already applied to the grid.
    #[serde(default, with = "crate::protocol::tail_bytes")]
    pub parser_prefix: Vec<u8>,
    /// Inclusive, zero-based margins; absent on old display-only snapshots.
    #[serde(default)]
    pub scroll_region: Option<(u16, u16)>,
    /// Position only, matching what the desktop emulator stores for DECSC.
    #[serde(default)]
    pub saved_cursor: Option<(u16, u16)>,
    #[serde(default)]
    pub pen: ScreenPen,
    /// Set when the cursor waits to wrap at the right edge: the column and
    /// base character (with its attributes) of the cell that put it there.
    /// A cursor move would lose that state, so the seed prints this cell again.
    #[serde(default)]
    pub pending_wrap: Option<(u16, ScreenSpan)>,
    // Origin, autowrap and insert mode are carried in the existing mode word.
    pub modes: u32,
    pub title: String,
    pub cwd: String,
    /// Active viewport, including trailing blank rows, without scrollback.
    pub lines: Vec<ScreenRow>,
}

/// One atomic parser delivery. Offsets refer to source bytes, including chunks
/// that only extend a partial escape sequence and produce no operations yet.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OperationFrame {
    pub base: u64,
    /// Atomic restoration at `base`; consumes no source bytes. Sent before
    /// live operations to new clients or clients that fell behind the ring.
    #[serde(default)]
    pub resume: Option<TerminalResume>,
    #[serde(with = "crate::protocol::tail_bytes")]
    pub data: Vec<u8>,
    #[serde(with = "operation_bytes")]
    pub operations: std::sync::Arc<Vec<dragent_terminal::operations::Operation>>,
}

impl OperationFrame {
    pub fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }
    pub fn memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.resume.as_ref().map_or(0, |resume| {
                resume.display.len()
                    + resume.cwd.len()
                    + resume.command.as_ref().map_or(0, String::len)
            })
            + self.data.capacity()
            + self
                .operations
                .iter()
                .map(|op| op.memory_size())
                .sum::<usize>()
            + (self.operations.capacity() - self.operations.len())
                * std::mem::size_of::<dragent_terminal::operations::Operation>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TerminalResume {
    #[serde(with = "crate::protocol::tail_bytes")]
    pub display: Vec<u8>,
    pub cwd: String,
    pub command: Option<String>,
    #[serde(default)]
    pub shell_ready: bool,
    /// The shell's path while it is still starting: its integration is
    /// installed but its first prompt has not arrived. Input sent meanwhile is
    /// held by the daemon and delivered once the prompt does.
    #[serde(default)]
    pub starting: Option<String>,
    /// The colours the running program was last told, if it asked.
    #[serde(default)]
    pub answered_colors: Option<TerminalColors>,
}

/// Parser continuity, exact frame boundaries and display/input state across a
/// daemon replacement. Legacy exports lack `resume` and require full raw replay.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationCheckpoint {
    pub parser: dragent_terminal::parser::Parser,
    pub frames: Vec<OperationFrame>,
    #[serde(default)]
    pub resume: Option<TerminalResume>,
}

// Keep high-volume operations out of JSON. Varint encoding packs CSI fields,
// and string/image payloads travel as bytes in the existing binary tail.
mod operation_bytes {
    use bincode::Options;
    use dragent_terminal::operations::Operation;
    use serde::{Deserializer, Serializer};
    use std::sync::Arc;

    fn codec() -> impl Options {
        bincode::DefaultOptions::new()
            .with_varint_encoding()
            .with_limit(512 * 1024)
            .reject_trailing_bytes()
    }

    pub fn serialize<S: Serializer>(
        operations: &Arc<Vec<Operation>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let bytes = codec()
            .serialize(operations.as_ref())
            .map_err(serde::ser::Error::custom)?;
        crate::protocol::tail_bytes::serialize(&bytes, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<Vec<Operation>>, D::Error> {
        let bytes = crate::protocol::tail_bytes::deserialize(deserializer)?;
        let operations: Vec<Operation> = codec()
            .deserialize(&bytes)
            .map_err(serde::de::Error::custom)?;
        if operations.iter().map(Operation::memory_size).sum::<usize>() > 512 * 1024 {
            return Err(serde::de::Error::custom(
                "terminal operation frame exceeds memory limit",
            ));
        }
        Ok(Arc::new(operations))
    }
}

/// Encode a complete viewport as VT, for a fresh parser of the same size.
/// The redraw finishes in parser ground state, then the pending prefix restores
/// an unfinished escape/UTF-8 sequence without re-executing embedded controls.
/// The daemon snapshots its grid and output_offset atomically; callers must
/// resume raw output at that offset rather than replaying pre-snapshot bytes.
pub fn terminal_seed(screen: &TerminalScreen) -> Result<Vec<u8>, String> {
    if screen.parser_ground != screen.parser_prefix.is_empty() {
        return Err("terminal parser continuation is unavailable".into());
    }
    if !screen.complete
        || (screen.rows == 0 || screen.cols == 0)
        || u32::from(screen.rows) * u32::from(screen.cols) > 65_536
        || screen.lines.len() != screen.rows as usize
    {
        return Err("incomplete terminal screen".into());
    }
    let mut out = Vec::with_capacity((screen.rows as usize) * (screen.cols as usize).min(256));
    out.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[H");
    if screen.alternate_screen {
        out.extend_from_slice(b"\x1b[?1049h\x1b[2J\x1b[H");
    }
    if screen.mouse_reporting {
        out.extend_from_slice(b"\x1b[?1000h");
    }
    if screen.sgr_mouse {
        out.extend_from_slice(b"\x1b[?1006h");
    }
    if screen.bracketed_paste {
        out.extend_from_slice(b"\x1b[?2004h");
    }
    if !screen.cursor_visible {
        out.extend_from_slice(b"\x1b[?25l");
    }
    if !screen.title.is_empty() {
        out.extend_from_slice(b"\x1b]2;");
        out.extend_from_slice(screen.title.as_bytes());
        out.extend_from_slice(b"\x1b\\");
    }
    for (row, line) in screen.lines.iter().enumerate() {
        // Styled spans omit wide-character spacer cells. Let the VT parser
        // place Unicode itself; do not guess width or pad with styled spaces.
        if row == 0 || !screen.lines[row - 1].wrapped {
            out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        }
        for span in &line.spans {
            append_sgr(&mut out, span);
            out.extend_from_slice(span.text.as_bytes());
        }
    }
    out.extend_from_slice(b"\x1b[0m");
    if let Some((top, bottom)) = screen.scroll_region {
        if top > bottom || bottom >= screen.rows {
            return Err("invalid terminal scroll region".into());
        }
        out.extend_from_slice(format!("\x1b[{};{}r", top + 1, bottom + 1).as_bytes());
    }
    use dragent_terminal::TermMode;
    let modes = TermMode::from_bits_truncate(screen.modes);
    let private_modes = [
        (TermMode::APP_CURSOR, 1),
        (TermMode::INSERT, 4),
        (TermMode::LINE_WRAP, 7),
        (TermMode::LINE_FEED_NEW_LINE, 20),
        (TermMode::ORIGIN, 6),
        (TermMode::ALTERNATE_SCROLL, 1007),
        (TermMode::FOCUS_IN_OUT, 1004),
        (TermMode::MOUSE_REPORT_CLICK, 1000),
        (TermMode::MOUSE_DRAG, 1002),
        (TermMode::MOUSE_MOTION, 1003),
        (TermMode::SGR_MOUSE, 1006),
        (TermMode::URXVT_MOUSE, 1015),
        (TermMode::BRACKETED_PASTE, 2004),
        (TermMode::SYNCHRONIZED_OUTPUT, 2026),
        (TermMode::COLOR_SCHEME_UPDATES, 2031),
    ];
    // Disable before enabling: disabling another mouse mode/encoding can
    // reset the parser's shared mouse state even when its mode bit is clear.
    for enabled in [false, true] {
        for (flag, code) in private_modes {
            if modes.contains(flag) == enabled {
                out.extend_from_slice(
                    format!("\x1b[?{}{}", code, if enabled { 'h' } else { 'l' }).as_bytes(),
                );
            }
        }
    }
    out.extend_from_slice(if modes.contains(TermMode::APP_KEYPAD) {
        b"\x1b="
    } else {
        b"\x1b>"
    });
    out.extend_from_slice(
        format!(
            "\x1b[4{}",
            if modes.contains(TermMode::INSERT) {
                'h'
            } else {
                'l'
            }
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("\x1b[={}u", modes.kitty_keyboard_flags()).as_bytes());
    let shape = match screen.cursor_shape {
        ScreenCursorShape::Default => 0,
        ScreenCursorShape::BlinkingBlock => 1,
        ScreenCursorShape::SteadyBlock => 2,
        ScreenCursorShape::BlinkingUnderline => 3,
        ScreenCursorShape::SteadyUnderline => 4,
        ScreenCursorShape::BlinkingBar => 5,
        ScreenCursorShape::SteadyBar => 6,
    };
    if screen.cursor_shape != ScreenCursorShape::Default {
        out.extend_from_slice(format!("\x1b[{} q", shape).as_bytes());
    }
    // Position absolutely with origin temporarily off. DECSC stores only
    // position in the desktop emulator. Re-enable origin before the real CUP.
    out.extend_from_slice(b"\x1b[?6l");
    if let Some((row, col)) = screen.saved_cursor {
        out.extend_from_slice(format!("\x1b[{};{}H\x1b7", row + 1, col + 1).as_bytes());
    }
    let origin = modes.contains(TermMode::ORIGIN);
    if origin {
        out.extend_from_slice(b"\x1b[?6h");
    }
    let row = screen.cursor_row.min(screen.rows.saturating_sub(1));
    let top = if origin {
        screen.scroll_region.map_or(0, |r| r.0)
    } else {
        0
    };
    out.extend_from_slice(
        format!(
            "\x1b[{};{}H",
            row.saturating_sub(top) + 1,
            screen.cursor_col.min(screen.cols.saturating_sub(1)) + 1
        )
        .as_bytes(),
    );
    // Relative movement can leave the cursor above the origin margin in this
    // emulator, including DECRC from a position saved before origin was enabled.
    if row < top {
        out.extend_from_slice(format!("\x1b[{}A", top - row).as_bytes());
    }
    if let Some((col, cell)) = &screen.pending_wrap {
        // Print the edge cell again, with wrap and insert off so it lands in
        // place, to leave the cursor waiting to wrap as on the desktop.
        if *col >= screen.cols || cell.text.chars().count() != 1 {
            return Err("invalid terminal wrap state".into());
        }
        out.extend_from_slice(format!("\x1b[{}G\x1b[?7l\x1b[4l", col + 1).as_bytes());
        append_sgr(&mut out, cell);
        out.extend_from_slice(cell.text.as_bytes());
        if modes.contains(TermMode::LINE_WRAP) {
            out.extend_from_slice(b"\x1b[?7h");
        }
        if modes.contains(TermMode::INSERT) {
            out.extend_from_slice(b"\x1b[4h");
        }
    }
    if screen.cursor_visible {
        out.extend_from_slice(b"\x1b[?25h");
    }
    append_sgr(
        &mut out,
        &ScreenSpan {
            text: String::new(),
            foreground: screen.pen.foreground.clone(),
            background: screen.pen.background.clone(),
            flags: screen.pen.flags,
            underline_color: screen.pen.underline_color.clone(),
        },
    );
    out.extend_from_slice(&screen.parser_prefix);
    if out.len() > 256 * 1024 {
        return Err("terminal seed too large".into());
    }
    Ok(out)
}

fn append_sgr(out: &mut Vec<u8>, pen: &ScreenSpan) {
    use dragent_terminal::Flags;
    let flags = Flags::from_bits_truncate(pen.flags);
    let mut sgr = vec![0u8];
    if flags.contains(Flags::BOLD) {
        sgr.push(1);
    }
    if flags.contains(Flags::DIM) {
        sgr.push(2);
    }
    if flags.contains(Flags::ITALIC) {
        sgr.push(3);
    }
    if flags.contains(Flags::UNDERLINE) {
        sgr.push(4);
    }
    if flags.contains(Flags::BLINKING) {
        sgr.push(5);
    }
    if flags.contains(Flags::INVERSE) {
        sgr.push(7);
    }
    if flags.contains(Flags::HIDDEN) {
        sgr.push(8);
    }
    if flags.contains(Flags::STRIKEOUT) {
        sgr.push(9);
    }
    let color = |prefix: u8, c: &ScreenColor, dst: &mut Vec<u8>| match c {
        ScreenColor::Default => {}
        ScreenColor::Indexed(i) => {
            dst.push(prefix);
            dst.push(5);
            dst.push(*i);
        }
        ScreenColor::Rgb(r, g, b) => {
            dst.push(prefix);
            dst.push(2);
            dst.extend_from_slice(&[*r, *g, *b]);
        }
    };

    if flags.contains(Flags::OVERLINE) {
        sgr.push(53);
    }
    if let Some(c) = &pen.underline_color {
        color(58, c, &mut sgr);
    }
    color(38, &pen.foreground, &mut sgr);
    color(48, &pen.background, &mut sgr);
    if !sgr.is_empty() {
        out.extend_from_slice(b"\x1b[");
        for (i, n) in sgr.iter().enumerate() {
            if i > 0 {
                out.push(b';');
            }
            out.extend_from_slice(n.to_string().as_bytes());
        }
        out.push(b'm');
    }
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        out.extend_from_slice(b"\x1b[4:2m");
    }
    if flags.contains(Flags::UNDERCURL) {
        out.extend_from_slice(b"\x1b[4:3m");
    }
}
