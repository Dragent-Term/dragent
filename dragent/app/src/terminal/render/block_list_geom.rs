//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::render::block_output::{find_links_in_run, link_target, wrapped_run_bounds};
use crate::terminal::view::actions::{EntryHint, LinkHint, LinkOccurrence, TerminalAction};
use crate::workspace::InputEditor;
use dragent_terminal::StyledLine;
use warpui::ViewHandle;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, ParentElement, Point, SavePosition, ScrollData,
    ScrollableElement, SelectableElement, Selection, SelectionFragment, SmartSelectFn, Text,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{Vector2F, vec2f};
use warpui::platform::Cursor;
use warpui::string_offset::{ByteOffset, CharOffset};
use warpui::text::{IsRect, SelectionDirection, SelectionType};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::Pixels;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};
use warpui_core::scene::ZIndex;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text::TextBuffer;
use warpui_core::text::word_boundaries::WordBoundariesPolicy;
use warpui_core::text_layout::ClipConfig;

use crate::console::block_filter::{BlockFilterState, count_block_matches};
use crate::console::dir_listing::{DirSort, DirView, entry_click_hint_label};
use crate::console::escape_parse::{ColoredLine, TextSpan, styled_to_colored};
use crate::icons::{AppIcon, render_icon};
use crate::terminal::model::block::{Block, BlockStatus, live_primary_line_count};
use crate::terminal::model::block_height_tree::{
    BLOCK_HEADER_HEIGHT_PX, BlockHeightIndex, BlockListEntry, COMPLETED_BLOCK_MAX_LINES,
};
use crate::terminal::model::is_signal_exit;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::render::CellGlyphCache;
use crate::terminal::render::block_card_element::header_command_line;
use crate::terminal::render::dir_listing_paint::{
    DIR_BODY_PAD_LEFT, DirBlockState, DirListingHit, DirListingLayout, DirListingPaintParams,
    dir_entry_char_width, dir_entry_name_origin_in_body, dir_listing_body_height,
    dir_listing_selection_lines, layout_dir_listing, paint_dir_listing, sort_dir_entries,
};
use crate::terminal::render::grid_metrics::{
    CellMetrics, char_index_at_terminal_column, line_terminal_column_count,
    slice_line_terminal_columns, terminal_column_at_char_index, terminal_column_center_x,
    x_to_terminal_column,
};
use crate::terminal::render::grid_renderer::{GridLineParams, build_alt_screen_column};
use crate::terminal::viewport::{ScrollPosition, VisibleEntry, VisibleEntryKind};
use crate::theme::{TERMINAL_PADDING_LEFT, Theme, mono_font_size};

/// Wrappable columns for block body text given the viewport width.
/// Left padding is 14px; returns 1 as a floor so nothing divides by zero.
pub(crate) fn body_wrap_cols(viewport_w: f32, cell_w: f32) -> usize {
    const BODY_PAD_LEFT: f32 = 14.0;
    if cell_w <= 0.0 || viewport_w <= BODY_PAD_LEFT {
        return 1;
    }
    ((viewport_w - BODY_PAD_LEFT) / cell_w).floor() as usize
}

/// Total visual (wrapped) row count for a slice of ColoredLines.
pub(crate) fn count_wrapped_rows(lines: &[ColoredLine], wrap_cols: usize) -> usize {
    if wrap_cols == 0 {
        return lines.len().max(1);
    }
    lines
        .iter()
        .map(|line| {
            let cols: usize = line
                .spans
                .iter()
                .map(|s| line_terminal_column_count(&s.text))
                .sum();
            if cols <= wrap_cols {
                1
            } else {
                (cols + wrap_cols - 1) / wrap_cols
            }
        })
        .sum::<usize>()
        .max(1)
}

/// Extract columns [start_col, end_col) from a ColoredLine, preserving span colours.
pub(crate) fn slice_colored_line_cols(
    line: &ColoredLine,
    start_col: usize,
    end_col: usize,
) -> ColoredLine {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut current_col = 0usize;
    for span in &line.spans {
        let span_cols = line_terminal_column_count(&span.text);
        let span_end = current_col + span_cols;
        let overlap_start = start_col.max(current_col);
        let overlap_end = end_col.min(span_end);
        if overlap_start < overlap_end {
            let local_start = overlap_start - current_col;
            let local_end = overlap_end - current_col;
            let text = slice_line_terminal_columns(&span.text, local_start, local_end);
            if !text.is_empty() {
                spans.push(TextSpan {
                    text,
                    color: span.color,
                    bg: span.bg,
                    flags: span.flags,
                });
            }
        }
        current_col = span_end;
        if current_col >= end_col {
            break;
        }
    }
    ColoredLine {
        spans,
        wrapped: false,
    }
}

impl super::block_list_element::BlockListElement {
    /// Terminal-column ranges to highlight for `line`, given the active filter.
    /// Empty unless `block_id` owns the filter and the line has matches. Match
    /// positions come back from the filter as char indices; convert them to
    /// columns the same way `paint_grid_line` accumulates `term_col`.
    pub(crate) fn filter_match_columns(
        &self,
        block_id: usize,
        line: &ColoredLine,
    ) -> Vec<(usize, usize)> {
        let Some((fid, filter)) = &self.match_filter else {
            return Vec::new();
        };
        if *fid != block_id {
            return Vec::new();
        }
        let plain = line.plain_text();
        filter
            .match_char_ranges(&plain)
            .into_iter()
            .map(|(cs, ce)| {
                (
                    terminal_column_at_char_index(&plain, cs),
                    terminal_column_at_char_index(&plain, ce),
                )
            })
            .filter(|(s, e)| s < e)
            .collect()
    }

    fn resolve_terminal_col(&self, rel_x: f32, pad_left: f32, line_text: &str) -> usize {
        let max = line_terminal_column_count(line_text);
        x_to_terminal_column(rel_x - pad_left, self.cell_metrics.width, max)
    }

    pub(crate) fn selection_slice(line: &str, start_col: usize, end_col: usize) -> String {
        let max_col = line_terminal_column_count(line);
        slice_line_terminal_columns(line, start_col.min(max_col), end_col.min(max_col))
    }

    pub(crate) fn line_text_at_paint_index(
        &self,
        block_id: usize,
        paint_line_idx: usize,
    ) -> Option<String> {
        if block_id == usize::MAX {
            let line = self.bootstrap_lines.get(paint_line_idx)?;
            return Some(line.spans.iter().map(|s| s.text.as_str()).collect());
        }
        if block_id == usize::MAX - 1 {
            let prompt = self.live_prompt.as_ref()?;
            let line = prompt.lines.get(paint_line_idx)?;
            let colored = styled_to_colored(line, &self.theme);
            return Some(colored.spans.iter().map(|s| s.text.as_str()).collect());
        }
        let (expanded, text_lines, _) = self.block_lines.get(&block_id)?;
        if !*expanded {
            return None;
        }
        let all_lines = self.all_block_lines.get(&block_id)?;
        let offset = all_lines.len().saturating_sub(text_lines.len());
        let line = all_lines.get(paint_line_idx + offset)?;
        Some(line.spans.iter().map(|s| s.text.as_str()).collect())
    }

    pub(crate) fn block_wrap_cols(&self) -> usize {
        let width = self.viewport_size.map(|s| s.x()).unwrap_or(0.0);
        body_wrap_cols(width, self.cell_metrics.width)
    }

    pub(crate) fn line_columns(line: &ColoredLine) -> usize {
        line.spans
            .iter()
            .map(|span| line_terminal_column_count(&span.text))
            .sum()
    }

    /// Map a y position in a block body to the logical line and visual chunk
    /// rendered at that position. Long terminal lines are painted as several
    /// rows, so hit testing must use the same row map instead of `line * cell_h`.
    pub(crate) fn wrapped_line_at_y(
        &self,
        block_id: usize,
        lines: &[ColoredLine],
        body_y: f32,
    ) -> Option<(usize, usize, usize)> {
        let wrap_cols = self.block_wrap_cols();
        let row = (body_y.max(0.0) / self.cell_metrics.height).floor() as usize;
        let offsets = self.row_offsets.get(&block_id)?;
        let line_idx = offsets
            .partition_point(|offset| *offset <= row)
            .saturating_sub(1)
            .min(lines.len().checked_sub(1)?);
        let cols = Self::line_columns(&lines[line_idx]);
        let chunk = row
            .saturating_sub(offsets[line_idx])
            .min(offsets[line_idx + 1] - offsets[line_idx] - 1);
        let chunk_start = chunk * wrap_cols;
        Some((
            line_idx,
            chunk_start,
            cols.saturating_sub(chunk_start).min(wrap_cols),
        ))
    }

    pub(crate) fn align_offset_px(&self) -> f32 {
        let total_h = self.height_index.total_height_px();
        if total_h < self.viewport_height {
            self.viewport_height - total_h
        } else {
            0.0
        }
    }

    pub(crate) fn element_local_y(&self, absolute_point: Vector2F) -> Option<f32> {
        Some((absolute_point - self.origin?.xy()).y())
    }

    /// Compare two absolute points using paint-space line Y (matches highlight ordering).
    pub(crate) fn compare_points_by_paint_position(
        &self,
        absolute_point: Vector2F,
        absolute_point_other: Vector2F,
    ) -> Option<std::cmp::Ordering> {
        match (
            self.resolve_point(absolute_point),
            self.resolve_point(absolute_point_other),
        ) {
            (Some(start), Some(end)) => {
                let y1 = self.paint_y_for_line(start.0, start.1).unwrap_or(-1.0);
                let y2 = self.paint_y_for_line(end.0, end.1).unwrap_or(-1.0);
                const Y_EPS: f32 = 0.1;
                if (y1 - y2).abs() > Y_EPS {
                    y1.partial_cmp(&y2)
                } else {
                    Some(start.2.cmp(&end.2))
                }
            }
            _ => {
                // Fallback when a point lands on non-text chrome (e.g. dir grid padding).
                let y1 = self.element_local_y(absolute_point)?;
                let y2 = self.element_local_y(absolute_point_other)?;
                const Y_EPS: f32 = 1.0;
                if (y1 - y2).abs() > Y_EPS {
                    y1.partial_cmp(&y2)
                } else {
                    let col1 = self
                        .resolve_point(absolute_point)
                        .map(|(_, _, c)| c)
                        .unwrap_or(0);
                    let col2 = self
                        .resolve_point(absolute_point_other)
                        .map(|(_, _, c)| c)
                        .unwrap_or(0);
                    Some(col1.cmp(&col2))
                }
            }
        }
    }

    pub(crate) fn actual_header_height(&self, block_id: usize) -> f32 {
        self.headers
            .get(&block_id)
            .and_then(|h| h.size())
            .map(|s| s.y())
            .unwrap_or(BLOCK_HEADER_HEIGHT_PX)
    }

    pub(crate) fn block_body_origin_y(&self, block_id: usize) -> Option<f32> {
        let mut y = self.align_offset_px();
        for entry in &self.visible_entries {
            if let VisibleEntryKind::Block { block_id: bid } = entry.kind {
                if bid == block_id {
                    return Some(y + self.actual_header_height(block_id));
                }
            }
            y += entry.height_px;
        }
        None
    }

    pub(crate) fn recompute_dir_layouts(&mut self, content_width_px: f32) {
        self.content_width_px = content_width_px;
        self.dir_layouts.clear();
        for (block_id, state) in &self.dir_blocks {
            self.dir_layouts.insert(
                *block_id,
                layout_dir_listing(&state.entries, self.dir_view, content_width_px),
            );
        }
    }

    pub(crate) fn dir_hit_at_point(&self, pt: Vector2F) -> Option<(usize, DirListingHit)> {
        let rel = pt - self.origin?.xy();
        let align_offset = self.align_offset_px();
        let mut y = align_offset;
        for entry in &self.visible_entries {
            let entry_bottom = y + entry.height_px;
            if rel.y() < y || rel.y() >= entry_bottom {
                y = entry_bottom;
                continue;
            }
            let VisibleEntryKind::Block { block_id } = entry.kind else {
                return None;
            };
            if !self.dir_blocks.contains_key(&block_id) {
                return None;
            }
            let y_in_entry = rel.y() - y;
            let header_h = self.actual_header_height(block_id);
            if y_in_entry < header_h {
                return None;
            }
            let body_local = vec2f(rel.x(), y_in_entry - header_h);
            let layout = self.dir_layouts.get(&block_id)?;
            let hit = layout.hit_test(body_local)?;
            return Some((
                block_id,
                match hit {
                    DirListingHit::Entry {
                        entry_index,
                        is_dir,
                        ..
                    } => DirListingHit::Entry {
                        block_id,
                        entry_index,
                        is_dir,
                    },
                    other => other,
                },
            ));
        }
        None
    }

    /// Y coordinate in element-local space for a paint-window line (matches `paint()` layout).
    pub(crate) fn paint_y_for_line(&self, block_id: usize, paint_line: usize) -> Option<f32> {
        let align_offset = self.align_offset_px();
        let cell_h = self.cell_metrics.height;
        let mut y = align_offset;
        for entry in &self.visible_entries {
            match entry.kind {
                VisibleEntryKind::Block { block_id: bid } if bid == block_id => {
                    let header_h = self.actual_header_height(block_id);
                    if let Some(layout) = self.dir_layouts.get(&block_id) {
                        if let Some(cy) = layout.line_center_y(paint_line) {
                            return Some(y + header_h + cy - cell_h * 0.5);
                        }
                    }
                    let wrapped_rows_before = self
                        .row_offsets
                        .get(&block_id)
                        .map(|offsets| offsets[paint_line.min(offsets.len() - 1)])
                        .unwrap_or(paint_line);
                    return Some(y + header_h + 20.0 + wrapped_rows_before as f32 * cell_h);
                }
                VisibleEntryKind::Bootstrap if block_id == usize::MAX => {
                    return Some(y + 8.0 + paint_line as f32 * cell_h);
                }
                VisibleEntryKind::LivePrompt if block_id == usize::MAX - 1 => {
                    return Some(y + 8.0 + paint_line as f32 * cell_h);
                }
                VisibleEntryKind::AgentBlock { .. } => y += entry.height_px,
                _ => y += entry.height_px,
            }
        }
        None
    }

    pub(crate) fn order_selection_endpoints(
        &self,
        start: (usize, usize, usize),
        end: (usize, usize, usize),
    ) -> ((usize, usize, usize), (usize, usize, usize)) {
        let is_reversed = if start.0 != end.0 {
            if start.0 == usize::MAX {
                false
            } else if end.0 == usize::MAX {
                true
            } else if start.0 == usize::MAX - 1 {
                true
            } else if end.0 == usize::MAX - 1 {
                false
            } else {
                let mut ordered_block_ids = Vec::new();
                for item_entry in self.height_index.visible_entries(0.0, f32::INFINITY) {
                    match item_entry.entry {
                        BlockListEntry::Block { block_id, .. } => {
                            ordered_block_ids.push(block_id);
                        }
                        BlockListEntry::AgentBlock { agent_block_id, .. } => {
                            ordered_block_ids.push(usize::MAX - 1000 - agent_block_id);
                        }
                        _ => {}
                    }
                }
                let pos_start = ordered_block_ids.iter().position(|&id| id == start.0);
                let pos_end = ordered_block_ids.iter().position(|&id| id == end.0);
                match (pos_start, pos_end) {
                    (Some(p_start), Some(p_end)) => p_start > p_end,
                    _ => start.0 > end.0,
                }
            }
        } else if start.1 != end.1 {
            start.1 > end.1
        } else {
            start.2 > end.2
        };
        if is_reversed {
            (end, start)
        } else {
            (start, end)
        }
    }

    pub(crate) fn selection_y_range(
        &self,
        n_start: (usize, usize, usize),
        n_end: (usize, usize, usize),
    ) -> (f32, f32) {
        (
            self.paint_y_for_line(n_start.0, n_start.1).unwrap_or(-1.0),
            self.paint_y_for_line(n_end.0, n_end.1).unwrap_or(-1.0),
        )
    }

    pub(crate) fn selection_cols_for_dir_line(
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
        paint_line: usize,
        line_text_len: usize,
    ) -> Option<(usize, usize)> {
        if paint_line < start_line || paint_line > end_line {
            return None;
        }
        let end_col = end_col.min(line_text_len);
        let start_col = start_col.min(line_text_len);
        if start_line == end_line && paint_line == start_line {
            Some((start_col, end_col.max(start_col)))
        } else if paint_line == start_line {
            Some((start_col, line_text_len))
        } else if paint_line == end_line {
            Some((0, end_col))
        } else {
            Some((0, line_text_len))
        }
    }

    pub(crate) fn paint_dir_entry_selection(
        &self,
        ctx: &mut PaintContext,
        body_origin: Vector2F,
        region: &crate::terminal::render::dir_listing_paint::DirEntryRegion,
        start_col: usize,
        end_col: usize,
    ) {
        if start_col >= end_col {
            return;
        }
        let char_w = dir_entry_char_width();
        let name_origin = body_origin + dir_entry_name_origin_in_body(region);
        let x = name_origin.x() + start_col as f32 * char_w;
        let w = (end_col - start_col) as f32 * char_w;
        let h = region.name_rect.size().y() - 8.0;
        let selection_bg = ColorU::new(25, 90, 120, 180);
        let rect = RectF::new(vec2f(x, name_origin.y()), vec2f(w, h.max(12.0)));
        ctx.scene
            .draw_rect_without_hit_recording(rect)
            .with_background(selection_bg);
    }

    pub(crate) fn selection_cols_for_paint_line(
        &self,
        selection: (usize, usize, usize, f32, f32),
        block_id: usize,
        paint_line: usize,
        line_text_len: usize,
    ) -> Option<(usize, usize)> {
        let (n_start, n_start_col, n_end_col, n_start_y, n_end_y) = selection;
        let curr_y = self.paint_y_for_line(block_id, paint_line)?;
        let cell_h = self.cell_metrics.height;

        if curr_y > n_start_y + 0.1 && curr_y + cell_h < n_end_y - 0.1 {
            Some((0, line_text_len))
        } else if (curr_y - n_start_y).abs() < 0.1 && (curr_y - n_end_y).abs() < 0.1 {
            Some((n_start_col, n_end_col))
        } else if (curr_y - n_start_y).abs() < 0.1 {
            Some((n_start_col, line_text_len))
        } else if (curr_y - n_end_y).abs() < 0.1 {
            Some((0, n_end_col))
        } else {
            None
        }
    }

    pub(crate) fn paint_selection_band(
        &self,
        ctx: &mut PaintContext,
        origin: Vector2F,
        start_col: usize,
        end_col: usize,
    ) {
        if start_col >= end_col {
            return;
        }
        let cell_w = self.cell_metrics.width;
        let cell_h = self.cell_metrics.height;
        let x = origin.x() + start_col as f32 * cell_w;
        let w = (end_col - start_col) as f32 * cell_w;
        let selection_bg = ColorU::new(25, 90, 120, 180);
        let rect = RectF::new(vec2f(x, origin.y()), vec2f(w, cell_h));
        ctx.scene
            .draw_rect_without_hit_recording(rect)
            .with_background(selection_bg);
    }

    pub(crate) fn resolve_point(&self, pt: Vector2F) -> Option<(usize, usize, usize)> {
        let mut rel = pt - self.origin?.xy();
        let align_offset = self.align_offset_px();
        let cell_w = self.cell_metrics.width;
        let cell_h = self.cell_metrics.height;

        // Selection drags intentionally continue outside the clipped viewport.
        // Resolve those points to the nearest rendered edge so the selection stays
        // materialized while edge auto-scroll brings more content into view.
        if !self.visible_entries.is_empty() {
            let content_top = align_offset;
            let content_bottom = align_offset
                + self
                    .visible_entries
                    .iter()
                    .map(|entry| entry.height_px)
                    .sum::<f32>();
            let max_y = (content_bottom - 0.01).max(content_top);
            rel.set_y(rel.y().clamp(content_top, max_y));
        }
        let mut y = align_offset;

        for entry in &self.visible_entries {
            let entry_bottom = y + entry.height_px;
            if rel.y() < y || rel.y() >= entry_bottom {
                y = entry_bottom;
                continue;
            }
            let y_in_entry = rel.y() - y;

            return match entry.kind {
                VisibleEntryKind::Block { block_id } => {
                    let header_h = self.actual_header_height(block_id);
                    if y_in_entry < header_h {
                        Some((block_id, 0, 0))
                    } else {
                        let body_y = y_in_entry - header_h;
                        if body_y < 0.0 {
                            Some((block_id, 0, 0))
                        } else if let Some(state) = self.dir_blocks.get(&block_id) {
                            let layout = self.dir_layouts.get(&block_id)?;
                            let body_local = vec2f(rel.x(), body_y);
                            let (line, col) = layout.hit_test_selection(
                                body_local,
                                &state.entries,
                                self.dir_view,
                            )?;
                            Some((block_id, line, col))
                        } else {
                            let (_, text_lines, _) = self.block_lines.get(&block_id)?;
                            let body_y = body_y - 20.0;
                            let (paint_line, chunk_start, chunk_len) =
                                self.wrapped_line_at_y(block_id, text_lines, body_y)?;
                            let col =
                                self.resolve_terminal_col(rel.x(), 14.0, &" ".repeat(chunk_len));
                            Some((block_id, paint_line, chunk_start + col))
                        }
                    }
                }
                VisibleEntryKind::Bootstrap => {
                    let body_y = y_in_entry - 8.0;
                    if body_y < 0.0 || self.bootstrap_lines.is_empty() {
                        Some((usize::MAX, 0, 0))
                    } else {
                        let paint_line =
                            ((body_y / cell_h) as usize).min(self.bootstrap_lines.len() - 1);
                        let line_text: String = self.bootstrap_lines[paint_line]
                            .spans
                            .iter()
                            .map(|s| s.text.as_str())
                            .collect();
                        let col =
                            self.resolve_terminal_col(rel.x(), TERMINAL_PADDING_LEFT, &line_text);
                        Some((usize::MAX, paint_line, col))
                    }
                }
                VisibleEntryKind::LivePrompt => {
                    let prompt = self.live_prompt.as_ref()?;
                    let body_y = y_in_entry - 8.0;
                    if body_y < 0.0 || prompt.lines.is_empty() {
                        Some((usize::MAX - 1, 0, 0))
                    } else {
                        let paint_line = ((body_y / cell_h) as usize).min(prompt.lines.len() - 1);
                        let colored = styled_to_colored(&prompt.lines[paint_line], &self.theme);
                        let line_text: String =
                            colored.spans.iter().map(|s| s.text.as_str()).collect();
                        let col =
                            self.resolve_terminal_col(rel.x(), TERMINAL_PADDING_LEFT, &line_text);
                        Some((usize::MAX - 1, paint_line, col))
                    }
                }
                VisibleEntryKind::AgentBlock { agent_block_id } => {
                    let sel_id = usize::MAX - 1000 - agent_block_id;
                    if y_in_entry < BLOCK_HEADER_HEIGHT_PX {
                        Some((sel_id, 0, 0))
                    } else {
                        let body_y = y_in_entry - BLOCK_HEADER_HEIGHT_PX;
                        if body_y < 0.0 {
                            Some((sel_id, 0, 0))
                        } else if let Some((_, text_lines, _)) = self.block_lines.get(&sel_id) {
                            let body_y = body_y - 8.0;
                            let (paint_line, chunk_start, chunk_len) =
                                self.wrapped_line_at_y(sel_id, text_lines, body_y)?;
                            let col =
                                self.resolve_terminal_col(rel.x(), 16.0, &" ".repeat(chunk_len));
                            Some((sel_id, paint_line, chunk_start + col))
                        } else {
                            // block_lines stale or empty; return start-of-block position.
                            // get_selection will use the element tree for accurate extraction.
                            Some((sel_id, 0, 0))
                        }
                    }
                }
                VisibleEntryKind::ViewportClearGap => None,
                VisibleEntryKind::BlockSpacing => {
                    // Trailing spacing before live prompt — snap to nearest selectable edge.
                    let rel_x = rel.x();
                    let prev = self.visible_entries.iter().rposition(|e| {
                        matches!(
                            e.kind,
                            VisibleEntryKind::Block { .. }
                                | VisibleEntryKind::Bootstrap
                                | VisibleEntryKind::LivePrompt
                        )
                    });
                    if let Some(prev_idx) = prev {
                        if let VisibleEntryKind::Block { block_id } =
                            self.visible_entries[prev_idx].kind
                        {
                            let (_, text_lines, _) = self.block_lines.get(&block_id)?;
                            if text_lines.is_empty() {
                                return None;
                            }
                            let last = text_lines.len() - 1;
                            let col = ((rel_x - 14.0) / cell_w).max(0.0) as usize;
                            return Some((block_id, last, col));
                        }
                    }
                    None
                }
            };
        }
        None
    }

    /// Returns true when `pt` falls inside a painted fix card, not in the block text body.
    /// Used to suppress URL hit-testing so fix card buttons receive the event.
    pub(crate) fn is_in_fix_card_area(&self, pt: Vector2F) -> bool {
        let Some(origin) = self.origin else {
            return false;
        };
        let rel = pt - origin.xy();
        let align_offset = self.align_offset_px();
        let cell_h = self.cell_metrics.height;
        let mut y = align_offset;

        for entry in &self.visible_entries {
            let entry_bottom = y + entry.height_px;
            if rel.y() < y || rel.y() >= entry_bottom {
                y = entry_bottom;
                continue;
            }
            let VisibleEntryKind::Block { block_id } = entry.kind else {
                return false;
            };
            if !self.fix_card_elements.contains_key(&block_id) {
                return false;
            }
            let y_in_entry = rel.y() - y;
            let body_text_h = self
                .block_lines
                .get(&block_id)
                .map(|(expanded, lines, _)| {
                    if *expanded && !lines.is_empty() {
                        20.0 + self
                            .row_offsets
                            .get(&block_id)
                            .and_then(|offsets| offsets.last())
                            .copied()
                            .unwrap_or(0) as f32
                            * cell_h
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);
            return y_in_entry >= self.actual_header_height(block_id) + body_text_h;
        }
        false
    }

    pub(crate) fn set_local_dir_hover(&mut self, hover: crate::terminal::view::actions::DirHover) {
        self.dir_hovered_entry = hover.entry;
        self.dir_hovered_sort = hover.sort;
        self.dir_hovered_view = hover.view;
    }

    /// True when `pt` is over the visible part of the list. `resolve_point`
    /// deliberately clamps outside points onto the nearest edge row (drag
    /// selection needs that), so hover/click hit-testing must check this
    /// first — otherwise the pointer over the input box below the list
    /// "hovers" whatever link sits on the last row.
    pub(crate) fn is_in_visible_viewport(&self, pt: Vector2F) -> bool {
        let Some(origin) = self.origin else {
            return false;
        };
        let rel = pt - origin.xy();
        let width = self.viewport_size.map_or(0.0, |s| s.x());
        rel.x() >= 0.0
            && rel.x() < width
            && rel.y() >= self.first_entry_offset_px
            && rel.y() < self.first_entry_offset_px + self.viewport_height
    }

    /// The lines link detection runs over for `block_id`, plus the offset
    /// from a paint line index to an index in that list (completed blocks
    /// paint only their tail, but a link's identity uses the full list).
    fn link_lines(&self, block_id: usize) -> Option<(Cow<'_, [ColoredLine]>, usize)> {
        if block_id == usize::MAX {
            Some((Cow::Borrowed(self.bootstrap_lines.as_slice()), 0))
        } else if block_id == usize::MAX - 1 {
            let prompt = self.live_prompt.as_ref()?;
            let lines: Vec<ColoredLine> = prompt
                .lines
                .iter()
                .map(|line| styled_to_colored(line, &self.theme))
                .collect();
            Some((Cow::Owned(lines), 0))
        } else {
            let (_, text_lines, _) = self.block_lines.get(&block_id)?;
            let all_lines = self.all_block_lines.get(&block_id)?;
            let offset = all_lines.len().saturating_sub(text_lines.len());
            Some((Cow::Borrowed(all_lines.as_slice()), offset))
        }
    }

    /// The link under `pt`, identified by where it starts — a path that the
    /// terminal soft-wrapped is found whole from either of its rows, and two
    /// rows showing the same text are still different links.
    pub(crate) fn find_url_at_point(&self, pt: Vector2F) -> Option<LinkHint> {
        if !self.is_in_visible_viewport(pt) {
            return None;
        }
        let (block_id, line_idx, col_idx) = self.resolve_point(pt)?;
        // Selection deliberately snaps headers, padding, spacing, and points
        // beyond the text to the nearest selectable position. Links must not
        // inherit those enlarged hit areas: verify the pointer is inside the
        // painted cell at the resolved position (including soft wrapping).
        let center = self.point_for_position(block_id, line_idx, col_idx)?;
        let half_width = self.cell_metrics.width * 0.5;
        let half_height = self.cell_metrics.height * 0.5;
        if !(pt.x() >= center.x() - half_width
            && pt.x() < center.x() + half_width
            && pt.y() >= center.y() - half_height
            && pt.y() < center.y() + half_height)
        {
            return None;
        }
        // An `ls` card is painted as a grid of entry chips with its own
        // hover/click handling; its line list is only the entry names kept for
        // text selection. Link-detecting those names (`src/` matches the folder
        // pattern) would claim every hover and click on the card, so its own
        // hover underline and entry selection would never run.
        if self.dir_blocks.contains_key(&block_id) {
            return None;
        }
        if block_id != usize::MAX && block_id != usize::MAX - 1 {
            let (expanded, _, _) = self.block_lines.get(&block_id)?;
            if !*expanded {
                return None;
            }
        }
        let (lines, offset) = self.link_lines(block_id)?;
        let full = line_idx + offset;
        let text = lines.get(full)?.plain_text();
        // The selection endpoint after the final character is not a glyph.
        if col_idx >= line_terminal_column_count(&text) {
            return None;
        }
        let char_idx = char_index_at_terminal_column(&text, col_idx);
        let (first, last) = wrapped_run_bounds(&lines, full);
        find_links_in_run(&lines, first, last)
            .into_iter()
            .find(|link| {
                link.segments
                    .iter()
                    .any(|&(row, start, end)| row == full && char_idx >= start && char_idx < end)
            })
            .map(|link| LinkHint {
                block_id,
                url: link_target(&link.kind),
                occurrence: Some(LinkOccurrence {
                    line: link.start.0,
                    start_char: link.start.1,
                }),
            })
    }

    /// Terminal-column range of `hint` on one painted line, if the link
    /// (or the part of it that wrapped onto this row) is on that line.
    fn link_range_on_line(
        &self,
        hint: &LinkHint,
        block_id: usize,
        line_idx: usize,
        line: &ColoredLine,
    ) -> Option<(usize, usize)> {
        if hint.block_id != block_id {
            return None;
        }
        let Some(occurrence) = hint.occurrence else {
            return self.find_url_range_in_colored_line(line, &hint.url);
        };
        let (lines, offset) = self.link_lines(block_id)?;
        let full = line_idx + offset;
        if occurrence.line > full {
            return None;
        }
        let (first, last) = wrapped_run_bounds(&lines, full);
        if occurrence.line < first || occurrence.line > last {
            return None;
        }
        let link = find_links_in_run(&lines, first, last)
            .into_iter()
            .find(|link| link.start == (occurrence.line, occurrence.start_char))?;
        let &(_, start_char, end_char) = link.segments.iter().find(|&&(row, _, _)| row == full)?;
        let text = line.plain_text();
        Some((
            terminal_column_at_char_index(&text, start_char),
            terminal_column_at_char_index(&text, end_char),
        ))
    }

    fn find_url_range_in_colored_line(
        &self,
        line: &ColoredLine,
        url: &str,
    ) -> Option<(usize, usize)> {
        let mut combined = String::new();
        for span in &line.spans {
            combined.push_str(&span.text);
        }

        for found in crate::terminal::render::block_output::find_terminal_links(&combined) {
            let target = match &found.kind {
                crate::terminal::render::block_output::TerminalLinkKind::Url(u) => u.as_str(),
                crate::terminal::render::block_output::TerminalLinkKind::FilePath {
                    raw, ..
                } => raw.as_str(),
            };
            if target == url {
                let start_char = combined[..found.start].chars().count();
                let end_char = start_char + combined[found.start..found.end].chars().count();
                return Some((
                    terminal_column_at_char_index(&combined, start_char),
                    terminal_column_at_char_index(&combined, end_char),
                ));
            }
        }
        None
    }

    pub(crate) fn get_url_ranges_for_line(
        &self,
        block_id: usize,
        line_idx: usize,
        line: &ColoredLine,
    ) -> (Option<(usize, usize)>, Option<(usize, usize)>) {
        let hovered = self
            .hovered_link
            .as_ref()
            .and_then(|hint| self.link_range_on_line(hint, block_id, line_idx, line));
        let selected = self
            .selected_link
            .as_ref()
            .and_then(|hint| self.link_range_on_line(hint, block_id, line_idx, line));
        (hovered, selected)
    }

    pub(crate) fn point_for_position(
        &self,
        block_id: usize,
        line_idx: usize,
        col_idx: usize,
    ) -> Option<Vector2F> {
        let origin = self.origin?.xy();
        let rel = self.point_for_position_relative_to_child_origin(block_id, line_idx, col_idx)?;
        Some(origin + rel)
    }

    pub(crate) fn point_for_position_relative_to_child_origin(
        &self,
        block_id: usize,
        line_idx: usize,
        col_idx: usize,
    ) -> Option<Vector2F> {
        let align_offset = self.align_offset_px();
        let cell_w = self.cell_metrics.width;
        let cell_h = self.cell_metrics.height;
        let mut y = align_offset;

        for entry in &self.visible_entries {
            let (body_top, pad_left, matches) = match entry.kind {
                VisibleEntryKind::Block { block_id: bid } if bid == block_id => {
                    let header_h = self.actual_header_height(block_id);
                    if self.dir_blocks.contains_key(&block_id) {
                        if let Some(layout) = self.dir_layouts.get(&block_id) {
                            if let Some(region) = layout
                                .entry_regions
                                .iter()
                                .find(|r| r.entry_index == line_idx)
                            {
                                let rel_y = y + header_h + region.name_rect.center().y();
                                let name_x =
                                    DIR_BODY_PAD_LEFT + dir_entry_name_origin_in_body(region).x();
                                let char_w = dir_entry_char_width();
                                let rel_x = name_x + col_idx as f32 * char_w + char_w * 0.5;
                                return Some(vec2f(rel_x, rel_y));
                            }
                        }
                    }
                    (header_h + 20.0, 14.0, true)
                }
                VisibleEntryKind::Block { block_id: bid } => {
                    (self.actual_header_height(bid) + 20.0, 14.0, bid == block_id)
                }
                VisibleEntryKind::Bootstrap => (8.0, TERMINAL_PADDING_LEFT, block_id == usize::MAX),
                VisibleEntryKind::LivePrompt => {
                    (8.0, TERMINAL_PADDING_LEFT, block_id == usize::MAX - 1)
                }
                _ => (0.0, 0.0, false),
            };

            if matches {
                if block_id != usize::MAX && block_id != usize::MAX - 1 {
                    if let Some((_, lines, _)) = self.block_lines.get(&block_id) {
                        let wrap_cols = self.block_wrap_cols();
                        let mut row = self
                            .row_offsets
                            .get(&block_id)
                            .and_then(|offsets| offsets.get(line_idx))
                            .copied()
                            .unwrap_or(0);
                        let line_cols = lines.get(line_idx).map(Self::line_columns).unwrap_or(0);
                        let clamped_col = col_idx.min(line_cols);
                        let chunk = clamped_col / wrap_cols.max(1);
                        row += chunk;
                        let local_col = clamped_col.saturating_sub(chunk * wrap_cols.max(1));
                        let rel_y = y + body_top + row as f32 * cell_h + cell_h / 2.0;
                        let rel_x = terminal_column_center_x(pad_left, local_col, cell_w);
                        return Some(vec2f(rel_x, rel_y));
                    }
                }
                let rel_y = y + body_top + (line_idx as f32 * cell_h) + (cell_h / 2.0);
                let rel_x = terminal_column_center_x(pad_left, col_idx, cell_w);
                return Some(vec2f(rel_x, rel_y));
            }
            y += entry.height_px;
        }
        None
    }

    pub(crate) fn find_url_position(&self, hint: &LinkHint) -> Option<(usize, usize, usize)> {
        let block_id = hint.block_id;
        let url = hint.url.as_str();
        if let Some(occurrence) = hint.occurrence {
            let (_, offset) = self.link_lines(block_id)?;
            let paint_line = occurrence.line.checked_sub(offset)?;
            return Some((block_id, paint_line, occurrence.start_char));
        }
        if block_id == usize::MAX {
            for (line_idx, line) in self.bootstrap_lines.iter().enumerate() {
                if let Some((start_char, _)) = self.find_url_range_in_colored_line(line, url) {
                    return Some((block_id, line_idx, start_char));
                }
            }
        } else if block_id == usize::MAX - 1 {
            let prompt = self.live_prompt.as_ref()?;
            for (line_idx, line) in prompt.lines.iter().enumerate() {
                let colored = styled_to_colored(line, &self.theme);
                if let Some((start_char, _)) = self.find_url_range_in_colored_line(&colored, url) {
                    return Some((block_id, line_idx, start_char));
                }
            }
        } else {
            let (_, text_lines, _) = self.block_lines.get(&block_id)?;
            let all_lines = self.all_block_lines.get(&block_id)?;
            let offset = all_lines.len().saturating_sub(text_lines.len());
            for (full_idx, line) in all_lines.iter().enumerate() {
                if let Some((start_char, _)) = self.find_url_range_in_colored_line(line, url) {
                    let paint_line_idx = full_idx.saturating_sub(offset);
                    return Some((block_id, paint_line_idx, start_char));
                }
            }
            return None;
        }
        None
    }
}
