//! Terminal command pipeline: submit, classify, intel, completion.
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
    pub(crate) fn submit_shell_command_inner(
        &mut self,
        command: String,
        ctx: &mut ViewContext<Self>,
    ) {
        self.last_submit_was_agent = false;
        if self.command_history.last().map(String::as_str) != Some(command.as_str()) {
            self.command_history.push(command.clone());
        }
        crate::console::shell_history::record(&command);
        self.previous_command = Some(command.clone());
        self.command_history_idx = None;
        self.history_recall_prefix = None;
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        // Controller → host: `set_text` fires no Changed event, so forward the
        // cleared command line explicitly — otherwise the host keeps the stale
        // text this controller just submitted (it only receives the PTY command).
        self.forward_control_surface(
            dragent_session_protocol::ControlSurface {
                command_input: Some(String::new()),
                command_cursor: Some(0),
                ..Default::default()
            },
            ctx,
        );
        self.record_command_for_suggestions(&command, ctx);
        self.current_running_command = Some(command.clone());
        ctx.update_model(&self.model, |m, mctx| {
            m.begin_command(command.clone(), mctx);
        });
        // Foreground command owns the keyboard now — make the collapsed input
        // bar inert so keys reach the PTY (cleared when the command finishes).
        self.command_input_passthrough
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let mode = self.active_term_mode(ctx);
        self.write_command(&command, mode);
        crate::confirm::set_foreground_command(self.pane_uuid, Some(command.clone()));
        self.pin_to_bottom(ctx);
        ctx.spawn(
            Timer::after(Duration::from_millis(0)),
            move |view, _, mut ctx| {
                view.ensure_command_input_focused(&mut ctx);
            },
        );
    }

    pub(crate) fn clear_input_after_submit(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text("", ectx);
            editor.set_ghost_suffix(None, ectx);
        });
        if self.is_remote_controller() {
            // set_text is intentionally silent, so send the cleared surface
            // explicitly and reject any stale host echo during its round trip.
            self.last_local_input_edit = Instant::now();
            self.forward_control_surface(
                dragent_session_protocol::ControlSurface {
                    command_input: Some(String::new()),
                    command_cursor: Some(0),
                    ..Default::default()
                },
                ctx,
            );
        }
        ctx.notify();
    }

    /// Always Shell when a process owns the terminal (running / alt-screen / ai-repl).
    pub(crate) fn effective_input_type(&self, ctx: &ViewContext<Self>) -> DetectedInputType {
        if self.uses_agent_transcript() { return DetectedInputType::Agent; }
        if !self.local_input_active(ctx) {
            return DetectedInputType::Shell;
        }
        self.input_config.input_type
    }

    pub(crate) fn toggle_input_type_lock(&mut self, ctx: &mut ViewContext<Self>) {
        self.input_config.toggle_lock();
        self.sync_input_placeholder(ctx);
        ctx.notify();
    }

    /// Update the command editor placeholder to match the current detected input type.
    pub(crate) fn sync_input_placeholder(&mut self, ctx: &mut ViewContext<Self>) {
        let input_type = self.input_config.input_type;
        let placeholder = match input_type {
            DetectedInputType::Agent => "Ask Dragent\u{2026}",
            DetectedInputType::Shell => "Ask AI or run a command",
        };
        let is_agent = input_type == DetectedInputType::Agent;
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_placeholder(placeholder, ectx);
            editor.set_agent_mode(is_agent, ectx);
        });
    }

    pub fn set_tab_session_kind(&mut self, kind: TabSessionKind) {
        self.tab_session_kind = kind;
        let autodetect = self.editor_settings.input_autodetection_enabled;
        self.input_config = InputConfig::for_tab_kind(kind, autodetect);
    }

    /// Bind this session to an agent-registry identity. The chip, rail, and
    /// input editor resolve name + image from the definition; detection
    /// remains as fallback for sessions predating this field.
    pub fn set_agent_id(&mut self, agent_id: Option<String>) {
        self.agent_id = agent_id;
    }

    pub(crate) fn build_classify_context(&self, ctx: &ViewContext<Self>) -> ClassifyContext {
        let last_block_failed = self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .rev()
                    .find(|b| b.is_completed())
                    .and_then(|b| b.completion.as_ref())
                    .is_some_and(|c| c.failed)
            })
        });
        ClassifyContext {
            current_type: self.input_config.input_type,
            recent_commands: self.command_history.clone(),
            last_block_failed,
            after_agent_submit: self.last_submit_was_agent,
        }
    }

    /// Spawn a debounced heuristic classification. The generation counter aborts stale results.
    pub(crate) fn spawn_classify(&mut self, text: String, ctx: &mut ViewContext<Self>) {
        // A dedicated agent conversation stays a conversation while typing.
        // Explicit /shell or input-mode selection still opens the shell view.
        if self.is_builtin_agent_tab() { return; }
        if text.trim().is_empty() {
            return;
        }

        self.classify_generation = self.classify_generation.wrapping_add(1);
        let generation = self.classify_generation;
        let classify_ctx = self.build_classify_context(ctx);
        ctx.spawn(
            Timer::after(Duration::from_millis(60)),
            move |view, _, mut ctx| {
                if view.classify_generation != generation {
                    return;
                }
                if view.input_config.is_locked || view.is_builtin_agent_tab() {
                    return;
                }
                let mut ctx_snapshot = classify_ctx.clone();
                ctx_snapshot.current_type = view.input_config.input_type;
                let Some(result) = HeuristicInputClassifier::new().classify(&text, &ctx_snapshot)
                else {
                    return;
                };
                if view.input_config.input_type != result {
                    view.input_config.input_type = result;
                    view.sync_input_placeholder(&mut ctx);
                    ctx.notify();
                }
            },
        );
    }

    pub(crate) fn record_command_for_suggestions(
        &mut self,
        command: &str,
        ctx: &ViewContext<Self>,
    ) {
        if let Some(dir_fragment) = command.strip_prefix("cd ") {
            let dir_fragment = dir_fragment.trim();
            if dir_fragment.is_empty() {
                return;
            }
            let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
            let resolved = if dir_fragment.starts_with('/') || dir_fragment.starts_with('~') {
                std::path::PathBuf::from(dir_fragment)
            } else {
                cwd.join(dir_fragment)
            };
            self.suggest_engine.record_cd(resolved);
        }
    }

    /// The AI-tool rich input. Only `!`-prefixed text (a scratch-shell command)
    /// gets a suggestion ghost — normal prose is left alone.
    pub(crate) fn on_rich_input_editor_event(
        &mut self,
        event: &InputEditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            InputEditorEvent::Changed { text, cursor } => {
                ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                    editor.set_remote_caret(None, ectx)
                });
                // Typing/deleting dismisses the `!` history popover too (same
                // reasoning as the command editor — previews use `set_text`, which
                // emits no Changed, so only real edits close it).
                if self.cmd_hist_open {
                    self.close_cmd_history(false, ctx);
                    self.restore_history_input_config(ctx);
                }
                // Local edit — suppress the host's echo on a controlling mirror.
                self.last_local_input_edit = Instant::now();
                self.refresh_rich_ghost(text, ctx);
                // Host → viewers: mirror the live AI-tool input as it's typed
                // (no-op unless this is the shared pane).
                self.broadcast_surface_state(ctx);
                // Controller → host: forward the live AI-tool input so the host
                // desktop reflects it (no-op unless this is a controlling mirror).
                self.forward_control_surface(
                    dragent_session_protocol::ControlSurface {
                        ai_input: Some(text.clone()),
                        ai_cursor: Some(*cursor),
                        ..Default::default()
                    },
                    ctx,
                );
            }
            // Arrow-button click (the editor's own accept affordance).
            InputEditorEvent::AcceptGhost => {
                self.accept_rich_ghost(ctx);
            }
            _ => {}
        }
    }

    /// Autocomplete for `!cmd` in the AI input: reuse the terminal's suggest
    /// engine against the scratch-shell's own history (then the terminal
    /// history), showing the completion as a ghost after the cursor.
    pub(crate) fn refresh_rich_ghost(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        let ghost = match text.strip_prefix('!') {
            Some(rest) if !rest.trim().is_empty() && !self.cmd_hist_open => {
                let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
                self.suggest_engine.on_cwd_changed(&cwd);
                let mut history = self.side_cmd_history.clone();
                history.extend(self.command_history.iter().cloned());
                self.suggest_engine.suggest(rest, &history, &cwd)
            }
            _ => None,
        };
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            editor.set_ghost_suffix(ghost, ectx);
        });
    }

    pub(crate) fn accept_rich_ghost(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            editor.accept_ghost_suffix(ectx);
        });
        ctx.notify();
    }

    pub(crate) fn on_command_editor_event(
        &mut self,
        event: &InputEditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            InputEditorEvent::Changed { text, cursor } => {
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.set_remote_caret(None, ectx)
                });
                self.command_history_idx = None;
                self.history_recall_prefix = None;
                // Typing or deleting dismisses the history popover immediately —
                // the user has left browsing and is composing again. Safe to hang
                // off `Changed`: arrow-key navigation previews via `set_text`,
                // which does NOT emit this event, so only real edits close it.
                // `restore: false` keeps what's in the input rather than snapping
                // back to the pre-browse buffer. Pressing ↑/↓ afterwards reopens
                // history filtered by whatever is now typed.
                if self.cmd_hist_open {
                    self.close_cmd_history(false, ctx);
                    // `open_cmd_history` pins the input to Shell so a previewed
                    // command can't be reclassified as an agent request. Closing
                    // without `restore` skips that undo, which would leave the
                    // input mode-locked once the user starts composing again.
                    self.restore_history_input_config(ctx);
                }
                // Mark this as a genuine LOCAL edit so a controlling mirror
                // suppresses the host's echo of it (avoids cursor-clobber).
                self.last_local_input_edit = Instant::now();
                self.refresh_input_intel(text, *cursor, ctx);
                self.on_input_changed(text, ctx);
                // Host → viewers: mirror the live command line on every keystroke
                // (no-op unless this is the shared pane).
                self.broadcast_surface_state(ctx);
                // Controller → host: forward the live command line so the host
                // desktop reflects what the web participant types (no-op unless
                // this is a controlling mirror).
                self.forward_control_surface(
                    dragent_session_protocol::ControlSurface {
                        command_input: Some(text.clone()),
                        command_cursor: Some(*cursor),
                        ..Default::default()
                    },
                    ctx,
                );
            }
            InputEditorEvent::Clicked(_) => {
                ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
                self.focus_command_editor(ctx);
                ctx.emit(());
                ctx.notify();
            }
            InputEditorEvent::AcceptGhost => {
                self.accept_command_ghost(ctx);
            }
            InputEditorEvent::InsertLastWord => {
                self.insert_last_word(ctx);
            }
            InputEditorEvent::HistoryNavigate { previous } => {
                self.recall_command_history(*previous, ctx);
            }
            InputEditorEvent::MentionQueryChanged { anchor, query } => {
                self.refresh_mention_candidates(*anchor, query.clone(), ctx);
            }
            InputEditorEvent::MentionClosed => {}
            // The command editor can share the generic InputEditor surface,
            // but document-workspace context-menu actions are handled by the
            // project sidebar owner. They are intentionally ignored here.
            InputEditorEvent::ContextMenuCommand { .. } => {}
            InputEditorEvent::DefinitionHoverRequested { .. }
            | InputEditorEvent::DefinitionHoverClicked { .. } => {}
            InputEditorEvent::GitDiffProjectionRowsChanged { .. } => {}
            InputEditorEvent::BreakpointsChanged { .. } | InputEditorEvent::DiffCommentSelection { .. } => {}
        }
    }

    /// Re-score `@`-mention candidates against the live project file index
    /// for the pane's cwd and push the results back into the command editor.
    pub(crate) fn refresh_mention_candidates(
        &mut self,
        anchor: usize,
        query: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
        let (sender, receiver) = async_channel::bounded(1);
        crate::bg::detach(move || {
            let _ = sender.send_blocking(crate::console::mention_popup::project_files(&cwd));
        });
        ctx.spawn(async move { receiver.recv().await.ok() }, move |view, files, ctx| {
            let Some((root, paths)) = files else {
                return;
            };
            let entries =
                crate::console::mention_popup::score_and_filter(&paths, &root, &query, 20);
            ctx.update_view(&view.command_editor, |editor, ectx| {
                editor.set_mention_candidates(anchor, entries, ectx);
            });
        });
    }

    /// React to edits in the embedded block-filter editor: sync the query into
    /// the active filter and re-filter the block output.
    pub(crate) fn on_block_filter_editor_event(
        &mut self,
        event: &InputEditorEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        if let InputEditorEvent::Changed { text, .. } = event {
            // A queued change from an editor that was just unmounted must not
            // re-filter the terminal or disturb its scroll position.
            if self.block_filter.is_none() {
                return;
            }
            self.freeze_block_scroll_position();
            if let Some((_id, filter)) = self.block_filter.as_mut() {
                filter.set_query(text);
                self.block_filter_match_index = 0;
            }
            self.rebuild_height_index(ctx);
            ctx.notify();
        }
    }

    pub(crate) fn refresh_input_intel(
        &mut self,
        _text: &str,
        _cursor_char: usize,
        ctx: &mut ViewContext<Self>,
    ) {
        // The per-token description ("ls → List directory contents") is no longer
        // shown: it explained commands the user already knows and added a line of
        // chatter under every keystroke. Always clear it, so any hint left over
        // from a previous build or code path disappears.
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_token_hint(None, ectx);
        });
    }

    pub(crate) fn insert_last_word(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(prev) = self.previous_command.as_deref() else {
            return;
        };
        let Some(word) = input_editing::last_word_of(prev) else {
            return;
        };
        let suffix = format!(" {word}");
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.insert_text_at_cursor(&suffix, ectx);
        });
        ctx.notify();
    }

    pub(crate) fn maybe_expand_alias(&mut self, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        if !text.ends_with(' ') {
            return false;
        }
        let trimmed = text.trim_end();
        let Some(expanded) = self.shell_aliases.try_expand(trimmed) else {
            return false;
        };
        let new_text = format!("{expanded} ");
        if new_text == text {
            return false;
        }
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&new_text, ectx);
        });
        self.refresh_input_intel(&new_text, new_text.chars().count(), ctx);
        true
    }

    pub(crate) fn on_input_changed(&mut self, text: &str, ctx: &mut ViewContext<Self>) {
        // Reset selection on every edit — the palette's candidate set is
        // recomputed fresh from `text` at render time (see
        // `slash_command_candidates`), so a stale index would otherwise
        // point past the end of a freshly-narrowed list.
        self.slash_palette_selected = 0;
        if self.history_search.open || self.workflow_search.open {
            return;
        }
        if self.workflow_session.is_some() {
            self.workflow_session = None;
            self.sync_workflow_highlight(ctx);
        }
        if self.maybe_expand_alias(text, ctx) {
            ctx.notify();
            return;
        }

        // Show immediate badge feedback for mode-switching slash commands.
        // The badge updates as soon as the command is fully typed; pressing Enter finalises it.
        match text.trim() {
            "/agent" if self.input_config.input_type != DetectedInputType::Agent => {
                self.input_config.input_type = DetectedInputType::Agent;
                self.input_config.is_locked = true;
                self.sync_input_placeholder(ctx);
                ctx.notify();
                return;
            }
            "/shell" if self.input_config.input_type != DetectedInputType::Shell => {
                self.input_config.input_type = DetectedInputType::Shell;
                self.input_config.is_locked = true;
                self.sync_input_placeholder(ctx);
                ctx.notify();
                return;
            }
            _ => {}
        }

        let completion_open = self.path_picker.read(ctx, |m, _| m.open);
        if completion_open {
            let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
            if text == "cd" || !should_suggest_paths(text) {
                ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
            } else {
                let with_help = self.input_config.input_type == DetectedInputType::Agent;
                ctx.update_model(&self.path_picker, |m, model_ctx| {
                    m.try_open_for_buffer(text, &cwd, with_help, model_ctx);
                });
            }
        }
        self.refresh_command_ghost(text, ctx);

        // Debounced heuristic classification (skipped when terminal is busy or input is locked)
        if self.local_input_active(ctx)
            && self
                .input_config
                .should_run_autodetection(&self.editor_settings)
        {
            let snapshot = text.to_string();
            self.spawn_classify(snapshot, ctx);
        }

        ctx.notify();
    }

    pub(crate) fn apply_path_completion(
        &mut self,
        new_buffer: String,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_text(&new_buffer, ectx);
        });
        self.on_input_changed(&new_buffer, ctx);
    }
}
