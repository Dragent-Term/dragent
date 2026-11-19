//! Monospace cell geometry derived from real font metrics.
//!
//! Cell width is the advance of the `m` glyph; cell height is the rendered
//! line height. These numbers must match what the renderer paints so the PTY
//! grid, resize, wrapping, mouse hit-testing, and cursor stay aligned.
//!
//! Block/alt grids render each row with the `Text` element, whose line height
//! is `font_size * line_height_ratio` — we use that formula for cell height.

use unicode_width::UnicodeWidthChar;
use warpui::fonts::{Cache as FontCache, FamilyId, Properties};

use crate::theme::{mono_font_size, terminal_line_height_ratio};

/// Pixel size of a single monospace grid cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

impl CellMetrics {
    /// Fallback used before a font is available (tests, headless). Approximate
    /// Menlo @ 13px; replaced by `from_font` as soon as the cache is ready.
    pub const FALLBACK: CellMetrics = CellMetrics {
        width: 7.8,
        height: crate::theme::MONO_FONT_SIZE * crate::theme::TERMINAL_LINE_HEIGHT_RATIO,
    };

    /// Compute cell geometry from the loaded monospace font.
    ///
    /// `width` is the horizontal advance of `m` at the render size (the renderer
    /// positions glyphs by their real advances, so we do NOT round — rounding
    /// would accumulate a multi-column drift across an 80+ column row).
    ///
    /// `height` is `font_size * line_height_ratio`, exactly what the `Text`
    /// element uses to stack rows.
    pub fn from_font(
        font_cache: &FontCache,
        family: FamilyId,
        font_size: f32,
        line_height_ratio: f32,
    ) -> Self {
        let height = (font_size * line_height_ratio).max(1.0);

        let font_id = font_cache.select_font(family, Properties::default());
        let width = font_cache
            .glyph_for_char(font_id, 'm', false)
            .and_then(|(glyph, _)| font_cache.glyph_advance(font_id, font_size, glyph).ok())
            .map(|advance| advance.x())
            .filter(|w| *w > 0.0)
            .unwrap_or(CellMetrics::FALLBACK.width);

        CellMetrics { width, height }
    }

    /// Compute using the project's standard monospace size and line-height ratio.
    pub fn for_terminal(font_cache: &FontCache, family: FamilyId) -> Self {
        Self::from_font(
            font_cache,
            family,
            mono_font_size(),
            terminal_line_height_ratio(),
        )
    }
}

impl Default for CellMetrics {
    fn default() -> Self {
        Self::FALLBACK
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Terminal column width (wcwidth) — shared by grid paint and hit-testing
// ─────────────────────────────────────────────────────────────────────────────

/// Display width of a character in terminal columns (matches PTY grid / `dragent_terminal`).
pub fn terminal_char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0).max(1)
}

/// Total terminal column count for a line of text.
pub fn line_terminal_column_count(line: &str) -> usize {
    line.chars().map(terminal_char_width).sum()
}

/// Map an X offset within a line (pixels from line origin) to a terminal column index.
pub fn x_to_terminal_column(x_in_line: f32, cell_width: f32, max_cols: usize) -> usize {
    if cell_width <= 0.0 {
        return 0;
    }
    let col = (x_in_line.max(0.0) / cell_width).floor() as usize;
    col.min(max_cols)
}

/// Center X of a terminal column in element-local coordinates.
pub fn terminal_column_center_x(pad_left: f32, col: usize, cell_width: f32) -> f32 {
    pad_left + col as f32 * cell_width + cell_width / 2.0
}

/// Terminal column index at the start of the character at `char_index`.
pub fn terminal_column_at_char_index(line: &str, char_index: usize) -> usize {
    line.chars().take(char_index).map(terminal_char_width).sum()
}

/// Character index whose terminal column range contains `col`.
pub fn char_index_at_terminal_column(line: &str, col: usize) -> usize {
    let mut acc = 0usize;
    for (i, ch) in line.chars().enumerate() {
        let w = terminal_char_width(ch);
        if col < acc + w {
            return i;
        }
        acc += w;
    }
    line.chars().count()
}

/// Extract the substring covering terminal columns `[start_col, end_col)`.
pub fn slice_line_terminal_columns(line: &str, start_col: usize, end_col: usize) -> String {
    if start_col >= end_col {
        return String::new();
    }
    let mut out = String::new();
    let mut col = 0usize;
    for ch in line.chars() {
        let w = terminal_char_width(ch);
        let ch_start = col;
        let ch_end = col + w;
        if ch_end > start_col && ch_start < end_col {
            out.push(ch);
        }
        col = ch_end;
        if col >= end_col {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_chars_use_two_columns() {
        assert_eq!(terminal_char_width('a'), 1);
        assert_eq!(terminal_char_width('你'), 2);
        assert_eq!(line_terminal_column_count("a你b"), 4);
    }

    #[test]
    fn terminal_column_round_trip_at_known_cells() {
        let line = "hello 世界";
        let cell_w = 8.0;
        let pad = 14.0;
        let max = line_terminal_column_count(line);

        for col in [0, 3, 6, 7, 8, max] {
            let x = terminal_column_center_x(pad, col, cell_w);
            let x_in_line = x - pad;
            let resolved = x_to_terminal_column(x_in_line, cell_w, max);
            assert_eq!(resolved, col.min(max), "col {col} round-trip");
        }
    }

    #[test]
    fn slice_line_terminal_columns_respects_wide_chars() {
        let line = "ab你cd";
        assert_eq!(slice_line_terminal_columns(line, 0, 2), "ab");
        assert_eq!(slice_line_terminal_columns(line, 2, 4), "你");
        assert_eq!(slice_line_terminal_columns(line, 4, 6), "cd");
    }

    #[test]
    fn char_index_terminal_column_mapping() {
        let line = "x你y";
        assert_eq!(terminal_column_at_char_index(line, 0), 0);
        assert_eq!(terminal_column_at_char_index(line, 1), 1);
        assert_eq!(terminal_column_at_char_index(line, 2), 3);
        assert_eq!(char_index_at_terminal_column(line, 2), 1);
        assert_eq!(char_index_at_terminal_column(line, 3), 2);
    }
}
