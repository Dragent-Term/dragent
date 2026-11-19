//! Layout helpers for the split-tab command input canvas — caret positioning
//! and click-to-index mapping using the same mono metrics as rendered text.

use warpui::fonts::{Cache as FontCache, FamilyId, Properties};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::AppContext;
use warpui_core::platform::LineStyle;
use warpui_core::text_layout::{ClipConfig, Line, StyleAndFont, TextAlignment};

use crate::console::input_bar::style_runs_for_command;

pub const LINE_SPACING: f32 = 1.0;
/// Keep editable fields stable when the terminal's line-height setting changes.
pub const INPUT_LINE_HEIGHT_RATIO: f32 = 1.15;

pub fn line_style(font_size: f32) -> LineStyle {
    LineStyle {
        font_size,
        line_height_ratio: INPUT_LINE_HEIGHT_RATIO,
        baseline_ratio: 0.8,
        fixed_width_tab_size: None,
    }
}

pub fn line_height_with_spacing(font_size: f32) -> f32 {
    font_size * INPUT_LINE_HEIGHT_RATIO + LINE_SPACING
}

/// Column range within a single line touched by a buffer-wide `[sel_start, sel_end)` selection.
pub fn selection_cols_on_line(
    line_char_offset: usize,
    line_len: usize,
    sel_start: usize,
    sel_end: usize,
) -> Option<(usize, usize)> {
    let line_end = line_char_offset + line_len;
    let eff_start = sel_start.max(line_char_offset);
    let eff_end = sel_end.min(line_end);
    if eff_start >= eff_end {
        return None;
    }
    Some((eff_start - line_char_offset, eff_end - line_char_offset))
}

/// Split a character cursor index into (line_index, column_within_line).

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

/// Unicode scalar count in `text[..byte_idx]`, snapping `byte_idx` to a valid UTF-8 boundary.
///
/// Text layout may return a byte index inside a multi-byte character when mapping x → caret.
fn char_count_before_byte(text: &str, byte_idx: usize) -> usize {
    let mut end = byte_idx.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].chars().count()
}

fn layout_line_uncached(
    text: &str,
    _font: FamilyId,
    font_size: f32,
    max_width: f32,
    style_runs: &[(std::ops::Range<usize>, StyleAndFont)],
    font_cache: &FontCache,
) -> warpui_core::text_layout::Line {
    font_cache.text_layout_system().layout_line(
        text,
        line_style(font_size),
        style_runs,
        max_width.max(1.0),
        ClipConfig::default(),
    )
}

/// Shape one unwrapped line with explicit font properties.
///
/// Source-editor painting, caret placement, and pointer hit-testing all call
/// this helper so font weight and ligature shaping cannot drift between those
/// paths. Colours do not affect glyph geometry and intentionally stay in the
/// presentation layer.
pub fn unwrapped_line_layout(
    text: &str,
    font: FamilyId,
    font_size: f32,
    properties: Properties,
    font_cache: &FontCache,
) -> Line {
    let style = StyleAndFont::new(
        font,
        properties,
        warpui_core::text_layout::TextStyle::default(),
    );
    layout_line_uncached(
        text,
        font,
        font_size,
        1_000_000.0,
        &[(0..text.len(), style)],
        font_cache,
    )
}

/// Return the layout engine's character offset for an unwrapped line.
/// `CaretPosition::start_offset` is already a character index (not a UTF-8
/// byte offset), including for non-ASCII text.
pub fn char_col_at_x_in_layout(text: &str, line: &Line, x: f32) -> usize {
    line.caret_index_for_x_unbounded(x.max(0.0))
        .min(text.chars().count())
}

pub fn caret_x_in_layout(text: &str, line: &Line, char_index: usize) -> f32 {
    line.caret_position_for_index(char_index.min(text.chars().count()))
}

/// Caret X for a plain single-style line (search / rename fields).
pub fn caret_x_plain(
    text: &str,
    char_index: usize,
    font: FamilyId,
    font_size: f32,
    app: &AppContext,
) -> f32 {
    let style_and_font = StyleAndFont::new(
        font,
        Properties::default(),
        warpui_core::text_layout::TextStyle::default(),
    );
    let style_runs = &[(0..text.len(), style_and_font)];
    let line = layout_line_uncached(text, font, font_size, 10000.0, style_runs, app.font_cache());
    line.caret_position_for_index(byte_index_for_char(text, char_index))
}

/// Caret X during `View::render` — uses font cache only (no presenter borrow).
pub fn caret_x_for_char_index(
    text: &str,
    char_index: usize,
    font: FamilyId,
    font_size: f32,
    theme: &crate::theme::Theme,
    app: &AppContext,
) -> f32 {
    let style_runs = style_runs_for_command(text, font, theme);
    let line = layout_line_uncached(
        text,
        font,
        font_size,
        10000.0,
        &style_runs,
        app.font_cache(),
    );
    line.caret_position_for_index(byte_index_for_char(text, char_index))
}

/// Map a click inside saved text bounds to a character index (plain text).
pub fn char_index_at_click_plain(
    buffer: &str,
    click_pos: Vector2F,
    bounds: RectF,
    font: FamilyId,
    font_size: f32,
    font_cache: &FontCache,
) -> usize {
    let relative_x = (click_pos.x() - bounds.origin().x()).max(0.0);
    let style_and_font = StyleAndFont::new(
        font,
        Properties::default(),
        warpui_core::text_layout::TextStyle::default(),
    );
    let style_runs = &[(0..buffer.len(), style_and_font)];
    // Use unbounded width so every character is laid out on one line —
    // bounds.width() is the viewport, not the content width, so using it
    // would truncate the layout for scrolled text and map clicks to wrong chars.
    let line = layout_line_uncached(buffer, font, font_size, 10000.0, style_runs, font_cache);
    let byte_idx = line.caret_index_for_x_unbounded(relative_x);
    char_count_before_byte(buffer, byte_idx)
}

/// Map a click inside saved text bounds to a character index (syntax-highlighted command).

/// Given a pixel x position, return the character column index within `line_text`.
pub fn char_col_at_x(
    line_text: &str,
    x: f32,
    font: FamilyId,
    font_size: f32,
    font_cache: &FontCache,
) -> usize {
    let style_and_font = StyleAndFont::new(
        font,
        Properties::default(),
        warpui_core::text_layout::TextStyle::default(),
    );
    let style_runs = &[(0..line_text.len(), style_and_font)];
    let line = layout_line_uncached(line_text, font, font_size, 10000.0, style_runs, font_cache);
    let byte_idx = line.caret_index_for_x_unbounded(x);
    char_count_before_byte(line_text, byte_idx)
}

/// Compute visual sub-line character ranges for a single explicit text line.
///
/// Returns `(char_start, char_end)` pairs relative to `text`, one entry per visual row.
/// `char_end` is exclusive. When `available_width >= 9999.0`, returns a single entry
/// covering the whole text (no wrapping).
pub fn visual_subline_char_ranges(
    text: &str,
    available_width: f32,
    _font: FamilyId,
    font_size: f32,
    style_runs: &[(std::ops::Range<usize>, StyleAndFont)],
    font_cache: &FontCache,
) -> Vec<(usize, usize)> {
    let char_count = text.chars().count();
    if text.is_empty() {
        return vec![(0, 0)];
    }
    if available_width >= 9999.0 {
        return vec![(0, char_count)];
    }

    let frame = font_cache.text_layout_system().layout_text(
        text,
        line_style(font_size),
        style_runs,
        available_width,
        f32::MAX,
        TextAlignment::default(),
        None,
    );

    // Reconstruct the char range of each visual row from the layout.
    //
    // We derive ranges from the *start* of each line and make them contiguous
    // (each row spans from its own start up to the next row's start; the last
    // row runs to the end of the text). We deliberately do NOT close a row at
    // its last visible glyph: at a soft-wrap boundary Core Text keeps the break
    // character (typically whitespace) as `trailing_whitespace_width` and drops
    // it from the line's glyph runs. Ending a row at its last glyph therefore
    // leaves that character in a gap between rows, hiding it from both display
    // and cursor/selection mapping — the "missing letter when pasted text wraps"
    // bug. Contiguous, start-anchored ranges guarantee every character belongs
    // to exactly one visual row.
    let mut starts: Vec<usize> = Vec::new();
    for line in frame.lines() {
        // Char index of this row's first character. Prefer the minimum glyph
        // byte index (glyphs carry byte indices); fall back to caret positions
        // (which carry char indices) for a row with no visible glyphs.
        let start_char = line
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.index)
            .min()
            .map(|byte| char_count_before_byte(text, byte))
            .or_else(|| line.caret_positions.first().map(|c| c.start_offset));
        if let Some(start) = start_char {
            // Keep starts strictly increasing so ranges stay well-formed even
            // if the layout reports rows out of logical order.
            if starts.last().is_none_or(|&prev| start > prev) {
                starts.push(start);
            }
        }
    }

    contiguous_ranges_from_starts(starts, char_count)
}

/// Return the actual laid-out rows used to derive soft-wrap boundaries.
/// Keeping the `Line` objects lets hit-testing use the same caret positions
/// that text layout computed, instead of rebuilding an approximate line later.
pub fn visual_subline_layouts(
    text: &str,
    available_width: f32,
    _font: FamilyId,
    font_size: f32,
    style_runs: &[(std::ops::Range<usize>, StyleAndFont)],
    font_cache: &FontCache,
) -> Vec<(usize, Line)> {
    if text.is_empty() {
        return vec![(
            0,
            layout_line_uncached(text, _font, font_size, 10000.0, style_runs, font_cache),
        )];
    }

    let lines: Vec<Line> = if available_width >= 9999.0 {
        vec![layout_line_uncached(
            text, _font, font_size, 10000.0, style_runs, font_cache,
        )]
    } else {
        font_cache
            .text_layout_system()
            .layout_text(
                text,
                line_style(font_size),
                style_runs,
                available_width,
                f32::MAX,
                TextAlignment::default(),
                None,
            )
            .lines()
            .clone()
    };

    let mut result = Vec::with_capacity(lines.len());
    for (row, line) in lines.into_iter().enumerate() {
        let start = line
            .caret_positions
            .first()
            .map(|caret| caret.start_offset)
            .or_else(|| {
                line.runs
                    .iter()
                    .flat_map(|run| run.glyphs.iter())
                    .map(|glyph| char_count_before_byte(text, glyph.index))
                    .min()
            })
            .unwrap_or(if row == 0 { 0 } else { text.chars().count() });
        result.push((start, line));
    }
    result
}

/// Turn per-row start char indices into contiguous, gap-free `(start, end)`
/// ranges covering `[0, char_count)`. Each row runs up to the next row's start;
/// the final row runs to `char_count`. The first row is anchored at 0 so any
/// leading whitespace belongs to it. Guarantees every character is covered by
/// exactly one row (no dropped characters at soft-wrap boundaries).
fn contiguous_ranges_from_starts(mut starts: Vec<usize>, char_count: usize) -> Vec<(usize, usize)> {
    if starts.is_empty() {
        return vec![(0, char_count)];
    }
    starts[0] = 0;

    let mut ranges = Vec::with_capacity(starts.len());
    for i in 0..starts.len() {
        let start = starts[i];
        let end = starts.get(i + 1).copied().unwrap_or(char_count);
        ranges.push((start, end));
    }
    ranges
}

/// Map a click position to a character index, accounting for visual soft-wrap.
///
/// `visual_lines` is a flat list of `(line_text, abs_char_offset)` — one entry per
/// visual row across ALL explicit lines in the buffer, in top-to-bottom order.
pub fn char_index_at_click_visual(
    click_pos: Vector2F,
    bounds: RectF,
    font: FamilyId,
    font_size: f32,
    visual_lines: &[(String, usize)],
    font_cache: &FontCache,
) -> usize {
    if visual_lines.is_empty() {
        return 0;
    }
    let relative_x = (click_pos.x() - bounds.origin().x()).max(0.0);
    let relative_y = (click_pos.y() - bounds.origin().y()).max(0.0);
    let row = ((relative_y / line_height_with_spacing(font_size)).floor() as usize)
        .min(visual_lines.len() - 1);
    let (line_text, abs_offset) = &visual_lines[row];
    let style_and_font = StyleAndFont::new(
        font,
        Properties::default(),
        warpui_core::text_layout::TextStyle::default(),
    );
    let style_runs = &[(0..line_text.len(), style_and_font)];
    let line = layout_line_uncached(line_text, font, font_size, 10000.0, style_runs, font_cache);
    let byte_idx = line.caret_index_for_x_unbounded(relative_x);
    abs_offset + char_count_before_byte(line_text, byte_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_count_before_byte_snaps_mid_utf8() {
        assert_eq!(char_count_before_byte("你好", 1), 0);
        assert_eq!(char_count_before_byte("你好", 2), 0);
        assert_eq!(char_count_before_byte("你好", 3), 1);
        assert_eq!(char_count_before_byte("$ 你好", 4), 2);
        assert_eq!(char_count_before_byte("$ 你好", 5), 3);
        assert_eq!(char_count_before_byte("$ 你好", 6), 3);
        assert_eq!(char_count_before_byte("\\$ 你好", 4), 3);
    }

    #[test]
    fn contiguous_ranges_cover_all_chars_no_gaps() {
        // Row starts as reported by the layout (e.g. "hello world foo" wrapping
        // into "hello world" / "foo" — the space at index 11 is the soft-wrap
        // break char that Core Text drops from the second row's glyphs).
        let ranges = contiguous_ranges_from_starts(vec![0, 12], 15);
        // Row 0 keeps the break char; nothing between the rows is lost.
        assert_eq!(ranges, vec![(0, 12), (12, 15)]);
        // Every character index belongs to exactly one row.
        let covered: usize = ranges.iter().map(|(s, e)| e - s).sum();
        assert_eq!(covered, 15);
        for w in ranges.windows(2) {
            assert_eq!(w[0].1, w[1].0, "ranges must be contiguous (no gaps)");
        }
    }

    #[test]
    fn contiguous_ranges_anchor_first_row_at_zero() {
        // Even if the layout reports a non-zero first start (leading whitespace
        // with no glyphs), row 0 is anchored at 0 so no leading char is lost.
        assert_eq!(
            contiguous_ranges_from_starts(vec![2, 6], 10),
            vec![(0, 6), (6, 10)]
        );
    }

    #[test]
    fn contiguous_ranges_single_row() {
        assert_eq!(contiguous_ranges_from_starts(vec![0], 7), vec![(0, 7)]);
        assert_eq!(contiguous_ranges_from_starts(vec![], 7), vec![(0, 7)]);
    }

    #[test]
    fn last_byte_excl_snaps_mid_utf8_glyph_index() {
        // Simulate a glyph whose .index lands at byte 2, inside 你 (bytes 0..3).
        // Snapping back to 0 and adding ch_len (3) should give 3.
        let text = "你世界";
        let glyph_index = 2usize; // inside 你
        let mut idx = glyph_index.min(text.len());
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        let ch_len = text[idx..]
            .chars()
            .next()
            .map(|c| c.len_utf8())
            .unwrap_or(0);
        assert_eq!(idx + ch_len, 3); // end of 你
    }
}
