//! Terminal viewport and scroll management.
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
    /// Keyboard focus belongs on the terminal itself, not an input inside
    /// it: a full-screen program or a running command, with no rich input.
    pub(crate) fn keeps_focus_itself(&self, ctx: &ViewContext<Self>) -> bool {
        !self.rich_input_active
            && self
                .model
                .read(ctx, |m, _| m.alt_screen_active() || m.is_command_running())
    }

    pub(crate) fn ensure_command_input_focused(&mut self, ctx: &mut ViewContext<Self>) {
        // Rich input wins even over an active alt-screen: while it's open it's
        // the ONLY input surface — a click landing anywhere on the raw canvas
        // underneath (which stays visible, just not interactive) must not
        // steal focus back to the native PTY passthrough. Checked first for
        // that reason; see `should_forward_key_to_pty_while_ai_panel_open`
        // and the raw-canvas click handlers for the rest of that isolation.
        if self.rich_input_active {
            ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                ectx.focus_self();
            });
            return;
        }
        if self.model.read(ctx, |m, _| m.alt_screen_active()) {
            ctx.focus_self();
            return;
        }
        if self.model.read(ctx, |m, _| m.is_command_running()) {
            ctx.focus_self();
        } else {
            self.focus_command_editor(ctx);
        }
    }

    pub(crate) fn focus_command_editor(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.command_editor, |_editor, editor_ctx| {
            editor_ctx.focus_self();
        });
    }

    pub(crate) fn ai_panel_actions() -> AiPanelActions<TerminalAction> {
        AiPanelActions {
            open_file_picker: TerminalAction::OpenFilePicker,
            toggle_rich_input: TerminalAction::ToggleRichInput,
            toggle_voice_input: TerminalAction::ToggleVoiceInput,
            view_switch: None,
            send: None,
        }
    }

    pub(crate) fn sync_output_viewport_width(&mut self, width: f32, ctx: &mut ViewContext<Self>) {
        if width > 1.0 && (width - self.output_viewport_width_px).abs() > 1.0 {
            self.output_viewport_width_px = width;
            self.rebuild_height_index_preserving_scroll(ctx);
            ctx.notify();
        }
    }

    /// Rebuild block heights without losing the user's current viewport when
    /// wrapping or typography changes alter the content dimensions.
    pub(crate) fn rebuild_height_index_preserving_scroll(&mut self, ctx: &mut ViewContext<Self>) {
        let old_total_height = self.block_height_index.total_height_px();
        let old_viewport_height = self.output_viewport_height_px.max(1.0);
        let old_scroll = *self.block_scroll_pos.lock().unwrap();
        let old_scroll_top = old_scroll.scroll_top_px(old_total_height, old_viewport_height);
        let old_max_scroll = (old_total_height - old_viewport_height).max(0.0);
        let was_following_bottom = matches!(old_scroll.mode, ScrollMode::FollowBottom)
            || old_scroll_top >= old_max_scroll - SCROLL_BOTTOM_EPSILON_PX;

        self.rebuild_height_index(ctx);

        let new_total_height = self.block_height_index.total_height_px();
        let new_max_scroll = (new_total_height - old_viewport_height).max(0.0);
        let new_scroll = if was_following_bottom {
            self.auto_scroll = true;
            ScrollPosition::follow_bottom()
        } else if old_max_scroll > 0.0 {
            // Keep the same relative document position after line reflow.
            let fraction = (old_scroll_top / old_max_scroll).clamp(0.0, 1.0);
            self.auto_scroll = false;
            ScrollPosition::fixed(fraction * new_max_scroll)
        } else {
            self.auto_scroll = false;
            ScrollPosition::fixed(0.0)
        };
        *self.block_scroll_pos.lock().unwrap() = new_scroll;
    }

    pub(crate) fn output_viewport_height(&self, ctx: &ViewContext<Self>) -> f32 {
        let id = format!("terminal_output_viewport_{}", self.pane_uuid);
        ctx.element_position_by_id(&id)
            .map(|rect| rect.height())
            .filter(|height| *height > 1.0)
            .unwrap_or(self.output_viewport_height_px)
    }

    pub(crate) fn schedule_viewport_geometry_sync(&self, ctx: &mut ViewContext<Self>) {
        let id = format!("terminal_output_viewport_{}", self.pane_uuid);
        ctx.spawn(
            Timer::after(Duration::from_millis(0)),
            move |view, _, mut ctx| {
                if let Some(rect) = ctx.element_position_by_id(&id) {
                    view.sync_output_viewport_width(rect.width(), &mut ctx);
                    if rect.height() > 1.0
                        && (rect.height() - view.output_viewport_height_px).abs() > 1.0
                    {
                        view.output_viewport_height_px = rect.height();
                        if view.auto_scroll {
                            view.scroll_to_bottom();
                        }
                        ctx.notify();
                    }
                }
            },
        );
    }

    pub(crate) fn on_pty_lifecycle_event(&mut self, event: PtyEvent, ctx: &mut ViewContext<Self>) {
        match event {
            PtyEvent::ProcessExited(code) => {
                self.model.update(ctx, |m, mctx| {
                    m.set_link_status(ShellLinkStatus::Exited(code), mctx);
                });
            }
            PtyEvent::Error(msg) => {
                log::error!("PTY error: {msg}");
                self.model.update(ctx, |m, mctx| {
                    m.set_link_status(ShellLinkStatus::Error, mctx);
                });
            }
        }
    }

    pub(crate) fn handle_terminal_wakeup(&mut self, ctx: &mut ViewContext<Self>) {
        self.event_listener.clear_pending();
        self.model.update(ctx, |m, mctx| {
            m.drain_pending_effects(mctx);
        });
        // PTY bytes mutate the shared terminal core off the UI thread.  The
        // emitted model events schedule the more expensive height-index rebuild,
        // but the view itself must also be invalidated now.  Otherwise command
        // state (prompt/running/alt-screen changes) can remain visually stale
        // until an unrelated mouse or keyboard event invalidates the window.
        ctx.notify();
    }

    pub(crate) fn on_model_event(&mut self, event: &TerminalEvent, ctx: &mut ViewContext<Self>) {
        // Background recovery/output must not change the currently focused
        // workspace, editor, or settings screen.
        let owns_focus = ctx.focus_handle().is_focused_within(ctx);
        match event {
            TerminalEvent::OutputAppended => {
                self.on_output_appended(ctx);
            }
            TerminalEvent::OutputChunk { block_id, bytes } => {
                // Broadcast this pane's output to shared-session viewers — but
                // only when THIS pane is the one being shared (host side).
                use warpui::SingletonEntity;
                let sm = crate::realtime::manager::SessionManager::handle(ctx);
                let pane = self.pane_uuid;
                let sharing = ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane));
                if sharing {
                    // The raw byte stream feeds a mirror that re-runs the VT
                    // parser (the desktop mirror tab) in ALL modes.
                    let bid = *block_id;
                    let raw = bytes.clone();
                    ctx.update_model(&sm, |m, _| m.broadcast_output(bid, &raw));
                    // Viewers that render styled snapshots (the web block viewer)
                    // get an alt-screen grid while a full-screen program owns the
                    // terminal (vim, htop), and block structure otherwise.
                    let alt = self.model.read(ctx, |m, _| m.alt_screen_active());
                    if alt {
                        // Alt grid is bounded (rows×cols) — snapshot every chunk.
                        let theme = self.theme;
                        let snap = self.model.read(ctx, |m, _| m.alt_screen_snapshot(&theme));
                        ctx.update_model(&sm, |m, _| m.broadcast_alt_screen(snap));
                    } else {
                        // A running block snapshot clones its whole scrollback, so
                        // throttle the per-chunk broadcast (leading + trailing edge,
                        // ~20fps). BlockCompleted also sends the final snapshot.
                        self.broadcast_shared_blocks_throttled(ctx);
                    }
                    self.broadcast_surface_state(ctx);
                }
            }
            TerminalEvent::HostStateRestored => {
                self.send_terminal_colors();
                // The session is live, so its connection notice goes now: it
                // keeps the input box under an alt screen and would make the
                // first real measurement smaller than the pane.
                log::debug!("[resize-debug] HostStateRestored handled");
                self.set_host_notice(None, ctx);
                self.sync_shell_starting_notice(ctx);
                // The restored alt screen is not a transition the TUI caused,
                // so it must not force a redraw resize at an unchanged size.
                self.model.read(ctx, |m, _| m.acknowledge_host_restore());
                self.last_dims = None;
                self.last_alt_active = self.model.read(ctx, |m, _| m.alt_screen_active());
                self.last_rich_input_active = self.rich_input_active;
                self.rebuild_height_index(ctx);
                ctx.notify();
            }
            TerminalEvent::SizeChanged => {
                self.rebuild_height_index(ctx);
                ctx.notify();
                if self.auto_scroll {
                    self.schedule_scroll_to_bottom(ctx, Duration::from_millis(0));
                }
            }
            TerminalEvent::TitleChanged => {
                // Presentation-only. In particular, do not refresh Git or
                // synchronize agent roots: TUIs are allowed to animate titles.
                ctx.notify();
            }
            TerminalEvent::StatusChanged => {
                self.sync_shell_starting_notice(ctx);
                ctx.notify();
                let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
                self.suggest_engine.on_cwd_changed(&cwd);
                self.arm_git_file_watcher(cwd.clone());
                ctx.update_model(&self.git_repo_status, |model, mctx| {
                    model.refresh_bypass_throttle(cwd, mctx);
                });
                // Debounce the agent-roots sync so rapid status/title/cwd
                // sequences only fire once after the user settles. A plain
                // timer is not a debounce: every scheduled closure still runs.
                self.agent_roots_sync_generation = self.agent_roots_sync_generation.wrapping_add(1);
                let generation = self.agent_roots_sync_generation;
                ctx.spawn(
                    Timer::after(Duration::from_millis(500)),
                    move |view, _, ctx| {
                        if view.agent_roots_sync_generation == generation {
                            view.sync_agent_allowed_roots(ctx);
                        }
                    },
                );
            }
            TerminalEvent::BlocksChanged => {
                // Track the foreground command from shell integration, not just
                // local submits: a shared-session controller's command arrives as
                // forwarded PTY bytes and starts via the shell's `preexec` hook
                // with no local `submit_shell_command`. Without this the host
                // never sets `current_running_command`, so AI-tool detection (and
                // the broadcast that drives the viewer's panel) never fires for a
                // web-initiated tool. Only set here; completion does the clearing.
                if !self.is_remote {
                    if let Some(cmd) = self
                        .model
                        .read(ctx, |m, _| m.with_core(|c| c.running_command()))
                    {
                        if self.current_running_command.as_deref() != Some(cmd.as_str()) {
                            self.current_running_command = Some(cmd);
                            self.broadcast_surface_state(ctx);
                        }
                    }
                }
                self.sync_foreground_registry(ctx);
                self.rebuild_height_index(ctx);
                self.arm_agent_ticker(ctx);
                ctx.notify();
                self.follow_output_if_auto_scroll(ctx);
                // A block started, completed, or the list changed — push the new
                // structure to shared-session viewers.
                self.broadcast_shared_blocks(ctx);
            }
            TerminalEvent::BlockCompleted { context } => {
                let was_agent_command = self.pending_agent_run_command.is_some();
                // If the agent routed a run_command through the PTY, post its
                // output back to the server so the LLM loop can continue.
                if let Some((task_id, call_id)) = self.pending_agent_run_command.take() {
                    let output = context.output_text.clone();
                    let is_error = context.failed;
                    crate::agent::checkpoint_store::finish_shell_checkpoint(&call_id);
                    ctx.update_model(&self.agent_controller, |controller, mctx| {
                        if controller.ingest_shell_checkpoint(&call_id) {
                            mctx.emit(
                                crate::agent::controller::AgentControllerEvent::BlocksChanged,
                            );
                        }
                    });
                    crate::bg::detach(move || {
                        if let Err(e) = crate::agent::pipeline::post_tool_result(
                            &task_id, &call_id, &output, is_error,
                        ) {
                            log::warn!("[agent] post_tool_result for run_command: {e}");
                        }
                    });
                    // The agent carries on under the command's output, with the
                    // loading indicator while the LLM processes the tool result.
                    ctx.update_model(&self.agent_controller, |ctrl, mctx| {
                        ctrl.continue_turn_below(mctx);
                    });
                }
                ctx.update_model(&self.agent_controller, |controller, mctx| {
                    controller.on_block_completed(context, mctx);
                });
                self.maybe_proactive_scan(context, was_agent_command, ctx);
                self.rebuild_height_index(ctx);
                ctx.notify();
                self.follow_output_if_auto_scroll(ctx);
                self.auto_scroll = false;
                let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
                self.suggest_engine.on_cwd_changed(&cwd);
                self.on_block_completed_git(context, ctx);
                self.sync_foreground_registry(ctx);
                let still_running = self.model.read(ctx, |m, _| m.is_command_running());
                // Keyboard ownership follows the foreground command: restore
                // normal input-bar key handling the moment nothing is running.
                self.command_input_passthrough
                    .store(still_running, std::sync::atomic::Ordering::Relaxed);
                let exited_tool = self
                    .current_running_command
                    .as_deref()
                    .and_then(|c| c.split_whitespace().next());
                let is_nonaltscreen_ai = matches!(exited_tool, Some("codex") | Some("ollama"));
                if is_nonaltscreen_ai && still_running {
                    // Stay attached
                } else if is_nonaltscreen_ai {
                    self.rich_input_active = false;
                    self.current_running_command = None;
                    self.last_dims = None;
                    if owns_focus { self.ensure_command_input_focused(ctx); }
                } else if !still_running {
                    self.current_running_command = None;
                    if owns_focus { self.ensure_command_input_focused(ctx); }
                    // Re-classify whatever is in the buffer now that input is idle again (AC-6.3).
                    let buffer = self
                        .command_editor
                        .read(ctx, |e, _| e.get_text().to_string());
                    if !buffer.is_empty()
                        && self
                            .input_config
                            .should_run_autodetection(&self.editor_settings)
                    {
                        self.spawn_classify(buffer, ctx);
                    }
                }
                // Push the block's final styled output + completion to viewers.
                self.broadcast_shared_blocks(ctx);
            }
            TerminalEvent::ScreenCleared => {
                self.apply_clear_viewport(ctx);
                self.rebuild_height_index(ctx);
                self.pin_to_bottom(ctx);
                // A submitted `clear` finalizes its block by nulling
                // `active_block_id`, so the usual `BlockCompleted` never fires —
                // and that's what normally restores input focus and clears the
                // keyboard-passthrough flag. Do it here, or the input bar stays
                // dead (keys routed to the PTY, focus on the pane) after `clear`
                // until some other event refocuses it.
                if self.is_remote {
                    // Mirror: a controlling viewer's `clear` forwards to the host
                    // and comes back as a re-parsed ScreenCleared, so the input
                    // needs re-enabling here too. But the mirror's
                    // `is_command_running` is derived from re-parsed host output
                    // and isn't a reliable gate (conditioning on it left the input
                    // dead), so — unless a full-screen TUI owns the keyboard —
                    // just make the command input usable again unconditionally.
                    if !self.rich_input_active {
                        self.command_input_passthrough
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        self.current_running_command = None;
                        if owns_focus { self.focus_command_editor(ctx); }
                    }
                } else {
                    // Local: restore only when nothing is running (mirrors
                    // BlockCompleted's own still-running gate).
                    let still_running = self.model.read(ctx, |m, _| m.is_command_running());
                    self.command_input_passthrough
                        .store(still_running, std::sync::atomic::Ordering::Relaxed);
                    if !still_running {
                        self.current_running_command = None;
                        if owns_focus { self.ensure_command_input_focused(ctx); }
                    }
                }
                ctx.notify();
            }
            TerminalEvent::AltEntered => {
                self.arm_alt_cursor_blink(ctx);
                if self.active_ai_tool(ctx).is_some() {
                    self.rich_input_active = true;
                    if owns_focus {
                        ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                            ectx.focus_self();
                        });
                    }
                } else if owns_focus {
                    ctx.focus_self();
                }
                ctx.notify();
                // Push an initial alt-screen grid so viewers switch to grid mode
                // immediately, before the first repaint chunk arrives.
                use warpui::SingletonEntity;
                let sm = crate::realtime::manager::SessionManager::handle(ctx);
                let pane = self.pane_uuid;
                if ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane)) {
                    let theme = self.theme;
                    let snap = self.model.read(ctx, |m, _| m.alt_screen_snapshot(&theme));
                    ctx.update_model(&sm, |m, _| m.broadcast_alt_screen(snap));
                }
            }
            TerminalEvent::AltExited => {
                self.rich_input_active = false;
                self.set_rich_input_proxy_chars(0, ctx);
                ctx.notify();
                // Tell viewers to leave the alt-screen grid and resume the block
                // list, then push current block structure.
                use warpui::SingletonEntity;
                let sm = crate::realtime::manager::SessionManager::handle(ctx);
                let pane = self.pane_uuid;
                if ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane)) {
                    ctx.update_model(&sm, |m, _| {
                        m.broadcast_delta(dragent_session_protocol::TerminalDelta::AltScreenEnded)
                    });
                    self.broadcast_shared_blocks(ctx);
                }
            }
            TerminalEvent::BlockDraining => {
                // Windows ConPTY: the block completed but its output may still
                // be flushing through ConPTY's repaint. The rendered fence
                // usually finalises it within a few ms; this timer is the
                // fallback so a missing fence can never leave the block
                // unsnapshotted (finalise is a no-op if the fence beat us).
                ctx.spawn(Timer::after(Duration::from_millis(160)), |view, _, ctx| {
                    let theme = view.theme;
                    view.model.update(ctx, |m, mctx| {
                        log::debug!("[drain] fallback timer fired");
                        m.finalise_draining_block(&theme, mctx);
                    });
                });
            }
            TerminalEvent::PtyWrite(_) => {}
            TerminalEvent::ClipboardWrite(text) => {
                // Only the pane the user is working in may replace the
                // clipboard: output in a background tab or from a shared
                // session's host cannot overwrite it behind their back.
                if !self.is_remote && self.pane_has_focus && self.workspace_active {
                    ctx.clipboard()
                        .write(warpui_core::clipboard::ClipboardContent::plain_text(
                            text.clone(),
                        ));
                }
            }
        }
    }

    /// True when the block list scroll position is at (or within epsilon of) the bottom.
    pub(crate) fn is_scrolled_to_bottom(&self, _ctx: &ViewContext<Self>) -> bool {
        let pos = self.block_scroll_pos.lock().unwrap();
        let total = self.block_height_index.total_height_px();
        let scroll_top = pos.scroll_top_px(total, self.output_viewport_height_px);
        let max_scroll = (total - self.output_viewport_height_px).max(0.0);
        scroll_top >= max_scroll - SCROLL_BOTTOM_EPSILON_PX
    }

    /// The block-list scroll position as a fraction of the scrollable range
    /// (`0.0` = top, `1.0` = bottom), or `None` when following the bottom. Sent to
    /// shared-session viewers so their mirror scrolls where the host scrolls,
    /// independent of window/font height differences. See
    /// [`TerminalSurfaceState::scroll_fraction`].
    pub(crate) fn scroll_fraction_for_share(&self) -> Option<f32> {
        let pos = self.block_scroll_pos.lock().unwrap();
        if matches!(pos.mode, ScrollMode::FollowBottom) {
            return None;
        }
        let total = self.block_height_index.total_height_px();
        let vp = self.output_viewport_height_px;
        let max_scroll = (total - vp).max(0.0);
        if max_scroll <= 0.0 {
            return None; // nothing to scroll — treat as bottom
        }
        let scroll_top = pos.scroll_top_px(total, vp);
        Some((scroll_top / max_scroll).clamp(0.0, 1.0))
    }

    /// Viewer: place the mirror's scroll at the host's `scroll_fraction`. `None`
    /// re-pins to the bottom (follow new output). Applied only on a read-only
    /// mirror, so the viewer tracks the host without a scroll feedback loop.
    pub(crate) fn apply_remote_scroll(&mut self, fraction: Option<f32>) {
        match fraction {
            None => {
                *self.block_scroll_pos.lock().unwrap() = ScrollPosition::follow_bottom();
                self.auto_scroll = true;
            }
            Some(f) => {
                let total = self.block_height_index.total_height_px();
                let vp = self.output_viewport_height_px;
                let max_scroll = (total - vp).max(0.0);
                let top = (f.clamp(0.0, 1.0) as f32 * max_scroll).max(0.0);
                *self.block_scroll_pos.lock().unwrap() = ScrollPosition::fixed(top);
                self.auto_scroll = false;
            }
        }
    }

    /// Reconcile `auto_scroll` state after async layout/scroll transitions.
    pub(crate) fn sync_auto_scroll_from_position(&mut self, ctx: &ViewContext<Self>) {
        self.auto_scroll = self.is_scrolled_to_bottom(ctx);
    }

    pub(crate) fn schedule_sync_auto_scroll(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(Duration::from_millis(0)),
            move |view, _, ctx| {
                view.sync_auto_scroll_from_position(ctx);
                ctx.notify();
            },
        );
    }

    pub(crate) fn sync_sticky_header(&mut self, ctx: &ViewContext<Self>) {
        if self.block_height_index.is_empty() {
            self.sticky_header = None;
            self.sticky_push_offset = 0.0;
            return;
        }

        let total_h = self.block_height_index.total_height_px();
        let scroll_top = self
            .block_scroll_pos
            .lock()
            .unwrap()
            .scroll_top_px(total_h, self.output_viewport_height_px);

        let entries = self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .map(|b| {
                        (
                            b.id,
                            b.command.clone(),
                            b.status,
                            b.duration_ms,
                            b.expanded,
                            b.is_running(),
                            b.started_at,
                        )
                    })
                    .collect::<Vec<_>>()
            })
        });
        let heal_states = self
            .self_heal_store
            .read(ctx, |store, _| store.all_states().clone());
        let auto_fix = crate::settings::AppSettings::global()
            .agent
            .auto_fix_on_failure;

        let indexed_entries = self.block_height_index.visible_entries(0.0, f32::INFINITY);

        // Scan entries top-to-bottom to find which block owns the sticky header.
        //
        // Rules:
        //   - A Block whose header has fully scrolled off (partial >= BLOCK_HEADER_HEIGHT)
        //     is a confirmed sticky candidate.
        //   - A Block whose header is entering the viewport (0 <= partial < BLOCK_HEADER_HEIGHT)
        //     immediately takes over as the sticky candidate once any previous block was
        //     confirmed — instant content switch, no push animation.
        //   - Non-Block spacing entries (BlockSpacing, …) are transparent: they
        //     leave the candidate unchanged so the previous block's header stays
        //     pinned across any inter-block gap.
        //   - A ViewportClearGap is NOT transparent: `clear` is a hard reset —
        //     the whole point is a blank slate, so a header pinned from before
        //     the clear must not keep hanging around once scrolled to/past it.
        //   - An AgentBlock is also a hard ownership boundary. Agent cards do
        //     not have sticky headers, so a command header must be removed as
        //     soon as the viewport enters an agent-only section.
        let mut sticky_block_id: Option<usize> = None;
        let mut has_confirmed = false;

        for ie in &indexed_entries {
            let block_top = ie.content_top_px;

            if block_top > scroll_top {
                break;
            }

            match ie.entry {
                crate::terminal::model::block_height_tree::BlockListEntry::Block {
                    block_id,
                    ..
                } => {
                    let partial = scroll_top - block_top;
                    if partial >= BLOCK_HEADER_HEIGHT {
                        has_confirmed = true;
                        sticky_block_id = Some(block_id);
                    } else if has_confirmed {
                        // Incoming block's header has reached the sticky-header position —
                        // switch immediately with no animation.
                        sticky_block_id = Some(block_id);
                    }
                    // else: first block, header still visible — don't show sticky yet.
                }
                crate::terminal::model::block_height_tree::BlockListEntry::ViewportClearGap {
                    ..
                } => {
                    sticky_block_id = None;
                    has_confirmed = false;
                }
                crate::terminal::model::block_height_tree::BlockListEntry::AgentBlock {
                    ..
                } => {
                    sticky_block_id = None;
                    has_confirmed = false;
                }
                _ => {}
            }
        }

        let new_header = sticky_block_id.and_then(|block_id| {
            entries.iter().find(|e| e.0 == block_id).map(|info| {
                let heal_state = heal_states
                    .get(&block_id)
                    .cloned()
                    .unwrap_or_default()
                    .projected_for_display(
                        auto_fix,
                        self.auto_analyzed_blocks.contains(&block_id),
                        self.revealed_fix_blocks.contains(&block_id),
                    );
                StickyBlockHeader {
                    id: info.0,
                    command: info.1.clone(),
                    status: info.2,
                    duration_ms: info.3,
                    expanded: info.4,
                    running: info.5,
                    heal_state,
                    started_at: info.6,
                }
            })
        });

        self.sticky_header = new_header;
        self.sticky_push_offset = 0.0;
    }

    pub(crate) fn schedule_sync_sticky_header(&self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(Duration::from_millis(0)),
            move |view, _, ctx| {
                view.sync_sticky_header(ctx);
                ctx.notify();
            },
        );
    }

    /// Scroll to the latest output when auto-follow is enabled.
    pub(crate) fn follow_output_if_auto_scroll(&self, ctx: &mut ViewContext<Self>) {
        if !self.auto_scroll {
            // SCROLL LOCK ACTIVE: Block frame updates paint below viewport area silently.
            return;
        }
        self.schedule_scroll_to_bottom(ctx, Duration::from_millis(0));
    }

    pub(crate) fn schedule_scroll_to_bottom(&self, ctx: &mut ViewContext<Self>, delay: Duration) {
        ctx.spawn(Timer::after(delay), move |view, _, ctx| {
            if view.model.read(ctx, |m, _| m.alt_screen_active()) {
                return;
            }
            if view.auto_scroll {
                view.scroll_to_bottom();
                ctx.notify();
            }
        });
    }

    /// Rebuild the block height index from current model state.
    /// Coalesces rapid OutputAppended events into one repaint per ~16 ms frame.
    /// With the model-level dedup (apply_effects emits at most 1 OutputAppended per drain
    /// cycle), this is called at most once per PTY wakeup. The pending guard ensures that
    /// during a sustained burst we do exactly one rebuild+notify per frame, not one per chunk.
    pub(crate) fn on_output_appended(&mut self, ctx: &mut ViewContext<Self>) {
        if self.output_rebuild_pending {
            return;
        }
        self.output_rebuild_pending = true;
        let activity = if self.workspace_active {
            ActivityLevel::Foreground
        } else {
            ActivityLevel::Background
        };
        let delay = ResourceBudget::for_state(activity, MemoryPressure::Normal).repaint_interval;
        ctx.spawn(Timer::after(delay), |view, _, ctx| {
            view.output_rebuild_pending = false;
            view.rebuild_height_index(ctx);
            view.last_height_rebuild_at = Some(Instant::now());
            // A pane can receive its final PTY output while workspace focus is
            // transitioning.  Always publish the completed layout rebuild; the
            // background resource budget above already limits repaint cadence.
            // Skipping this notification left the visible terminal stale until
            // the user clicked somewhere in the app.
            ctx.notify();
            if view.workspace_active && view.auto_scroll {
                view.schedule_scroll_to_bottom(ctx, Duration::from_millis(0));
            }
        });
    }

    /// Change this pane's workspace visibility without pausing its underlying
    /// process. Returning to the foreground performs one authoritative rebuild,
    /// then resumes only the visual timers that are actually needed.
    pub fn set_workspace_active(&mut self, active: bool, ctx: &mut ViewContext<Self>) {
        if !active {
            self.pane_has_focus = false;
        }
        if self.workspace_active == active {
            return;
        }
        self.workspace_active = active;
        if !active {
            self.stop_git_file_watcher();
            return;
        }

        if let Some(cwd) = self.git_watch_cwd.clone() {
            self.arm_git_file_watcher(cwd);
        }
        self.rebuild_height_index(ctx);
        if self.auto_scroll {
            self.schedule_scroll_to_bottom(ctx, Duration::from_millis(0));
        }
        if self.side_card_open && self.side_shell.is_some() {
            self.arm_side_poll(ctx);
        }
        if self.model.read(ctx, |model, _| model.alt_screen_active()) {
            self.arm_alt_cursor_blink(ctx);
        }
        self.arm_agent_ticker(ctx);
        ctx.notify();
    }

    /// Called on model events that change the block list structure or live block content.
    /// Also drains the measured_block_heights sink written by BlockListElement::after_layout.
    pub(crate) fn rebuild_height_index(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::terminal::model::block_height_tree::{
            estimate_block_height, estimate_dir_listing_height,
        };
        let cell_h = self.cell_metrics.height;
        let theme = self.theme;
        let dir_view = self.dir_view;
        let viewport_width = self.output_viewport_width_px;

        // Collect only primitive data to avoid cloning non-Clone Block.
        // The shared prompt_line_count() helper is used here (index path) and in
        // build_block_list_child (paint path) to keep the two in sync.
        // Running block (has a live emulator) captured for the height diagnostic log.
        let mut running_dbg: Option<(usize, f32, usize)> = None;
        let (block_data, bootstrap_line_count, prompt_lines) = self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                // The `clear`/`cls` command's own block: the active gap already
                // stands in for it visually, so it must not ALSO get a block
                // entry — that's what left it rendering at the bottom, its own
                // (empty) header visible right below the gap.
                let hidden_block_id = core.active_clear_gap().and_then(|g| g.trigger_block_id);
                let blocks = core.blocks().blocks();
                let data: Vec<(usize, f32, i64)> = blocks
                    .iter()
                    .filter(|b| Some(b.id) != hidden_block_id)
                    .map(|b| {
                        let h = if !b.expanded {
                            crate::terminal::model::block_height_tree::BLOCK_HEADER_HEIGHT_PX
                        } else if let Some(listing) = b
                            .listing(&theme)
                            .filter(|_| b.status.exit_code().unwrap_or(0) == 0)
                        {
                            let content_width = dir_grid_content_width(viewport_width);
                            estimate_dir_listing_height(&listing, dir_view, content_width)
                        } else {
                            estimate_block_height(b, cell_h)
                        };
                        if let Some(emu) = &b.emulator {
                            running_dbg = Some((
                                b.id,
                                h,
                                crate::terminal::model::block::live_primary_line_count(emu),
                            ));
                        }
                        (b.id, h, b.created_at_unix_ms)
                    })
                    .collect();
                (
                    data,
                    core.blocks().bootstrap_lines.len(),
                    prompt_line_count(core),
                )
            })
        });
        // Update cached prompt height; single source of truth for index + paint.
        self.live_prompt_height_px = (prompt_lines as f32) * cell_h;
        let bootstrap_height = (bootstrap_line_count as f32) * cell_h;
        // Merge measured heights from BlockListElement::after_layout (written via shared Arc).
        let measured = self.measured_block_heights.lock().unwrap();
        let running_id = running_dbg.map(|(id, _, _)| id);
        let final_block_data: Vec<(usize, f32, i64)> = block_data
            .into_iter()
            .map(|(id, estimated, created_at_unix_ms)| {
                // The running block grows every output tick, and its measured height in
                // the sink lags one frame behind the content. The output-triggered
                // rebuild would then use a stale height and the measured rebuild a fresh
                // one — the viewport wobbles between the two. Use the fresh estimate
                // (which reads the current emulator line count) for the running block so
                // both rebuild paths produce the same height. Requires the estimate to
                // match the measured formula (see estimate_block_height).
                let h = if Some(id) == running_id {
                    estimated
                } else {
                    measured.get(&id).copied().unwrap_or(estimated)
                };
                (id, h, created_at_unix_ms)
            })
            .collect();
        drop(measured);
        let live_prompt_h = self.live_prompt_height_px;

        let agent_block_data: Vec<(usize, f32, i64)> = self.agent_controller.read(ctx, |c, _| {
            c.agent_blocks()
                .iter()
                .map(|b| (b.id, b.kind.estimated_height_px(), b.created_at_unix_ms))
                .collect()
        });

        let measured_agent = self.measured_agent_block_heights.lock().unwrap();
        let final_agent_block_data: Vec<(usize, f32, i64)> = agent_block_data
            .into_iter()
            .map(|(id, estimated, created_at_unix_ms)| {
                let h = measured_agent.get(&id).copied().unwrap_or(estimated);
                (id, h, created_at_unix_ms)
            })
            .collect();
        drop(measured_agent);

        let heights_map: std::collections::HashMap<usize, f32> =
            final_block_data.iter().map(|&(id, h, _)| (id, h)).collect();
        let agent_heights_map: std::collections::HashMap<usize, f32> = final_agent_block_data
            .iter()
            .map(|&(id, h, _)| (id, h))
            .collect();
        let gap_trigger = self.model.read(ctx, |m, _| {
            m.with_core(|core| core.active_clear_gap().and_then(|g| g.trigger_block_id))
        });

        let mut shell_blocks_created: std::collections::HashMap<usize, i64> = final_block_data
            .iter()
            .map(|&(id, _, created)| (id, created))
            .collect();
        if let Some(trigger_id) = gap_trigger {
            if let Some(created) = self.model.read(ctx, |m, _| {
                m.with_core(|core| core.blocks().get(trigger_id).map(|b| b.created_at_unix_ms))
            }) {
                shell_blocks_created.insert(trigger_id, created);
            }
        }
        let agent_blocks_created: std::collections::HashMap<usize, i64> = final_agent_block_data
            .iter()
            .map(|&(id, _, created)| (id, created))
            .collect();

        self.model.update(ctx, |m, mctx| {
            m.shrink_active_clear_gap(
                &heights_map,
                &agent_heights_map,
                &shell_blocks_created,
                &agent_blocks_created,
                live_prompt_h,
                mctx,
            );
        });

        let gap = self.model.read(ctx, |m, _| {
            m.with_core(|core| core.active_clear_gap().cloned())
        });
        let (gap_h, gap_before, gap_after, gap_trigger) = match gap {
            Some(g) => (
                g.current_height_px,
                g.before_block_id,
                g.after_all,
                g.trigger_block_id,
            ),
            None => (0.0, None, false, None),
        };

        let mut chronological_data: Vec<(
            i64,
            u8,
            usize,
            crate::terminal::model::block_height_tree::ChronologicalEntryData,
        )> = Vec::new();
        for &(id, h, created) in &final_block_data {
            chronological_data.push((
                created,
                0,
                id,
                crate::terminal::model::block_height_tree::ChronologicalEntryData::Block {
                    block_id: id,
                    height_px: h,
                    order_ms: created,
                },
            ));
        }
        for &(id, h, created) in &final_agent_block_data {
            chronological_data.push((
                created,
                1,
                id,
                crate::terminal::model::block_height_tree::ChronologicalEntryData::AgentBlock {
                    agent_block_id: id,
                    height_px: h,
                    order_ms: created,
                },
            ));
        }

        // Sort stable by creation time
        chronological_data.sort_by_key(|&(created, kind, id, _)| (created, kind, id));

        let final_chronological_data: Vec<
            crate::terminal::model::block_height_tree::ChronologicalEntryData,
        > = chronological_data
            .into_iter()
            .map(|(_, _, _, data)| data)
            .collect();

        let clear_gap_before_order_ms = gap_trigger.or(gap_before).and_then(|id| {
            self.model.read(ctx, |m, _| {
                m.with_core(|core| core.blocks().get(id).map(|b| b.created_at_unix_ms))
            })
        });
        self.block_height_index.rebuild_from_entries(
            bootstrap_height,
            gap_h,
            gap_before,
            gap_after,
            live_prompt_h,
            clear_gap_before_order_ms,
            &final_chronological_data,
        );

        if std::env::var_os("DRAGENT_LOG_HEIGHT").is_some() {
            let total = self.block_height_index.total_height_px();
            let vp = self.output_viewport_height(ctx);
            let (rid, rest, rmeas, rlog) = match running_dbg {
                Some((id, est, logical)) => {
                    let measured = self
                        .measured_block_heights
                        .lock()
                        .unwrap()
                        .get(&id)
                        .copied();
                    (id as i64, est, measured, logical as i64)
                }
                None => (-1, 0.0, None, -1),
            };
            // Watch `total` across consecutive rebuilds (timestamps show the rate):
            // if it ping-pongs instead of climbing monotonically, that's the shake.
            // `est` vs `measured` is the index/paint height disagreement for the
            // running block; `logical_lines` vs measure log's `wrapped_rows` shows
            // whether soft-wrapping is the cause.
            log::info!(
                "[height] rebuild total={:.1} viewport={:.1} bottom_anchored={} auto_scroll={} | running block={} est={:.1} measured={:?} logical_lines={}",
                total, vp, total < vp, self.auto_scroll, rid, rest, rmeas, rlog,
            );
        }
    }

    pub(crate) fn pin_to_bottom(&mut self, ctx: &mut ViewContext<Self>) {
        self.auto_scroll = true;
        self.scroll_to_bottom();
        ctx.notify();
    }

    pub(crate) fn scroll_to_bottom(&mut self) {
        *self.block_scroll_pos.lock().unwrap() = ScrollPosition::follow_bottom();
        // Legacy ClippedScrollable path (bootstrap/non-block mode).
        self.block_scroll_state.scroll_to(Pixels::new(f32::MAX));
        self.block_scroll_state.scroll_to_position(ScrollTarget {
            position_id: BLOCK_LIST_BOTTOM_POSITION_ID.to_string(),
            mode: ScrollToPositionMode::FullyIntoView,
        });
    }

    /// Leave follow-bottom mode while a block filter changes the layout. The
    /// filtered block can become shorter on every keystroke, so retaining the
    /// FollowBottom marker would move the viewport to the application's tail.
    pub(crate) fn freeze_block_scroll_position(&mut self) {
        let total = self.block_height_index.total_height_px();
        let viewport = self.output_viewport_height_px.max(1.0);
        let current_top = self
            .block_scroll_pos
            .lock()
            .unwrap()
            .scroll_top_px(total, viewport);
        self.auto_scroll = false;
        *self.block_scroll_pos.lock().unwrap() = ScrollPosition::fixed(current_top.max(0.0));
    }
}
