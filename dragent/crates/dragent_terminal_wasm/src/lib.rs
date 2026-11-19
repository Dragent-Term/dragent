//! Browser adapter for the shared terminal emulator.
//!
//! The terminal parser and state machine remain in `dragent_terminal`; this
//! crate only exposes a small JSON snapshot API to JavaScript. Keeping the
//! renderer contract cell-based means the browser can use Canvas/WebGL while
//! retaining the desktop emulator's VT behavior.

use dragent_terminal::{Cell, Color, Emulator, Flags, Grid, Row, TermMode};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct CellSnapshot {
    text: String,
    fg: WireColor,
    bg: WireColor,
    flags: u16,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum WireColor {
    Default,
    Named(u8),
    Indexed(u8),
    Rgb([u8; 3]),
}

#[derive(Serialize)]
struct TerminalSnapshot {
    rows: usize,
    cols: usize,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    alt_screen: bool,
    // Raw-mode source is the phone's own emulator; report the same modes the
    // host reports so input/scroll decisions match without a screen RPC.
    mouse_reporting: bool,
    sgr_mouse: bool,
    bracketed_paste: bool,
    scrollback_len: usize,
    scroll_offset: usize,
    cells: Vec<Vec<CellSnapshot>>,
    surface: SurfaceState,
}

/// Everything a renderer needs to place rows, without the rows themselves.
#[derive(Serialize)]
struct ViewInfo {
    rows: usize,
    cols: usize,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    alt_screen: bool,
    mouse_reporting: bool,
    sgr_mouse: bool,
    bracketed_paste: bool,
    scrollback_len: usize,
    /// Rows dropped off the top of a full history. A line's absolute index
    /// plus this count names it for as long as it exists, so a renderer can
    /// keep a picture of the line across new output.
    evicted: usize,
    /// Changes whenever history is rewritten in place (cleared, reflowed),
    /// after which no remembered line can be trusted.
    rewrite: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SurfaceState {
    command_input: String,
    command_ghost: Option<String>,
    command_cursor: usize,
    ai_tool: Option<String>,
    ai_input: String,
    ai_ghost: Option<String>,
    ai_cursor: usize,
    ai_status: Option<String>,
    attachments: Vec<String>,
    activity: Option<String>,
    side_command: Option<String>,
    command_running: bool,
    cwd: Option<String>,
}

/// One number per colour for the compact row form: -1 is the default colour,
/// 0-255 a palette index, and 0x1000000 + 0xRRGGBB a direct colour.
fn color_code(value: Color) -> i64 {
    match value {
        Color::Default => -1,
        Color::Named(name) => i64::from(name.index()),
        Color::Indexed(index) => i64::from(index),
        Color::Rgb(r, g, b) => 0x100_0000 + (i64::from(r) << 16 | i64::from(g) << 8 | i64::from(b)),
    }
}

/// Attributes that draw something even on a blank cell.
const VISIBLE_ON_BLANK: Flags = Flags::from_bits_truncate(
    Flags::INVERSE.bits()
        | Flags::UNDERLINE.bits()
        | Flags::DOUBLE_UNDERLINE.bits()
        | Flags::UNDERCURL.bits()
        | Flags::STRIKEOUT.bits()
        | Flags::OVERLINE.bits(),
);

/// A space whose foreground and text attributes cannot show.
fn plain_blank(cell: &Cell) -> bool {
    cell.ch == ' ' && cell.grapheme().len() == 1 && !cell.flags.intersects(VISIBLE_ON_BLANK)
}

struct CompactRun {
    col: usize,
    fg: i64,
    bg: i64,
    flags: u16,
    text: String,
    blank: bool,
}

fn push_run(
    out: &mut String,
    first: &mut bool,
    col: usize,
    fg: i64,
    bg: i64,
    flags: u16,
    text: &str,
    width: Option<usize>,
) {
    if !*first {
        out.push(',');
    }
    *first = false;
    out.push_str(&format!("[{col},{fg},{bg},{flags},"));
    out.push_str(&serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()));
    if let Some(width) = width {
        out.push_str(&format!(",{width}"));
    }
    out.push(']');
}

/// Encode one row as a JSON array of style runs. A run is
/// `[col, fg, bg, flags, text]`, where every character of `text` fills one
/// cell from `col` on, or `[col, fg, bg, flags, grapheme, width]` for a wide
/// character or a combining cluster that must be placed on its own. Trailing
/// blanks are left out, and a blank whose colour cannot show joins the run
/// beside it, so a typical line is a handful of runs instead of a cell list.
fn encode_row(cells: &[Cell], out: &mut String) {
    let end = cells
        .iter()
        .rposition(|cell| !(plain_blank(cell) && cell.bg == Color::Default))
        .map_or(0, |index| index + 1);
    out.push('[');
    let mut first = true;
    let mut run: Option<CompactRun> = None;
    let mut col = 0;
    while col < end {
        let cell = &cells[col];
        let fg = color_code(cell.fg);
        let bg = color_code(cell.bg);
        let flags = (cell.flags & Flags::SGR_MASK).bits();
        let grapheme = cell.grapheme();
        let wide = cell.flags.contains(Flags::WIDE_CHAR);
        if wide || grapheme.chars().count() != 1 {
            if let Some(done) = run.take() {
                push_run(
                    out, &mut first, done.col, done.fg, done.bg, done.flags, &done.text, None,
                );
            }
            push_run(
                out,
                &mut first,
                col,
                fg,
                bg,
                flags,
                &grapheme,
                Some(if wide { 2 } else { 1 }),
            );
            col += if wide
                && cells
                    .get(col + 1)
                    .is_some_and(|next| next.flags.contains(Flags::WIDE_CHAR_SPACER))
            {
                2
            } else {
                1
            };
            continue;
        }
        let blank = plain_blank(cell);
        let inverse = cell.flags.contains(Flags::INVERSE);
        let joins = run.as_mut().is_some_and(|current| {
            let same_style = current.fg == fg && current.bg == bg && current.flags == flags;
            // Inverse paints the foreground behind the text, so a blank's
            // background only matches a run that is not inverted either.
            let same_paper =
                current.bg == bg && current.flags & Flags::INVERSE.bits() == 0 && !inverse;
            if same_style || (blank && same_paper) {
                current.blank &= blank;
                true
            } else if current.blank && same_paper {
                // Leading blanks take the style of the text that follows them.
                current.fg = fg;
                current.flags = flags;
                current.blank = false;
                true
            } else {
                false
            }
        });
        if joins {
            if let Some(current) = run.as_mut() {
                current.text.push_str(&grapheme);
            }
        } else {
            if let Some(done) = run.take() {
                push_run(
                    out, &mut first, done.col, done.fg, done.bg, done.flags, &done.text, None,
                );
            }
            run = Some(CompactRun {
                col,
                fg,
                bg,
                flags,
                text: grapheme,
                blank,
            });
        }
        col += 1;
    }
    if let Some(done) = run.take() {
        push_run(
            out, &mut first, done.col, done.fg, done.bg, done.flags, &done.text, None,
        );
    }
    out.push(']');
}

fn color(value: Color) -> WireColor {
    match value {
        Color::Default => WireColor::Default,
        Color::Named(name) => WireColor::Named(name.index()),
        Color::Indexed(index) => WireColor::Indexed(index),
        Color::Rgb(r, g, b) => WireColor::Rgb([r, g, b]),
    }
}

#[wasm_bindgen]
pub struct Terminal {
    emulator: Emulator,
    surface: SurfaceState,
    #[cfg(target_family = "wasm")]
    canvas: Option<web_sys::HtmlCanvasElement>,
}

#[wasm_bindgen]
impl Terminal {
    #[wasm_bindgen(constructor)]
    pub fn new(rows: usize, cols: usize) -> Terminal {
        Terminal {
            emulator: Emulator::new(rows.max(1), cols.max(1)),
            surface: SurfaceState::default(),
            #[cfg(target_family = "wasm")]
            canvas: None,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.emulator.feed(bytes);
        #[cfg(target_family = "wasm")]
        self.render();
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.emulator.resize(rows.max(1), cols.max(1));
        #[cfg(target_family = "wasm")]
        self.render();
    }

    /// Apply the host's non-PTY presentation state.
    pub fn set_surface_state(&mut self, json: &str) -> Result<(), JsValue> {
        self.surface =
            serde_json::from_str(json).map_err(|error| JsValue::from_str(&error.to_string()))?;
        #[cfg(target_family = "wasm")]
        self.render();
        Ok(())
    }

    /// Mount the terminal surface. React supplies only the host element;
    /// canvas creation, keyboard handling, and painting stay in WASM.
    #[cfg(target_family = "wasm")]
    pub fn mount(
        &mut self,
        container: web_sys::HtmlElement,
        can_type: bool,
    ) -> Result<(), JsValue> {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        let document = web_sys::window()
            .and_then(|window| window.document())
            .ok_or_else(|| JsValue::from_str("document unavailable"))?;
        let canvas = document
            .create_element("canvas")?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;
        canvas.set_tab_index(0);
        canvas.set_class_name("block h-full w-full outline-none");
        container.replace_children_with_node_1(&canvas);
        let target = canvas.clone();
        let listener = Closure::wrap(Box::new(move |event: web_sys::KeyboardEvent| {
            if !can_type {
                return;
            }
            let mut value = event.key();
            if (event.ctrl_key() || event.meta_key()) && value.chars().count() == 1 {
                let code = value.to_ascii_uppercase().as_bytes()[0];
                value = char::from(code.saturating_sub(b'@')).to_string();
            } else {
                value = match value.as_str() {
                    "Enter" => "\r".into(),
                    "Backspace" => "\x7f".into(),
                    "Tab" => "\t".into(),
                    "Escape" => "\x1b".into(),
                    "ArrowUp" => "\x1b[A".into(),
                    "ArrowDown" => "\x1b[B".into(),
                    "ArrowRight" => "\x1b[C".into(),
                    "ArrowLeft" => "\x1b[D".into(),
                    _ if value.chars().count() == 1 => value,
                    _ => return,
                };
            }
            event.prevent_default();
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
            let init = web_sys::CustomEventInit::new();
            init.set_detail(&JsValue::from_str(&encoded));
            if let Ok(input) =
                web_sys::CustomEvent::new_with_event_init_dict("dragent-terminal-input", &init)
            {
                let _ = target.dispatch_event(&input);
            }
        }) as Box<dyn FnMut(_)>);
        canvas.add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref())?;
        listener.forget();
        self.canvas = Some(canvas);
        self.render();
        Ok(())
    }

    #[cfg(target_family = "wasm")]
    fn render(&self) {
        use wasm_bindgen::JsCast;
        let Some(canvas) = &self.canvas else { return };
        let Ok(Some(raw)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(context) = raw.dyn_into::<web_sys::CanvasRenderingContext2d>() else {
            return;
        };
        let width = canvas.client_width().max(1) as f64;
        let height = canvas.client_height().max(1) as f64;
        let dpr = web_sys::window()
            .map(|window| window.device_pixel_ratio())
            .unwrap_or(1.0);
        canvas.set_width((width * dpr) as u32);
        canvas.set_height((height * dpr) as u32);
        let _ = context.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
        context.set_fill_style_str("#0b0d10");
        context.fill_rect(0.0, 0.0, width, height);
        context.set_font("14px ui-monospace, SFMono-Regular, Menlo, monospace");
        context.set_text_baseline("top");
        for (row_idx, row) in self.active_grid().screen_rows().iter().enumerate() {
            for (col_idx, cell) in row.cells.iter().enumerate() {
                if cell.grapheme().trim().is_empty() {
                    continue;
                }
                context.set_fill_style_str("#d7dae0");
                let _ = context.fill_text(
                    &cell.grapheme(),
                    col_idx as f64 * 8.4,
                    row_idx as f64 * 20.0 + 2.0,
                );
            }
        }
        let input = if self.surface.ai_tool.is_some() {
            &self.surface.ai_input
        } else {
            &self.surface.command_input
        };
        let ghost = if self.surface.ai_tool.is_some() {
            self.surface.ai_ghost.as_deref()
        } else {
            self.surface.command_ghost.as_deref()
        };
        let footer = height - 28.0;
        context.set_fill_style_str("rgba(20, 23, 29, .96)");
        context.fill_rect(0.0, footer, width, 28.0);
        context.set_fill_style_str("#d7dae0");
        let prefix = if self.surface.ai_tool.is_some() {
            "› "
        } else {
            "❯ "
        };
        let _ = context.fill_text(&format!("{prefix}{input}"), 12.0, footer + 5.0);
        if let Some(ghost) = ghost {
            context.set_fill_style_str("#626a78");
            let _ = context.fill_text(
                ghost,
                12.0 + (prefix.chars().count() + input.chars().count()) as f64 * 8.4,
                footer + 5.0,
            );
        }
        if let Some(activity) = &self.surface.activity {
            context.set_fill_style_str("#e5c07b");
            let _ = context.fill_text(activity, 12.0, footer - 20.0);
        }
        if let Some(status) = &self.surface.ai_status {
            context.set_fill_style_str("#61afef");
            let _ = context.fill_text(status, width - 180.0, footer - 40.0);
        }
        if let Some(side_command) = &self.surface.side_command {
            context.set_fill_style_str("#98c379");
            let _ = context.fill_text(side_command, 12.0, footer - 40.0);
        }
        if !self.surface.attachments.is_empty() {
            context.set_fill_style_str("#c678dd");
            let _ = context.fill_text(
                &format!("{} attachment(s)", self.surface.attachments.len()),
                width - 150.0,
                footer - 20.0,
            );
        }
    }

    pub fn snapshot(&self) -> Result<String, JsValue> {
        self.viewport_snapshot(0)
    }

    /// Dimensions, cursor, modes and history position without any cells.
    pub fn view_info(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.build_info())
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Rows `start..start + count` of the active screen, one JSON array per
    /// line, joined by newlines; a row past the end is `[]`. Each array holds
    /// style runs: `[col, fg, bg, flags, text]`, where every character of
    /// `text` fills one cell from `col` on, or `[col, fg, bg, flags, text,
    /// width]` for a wide character or combining cluster placed on its own.
    /// Colours are -1 (default), 0-255 (palette) or 0x1000000 + 0xRRGGBB;
    /// flags are the SGR bits of `snapshot()`. Trailing blanks are left out.
    /// On the primary screen `start` is an absolute line: 0 is the oldest
    /// history line and `scrollback_len` the top of the screen. On the
    /// alternate screen it is a screen row.
    pub fn rows_compact(&self, start: usize, count: usize) -> String {
        const MOST: usize = 1024;
        let alt = self.emulator.alt_screen_active();
        let grid = self.active_grid();
        let mut out = String::new();
        for index in start..start.saturating_add(count.min(MOST)) {
            if index > start {
                out.push('\n');
            }
            if alt {
                match grid.screen_rows().get(index) {
                    Some(row) => encode_row(&row.cells, &mut out),
                    None => out.push_str("[]"),
                }
            } else {
                match grid.line(index) {
                    Some(row) => encode_row(&row.cells, &mut out),
                    None => out.push_str("[]"),
                }
            }
        }
        out
    }

    /// Read a bounded window into primary scrollback without changing the PTY.
    /// While a program is on the alternate screen there is no history to
    /// scroll: the snapshot is always that screen as it stands.
    pub fn viewport_snapshot(&self, offset: usize) -> Result<String, JsValue> {
        serde_json::to_string(&self.build_snapshot(offset))
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl Terminal {
    /// The screen the running program is drawing on. The emulator's `grid()`
    /// always means the primary screen (with its scrollback), so the alternate
    /// screen has to be asked for separately while it is active.
    fn active_grid(&self) -> &Grid {
        if self.emulator.alt_screen_active() {
            self.emulator.alt_grid()
        } else {
            self.emulator.grid()
        }
    }

    fn build_info(&self) -> ViewInfo {
        let alt_screen = self.emulator.alt_screen_active();
        let grid = self.active_grid();
        let mode = self.emulator.mode();
        let (cursor_row, cursor_col) = self.emulator.cursor_position();
        ViewInfo {
            rows: grid.rows,
            cols: grid.cols,
            cursor_row,
            cursor_col,
            cursor_visible: self.emulator.cursor_visible(),
            alt_screen,
            mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            scrollback_len: if alt_screen { 0 } else { grid.scrollback_len() },
            evicted: if alt_screen { 0 } else { grid.evicted() },
            rewrite: if alt_screen {
                0
            } else {
                grid.scrollback_rewrite_version()
            },
        }
    }

    fn build_snapshot(&self, offset: usize) -> TerminalSnapshot {
        let alt_screen = self.emulator.alt_screen_active();
        let grid = self.active_grid();
        let history = if alt_screen { 0 } else { grid.scrollback_len() };
        let offset = offset.min(history);
        let row_cells = |row: &Row| -> Vec<CellSnapshot> {
            row.cells
                .iter()
                .map(|cell| CellSnapshot {
                    text: cell.grapheme(),
                    fg: color(cell.fg),
                    bg: color(cell.bg),
                    flags: (cell.flags & Flags::SGR_MASK).bits(),
                })
                .collect()
        };
        let cells = if alt_screen {
            grid.screen_rows().iter().map(row_cells).collect()
        } else {
            let start = history - offset;
            (start..start + grid.rows)
                .filter_map(|index| grid.line(index))
                .map(|row| row_cells(&row))
                .collect()
        };
        let mode = self.emulator.mode();
        TerminalSnapshot {
            rows: grid.rows,
            cols: grid.cols,
            cursor_row: self.emulator.cursor_position().0,
            cursor_col: self.emulator.cursor_position().1,
            cursor_visible: offset == 0 && self.emulator.cursor_visible(),
            alt_screen,
            mouse_reporting: mode.intersects(TermMode::MOUSE_MODE),
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            scrollback_len: history,
            scroll_offset: offset,
            cells,
            surface: self.surface.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Terminal;
    use serde_json::Value;

    fn snapshot(terminal: &Terminal, offset: usize) -> Value {
        serde_json::to_value(terminal.build_snapshot(offset)).expect("snapshot serializes")
    }

    fn row_text(snapshot: &Value, row: usize) -> String {
        snapshot["cells"][row]
            .as_array()
            .expect("row cells")
            .iter()
            .map(|cell| cell["text"].as_str().unwrap_or_default())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn all_text(snapshot: &Value) -> String {
        (0..snapshot["cells"].as_array().unwrap().len())
            .map(|row| row_text(snapshot, row))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn alternate_screen_cells_replace_the_primary_screen() {
        let mut terminal = Terminal::new(4, 20);
        terminal.write(b"HELLO");
        terminal.write(b"\x1b[?1049hALT");

        let snap = snapshot(&terminal, 0);
        assert_eq!(snap["alt_screen"], true);
        assert_eq!(row_text(&snap, 0), "ALT");
        assert!(!all_text(&snap).contains("HELLO"));
        assert_eq!(snap["scrollback_len"], 0);
        assert_eq!(snap["cells"].as_array().unwrap().len(), 4);
        assert_eq!(snap["cursor_row"], 0);
        assert_eq!(snap["cursor_col"], 3);
    }

    #[test]
    fn alternate_screen_keeps_colour_and_ignores_scroll_offset() {
        let mut terminal = Terminal::new(4, 20);
        for line in 0..10 {
            terminal.write(format!("line {line}\r\n").as_bytes());
        }
        terminal.write(b"\x1b[?1049hALT\x1b[H\x1b[31mRED");

        let snap = snapshot(&terminal, 5);
        assert_eq!(row_text(&snap, 0), "RED");
        let first = &snap["cells"][0][0];
        assert_eq!(first["text"], "R");
        // SGR 31 is palette entry 1 (red); the emulator stores it as indexed.
        assert_eq!(first["fg"]["kind"], "indexed");
        assert_eq!(first["fg"]["value"], 1);
        assert_eq!(snap["scroll_offset"], 0);
        assert_eq!(snap["scrollback_len"], 0);
    }

    #[test]
    fn leaving_the_alternate_screen_restores_primary_with_scrollback() {
        let mut terminal = Terminal::new(3, 20);
        for line in 0..6 {
            terminal.write(format!("row {line}\r\n").as_bytes());
        }
        terminal.write(b"HELLO");
        let before = snapshot(&terminal, 0);

        terminal.write(b"\x1b[?1049hALT\x1b[H\x1b[31mRED\x1b[?1049l");

        let after = snapshot(&terminal, 0);
        assert_eq!(after["alt_screen"], false);
        assert_eq!(after["cells"], before["cells"]);
        assert_eq!(after["scrollback_len"], before["scrollback_len"]);
        assert!(after["scrollback_len"].as_u64().unwrap() > 0);
        assert!(all_text(&after).contains("HELLO"));
        assert!(!all_text(&after).contains("RED"));
        assert!(!all_text(&after).contains("ALT"));
    }

    fn compact(terminal: &Terminal, start: usize, count: usize) -> Vec<Value> {
        terminal
            .rows_compact(start, count)
            .split('\n')
            .map(|row| serde_json::from_str(row).expect("compact row is JSON"))
            .collect()
    }

    #[test]
    fn compact_rows_group_styles_and_place_wide_and_combined_text() {
        let mut terminal = Terminal::new(3, 20);
        terminal.write("ab \x1b[31mred\x1b[0m 界e\u{301}Z\x1b[7m  \x1b[0m   ".as_bytes());
        let rows = compact(&terminal, 0, 3);
        assert_eq!(rows.len(), 3);
        let runs = rows[0].as_array().unwrap();
        // Plain text and the blank before the red word share one run; the
        // blank after it joins the red run, since its colour cannot show.
        assert_eq!(runs[0], serde_json::json!([0, -1, -1, 0, "ab "]));
        assert_eq!(runs[1], serde_json::json!([3, 1, -1, 0, "red "]));
        assert_eq!(runs[2], serde_json::json!([7, -1, -1, 0, "界", 2]));
        assert_eq!(runs[3], serde_json::json!([9, -1, -1, 0, "e\u{301}", 1]));
        assert_eq!(runs[4], serde_json::json!([10, -1, -1, 0, "Z"]));
        // Inverse blanks paint, so they keep their own run; trailing blanks go.
        assert_eq!(runs[5], serde_json::json!([11, -1, -1, 64, "  "]));
        assert_eq!(runs.len(), 6);
        assert_eq!(rows[1], serde_json::json!([]));
    }

    #[test]
    fn compact_rows_encode_direct_colours_and_attributes() {
        let mut terminal = Terminal::new(2, 20);
        terminal.write(b"\x1b[1;4;38;2;1;2;3;48;5;200mX\x1b[0m\x1b[43m \x1b[0m");
        let rows = compact(&terminal, 0, 1);
        let runs = rows[0].as_array().unwrap();
        assert_eq!(
            runs[0],
            serde_json::json!([0, 0x100_0000 + 0x010203, 200, 9, "X"])
        );
        // A coloured background is not a trailing blank.
        assert_eq!(runs[1], serde_json::json!([1, -1, 3, 0, " "]));
    }

    #[test]
    fn compact_rows_read_history_by_absolute_line_and_info_names_lines() {
        let mut terminal = Terminal::new(3, 20);
        for line in 0..8 {
            terminal.write(format!("row {line}\r\n").as_bytes());
        }
        let info: Value = serde_json::from_str(&terminal.view_info().unwrap()).unwrap();
        assert_eq!(info["scrollback_len"], 6);
        assert_eq!(info["rows"], 3);
        assert_eq!(info["alt_screen"], false);
        assert_eq!(info["evicted"], 0);
        let rows = compact(&terminal, 4, 4);
        assert_eq!(rows[0], serde_json::json!([[0, -1, -1, 0, "row 4"]]));
        assert_eq!(rows[3], serde_json::json!([[0, -1, -1, 0, "row 7"]]));
        // Past the end is empty, not missing.
        assert_eq!(
            compact(&terminal, 100, 2),
            vec![serde_json::json!([]), serde_json::json!([])]
        );
        terminal.write(b"\x1b[?1049hALT");
        let info: Value = serde_json::from_str(&terminal.view_info().unwrap()).unwrap();
        assert_eq!(info["alt_screen"], true);
        assert_eq!(info["scrollback_len"], 0);
        assert_eq!(
            compact(&terminal, 0, 1)[0],
            serde_json::json!([[0, -1, -1, 0, "ALT"]])
        );
    }

    #[test]
    fn primary_viewport_offset_reads_scrollback() {
        let mut terminal = Terminal::new(3, 20);
        for line in 0..8 {
            terminal.write(format!("row {line}\r\n").as_bytes());
        }
        terminal.write(b"HELLO");

        let live = snapshot(&terminal, 0);
        assert_eq!(row_text(&live, 2), "HELLO");
        let history = live["scrollback_len"].as_u64().unwrap() as usize;
        assert_eq!(history, 6);

        let scrolled = snapshot(&terminal, 2);
        assert_eq!(scrolled["scroll_offset"], 2);
        assert_eq!(scrolled["cursor_visible"], false);
        assert_eq!(row_text(&scrolled, 0), "row 4");
        assert_eq!(row_text(&scrolled, 2), "row 6");

        let clamped = snapshot(&terminal, 1000);
        assert_eq!(clamped["scroll_offset"], history);
        assert_eq!(row_text(&clamped, 0), "row 0");

        // The public JSON entry point returns the same document.
        let json: Value = serde_json::from_str(&terminal.viewport_snapshot(2).unwrap()).unwrap();
        assert_eq!(json, scrolled);
    }
}
