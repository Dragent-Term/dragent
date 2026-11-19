//! Input editor vim and large-source handling.
//!
//! Responsible for: vim dispatch/effects, cursor movement reporting,
//! large-source (virtualized) editing paths.

use super::actions::keystroke_to_char;
use super::model::*;
use super::text::*;
use super::InputEditor;
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

impl InputEditor {
    fn try_handle_vim(&mut self, keystroke: &Keystroke, ctx: &mut ViewContext<Self>) -> bool {
        if !self.vim_enabled || self.mode != InputMode::TerminalCommand {
            return false;
        }
        let (cursor_visual_row, visual_line_count) = if self.buffer.is_empty() {
            (0, 1)
        } else {
            let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
            (Self::cursor_visual_row(self.cursor, &vls), vls.len())
        };
        let vctx = VimContext {
            buffer: &self.buffer,
            cursor: self.cursor,
            cursor_visual_row,
            visual_line_count,
            selection: self.active_selection_range(),
        };
        match self.vim_state.handle_key(keystroke, &vctx) {
            VimDispatch::Consumed => true,
            VimDispatch::Fallthrough => false,
            VimDispatch::Effects(effects) => {
                self.apply_vim_effects(effects, ctx);
                true
            }
        }
    }

    fn notify_cursor_moved(&mut self, ctx: &mut ViewContext<Self>) {
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        if self.mode == InputMode::TerminalCommand {
            ctx.emit(InputEditorEvent::Changed {
                text: self.buffer.clone(),
                cursor: self.cursor,
            });
        }
        ctx.notify();
    }

    fn apply_vim_effects(&mut self, effects: Vec<VimEffect>, ctx: &mut ViewContext<Self>) {
        let mut cursor_moved = false;
        for effect in effects {
            match effect {
                VimEffect::SetMode(mode) => self.vim_state.mode = mode,
                VimEffect::MoveCursor(pos) => {
                    self.clear_selection();
                    let len = self.buffer.chars().count();
                    let clamped = if matches!(self.vim_state.mode, VimMode::Normal) && len > 0 {
                        pos.min(len - 1)
                    } else {
                        pos.min(len)
                    };
                    if clamped != self.cursor {
                        cursor_moved = true;
                    }
                    self.cursor = clamped;
                }
                VimEffect::DeleteRange(start, end) => {
                    self.delete_range(start, end, ctx);
                    if matches!(self.vim_state.mode, VimMode::Normal) {
                        let len = self.buffer.chars().count();
                        if len > 0 && self.cursor >= len {
                            self.cursor = len - 1;
                        }
                    }
                }
                VimEffect::Yank(text) => self.vim_state.register = text,
                VimEffect::Paste(text) => self.insert_text_at_cursor(&text, ctx),
                VimEffect::Undo => self.handle_undo(ctx),
                VimEffect::SelectRange(start, end) => {
                    self.selection_anchor = Some(start.min(end));
                    self.cursor = start.max(end);
                }
                VimEffect::ClearSelection => self.clear_selection(),
                VimEffect::HistoryUp => {
                    ctx.emit(InputEditorEvent::HistoryNavigate { previous: true });
                }
                VimEffect::HistoryDown => {
                    ctx.emit(InputEditorEvent::HistoryNavigate { previous: false });
                }
                VimEffect::MoveVisualLine(delta) => {
                    if delta < 0 {
                        self.handle_cursor_up(ctx);
                    } else if delta > 0 {
                        self.handle_cursor_down(ctx);
                    }
                    cursor_moved = true;
                }
            }
        }
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        if cursor_moved {
            let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
            self.scroll_cursor_into_view_for_layout(&vls, ctx);
            self.scroll_horizontal_cursor_into_view(ctx);
        }
        if self.vim_enabled {
            self.sync_vim_status_hint();
        }
        if cursor_moved {
            self.notify_cursor_moved(ctx);
        } else {
            ctx.notify();
        }
    }

    pub(crate) fn handle_keydown(&mut self, keystroke: &Keystroke, ctx: &mut ViewContext<Self>) {
        if !self.focus_handle.is_focused(ctx) {
            return;
        }
        if self
            .git_diff_projection
            .as_ref()
            .is_some_and(|projection| projection.read_only)
        {
            let navigation_or_copy = matches!(
                keystroke.key.as_str(),
                "left"
                    | "right"
                    | "up"
                    | "down"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
                    | "escape"
            ) || ((keystroke.cmd || keystroke.ctrl)
                && matches!(keystroke.key.as_str(), "a" | "c"));
            if !navigation_or_copy {
                return;
            }
        }
        if self.is_source_editor() && self.document_capabilities.large_file {
            self.handle_large_source_keydown(keystroke, ctx);
            return;
        }
        // Keep every keyboard edit path within the current character buffer.
        self.cursor = self.cursor.min(self.buffer.chars().count());
        if self.try_handle_vim(keystroke, ctx) {
            return;
        }
        if let Some(at) = self.active_mention {
            let no_mods = !keystroke.cmd && !keystroke.ctrl && !keystroke.alt && !keystroke.meta;
            if no_mods && keystroke.key == "up" {
                if !self.mention_candidates.is_empty() {
                    self.mention_selected = if self.mention_selected == 0 {
                        self.mention_candidates.len() - 1
                    } else {
                        self.mention_selected - 1
                    };
                    ctx.notify();
                }
                return;
            }
            if no_mods && keystroke.key == "down" {
                if !self.mention_candidates.is_empty() {
                    self.mention_selected =
                        (self.mention_selected + 1) % self.mention_candidates.len();
                    ctx.notify();
                }
                return;
            }
            if keystroke.key == "escape" {
                self.close_mention(ctx);
                ctx.notify();
                return;
            }
            if no_mods && !keystroke.shift && keystroke.key == "enter" {
                if let Some(entry) = self.mention_candidates.get(self.mention_selected).cloned() {
                    self.insert_chip(at, entry.path, entry.label, entry.is_dir, ctx);
                } else {
                    self.close_mention(ctx);
                    ctx.notify();
                }
                return;
            }
        }
        // Undo: Ctrl+Z or Cmd+Z
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "z" && !keystroke.shift {
            self.handle_undo(ctx);
            return;
        }
        // Redo: Ctrl+Shift+Z, Cmd+Shift+Z, or Ctrl+Y
        if (keystroke.cmd || keystroke.ctrl)
            && (keystroke.key == "y" || (keystroke.key == "z" && keystroke.shift))
        {
            self.handle_redo(ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "c" {
            self.copy_selection(ctx);
            return;
        }
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "v" {
            self.paste_clipboard(ctx);
            return;
        }
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "x" {
            self.cut_selection(ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "a" {
            self.select_all(ctx);
            return;
        }
        // Line-level commands (move/delete/duplicate) only make sense where
        // a buffer can actually have more than one line — not the
        // single-line fields (Search, rename, settings values, …).
        let supports_line_ops = self.is_source_editor()
            || matches!(
                self.mode,
                InputMode::TerminalCommand | InputMode::MultiLine { .. }
            );
        if supports_line_ops && keystroke.alt && !keystroke.shift && keystroke.key == "up" {
            self.handle_move_line(true, ctx);
            return;
        }
        if supports_line_ops && keystroke.alt && !keystroke.shift && keystroke.key == "down" {
            self.handle_move_line(false, ctx);
            return;
        }
        if supports_line_ops && keystroke.shift && keystroke.alt && keystroke.key == "up" {
            self.handle_duplicate_line(false, ctx);
            return;
        }
        if supports_line_ops && keystroke.shift && keystroke.alt && keystroke.key == "down" {
            self.handle_duplicate_line(true, ctx);
            return;
        }
        if supports_line_ops
            && (keystroke.cmd || keystroke.ctrl)
            && keystroke.shift
            && keystroke.key == "k"
        {
            self.handle_delete_line(ctx);
            return;
        }
        if self.mode == InputMode::TerminalCommand && keystroke.alt && keystroke.key == "." {
            ctx.emit(InputEditorEvent::InsertLastWord);
            return;
        }

        let extend = keystroke.shift;
        // Do not materialize the complete buffer as `Vec<char>` for every
        // keypress. Printable insertion, backspace and simple arrows do not
        // need it; on a megabyte source file this copy was paid before the
        // character could appear.
        let len = self.buffer.chars().count();

        // Native editor behavior: an unmodified horizontal arrow collapses a
        // selection to that edge without moving one additional character.
        // This lives in the shared editor so terminal, AI, search, settings,
        // and source inputs all behave consistently.
        if !extend && matches!(keystroke.key.as_str(), "left" | "right") {
            if self.collapse_selection_for_horizontal_arrow(keystroke.key == "right") {
                self.cursor_visible = true;
                self.schedule_cursor_blink(ctx);
                ctx.notify();
                return;
            }
        }

        if self.is_source_editor() && keystroke.key == "tab" {
            self.handle_source_tab(keystroke.shift, ctx);
            return;
        }

        if keystroke.key == "home" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(line_start(&chars, self.cursor), false);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if keystroke.key == "end" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(line_end(&chars, self.cursor), true);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "left" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(line_start(&chars, self.cursor), false);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "right" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(line_end(&chars, self.cursor), true);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if keystroke.alt && keystroke.key == "left" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(word_start(&chars, self.cursor), false);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if keystroke.alt && keystroke.key == "right" {
            let chars = self.char_vec();
            let tgt = self.clamp_off_chip(word_end(&chars, self.cursor), true);
            self.move_cursor_with_selection(tgt, extend, ctx);
            return;
        }
        if (keystroke.alt || keystroke.ctrl) && keystroke.key == "backspace" {
            if let Some((start, end)) = self.active_selection_range() {
                self.delete_range(start, end, ctx);
            } else {
                let chars = self.char_vec();
                let (start, end) =
                    self.expand_range_to_chips(word_start(&chars, self.cursor), self.cursor);
                self.delete_range(start, end, ctx);
            }
            return;
        }
        if (keystroke.alt || keystroke.ctrl) && keystroke.key == "delete" {
            if let Some((start, end)) = self.active_selection_range() {
                self.delete_range(start, end, ctx);
            } else {
                let chars = self.char_vec();
                let (start, end) =
                    self.expand_range_to_chips(self.cursor, word_end(&chars, self.cursor));
                self.delete_range(start, end, ctx);
            }
            return;
        }

        let mut changed = false;
        let mut cursor_moved = false;
        if keystroke.key == "escape" {
            match &self.mode {
                InputMode::RenameTab(_) => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::CancelRenameTab);
                }
                InputMode::RenameWorkspaceTab(_) => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::CancelRenameWorkspaceTab);
                }
                InputMode::LanguageRename { .. } => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::CancelLanguageRename);
                }
                InputMode::Search => {
                    self.buffer.clear();
                    self.invalidate_buffer_content();
                    self.cursor = 0;
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::SidebarSearchInput {
                        text: String::new(),
                    });
                    ctx.notify();
                }
                InputMode::TerminalCommand => {}
                InputMode::Settings(_) => {}
                InputMode::MultiLine { .. } => {}
                InputMode::Plain { key } if key == "explorer_entry_name" => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::ExplorerCancelPrompt);
                }
                InputMode::Plain { .. } => {}
            }
            return;
        }
        if keystroke.key == "enter" {
            match &self.mode {
                InputMode::RenameTab(tab_id) => {
                    let tab_id = *tab_id;
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::CommitRenameTab { tab_id });
                    return;
                }
                InputMode::RenameWorkspaceTab(workspace_tab_id) => {
                    let workspace_tab_id = *workspace_tab_id;
                    ctx.dispatch_typed_action_deferred(
                        WorkspaceAction::CommitRenameWorkspaceTab { workspace_tab_id },
                    );
                    return;
                }
                InputMode::LanguageRename { cursor } => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::RequestLanguageRename {
                        cursor: *cursor,
                        new_name: self.buffer.trim().to_string(),
                    });
                    return;
                }
                InputMode::Plain { key } if key == "explorer_entry_name" => {
                    ctx.dispatch_typed_action_deferred(WorkspaceAction::ExplorerCommitPrompt);
                    return;
                }
                InputMode::Search | InputMode::Settings(_) | InputMode::Plain { .. } => {
                    return;
                }
                InputMode::TerminalCommand => {
                    if keystroke.shift {
                        self.snapshot_undo();
                        let mut chars: Vec<char> = self.buffer.chars().collect();
                        chars.insert(self.cursor, '\n');
                        self.buffer = chars.into_iter().collect();
                        self.cursor += 1;
                        changed = true;
                    } else {
                        return;
                    }
                }
                InputMode::MultiLine { .. } => {
                    // Enter (with or without shift) always inserts a newline.
                    // Source editors run the smart indentation engine: the
                    // lexical scanner derives the new line's indentation from
                    // the unclosed delimiter stack (strings/comments are
                    // opaque), language rules (Python colon, JSX tags) and
                    // the previous line's literal indent — and expands a
                    // delimiter pair when the cursor sits between one. This
                    // is one undoable transaction (newline + indent + closer
                    // relocation spliced together). Large-file mode keeps the
                    // cheap previous-line fallback so Enter never waits on
                    // heavy processing.
                    self.snapshot_undo();
                    let mut chars: Vec<char> = self.buffer.chars().collect();
                    let smart = self.is_source_editor() && !self.document_capabilities.large_file;
                    let unit = self.indent_unit();
                    let newline = if smart {
                        Some(crate::workspace::smart_indent::newline_indent_with(
                            &chars,
                            self.cursor,
                            self.source_language,
                            &unit,
                        ))
                    } else {
                        None
                    };
                    let (indent, closer_line) = match newline {
                        Some(result) => (result.indent, result.closer_line.unwrap_or_default()),
                        None => (
                            self.is_source_editor()
                                .then(|| {
                                    super::text::source_newline_indent_with(
                                        &chars,
                                        self.cursor,
                                        self.source_language,
                                        &unit,
                                    )
                                })
                                .unwrap_or_default(),
                            String::new(),
                        ),
                    };
                    chars.splice(
                        self.cursor..self.cursor,
                        std::iter::once('\n')
                            .chain(indent.chars())
                            .chain(closer_line.chars()),
                    );
                    self.buffer = chars.into_iter().collect();
                    self.cursor += 1 + indent.chars().count();
                    changed = true;
                }
            }
        } else if keystroke.key == "backspace" {
            // Cursor and selection positions are character offsets. Clamp them
            // before any range operation because async editor updates
            // can otherwise leave one stale position after a multi-byte paste.
            let buffer_len = self.buffer.chars().count();
            self.cursor = self.cursor.min(buffer_len);
            if let Some((start, end)) = self.active_selection_range() {
                self.snapshot_undo();
                let byte_start = byte_index_for_char(&self.buffer, start);
                let byte_end = byte_index_for_char(&self.buffer, end);
                self.buffer.replace_range(byte_start..byte_end, "");
                self.cursor = start;
                self.clear_selection();
                self.drop_and_rebase_chips(start, end);
                changed = true;
            } else if self.cursor > 0 {
                self.snapshot_undo();
                // A chip ending exactly at the cursor is deleted whole; otherwise
                // fall back to the normal single-char removal.
                let chip_end = self
                    .chip_spans
                    .iter()
                    .find(|c| c.range.end == self.cursor)
                    .map(|c| c.range.start);
                let del_start = chip_end.unwrap_or(self.cursor - 1);
                let del_end = self.cursor;
                let del_start = del_start.min(del_end);
                let byte_start = byte_index_for_char(&self.buffer, del_start);
                let byte_end = byte_index_for_char(&self.buffer, del_end);
                self.buffer.replace_range(byte_start..byte_end, "");
                self.cursor = del_start;
                self.drop_and_rebase_chips(del_start, del_end);
                changed = true;
            }
        } else if keystroke.key == "left" {
            if !keystroke.shift && self.collapse_selection_for_horizontal_arrow(false) {
                cursor_moved = true;
            } else {
                let naive = self.cursor.saturating_sub(1);
                let clamped = self.clamp_off_chip(naive, false);
                if keystroke.shift {
                    if self.selection_anchor.is_none() {
                        self.selection_anchor = Some(self.cursor);
                    }
                    if self.cursor > 0 {
                        self.cursor = clamped;
                        cursor_moved = true;
                    }
                } else {
                    self.clear_selection();
                    if self.cursor > 0 {
                        self.cursor = clamped;
                        cursor_moved = true;
                    }
                }
            }
        } else if keystroke.key == "right" {
            let len = self.buffer.chars().count();
            // IntelliSense-style: with the cursor at end-of-buffer and a ghost
            // suggestion showing, a bare → accepts it (same as the on-screen
            // accept arrow) instead of being a no-op. Any selection or Shift
            // still falls through to normal caret movement.
            if !keystroke.shift
                && self.active_selection_range().is_none()
                && self.cursor >= len
                && self.ghost_suffix.is_some()
            {
                ctx.emit(InputEditorEvent::AcceptGhost);
                return;
            }
            if !keystroke.shift && self.collapse_selection_for_horizontal_arrow(true) {
                cursor_moved = true;
            } else if keystroke.shift {
                let naive = (self.cursor + 1).min(len);
                let clamped = self.clamp_off_chip(naive, true);
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor);
                }
                if self.cursor < len {
                    self.cursor = clamped;
                    cursor_moved = true;
                }
            } else {
                let naive = (self.cursor + 1).min(len);
                let clamped = self.clamp_off_chip(naive, true);
                if self.cursor < len {
                    self.cursor = clamped;
                    cursor_moved = true;
                }
            }
        } else if keystroke.key == "up" {
            self.handle_cursor_up(ctx);
            return;
        } else if keystroke.key == "down" {
            self.handle_cursor_down(ctx);
            return;
        } else if let Some(c) = keystroke_to_char(keystroke) {
            self.insert_char(c, ctx);
            return;
        }

        if cursor_moved {}
        if changed || cursor_moved {
            if changed {
                self.definition_hover = None;
                self.definition_click_candidate = None;
                self.invalidate_buffer_content();
            }
            self.cursor_visible = true;
            self.schedule_cursor_blink(ctx);

            self.publish_buffer_change(changed, ctx);
            if changed {
                self.schedule_fold_refresh(ctx);
            }
            if self.is_agent_mode {
                self.refresh_mention_state(ctx);
            }
            ctx.notify();
        }
    }

    fn byte_offset_for_char(&self, char_index: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_index)
            .map(|(byte, _)| byte)
            .unwrap_or(self.buffer.len())
    }

    fn large_source_line_bounds(&self, cursor: usize) -> (usize, usize, usize) {
        let index = self.large_source_index();
        let line_number = index.line_for_char(cursor);
        let (_, _, length) = index
            .line_range(&self.buffer, line_number)
            .unwrap_or((0, 0, 0));
        let line_start = index
            .starts
            .get(line_number)
            .map(|(_, start)| *start)
            .unwrap_or(0);
        (line_start, line_start + length, line_number)
    }

    fn large_source_replace_chars(
        &mut self,
        start: usize,
        end: usize,
        replacement: &str,
        ctx: &mut ViewContext<Self>,
    ) {
        let start = start.min(self.buffer.chars().count());
        let end = end.min(self.buffer.chars().count()).max(start);
        let byte_start = self.byte_offset_for_char(start);
        let byte_end = self.byte_offset_for_char(end);
        self.snapshot_undo();
        self.buffer.replace_range(byte_start..byte_end, replacement);
        self.cursor = start + replacement.chars().count();
        self.clear_selection();
        self.invalidate_buffer_content();
        self.apply_edit(ctx);
    }

    fn large_source_insert(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        if let Some((start, end)) = self.active_selection_range() {
            self.large_source_replace_chars(start, end, text, ctx);
            return;
        }
        let cursor = self.cursor.min(self.buffer.chars().count());
        let byte = self.byte_offset_for_char(cursor);
        let normalized = text.replace('\r', "");
        self.snapshot_undo();
        self.buffer.insert_str(byte, &normalized);
        self.cursor = cursor + normalized.chars().count();
        self.invalidate_buffer_content();
        self.apply_edit(ctx);
    }

    fn large_source_indent_for_newline(&self) -> String {
        let (start, _, _) = self.large_source_line_bounds(self.cursor);
        let cursor_byte = self.byte_offset_for_char(self.cursor);
        let prefix = self.buffer.get(start..cursor_byte).unwrap_or_default();
        let base: String = prefix
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect();
        let trimmed = prefix.trim_end();
        let opens_block =
            !matches!(
                self.source_language,
                Language::Python | Language::Markdown | Language::PlainText
            ) && (trimmed.ends_with('{') || trimmed.ends_with('[') || trimmed.ends_with('('));
        if opens_block {
            format!("{base}{tab}", tab = self.indent_unit())
        } else {
            base
        }
    }

    fn handle_large_source_keydown(&mut self, keystroke: &Keystroke, ctx: &mut ViewContext<Self>) {
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "z" && !keystroke.shift {
            self.handle_undo(ctx);
            return;
        }
        if (keystroke.cmd || keystroke.ctrl)
            && (keystroke.key == "y" || (keystroke.key == "z" && keystroke.shift))
        {
            self.handle_redo(ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "c" {
            self.copy_selection(ctx);
            return;
        }
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "v" {
            self.paste_clipboard(ctx);
            return;
        }
        if (keystroke.cmd || keystroke.ctrl) && keystroke.key == "x" {
            self.cut_selection(ctx);
            return;
        }
        if keystroke.cmd && keystroke.key == "a" {
            self.select_all(ctx);
            return;
        }

        let len = self.buffer.chars().count();
        self.cursor = self.cursor.min(len);
        if keystroke.key == "home" || (keystroke.cmd && keystroke.key == "left") {
            let (start, _, _) = self.large_source_line_bounds(self.cursor);
            self.move_cursor_with_selection(start, keystroke.shift, ctx);
            return;
        }
        if keystroke.key == "end" || (keystroke.cmd && keystroke.key == "right") {
            let (_, end, _) = self.large_source_line_bounds(self.cursor);
            self.move_cursor_with_selection(end, keystroke.shift, ctx);
            return;
        }
        if keystroke.key == "left" || keystroke.key == "right" {
            let moving_right = keystroke.key == "right";
            if !keystroke.shift && self.collapse_selection_for_horizontal_arrow(moving_right) {
                self.notify_cursor_moved(ctx);
                return;
            }
            let next = if moving_right {
                self.cursor.saturating_add(1).min(len)
            } else {
                self.cursor.saturating_sub(1)
            };
            self.move_cursor_with_selection(next, keystroke.shift, ctx);
            return;
        }
        if keystroke.key == "up" || keystroke.key == "down" {
            let index = self.large_source_index();
            let current_line = index.line_for_char(self.cursor);
            let target_line = if keystroke.key == "up" {
                current_line.saturating_sub(1)
            } else {
                (current_line + 1).min(index.line_count().saturating_sub(1))
            };
            let (current_start, _, _) = self.large_source_line_bounds(self.cursor);
            let column = self.cursor.saturating_sub(current_start);
            let target_start = index.starts[target_line].1;
            let target_len = index
                .line_range(&self.buffer, target_line)
                .map(|(_, _, length)| length)
                .unwrap_or(0);
            self.move_cursor_with_selection(
                target_start + column.min(target_len),
                keystroke.shift,
                ctx,
            );
            return;
        }
        if keystroke.key == "backspace" {
            if let Some((start, end)) = self.active_selection_range() {
                self.large_source_replace_chars(start, end, "", ctx);
            } else if self.cursor > 0 {
                self.large_source_replace_chars(self.cursor - 1, self.cursor, "", ctx);
            }
            return;
        }
        if keystroke.key == "delete" {
            if let Some((start, end)) = self.active_selection_range() {
                self.large_source_replace_chars(start, end, "", ctx);
            } else if self.cursor < len {
                self.large_source_replace_chars(self.cursor, self.cursor + 1, "", ctx);
            }
            return;
        }
        if keystroke.key == "enter" {
            self.large_source_insert(
                &format!("\n{}", self.large_source_indent_for_newline()),
                ctx,
            );
            return;
        }
        if keystroke.key == "tab" {
            let unit = self.indent_unit();
            self.large_source_insert(&unit, ctx);
            return;
        }
        if let Some(character) = keystroke_to_char(keystroke) {
            self.large_source_insert(&character.to_string(), ctx);
        }
    }
}
