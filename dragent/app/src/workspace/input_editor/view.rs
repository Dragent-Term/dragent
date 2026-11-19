//! Input editor View implementation.
//!
//! Responsible for: ui_name, render dispatch, focus/blur handling.
//! Presentation only.

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

impl View for InputEditor {
    fn ui_name() -> &'static str {
        "InputEditor"
    }

    fn render(&self, ctx: &warpui::AppContext) -> Box<dyn Element> {
        let is_multiline = matches!(self.mode, InputMode::MultiLine { .. });
        let is_review_instructions = matches!(&self.mode,InputMode::MultiLine{key,..} if key=="review_instructions" || key=="preview_review");
        let is_source_editor = self.is_source_editor();
        let mut row = Flex::row()
            // A source editor is a viewport, not an intrinsically-sized input.
            // Centering on the cross axis let its dual-axis scrollable collapse
            // to the rendered lines near EOF, so the horizontal scrollbar sat
            // above a large empty strip. Stretch it to the allocated document
            // body so the scrollbar remains pinned to the bottom edge.
            .with_cross_axis_alignment(if is_source_editor {
                CrossAxisAlignment::Stretch
            } else {
                CrossAxisAlignment::Center
            })
            .with_main_axis_size(MainAxisSize::Max);

        if self.buffer.is_empty() {
            row = row.with_child(Box::new(Expanded::new(1.0, self.render_empty_state(ctx))));
        } else if self.mode == InputMode::TerminalCommand || is_multiline {
            let canvas = if is_source_editor {
                self.viewport_handle.as_ref().map(|handle| {
                    super::viewport::SourceEditorViewport::new(handle.clone()).finish()
                })
            } else {
                None
            }
            .unwrap_or_else(|| self.render_command_canvas(ctx));
            row = row.with_child(Box::new(Expanded::new(1.0, canvas)));
        } else {
            row = row.with_child(Box::new(Expanded::new(1.0, self.render_plain_canvas(ctx))));
        }

        let child = Container::new(row.finish()).finish();
        let child = SavePosition::new(child, &self.position_id()).finish();
        let child = match (self.render_mention_popup(), self.mention_anchor_offset(ctx)) {
            (Some(popup), Some(anchor)) => {
                let offset = OffsetPositioning::offset_from_parent(
                    anchor,
                    ParentOffsetBounds::Unbounded,
                    ParentAnchor::TopLeft,
                    ChildAnchor::BottomLeft,
                );
                Stack::new()
                    .with_child(child)
                    .with_positioned_child(popup, offset)
                    .finish()
            }
            _ => child,
        };
        let mut child = if let Some(position) = self.context_menu_position {
            let menu = self.render_context_menu();
            let offset = OffsetPositioning::offset_from_parent(
                position,
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            );
            Stack::new()
                .with_child(child)
                .with_positioned_overlay_child(menu, offset)
                .finish()
        } else {
            child
        };
        // Inline rename (VS Code-style): an invisible scrim over the whole
        // editor cancels on outside clicks, while the card itself renders
        // above it, anchored directly under the symbol being renamed. The card
        // is a normal positioned child (NOT an overlay-layer child): nested
        // clipped scrollables inside an overlay layer fail to paint their
        // content, which hid the rename editor's text and selection.
        if let Some((card, offset, card_size)) = self.render_inline_rename_overlay(ctx) {
            let position_id = self.position_id();
            let card_rect = RectF::new(offset, card_size);
            let scrim = EventHandler::new(
                Container::new(Align::new(Empty::new().finish()).finish())
                    .with_background_color(ColorU::new(0, 0, 0, 0))
                    .finish(),
            )
            .on_left_mouse_down(move |event_ctx, _app, pos| {
                // Release builds broadcast events to every painted stack
                // child, so clicks inside the card reach this scrim too.
                // Only cancel when the click lands outside the card.
                let inside_card = event_ctx
                    .element_position_by_id(&position_id)
                    .is_some_and(|bounds| card_rect.contains_point(pos - bounds.origin()));
                if !inside_card {
                    event_ctx.dispatch_typed_action(WorkspaceAction::CancelLanguageRename);
                }
                DispatchEventResult::StopPropagation
            })
            .finish();
            child = Stack::new()
                .with_child(child)
                .with_child(scrim)
                .with_positioned_child(
                    card,
                    OffsetPositioning::offset_from_parent(
                        offset,
                        ParentOffsetBounds::Unbounded,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                )
                .finish();
        }

        // Capture focus state at render time so the on_keydown closure can guard
        // against consuming keystrokes when this editor is not the focused view.
        // Without this guard every InputEditor (including unfocused ones) would
        // intercept keystrokes and return StopPropagation, preventing other
        // focused editors (sidebar search, rename input) from receiving input.
        let is_focused = self.focus_handle.is_focused(ctx);
        let has_input_selection = self.active_selection_range().is_some();
        let is_terminal_command = self.mode == InputMode::TerminalCommand;
        // NOTE: LanguageRename must NOT count as plain here — its Enter
        // (commit) and Escape (cancel) are handled by this editor itself in
        // handle_keydown, not by an enclosing view.
        let is_plain = matches!(self.mode, InputMode::Plain { .. });
        let is_explorer_name = matches!(
            &self.mode,
            InputMode::Plain { key } if key == "explorer_entry_name"
        );
        let cursor_at_end = self.cursor == self.buffer.chars().count();
        let vim_blocks_history = self.vim_blocks_history_propagation();
        let vim_enabled = self.vim_enabled;
        // Whether an @-mention popup is open — while true, Up/Down/Enter/Escape
        // are claimed for popup navigation regardless of mode-specific rules
        // below (e.g. MultiLine normally propagates Escape/Tab to the parent).
        let mention_active = self.active_mention.is_some();
        let context_menu_open = self.context_menu_position.is_some();
        // Shortcut labels shown in the open menu are live keybindings: capture
        // the enabled entries' (shortcut, command) pairs at render time so the
        // keydown closure can invoke the matching command directly.
        let context_menu_shortcuts: Vec<(String, EditorContextMenuCommand)> = if context_menu_open {
            self.context_menu_entries()
                .iter()
                .filter_map(|entry| match entry {
                    ContextMenuEntry::Action {
                        shortcut: Some(shortcut),
                        command,
                        enabled: true,
                        ..
                    } => Some(((*shortcut).to_string(), *command)),
                    _ => None,
                })
                .collect()
        } else {
            Vec::new()
        };
        // The language/navigation commands also work without opening the
        // menu: as long as this editor holds the caret, F12/F2/⌘F12/… invoke
        // them directly. Text-editing shortcuts (undo/copy/paste/…) are
        // excluded — the keydown path below already owns those chords.
        let editor_shortcuts: Vec<(String, EditorContextMenuCommand)> = self
            .context_menu_entries()
            .iter()
            .filter_map(|entry| match entry {
                ContextMenuEntry::Action {
                    shortcut: Some(shortcut),
                    command,
                    enabled: true,
                    ..
                } if is_global_editor_command(*command) => {
                    Some(((*shortcut).to_string(), *command))
                }
                _ => None,
            })
            .collect();

        // Compute visual row info at render time so the keydown closure can decide
        // whether up/down should navigate within the editor or propagate to history.
        let uses_visual_lines = is_terminal_command || is_multiline;
        let (cursor_visual_row, visual_line_count) = if uses_visual_lines && !self.buffer.is_empty()
        {
            if is_source_editor && self.document_capabilities.large_file {
                let index = self.large_source_index();
                (index.line_for_char(self.cursor), index.line_count())
            } else {
                let vls = self.all_visual_lines(self.layout_width.get(), ctx.font_cache());
                let row = Self::cursor_visual_row(self.cursor, &vls);
                (row, vls.len())
            }
        } else {
            (0, 1)
        };

        let passthrough_keys = self.passthrough_keys.clone();
        let native_slash_trigger = self.native_slash_commands && self.buffer.trim().is_empty();
        let event_handler = EventHandler::new(child)
            .on_keydown(move |event_ctx, _app, keystroke| {
                // Native slash commands belong to the wrapped CLI, including
                // the initial slash. Do not also insert them into this buffer.
                if native_slash_trigger
                    && keystroke.key == "/"
                    && !keystroke.cmd
                    && !keystroke.ctrl
                    && !keystroke.alt
                    && !keystroke.meta
                {
                    return DispatchEventResult::PropagateToParent;
                }
                // Inert mode: a foreground command owns the keyboard. This
                // editor is collapsed (zero height) but may still hold focus —
                // consuming keys here would swallow them into an invisible
                // buffer. Propagate everything so the pane forwards to the PTY.
                if passthrough_keys
                    .as_ref()
                    .is_some_and(|f| f.load(Ordering::Relaxed))
                {
                    return DispatchEventResult::PropagateToParent;
                }
                // Unfocused editors must not consume keystrokes — propagate so
                // the focused editor (or the pane's outer handler) can act.
                if !is_focused {
                    return DispatchEventResult::PropagateToParent;
                }
                if context_menu_open {
                    if keystroke.key == "escape" {
                        event_ctx.dispatch_typed_action(InputEditorAction::CloseContextMenu);
                        return DispatchEventResult::StopPropagation;
                    }
                    // Any shortcut label rendered in the menu activates its
                    // entry (F2, ⌘F12, ⇧F12, ⌥⇧F, ⌘Z, …).
                    for (shortcut, command) in &context_menu_shortcuts {
                        if context_menu_shortcut_matches(shortcut, keystroke) {
                            event_ctx.dispatch_typed_action(InputEditorAction::ContextMenuCommand(
                                *command,
                            ));
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                    if !keystroke.cmd
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.meta
                        && keystroke.key == "up"
                    {
                        event_ctx.dispatch_typed_action(
                            InputEditorAction::MoveContextMenuSelection { direction: -1 },
                        );
                        return DispatchEventResult::StopPropagation;
                    }
                    if !keystroke.cmd
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.meta
                        && keystroke.key == "down"
                    {
                        event_ctx.dispatch_typed_action(
                            InputEditorAction::MoveContextMenuSelection { direction: 1 },
                        );
                        return DispatchEventResult::StopPropagation;
                    }
                    if !keystroke.cmd
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.meta
                        && keystroke.key == "enter"
                    {
                        event_ctx
                            .dispatch_typed_action(InputEditorAction::ActivateContextMenuSelection);
                        return DispatchEventResult::StopPropagation;
                    }
                }
                if mention_active
                    && !keystroke.cmd
                    && !keystroke.ctrl
                    && !keystroke.alt
                    && !keystroke.meta
                    && matches!(keystroke.key.as_str(), "up" | "down" | "escape" | "enter")
                {
                    event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                    return DispatchEventResult::StopPropagation;
                }
                // Menu closed but the caret is here: the language/navigation
                // shortcuts (F12, F2, ⌘F12, ⇧F12, ⌥⇧F, …) still fire.
                for (shortcut, command) in &editor_shortcuts {
                    if context_menu_shortcut_matches(shortcut, keystroke) {
                        event_ctx
                            .dispatch_typed_action(InputEditorAction::ContextMenuCommand(*command));
                        return DispatchEventResult::StopPropagation;
                    }
                }
                if keystroke.cmd || keystroke.ctrl {
                    if keystroke.key == "c" {
                        // A focused input selection owns the platform copy chord,
                        // including the terminal command input.  Previously that
                        // editor always propagated Cmd+C to TerminalView, whose
                        // stale output selection could win and replace the text
                        // the user had visibly selected in the input.
                        // Ctrl+C is the terminal INTERRUPT, not copy (this matters on
                        // Windows where Ctrl+C is the only "c" chord) — it must always
                        // propagate so the pane can send it to the running command.
                        if keystroke.cmd && !keystroke.ctrl && has_input_selection {
                            event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(
                                keystroke.clone(),
                            ));
                            return DispatchEventResult::StopPropagation;
                        }
                        // With no input selection, let the enclosing surface
                        // handle the chord (for example, selected terminal output).
                        return DispatchEventResult::PropagateToParent;
                    }
                    if keystroke.key == "a"
                        || keystroke.key == "z"
                        || keystroke.key == "y"
                        || keystroke.key == "v"
                        || keystroke.key == "x"
                    {
                        event_ctx
                            .dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                        return DispatchEventResult::StopPropagation;
                    }
                    return DispatchEventResult::PropagateToParent;
                }
                // Plain single-line (e.g. block filter): let the parent handle
                // Enter/Escape (e.g. to close), but keep all text-editing keys.
                if is_plain && (keystroke.key == "escape" || keystroke.key == "enter") {
                    // Explorer naming handles commit/cancel in handle_keydown;
                    // forwarding these keys to the parent bypasses that logic.
                    if is_explorer_name {
                        event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                        return DispatchEventResult::StopPropagation;
                    }
                    return DispatchEventResult::PropagateToParent;
                }
                // MultiLine: up/down always navigate within editor; enter inserts newline.
                if is_multiline {
                    if keystroke.key == "up" {
                        if cursor_visual_row > 0 {
                            event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(
                                keystroke.clone(),
                            ));
                            return DispatchEventResult::StopPropagation;
                        }
                        return DispatchEventResult::PropagateToParent;
                    }
                    if keystroke.key == "down" {
                        if cursor_visual_row < visual_line_count.saturating_sub(1) {
                            event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(
                                keystroke.clone(),
                            ));
                            return DispatchEventResult::StopPropagation;
                        }
                        return DispatchEventResult::PropagateToParent;
                    }
                    if keystroke.key == "escape" || (keystroke.key == "tab" && !is_source_editor)
                        || (is_review_instructions && keystroke.key=="enter" && (keystroke.cmd || keystroke.ctrl)) {
                        return DispatchEventResult::PropagateToParent;
                    }
                    // enter (with or without shift) → insert newline handled in handle_keydown
                    event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                    return DispatchEventResult::StopPropagation;
                }
                if is_terminal_command && !vim_blocks_history {
                    // Shift+Up/Down always propagate so the parent can recall
                    // command history (the AI rich input), regardless of which
                    // visual row the cursor is on.
                    if keystroke.shift && (keystroke.key == "up" || keystroke.key == "down") {
                        return DispatchEventResult::PropagateToParent;
                    }
                    // up/down: navigate visual rows when multi-line, otherwise history
                    if keystroke.key == "up" {
                        if cursor_visual_row == 0 {
                            return DispatchEventResult::PropagateToParent;
                        }
                        event_ctx
                            .dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "down" {
                        if cursor_visual_row >= visual_line_count.saturating_sub(1) {
                            return DispatchEventResult::PropagateToParent;
                        }
                        event_ctx
                            .dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "tab"
                        || (keystroke.key == "enter" && !keystroke.shift)
                        || (keystroke.key == "escape" && !(vim_enabled && is_terminal_command))
                        || (keystroke.key == "right" && cursor_at_end && !has_input_selection)
                    {
                        return DispatchEventResult::PropagateToParent;
                    }
                }
                event_ctx.dispatch_typed_action(InputEditorAction::KeyDown(keystroke.clone()));
                DispatchEventResult::StopPropagation
            })
            .on_left_mouse_down_with_count(move |event_ctx, _app, click_pos, count| {
                if context_menu_open {
                    event_ctx.dispatch_typed_action(InputEditorAction::CloseContextMenu);
                    return DispatchEventResult::StopPropagation;
                }
                // Tapping a text input on mobile must raise the soft keyboard —
                // a wgpu <canvas> has no native input, so nothing shows the
                // keyboard otherwise (this is why typing was impossible on phones).
                // No-op on desktop (only the mobile wasm driver reads the flag).
                event_ctx.request_soft_keyboard();
                let action = match count {
                    2 => InputEditorAction::DoubleClickAt(click_pos),
                    n if n >= 3 => InputEditorAction::TripleClickAt(click_pos),
                    _ => InputEditorAction::ClickAt(click_pos),
                };
                event_ctx.dispatch_typed_action(action);
                DispatchEventResult::StopPropagation
            })
            .on_right_mouse_down(move |event_ctx, _app, position| {
                event_ctx.dispatch_typed_action(InputEditorAction::ContextMenuAt(position));
                DispatchEventResult::StopPropagation
            })
            .on_mouse_dragged(move |event_ctx, _app, drag_pos| {
                event_ctx.dispatch_typed_action(InputEditorAction::DragAt(drag_pos));
                DispatchEventResult::StopPropagation
            })
            .on_left_mouse_up(move |event_ctx, _app, release_pos| {
                event_ctx.dispatch_typed_action(InputEditorAction::ReleaseAt(release_pos));
                DispatchEventResult::StopPropagation
            });

        let definition_interactions = EventHandler::new(event_handler.finish())
            .with_always_handle()
            .on_mouse_in(
                |event_ctx, _app, position| {
                    event_ctx.dispatch_typed_action(InputEditorAction::DefinitionHoverAt(position));
                    DispatchEventResult::PropagateToParent
                },
                None,
            )
            .on_mouse_out(|event_ctx, _app, position| {
                // The framework fires mouse_out on EVERY MouseMoved that the
                // mouse_in callback does not claim, so this is not a reliable
                // "pointer left the editor" signal. Re-evaluate at the
                // position instead of clearing: inside the source content the
                // symbol-level dedup makes it a no-op, while a position
                // outside the content (or over the gutter) clears the hover.
                event_ctx.dispatch_typed_action(InputEditorAction::DefinitionHoverAt(position));
                DispatchEventResult::PropagateToParent
            })
            .on_modifier_state_changed(|event_ctx, _app, key, state| {
                if is_platform_definition_modifier(key) {
                    event_ctx.dispatch_typed_action(InputEditorAction::DefinitionModifierHeld(
                        matches!(state, KeyState::Pressed),
                    ));
                }
                DispatchEventResult::PropagateToParent
            });

        Box::new(definition_interactions)
    }

    fn on_focus(&mut self, _focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        // No shadow bool to set — is_focused() queries the framework directly
        // via focus_handle.  Just start the cursor blink.
        self.cursor_visible = true;
        self.schedule_cursor_blink(ctx);
        ctx.notify();
    }

    fn on_blur(&mut self, _blur_ctx: &warpui::BlurContext, ctx: &mut ViewContext<Self>) {
        // No shadow bool to clear — is_focused() queries the framework directly.
        self.cursor_visible = false;
        self.definition_modifier_held = false;
        self.last_pointer_position = None;
        self.definition_hover = None;
        self.definition_click_candidate = None;
        if let Some(handle) = self.blink_handle.take() {
            handle.abort();
        }
        ctx.notify();
    }
}
