//! Monospace cell grid painter for virtualized terminal output blocks.

use warpui::color::ColorU;
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui_core::presenter::PaintContext;

use crate::console::editor_settings::CursorStyle;
use crate::console::escape_parse::ColoredLine;
use crate::terminal::render::grid_metrics::{line_terminal_column_count, terminal_char_width};
use crate::theme::mono_font_size;

/// Paints a single terminal output line by drawing glyphs and background rectangles directly onto the scene graph.
pub fn paint_grid_line(
    paint_ctx: &mut PaintContext,
    app: &warpui::AppContext,
    line: &ColoredLine,
    origin: Vector2F,
    mono: FamilyId,
    theme: crate::theme::Theme,
    cell_width: f32,
    cell_height: f32,
    selection_range: Option<(usize, usize)>, // terminal column range [start, end)
    cursor_col: Option<usize>,
    hovered_url_range: Option<(usize, usize)>,
    selected_url_range: Option<(usize, usize)>,
    // Terminal column ranges [start, end) to highlight as filter matches.
    match_ranges: &[(usize, usize)],
    glyph_cache: &mut crate::terminal::render::CellGlyphCache,
) {
    let font_cache = app.font_cache();
    let font_id = font_cache.select_font(mono, warpui::fonts::Properties::default());
    let ascent = font_cache.ascent(font_id, mono_font_size());
    let fallback = theme.main_text();

    let line_text: String = line.spans.iter().map(|s| s.text.as_str()).collect();
    let line_cols = line_terminal_column_count(&line_text);
    let mut term_col = 0usize;

    // Translucent warm highlight behind filter-matched cells (alpha keeps the
    // glyph legible). Distinct from the blue selection band so they never read as
    // the same thing.
    let match_highlight_bg = ColorU::new(230, 176, 32, 110);

    for span in &line.spans {
        if span.text.is_empty() {
            continue;
        }

        let span_start_col = term_col;
        let span_cols: usize = span.text.chars().map(terminal_char_width).sum();
        let span_end_col = span_start_col + span_cols;

        // 1. Draw background rectangle if span has background color
        if let Some(bg_color) = span.bg {
            let x = origin.x() + (span_start_col as f32 * cell_width);
            let w = span_cols as f32 * cell_width;
            let rect = RectF::new(vec2f(x, origin.y()), vec2f(w, cell_height));
            paint_ctx
                .scene
                .draw_rect_without_hit_recording(rect)
                .with_background(bg_color);
        }

        // 1b. Draw filter-match highlight(s). Painted before selection so an
        // active selection band visibly overrides a match on the same cells.
        for &(match_start, match_end) in match_ranges {
            let intersect_start = match_start.max(span_start_col);
            let intersect_end = match_end.min(span_end_col);
            if intersect_start < intersect_end {
                let x = origin.x() + (intersect_start as f32 * cell_width);
                let w = (intersect_end - intersect_start) as f32 * cell_width;
                let rect = RectF::new(vec2f(x, origin.y()), vec2f(w, cell_height));
                paint_ctx
                    .scene
                    .draw_rect_without_hit_recording(rect)
                    .with_background(match_highlight_bg);
            }
        }

        // 2. Draw selection background override
        if let Some((start_idx, end_idx)) = selection_range {
            let intersect_start = start_idx.max(span_start_col);
            let intersect_end = end_idx.min(span_end_col);

            if intersect_start < intersect_end {
                let x = origin.x() + (intersect_start as f32 * cell_width);
                let w = (intersect_end - intersect_start) as f32 * cell_width;
                let selection_bg = ColorU::new(25, 90, 120, 180);
                let rect = RectF::new(vec2f(x, origin.y()), vec2f(w, cell_height));
                paint_ctx
                    .scene
                    .draw_rect_without_hit_recording(rect)
                    .with_background(selection_bg);
            }
        }

        // Draw selected URL background override
        if let Some((start_idx, end_idx)) = selected_url_range {
            let intersect_start = start_idx.max(span_start_col);
            let intersect_end = end_idx.min(span_end_col);

            if intersect_start < intersect_end {
                let x = origin.x() + (intersect_start as f32 * cell_width);
                let w = (intersect_end - intersect_start) as f32 * cell_width;
                let rect = RectF::new(vec2f(x, origin.y()), vec2f(w, cell_height));
                paint_ctx
                    .scene
                    .draw_rect_without_hit_recording(rect)
                    .with_background(theme.link_surface());
            }
        }

        // 3. Draw each character glyph
        for ch in span.text.chars() {
            let w = terminal_char_width(ch);
            let x = origin.x() + (term_col as f32 * cell_width);
            let y = origin.y() + ascent;
            let cell_w = w as f32 * cell_width;

            let is_selected = selection_range
                .map(|(s, e)| term_col < e && term_col + w > s)
                .unwrap_or(false);

            let is_cursor = cursor_col
                .map(|c| c >= term_col && c < term_col + w)
                .unwrap_or(false);

            let is_hovered_url = hovered_url_range
                .map(|(s, e)| term_col < e && term_col + w > s)
                .unwrap_or(false);
            let is_selected_url = selected_url_range
                .map(|(s, e)| term_col < e && term_col + w > s)
                .unwrap_or(false);

            if is_cursor {
                // Honor the cursor-style setting like the element-based grid
                // renderer does: full-cell block, 2px left bar, or 2px bottom
                // underline. Previously this scene-graph path always painted
                // the full block.
                match crate::console::editor_settings::EditorSettings::global()
                    .terminal
                    .cursor_style
                {
                    CursorStyle::Block => {
                        let rect = RectF::new(vec2f(x, origin.y()), vec2f(cell_w, cell_height));
                        paint_ctx
                            .scene
                            .draw_rect_without_hit_recording(rect)
                            .with_background(theme.accent);
                    }
                    CursorStyle::Bar => {
                        let rect = RectF::new(vec2f(x, origin.y()), vec2f(2.0, cell_height));
                        paint_ctx
                            .scene
                            .draw_rect_without_hit_recording(rect)
                            .with_background(theme.accent);
                    }
                    CursorStyle::Underline => {
                        let rect = RectF::new(
                            vec2f(x, origin.y() + cell_height - 2.0),
                            vec2f(cell_w, 2.0),
                        );
                        paint_ctx
                            .scene
                            .draw_rect_without_hit_recording(rect)
                            .with_background(theme.accent);
                    }
                }
            }

            let color = if is_cursor {
                theme.background
            } else if is_selected {
                theme.main_text()
            } else if is_hovered_url || is_selected_url {
                theme.link()
            } else {
                span.foreground(fallback)
            };

            if (is_hovered_url || is_selected_url) && !is_cursor {
                let underline_y = origin.y() + cell_height - 2.0;
                let rect = RectF::new(vec2f(x, underline_y), vec2f(cell_w, 1.0));
                paint_ctx
                    .scene
                    .draw_rect_without_hit_recording(rect)
                    .with_background(color);
            }

            if let Some((glyph_id, font_id_resolved)) = glyph_cache.get(font_cache, font_id, ch) {
                paint_ctx.scene.draw_glyph(
                    vec2f(x, y),
                    glyph_id,
                    font_id_resolved,
                    mono_font_size(),
                    color,
                );
            }

            term_col += w;
        }
    }

    // 4. Draw cursor past end-of-line if needed
    if let Some(c_col) = cursor_col {
        if c_col >= line_cols {
            let x = origin.x() + (c_col as f32 * cell_width);
            let (w, h, dy) = match crate::console::editor_settings::EditorSettings::global()
                .terminal
                .cursor_style
            {
                CursorStyle::Block => (cell_width, cell_height, 0.0),
                CursorStyle::Bar => (2.0, cell_height, 0.0),
                CursorStyle::Underline => (cell_width, 2.0, cell_height - 2.0),
            };
            let rect = RectF::new(vec2f(x, origin.y() + dy), vec2f(w, h));
            paint_ctx
                .scene
                .draw_rect_without_hit_recording(rect)
                .with_background(theme.accent);
        }
    }
}
