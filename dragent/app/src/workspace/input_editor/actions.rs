//! Input editor actions.
//!
//! Responsible for: InputEditorAction dispatch, keystroke helpers.
//! Translates actions into editor operations.

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

impl TypedActionView for InputEditor {
    type Action = InputEditorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        if let Some(bounds) = ctx.element_position_by_id(&self.position_id()) {
            self.schedule_width_reflow(self.text_layout_width(bounds.width()), ctx);
        }
        if let Some(bounds) = ctx.element_position_by_id(&self.viewport_position_id()) {
            self.available_height.set(bounds.height());
        }
        // Scrolling a source file does not move its caret. Remember whether
        // the caret was visible before a key event so a deliberately scrolled
        // viewport is not snapped back to an older edit position on every
        // keystroke. When the caret starts visible, normal editor following is
        // retained (including after inserting a new line at the viewport edge).
        let source_cursor_was_visible = (self.is_source_editor()
            && matches!(action, InputEditorAction::KeyDown(_)))
        .then(|| self.source_cursor_is_visible(ctx));
        match action {
            InputEditorAction::ContextMenuAt(position) => {
                ctx.focus_self();
                let clicked = self
                    .char_index_at_position(*position, ctx)
                    .map(|index| self.clamp_off_chip_nearest(index))
                    .unwrap_or(self.cursor);
                let preserve_selection = self
                    .active_selection_range()
                    .is_some_and(|(start, end)| clicked >= start && clicked <= end);
                if !preserve_selection {
                    self.clear_selection();
                    self.cursor = clicked.min(self.buffer.chars().count());
                }
                // Event positions are window-space, while WindowByPosition
                // expects an offset from this view's parent. Keeping the
                // conversion here prevents the editor's own panel origin from
                // being added twice when the popup is painted.
                self.context_menu_cursor = Some(clicked);
                self.context_menu_position = Some(
                    ctx.element_position_by_id(&self.position_id())
                        .map(|bounds| *position - bounds.origin())
                        .unwrap_or(*position),
                );
                self.context_menu_selected = self
                    .context_menu_entries()
                    .iter()
                    .position(|entry| {
                        matches!(entry, ContextMenuEntry::Action { enabled: true, .. })
                    })
                    .unwrap_or(0);
                ctx.notify();
            }
            InputEditorAction::CloseContextMenu => {
                if self.context_menu_position.take().is_some() {
                    self.context_menu_cursor = None;
                    self.context_menu_selected = 0;
                    ctx.notify();
                }
            }
            InputEditorAction::MoveContextMenuSelection { direction } => {
                if self.context_menu_position.is_some() {
                    self.context_menu_selected = self.next_context_menu_index(*direction);
                    ctx.notify();
                }
            }
            InputEditorAction::ActivateContextMenuSelection => {
                if self.context_menu_position.is_some() {
                    if let Some(command) = self.selected_context_menu_command() {
                        self.execute_context_menu_command(command, ctx);
                    }
                }
            }
            InputEditorAction::ContextMenuCommand(command) => {
                self.execute_context_menu_command(*command, ctx);
            }
            InputEditorAction::KeyDown(keystroke) => {
                self.handle_keydown(keystroke, ctx);
            }
            InputEditorAction::ClickAt(click_pos) => {
                if !self.handle_gutter_click(*click_pos, ctx) {
                    self.handle_click_at(*click_pos, ctx);
                }
            }
            InputEditorAction::DoubleClickAt(click_pos) => {
                self.handle_double_click_at(*click_pos, ctx);
            }
            InputEditorAction::TripleClickAt(click_pos) => {
                self.handle_triple_click_at(*click_pos, ctx);
            }
            InputEditorAction::ExpandGitDiffRegion { id, direction } => {
                if let Some(projection) = self.git_diff_projection.as_mut() {
                    projection.expand(*id, *direction);
                    ctx.emit(InputEditorEvent::GitDiffProjectionRowsChanged {
                        rows: projection.rows.len(),
                    });
                    ctx.notify();
                }
            }
            InputEditorAction::DragAt(drag_pos) => {
                self.handle_drag_at(*drag_pos, ctx);
            }
            InputEditorAction::ReleaseAt(release_pos) => {
                self.handle_release_at(*release_pos, ctx);
            }
            InputEditorAction::DefinitionHoverAt(position) => {
                self.update_definition_hover_at(*position, ctx);
            }
            InputEditorAction::ClearDefinitionHover => {
                self.last_pointer_position = None;
                self.clear_definition_hover(ctx);
            }
            InputEditorAction::DefinitionModifierHeld(held) => {
                if self.definition_modifier_held != *held {
                    self.definition_modifier_held = *held;
                    log::info!(
                        "[definition-link] modifier {}",
                        if *held { "pressed" } else { "released" }
                    );
                    if *held {
                        if let Some(position) = self.last_pointer_position {
                            self.update_definition_hover_at(position, ctx);
                        } else {
                            log::info!(
                                "[definition-link] modifier pressed but no pointer position known yet"
                            );
                        }
                    } else {
                        self.clear_definition_hover(ctx);
                    }
                }
            }
            InputEditorAction::ScrollWheel(delta_y) => {
                if self.is_source_editor() {
                    return;
                }
                if *delta_y > 0.0 {
                    self.follow_bottom.set(false);
                } else if *delta_y < 0.0 {
                    let visual_lines =
                        self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
                    let viewport_h = self.vertical_viewport_height(
                        ctx.element_position_by_id(&self.viewport_position_id())
                            .map(|bounds| bounds.height()),
                    );
                    let current = self.clamped_vertical_scroll(visual_lines.len(), viewport_h);
                    let line_h = self.line_height();
                    let content_h = visual_lines.len().max(1) as f32 * line_h;
                    let max_scroll = (content_h - viewport_h).max(0.0);
                    if current - *delta_y >= max_scroll - 2.0 {
                        self.follow_bottom.set(true);
                    }
                }
            }
            InputEditorAction::AcceptGhost => {
                ctx.emit(InputEditorEvent::AcceptGhost);
            }
            InputEditorAction::ClickMentionEntry(index) => {
                if let (Some(at), Some(entry)) = (
                    self.active_mention,
                    self.mention_candidates.get(*index).cloned(),
                ) {
                    self.insert_chip(at, entry.path, entry.label, entry.is_dir, ctx);
                }
            }
            InputEditorAction::OpenDiagnostic(index) => self.open_diagnostic(*index, ctx),
            InputEditorAction::PreviousDiagnostic => {
                if !self.source_diagnostics.is_empty() {
                    let index = self
                        .active_diagnostic
                        .unwrap_or(0)
                        .checked_sub(1)
                        .unwrap_or(self.source_diagnostics.len() - 1);
                    self.open_diagnostic(index, ctx);
                }
            }
            InputEditorAction::NextDiagnostic => {
                if !self.source_diagnostics.is_empty() {
                    let index =
                        (self.active_diagnostic.unwrap_or(0) + 1) % self.source_diagnostics.len();
                    self.open_diagnostic(index, ctx);
                }
            }
            InputEditorAction::CloseDiagnostic => {
                self.active_diagnostic = None;
                ctx.notify();
            }
        }
        // Keyboard edits/navigation should keep the cursor visible. Mouse
        // positioning must preserve the current viewport, like terminal
        // scrollback: selecting older content must not jump to that content's
        // logical position or to the tail.
        let uses_visual = matches!(
            self.mode,
            InputMode::TerminalCommand | InputMode::MultiLine { .. }
        );
        let is_mouse_positioning = matches!(
            action,
            InputEditorAction::ClickAt(_)
                | InputEditorAction::DoubleClickAt(_)
                | InputEditorAction::TripleClickAt(_)
                | InputEditorAction::DragAt(_)
                | InputEditorAction::ReleaseAt(_)
                | InputEditorAction::DefinitionHoverAt(_)
                | InputEditorAction::ClearDefinitionHover
                | InputEditorAction::DefinitionModifierHeld(_)
                | InputEditorAction::ExpandGitDiffRegion { .. }
                | InputEditorAction::ScrollWheel(_)
        );
        if self.buffer.is_empty() {
            self.scroll_state.scroll_to(0.0.into_pixels());
        } else if uses_visual {
            if self.is_source_editor() {
                if !is_mouse_positioning && source_cursor_was_visible.unwrap_or(true) {
                    if self.document_capabilities.large_file {
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
                        let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
                        self.scroll_cursor_into_view_for_layout(&vls, ctx);
                    }
                    self.scroll_source_cursor_horizontally_into_view(ctx);
                }
            } else if self.follow_bottom.get() {
                self.scroll_state.scroll_to(f32::MAX.into_pixels());
            } else if !is_mouse_positioning {
                let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
                self.scroll_cursor_into_view_for_layout(&vls, ctx);
            }
        } else {
            self.scroll_horizontal_cursor_into_view(ctx);
        }
    }
}

pub fn keystroke_to_char(keystroke: &Keystroke) -> Option<char> {
    if keystroke.cmd || keystroke.ctrl || keystroke.alt || keystroke.meta {
        return None;
    }

    if keystroke.key == "space" {
        return Some(' ');
    }

    if keystroke.key.chars().count() == 1 {
        let mut c = keystroke.key.chars().next().unwrap();
        // Newline characters from IME text-input events must not be re-inserted here;
        // shift+enter is already handled by the "enter" branch above.
        if c == '\n' || c == '\r' {
            return None;
        }
        // Shift and CapsLock both uppercase letters; together they cancel.
        if (keystroke.shift ^ keystroke.caps_lock) && c.is_ascii_lowercase() {
            c = c.to_ascii_uppercase();
        }
        return Some(c);
    }
    None
}
