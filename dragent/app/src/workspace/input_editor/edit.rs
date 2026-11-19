//! Input editor text and scroll operations.
//!
//! Responsible for: text mutation, scrolling, rename overlays,
//! placeholders, ghost text, diagnostics selection.
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
    pub fn set_external_horizontal_scroll_state(
        &mut self,
        state: Option<ClippedScrollStateHandle>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.external_horizontal_scroll_state.is_some() == state.is_some() {
            return;
        }
        if state.is_some() {
            self.horizontal_scroll_state.scroll_to(0.0.into_pixels());
        }
        self.external_horizontal_scroll_state = state;
        ctx.notify();
    }

    pub(crate) fn effective_horizontal_scroll_state(&self) -> &ClippedScrollStateHandle {
        self.external_horizontal_scroll_state
            .as_ref()
            .unwrap_or(&self.horizontal_scroll_state)
    }

    pub(crate) fn rendered_horizontal_scroll_offset(&self) -> f32 {
        if self.external_horizontal_scroll_state.is_some() {
            0.0
        } else {
            self.horizontal_scroll_state
                .scroll_start()
                .as_f32()
                .max(0.0)
        }
    }

    pub(crate) fn rendered_horizontal_scroll_state(&self) -> ClippedScrollStateHandle {
        if self.external_horizontal_scroll_state.is_some() {
            self.horizontal_scroll_state.clone()
        } else {
            self.effective_horizontal_scroll_state().clone()
        }
    }

    pub(crate) fn source_horizontal_viewport_width(&self, editor_width: f32) -> f32 {
        let surface_width = self
            .external_horizontal_scroll_state
            .as_ref()
            .map(ClippedScrollStateHandle::viewport_width)
            .filter(|width| width.is_finite() && *width > 0.0)
            .unwrap_or(editor_width);
        (surface_width - self.source_gutter_width() - EDITOR_CONTENT_PADDING_LEFT).max(1.0)
    }

    /// Replace the whole buffer as one normal undoable editor transaction.
    /// Language-server formatting and rename edits use this path so the
    /// rendered editor, working-copy model, and undo stack stay synchronized.
    pub fn replace_text_with_undo(
        &mut self,
        text: &str,
        cursor: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.buffer == text {
            self.set_cursor_char_index(cursor, ctx);
            return;
        }
        self.snapshot_undo();
        self.buffer = text.to_string();
        self.document_capabilities = capabilities_for_text(&self.buffer);
        self.invalidate_buffer_content_fully();
        self.cursor = cursor.min(self.buffer.chars().count());
        self.clear_selection();
        self.chip_spans.clear();
        self.definition_hover = None;
        self.definition_click_candidate = None;
        self.apply_edit(ctx);
    }

    pub fn get_text(&self) -> &str {
        &self.buffer
    }

    /// Current logical cursor position for remote surface snapshots.
    pub fn cursor_char_index(&self) -> usize {
        self.cursor
    }

    pub fn mode(&self) -> &InputMode {
        &self.mode
    }

    /// Ghost completion suffix for remote surface snapshots.
    pub fn ghost_suffix(&self) -> Option<&str> {
        self.ghost_suffix.as_deref()
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        self.mode = mode;
    }

    /// Anchor an inline rename widget under the character range
    /// `anchor_char..end_char` of this document. The widget hosts `editor`
    /// (a separate single-line InputEditor) and tracks scrolling until
    /// [`Self::end_inline_rename`] clears it.
    pub fn begin_inline_rename(&mut self, state: InlineRenameState, ctx: &mut ViewContext<Self>) {
        self.inline_rename = Some(state);
        ctx.notify();
    }

    pub fn end_inline_rename(&mut self, ctx: &mut ViewContext<Self>) {
        if self.inline_rename.take().is_some() {
            ctx.notify();
        }
    }

    /// Build the VS Code-style rename input rendered as a positioned overlay
    /// directly under the symbol being renamed. Returns the element plus its
    /// offset and size in this editor's local coordinates.
    pub(crate) fn render_inline_rename_overlay(
        &self,
        ctx: &AppContext,
    ) -> Option<(Box<dyn Element>, Vector2F, Vector2F)> {
        let state = self.inline_rename.as_ref()?;
        if !self.is_source_editor() {
            return None;
        }
        let font_cache = ctx.font_cache();
        let anchor_char = state.anchor_char.min(self.buffer.chars().count());
        let end_char = state.end_char.min(self.buffer.chars().count());

        // Shape the line that contains the symbol and find its visible row so
        // both the pixel extent and the y position account for folds/scroll.
        let (line, display_row) = if self.document_capabilities.large_file {
            let index = self.large_source_index();
            let row = index.line_for_char(anchor_char);
            (
                self.large_source_visual_line(&index, row, font_cache, true)?,
                row,
            )
        } else {
            let lines = self.visual_line_map(self.layout_width.get(), font_cache);
            let line_index = lines.iter().position(|line| {
                let start = line.abs_char_offset;
                anchor_char >= start && anchor_char <= start + line.text.chars().count()
            })?;
            let fold_index = self.fold_index();
            let visible = self.visible_visual_indices(&lines, &fold_index, self.layout_width.get());
            let display_row = visible.iter().position(|index| *index == line_index)?;
            (lines.get(line_index)?.clone(), display_row)
        };

        let col_start = anchor_char.saturating_sub(line.abs_char_offset);
        let col_end = end_char.saturating_sub(line.abs_char_offset).max(col_start);
        let start_caret_x = caret_x_in_layout(&line.text, &line.layout, col_start);
        let end_caret_x = caret_x_in_layout(&line.text, &line.layout, col_end);
        let scroll_y = self.scroll_state.scroll_start().as_f32().max(0.0);
        let horizontal_scroll = self.rendered_horizontal_scroll_offset();
        let (offset, symbol_width) = inline_rename_anchor(
            display_row,
            scroll_y,
            horizontal_scroll,
            self.line_height(),
            self.source_gutter_width(),
            start_caret_x,
            end_caret_x,
        );

        // Size to the name currently typed so the input grows with editing,
        // but keep it at least a little wider than the original symbol.
        let rename_font_size = crate::theme::scaled(INLINE_RENAME_FONT_SIZE);
        let text_width = state.editor.read(ctx, |editor, _| {
            let text = editor.get_text();
            let layout = unwrapped_line_layout(
                text,
                self.ui_font,
                rename_font_size,
                Properties::default(),
                font_cache,
            );
            caret_x_in_layout(text, &layout, text.chars().count())
        });
        let input_width = inline_rename_input_width(symbol_width, text_width);

        // VS Code-style card: opaque (background-image themes render
        // `background` translucent), the input on top and a small keybinding
        // hint underneath.
        const HINT: &str = "⏎ to Rename, Esc to Cancel";
        let hint_font_size = crate::theme::text_size(10.0);
        let hint_layout = unwrapped_line_layout(
            HINT,
            self.ui_font,
            hint_font_size,
            Properties::default(),
            font_cache,
        );
        let hint_width = caret_x_in_layout(HINT, &hint_layout, HINT.chars().count());
        const CARD_PADDING_H: f32 = 8.0;
        let card_width = input_width.max(hint_width + CARD_PADDING_H * 2.0 + 4.0);
        let input_height = line_height_with_spacing(rename_font_size) + 14.0;

        let input = ConstrainedBox::new(crate::shared_input::shared_input(
            &self.theme,
            &state.editor,
            crate::shared_input::SharedInputOpts {
                height: Some(input_height),
                ..Default::default()
            },
        ))
        .with_width(card_width - CARD_PADDING_H * 2.0)
        .finish();
        let card = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(5.0)
                .with_child(input)
                .with_child(
                    Text::new(HINT, self.ui_font, hint_font_size)
                        .with_color(self.theme.sub_text())
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(CARD_PADDING_H)
        .with_padding_right(CARD_PADDING_H)
        .with_padding_top(7.0)
        .with_padding_bottom(6.0)
        .with_background_color(self.theme.solid_bg())
        .with_border(Border::all(1.0).with_border_color(self.theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();
        // The card must keep its hug-fit width: positioned stack children are
        // laid out with window-size constraints, which would otherwise stretch
        // the unconstrained container across the whole editor.
        let card = ConstrainedBox::new(card).with_width(card_width).finish();
        // Clicks inside the card must not reach the cancel scrim underneath.
        let card = EventHandler::new(card)
            .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
            .finish();
        // Estimated card height (input + spacing + hint + vertical padding) so
        // the scrim can ignore clicks that land inside the card.
        let card_height = 7.0 + input_height + 5.0 + line_height_with_spacing(hint_font_size) + 6.0;
        Some((card, offset, vec2f(card_width, card_height)))
    }

    pub fn set_placeholder(&mut self, placeholder: &str, ctx: &mut ViewContext<Self>) {
        if self.placeholder != placeholder {
            self.placeholder = placeholder.to_string();
            ctx.notify();
        }
    }

    pub fn set_agent_mode(&mut self, agent: bool, ctx: &mut ViewContext<Self>) {
        if self.is_agent_mode != agent {
            self.is_agent_mode = agent;
            ctx.notify();
        }
    }

    /// Turn off shell syntax highlighting for this editor for good — used by the
    /// AI-tool rich input so its text stays plain (white on the tool's dark
    /// chrome) instead of green command coloring. Set once at construction.
    pub fn set_suppress_command_highlight(&mut self, v: bool) {
        self.suppress_command_highlight = v;
    }

    pub fn set_secure(&mut self, secure: bool, ctx: &mut ViewContext<Self>) {
        if self.secure != secure {
            self.secure = secure;
            ctx.notify();
        }
    }

    /// Text used for display and layout. Real buffer is always kept for edit ops.
    pub(crate) fn display_text(&self) -> String {
        if self.secure {
            "•".repeat(self.buffer.chars().count())
        } else {
            self.buffer.clone()
        }
    }

    /// Whether a ghost (autocomplete) suffix is currently showing. Callers use
    /// this to keep the accept key (→) in the editor rather than routing it away.
    pub fn has_ghost_suffix(&self) -> bool {
        self.ghost_suffix.is_some()
    }

    pub fn set_ghost_suffix(&mut self, ghost_suffix: Option<String>, ctx: &mut ViewContext<Self>) {
        let ghost_opt = ghost_suffix.filter(|s| !s.is_empty());
        if self.ghost_suffix != ghost_opt {
            self.ghost_suffix = ghost_opt;
            ctx.notify();
        }
    }

    pub fn insert_text_at_cursor(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        self.snapshot_undo();
        if let Some((start, end)) = self.active_selection_range() {
            let (start, end) = self.expand_range_to_chips(start, end);
            let mut chars = self.char_vec();
            let start = start.min(chars.len());
            let end = end.min(chars.len());
            if start < end {
                chars.drain(start..end);
            }
            self.buffer = chars.into_iter().collect();
            self.cursor = start;
            self.drop_and_rebase_chips(start, end);
        }
        self.cursor = self.cursor.min(self.buffer.chars().count());
        self.cursor = self.clamp_off_chip(self.cursor, true);
        let edit_at = self.cursor;
        let mut chars: Vec<char> = self.buffer.chars().collect();
        let mut inserted_len = 0isize;
        for c in text.chars().filter(|c| *c != '\r') {
            chars.insert(self.cursor, c);
            self.cursor += 1;
            inserted_len += 1;
        }
        self.buffer = chars.into_iter().collect();
        self.invalidate_buffer_content_fully();
        self.rebase_chips(edit_at, inserted_len);
        self.clear_selection();
        // Paste must follow the same parent-update path as typed characters.
        // Settings inputs are `InputMode::Settings`, so they do not emit the
        // generic Changed event; skipping this branch made pasted provider keys
        // appear in the field but never persist or unlock BYOK model choices.
        self.publish_buffer_change(true, ctx);
        if matches!(
            self.mode,
            InputMode::TerminalCommand | InputMode::MultiLine { .. }
        ) {
            if self.follow_bottom.get() {
                // Let ClippedScrollable clamp this to the actual post-layout
                // content height, just like the terminal viewport's tail mode.
                self.scroll_state.scroll_to(f32::MAX.into_pixels());
            }
            let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
            if !self.follow_bottom.get() {
                self.scroll_cursor_into_view_for_layout(&vls, ctx);
            }
        } else {
            self.scroll_horizontal_cursor_into_view(ctx);
        }
        ctx.notify();
    }

    pub fn accept_ghost_suffix(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        let Some(suffix) = self.ghost_suffix.take() else {
            return false;
        };
        if suffix.is_empty() {
            return false;
        }
        self.insert_text_at_cursor(&suffix, ctx);
        true
    }

    /// Returns `true` if this editor currently holds keyboard focus.
    ///
    /// Queries the framework's live focus state via the stored [`FocusHandle`]
    /// rather than a manually maintained shadow bool.
    pub fn is_focused(&self, cx: &AppContext) -> bool {
        self.focus_handle.is_focused(cx)
    }

    pub fn request_focus(&self, ctx: &mut ViewContext<Self>) {
        self.focus_handle.request_focus(ctx);
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.active_selection_range()?;
        let text: String = self.buffer.chars().skip(start).take(end - start).collect();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn set_search_matches(
        &mut self,
        matches: Vec<std::ops::Range<usize>>,
        active: Option<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.search_matches = matches;
        self.active_search_match = active.filter(|index| *index < self.search_matches.len());
        ctx.notify();
    }

    pub fn set_source_line_changes(
        &mut self,
        changes: std::collections::HashMap<usize, LineChangeKind>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.source_line_changes != changes {
            self.source_line_changes = changes;
            ctx.notify();
        }
    }

    pub fn set_git_diff_projection(
        &mut self,
        projection: Option<(DocumentDiff, usize, usize, bool)>,
        ctx: &mut ViewContext<Self>,
    ) {
        let was_active = self.git_diff_projection.is_some();
        let expanded = self
            .git_diff_projection
            .as_ref()
            .map(|projection| projection.expanded_regions.clone())
            .unwrap_or_default();
        self.git_diff_projection =
            projection.map(|(diff, old_line_count, new_line_count, read_only)| {
                let mut projection =
                    GitDiffProjection::new(diff, old_line_count, new_line_count, read_only);
                projection.expanded_regions = expanded;
                projection.rows = projection.diff.display_rows_with_expansions(
                    projection.old_line_count,
                    projection.new_line_count,
                    1,
                    &projection.expanded_regions,
                );
                projection
            });
        // Syntax folding and Git context collapsing are independent display
        // transforms. The focused diff owns row visibility while active.
        self.fold_visibility_cache.get_mut().take();
        if was_active != self.git_diff_projection.is_some() {
            self.scroll_state.scroll_to(0.0.into_pixels());
        }
        ctx.notify();
    }

    pub fn git_diff_projected_row_count(&self) -> Option<usize> {
        self.git_diff_projection
            .as_ref()
            .map(|projection| projection.rows.len())
    }

    pub fn has_git_diff_projection(&self) -> bool {
        self.git_diff_projection.is_some()
    }

    pub fn set_source_diagnostic_lines(
        &mut self,
        diagnostics: std::collections::HashMap<usize, dragent_editor::DiagnosticSeverity>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.source_diagnostic_lines != diagnostics {
            self.source_diagnostic_lines = diagnostics;
            ctx.notify();
        }
    }

    pub fn set_source_diagnostics(
        &mut self,
        diagnostics: Vec<ResolvedDiagnostic>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.source_diagnostics != diagnostics {
            let active = self
                .active_diagnostic
                .and_then(|index| self.source_diagnostics.get(index))
                .cloned();
            self.source_diagnostics = diagnostics;
            self.source_diagnostics_by_line.clear();
            for (index, resolved) in self.source_diagnostics.iter().enumerate() {
                let start = resolved.diagnostic.range.start.line as usize + 1;
                let end = resolved.diagnostic.range.end.line as usize + 1;
                for line in start..=end.max(start) {
                    self.source_diagnostics_by_line
                        .entry(line)
                        .or_default()
                        .push(index);
                }
            }
            self.diagnostic_mouse_states
                .resize_with(self.source_diagnostics.len(), MouseStateHandle::default);
            self.active_diagnostic = active.and_then(|active| {
                self.source_diagnostics
                    .iter()
                    .position(|diagnostic| *diagnostic == active)
            });
            ctx.notify();
        }
    }

    pub(crate) fn open_diagnostic(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(diagnostic) = self.source_diagnostics.get(index) else {
            return;
        };
        self.active_diagnostic = Some(index);
        self.cursor = diagnostic.range.start.min(self.buffer.chars().count());
        self.selection_anchor = None;
        let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        self.scroll_cursor_into_view_for_layout(&visual_lines, ctx);
        self.scroll_source_cursor_horizontally_into_view(ctx);
        ctx.notify();
    }

    pub fn select_char_range(
        &mut self,
        range: std::ops::Range<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let len = self.buffer.chars().count();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        self.selection_anchor = Some(start);
        self.drag_anchor = None;
        self.cursor = end;
        let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        self.scroll_cursor_into_view_for_layout(&visual_lines, ctx);
        self.scroll_source_cursor_horizontally_into_view(ctx);
        ctx.notify();
    }

    /// Navigate to a semantic target, expanding a containing fold and making
    /// the exact symbol range the real editor selection. This is shared by all
    /// URI+range navigation features (definition, implementation, references,
    /// and type/declaration navigation).
    pub fn navigate_to_char_range(
        &mut self,
        range: std::ops::Range<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let len = self.buffer.chars().count();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        if self.document_capabilities.folding {
            let target_line = source_line_number(&self.char_vec(), start);
            let fold_index = self.fold_index();
            if self.fold_state.unfold_line(target_line, &fold_index) {
                self.fold_visibility_cache.get_mut().take();
            }
        }
        self.selection_anchor = (end > start).then_some(start);
        self.drag_anchor = None;
        self.cursor = end;
        self.cursor_visible = true;
        if self.document_capabilities.large_file && self.git_diff_projection.is_none() {
            // Never build or shape every line merely to jump to a distant search
            // result. The compact source index maps the cursor directly to a
            // physical row; rendering will materialize that viewport afterward.
            let index = self.large_source_index();
            let bounds_h = ctx
                .element_position_by_id(&self.viewport_position_id())
                .or_else(|| ctx.element_position_by_id(&self.position_id()))
                .map(|bounds| bounds.height());
            let viewport_h = self.vertical_viewport_height(bounds_h);
            self.scroll_cursor_into_view(
                index.line_for_char(self.cursor),
                index.line_count(),
                viewport_h,
            );
        } else {
            let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
            self.scroll_cursor_into_view_for_layout(&visual_lines, ctx);
        }
        self.scroll_source_cursor_horizontally_into_view(ctx);
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    /// Replace one or more non-overlapping character ranges as a single edit.
    /// Applying from the end keeps every earlier character offset stable and
    /// gives Replace All one undo checkpoint instead of one per match.
    pub fn replace_char_ranges(
        &mut self,
        ranges: &[std::ops::Range<usize>],
        replacement: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        if ranges.is_empty() {
            return;
        }
        self.snapshot_undo();
        let mut chars = self.char_vec();
        let mut ranges = ranges.to_vec();
        ranges.sort_by_key(|range| (range.start, range.end));
        let replacement_chars = replacement.chars().collect::<Vec<_>>();
        for range in ranges.iter().rev() {
            let start = range.start.min(chars.len());
            let end = range.end.min(chars.len()).max(start);
            chars.splice(start..end, replacement_chars.iter().copied());
        }
        self.buffer = chars.into_iter().collect();
        self.invalidate_buffer_content_fully();
        let first = &ranges[0];
        self.cursor = (first.start + replacement_chars.len()).min(self.buffer.chars().count());
        self.clear_selection();
        self.search_matches.clear();
        self.active_search_match = None;
        self.publish_buffer_change(true, ctx);
        let visual_lines = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
        self.scroll_cursor_into_view_for_layout(&visual_lines, ctx);
        self.scroll_source_cursor_horizontally_into_view(ctx);
        ctx.notify();
    }

    pub fn clear_text_selection(&mut self, ctx: &mut ViewContext<Self>) {
        self.clear_selection();
        ctx.notify();
    }

    pub(crate) fn collapse_selection_for_horizontal_arrow(&mut self, move_right: bool) -> bool {
        let Some((start, end)) = self.active_selection_range() else {
            return false;
        };
        self.cursor = if move_right { end } else { start };
        self.clear_selection();
        true
    }
}
