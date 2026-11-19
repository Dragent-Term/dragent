//! Terminal cell grid: visible screen, cursor, scroll region, and scrollback.
//!
//! The visible screen is a plain `Vec<Row>` (random access needed).
//! The scrollback is backed by a `Scrollback` (B+ tree) giving O(log n)
//! seeks and O(log n) eviction without copying data.

use unicode_width::UnicodeWidthChar;

use crate::cell::{Cell, Style, StyledLine, StyledSpan};
use crate::color::Color;
use crate::flags::Flags;
use crate::scrollback::Scrollback;
use crate::tab_stops::TabStops;

// ─────────────────────────────────────────────────────────────────────────────
// Row
// ─────────────────────────────────────────────────────────────────────────────

/// One row of cells in the grid.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// True when this row ended by auto-wrap (soft newline). Used for reflow
    /// on resize and for logical-line selection.
    pub wrapped: bool,
    /// True when this row is part of a collapsed block and should be hidden
    /// from the rendered view.  The `RowSummary::visible_row_count` field is
    /// set to 0 for collapsed rows, letting `FilterPath` skip entire
    /// collapsed subtrees in O(log n).
    pub collapsed: bool,
}

impl Row {
    pub fn blank(cols: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols],
            wrapped: false,
            collapsed: false,
        }
    }

    pub fn blank_styled(cols: usize, style: Style) -> Self {
        Self {
            cells: (0..cols).map(|_| Cell::blank(style.fg, style.bg)).collect(),
            wrapped: false,
            collapsed: false,
        }
    }

    pub fn resize(&mut self, new_cols: usize, fill_style: Style) {
        if new_cols > self.cells.len() {
            let extra = new_cols - self.cells.len();
            self.cells
                .extend((0..extra).map(|_| Cell::blank(fill_style.fg, fill_style.bg)));
        } else {
            self.cells.truncate(new_cols);
        }
    }

    /// Build a `StyledLine` from this row's cells (trailing blanks trimmed).
    pub fn styled_line(&self) -> StyledLine {
        let mut end = self.cells.len();
        while end > 0 {
            let c = &self.cells[end - 1];
            if c.ch == ' ' && c.fg == Color::Default && c.bg == Color::Default && c.flags.is_empty()
            {
                end -= 1;
            } else {
                break;
            }
        }
        let mut spans: Vec<StyledSpan> = Vec::new();
        for cell in &self.cells[..end] {
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let style = Style {
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags & Flags::SGR_MASK,
                underline_color: cell.extra().and_then(|e| e.underline_color),
            };
            let text = cell.grapheme();
            match spans.last_mut() {
                Some(span) if span.style == style => span.text.push_str(&text),
                _ => spans.push(StyledSpan { text, style }),
            }
        }
        StyledLine {
            spans,
            wrapped: self.wrapped,
        }
    }

    pub fn plain_text(&self) -> String {
        self.styled_line().plain_text()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Grid
// ─────────────────────────────────────────────────────────────────────────────

/// A 2-D terminal screen with cursor, scroll region, and B+ tree scrollback.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// Visible rows (always exactly `rows` long, random-access).
    pub(crate) screen: Vec<Row>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    saved_cursor: Option<(usize, usize)>,
    scroll_top: usize,
    scroll_bottom: usize,
    /// Pending-wrap: cursor is logically past the last column; wraps on next print.
    pub(crate) pending_wrap: bool,
    /// B+ tree scrollback (oldest-first).
    scrollback: Scrollback,
    /// Tab stop configuration.
    tab_stops: TabStops,
    /// Fill style for newly created (blank / erased) cells.
    pub fill_style: Style,
    /// DECAWM — auto-wrap at right margin (default on).
    pub auto_wrap: bool,
}

impl Grid {
    pub(crate) fn valid_checkpoint(&self) -> bool {
        self.rows > 0
            && self.cols > 0
            && self.rows.saturating_mul(self.cols) <= 65_536
            && self.screen.len() == self.rows
            && self.screen.iter().all(|row| row.cells.len() == self.cols)
            && self.cursor_row < self.rows
            && self.cursor_col < self.cols
            && self.scroll_top <= self.scroll_bottom
            && self.scroll_bottom < self.rows
    }

    /// The last printed character filled the right edge; the next printable
    /// one wraps first (or overwrites it when autowrap is off).
    pub fn pending_wrap(&self) -> bool {
        self.pending_wrap
    }

    pub fn scroll_region_bottom(&self) -> usize {
        self.scroll_bottom
    }

    /// The emulator saves position only; attributes and charsets are not stored.
    pub fn saved_cursor(&self) -> Option<(usize, usize)> {
        self.saved_cursor
    }

    pub fn scroll_region_top(&self) -> usize {
        self.scroll_top
    }

    pub fn new(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Self {
            cols,
            rows,
            screen: (0..rows).map(|_| Row::blank(cols)).collect(),
            cursor_row: 0,
            cursor_col: 0,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            pending_wrap: false,
            scrollback: Scrollback::new(max_scrollback),
            tab_stops: TabStops::new(cols),
            fill_style: Style::default(),
            auto_wrap: true,
        }
    }

    /// Clear and reset the grid contents, cursor, scroll regions, pending wrap, and tab stops.
    pub fn clear(&mut self) {
        let style = Style::default();
        for row in self.screen.iter_mut() {
            for cell in row.cells.iter_mut() {
                cell.reset(style);
            }
        }
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.saved_cursor = None;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.pending_wrap = false;
        self.tab_stops = TabStops::new(self.cols);
        self.fill_style = style;
        self.auto_wrap = true;
        self.scrollback.clear();
    }

    // ── Public metrics ───────────────────────────────────────────────────────

    /// Version counter of the scrollback tree — increments on every mutation.
    /// Use this to detect changes without diffing the content.
    pub fn scrollback_rewrite_version(&self) -> u64 {
        self.scrollback.rewrite_version()
    }

    pub fn scrollback_version(&self) -> u64 {
        self.scrollback.version()
    }

    /// Number of rows evicted from scrollback when it was full.
    pub fn evicted(&self) -> usize {
        self.scrollback.evicted
    }

    /// Number of rows currently stored in the scrollback buffer.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    // ── Cursor movement ──────────────────────────────────────────────────────

    pub fn move_to(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.rows - 1);
        self.cursor_col = col.min(self.cols - 1);
        self.pending_wrap = false;
    }

    pub fn move_rows(&mut self, delta: isize) {
        let new_row = (self.cursor_row as isize + delta)
            .max(0)
            .min(self.rows as isize - 1) as usize;
        self.cursor_row = new_row;
        self.pending_wrap = false;
    }

    pub fn move_cols(&mut self, delta: isize) {
        let new_col = (self.cursor_col as isize + delta)
            .max(0)
            .min(self.cols as isize - 1) as usize;
        self.cursor_col = new_col;
        self.pending_wrap = false;
    }

    pub fn carriage_return(&mut self) {
        self.cursor_col = 0;
        self.pending_wrap = false;
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor_row, self.cursor_col));
    }

    pub fn restore_cursor(&mut self) {
        if let Some((r, c)) = self.saved_cursor {
            self.cursor_row = r.min(self.rows - 1);
            self.cursor_col = c.min(self.cols - 1);
            self.pending_wrap = false;
        }
    }

    // ── Linefeed / scroll ────────────────────────────────────────────────────

    pub fn linefeed(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor_row < self.rows - 1 {
            self.cursor_row += 1;
        }
        self.pending_wrap = false;
    }

    pub fn reverse_index(&mut self) {
        if self.cursor_row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
        }
        self.pending_wrap = false;
    }

    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_top = top.min(self.rows - 1);
        self.scroll_bottom = bottom.min(self.rows - 1);
        if self.scroll_top > self.scroll_bottom {
            self.scroll_bottom = self.scroll_top;
        }
    }

    pub fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let evicted = self.screen.remove(self.scroll_top);
            if self.scroll_top == 0 {
                self.scrollback.push(evicted);
            }
            let blank = Row::blank_styled(self.cols, self.fill_style);
            self.screen.insert(self.scroll_bottom, blank);
        }
    }

    pub fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            if self.scroll_bottom + 1 <= self.screen.len() {
                self.screen.remove(self.scroll_bottom);
            }
            let blank = Row::blank_styled(self.cols, self.fill_style);
            self.screen.insert(self.scroll_top, blank);
        }
    }

    // ── Tab handling ─────────────────────────────────────────────────────────

    pub fn tab(&mut self) {
        self.cursor_col = self.tab_stops.next_tab_stop(self.cursor_col);
        self.pending_wrap = false;
    }

    pub fn tab_forward(&mut self, n: usize) {
        self.cursor_col = self.tab_stops.advance_n(self.cursor_col, n);
        self.pending_wrap = false;
    }

    pub fn tab_backward(&mut self, n: usize) {
        self.cursor_col = self.tab_stops.retreat_n(self.cursor_col, n);
        self.pending_wrap = false;
    }

    pub fn set_tab_stop(&mut self) {
        self.tab_stops.set(self.cursor_col);
    }

    pub fn clear_tab_stop(&mut self) {
        self.tab_stops.clear(self.cursor_col);
    }

    pub fn clear_all_tab_stops(&mut self) {
        self.tab_stops.clear_all();
    }

    // ── Character output ─────────────────────────────────────────────────────

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
        self.pending_wrap = false;
    }

    /// Write a character, handling wide Unicode, combining chars, and wrap.
    pub fn put_char(&mut self, ch: char, style: Style) {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);

        // Zero-width combining character: attach to previous cell.
        if char_width == 0 {
            self.put_combining(ch);
            return;
        }

        // Pending soft-wrap.
        if self.pending_wrap {
            if self.auto_wrap {
                self.screen[self.cursor_row].wrapped = true;
                self.linefeed();
                self.cursor_col = 0;
            }
            // With auto_wrap off: cursor stays at last column; overwrite it.
        }

        if char_width == 2 {
            // If only 1 col remains and auto_wrap is on, fill and wrap first.
            if self.auto_wrap && self.cursor_col + 1 >= self.cols {
                let blank = Cell::blank(style.fg, style.bg);
                self.screen[self.cursor_row].cells[self.cursor_col] = blank;
                self.screen[self.cursor_row].wrapped = true;
                self.linefeed();
                self.cursor_col = 0;
            }
            self.clear_wide_spacer(self.cursor_col);
            let mut wide_cell = Cell::from_char_style(ch, style);
            wide_cell.flags.insert(Flags::WIDE_CHAR);
            self.screen[self.cursor_row].cells[self.cursor_col] = wide_cell;

            if self.cursor_col + 1 < self.cols {
                let mut spacer = Cell::blank(style.fg, style.bg);
                spacer.flags.insert(Flags::WIDE_CHAR_SPACER);
                self.screen[self.cursor_row].cells[self.cursor_col + 1] = spacer;
            }

            if self.cursor_col + 2 >= self.cols {
                self.pending_wrap = true;
                self.cursor_col = self.cols.saturating_sub(1);
            } else {
                self.cursor_col += 2;
            }
        } else {
            self.clear_wide_spacer(self.cursor_col);
            self.screen[self.cursor_row].cells[self.cursor_col] = Cell::from_char_style(ch, style);
            if self.cursor_col + 1 >= self.cols {
                self.pending_wrap = true;
            } else {
                self.cursor_col += 1;
            }
        }
    }

    fn clear_wide_spacer(&mut self, col: usize) {
        if col < self.cols
            && self.screen[self.cursor_row].cells[col]
                .flags
                .contains(Flags::WIDE_CHAR_SPACER)
        {
            if col > 0 {
                self.screen[self.cursor_row].cells[col - 1]
                    .flags
                    .remove(Flags::WIDE_CHAR);
            }
        }
    }

    fn put_combining(&mut self, ch: char) {
        let col = if self.cursor_col > 0 {
            self.cursor_col - 1
        } else {
            0
        };
        self.screen[self.cursor_row].cells[col].push_combining(ch);
    }

    // ── Erase ────────────────────────────────────────────────────────────────

    pub fn erase_display(&mut self, mode: u16, style: Style) {
        match mode {
            0 => {
                for col in self.cursor_col..self.cols {
                    self.screen[self.cursor_row].cells[col].reset(style);
                }
                for row in (self.cursor_row + 1)..self.rows {
                    for col in 0..self.cols {
                        self.screen[row].cells[col].reset(style);
                    }
                }
            }
            1 => {
                for row in 0..self.cursor_row {
                    for col in 0..self.cols {
                        self.screen[row].cells[col].reset(style);
                    }
                }
                for col in 0..=self.cursor_col {
                    self.screen[self.cursor_row].cells[col].reset(style);
                }
            }
            2 | 3 => {
                for row in self.screen.iter_mut() {
                    for cell in row.cells.iter_mut() {
                        cell.reset(style);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn erase_line(&mut self, mode: u16, style: Style) {
        let row = self.cursor_row;
        match mode {
            0 => {
                for col in self.cursor_col..self.cols {
                    self.screen[row].cells[col].reset(style);
                }
            }
            1 => {
                for col in 0..=self.cursor_col {
                    self.screen[row].cells[col].reset(style);
                }
            }
            2 => {
                for col in 0..self.cols {
                    self.screen[row].cells[col].reset(style);
                }
            }
            _ => {}
        }
    }

    pub fn erase_chars(&mut self, n: usize, style: Style) {
        let end = (self.cursor_col + n).min(self.cols);
        for col in self.cursor_col..end {
            self.screen[self.cursor_row].cells[col].reset(style);
        }
    }

    // ── Insert / delete ──────────────────────────────────────────────────────

    pub fn insert_lines(&mut self, n: usize) {
        for _ in 0..n {
            if self.scroll_bottom + 1 <= self.screen.len() {
                self.screen.remove(self.scroll_bottom);
            }
            let blank = Row::blank_styled(self.cols, self.fill_style);
            self.screen.insert(self.cursor_row, blank);
        }
    }

    pub fn delete_lines(&mut self, n: usize) {
        for _ in 0..n {
            self.screen.remove(self.cursor_row);
            let blank = Row::blank_styled(self.cols, self.fill_style);
            self.screen.insert(self.scroll_bottom, blank);
        }
    }

    pub fn insert_chars(&mut self, n: usize, style: Style) {
        let row = &mut self.screen[self.cursor_row];
        let col = self.cursor_col;
        let end = row.cells.len();
        for i in (col..end).rev() {
            if i + n < end {
                row.cells[i + n] = row.cells[i].clone();
            }
        }
        for i in col..(col + n).min(end) {
            row.cells[i] = Cell::blank(style.fg, style.bg);
        }
    }

    pub fn delete_chars(&mut self, n: usize, style: Style) {
        let row = &mut self.screen[self.cursor_row];
        let col = self.cursor_col;
        let end = row.cells.len();
        row.cells.drain(col..(col + n).min(end));
        while row.cells.len() < end {
            row.cells.push(Cell::blank(style.fg, style.bg));
        }
    }

    // ── Hyperlink cell access ────────────────────────────────────────────────

    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        self.screen.get_mut(row)?.cells.get_mut(col)
    }

    pub fn set_cell_hyperlink(&mut self, row: usize, col: usize, id: u32) {
        if let Some(cell) = self.cell_mut(row, col) {
            cell.set_hyperlink_id(id);
        }
    }

    // ── Collapse / expand blocks ─────────────────────────────────────────────

    /// Collapse scrollback rows `[abs_start, abs_end)` so they are hidden
    /// from visible iteration.  Absolute coordinates — scrollback only.
    pub fn collapse_block(&mut self, abs_start: usize, abs_end: usize) {
        let sb_len = self.scrollback.len();
        let start = abs_start.min(sb_len);
        let end = abs_end.min(sb_len);
        self.scrollback.collapse_rows(start, end);
    }

    /// Expand (un-collapse) scrollback rows `[abs_start, abs_end)`.
    pub fn expand_block(&mut self, abs_start: usize, abs_end: usize) {
        let sb_len = self.scrollback.len();
        let start = abs_start.min(sb_len);
        let end = abs_end.min(sb_len);
        self.scrollback.expand_rows(start, end);
    }

    /// Number of currently visible scrollback rows (excludes collapsed).
    pub fn visible_scrollback_len(&self) -> usize {
        self.scrollback.visible_len()
    }

    // ── Live cell edits via update_at ────────────────────────────────────────

    /// Mutate a single cell in **scrollback** in-place via `f`. O(log n).
    ///
    /// This is the `update_at`-based live edit API: use it to apply
    /// asynchronous decorations (link underlines, LSP diagnostics, semantic
    /// highlights) to rows that have already scrolled into history.
    ///
    /// `abs_row` is the absolute row index (scrollback-relative, 0 = oldest).
    /// Returns `true` if the row and column were found.
    pub fn update_scrollback_cell<F>(&mut self, abs_row: usize, col: usize, f: F) -> bool
    where
        F: FnOnce(&mut Cell),
    {
        let sb_len = self.scrollback.len();
        if abs_row >= sb_len {
            return false;
        }
        self.scrollback.update_row(abs_row, |row| {
            if let Some(cell) = row.cells.get_mut(col) {
                f(cell);
            }
        })
    }

    /// Mutate every cell in a column range of a **scrollback** row. O(log n).
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
        let sb_len = self.scrollback.len();
        if abs_row >= sb_len {
            return false;
        }
        self.scrollback.update_row(abs_row, |row| {
            let end = col_end.min(row.cells.len());
            for cell in &mut row.cells[col_start.min(end)..end] {
                f(cell);
            }
        })
    }

    // ── Resize with reflow ───────────────────────────────────────────────────

    pub fn resize(&mut self, new_rows: usize, new_cols: usize) {
        if new_cols != self.cols {
            self.resize_cols(new_cols);
        }
        if new_rows != self.rows {
            self.resize_rows(new_rows);
        }
    }

    fn resize_cols(&mut self, new_cols: usize) {
        // ── 1. Gather all logical lines from scrollback + screen ──────────────
        let mut sb_rows = self.scrollback.all_rows();
        let mut all_rows: Vec<Row> = sb_rows.drain(..).chain(self.screen.drain(..)).collect();

        // ── 2. Concatenate soft-wrapped rows into logical lines ───────────────
        let mut logical: Vec<Vec<Cell>> = Vec::new();
        let mut current: Vec<Cell> = Vec::new();
        for row in all_rows.drain(..) {
            current.extend(row.cells.into_iter());
            if !row.wrapped {
                // Trim trailing blanks.
                while current.last().is_some_and(|c: &Cell| {
                    c.ch == ' '
                        && c.fg == Color::Default
                        && c.bg == Color::Default
                        && c.flags.is_empty()
                }) {
                    current.pop();
                }
                logical.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            logical.push(current);
        }

        // ── 3. Re-wrap at new_cols ────────────────────────────────────────────
        let mut new_rows_vec: Vec<Row> = Vec::new();
        for cells in &logical {
            if cells.is_empty() {
                new_rows_vec.push(Row::blank(new_cols));
                continue;
            }
            let mut offset = 0;
            while offset < cells.len() {
                let end = (offset + new_cols).min(cells.len());
                let mut chunk: Vec<Cell> = cells[offset..end].to_vec();
                while chunk.len() < new_cols {
                    chunk.push(Cell::default());
                }
                let wrapped = end < cells.len();
                new_rows_vec.push(Row {
                    cells: chunk,
                    wrapped,
                    collapsed: false,
                });
                offset = end;
            }
        }

        // ── 4. Split into scrollback + screen ─────────────────────────────────
        let screen_count = new_rows_vec.len().min(self.rows);
        let sb_count = new_rows_vec.len().saturating_sub(screen_count);

        let new_sb = new_rows_vec.drain(..sb_count);
        let max_sb = self.scrollback.max_rows;
        // Rebuild scrollback.
        let mut new_scrollback = Scrollback::new(max_sb);
        new_scrollback.replace_all(new_sb);
        // Carry over the eviction count.
        new_scrollback.evicted += self.scrollback.evicted;
        self.scrollback = new_scrollback;

        let mut new_screen: Vec<Row> = new_rows_vec;
        while new_screen.len() < self.rows {
            new_screen.push(Row::blank(new_cols));
        }
        new_screen.truncate(self.rows);

        self.screen = new_screen;
        self.cols = new_cols;
        self.cursor_col = self.cursor_col.min(new_cols - 1);
        self.tab_stops.resize(new_cols);
    }

    fn resize_rows(&mut self, new_rows: usize) {
        if new_rows > self.rows {
            let extra = new_rows - self.rows;
            // Pull from scrollback first.
            let from_sb = self.scrollback.pop_last_n(extra);
            for row in from_sb {
                self.screen.insert(0, row);
            }
            // Fill remaining with blanks.
            while self.screen.len() < new_rows {
                self.screen.push(Row::blank(self.cols));
            }
        } else {
            let remove = self.rows - new_rows;
            for _ in 0..remove {
                if self.screen.is_empty() {
                    break;
                }
                let row = self.screen.remove(0);
                self.scrollback.push(row);
            }
        }
        self.rows = new_rows;
        self.cursor_row = self.cursor_row.min(new_rows - 1);
        self.scroll_top = 0;
        self.scroll_bottom = new_rows - 1;
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    pub fn cursor_abs_line(&self) -> usize {
        self.scrollback.len() + self.cursor_row
    }

    /// Return a row by absolute index (cloned from scrollback, referenced from screen).
    ///
    /// Returns `None` for out-of-range indices.
    pub fn line(&self, abs: usize) -> Option<Row> {
        let sb_len = self.scrollback.len();
        if abs < sb_len {
            self.scrollback.get_cloned(abs)
        } else {
            self.screen.get(abs - sb_len).cloned()
        }
    }

    /// All visible rows as `StyledLine`s.
    pub fn screen_styled_lines(&self) -> Vec<StyledLine> {
        self.screen.iter().map(Row::styled_line).collect()
    }

    /// Exclusive end row of visible screen content.
    ///
    /// Terminal UIs may render content below the cursor (for example Codex puts
    /// its footer and slash-command popup there). Callers that expose the live
    /// primary buffer must therefore not treat the cursor row as the screen's
    /// content boundary.
    pub fn screen_content_end(&self) -> usize {
        self.screen
            .iter()
            .rposition(|row| {
                row.cells.iter().any(|cell| {
                    cell.ch != ' '
                        || cell.fg != Color::Default
                        || cell.bg != Color::Default
                        || !cell.flags.is_empty()
                })
            })
            .map_or(0, |row| row + 1)
    }

    /// Visible rows for renderer adapters such as the browser/WASM bridge.
    /// The native renderer can continue using styled lines; web renderers need
    /// the individual cells so they can preserve colors and attributes.
    pub fn screen_rows(&self) -> &[Row] {
        &self.screen
    }

    /// A range of absolute lines as `StyledLine`s.
    pub fn styled_lines_in(&self, start: usize, end: usize) -> Vec<StyledLine> {
        (start..end)
            .filter_map(|i| self.line(i))
            .map(|r| r.styled_line())
            .collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_char_basic() {
        let mut g = Grid::new(3, 10, 0);
        g.put_char('a', Style::default());
        g.put_char('b', Style::default());
        assert_eq!(g.screen[0].cells[0].ch, 'a');
        assert_eq!(g.screen[0].cells[1].ch, 'b');
        assert_eq!(g.cursor_col, 2);
    }

    #[test]
    fn linefeed_pushes_to_scrollback() {
        let mut g = Grid::new(2, 5, 100);
        g.put_char('x', Style::default());
        g.linefeed();
        g.put_char('y', Style::default());
        g.linefeed(); // scrollback receives the evicted row
        assert_eq!(g.scrollback.len(), 1);
    }

    #[test]
    fn tab_stops_advance() {
        let mut g = Grid::new(2, 40, 0);
        g.cursor_col = 0;
        g.tab();
        assert_eq!(g.cursor_col, 8);
        g.tab();
        assert_eq!(g.cursor_col, 16);
    }

    #[test]
    fn wide_char_occupies_two_cols() {
        let mut g = Grid::new(3, 10, 0);
        g.put_char('中', Style::default());
        assert!(g.screen[0].cells[0].flags.contains(Flags::WIDE_CHAR));
        assert!(g.screen[0].cells[1].flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(g.cursor_col, 2);
    }

    #[test]
    fn scrollback_seek_o_log_n() {
        let mut g = Grid::new(2, 5, 1000);
        for i in 0..100u8 {
            g.put_char(i as char, Style::default());
            g.linefeed();
            g.linefeed();
        }
        // Scroll enough to have meaningful history.
        let sb_len = g.scrollback.len();
        assert!(sb_len > 0, "expected non-empty scrollback");
        // Seek to middle row — should succeed.
        let mid = sb_len / 2;
        let row = g.scrollback.get_cloned(mid);
        assert!(row.is_some(), "expected row at index {mid}");
    }

    #[test]
    fn resize_wider_reflows() {
        let mut g = Grid::new(2, 5, 100);
        for ch in "hello world".chars() {
            g.put_char(ch, Style::default());
        }
        g.resize(2, 20);
        let combined: String = g
            .screen_styled_lines()
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("")
            .trim_end()
            .to_string();
        assert!(
            combined.contains("hello"),
            "reflow failed, got: {combined:?}"
        );
    }
}
