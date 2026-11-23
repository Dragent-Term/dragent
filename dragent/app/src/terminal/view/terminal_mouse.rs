//! Terminal mouse, clipboard, and directory entries.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    AltLeftGesture, AltPanAxis, TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT, CMD_HIST_CHROME_PX,
    CMD_HIST_ROW_PX, RESIZE_PADDING_X, RESIZE_PADDING_Y, SCROLL_BOTTOM_EPSILON_PX,
};
use crate::agent::input_classifier::{
    heuristic::HeuristicInputClassifier, ClassifyContext, InputClassifier,
};
use crate::agent::{DetectedInputType, InputConfig, TabSessionKind};
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::console::ai_toolbar::{self, AiPanelActions, AiTool};
use crate::console::block_filter::{block_output_lines, count_block_matches};
use crate::console::command_intel;
use crate::console::dir_listing::{resolve_entry_path, DirSort, DirView};
use crate::console::editor_settings::EditorSettings;
use crate::console::escape_parse::styled_to_colored;
use crate::console::fs_suggest::should_suggest_paths;
use crate::console::history_search::HistorySearchState;
use crate::console::input_bar;
use crate::console::input_editing;
use crate::console::input_overlays;
use crate::console::path_picker::PathPicker;
use crate::console::prefixed_history;
use crate::console::shell_aliases::ShellAliases;
use crate::console::suggest_engine::SuggestEngine;
use crate::console::workflow_search::WorkflowSearchState;
use crate::console::workflows::{Workflow, WorkflowSession, WorkflowStore};
use crate::icons::ICON_SIZE_SM;
use crate::process_ext::NoWindow;
use crate::setup::UiSettings;
use crate::terminal::input::{
    alt_left_button_goes_to_app, encode_keystroke, encode_paste, map_pointer_to_cell,
    modifiers_from_ui, mouse_event,
    scroll_arrow_bytes, should_encode_wheel_as_mouse,
    should_forward_key_to_pty_while_ai_panel_open, should_passthrough_mouse, wheel_button,
    RichInputKeyContext,
};
use crate::terminal::manager::TerminalManager;
use crate::terminal::model::block_height_tree::BlockHeightIndex;
use crate::terminal::model::{
    ShellLinkStatus, SizeInfo, SizeReason, SizeUpdate, TerminalEvent, TerminalModel,
};
use crate::terminal::render::{
    build_alt_screen_ui, build_block_options_menu, build_bootstrap_grid_ui,
    build_sticky_block_header, build_visible_block_list_column, dir_grid_content_width,
    extract_prompt_tail, sample_ai_chrome, BlockListScrollAdapter, BlockRenderContext,
    CellGlyphCache, CellMetrics, DiagramCanvasState, StickyBlockHeader, TerminalSizeElement,
    BLOCK_HEADER_HEIGHT, BLOCK_LIST_BOTTOM_POSITION_ID,
};
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
use crate::terminal::viewport::{
    is_block_list_virtualization_enabled, ScrollMode, ScrollPosition, ViewportState,
};
use crate::terminal::TerminalEventListener;
use crate::theme::{Theme, INPUT_FONT_SIZE, TERMINAL_PADDING_LEFT};
use crate::workspace::persistence::PersistedBlock;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode, DEFAULT_SESSION_TITLE};
use async_channel::Sender;
use dragent_pty::{resolve_shell, PtyEvent, SessionPlatform};
use dragent_resource_policy::{ActivityLevel, MemoryPressure, ResourceBudget};
use dragent_shell_hooks::ShellKind;
use dragent_terminal::{MouseButton, MouseEventKind, TermMode};
use parking_lot::{FairMutex, RwLock};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, Dismiss, DispatchEventResult, Element, EventHandler, Expanded,
    Fill, Flex, Hoverable, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, PositionedElementAnchor, PositionedElementOffsetBounds, SavePosition,
    ScrollStateHandle, Scrollable, ScrollableElement, ScrollbarWidth, SelectableArea,
    SelectionHandle, SelectionUpdateArgs, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::geometry::vector::Vector2F;
use warpui::keymap::Keystroke;
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::{IntoPixels, Pixels};
use warpui::{
    r#async::Timer, AppContext, Entity, EntityId, FocusContext, GetSingletonModelHandle,
    ModelHandle, ReadModel, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{LiveElement, ScrollTarget, ScrollToPositionMode};
use warpui_core::event::{KeyState, ModifiersState};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::keyboard::KeyCode;
use warpui_core::scene::Border;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::WindowId;
use web_time::Instant;

impl TerminalView {
    pub(crate) fn viewport_origin(&self, ctx: &ViewContext<Self>) -> Option<Vector2F> {
        let alt_id = format!("terminal_alt_viewport_{}", self.pane_uuid);
        let block_id = format!("terminal_block_viewport_{}", self.pane_uuid);
        ctx.element_position_by_id(&alt_id)
            .or_else(|| ctx.element_position_by_id(&block_id))
            .map(|rect| rect.origin())
    }

    /// Returns whether anything reached the clipboard.
    pub(crate) fn write_output_clipboard(&self, text: String, ctx: &mut ViewContext<Self>) -> bool {
        if text.is_empty() {
            return false;
        }
        // Strip gutter whitespace on copy.
        let text = crate::terminal::input::maybe_trim_gutter(text);
        if text.is_empty() {
            return false;
        }
        ctx.clipboard().write(ClipboardContent::plain_text(text));
        true
    }

    /// Copies `text` and, if anything reached the clipboard, shows the
    /// checkmark on the copy button named `key` for a moment.
    pub(crate) fn copy_with_confirmation(
        &mut self,
        key: String,
        text: String,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.write_output_clipboard(text, ctx) {
            return;
        }
        let generation = self.copy_confirmation.show(key);
        ctx.spawn(
            warpui::r#async::Timer::after(super::copy_confirmation::COPY_CONFIRMATION),
            move |view, _, ctx| {
                if view.copy_confirmation.expire(generation) {
                    ctx.notify();
                }
            },
        );
    }

    /// Explicit copy (Cmd/Ctrl+C on output, context menu): always copies.
    pub(crate) fn copy_selected_output(&mut self, ctx: &mut ViewContext<Self>) -> bool {
        // The input you're typing in owns the copy chord: text selected in the
        // focused rich input (an agent's message box) or command input is what
        // you mean to copy, not an older selection in the output.
        for editor in [self.rich_input_editor.clone(), self.command_editor.clone()] {
            let selected = editor.read(ctx, |editor, app| {
                editor.is_focused(app).then(|| editor.selected_text()).flatten()
            });
            if let Some(text) = selected.filter(|t| !t.is_empty()) {
                self.write_output_clipboard(text, ctx);
                return true;
            }
        }
        // Block output selection takes priority over the command input so dragging
        // in the terminal copies output even when the input still has a stale range.
        if let Some(text) = self
            .block_selected_text
            .read()
            .clone()
            .filter(|t| !t.is_empty())
        {
            self.write_output_clipboard(text, ctx);
            return true;
        }
        if let Some(text) = self.model.read(ctx, |m, _| m.selected_text()) {
            self.write_output_clipboard(text, ctx);
            return true;
        }
        if let Some(text) = self
            .command_editor
            .read(ctx, |editor, _| editor.selected_text())
        {
            self.write_output_clipboard(text, ctx);
            return true;
        }
        if let Some(text) = self
            .rich_input_editor
            .read(ctx, |editor, _| editor.selected_text())
            .filter(|t| !t.is_empty())
        {
            self.write_output_clipboard(text, ctx);
            return true;
        }
        false
    }

    /// Auto-copy after a drag selection: only when copy-on-select is on.
    fn auto_copy_selection(&mut self, ctx: &mut ViewContext<Self>) {
        if !crate::terminal::input::clipboard_on_select_enabled() {
            return;
        }
        self.copy_selected_output(ctx);
    }

    /// Middle-click paste.
    fn maybe_middle_click_paste(
        &mut self,
        button: MouseButton,
        kind: MouseEventKind,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        use dragent_terminal::MouseButton as MB;
        if button != MB::Middle || kind != MouseEventKind::Press {
            return false;
        }
        if !crate::terminal::input::middle_click_paste_enabled() {
            return false;
        }
        let text = ctx.clipboard().read().plain_text;
        if text.is_empty() {
            return true;
        }
        let term_mode = self
            .model
            .read(ctx, |m, _| m.with_core(|core| core.active_emulator().mode()));
        let bytes = crate::terminal::input::encode_paste(&text, term_mode);
        self.manager.writer().write(&bytes);
        true
    }

    pub(crate) fn clear_command_editor_selection(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.command_editor, |editor, editor_ctx| {
            editor.clear_text_selection(editor_ctx);
        });
    }

    pub(crate) fn handle_terminal_mouse(
        &mut self,
        position: Vector2F,
        button: MouseButton,
        kind: MouseEventKind,
        _modifiers: ModifiersState,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.maybe_middle_click_paste(button, kind, ctx) {
            ctx.notify();
            return;
        }
        self.last_mouse_pos = position;
        // A drag that is panning the alt-screen must NOT also reach the TUI:
        // Claude Code (and any app with mouse reporting on) treats forwarded
        // drag-motion as its own scroll/selection gesture, so the content slid
        // vertically while the user was dragging sideways — making the text
        // impossible to read. Swallow only MOTION while a pan is in progress;
        // press/release still go through, so a plain tap on the TUI still works.
        // ONLY a horizontal pan is withheld. A vertical drag is the TUI's own
        // scroll gesture (the only way to scroll Claude Code on a touch screen),
        // so it must pass through untouched — suppressing every direction would
        // swallow it: the grid wouldn't pan (no x movement) and the TUI would
        // never see it, leaving vertical drags dead on a phone.
        if matches!(kind, MouseEventKind::Motion)
            && self
                .alt_pan_drag
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|d| d.axis == Some(AltPanAxis::Horizontal))
        {
            return;
        }
        // Belt-and-braces state clear: the pan's own mouse-up handler may never
        // fire if an inner handler consumes the release first, and stale drag
        // state would suppress TUI drag-motion forever after one pan.
        if matches!(kind, MouseEventKind::Release) {
            *self.alt_pan_drag.lock().unwrap() = None;
        }
        let alt = self.alt_held;
        let (mouse_reporting, alt_active, size) = self.model.read(ctx, |m, _| {
            (m.mouse_reporting_mode(), m.alt_screen_active(), m.size())
        });
        let passthrough = should_passthrough_mouse(alt, mouse_reporting);

        if let Some(origin) = self.viewport_origin(ctx) {
            if let Some((row, col)) = map_pointer_to_cell(position, origin, size, 0.0) {
                let ui_mods = ModifiersState {
                    shift: self.shift_held,
                    ctrl: self.ctrl_held,
                    alt: self.alt_held,
                    cmd: self.cmd_held,
                    func: false,
                };
                let app_wants_mouse = self.model.read(ctx, |m, _| m.terminal_mouse_active());

                if alt_active && button == MouseButton::Left {
                    match kind {
                        MouseEventKind::Press => {
                            self.alt_press = None;
                            self.drag_selecting = false;
                            self.model.update(ctx, |m, mctx| {
                                m.clear_alt_selection(mctx);
                            });
                            if (ui_mods.cmd || ui_mods.ctrl)
                                && self.open_alt_screen_link_at(row, col, ctx)
                            {
                                self.alt_left_gesture = Some(AltLeftGesture::Link);
                            } else if alt_left_button_goes_to_app(
                                passthrough,
                                app_wants_mouse,
                                self.remote_read_only,
                            ) {
                                self.alt_left_gesture = Some(AltLeftGesture::App {
                                    last_cell: (row, col),
                                });
                                self.forward_left_button(kind, row, col, ui_mods, ctx);
                            } else {
                                self.alt_left_gesture = Some(AltLeftGesture::Local);
                                self.alt_press = Some((row, col, position));
                            }
                            ctx.notify();
                        }
                        MouseEventKind::Motion => match self.alt_left_gesture {
                            Some(AltLeftGesture::App { last_cell }) if last_cell != (row, col) => {
                                self.alt_left_gesture = Some(AltLeftGesture::App {
                                    last_cell: (row, col),
                                });
                                self.forward_left_button(kind, row, col, ui_mods, ctx);
                            }
                            Some(AltLeftGesture::Local) => {
                                if let Some((sr, sc, spos)) = self.alt_press {
                                    const DRAG_THRESHOLD: f32 = 3.0;
                                    if !self.drag_selecting {
                                        let dx = position.x() - spos.x();
                                        let dy = position.y() - spos.y();
                                        if (dx * dx + dy * dy).sqrt() > DRAG_THRESHOLD {
                                            self.drag_selecting = true;
                                            self.model.update(ctx, |m, mctx| {
                                                m.start_selection(sr, sc, mctx);
                                            });
                                        }
                                    }
                                    if self.drag_selecting {
                                        // Clamp to the edge so dragging past the top/bottom
                                        // keeps selecting those lines.
                                        self.model.update(ctx, |m, mctx| {
                                            m.extend_selection(row, col, mctx);
                                        });
                                        ctx.notify();
                                    }
                                }
                            }
                            _ => {}
                        },
                        MouseEventKind::Release => match self.alt_left_gesture.take() {
                            Some(AltLeftGesture::App { .. }) => {
                                self.forward_left_button(kind, row, col, ui_mods, ctx);
                                ctx.notify();
                            }
                            Some(AltLeftGesture::Local) => {
                                self.alt_press = None;
                                if self.drag_selecting {
                                    self.drag_selecting = false;
                                    self.auto_copy_selection(ctx);
                                }
                                ctx.notify();
                            }
                            Some(AltLeftGesture::Link) | None => {}
                        },
                    }
                } else if passthrough && app_wants_mouse {
                    // Wheel and non-left buttons: forward to the app.
                    let event = mouse_event(button, kind, row, col, modifiers_from_ui(ui_mods));
                    if let Some(bytes) = self.model.read(ctx, |m, _| m.encode_terminal_mouse(event))
                    {
                        self.manager.writer().write(&bytes);
                    }
                }
            }
        }
    }

    /// Report part of a left-button gesture to the app in the terminal.
    fn forward_left_button(
        &mut self,
        kind: MouseEventKind,
        row: usize,
        col: usize,
        modifiers: ModifiersState,
        ctx: &mut ViewContext<Self>,
    ) {
        let event = mouse_event(
            MouseButton::Left,
            kind,
            row,
            col,
            modifiers_from_ui(modifiers),
        );
        if let Some(bytes) = self.model.read(ctx, |m, _| m.encode_terminal_mouse(event)) {
            self.manager.writer().write(&bytes);
        }
    }

    /// Cmd/Ctrl+Click on the alt-screen grid (full-screen TUIs like vim,
    /// htop, or `opencode`): if the clicked cell falls inside a detected URL
    /// or file path, open it and return `true`. Takes priority over both the
    /// local text-selection gesture and PTY mouse-passthrough so a modified
    /// click always means "open", matching normal block output.
    pub(crate) fn open_alt_screen_link_at(
        &mut self,
        row: usize,
        col: usize,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        use crate::terminal::render::block_output::{
            grid_link_at, resolve_terminal_path, TerminalLinkKind,
        };

        // Same detection as the hover underline: a link the app wrapped onto
        // following rows opens whole, from whichever row was clicked.
        let (lines, wrapped, cols, cwd) = self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                let emu = core.alt_screen_emulator();
                let styled = emu.alt_screen_all_lines();
                (
                    styled
                        .iter()
                        .map(|line| line.plain_text())
                        .collect::<Vec<_>>(),
                    styled.iter().map(|line| line.wrapped).collect::<Vec<_>>(),
                    emu.dimensions().1,
                    core.cwd().clone(),
                )
            })
        });
        let Some(found) = grid_link_at(&lines, &wrapped, cols, Some(&cwd), row, col) else {
            return false;
        };

        match found.kind {
            TerminalLinkKind::Url(url) => self.open_web_link(url, ctx),
            TerminalLinkKind::FilePath { raw, line, column } => {
                let (raw_path, _, _) =
                    crate::terminal::render::block_output::split_path_suffix(&raw);
                if raw_path.is_empty() {
                    return false;
                }
                let path = resolve_terminal_path(raw_path, &cwd);
                ctx.dispatch_typed_action_deferred(
                    crate::workspace::WorkspaceAction::OpenTerminalPath { path, line, column },
                );
            }
        }
        true
    }

    pub(crate) fn send_terminal_wheel(
        &mut self,
        position: Vector2F,
        button: MouseButton,
        modifiers: ModifiersState,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let Some(origin) = self.viewport_origin(ctx) else {
            return false;
        };
        let size = self.model.read(ctx, |m, _| m.size());
        let Some((row, col)) = map_pointer_to_cell(position, origin, size, 0.0) else {
            return false;
        };
        let event = mouse_event(
            button,
            MouseEventKind::Press,
            row,
            col,
            modifiers_from_ui(modifiers),
        );
        let Some(bytes) = self.model.read(ctx, |m, _| m.encode_terminal_mouse(event)) else {
            return false;
        };
        self.manager.writer().write(&bytes);
        true
    }

    pub(crate) fn handle_terminal_wheel(
        &mut self,
        position: Vector2F,
        delta_y: f32,
        precise: bool,
        modifiers: ModifiersState,
        ctx: &mut ViewContext<Self>,
    ) {
        // `delta_y` is in different units depending on the device, so the
        // "is this a real scroll?" threshold has to be too. A discrete wheel
        // reports LINES, where any non-zero value is a deliberate notch. A
        // trackpad reports PIXELS, where sub-pixel jitter during a two-finger
        // rest would otherwise register as scrolling and fire an arrow key.
        let deadzone = if precise { 1.0 } else { f32::EPSILON };
        if delta_y.abs() <= deadzone {
            return;
        }
        // Shown as a chat, the chat scrolls itself. The wheel must not reach
        // the agent's screen behind it: as wheel or arrow keys it made the
        // agent redraw (read as working) and could walk its prompt history.
        if self.showing_cli_chat(ctx) {
            return;
        }
        let idle_shell = self.model.read(ctx, |m, _| {
            m.with_core(|core| core.integration_active() && !core.is_command_running())
        });
        if idle_shell {
            ctx.dispatch_typed_action(&TerminalAction::BlockScroll { delta_y });
            return;
        }
        self.last_mouse_pos = position;
        // Scrolling makes an alt-screen selection stale — its rows now map to
        // different (repainted) content. Clear it so the highlight doesn't linger
        // over the newly-scrolled text, and let the scroll go through so the user
        // can reveal off-screen content and select a fresh screenful.
        if !self.drag_selecting {
            self.alt_press = None;
            self.model.update(ctx, |m, mctx| {
                m.clear_alt_selection(mctx);
            });
        }
        let (mouse_reporting, app_cursor) = self
            .model
            .read(ctx, |m, _| (m.mouse_reporting_mode(), m.app_cursor_keys()));

        if should_encode_wheel_as_mouse(mouse_reporting) {
            let button = wheel_button(delta_y);
            // One browser wheel event maps to one terminal wheel event. The
            // browser delta is a physical distance, not a repeat count; using
            // ceil(delta) here made trackpads send large bursts to Claude.
            let sent = self.send_terminal_wheel(self.last_mouse_pos, button, modifiers, ctx);
            // If the command finished between dispatch and encoding, do not
            // substitute arrow keys: those would reach the shell prompt.
            if !sent {
                ctx.dispatch_typed_action(&TerminalAction::BlockScroll { delta_y });
            }
        } else {
            let alt_active = self.model.read(ctx, |m, _| m.alt_screen_active());
            let use_block_scroll = self.model.read(ctx, |m, _| {
                m.with_core(|core| {
                    !alt_active
                        && (core.integration_active()
                            || !core.blocks().is_empty()
                            || core.emulator().grid().scrollback_len() > 0)
                })
            });
            if use_block_scroll {
                ctx.dispatch_typed_action(&TerminalAction::BlockScroll { delta_y });
            } else if alt_active {
                let bytes = scroll_arrow_bytes(delta_y, app_cursor);
                self.manager.writer().write(&bytes);
            } else {
                // An empty normal buffer cannot scroll; arrow keys would edit
                // shell history and ring the bell instead.
                ctx.dispatch_typed_action(&TerminalAction::BlockScroll { delta_y });
            }
        }
    }
}
