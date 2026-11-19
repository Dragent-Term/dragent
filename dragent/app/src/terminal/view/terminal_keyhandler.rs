//! Terminal outer key handler.
//!
//! Blocks here are impl TerminalView: same type, same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{
    TerminalView, BLOCK_LIST_BOTTOM_GAP_PX, BLOCK_MENU_EST_HEIGHT, CMD_HIST_CHROME_PX,
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
    encode_keystroke, encode_paste, map_pointer_to_cell, modifiers_from_ui, mouse_event,
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
    pub(crate) fn wrap_outer_key_handler(
        &self,
        root: Box<dyn Element>,
        app: &AppContext,
        active_ai: Option<AiTool>,
    ) -> Box<dyn Element> {
        let pane_view_id = self.pane_view_id;
        let command_editor_view_id = self.command_editor.id();
        let command_editor_handle = self.command_editor.clone();
        let rich_input_editor_view_id = self.rich_input_editor.id();
        let rich_input_editor_handle = self.rich_input_editor.clone();
        let feedback_editor_view_id = self.feedback_editor.id();
        let question_editor_view_id = self.question_editor.id();
        let block_filter_editor_view_id = self.block_filter_editor.id();
        // Popover editors must count as "input active" too, or the pane's
        // focus gate drops their Escape/Enter before the handlers below run —
        // the same trap that once made raw-mode prompts swallow every key.
        let new_branch_editor_view_id = self.new_branch_editor.id();
        let model_search_editor_view_id = self.model_search_editor.id();
        let path_picker = self.path_picker.clone();
        let window_id_for_focus = self.window_id;
        let ai_tool_active = active_ai.is_some();
        let rich_input_active = self.rich_input_active;
        // Shown as a chat, Esc stops the agent (when it works) and never
        // closes the input the chat is typed in.
        let chat_shown = self.showing_cli_chat(app);
        let chat_busy = chat_shown && self.cli_chat_busy(app);
        let rich_input_proxy_chars = self.rich_input_proxy_chars;
        let history_search_open = self.history_search.open;
        let workflow_search_open = self.workflow_search.open;
        let block_filter_block: Option<usize> = self.block_filter.as_ref().map(|(id, _)| *id);
        let branch_menu_open = self.branch_menu_open;
        let folder_menu_open = self.folder_menu_open;
        let model_menu_open = self.model_menu_open;
        let slash_palette_selected = self.slash_palette_selected;
        let slash_project_root = self.cwd(app);
        let cmd_hist_open = self.cmd_hist_open;

        let (app_cursor_mode, app_keypad_mode) = self.model.read(app, |m, _| {
            m.with_core(|core| {
                let mode = core.active_emulator().mode();
                (
                    mode.contains(TermMode::APP_CURSOR),
                    mode.contains(TermMode::APP_KEYPAD),
                )
            })
        });
        let command_running = self.model.read(app, |m, _| m.is_command_running());
        // An open ask_user card enables Alt-gated keyboard nav (Alt+Up/Down to
        // move, Alt+1..9 to pick, Alt+Enter to select). Read up front so the
        // key closure below can route without model access.
        let question_nav_id = self
            .agent_controller
            .read(app, |c, _| c.first_pending_question_id());
        // "This pane is the active one" — survives focus landing on non-input
        // views (or nowhere), unlike focused_view_id. Disambiguates splits for
        // the running-command escape hatch below.
        let pane_active = self.pane_has_focus;
        // View-only shared-session viewer: keystrokes are dropped entirely (only
        // copy is allowed), so nothing types locally or forwards to the host PTY.
        let remote_read_only = self.remote_read_only;
        let (mouse_reporting, alt_active_for_keys, term_mode) = self.model.read(app, |m, _| {
            (
                m.mouse_reporting_mode(),
                m.alt_screen_active(),
                m.with_core(|core| core.active_emulator().mode()),
            )
        });

        let root = if remote_read_only {
            root
        } else {
            warpui::elements::DropTarget::new(
                root,
                crate::agent::file_drag::AgentInputDropTarget::Terminal(pane_view_id),
            )
            .finish()
        };
        EventHandler::new(root)
            .with_always_handle()
            .on_files_dropped(move |event_ctx, _app, paths, _location| {
                if remote_read_only {
                    return DispatchEventResult::StopPropagation;
                }
                event_ctx.dispatch_typed_action(TerminalAction::AttachDroppedFiles {
                    paths: paths.to_vec(),
                });
                DispatchEventResult::StopPropagation
            })
            .on_keydown(move |event_ctx, app, keystroke| {
                // A view-only viewer can copy from the output but cannot type or
                // send anything to the host — swallow every other key.
                if remote_read_only {
                    if let Some(action) = clipboard_shortcut_action(keystroke) {
                        event_ctx.dispatch_typed_action(action);
                    }
                    return DispatchEventResult::StopPropagation;
                }
                // ── Shortcut policy + custom terminal chords ─────────
                // User overrides for terminal-embedded chords (Settings →
                // Keyboard) win over the defaults below.
                {
                    let shortcuts = &crate::settings::AppSettings::global().shortcuts;
                    if let Some(action) = custom_terminal_chord_action(shortcuts, keystroke) {
                        event_ctx.dispatch_typed_action(action);
                        return DispatchEventResult::StopPropagation;
                    }
                    // Terminal-first: while a command runs in the active pane,
                    // modifier chords go to the PTY before app handlers claim
                    // them (palette, input-type lock…). Copy/paste, the
                    // Ctrl+C interrupt, and Alt-gated question nav stay with
                    // the app. App-first (default) keeps today's behavior.
                    let is_interrupt = keystroke.key == "c"
                        && keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.cmd;
                    if !shortcuts.terminal_shortcut_policy.app_chords_win()
                        && command_running
                        && pane_active
                        && !keystroke.alt
                        && !is_interrupt
                        && clipboard_shortcut_action(keystroke).is_none()
                        && chord_modifier_active(keystroke)
                    {
                        if let Some(bytes) = terminal_keystroke_to_pty_bytes(
                            keystroke,
                            app_cursor_mode,
                            app_keypad_mode,
                        ) {
                            event_ctx.dispatch_typed_action(TerminalAction::SendPty(bytes));
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                }
                let focused = app.focused_view_id(window_id_for_focus);
                let input_keys_active = focused.is_some_and(|id| {
                    id == pane_view_id
                        || id == command_editor_view_id
                        || id == rich_input_editor_view_id
                        || id == feedback_editor_view_id
                        || id == question_editor_view_id
                        || id == block_filter_editor_view_id
                        || id == new_branch_editor_view_id
                        || id == model_search_editor_view_id
                });
                let embedded_editor_focused = focused.is_some_and(|id| {
                    id == command_editor_view_id
                        || id == rich_input_editor_view_id
                        || id == feedback_editor_view_id
                        || id == question_editor_view_id
                        || id == block_filter_editor_view_id
                        || id == new_branch_editor_view_id
                        || id == model_search_editor_view_id
                });
                // Ask-user card keyboard nav (opencode QUESTION_MODE parity).
                // Alt-gated so typing, history, completion, and global chords
                // are untouched; the PTY pane is excluded so no bytes are ever
                // hijacked from a running command.
                if let Some(action_id) = question_nav_id {
                    let pane_focused = focused.is_some_and(|id| id == pane_view_id);
                    if !pane_focused && keystroke.alt && !keystroke.ctrl && !keystroke.cmd {
                        let nav = match keystroke.key.as_str() {
                            "up" => Some(TerminalAction::MoveAgentQuestionCursor {
                                action_id,
                                delta: -1,
                            }),
                            "down" => Some(TerminalAction::MoveAgentQuestionCursor {
                                action_id,
                                delta: 1,
                            }),
                            "enter" => Some(TerminalAction::PickAgentQuestionCursor { action_id }),
                            digit if digit.len() == 1 => digit.chars().next().and_then(|c| {
                                ('1'..='9').contains(&c).then(|| {
                                    TerminalAction::PickAgentQuestionDigit {
                                        action_id,
                                        index: (c as usize) - ('1' as usize),
                                    }
                                })
                            }),
                            _ => None,
                        };
                        if let Some(action) = nav {
                            event_ctx.dispatch_typed_action(action);
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                }

                // EventHandler::with_always_handle lets this terminal wrapper
                // observe a key even after the focused InputEditor handled it.
                // Do not then reinterpret the editor's Cmd+C/Cmd+V as terminal
                // output copy/paste: that can overwrite the selected input text
                // or paste into the PTY instead.  The focused editor owns these
                // standard clipboard chords completely.
                let rich_proxy_focused = focused == Some(rich_input_editor_view_id)
                    && rich_input_proxy_chars > 0;
                let paste_shortcut = (keystroke.cmd || keystroke.ctrl)
                    && !keystroke.alt
                    && keystroke.key == "v";
                if embedded_editor_focused
                    && !rich_proxy_focused
                    && (clipboard_shortcut_action(keystroke).is_some() || paste_shortcut)
                {
                    return DispatchEventResult::PropagateToParent;
                }
                if rich_proxy_focused && paste_shortcut {
                    event_ctx.dispatch_typed_action(TerminalAction::Paste);
                    return DispatchEventResult::StopPropagation;
                }
                // Allow clipboard shortcuts when focus is outside the command input
                // (e.g. output area or another pane control).
                if !input_keys_active {
                    if let Some(action) = clipboard_shortcut_action(keystroke) {
                        event_ctx.dispatch_typed_action(action);
                        return DispatchEventResult::StopPropagation;
                    }
                    if let Some(action) = terminal_palette_keystroke(keystroke) {
                        event_ctx.dispatch_typed_action(action);
                        return DispatchEventResult::StopPropagation;
                    }
                    // ── Running-command escape hatch ─────────────────────────
                    // A foreground command must NEVER become uncontrollable.
                    // Focus can end up on a non-input view (or nowhere at all —
                    // the input bar collapses to zero height while a command
                    // runs, orphaning the focus it held), and every key used to
                    // die right here — including Ctrl+C, leaving raw-mode
                    // prompts like `gh auth login` with no way out. Gated on
                    // pane_active so a split's inactive pane doesn't hijack
                    // keys that belong to its sibling.
                    if command_running && pane_active {
                        // Ctrl+C: interrupt, no matter where focus wandered.
                        if keystroke.key == "c"
                            && keystroke.ctrl
                            && !keystroke.alt
                            && !keystroke.cmd
                        {
                            event_ctx.dispatch_typed_action(TerminalAction::Interrupt);
                            return DispatchEventResult::StopPropagation;
                        }
                        // Focus orphaned entirely: forward keys to the PTY so
                        // interactive prompts (y/n, arrows) keep working.
                        if focused.is_none() {
                            if let Some(bytes) = terminal_keystroke_to_pty_bytes(
                                keystroke,
                                app_cursor_mode,
                                app_keypad_mode,
                            ) {
                                event_ctx.dispatch_typed_action(TerminalAction::SendPty(bytes));
                                return DispatchEventResult::StopPropagation;
                            }
                        }
                    }
                    return DispatchEventResult::PropagateToParent;
                }

                if let Some(action) = clipboard_shortcut_action(keystroke) {
                    event_ctx.dispatch_typed_action(action);
                    // Ctrl+C remains the terminal-wide interrupt, even while
                    // the filter has focus (Cmd+C remains the platform copy
                    // chord and is handled by the editor).
                    if !(keystroke.key == "c" && keystroke.ctrl && !keystroke.alt && !keystroke.cmd)
                    {
                        return DispatchEventResult::StopPropagation;
                    }
                }

                // Palette shortcuts (Ctrl/Cmd+Shift+P, Ctrl/Cmd+R) before PTY forwarding.
                if let Some(action) = terminal_palette_keystroke(keystroke) {
                    event_ctx.dispatch_typed_action(action);
                    return DispatchEventResult::StopPropagation;
                }

                // Block output filter: only intercept Escape/Enter while the
                // filter editor actually owns focus. The filter may remain open
                // while the command or AI editor is focused, and must not block
                // those editors' normal submission and keyboard actions.
                let filter_editor_focused =
                    focused.is_some_and(|id| id == block_filter_editor_view_id);
                if let Some(fb_id) = block_filter_block.filter(|_| filter_editor_focused) {
                    if keystroke.key == "escape" {
                        event_ctx.dispatch_typed_action(TerminalAction::CloseBlockFilter {
                            block_id: fb_id,
                        });
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "enter" {
                        return DispatchEventResult::StopPropagation;
                    }

                    // The embedded filter editor owns every other key as well.
                    // Do not let ordinary text, editing keys, or navigation
                    // bubble into the terminal/pane handler: while a command is
                    // running that handler can forward them to the PTY (and can
                    // reclaim focus), producing duplicated characters and a
                    // caret that jumps out of the filter. InputEditor has already
                    // processed the key; this boundary only prevents a second
                    // consumer from seeing it. Ctrl+C remains the terminal-wide
                    // interrupt even while the filter has focus; Cmd+C remains
                    // the platform copy chord and is handled by InputEditor.
                    if !(keystroke.key == "c" && keystroke.ctrl && !keystroke.alt && !keystroke.cmd)
                    {
                        return DispatchEventResult::StopPropagation;
                    }
                }

                // Chip popovers. The branch popover's name field is a Plain
                // editor, so Enter/Escape arrive here rather than being eaten
                // by the editor — Enter creates the branch, Escape closes.
                // Neither may fall through to the PTY.
                if branch_menu_open {
                    if keystroke.key == "escape" {
                        event_ctx.dispatch_typed_action(TerminalAction::CloseBranchMenu);
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "enter" {
                        event_ctx.dispatch_typed_action(TerminalAction::CreateBranchFromInput);
                        return DispatchEventResult::StopPropagation;
                    }
                }
                if folder_menu_open && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action(TerminalAction::CloseFolderMenu);
                    return DispatchEventResult::StopPropagation;
                }
                // Model picker: its search box is a Plain editor, so these
                // arrive here rather than being consumed by the editor.
                if model_menu_open && matches!(keystroke.key.as_str(), "escape" | "enter") {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleModelMenu);
                    return DispatchEventResult::StopPropagation;
                }

                if history_search_open {
                    if let Some(action) = terminal_history_search_keystroke(keystroke) {
                        event_ctx.dispatch_typed_action(action);
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "backspace" {
                        event_ctx.dispatch_typed_action(TerminalAction::HistorySearchBackspace);
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key.len() == 1
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.cmd
                    {
                        event_ctx.dispatch_typed_action(TerminalAction::HistorySearchQueryAppend(
                            keystroke.key.clone(),
                        ));
                        return DispatchEventResult::StopPropagation;
                    }
                }

                if workflow_search_open {
                    if let Some(action) = terminal_workflow_search_keystroke(keystroke) {
                        event_ctx.dispatch_typed_action(action);
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key == "backspace" {
                        event_ctx.dispatch_typed_action(TerminalAction::WorkflowSearchBackspace);
                        return DispatchEventResult::StopPropagation;
                    }
                    if keystroke.key.len() == 1
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.cmd
                    {
                        event_ctx.dispatch_typed_action(TerminalAction::WorkflowSearchQueryAppend(
                            keystroke.key.clone(),
                        ));
                        return DispatchEventResult::StopPropagation;
                    }
                }

                let completion_open = path_picker.read(app, |m, _| m.open);
                if completion_open
                    && keystroke.key != "tab"
                    && keystroke.key != "enter"
                    && keystroke.key != "escape"
                {
                    event_ctx.dispatch_typed_action(TerminalAction::DismissCompletion);
                    return DispatchEventResult::PropagateToParent;
                }

                if ai_tool_active && keystroke.ctrl && keystroke.key == "g" {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleRichInput);
                    return DispatchEventResult::StopPropagation;
                }
                // Rich input live-proxy mode: typing "/" as the very first
                // character (or continuing while already proxying) forwards
                // keystrokes straight to the wrapped CLI's PTY instead of the
                // rich editor's own buffer, so the CLI's native interactive UI
                // (e.g. its own slash-command popup) works through rich input.
                // Checked before the panel's own Escape/Enter/Ctrl handling
                // below so it can intercept Escape as "cancel this popup" (stay
                // in rich input) rather than that block's "close rich input"
                // Escape meaning.
                if ai_tool_active && rich_input_active {
                    let rich_focused =
                        rich_input_editor_handle.read(app, |editor, _| editor.is_focused(app));
                    if rich_focused && rich_input_proxy_chars > 0 {
                        if keystroke.key == "escape"
                            || (keystroke.key == "enter"
                                && !keystroke.shift
                                && !keystroke.cmd
                                && !keystroke.ctrl
                                && !keystroke.alt
                                && !keystroke.meta)
                        {
                            let bytes = terminal_keystroke_to_pty_bytes(
                                keystroke,
                                app_cursor_mode,
                                app_keypad_mode,
                            )
                            .unwrap_or_default();
                            event_ctx.dispatch_typed_action(
                                TerminalAction::ForwardRichInputProxyKey {
                                    bytes,
                                    delta: -(rich_input_proxy_chars as i32),
                                },
                            );
                            return DispatchEventResult::StopPropagation;
                        }
                        if let Some(bytes) = terminal_keystroke_to_pty_bytes(
                            keystroke,
                            app_cursor_mode,
                            app_keypad_mode,
                        ) {
                            let no_mods = !keystroke.cmd
                                && !keystroke.ctrl
                                && !keystroke.alt
                                && !keystroke.meta;
                            let delta = if no_mods && keystroke.key == "backspace" {
                                -1
                            } else if no_mods
                                && (keystroke.key == "space" || keystroke.key.chars().count() == 1)
                            {
                                1
                            } else {
                                0
                            };
                            event_ctx.dispatch_typed_action(
                                TerminalAction::ForwardRichInputProxyKey { bytes, delta },
                            );
                        }
                        return DispatchEventResult::StopPropagation;
                    }
                    if rich_focused && rich_input_proxy_chars == 0 {
                        let is_slash_trigger = !keystroke.cmd
                            && !keystroke.ctrl
                            && !keystroke.alt
                            && !keystroke.meta
                            && keystroke.key == "/"
                            && rich_input_editor_handle
                                .read(app, |editor, _| editor.get_text().trim().is_empty());
                        if is_slash_trigger {
                            if let Some(bytes) = terminal_keystroke_to_pty_bytes(
                                keystroke,
                                app_cursor_mode,
                                app_keypad_mode,
                            ) {
                                event_ctx.dispatch_typed_action(
                                    TerminalAction::ForwardRichInputProxyKey { bytes, delta: 1 },
                                );
                                return DispatchEventResult::StopPropagation;
                            }
                        }
                    }
                }

                if chat_shown && keystroke.key == "escape" {
                    if chat_busy {
                        event_ctx.dispatch_typed_action(TerminalAction::StopChatAgent);
                    }
                    return DispatchEventResult::StopPropagation;
                }

                if ai_tool_active && rich_input_active && keystroke.key == "escape" {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleRichInput);
                    return DispatchEventResult::StopPropagation;
                }

                if ai_tool_active
                    && rich_input_active
                    && should_forward_key_to_pty_while_ai_panel_open(
                        keystroke,
                        command_running,
                        mouse_reporting,
                        alt_active_for_keys,
                        term_mode,
                        rich_input_editor_handle.read(app, |editor, _| RichInputKeyContext {
                            focused: editor.is_focused(app),
                            has_text: !editor.get_text().trim().is_empty(),
                            has_ghost: editor.has_ghost_suffix(),
                        }),
                        ai_tool_active,
                    )
                {
                    if let Some(bytes) =
                        terminal_keystroke_to_pty_bytes(keystroke, app_cursor_mode, app_keypad_mode)
                    {
                        event_ctx.dispatch_typed_action(TerminalAction::SendPty(bytes));
                        return DispatchEventResult::StopPropagation;
                    }
                }

                if ai_tool_active && rich_input_active {
                    let rich_focused =
                        rich_input_editor_handle.read(app, |editor, _| editor.is_focused(app));
                    if rich_focused {
                        // AI-tool `!` scratch-shell history: the global history
                        // popover doesn't open here, so `!` gets its own. ↑ opens
                        // it (when the buffer starts with `!`) and browses; ↓
                        // browses; Esc dismisses; Enter runs the previewed command
                        // (SubmitRichInput closes the popover). Handled BEFORE the
                        // Esc/Enter defaults below so they don't pre-empt it.
                        let plain =
                            !keystroke.cmd && !keystroke.ctrl && !keystroke.alt && !keystroke.meta;
                        if cmd_hist_open {
                            if keystroke.key == "escape" {
                                event_ctx.dispatch_typed_action(TerminalAction::CloseCmdHistory);
                                return DispatchEventResult::StopPropagation;
                            }
                            if plain && keystroke.key == "up" {
                                event_ctx.dispatch_typed_action(TerminalAction::HistoryUp);
                                return DispatchEventResult::StopPropagation;
                            }
                            if plain && keystroke.key == "down" {
                                event_ctx.dispatch_typed_action(TerminalAction::HistoryDown);
                                return DispatchEventResult::StopPropagation;
                            }
                        } else if keystroke.shift && keystroke.key == "up" {
                            // Shift+↑/↓ = bash-style command-history recall (no popover).
                            event_ctx.dispatch_typed_action(TerminalAction::RichHistoryUp);
                            return DispatchEventResult::StopPropagation;
                        } else if keystroke.shift && keystroke.key == "down" {
                            event_ctx.dispatch_typed_action(TerminalAction::RichHistoryDown);
                            return DispatchEventResult::StopPropagation;
                        } else if plain && !keystroke.shift && keystroke.key == "up" {
                            let bang = rich_input_editor_handle
                                .read(app, |e, _| e.get_text().trim_start().starts_with('!'));
                            if bang {
                                event_ctx.dispatch_typed_action(TerminalAction::OpenSideCmdHistory);
                                return DispatchEventResult::StopPropagation;
                            }
                        }
                        // `!cmd` autocomplete: Tab accepts the suggestion ghost.
                        if plain && !keystroke.shift && keystroke.key == "tab" {
                            let has_ghost =
                                rich_input_editor_handle.read(app, |e, _| e.ghost_suffix.is_some());
                            if has_ghost {
                                event_ctx.dispatch_typed_action(TerminalAction::AcceptRichGhost);
                                return DispatchEventResult::StopPropagation;
                            }
                        }
                        if keystroke.key == "escape" {
                            event_ctx.dispatch_typed_action(TerminalAction::ToggleRichInput);
                            return DispatchEventResult::StopPropagation;
                        }
                        if keystroke.key == "enter" && !keystroke.shift {
                            event_ctx.dispatch_typed_action(TerminalAction::SubmitRichInput);
                            return DispatchEventResult::StopPropagation;
                        }
                        if keystroke.ctrl && keystroke.key != "g" {
                            if let Some(bytes) = terminal_keystroke_to_pty_bytes(
                                keystroke,
                                app_cursor_mode,
                                app_keypad_mode,
                            ) {
                                event_ctx.dispatch_typed_action(TerminalAction::SendPty(bytes));
                                return DispatchEventResult::StopPropagation;
                            }
                        }
                        return DispatchEventResult::PropagateToParent;
                    }
                }

                let pane_focused =
                    focused.is_some_and(|id| id == pane_view_id || id == command_editor_view_id);
                if is_vim_toggle_keystroke(keystroke) {
                    event_ctx.dispatch_typed_action(TerminalAction::ToggleVimMode);
                    return DispatchEventResult::StopPropagation;
                }
                // Ctrl+C is dispatched before any focus test. The input bar is
                // collapsed to zero height while a command runs, so the command
                // editor cannot hold focus; if the pane didn't hold it either,
                // the old `command_running && pane_focused` gate matched neither
                // arm and dropped the keystroke — leaving no way to stop a running
                // command. Interrupt is the right action whether or not a command
                // is running: its handler copies a selection when the terminal is
                // idle and only sends the interrupt when something is running.
                if keystroke.key == "c" && keystroke.ctrl && !keystroke.alt && !keystroke.cmd {
                    event_ctx.dispatch_typed_action(TerminalAction::Interrupt);
                    return DispatchEventResult::StopPropagation;
                }
                if command_running && pane_focused {
                    if let Some(bytes) =
                        terminal_keystroke_to_pty_bytes(keystroke, app_cursor_mode, app_keypad_mode)
                    {
                        event_ctx.dispatch_typed_action(TerminalAction::SendPty(bytes));
                        return DispatchEventResult::StopPropagation;
                    }
                } else if !command_running {
                    let editor_focused = focused.is_some_and(|id| id == command_editor_view_id);
                    let vim_normal = editor_focused
                        && command_editor_handle
                            .read(app, |editor, _| editor.vim_blocks_history_propagation());
                    if vim_normal {
                        match keystroke.key.as_str() {
                            "escape" | "up" | "down" | "left" | "right" => {
                                return DispatchEventResult::PropagateToParent;
                            }
                            _ => {}
                        }
                    } else if keystroke.key == "escape"
                        && editor_focused
                        && command_editor_handle
                            .read(app, |editor, _| editor.vim_mode_label().is_some())
                    {
                        return DispatchEventResult::PropagateToParent;
                    }
                    // Command-history popover: Esc dismisses it (restoring the
                    // typed buffer). ↑/↓ and Enter flow through their normal
                    // history/submit actions, which are popover-aware.
                    if cmd_hist_open && keystroke.key == "escape" {
                        event_ctx.dispatch_typed_action(TerminalAction::CloseCmdHistory);
                        return DispatchEventResult::StopPropagation;
                    }
                    // Slash-command palette: while the buffer is a bare `/`-prefixed
                    // token, Up/Down/Enter are claimed for palette navigation instead
                    // of history recall / submit.
                    let slash_open = editor_focused
                        && command_editor_handle.read(app, |editor, _| {
                            slash_command_candidates(editor.get_text(), &slash_project_root)
                                .is_some()
                        });
                    if slash_open
                        && !keystroke.cmd
                        && !keystroke.ctrl
                        && !keystroke.alt
                        && !keystroke.meta
                        && (keystroke.key == "up" || keystroke.key == "down")
                    {
                        event_ctx.dispatch_typed_action(TerminalAction::SlashPaletteMove {
                            forward: keystroke.key == "down",
                        });
                        return DispatchEventResult::StopPropagation;
                    }
                    if slash_open && keystroke.key == "enter" && !keystroke.shift {
                        let name = command_editor_handle.read(app, |editor, _| {
                            slash_command_candidates(editor.get_text(), &slash_project_root)
                                .and_then(|c| {
                                    c.get(slash_palette_selected)
                                        .or_else(|| c.first())
                                        .map(|cmd| cmd.name.to_string())
                                })
                        });
                        if let Some(name) = name {
                            event_ctx
                                .dispatch_typed_action(TerminalAction::RunSlashCommand { name });
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                    if let Some(action) = terminal_keystroke_to_action(keystroke) {
                        // InsertNewline (shift+enter) is already handled by the command_editor's
                        // InputEditor when it has focus. with_always_handle() causes this outer
                        // handler to fire even then, so we skip to avoid a double newline.
                        let editor_focused = focused.is_some_and(|id| id == command_editor_view_id);
                        // An open @-mention popup claims Up/Down (list nav), Enter (accept),
                        // and Escape (dismiss) for itself — InputEditor's own handler already
                        // owns these; skip this outer handler's history/submit/dismiss actions
                        // so they don't double-fire underneath the popup.
                        let mention_open = editor_focused
                            && command_editor_handle.read(app, |editor, _| editor.mention_active());
                        if matches!(action, TerminalAction::InsertNewline) && editor_focused {
                            // InputEditor handles it; don't dispatch a second time.
                        } else if mention_open
                            && matches!(
                                action,
                                TerminalAction::HistoryUp
                                    | TerminalAction::HistoryDown
                                    | TerminalAction::Submit
                                    | TerminalAction::DismissCompletion
                            )
                        {
                            // Owned by the mention popup; don't dispatch here.
                        } else {
                            event_ctx.dispatch_typed_action(action);
                            return DispatchEventResult::StopPropagation;
                        }
                    }
                }
                DispatchEventResult::PropagateToParent
            })
            .finish()
    }

    // ── Layer 6a: Git status helpers ──────────────────────────────────────────
}
