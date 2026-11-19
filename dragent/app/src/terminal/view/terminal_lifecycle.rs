//! Terminal lifecycle: construction, remote sessions, apply-deltas.
//!
//! Constructors, remote apply paths, shutdown. Moved verbatim; same behavior.

use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::TerminalView;
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
    #[cfg(any(test, feature = "performance-harness"))]
    pub fn new(cwd: PathBuf, preferred_shell: Option<String>, ctx: &mut ViewContext<Self>) -> Self {
        Self::new_inner(
            cwd,
            super::terminal_session::TerminalBackend::Local { preferred_shell },
            ctx,
        )
    }

    /// Stop this pane's PTY immediately. Normally `TerminalManager::drop` owns
    /// this lifecycle; dock tabs also need an explicit close path because view
    /// subscriptions can keep the view handle alive beyond its last render.
    pub fn shutdown(&self) {
        self.manager.writer().shutdown();
    }

    /// A shared-session MIRROR view: renders the host's terminal identically
    /// (blocks, alt-screen, background) by being fed the host's broadcast output
    /// via [`feed_remote_output`](Self::feed_remote_output), with no local shell.
    /// Its input (typed commands + raw alt-screen keystrokes) forwards to the
    /// host's PTY through `sink`.
    pub fn new_remote(
        cwd: PathBuf,
        sink: crate::terminal::manager::pty_writer::RemoteInputSink,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        Self::new_inner(
            cwd,
            super::terminal_session::TerminalBackend::Remote { sink },
            ctx,
        )
    }

    /// Host-OWNED tab view: renders the daemon's terminal identically (blocks,
    /// alt-screen, background) by being fed its output via
    /// [`feed_remote_output`](Self::feed_remote_output), with no local shell.
    /// Input and resize route to the daemon through `binding`, so the session
    /// — and any servers it runs — outlives the app process.
    pub fn new_host(
        cwd: PathBuf,
        binding: std::sync::Arc<crate::terminal::manager::pty_writer::HostPtyBinding>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        Self::new_inner(
            cwd,
            super::terminal_session::TerminalBackend::Host { binding },
            ctx,
        )
    }

    /// This pane's shell runs on a remote machine (a remote project).
    pub fn set_remote_machine(&mut self, machine_id: Option<String>) {
        self.session_core.lock().remote_machine = machine_id;
    }

    /// Shared-session control gate: a viewer without control is read-only (input
    /// disabled). The host granting/revoking control flips this; the input bar
    /// swaps between the padlock prompt and the live editor. No-op for a local
    /// terminal (never read-only).
    pub fn set_remote_read_only(&mut self, read_only: bool, ctx: &mut ViewContext<Self>) {
        let read_only = read_only && self.is_remote;
        if self.remote_read_only != read_only {
            self.remote_read_only = read_only;
            ctx.notify();
        }
    }

    /// Shared-session mirror: mark whether this viewer is the host's OWN account.
    /// Only such a controller drives the host's terminal size; a teammate adopts
    /// the host's grid (letterboxed). No-op for a local terminal.
    pub fn set_remote_is_host_account(&mut self, is_host: bool, ctx: &mut ViewContext<Self>) {
        let is_host = is_host && self.is_remote;
        if self.remote_is_host_account != is_host {
            self.remote_is_host_account = is_host;
            ctx.notify();
        }
    }

    /// A controlling mirror that may drive the host's terminal size — only true
    /// for a controller (Edit) that is the host's OWN account. Everyone else
    /// (read-only, or a teammate controller) adopts the host's grid instead.
    pub(crate) fn remote_drives_host_size(&self) -> bool {
        self.is_remote && !self.remote_read_only && self.remote_is_host_account
    }

    /// Apply the host's broadcast surface state to this mirror so its input layer
    /// matches the host's — chiefly which AI-tool panel is showing (Claude Code,
    /// Cursor Agent, Ollama, …). A read-only viewer, which can't type, also gets
    /// the host's live AI-input text mirrored in; a controlling viewer keeps its
    /// own editor. No-op for a local terminal.
    /// Shared-session MIRROR: apply one of the host's block deltas so the mirror
    /// shows the host's OWN rendering rather than its re-parse of the raw stream.
    /// See [`TerminalSessionCore::apply_remote_block`] for why that matters. No-op
    /// on a local terminal, which owns its blocks.
    pub fn apply_remote_block_delta(
        &mut self,
        delta: &dragent_session_protocol::TerminalDelta,
        ctx: &mut ViewContext<Self>,
    ) {
        use dragent_session_protocol::TerminalDelta as D;
        if !self.is_remote {
            return;
        }
        // Block ids are the host's; parse rather than invent so every delta for
        // the same block lands on the same one.
        let parse = |s: &str| s.parse::<usize>().ok();
        match delta {
            D::BlockStarted {
                block_id,
                command,
                cwd,
                ts_ms,
                ..
            } => {
                if let Some(id) = parse(block_id) {
                    let (command, cwd) = (command.clone(), cwd.clone());
                    self.model.update(ctx, |m, mctx| {
                        m.apply_remote_block(
                            id,
                            command,
                            cwd.map(std::path::PathBuf::from),
                            *ts_ms,
                            None,
                            None,
                            true,
                            None,
                            None,
                            mctx,
                        );
                    });
                }
            }
            D::BlockLines {
                block_id,
                lines,
                dir_entries,
                ..
            } => {
                if let Some(id) = parse(block_id) {
                    let colored = crate::terminal::model::block_share::colored_lines(lines);
                    // The host resolved this listing from its own filesystem; a
                    // mirror cannot, so carry it onto the block.
                    let theme = self.theme;
                    let listing = dir_entries.as_ref().map(|e| {
                        crate::terminal::model::block_share::dir_entries_from_wire(e, &theme)
                    });
                    let running = self.model.read(ctx, |m, _| m.remote_block_running(id));
                    self.model.update(ctx, |m, mctx| {
                        m.apply_remote_block(
                            id,
                            String::new(),
                            None,
                            0,
                            Some(colored),
                            listing,
                            running,
                            None,
                            None,
                            mctx,
                        );
                    });
                }
            }
            D::BlockCompleted {
                block_id,
                exit_code,
                duration_ms,
                ..
            } => {
                // Re-enable the input. Submitting sets `command_input_passthrough`
                // so keys reach the running command's PTY, and it is normally
                // cleared by the LOCAL `BlockCompleted` event — which a mirror no
                // longer produces, because it stopped building blocks from the
                // byte stream (`suppress_local_blocks`). Without clearing it here
                // the flag stayed set forever and the viewer could never type
                // again after running one command.
                self.command_input_passthrough
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                self.current_running_command = None;
                if let Some(id) = parse(block_id) {
                    let (exit_code, duration_ms) = (*exit_code, *duration_ms);
                    self.model.update(ctx, |m, mctx| {
                        m.apply_remote_block(
                            id,
                            String::new(),
                            None,
                            0,
                            None,
                            None,
                            false,
                            exit_code,
                            duration_ms,
                            mctx,
                        );
                    });
                }
            }
            _ => return,
        }
        self.rebuild_height_index(ctx);
        ctx.notify();
        // Remote block snapshots change the content height without producing
        // the local output events that normally keep a terminal at the tail.
        // Re-apply follow-bottom after rebuilding so a viewer follows new
        // commands just like the host. `auto_scroll` is false when the user
        // (or the host surface) intentionally scrolled up, so this does not
        // take control away from an intentional scroll position.
        self.follow_output_if_auto_scroll(ctx);
    }

    /// Shared-session mirror: display the host's authoritative agent
    /// conversation. The mirror never executes or persists these blocks.
    pub fn apply_remote_agent_blocks(
        &mut self,
        blocks: Vec<dragent_session_protocol::WireAgentBlock>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.is_remote {
            return;
        }
        let blocks = crate::agent::agent_share::agent_blocks_from_wire(blocks);
        ctx.update_model(&self.agent_controller, move |controller, mctx| {
            controller.replace_remote_blocks(blocks, mctx);
        });
        self.last_submit_was_agent = true;
        self.pin_to_bottom(ctx);
        self.arm_agent_ticker(ctx);
        ctx.notify();
    }

    /// Host-side entry point for an agent query relayed from a controlling
    /// shared-session participant. This deliberately uses the exact local
    /// pipeline so billing, permissions, model selection, and tools stay owned
    /// by the signed-in desktop host.
    pub fn submit_shared_agent_query(
        &mut self,
        query: String,
        author: Option<crate::agent::controller::AgentAuthor>,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.is_remote {
            self.submit_agent_query_with_system_context(query, None, author, ctx);
            // A relayed prompt must be visible to every participant as soon as
            // the host accepts it. Do not depend only on the controller model's
            // deferred BlocksChanged subscription: that path is ideal for token
            // streaming, but can leave the originating viewer on the previous
            // snapshot until a later event (or reconnect) forces a replay.
            self.broadcast_shared_agent_blocks(ctx);
        }
    }

    /// Host-side entry point for a relayed Stop action.
    pub fn stop_shared_agent(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_remote {
            return;
        }
        for cmd in crate::agent::local_tools::foreground_agent_commands_for(self.pane_uuid) {
            let _ = crate::agent::local_tools::kill_command(&cmd.id);
        }
        ctx.update_model(&self.agent_controller, |controller, mctx| {
            controller.cancel_stream(mctx);
        });
        ctx.notify();
    }

    pub fn apply_remote_surface(
        &mut self,
        state: &dragent_session_protocol::TerminalSurfaceState,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.is_remote {
            return;
        }

        // Record the host's grid dimensions AND apply them immediately.
        //
        // Applying here rather than deferring to `after_terminal_view_layout`
        // matters because a mirror re-parses the raw byte stream: output that
        // arrives before the emulator has the host's WIDTH wraps at the wrong
        // column, and a terminal cannot re-wrap scrollback once parsed — the
        // damage is permanent. Waiting a frame for layout left exactly that
        // window open, which is what mangled a command that was already running
        // when the viewer joined. The size depends only on the host's grid and
        // our cell metrics, not on this window, so there is nothing to wait for.
        if let (Some(rows), Some(cols)) = (state.rows, state.cols) {
            if rows > 0 && cols > 0 && self.remote_dims != Some((rows, cols)) {
                self.remote_dims = Some((rows, cols));
                if !self.remote_drives_host_size() {
                    let cur = self
                        .model
                        .read(ctx, |m, _| m.with_core(|c| (c.size().rows, c.size().cols)));
                    if cur != (rows, cols) {
                        let cw = self.cell_metrics.width.max(1.0);
                        let ch = self.cell_metrics.height.max(1.0);
                        let size = SizeInfo {
                            rows,
                            cols,
                            cell_width: cw,
                            cell_height: ch,
                            viewport_width_px: cols as f32 * cw,
                            viewport_height_px: rows as f32 * ch,
                        };
                        let update = SizeUpdate::new(size, SizeReason::AltScreenTransition);
                        self.model.update(ctx, |m, mctx| {
                            m.apply_size_update(update, |_, _| {}, mctx);
                        });
                    }
                }
            }
        }

        let tool = state.ai_tool.as_deref().and_then(AiTool::from_label);
        self.remote_ai_tool = tool;

        // Reflect the host's live input text (host → web) so BOTH a watcher and a
        // controller see what the host types. A read-only viewer always reflects;
        // a controller reflects only when it hasn't typed in the last window —
        // otherwise the host's echo of the controller's OWN forwarded keystrokes
        // clobbers the in-flight local edit and jumps the cursor. `set_text` is
        // guarded on a real change (and emits no Changed event, so reflecting can
        // never bounce back out as another forward).
        const ECHO_SUPPRESS: Duration = Duration::from_millis(700);
        let reflect_input =
            self.remote_read_only || self.last_local_input_edit.elapsed() >= ECHO_SUPPRESS;
        let was_rich_active = self.rich_input_active;

        if tool.is_some() {
            // A deliberate close WINS over the host's "tool is active" signal
            // until the viewer reopens the panel.
            if !self.remote_rich_input_closed {
                self.rich_input_active = true;
            }
            if reflect_input {
                let text = state.ai_input.clone();
                ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                    if editor.get_text() != text {
                        editor.set_text(&text, ectx);
                    }
                });
            }
            // Controlling viewer: focus the rich input the moment a tool goes
            // active so Enter forwards to the host's AI tool (the SubmitRichInput
            // path is gated on ai_tool_active && rich_input_active && focused).
            if !self.remote_read_only && !was_rich_active {
                ctx.update_view(&self.rich_input_editor, |_e, ectx| ectx.focus_self());
            }
        } else {
            self.rich_input_active = false;
            // The host's tool exited — forget the close so the next tool session
            // opens its panel normally.
            self.remote_rich_input_closed = false;
            if reflect_input {
                let text = state.command_input.clone();
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    if editor.get_text() != text {
                        editor.set_text(&text, ectx);
                    }
                });
            }
        }

        // Scroll: a read-only viewer follows the host; a controller drives the
        // host's scroll itself (forwarded), so it must not also be driven here.
        if self.remote_read_only {
            self.apply_remote_scroll(state.scroll_fraction);
        }

        ctx.notify();
    }

    /// Host: apply a controlling viewer's live input/scroll surface to THIS
    /// host's own command editor / AI-tool input / scroll — so the host desktop
    /// reflects what the web participant (with edit permission) is doing. The
    /// mirror of [`apply_remote_surface`]. Present fields only; a set_text fires
    /// the editor's Changed event which re-broadcasts to the other viewers, and
    /// the trailing surface broadcast covers scroll (which has no Changed event).
    pub fn apply_control_surface(
        &mut self,
        surface: &dragent_session_protocol::ControlSurface,
        author: Option<&crate::agent::controller::AgentAuthor>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.is_remote {
            return; // only the host owns real editors + scroll to reflect onto
        }
        if let Some(text) = surface.command_input.clone() {
            let cursor = surface.command_cursor;
            let caret = author.map(|author| crate::workspace::RemoteCaretIdentity {
                display_name: author.display_name.clone(),
                avatar_url: author.avatar_url.clone(),
            });
            ctx.update_view(&self.command_editor, |editor, ectx| {
                if editor.get_text() != text {
                    editor.set_text(&text, ectx);
                }
                if let Some(c) = cursor {
                    editor.set_cursor_char_index(c, ectx);
                }
                editor.set_remote_caret(caret, ectx);
            });
        }
        if let Some(text) = surface.ai_input.clone() {
            let cursor = surface.ai_cursor;
            let caret = author.map(|author| crate::workspace::RemoteCaretIdentity {
                display_name: author.display_name.clone(),
                avatar_url: author.avatar_url.clone(),
            });
            ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                if editor.get_text() != text {
                    editor.set_text(&text, ectx);
                }
                if let Some(c) = cursor {
                    editor.set_cursor_char_index(c, ectx);
                }
                editor.set_remote_caret(caret, ectx);
            });
        }
        if let Some(scroll) = &surface.scroll {
            self.apply_remote_scroll(scroll.fraction);
            self.schedule_sync_sticky_header(ctx);
        }
        ctx.notify();
        // Converge the OTHER (read-only) viewers on the controller's surface —
        // scroll has no Changed event, so broadcast explicitly.
        self.broadcast_surface_state(ctx);
    }
}
