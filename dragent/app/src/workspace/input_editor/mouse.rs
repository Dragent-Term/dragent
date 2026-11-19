//! Mouse/keyboard editing gestures: clicks, drag, undo, tab.
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
    Breakpoint, BreakpointRegistry, CommentStyle, DiagnosticSeverity, FoldIndex, FoldParser,
    FoldRegion, FoldState, Language, LineChangeKind, ResolvedDiagnostic,
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
    pub(crate) fn handle_cursor_up(&mut self, ctx: &mut ViewContext<Self>) {
        let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        let row = Self::cursor_visual_row(self.cursor, &vls);
        if row == 0 {
            return;
        }
        let col_in_row = self
            .cursor
            .saturating_sub(vls[row].1)
            .min(vls[row].0.chars().count());
        let cursor_x = caret_x_for_char_index(
            &vls[row].0,
            col_in_row,
            self.ui_font,
            self.font_size(),
            &self.theme,
            ctx,
        );
        let tgt = row - 1;
        let tgt_col = char_col_at_x(
            &vls[tgt].0,
            cursor_x,
            self.ui_font,
            self.font_size(),
            ctx.font_cache(),
        );
        self.cursor = self.clamp_off_chip_nearest(vls[tgt].1 + tgt_col);
        self.clear_selection();
        self.cursor_visible = true;
        self.scroll_cursor_into_view_for_layout(&vls, ctx);
        ctx.notify();
    }

    pub(crate) fn handle_cursor_down(&mut self, ctx: &mut ViewContext<Self>) {
        let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        let row = Self::cursor_visual_row(self.cursor, &vls);
        if row + 1 >= vls.len() {
            return;
        }
        let col_in_row = self
            .cursor
            .saturating_sub(vls[row].1)
            .min(vls[row].0.chars().count());
        let cursor_x = caret_x_for_char_index(
            &vls[row].0,
            col_in_row,
            self.ui_font,
            self.font_size(),
            &self.theme,
            ctx,
        );
        let tgt = row + 1;
        let tgt_col = char_col_at_x(
            &vls[tgt].0,
            cursor_x,
            self.ui_font,
            self.font_size(),
            ctx.font_cache(),
        );
        self.cursor = self.clamp_off_chip_nearest(vls[tgt].1 + tgt_col);
        self.clear_selection();
        self.cursor_visible = true;
        self.scroll_cursor_into_view_for_layout(&vls, ctx);
        ctx.notify();
    }

    pub(crate) fn word_range_at(&self, char_idx: usize) -> (usize, usize) {
        let chars: Vec<char> = self.buffer.chars().collect();
        let len = chars.len();
        if len == 0 {
            return (0, 0);
        }
        let idx = char_idx.min(len.saturating_sub(1));
        let ch = chars[idx];
        // Extra chars that break words.
        let extra: Option<String> =
            crate::console::editor_settings::EditorSettings::global()
                .terminal
                .extra_word_separators()
                .map(str::to_string);
        let is_sep = |c: char| extra.as_deref().is_some_and(|s| s.contains(c));
        // A separator char selects its own run (it breaks neighboring words).
        if is_sep(ch) {
            let mut start = idx;
            let mut end = idx;
            while start > 0 && is_sep(chars[start - 1]) {
                start -= 1;
            }
            while end < len && is_sep(chars[end]) {
                end += 1;
            }
            return (start, end);
        }
        let is_word = |c: char| (c.is_alphanumeric() || c == '_') && !is_sep(c);
        let mut start = idx;
        let mut end = idx;
        if is_word(ch) {
            while start > 0 && is_word(chars[start - 1]) {
                start -= 1;
            }
            while end < len && is_word(chars[end]) {
                end += 1;
            }
        } else if !ch.is_whitespace() {
            while start > 0
                && !is_word(chars[start - 1])
                && chars[start - 1] != '_'
                && !chars[start - 1].is_whitespace()
                && !is_sep(chars[start - 1])
            {
                start -= 1;
            }
            while end < len
                && !is_word(chars[end])
                && chars[end] != '_'
                && !chars[end].is_whitespace()
                && !is_sep(chars[end])
            {
                end += 1;
            }
        } else {
            while start > 0 && chars[start - 1].is_whitespace() {
                start -= 1;
            }
            while end < len && chars[end].is_whitespace() {
                end += 1;
            }
        }
        (start, end)
    }

    /// Snapshot the current buffer+cursor before a mutation.
    /// Every call pushes an independent entry — undo is character-by-character.
    pub(crate) fn snapshot_undo(&mut self) {
        // Full-string snapshots are deliberately coarse for large files. A
        // snapshot per keystroke would duplicate tens or hundreds of MiB and
        // make typing slower than the edit itself. Keep one restore point for
        // the current editing burst; normal-sized buffers retain character
        // granular undo behavior.
        if self.document_capabilities.large_file && !self.undo_stack.is_empty() {
            self.redo_stack.clear();
            return;
        }
        self.undo_stack
            .push((self.buffer.clone(), self.cursor, self.chip_spans.clone()));
        self.redo_stack.clear();
    }

    pub(crate) fn handle_undo(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some((buf, cur, chips)) = self.undo_stack.pop() {
            self.redo_stack
                .push((self.buffer.clone(), self.cursor, self.chip_spans.clone()));
            self.buffer = buf;
            self.invalidate_buffer_content_fully();
            self.cursor = cur;
            self.chip_spans = chips;
            self.clear_selection();
            self.notify_changed(ctx);
        }
    }

    pub(crate) fn handle_redo(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some((buf, cur, chips)) = self.redo_stack.pop() {
            self.undo_stack
                .push((self.buffer.clone(), self.cursor, self.chip_spans.clone()));
            self.buffer = buf;
            self.invalidate_buffer_content_fully();
            self.cursor = cur;
            self.chip_spans = chips;
            self.clear_selection();
            self.notify_changed(ctx);
        }
    }

    pub(crate) fn notify_changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        self.publish_buffer_change(true, ctx);
        self.schedule_fold_refresh(ctx);
        ctx.notify();
    }

    pub(crate) fn handle_double_click_at(
        &mut self,
        click_pos: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.focus_self();
        self.follow_bottom.set(false);
        if let Some(char_idx) = self.char_index_at_position(click_pos, ctx) {
            let (start, end) = self.word_range_at(char_idx);
            self.selection_anchor = Some(start);
            self.drag_anchor = None;
            self.cursor = end;
            self.cursor_visible = true;
            self.schedule_cursor_blink(ctx);
            ctx.notify();
        }
    }

    pub(crate) fn handle_triple_click_at(
        &mut self,
        click_pos: Vector2F,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.focus_self();
        self.follow_bottom.set(false);
        // Resolve the character under the click, then expand to the enclosing logical line.
        let click_char = self
            .char_index_at_position(click_pos, ctx)
            .unwrap_or(self.cursor);
        let chars: Vec<char> = self.buffer.chars().collect();
        let line_start = chars[..click_char]
            .iter()
            .rposition(|&c| c == '\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let line_end = chars[click_char..]
            .iter()
            .position(|&c| c == '\n')
            .map(|i| click_char + i)
            .unwrap_or(chars.len());
        self.selection_anchor = Some(line_start);
        self.cursor = line_end;
        self.drag_anchor = None;
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    pub(crate) fn diff_comment_line_at(&self, pos: Vector2F, ctx: &ViewContext<Self>) -> Option<(usize, bool)> {
        let projection = self.git_diff_projection.as_ref()?;
        let bounds = ctx.element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))?;
        let y = pos.y() - bounds.origin().y() + self.scroll_state.scroll_start().as_f32().max(0.0);
        let row = (y.max(0.0) / self.line_height()).floor() as usize;
        match projection.rows.get(row)? {
            GitDiffDisplayRow::Deleted { old_line, .. } => Some((*old_line, true)),
            GitDiffDisplayRow::Current { line, .. } => Some((*line, false)),
            _ => None,
        }
    }

    /// Track which diff line the pointer is on, anywhere along the line, so
    /// its gutter shows the comment + without aiming at the line numbers.
    pub(crate) fn update_diff_comment_hover(&mut self, pos: Vector2F, ctx: &mut ViewContext<Self>) {
        let inside = ctx
            .element_position_by_id(&self.viewport_position_id())
            .or_else(|| ctx.element_position_by_id(&self.position_id()))
            .is_some_and(|bounds| bounds.contains_point(pos));
        let line = if inside && self.git_diff_projection.is_some() {
            self.diff_comment_line_at(pos, ctx)
        } else {
            None
        };
        if line != self.diff_comment_hover_line {
            self.diff_comment_hover_line = line;
            ctx.notify();
        }
    }

    pub(crate) fn handle_click_at(&mut self, click_pos: Vector2F, ctx: &mut ViewContext<Self>) {
        ctx.focus_self();
        self.follow_bottom.set(false);
        if let Some(char_idx) = self.char_index_at_position(click_pos, ctx) {
            let char_idx = self.clamp_off_chip_nearest(char_idx);
            if self.definition_hover_enabled() {
                // Prefer the already-resolved hover target, but do not make
                // navigation depend on an LSP response winning a race with
                // the click. A slow or missed hover event must still follow
                // the same semantic Go-to-Definition command on mouse-up;
                // an unresolvable token simply produces the normal LSP
                // "no definition" result and never navigates by text search.
                let hovered_symbol = self.definition_hover.as_ref().filter(|hover| {
                    char_idx >= hover.symbol_range.start && char_idx < hover.symbol_range.end
                });
                if let Some(symbol_range) = hovered_symbol
                    .map(|hover| hover.symbol_range.clone())
                    .or_else(|| self.string_link_range_at(char_idx))
                    .or_else(|| self.identifier_range_at(char_idx))
                {
                    log::info!(
                        "[definition-link] modifier-click candidate anchor={} range={:?} resolved_hover={}",
                        char_idx,
                        symbol_range,
                        hovered_symbol.is_some(),
                    );
                    self.definition_click_candidate = Some(DefinitionClickCandidate {
                        request_id: hovered_symbol.map_or(0, |hover| hover.request_id),
                        symbol_range,
                        anchor: char_idx,
                    });
                    return;
                }
            }
            self.selection_anchor = None;
            self.drag_anchor = Some(char_idx);
            self.cursor = char_idx;
            self.cursor_visible = true;
            self.schedule_cursor_blink(ctx);
            if self.mode == InputMode::TerminalCommand {
                ctx.emit(InputEditorEvent::Clicked(click_pos));
                ctx.emit(InputEditorEvent::Changed {
                    text: self.buffer.clone(),
                    cursor: self.cursor,
                });
            }
            if self.is_agent_mode {
                self.refresh_mention_state(ctx);
            }
            ctx.notify();
        } else {
            ctx.notify();
        }
    }

    pub(crate) fn handle_drag_at(&mut self, drag_pos: Vector2F, ctx: &mut ViewContext<Self>) {
        if self.diff_comment_drag.is_some() { ctx.notify(); return; }
        if let Some(candidate) = self.definition_click_candidate.take() {
            // A completed click navigates; movement becomes an ordinary text
            // selection and must never trigger definition navigation.
            self.drag_anchor = Some(candidate.anchor);
            self.selection_anchor = None;
        }
        self.follow_bottom.set(false);
        self.auto_scroll_for_drag(drag_pos, ctx);
        if let Some(char_idx) = self.char_index_at_position(drag_pos, ctx) {
            self.cursor = self.clamp_off_chip_nearest(char_idx);
            self.cursor_visible = true;
            if self.is_agent_mode {
                self.refresh_mention_state(ctx);
            }
            ctx.notify();
        }
    }

    pub(crate) fn handle_release_at(&mut self, release_pos: Vector2F, ctx: &mut ViewContext<Self>) {
        if let Some((start, start_deleted)) = self.diff_comment_drag.take() {
            let (end, end_deleted) = self.diff_comment_line_at(release_pos, ctx).unwrap_or((start, start_deleted));
            ctx.emit(InputEditorEvent::DiffCommentSelection { start, end, start_deleted, end_deleted });
            ctx.notify();
            return;
        }
        if let Some(candidate) = self.definition_click_candidate.take() {
            let release_index = self.char_index_at_position(release_pos, ctx);
            if self.definition_modifier_held
                && release_index.is_some_and(|index| {
                    index >= candidate.symbol_range.start && index < candidate.symbol_range.end
                })
            {
                log::info!(
                    "[definition-link] valid link clicked id={} anchor={}",
                    candidate.request_id,
                    candidate.anchor
                );
                ctx.emit(InputEditorEvent::DefinitionHoverClicked {
                    request_id: candidate.request_id,
                    cursor: candidate.anchor,
                });
                self.definition_hover = None;
                ctx.notify();
                return;
            }
            log::info!(
                "[definition-link] click released but not a link navigation (modifier={} within={:?})",
                self.definition_modifier_held,
                release_index,
            );
        }
        if let Some(anchor) = self.drag_anchor.take() {
            // Drag release — resolve final cursor position then commit selection.
            if let Some(char_idx) = self.char_index_at_position(release_pos, ctx) {
                self.cursor = self.clamp_off_chip_nearest(char_idx);
            }
            if anchor != self.cursor {
                self.selection_anchor = Some(anchor);
            } else {
                self.selection_anchor = None;
            }
            if self.is_agent_mode {
                self.refresh_mention_state(ctx);
            }
        }
        // Double/triple-click: drag_anchor is None, so we leave cursor and
        // selection_anchor exactly where the multi-click handler placed them.
        ctx.notify();
    }

    pub(crate) fn handle_source_tab(&mut self, outdent: bool, ctx: &mut ViewContext<Self>) {
        let chars = self.char_vec();
        let cursor = self.cursor.min(chars.len());
        let selection = self.active_selection_range();

        let unit = self.indent_unit();
        let unit_is_tab = unit == "\t";

        if selection.is_none() && !outdent {
            if unit_is_tab {
                self.insert_text_at_cursor("\t", ctx);
            } else {
                let column = cursor.saturating_sub(line_start(&chars, cursor));
                let tab = unit.len().max(1);
                let width = tab - (column % tab);
                self.insert_text_at_cursor(&" ".repeat(width), ctx);
            }
            return;
        }

        let (start, end) = selection.unwrap_or((cursor, cursor));
        let first_line = line_start(&chars, start);
        let effective_end = if end > start && chars.get(end.saturating_sub(1)) == Some(&'\n') {
            end.saturating_sub(1)
        } else {
            end
        };
        let mut line_starts = vec![first_line];
        for index in first_line..effective_end.min(chars.len()) {
            if chars[index] == '\n' && index + 1 <= effective_end {
                line_starts.push(index + 1);
            }
        }

        self.snapshot_undo();
        let mut next = chars;
        let mut edits = Vec::new();
        if outdent {
            for &line in line_starts.iter().rev() {
                let remove = if next.get(line) == Some(&'\t') {
                    1
                } else {
                    next.iter()
                        .skip(line)
                        .take(if unit_is_tab {
                            super::text::source_tab_size()
                        } else {
                            unit.len()
                        })
                        .take_while(|ch| **ch == ' ')
                        .count()
                };
                if remove > 0 {
                    next.drain(line..line + remove);
                    edits.push((line, -(remove as isize)));
                }
            }
        } else {
            for &line in line_starts.iter().rev() {
                let insert = unit.clone();
                let delta = insert.chars().count() as isize;
                next.splice(line..line, insert.chars());
                edits.push((line, delta));
            }
        }

        if edits.is_empty() {
            return;
        }
        edits.sort_by_key(|(position, _)| *position);
        let adjust = |position: usize| -> usize {
            edits.iter().fold(position as isize, |value, (at, delta)| {
                if *at < position || (*at == position && *delta > 0) {
                    (value + delta).max(*at as isize)
                } else {
                    value
                }
            }) as usize
        };
        self.buffer = next.into_iter().collect();
        self.invalidate_buffer_content_fully();
        self.cursor = adjust(self.cursor);
        if let Some(anchor) = self.selection_anchor {
            self.selection_anchor = Some(adjust(anchor));
        }
        self.drag_anchor = None;
        self.publish_buffer_change(true, ctx);
        ctx.notify();
    }

    /// Toggle line/block comment syntax over the current line or selection
    /// (Cmd+/ on macOS, Ctrl+/ elsewhere) — matches VS Code: if every
    /// touched non-blank line is already commented, uncomment; otherwise
    /// comment every touched line. Languages with no comment syntax that's
    /// safe to insert (strict JSON, plain text) are a no-op. The character
    /// math lives in the pure `compute_*_comment_toggle` helpers (see
    /// `model.rs`) so it's unit-testable without a live `ViewContext`.
    ///
    /// The comment syntax itself is resolved from where the selection starts
    /// against the real parsed syntax tree (`dragent_editor::comment_style_at`
    /// — the same tree-sitter grammars `folding.rs` already parses this
    /// language with), not from the file's language alone: a `.tsx` file is
    /// mostly TypeScript (`//`) but JSX markup inside it needs `{/* */}`, and
    /// an HTML `<script>`/`<style>` body comments like JS/CSS, not like HTML.
    /// A single per-file answer would insert a comment token that's invalid
    /// at the cursor's actual position.
    pub(crate) fn handle_toggle_line_comment(&mut self, ctx: &mut ViewContext<Self>) {
        let chars = self.char_vec();
        let cursor = self.cursor.min(chars.len());
        let selection = self.active_selection_range();
        let context_position = selection.map_or(cursor, |(start, _)| start);
        let byte_position = self
            .buffer
            .char_indices()
            .nth(context_position)
            .map_or(self.buffer.len(), |(byte, _)| byte);
        let Some(style) =
            dragent_editor::comment_style_at(self.source_language, &self.buffer, byte_position)
        else {
            return;
        };

        match style {
            CommentStyle::Line(prefix) => {
                let (start, end) = selection.unwrap_or((cursor, cursor));
                let first_line = line_start(&chars, start);
                let effective_end =
                    if end > start && chars.get(end.saturating_sub(1)) == Some(&'\n') {
                        end.saturating_sub(1)
                    } else {
                        end
                    };
                let mut line_starts = vec![first_line];
                for index in first_line..effective_end.min(chars.len()) {
                    if chars[index] == '\n' && index + 1 <= effective_end {
                        line_starts.push(index + 1);
                    }
                }
                let Some((next, edits)) = compute_line_comment_toggle(&chars, &line_starts, prefix)
                else {
                    return;
                };
                self.snapshot_undo();
                let adjust = |position: usize| -> usize {
                    edits.iter().fold(position as isize, |value, (at, delta)| {
                        if *at < position || (*at == position && *delta > 0) {
                            (value + delta).max(*at as isize)
                        } else {
                            value
                        }
                    }) as usize
                };
                self.buffer = next.into_iter().collect();
                self.invalidate_buffer_content_fully();
                self.cursor = adjust(self.cursor);
                if let Some(anchor) = self.selection_anchor {
                    self.selection_anchor = Some(adjust(anchor));
                }
                self.drag_anchor = None;
                self.publish_buffer_change(true, ctx);
                ctx.notify();
            }
            CommentStyle::Block(open, close) => {
                let (span_start, span_end) = match selection {
                    Some((s, e)) => (s, e),
                    None => (line_start(&chars, cursor), line_end(&chars, cursor)),
                };
                let Some((next, trimmed_start, trimmed_end, delta)) =
                    compute_block_comment_toggle(&chars, span_start, span_end, open, close)
                else {
                    return;
                };
                self.snapshot_undo();
                let adjust = |position: usize| -> usize {
                    if position <= trimmed_start {
                        position
                    } else if position >= trimmed_end {
                        (position as isize + delta).max(trimmed_start as isize) as usize
                    } else {
                        trimmed_start
                    }
                };
                self.buffer = next.into_iter().collect();
                self.invalidate_buffer_content_fully();
                self.cursor = adjust(self.cursor);
                if let Some(anchor) = self.selection_anchor {
                    self.selection_anchor = Some(adjust(anchor));
                }
                self.drag_anchor = None;
                self.publish_buffer_change(true, ctx);
                ctx.notify();
            }
        }
    }

    /// Move the current line, or every line touched by the selection, up or
    /// down past its neighbor (Alt+Up / Alt+Down). No-op at a buffer edge.
    pub(crate) fn handle_move_line(&mut self, up: bool, ctx: &mut ViewContext<Self>) {
        let chars = self.char_vec();
        let cursor = self.cursor.min(chars.len());
        let selection = self.active_selection_range();
        let (sel_start, sel_end) = selection.unwrap_or((cursor, cursor));
        let (block_start, block_end) = touched_line_block(&chars, sel_start, sel_end);
        let Some((next, delta)) = compute_move_line(&chars, block_start, block_end, up) else {
            return;
        };
        self.snapshot_undo();
        self.buffer = next.into_iter().collect();
        self.invalidate_buffer_content_fully();
        self.cursor = (self.cursor.min(chars.len()) as isize + delta) as usize;
        if let Some((start, end)) = selection {
            self.selection_anchor = Some((start as isize + delta) as usize);
            self.cursor = (end as isize + delta) as usize;
        }
        self.drag_anchor = None;
        self.publish_buffer_change(true, ctx);
        ctx.notify();
    }

    /// Delete the current line, or every line touched by the selection,
    /// entirely (Cmd+Shift+K / Ctrl+Shift+K). The cursor lands at the start
    /// of whichever line now occupies that position.
    pub(crate) fn handle_delete_line(&mut self, ctx: &mut ViewContext<Self>) {
        let chars = self.char_vec();
        let cursor = self.cursor.min(chars.len());
        let selection = self.active_selection_range();
        let (sel_start, sel_end) = selection.unwrap_or((cursor, cursor));
        let (block_start, block_end) = touched_line_block(&chars, sel_start, sel_end);
        let next = compute_delete_line(&chars, block_start, block_end);
        self.snapshot_undo();
        self.buffer = next.into_iter().collect();
        self.invalidate_buffer_content_fully();
        self.cursor = block_start.min(self.buffer.chars().count());
        self.selection_anchor = None;
        self.drag_anchor = None;
        self.publish_buffer_change(true, ctx);
        ctx.notify();
    }

    /// Duplicate the current line, or every line touched by the selection,
    /// immediately below (Shift+Alt+Down) or above (Shift+Alt+Up) itself.
    pub(crate) fn handle_duplicate_line(&mut self, below: bool, ctx: &mut ViewContext<Self>) {
        let chars = self.char_vec();
        let cursor = self.cursor.min(chars.len());
        let selection = self.active_selection_range();
        let (sel_start, sel_end) = selection.unwrap_or((cursor, cursor));
        let (block_start, block_end) = touched_line_block(&chars, sel_start, sel_end);
        let (next, delta) = compute_duplicate_line(&chars, block_start, block_end, below);
        self.snapshot_undo();
        self.buffer = next.into_iter().collect();
        self.invalidate_buffer_content_fully();
        self.cursor = (self.cursor.min(chars.len()) as isize + delta) as usize;
        if let Some((start, end)) = selection {
            self.selection_anchor = Some((start as isize + delta) as usize);
            self.cursor = (end as isize + delta) as usize;
        }
        self.drag_anchor = None;
        self.publish_buffer_change(true, ctx);
        ctx.notify();
    }
}
