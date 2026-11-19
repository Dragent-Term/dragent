//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::render::block_list_element::BlockListElement;
use crate::terminal::render::block_list_geom::body_wrap_cols;
use crate::terminal::render::block_list_geom::count_wrapped_rows;
use crate::terminal::render::block_list_geom::slice_colored_line_cols;
use crate::terminal::render::block_list_header::is_error_status;
use crate::terminal::view::actions::{DirHover, EntryHint, LinkHint, TerminalAction};
use crate::workspace::InputEditor;
use dragent_terminal::StyledLine;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, ParentElement, Point, SavePosition, ScrollData,
    ScrollableElement, SelectableElement, Selection, SelectionFragment, SmartSelectFn, Text,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::string_offset::{ByteOffset, CharOffset};
use warpui::text::{IsRect, SelectionDirection, SelectionType};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::Pixels;
use warpui::ViewHandle;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};
use warpui_core::scene::ZIndex;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text::word_boundaries::WordBoundariesPolicy;
use warpui_core::text::TextBuffer;
use warpui_core::text_layout::ClipConfig;

use crate::console::block_filter::{count_block_matches, BlockFilterState};
use crate::console::dir_listing::{entry_click_hint_label, DirSort, DirView};
use crate::console::escape_parse::{styled_to_colored, ColoredLine, TextSpan};
use crate::icons::{render_icon, AppIcon};
use crate::terminal::model::block::{live_primary_line_count, Block, BlockStatus};
use crate::terminal::model::block_height_tree::{
    BlockHeightIndex, BlockListEntry, BLOCK_HEADER_HEIGHT_PX, COMPLETED_BLOCK_MAX_LINES,
};
use crate::terminal::model::is_signal_exit;
use crate::terminal::render::block_card_element::header_command_line;
use crate::terminal::render::dir_listing_paint::{
    dir_entry_char_width, dir_entry_name_origin_in_body, dir_listing_body_height,
    dir_listing_selection_lines, layout_dir_listing, paint_dir_listing, sort_dir_entries,
    DirBlockState, DirListingHit, DirListingLayout, DirListingPaintParams, DIR_BODY_PAD_LEFT,
};
use crate::terminal::render::grid_metrics::{
    line_terminal_column_count, slice_line_terminal_columns, terminal_column_at_char_index,
    terminal_column_center_x, x_to_terminal_column, CellMetrics,
};
use crate::terminal::render::grid_renderer::{build_alt_screen_column, GridLineParams};
use crate::terminal::render::BlockRenderContext;
use crate::terminal::render::CellGlyphCache;
use crate::terminal::viewport::{ScrollPosition, VisibleEntry, VisibleEntryKind};
use crate::theme::{mono_font_size, Theme, TERMINAL_PADDING_LEFT};

impl Element for BlockListElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let width = constraint.max.x().max(0.0);
        let wrap_cols = body_wrap_cols(width, self.cell_metrics.width);
        {
            let mut cache = self.text_cache.lock().unwrap();
            self.row_offsets = self
                .block_lines
                .iter()
                .map(|(id, (_, lines, _))| (*id, cache.row_offsets(*id, lines, wrap_cols)))
                .collect();
        }
        let content_h = self.height_index.total_height_px();
        let height = if content_h < self.viewport_height {
            self.viewport_height
        } else {
            content_h
        };
        let size = warpui::geometry::vector::vec2f(width, height);
        self.viewport_size = Some(size);

        let content_width = crate::console::dir_listing::dir_grid_content_width(width);
        self.recompute_dir_layouts(content_width);

        let child_constraint = SizeConstraint {
            min: warpui::geometry::vector::vec2f(size.x(), 0.0),
            max: warpui::geometry::vector::vec2f(size.x(), f32::INFINITY),
        };

        for header in self.headers.values_mut() {
            header.layout(child_constraint, ctx, app);
        }

        for el in self.agent_block_elements.values_mut() {
            el.layout(child_constraint, ctx, app);
        }

        for el in self.fix_card_elements.values_mut() {
            el.layout(child_constraint, ctx, app);
        }

        if let Some(tooltip) = &mut self.tooltip {
            let tooltip_constraint = SizeConstraint {
                min: Vector2F::zero(),
                max: vec2f(size.x(), f32::INFINITY),
            };
            tooltip.layout(tooltip_constraint, ctx, app);
        }

        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        for header in self.headers.values_mut() {
            header.after_layout(ctx, app);
        }
        for el in self.agent_block_elements.values_mut() {
            el.after_layout(ctx, app);
        }
        for el in self.fix_card_elements.values_mut() {
            el.after_layout(ctx, app);
        }
        if let Some(tooltip) = &mut self.tooltip {
            tooltip.after_layout(ctx, app);
        }

        // ── Measured-height feedback ───────────────────────────────────────────
        // For each visible block, compute the actual rendered pixel height:
        //   • header_h  = laid-out header element height (may exceed BLOCK_HEADER_HEIGHT_PX
        //                 when a long command wraps to a second line)
        //   • dir body  = dir_listing element height after layout (varies with content width)
        //   • text body = 20.0 (top pad) + line_count * cell_h (grid-painted, deterministic)
        // Changed values are written to the shared sink so that TerminalView can update
        // its height index on the next rebuild_height_index call.
        let cell_h = self.cell_metrics.height;
        let mut changed_heights: Vec<(usize, f32)> = Vec::new();
        let mut changed_agent_heights: Vec<(usize, f32)> = Vec::new();

        // Change detection MUST compare against the persistent shared sink, never a
        // per-element field: BlockListElement is rebuilt every render, so a local
        // "last dispatched" map is always empty — every block then looks changed,
        // firing heights_tx every frame → an endless rebuild→render→rebuild loop
        // (the UI shake). Snapshotting the sink here makes the guard actually work,
        // so heights_tx fires only when a height truly changes.
        let prev_block_heights = self.measured_sink.lock().unwrap().clone();
        let prev_agent_heights = self.measured_agent_sink.lock().unwrap().clone();

        for entry in &self.visible_entries {
            match entry.kind {
                VisibleEntryKind::Block { block_id } => {
                    let header_h = self
                        .headers
                        .get(&block_id)
                        .and_then(|h| h.size())
                        .map(|s| s.y())
                        .unwrap_or(BLOCK_HEADER_HEIGHT_PX);

                    let body_h = if self.dir_blocks.contains_key(&block_id) {
                        self.dir_layouts
                            .get(&block_id)
                            .map(|l| l.body_height)
                            .unwrap_or_else(|| {
                                dir_listing_body_height(
                                    &self.dir_blocks[&block_id].entries,
                                    self.dir_view,
                                    self.content_width_px,
                                )
                            })
                    } else if let Some((expanded, lines, _)) = self.block_lines.get(&block_id) {
                        if *expanded && !lines.is_empty() {
                            let vw = self.viewport_size.map(|s| s.x()).unwrap_or(0.0);
                            let wrap_cols = body_wrap_cols(vw, self.cell_metrics.width);
                            let wrapped = self.row_offsets[&block_id].last().copied().unwrap_or(0);
                            if std::env::var_os("DRAGENT_LOG_HEIGHT").is_some() {
                                // The gap between logical_lines and wrapped_rows is the
                                // index/paint disagreement that drives the scroll shake.
                                log::info!(
                                    "[height] measure block={} logical_lines={} wrapped_rows={} wrap_cols={} header_h={:.1} body_h={:.1}",
                                    block_id, lines.len(), wrapped, wrap_cols, header_h,
                                    20.0 + wrapped as f32 * cell_h,
                                );
                            }
                            20.0 + wrapped as f32 * cell_h
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };

                    let fix_card_h = self
                        .fix_card_elements
                        .get(&block_id)
                        .and_then(|el| el.size())
                        .map(|s| s.y())
                        .unwrap_or(0.0);
                    let total_h = header_h + body_h + fix_card_h;
                    let prev = prev_block_heights.get(&block_id).copied().unwrap_or(-1.0);
                    if (total_h - prev).abs() > 0.5 {
                        changed_heights.push((block_id, total_h));
                    }
                }
                VisibleEntryKind::AgentBlock { agent_block_id } => {
                    if let Some(el) = self.agent_block_elements.get(&agent_block_id) {
                        if let Some(size) = el.size() {
                            let total_h = size.y();
                            let prev = prev_agent_heights
                                .get(&agent_block_id)
                                .copied()
                                .unwrap_or(-1.0);
                            if (total_h - prev).abs() > 0.5 {
                                changed_agent_heights.push((agent_block_id, total_h));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if !changed_heights.is_empty() {
            // Write into the shared sink; TerminalView drains this in rebuild_height_index.
            let mut sink = self.measured_sink.lock().unwrap();
            for &(id, h) in &changed_heights {
                sink.insert(id, h);
            }
        }

        if !changed_agent_heights.is_empty() {
            let mut sink = self.measured_agent_sink.lock().unwrap();
            for &(id, h) in &changed_agent_heights {
                sink.insert(id, h);
            }
        }

        // Patch this frame's own `visible_entries` in place instead of only
        // writing to the sink and waiting for heights_tx -> TerminalView::
        // rebuild_height_index -> next render() to pick it up a tick later.
        // layout(), after_layout(), and paint() all run on this SAME element
        // instance within one frame (see Presenter::build_scene), so without
        // this, `paint()` below positions every entry after a block whose
        // height just changed using the stale pre-layout estimate while that
        // block itself paints at its true, just-measured size — for a small
        // text block a one-frame-late correction is invisible, but for a
        // Mermaid diagram (which can jump from a text-line estimate to
        // hundreds of pixels the instant it finishes rendering) it's a
        // glaring visual overlap, and looks permanent if nothing happens to
        // trigger another repaint right after.
        if !changed_heights.is_empty() || !changed_agent_heights.is_empty() {
            let mut delta = 0.0f32;
            for entry in self.visible_entries.iter_mut() {
                entry.y_offset_in_viewport += delta;
                let new_h = match entry.kind {
                    VisibleEntryKind::Block { block_id } => changed_heights
                        .iter()
                        .find(|&&(id, _)| id == block_id)
                        .map(|&(_, h)| h),
                    VisibleEntryKind::AgentBlock { agent_block_id } => changed_agent_heights
                        .iter()
                        .find(|&&(id, _)| id == agent_block_id)
                        .map(|&(_, h)| h),
                    _ => None,
                };
                if let Some(h) = new_h {
                    delta += h - entry.height_px;
                    entry.height_px = h;
                }
            }
        }

        if !changed_heights.is_empty() || !changed_agent_heights.is_empty() {
            let _ = self.heights_tx.try_send(());
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let mut glyph_cache = self.glyph_cache.lock().unwrap();
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let total_h = self.height_index.total_height_px();
        let viewport_h = self.viewport_height;
        let align_offset = if total_h < viewport_h {
            viewport_h - total_h
        } else {
            0.0
        };
        let mut y = align_offset;

        let selection_data = if let Some(sel) = ctx.current_selection {
            if let (Some(start), Some(end)) =
                (self.resolve_point(sel.start), self.resolve_point(sel.end))
            {
                let (n_start, n_end) = self.order_selection_endpoints(start, end);
                Some((n_start, n_end))
            } else {
                None
            }
        } else {
            None
        };

        for entry in &self.visible_entries {
            match &entry.kind {
                VisibleEntryKind::Bootstrap => {
                    let mut line_y = y + 8.0;
                    for (line_idx, line) in self.bootstrap_lines.iter().enumerate() {
                        let line_origin = origin + vec2f(TERMINAL_PADDING_LEFT, line_y);

                        let sel_range = selection_data.and_then(|(n_start, n_end)| {
                            let (n_start_y, n_end_y) = self.selection_y_range(n_start, n_end);
                            let line_text: String =
                                line.spans.iter().map(|s| s.text.as_str()).collect();
                            let line_text_len = line_terminal_column_count(&line_text);
                            self.selection_cols_for_paint_line(
                                (n_start.0, n_start.2, n_end.2, n_start_y, n_end_y),
                                usize::MAX,
                                line_idx,
                                line_text_len,
                            )
                        });

                        let (hovered_range, selected_range) =
                            self.get_url_ranges_for_line(usize::MAX, line_idx, &line);

                        paint_grid_line(
                            ctx,
                            app,
                            line,
                            line_origin,
                            self.mono,
                            self.theme,
                            self.cell_metrics.width,
                            self.cell_metrics.height,
                            sel_range,
                            None,
                            hovered_range,
                            selected_range,
                            &[],
                            &mut *glyph_cache,
                        );
                        line_y += self.cell_metrics.height;
                    }
                }
                VisibleEntryKind::Block { block_id } => {
                    if self
                        .block_status
                        .get(block_id)
                        .is_some_and(|status| is_error_status(*status))
                    {
                        let vw = self.viewport_size.map(|s| s.x()).unwrap_or(0.0);
                        ctx.scene
                            .draw_rect_without_hit_recording(RectF::new(
                                origin + vec2f(0.0, y),
                                vec2f(vw, entry.height_px),
                            ))
                            .with_background(self.theme.tinted_surface(self.theme.ui_red()));
                    }

                    if let Some(header) = self.headers.get_mut(block_id) {
                        header.paint(origin + vec2f(0.0, y), ctx, app);
                    }

                    if let Some((expanded, text_lines, _)) = self.block_lines.get(block_id) {
                        if *expanded {
                            let block_body_origin =
                                origin + vec2f(0.0, y + self.actual_header_height(*block_id));
                            if let Some(state) = self.dir_blocks.get(block_id) {
                                if let Some(layout) = self.dir_layouts.get(block_id) {
                                    let (hover_block, hover_entry) =
                                        self.dir_hovered_entry.unwrap_or((usize::MAX, 0));
                                    let hovered_entry =
                                        (hover_block == *block_id).then_some(hover_entry);
                                    let paint_params = DirListingPaintParams {
                                        block_id: *block_id,
                                        cwd: &state.cwd,
                                        entries: &state.entries,
                                        layout,
                                        theme: &self.theme,
                                        mono: self.mono,
                                        dir_sort: self.dir_sort,
                                        dir_view: self.dir_view,
                                        entry_hint: self.entry_hint.as_ref(),
                                        hovered_entry,
                                        hovered_sort: self.dir_hovered_sort,
                                        hovered_view: self.dir_hovered_view,
                                    };
                                    paint_dir_listing(
                                        block_body_origin,
                                        &paint_params,
                                        ctx,
                                        app,
                                        &mut *glyph_cache,
                                    );

                                    if let Some((n_start, n_end)) = selection_data {
                                        if n_start.0 == *block_id && n_end.0 == *block_id {
                                            let body_x =
                                                block_body_origin + vec2f(DIR_BODY_PAD_LEFT, 0.0);
                                            for region in &layout.entry_regions {
                                                let line_text: String = text_lines
                                                    .get(region.entry_index)
                                                    .map(|line| {
                                                        line.spans
                                                            .iter()
                                                            .map(|s| s.text.as_str())
                                                            .collect()
                                                    })
                                                    .unwrap_or_default();
                                                let line_len =
                                                    line_terminal_column_count(&line_text);
                                                if let Some((start_col, end_col)) =
                                                    Self::selection_cols_for_dir_line(
                                                        n_start.1,
                                                        n_start.2,
                                                        n_end.1,
                                                        n_end.2,
                                                        region.entry_index,
                                                        line_len,
                                                    )
                                                {
                                                    self.paint_dir_entry_selection(
                                                        ctx, body_x, region, start_col, end_col,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                let mut line_y = y + self.actual_header_height(*block_id) + 20.0;
                                let cursor =
                                    self.block_lines.get(block_id).and_then(|(_, _, c)| *c);
                                let vw = self.viewport_size.map(|s| s.x()).unwrap_or(0.0);
                                let wrap_cols = body_wrap_cols(vw, self.cell_metrics.width);
                                let cell_h = self.cell_metrics.height;
                                let win_top = self.first_entry_offset_px - cell_h;
                                let win_bot =
                                    self.first_entry_offset_px + self.viewport_height + cell_h;
                                let offsets = &self.row_offsets[block_id];
                                let first_row =
                                    ((win_top - line_y).max(0.0) / cell_h).floor() as usize;
                                let first_line = offsets
                                    .partition_point(|row| *row <= first_row)
                                    .saturating_sub(1)
                                    .min(text_lines.len());
                                line_y += offsets[first_line] as f32 * cell_h;
                                for (line_idx, line) in
                                    text_lines.iter().enumerate().skip(first_line)
                                {
                                    if line_y > win_bot {
                                        break;
                                    }
                                    // Viewport culling: paint only lines inside the visible
                                    // window. Without this a tall block (e.g. a 1000-line `yes`
                                    // tail) repaints every line every frame though only ~45 fit
                                    // the viewport — the firehose lag. Height is derived cheaply
                                    // from span widths so off-screen lines skip all per-line work.
                                    let n_rows = offsets[line_idx + 1] - offsets[line_idx];
                                    let line_block_h = n_rows as f32 * cell_h;
                                    if line_y + line_block_h < win_top {
                                        line_y += line_block_h;
                                        continue;
                                    }

                                    let line_text: String =
                                        line.spans.iter().map(|s| s.text.as_str()).collect();
                                    let line_text_len = line_terminal_column_count(&line_text);

                                    let full_sel_range =
                                        selection_data.and_then(|(n_start, n_end)| {
                                            let (n_start_y, n_end_y) =
                                                self.selection_y_range(n_start, n_end);
                                            self.selection_cols_for_paint_line(
                                                (n_start.0, n_start.2, n_end.2, n_start_y, n_end_y),
                                                *block_id,
                                                line_idx,
                                                line_text_len,
                                            )
                                        });

                                    let logical_cursor_col =
                                        cursor.and_then(|(c_row, c_col, c_visible)| {
                                            if c_visible && c_row == line_idx {
                                                Some(c_col)
                                            } else {
                                                None
                                            }
                                        });

                                    let (full_hovered, full_selected) =
                                        self.get_url_ranges_for_line(*block_id, line_idx, line);

                                    let full_matches = self.filter_match_columns(*block_id, line);

                                    // Paint wrapped chunks of this logical line.
                                    let num_chunks = if line_text_len <= wrap_cols {
                                        1
                                    } else {
                                        (line_text_len + wrap_cols - 1) / wrap_cols
                                    };

                                    for chunk_idx in 0..num_chunks {
                                        let col_start = chunk_idx * wrap_cols;
                                        let col_end = (col_start + wrap_cols).min(line_text_len);

                                        let chunk_line = if num_chunks == 1 {
                                            // Avoid extra work for the common single-chunk case.
                                            std::borrow::Cow::Borrowed(line)
                                        } else {
                                            std::borrow::Cow::Owned(slice_colored_line_cols(
                                                line, col_start, col_end,
                                            ))
                                        };

                                        let chunk_sel = full_sel_range.and_then(|(a, b)| {
                                            let os = a.max(col_start);
                                            let oe = b.min(col_end);
                                            if os < oe {
                                                Some((
                                                    os.saturating_sub(col_start),
                                                    oe.saturating_sub(col_start),
                                                ))
                                            } else {
                                                None
                                            }
                                        });

                                        let chunk_cursor = logical_cursor_col.and_then(|c| {
                                            if c >= col_start
                                                && (c < col_end
                                                    || (c == col_end && col_end == line_text_len))
                                            {
                                                Some(c - col_start)
                                            } else {
                                                None
                                            }
                                        });

                                        let chunk_hovered = full_hovered.and_then(|(a, b)| {
                                            let os = a.max(col_start);
                                            let oe = b.min(col_end);
                                            if os < oe {
                                                Some((
                                                    os.saturating_sub(col_start),
                                                    oe.saturating_sub(col_start),
                                                ))
                                            } else {
                                                None
                                            }
                                        });

                                        let chunk_selected = full_selected.and_then(|(a, b)| {
                                            let os = a.max(col_start);
                                            let oe = b.min(col_end);
                                            if os < oe {
                                                Some((
                                                    os.saturating_sub(col_start),
                                                    oe.saturating_sub(col_start),
                                                ))
                                            } else {
                                                None
                                            }
                                        });

                                        let chunk_matches: Vec<(usize, usize)> = full_matches
                                            .iter()
                                            .filter_map(|&(a, b)| {
                                                let os = a.max(col_start);
                                                let oe = b.min(col_end);
                                                (os < oe).then_some((
                                                    os.saturating_sub(col_start),
                                                    oe.saturating_sub(col_start),
                                                ))
                                            })
                                            .collect();

                                        paint_grid_line(
                                            ctx,
                                            app,
                                            &*chunk_line,
                                            origin + vec2f(14.0, line_y),
                                            self.mono,
                                            self.theme,
                                            self.cell_metrics.width,
                                            self.cell_metrics.height,
                                            chunk_sel,
                                            chunk_cursor,
                                            chunk_hovered,
                                            chunk_selected,
                                            &chunk_matches,
                                            &mut *glyph_cache,
                                        );
                                        line_y += self.cell_metrics.height;
                                    }
                                }
                            }
                        }
                    }

                    // Paint fix card below block body (after text output).
                    let fix_card_header_h = self.actual_header_height(*block_id);
                    if let Some(fix_card) = self.fix_card_elements.get_mut(block_id) {
                        let body_text_h =
                            if let Some((expanded, lines, _)) = self.block_lines.get(block_id) {
                                if *expanded && !lines.is_empty() {
                                    let vw = self.viewport_size.map(|s| s.x()).unwrap_or(0.0);
                                    let wrap_cols = body_wrap_cols(vw, self.cell_metrics.width);
                                    20.0 + count_wrapped_rows(lines, wrap_cols) as f32
                                        * self.cell_metrics.height
                                } else {
                                    0.0
                                }
                            } else {
                                0.0
                            };
                        fix_card.paint(
                            origin + vec2f(0.0, y + fix_card_header_h + body_text_h),
                            ctx,
                            app,
                        );
                    }
                }
                VisibleEntryKind::AgentBlock { agent_block_id } => {
                    if let Some(el) = self.agent_block_elements.get_mut(agent_block_id) {
                        el.paint(origin + vec2f(0.0, y), ctx, app);
                    }

                    let sel_id = usize::MAX - 1000 - agent_block_id;
                    if let Some((n_start, n_end)) = selection_data {
                        if let Some((_, text_lines, _)) = self.block_lines.get(&sel_id) {
                            let (n_start_y, n_end_y) = self.selection_y_range(n_start, n_end);
                            let body_y_start = y + BLOCK_HEADER_HEIGHT_PX;
                            for (line_idx, line) in text_lines.iter().enumerate() {
                                let line_text: String =
                                    line.spans.iter().map(|s| s.text.as_str()).collect();
                                let line_text_len = line_terminal_column_count(&line_text);
                                if let Some((start_col, end_col)) = self
                                    .selection_cols_for_paint_line(
                                        (n_start.0, n_start.2, n_end.2, n_start_y, n_end_y),
                                        sel_id,
                                        line_idx,
                                        line_text_len,
                                    )
                                {
                                    let line_y = body_y_start
                                        + 8.0
                                        + (line_idx as f32 * self.cell_metrics.height);
                                    let start_x =
                                        16.0 + (start_col as f32 * self.cell_metrics.width);
                                    let end_x = 16.0 + (end_col as f32 * self.cell_metrics.width);
                                    let rect = RectF::new(
                                        origin + vec2f(start_x, line_y),
                                        vec2f(end_x - start_x, self.cell_metrics.height),
                                    );
                                    let selection_bg = ColorU::new(25, 90, 120, 180);
                                    ctx.scene
                                        .draw_rect_without_hit_recording(rect)
                                        .with_background(selection_bg);
                                }
                            }
                        }
                    }
                }
                VisibleEntryKind::ViewportClearGap | VisibleEntryKind::BlockSpacing => {}
                VisibleEntryKind::LivePrompt => {
                    if let Some(prompt) = &self.live_prompt {
                        let mut line_y = y + 8.0;
                        for (line_idx, line) in prompt.lines.iter().enumerate() {
                            let colored = styled_to_colored(line, &self.theme);
                            let line_origin = origin + vec2f(TERMINAL_PADDING_LEFT, line_y);

                            let sel_range = selection_data.and_then(|(n_start, n_end)| {
                                let (n_start_y, n_end_y) = self.selection_y_range(n_start, n_end);
                                let line_text: String =
                                    colored.spans.iter().map(|s| s.text.as_str()).collect();
                                let line_text_len = line_terminal_column_count(&line_text);
                                self.selection_cols_for_paint_line(
                                    (n_start.0, n_start.2, n_end.2, n_start_y, n_end_y),
                                    usize::MAX - 1,
                                    line_idx,
                                    line_text_len,
                                )
                            });

                            let cursor_col =
                                if prompt.cursor_visible && prompt.cursor_row == line_idx {
                                    Some(prompt.cursor_col)
                                } else {
                                    None
                                };

                            let (hovered_range, selected_range) =
                                self.get_url_ranges_for_line(usize::MAX - 1, line_idx, &colored);

                            paint_grid_line(
                                ctx,
                                app,
                                &colored,
                                line_origin,
                                self.mono,
                                self.theme,
                                self.cell_metrics.width,
                                self.cell_metrics.height,
                                sel_range,
                                cursor_col,
                                hovered_range,
                                selected_range,
                                &[],
                                &mut *glyph_cache,
                            );
                            line_y += self.cell_metrics.height;
                        }
                    }
                }
            }
            y += entry.height_px;
        }

        if let Some(ref hint) = self.selected_link {
            if let Some((block_id, line_idx, start_char)) = self.find_url_position(hint) {
                let url_char_count = hint.url.chars().count();
                if let (Some(start_rel), Some(end_rel)) = (
                    self.point_for_position_relative_to_child_origin(
                        block_id, line_idx, start_char,
                    ),
                    self.point_for_position_relative_to_child_origin(
                        block_id,
                        line_idx,
                        start_char + url_char_count,
                    ),
                ) {
                    let url_middle_rel = vec2f((start_rel.x() + end_rel.x()) / 2.0, start_rel.y());
                    let cell_h = self.cell_metrics.height;
                    let cell_top_y = url_middle_rel.y() - (cell_h / 2.0) - 6.0;

                    if let Some(tooltip) = &mut self.tooltip {
                        let tooltip_size = tooltip.size().unwrap_or_default();
                        let tx = url_middle_rel.x() - (tooltip_size.x() / 2.0);
                        let ty = cell_top_y - tooltip_size.y();
                        ctx.scene.start_layer(ClipBounds::None);
                        tooltip.paint(origin + vec2f(tx, ty), ctx, app);
                        ctx.scene.stop_layer();
                    }
                }
            }
        } else if let Some(ref hint) = self.entry_hint {
            if let (Some(body_y), Some(layout), Some(state)) = (
                self.block_body_origin_y(hint.block_id),
                self.dir_layouts.get(&hint.block_id),
                self.dir_blocks.get(&hint.block_id),
            ) {
                let region = layout.entry_regions.iter().find(|region| {
                    state
                        .entries
                        .get(region.entry_index)
                        .is_some_and(|entry| entry.name == hint.name)
                });
                if let Some(region) = region {
                    let center = region.name_rect.center();
                    let anchor = vec2f(
                        DIR_BODY_PAD_LEFT + center.x(),
                        body_y + center.y() - region.name_rect.size().y() * 0.5 - 6.0,
                    );
                    if let Some(tooltip) = &mut self.tooltip {
                        let tooltip_size = tooltip.size().unwrap_or_default();
                        let tx = anchor.x() - (tooltip_size.x() / 2.0);
                        let ty = anchor.y() - tooltip_size.y();
                        ctx.scene.start_layer(ClipBounds::None);
                        tooltip.paint(origin + vec2f(tx, ty), ctx, app);
                        ctx.scene.stop_layer();
                    }
                }
            }
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let z_index = self
            .origin
            .map(|o| o.z_index())
            .unwrap_or(ZIndex::Normal(0));

        if let Some(tooltip) = &mut self.tooltip {
            if tooltip.dispatch_event(event, ctx, app) {
                return true;
            }
        }

        match event.raw_event() {
            warpui::event::Event::MouseMoved { position, .. } => {
                let position_ref = *position;
                let in_view = self.is_in_visible_viewport(position_ref);
                if in_view && !self.is_in_fix_card_area(position_ref) {
                    if let Some(hint) = self.find_url_at_point(position_ref) {
                        // Hover must be recorded in BOTH places: the view's
                        // field (dispatch) survives a rebuild, and this
                        // element's copy is what paints — and what the next
                        // move compares against — when the element is NOT
                        // rebuilt. Updating only one lets them drift, so
                        // leaving and returning to a link looks like "no
                        // change" and the underline never comes back.
                        if self.hovered_link.as_ref() != Some(&hint) {
                            self.hovered_link = Some(hint.clone());
                            ctx.dispatch_typed_action(TerminalAction::HoverLink { hint });
                            ctx.notify();
                        }
                        ctx.set_cursor(Cursor::PointingHand, z_index);
                        return true;
                    }
                }
                if self.hovered_link.take().is_some() {
                    ctx.dispatch_typed_action(TerminalAction::ClearHoverLink);
                    ctx.reset_cursor();
                    ctx.notify();
                }

                // Same two-place bookkeeping as link hover above.
                let current = DirHover {
                    entry: self.dir_hovered_entry,
                    sort: self.dir_hovered_sort,
                    view: self.dir_hovered_view,
                };
                if let Some((block_id, hit)) =
                    self.dir_hit_at_point(position_ref).filter(|_| in_view)
                {
                    let new_hover = match hit {
                        DirListingHit::Entry { entry_index, .. } => DirHover {
                            entry: Some((block_id, entry_index)),
                            ..DirHover::default()
                        },
                        DirListingHit::Sort(sort) => DirHover {
                            sort: Some(sort),
                            ..DirHover::default()
                        },
                        DirListingHit::View(view) => DirHover {
                            view: Some(view),
                            ..DirHover::default()
                        },
                    };
                    if new_hover != current {
                        self.set_local_dir_hover(new_hover);
                        ctx.dispatch_typed_action(TerminalAction::SetDirHover(new_hover));
                        ctx.notify();
                    }
                    ctx.set_cursor(Cursor::PointingHand, z_index);
                    return true;
                } else if current != DirHover::default() {
                    self.set_local_dir_hover(DirHover::default());
                    ctx.dispatch_typed_action(TerminalAction::SetDirHover(DirHover::default()));
                    ctx.reset_cursor();
                    ctx.notify();
                }
            }
            warpui::event::Event::LeftMouseDown {
                position,
                modifiers,
                ..
            } => {
                let position_ref = *position;
                let in_view = self.is_in_visible_viewport(position_ref);
                if in_view && !self.is_in_fix_card_area(position_ref) {
                    if let Some(hint) = self.find_url_at_point(position_ref) {
                        let is_cmd = modifiers.cmd || modifiers.ctrl;
                        if is_cmd {
                            ctx.dispatch_typed_action(TerminalAction::OpenLink {
                                block_id: hint.block_id,
                                url: hint.url,
                            });
                        } else {
                            ctx.dispatch_typed_action(TerminalAction::SelectLink {
                                block_id: hint.block_id,
                                url: hint.url,
                                occurrence: hint.occurrence,
                                absolute_position: position_ref,
                            });
                        }
                        return true;
                    }
                }
                if self.selected_link.is_some() {
                    ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
                }

                if let Some((block_id, hit)) =
                    self.dir_hit_at_point(position_ref).filter(|_| in_view)
                {
                    match hit {
                        DirListingHit::Entry {
                            entry_index,
                            is_dir,
                            ..
                        } => {
                            let name = self
                                .dir_blocks
                                .get(&block_id)
                                .and_then(|s| s.entries.get(entry_index))
                                .map(|e| e.name.clone());
                            if let Some(name) = name {
                                if modifiers.cmd || modifiers.ctrl {
                                    ctx.dispatch_typed_action(TerminalAction::OpenDirEntry {
                                        block_id,
                                        name,
                                        is_dir,
                                    });
                                } else {
                                    ctx.dispatch_typed_action(TerminalAction::SelectDirEntry {
                                        block_id,
                                        name,
                                        is_dir,
                                    });
                                }
                            }
                            return true;
                        }
                        DirListingHit::Sort(sort) => {
                            ctx.dispatch_typed_action(TerminalAction::SetDirSort(sort));
                            return true;
                        }
                        DirListingHit::View(view) => {
                            ctx.dispatch_typed_action(TerminalAction::SetDirView(view));
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }

        for header in self.headers.values_mut() {
            if header.dispatch_event(event, ctx, app) {
                return true;
            }
        }
        for el in self.fix_card_elements.values_mut() {
            if el.dispatch_event(event, ctx, app) {
                return true;
            }
        }
        for el in self.agent_block_elements.values_mut() {
            if el.dispatch_event(event, ctx, app) {
                return true;
            }
        }
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.viewport_size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn as_selectable_element(&self) -> Option<&dyn SelectableElement> {
        Some(self)
    }
}
