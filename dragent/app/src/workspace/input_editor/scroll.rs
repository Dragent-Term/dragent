//! Visual lines, scrolling, and viewport math.
//!
//! Blocks here are impl InputEditor: same type, same behavior.

use super::model::*;
use super::text::*;
use crate::console::command_intel::{diagnostic_cols_on_line, diagnostics_for};
use crate::console::input_bar::{
    build_highlighted_command, is_likely_command, style_runs_for_command,
};
use crate::console::input_editing::{
    line_end, line_start, matching_close_bracket, word_end, word_start,
};
use crate::console::input_layout::{
    caret_x_for_char_index, caret_x_in_layout, caret_x_plain, char_col_at_x,
    char_col_at_x_in_layout, char_index_at_click_plain, line_height_with_spacing,
    selection_cols_on_line, unwrapped_line_layout, visual_subline_layouts,
};
use crate::console::mention_popup::MentionEntry;
use crate::console::vim_mode::{VimContext, VimDispatch, VimEffect, VimMode, VimState};
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::editor_render::{
    self, EDITOR_BREAKPOINT_ZONE_WIDTH, EDITOR_CONTENT_PADDING_LEFT, EDITOR_FOLD_ZONE_WIDTH,
};
use dragent_document::{
    capabilities_for_text, DiffLineKind, DocumentCapabilities, DocumentDiff, GitDiffDisplayRow,
};
use dragent_editor::{
    Breakpoint, BreakpointRegistry, DiagnosticSeverity, FoldIndex, FoldParser, FoldRegion,
    FoldState, Language, LineChangeKind, ResolvedDiagnostic,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance, SingleAxisConfig,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, Element, Empty, EventHandler, Expanded,
    Fill, Flex, MainAxisAlignment, MainAxisSize, NewScrollable, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Point, SavePosition, ScrollbarWidth, Stack, Text,
};
use warpui::fonts::{Cache as FontCache, FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::keymap::Keystroke;
use warpui::r#async::{SpawnedFutureHandle, Timer};
use warpui::{
    AfterLayoutContext, AppContext, Entity, EventContext, FocusHandle, LayoutContext, PaintContext,
    SizeConstraint, TypedActionView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle, SELECTED_HIGHLIGHT_COLOR};
use warpui_core::event::DispatchedEvent;
use warpui_core::event::KeyState;
use warpui_core::event::ModifiersState;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::units::IntoPixels;

use super::InputEditor;

impl InputEditor {
    /// Compute all visual sub-lines for the buffer given an available pixel width.
    /// Returns `(visual_text, abs_char_offset)` pairs, one per visual row, top-to-bottom.
    pub(crate) fn all_visual_lines(
        &self,
        available_width: f32,
        font_cache: &FontCache,
    ) -> Vec<(String, usize)> {
        let lines = self.visual_line_map(available_width, font_cache);
        if self.source_path.is_some() {
            let fold_index = self.fold_index();
            return self
                .visible_visual_indices(&lines, &fold_index, available_width)
                .iter()
                .filter_map(|index| {
                    let line = lines.get(*index)?;
                    Some((line.text.clone(), line.abs_char_offset))
                })
                .collect();
        }
        lines
            .iter()
            .map(|line| (line.text.clone(), line.abs_char_offset))
            .collect()
    }

    pub(crate) fn visual_line_map(
        &self,
        available_width: f32,
        font_cache: &FontCache,
    ) -> Rc<Vec<VisualLine>> {
        // Source files follow the normal code-editor model: one physical source
        // line is one visual line. Normalising this key also prevents window
        // resizing from reshaping every line when word-wrap is disabled.
        let layout_width = if self.is_source_editor() {
            1_000_000.0
        } else {
            available_width
        };
        if let Some(cache) = self.visual_line_cache.borrow().as_ref() {
            if cache.revision == self.buffer_revision
                && (cache.available_width - layout_width).abs() < 0.5
            {
                return Rc::clone(&cache.lines);
            }
        }

        let explicit_lines: Vec<&str> = self.buffer.split('\n').collect();
        // Bound the content-addressed cache: once it holds roughly twice as
        // many entries as the document has lines (stale text from deleted
        // lines), reset it — one full reshape, then steady state again.
        if self.line_layout_cache.borrow().len() > explicit_lines.len() * 2 + 64 {
            self.line_layout_cache.borrow_mut().clear();
        }
        let mut result = Vec::new();
        let mut abs_offset = 0usize;
        let reshape_started = std::time::Instant::now();
        let mut reshaped_lines = 0usize;
        for (source_index, line_text) in explicit_lines.iter().enumerate() {
            let line_char_count = line_text.chars().count();
            let layouts = if self.is_source_editor() {
                // Incremental shaping: reuse the cached layout when the line's
                // text is unchanged, so a keystroke re-shapes only the edited
                // line instead of the entire document.
                let cached = self
                    .line_layout_cache
                    .borrow()
                    .get(*line_text)
                    .map(std::rc::Rc::clone);
                match cached {
                    Some(layout) => vec![(0, layout)],
                    None => {
                        reshaped_lines += 1;
                        let layout = std::rc::Rc::new(unwrapped_line_layout(
                            line_text,
                            self.ui_font,
                            self.font_size(),
                            Properties::default().weight(self.font_weight),
                            font_cache,
                        ));
                        self.line_layout_cache
                            .borrow_mut()
                            .insert((*line_text).to_string(), std::rc::Rc::clone(&layout));
                        vec![(0, layout)]
                    }
                }
            } else {
                let style_runs = style_runs_for_command(line_text, self.ui_font, &self.theme);
                visual_subline_layouts(
                    line_text,
                    layout_width,
                    self.ui_font,
                    self.font_size(),
                    &style_runs,
                    font_cache,
                )
                .into_iter()
                .map(|(start, layout)| (start, std::rc::Rc::new(layout)))
                .collect()
            };
            let starts: Vec<usize> = layouts.iter().map(|(start, _)| *start).collect();
            for (row, (char_start, layout)) in layouts.into_iter().enumerate() {
                let char_end = starts.get(row + 1).copied().unwrap_or(line_char_count);
                let text_slice: String = line_text
                    .chars()
                    .skip(char_start)
                    .take(char_end.saturating_sub(char_start))
                    .collect();
                result.push(VisualLine {
                    text: text_slice,
                    abs_char_offset: abs_offset + char_start,
                    source_line: source_index + 1,
                    continuation: row > 0,
                    layout,
                });
            }
            abs_offset += line_char_count + 1; // +1 for '\n'
        }
        let lines = Rc::new(result);
        let reshape_elapsed = reshape_started.elapsed();
        if reshape_elapsed > std::time::Duration::from_millis(50) {
            log::warn!(
                "[editor-perf] slow visual-line rebuild: {:?} lines={} reshaped={}",
                reshape_elapsed,
                lines.len(),
                reshaped_lines
            );
        }
        *self.visual_line_cache.borrow_mut() = Some(VisualLineCache {
            revision: self.buffer_revision,
            available_width: layout_width,
            lines: Rc::clone(&lines),
        });
        lines
    }

    pub(crate) fn lexical_line_start_states(&self) -> Rc<Vec<dragent_editor::HighlightState>> {
        if let Some(cache) = self.lexical_state_cache.borrow().as_ref() {
            if cache.revision == self.text_revision && cache.language == self.source_language {
                return Rc::clone(&cache.states);
            }
        }

        let started = std::time::Instant::now();
        let previous = self.lexical_state_cache.borrow_mut().take();
        let dirty_line = self.lexical_dirty_line.replace(None).unwrap_or(0);
        let force_full = self.lexical_force_full_rebuild.replace(false);
        let previous_states = previous
            .as_ref()
            .filter(|cache| cache.language == self.source_language)
            .map(|cache| cache.states.as_slice());
        let states = Rc::new(lexical_line_start_states_after_edit(
            self.source_language,
            &self.buffer,
            previous_states,
            dirty_line,
            force_full,
        ));
        let elapsed = started.elapsed();
        if elapsed > std::time::Duration::from_millis(10) {
            log::warn!(
                "[editor-perf] slow lexical-state rebuild: {:?} lines={}",
                elapsed,
                states.len()
            );
        }
        *self.lexical_state_cache.borrow_mut() = Some(LexicalStateCache {
            revision: self.text_revision,
            language: self.source_language,
            states: Rc::clone(&states),
        });
        states
    }

    /// Find the visual row index that contains `cursor` in `visual_lines`.
    pub(crate) fn cursor_visual_row(cursor: usize, visual_lines: &[(String, usize)]) -> usize {
        // A folded source range leaves a gap between visible rows. The cursor
        // can remain inside that hidden range, so it may not be contained by
        // any row at all. In that case, use the preceding visible row rather
        // than falling through to the final row (whose start can be after the
        // cursor and would underflow when calculating its local column).
        let mut previous_row = 0;
        for (row, (text, offset)) in visual_lines.iter().enumerate() {
            if cursor < *offset {
                return row.saturating_sub(1);
            }
            let end = offset.saturating_add(text.chars().count());
            if cursor <= end {
                return row;
            }
            previous_row = row;
        }
        previous_row
    }

    /// Scroll the ClippedScrollable so `cursor_row` is visible within the editor's max height.
    pub(crate) fn vertical_max_height(&self) -> f32 {
        match &self.mode {
            InputMode::MultiLine { max_height, .. } => *max_height as f32,
            _ => INPUT_MAX_HEIGHT,
        }
    }

    /// Standalone source editors keep a small tail so the final line can be
    /// moved away from the bottom edge. An editor embedded in the aggregate
    /// Git diff is already inside the diff's scroll surface, so that padding
    /// would become visible blank space between file cards.
    pub(crate) fn source_editor_bottom_padding(&self) -> f32 {
        if self.git_diff_projection.is_some() && self.external_horizontal_scroll_state.is_some() {
            0.0
        } else {
            SOURCE_EDITOR_BOTTOM_PADDING
        }
    }

    pub(crate) fn vertical_viewport_height(&self, laid_out_height: Option<f32>) -> f32 {
        let max_h = self.vertical_max_height();
        let lh = self.line_height();
        // The scroll handle records the actual clipped viewport.  A saved
        // element bound can describe the editor's larger laid-out canvas when
        // it is embedded in the document workspace; using that value makes a
        // search result look "already visible" even though it is below the
        // screen. Prefer the real clip, with bounds/configuration as fallbacks
        // before the first layout pass.
        let clipped_height = self.scroll_state.viewport_height();
        let h = (clipped_height.is_finite() && clipped_height > 0.0)
            .then_some(clipped_height)
            .or(laid_out_height)
            .filter(|h| h.is_finite() && *h > 0.0)
            .unwrap_or(max_h);
        h.min(max_h).max(lh)
    }

    pub(crate) fn visual_content_height(&self, lines: &[VisualLine]) -> f32 {
        lines.len().max(1) as f32 * self.line_height()
            + if self.is_source_editor() {
                self.source_editor_bottom_padding()
            } else {
                0.0
            }
    }

    /// Move the viewport while a selection is dragged against its edge. The
    /// pointer remains in screen coordinates; after this changes the scroll
    /// state, the normal visual-map hit test resolves the pointer again.
    pub(crate) fn auto_scroll_for_drag(&self, drag_pos: Vector2F, ctx: &ViewContext<Self>) {
        let Some(bounds) = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
        else {
            return;
        };

        if self.is_source_editor()
            && self.document_capabilities.large_file
            && self.git_diff_projection.is_none()
        {
            let index = self.large_source_index();
            let viewport_h = self.vertical_viewport_height(Some(bounds.height()));
            let line_h = self.line_height();
            let content_h =
                index.line_count().max(1) as f32 * line_h + self.source_editor_bottom_padding();
            let max_scroll = (content_h - viewport_h).max(0.0);
            let current = self
                .scroll_state
                .scroll_start()
                .as_f32()
                .clamp(0.0, max_scroll);
            let top = bounds.origin().y();
            let bottom = top + bounds.height();
            let edge = 28.0;
            let next = if drag_pos.y() < top + edge {
                current - (top + edge - drag_pos.y()).max(6.0)
            } else if drag_pos.y() > bottom - edge {
                current + (drag_pos.y() - (bottom - edge)).max(6.0)
            } else {
                current
            }
            .clamp(0.0, max_scroll);
            if (next - current).abs() > f32::EPSILON {
                self.scroll_state.scroll_to(next.into_pixels());
            }

            let viewport_width = self.source_horizontal_viewport_width(bounds.width());
            let horizontal_current = self
                .effective_horizontal_scroll_state()
                .scroll_start()
                .as_f32()
                .max(0.0);
            let content_left = bounds.origin().x()
                + if self.external_horizontal_scroll_state.is_some() {
                    horizontal_current
                } else {
                    0.0
                }
                + self.source_gutter_width();
            let content_right = content_left + EDITOR_CONTENT_PADDING_LEFT + viewport_width;
            let estimated_content_width = large_source_layout_char_extent(&index) as f32
                * (self.font_size() * 0.62)
                + SOURCE_EDITOR_RIGHT_PADDING;
            let horizontal_max = (estimated_content_width - viewport_width).max(0.0);
            let horizontal_next = if drag_pos.x() < content_left + edge {
                horizontal_current - (content_left + edge - drag_pos.x()).max(6.0)
            } else if drag_pos.x() > content_right - edge {
                horizontal_current + (drag_pos.x() - (content_right - edge)).max(6.0)
            } else {
                horizontal_current
            }
            .clamp(0.0, horizontal_max);
            if (horizontal_next - horizontal_current).abs() > f32::EPSILON {
                self.effective_horizontal_scroll_state()
                    .scroll_to(horizontal_next.into_pixels());
            }
            return;
        }

        let lines = self.visual_line_map(self.layout_width.get(), ctx.font_cache());
        let viewport_h = self.vertical_viewport_height(Some(bounds.height()));
        let max_scroll = (self.visual_content_height(&lines) - viewport_h).max(0.0);
        let current = self
            .scroll_state
            .scroll_start()
            .as_f32()
            .clamp(0.0, max_scroll);
        let top = bounds.origin().y();
        let bottom = top + bounds.height();
        let edge = 28.0;
        let next = if drag_pos.y() < top + edge {
            current - (top + edge - drag_pos.y()).max(6.0)
        } else if drag_pos.y() > bottom - edge {
            current + (drag_pos.y() - (bottom - edge)).max(6.0)
        } else {
            current
        };
        let next = next.clamp(0.0, max_scroll);
        if (next - current).abs() > f32::EPSILON {
            self.scroll_state.scroll_to(next.into_pixels());
        }

        // Source editors deliberately keep long lines unwrapped and use a
        // separate horizontal viewport. During a drag, the pointer can leave
        // that viewport without producing a new character hit, so update the
        // horizontal scroll directly from the pointer position as well.
        if self.is_source_editor() {
            let viewport_width = self.source_horizontal_viewport_width(bounds.width());
            let horizontal_current = self
                .effective_horizontal_scroll_state()
                .scroll_start()
                .as_f32()
                .max(0.0);
            let content_left = bounds.origin().x()
                + if self.external_horizontal_scroll_state.is_some() {
                    horizontal_current
                } else {
                    0.0
                }
                + self.source_gutter_width();
            let content_right = content_left + EDITOR_CONTENT_PADDING_LEFT + viewport_width;
            let horizontal_content_width = lines
                .iter()
                .map(|line| line.layout.width + line.layout.trailing_whitespace_width)
                .fold(0.0_f32, f32::max)
                + SOURCE_EDITOR_RIGHT_PADDING;
            let horizontal_max = (horizontal_content_width - viewport_width).max(0.0);
            let horizontal_next = if drag_pos.x() < content_left + edge {
                horizontal_current - (content_left + edge - drag_pos.x()).max(6.0)
            } else if drag_pos.x() > content_right - edge {
                horizontal_current + (drag_pos.x() - (content_right - edge)).max(6.0)
            } else {
                horizontal_current
            }
            .clamp(0.0, horizontal_max);
            if (horizontal_next - horizontal_current).abs() > f32::EPSILON {
                self.effective_horizontal_scroll_state()
                    .scroll_to(horizontal_next.into_pixels());
            }
        }
    }

    pub(crate) fn clamped_vertical_scroll(&self, visual_line_count: usize, viewport_h: f32) -> f32 {
        let lh = self.line_height();
        let content_h = visual_line_count.max(1) as f32 * lh
            + if self.is_source_editor() {
                self.source_editor_bottom_padding()
            } else {
                0.0
            };
        let max_scroll = (content_h - viewport_h).max(0.0);
        let current = self
            .scroll_state
            .scroll_start()
            .as_f32()
            .clamp(0.0, max_scroll);
        if (current - self.scroll_state.scroll_start().as_f32()).abs() > f32::EPSILON {
            self.scroll_state.scroll_to(current.into_pixels());
        }
        current
    }

    pub(crate) fn scroll_cursor_into_view(
        &self,
        cursor_row: usize,
        visual_line_count: usize,
        viewport_h: f32,
    ) {
        let lh = self.line_height();
        let top = cursor_row as f32 * lh;
        let bottom = top + lh;
        let current = self.clamped_vertical_scroll(visual_line_count, viewport_h);
        let bottom_margin = if self.is_source_editor() {
            SOURCE_EDITOR_CURSOR_BOTTOM_MARGIN.min((viewport_h - lh).max(0.0))
        } else {
            0.0
        };
        let new_scroll = if top < current {
            top
        } else if bottom > current + viewport_h - bottom_margin {
            (bottom + bottom_margin - viewport_h).max(0.0)
        } else {
            return;
        };
        self.scroll_state.scroll_to(new_scroll.into_pixels());
    }

    pub(crate) fn scroll_cursor_into_view_for_layout(
        &self,
        visual_lines: &[(String, usize)],
        ctx: &ViewContext<Self>,
    ) {
        let bounds_h = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
            .map(|b| b.height());
        let viewport_h = self.vertical_viewport_height(bounds_h);

        // Large-source rendering intentionally materializes only the visible
        // rows. Search navigation can jump anywhere in the document, so resolve
        // that destination through the complete compact line index instead of
        // a visual map tied to the previous viewport.
        if self.document_capabilities.large_file && self.git_diff_projection.is_none() {
            let index = self.large_source_index();
            self.scroll_cursor_into_view(
                index.line_for_char(self.cursor),
                index.line_count(),
                viewport_h,
            );
            return;
        }

        let projected = self.git_diff_projection.as_ref().and_then(|projection| {
            let source_line = source_line_number(&self.char_vec(), self.cursor);
            projection.rows.iter().position(|row| {
                matches!(row, GitDiffDisplayRow::Current { line, .. } if *line == source_line)
            })
        });
        let row = projected.unwrap_or_else(|| Self::cursor_visual_row(self.cursor, visual_lines));
        let total_rows = self
            .git_diff_projection
            .as_ref()
            .map_or(visual_lines.len(), |projection| projection.rows.len());
        self.scroll_cursor_into_view(row, total_rows, viewport_h);
    }

    pub(crate) fn source_cursor_is_visible(&self, ctx: &ViewContext<Self>) -> bool {
        let bounds_h = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
            .map(|bounds| bounds.height());
        let viewport_h = self.vertical_viewport_height(bounds_h);
        if self.document_capabilities.large_file {
            let index = self.large_source_index();
            let row = index.line_for_char(self.cursor);
            let scroll_top = self.clamped_vertical_scroll(index.line_count(), viewport_h);
            return visual_row_is_visible(row, scroll_top, viewport_h, self.line_height());
        }
        let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        let projected = self.git_diff_projection.as_ref().and_then(|projection| {
            let source_line = source_line_number(&self.char_vec(), self.cursor);
            projection.rows.iter().position(|row| {
                matches!(row, GitDiffDisplayRow::Current { line, .. } if *line == source_line)
            })
        });
        let row = projected.unwrap_or_else(|| Self::cursor_visual_row(self.cursor, &visual_lines));
        let total_rows = self
            .git_diff_projection
            .as_ref()
            .map_or(visual_lines.len(), |projection| projection.rows.len());
        let scroll_top = self.clamped_vertical_scroll(total_rows, viewport_h);
        visual_row_is_visible(row, scroll_top, viewport_h, self.line_height())
    }

    pub(crate) fn scroll_source_cursor_horizontally_into_view(&self, ctx: &ViewContext<Self>) {
        if !self.is_source_editor() {
            return;
        }
        let Some(bounds) = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
        else {
            return;
        };
        if self.document_capabilities.large_file {
            let index = self.large_source_index();
            let line_number = index.line_for_char(self.cursor);
            let Some(line) =
                self.large_source_visual_line(&index, line_number, ctx.font_cache(), true)
            else {
                return;
            };
            let cursor_col = self
                .cursor
                .saturating_sub(line.abs_char_offset)
                .min(line.text.chars().count());
            let cursor_x = caret_x_in_layout(&line.text, &line.layout, cursor_col);
            let viewport_width = self.source_horizontal_viewport_width(bounds.width());
            let current = self
                .effective_horizontal_scroll_state()
                .scroll_start()
                .as_f32()
                .max(0.0);
            let margin = 12.0;
            let next = if cursor_x < current + margin {
                (cursor_x - margin).max(0.0)
            } else if cursor_x > current + viewport_width - margin {
                cursor_x - viewport_width + margin
            } else {
                return;
            };
            let estimated_content_width = large_source_layout_char_extent(&index) as f32
                * (self.font_size() * 0.62)
                + SOURCE_EDITOR_RIGHT_PADDING;
            let max_scroll = (estimated_content_width - viewport_width).max(0.0);
            self.effective_horizontal_scroll_state()
                .scroll_to(next.clamp(0.0, max_scroll).into_pixels());
            return;
        }
        let lines = self.visual_line_map(self.layout_width.get(), ctx.font_cache());
        let Some(line) = lines.iter().find(|line| {
            self.cursor >= line.abs_char_offset
                && self.cursor <= line.abs_char_offset + line.text.chars().count()
        }) else {
            return;
        };
        let cursor_col = self.cursor.saturating_sub(line.abs_char_offset);
        let cursor_x = caret_x_in_layout(&line.text, &line.layout, cursor_col);
        let viewport_width = self.source_horizontal_viewport_width(bounds.width());
        let current = self
            .effective_horizontal_scroll_state()
            .scroll_start()
            .as_f32()
            .max(0.0);
        let margin = 12.0;
        let next = if cursor_x < current + margin {
            (cursor_x - margin).max(0.0)
        } else if cursor_x > current + viewport_width - margin {
            cursor_x - viewport_width + margin
        } else {
            return;
        };
        let content_width = lines
            .iter()
            .map(|line| line.layout.width + line.layout.trailing_whitespace_width)
            .fold(0.0_f32, f32::max)
            + SOURCE_EDITOR_RIGHT_PADDING;
        let max_scroll = (content_width - viewport_width).max(0.0);
        self.effective_horizontal_scroll_state()
            .scroll_to(next.clamp(0.0, max_scroll).into_pixels());
    }

    pub(crate) fn scroll_horizontal_cursor_into_view(&self, app: &AppContext) {
        let display_owned;
        let display: &str = if self.secure {
            display_owned = self.display_text();
            &display_owned
        } else {
            &self.buffer
        };
        let cursor_x = self.caret_x_on_line(display, self.cursor, app, false);
        let mut width = self.scroll_state.viewport_width();
        if width <= 0.0 || width > 1000.0 {
            width = 200.0; // Sensible default sidebar width until bounds are laid out
        }
        let current = self.scroll_state.scroll_start().as_f32();

        let left_margin = 20.0f32.min(width * 0.1);
        // Right margin MUST equal the padding_right added to the content in render_plain_canvas
        // (currently 8px). If it were larger, scroll_to() would overshoot the max scroll that
        // ClippedScrollable::after_layout will clamp to, causing the cursor drawn in render()
        // to appear left of where the content actually ends up in paint().
        let right_margin = PLAIN_CANVAS_RIGHT_PADDING;

        let new_scroll = if cursor_x < current + left_margin {
            (cursor_x - left_margin).max(0.0)
        } else if cursor_x > current + width - right_margin {
            (cursor_x - width + right_margin).max(0.0)
        } else {
            return;
        };
        self.scroll_state.scroll_to(new_scroll.into_pixels());
    }
}
