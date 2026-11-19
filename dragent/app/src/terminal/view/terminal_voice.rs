//! Terminal voice input methods.
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
    /// The editor voice dictation lands in. In AI-tool mode the main command
    /// bar is not rendered, so the rich input editor is the only visible
    /// target — regardless of whether the panel is expanded.
    pub(crate) fn voice_target_editor<C: ReadModel>(
        &self,
        ctx: &C,
    ) -> warpui::ViewHandle<crate::workspace::InputEditor> {
        if self.rich_input_active || self.active_ai_tool(ctx).is_some() {
            self.rich_input_editor.clone()
        } else {
            self.command_editor.clone()
        }
    }

    /// Voice dictation and file attachments are agent inputs — flip the main
    /// input to Agent mode (and pin it so the per-keystroke autodetector can't
    /// flip it back to Shell mid-dictation). No-op when an AI tool owns input.
    pub(crate) fn force_agent_input_mode(&mut self, ctx: &mut ViewContext<Self>) {
        if self.active_ai_tool(ctx).is_some() {
            return;
        }
        if self.input_config.input_type != crate::agent::DetectedInputType::Agent
            || !self.input_config.is_locked
        {
            self.input_config.input_type = crate::agent::DetectedInputType::Agent;
            self.input_config.is_locked = true;
            self.sync_input_placeholder(ctx);
            ctx.notify();
        }
    }

    pub(crate) fn toggle_voice_input(&mut self, ctx: &mut ViewContext<Self>) {
        use crate::voice::VoiceUiState;
        if self.voice_ui == VoiceUiState::Idle {
            if let Some(text) = self.voice_pending_confirm.take() {
                self.voice_apply_snapshot(&text, true, ctx);
                self.voice_clear_state();
                self.input_notice = None;
                ctx.notify();
                return;
            }
        }
        if let Some(session) = self.voice_session.take() {
            session.stop();
            if self.voice_ui == VoiceUiState::Connecting {
                // Nothing captured yet — turn off immediately. The engine
                // thread self-terminates at its next stop check; bumping the
                // generation (in clear) makes any stragglers from it ghosts.
                self.voice_clear_state();
                ctx.notify();
                return;
            }
            // Recording: keep the "finishing" state until Final flushes the
            // last words — but never trust that unconditionally. If the engine
            // is wedged (dead network mid-stream), force-off after a grace
            // period so the mic can't become unstoppable.
            let gen_at_stop = self.voice_gen;
            ctx.spawn(
                Timer::after(Duration::from_secs(6)),
                move |view, _, mctx| {
                    if view.voice_gen == gen_at_stop && view.voice_ui != VoiceUiState::Idle {
                        log::warn!("[voice] no Final within 6s of stop — force-clearing session");
                        view.voice_clear_state();
                        view.input_notice = Some("Voice input stopped.".to_string());
                        mctx.notify();
                    }
                },
            );
            return;
        }
        // A previous session is still flushing ("finishing"): don't start a
        // second engine in parallel — that's how one stuck session became
        // four concurrent ones (each click minted a new token + thread).
        if self.voice_ui != VoiceUiState::Idle {
            return;
        }
        self.voice_gen = self.voice_gen.wrapping_add(1);
        let gen = self.voice_gen;
        let (tx, rx) = async_channel::unbounded::<crate::voice::VoiceEvent>();
        match crate::voice::start(tx) {
            Ok(session) => {
                self.voice_session = Some(session);
                // Connecting, not recording: permissions, token fetch and the
                // socket handshake are still ahead. VoiceEvent::Live flips it.
                self.voice_ui = crate::voice::VoiceUiState::Connecting;
                self.input_notice = None;
                self.voice_spin = 0;
                self.tick_voice_spinner(gen, ctx);
                self.force_agent_input_mode(ctx);
                // In AI-tool mode dictation lands in the rich input editor;
                // expand the collapsed panel so partials are visible as they
                // arrive — same live feedback as the main input bar.
                if !self.rich_input_active && self.active_ai_tool(ctx).is_some() {
                    self.rich_input_active = true;
                    self.last_dims = None;
                    ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                        ectx.focus_self();
                    });
                }
                let editor = self.voice_target_editor(ctx);
                let existing = editor.read(ctx, |e, _| e.get_text().to_string());
                self.voice_fold = crate::voice::fold::VoiceFold::start_with_confirmation(
                    existing, crate::settings::AppSettings::global().voice.confirm_before_insert,
                );
                self.voice_rebase_pending = false;
                ctx.spawn_stream_local(
                    rx,
                    move |view, ev: crate::voice::VoiceEvent, ctx| {
                        view.on_voice_event(ev, gen, ctx)
                    },
                    |_, _| {},
                );
                // Connect watchdog. Every step before `Live` is individually
                // bounded, but a missed timeout anywhere used to strand the UI
                // in "connecting" indefinitely with no way back except a
                // restart. Nothing may hold the mic in a pre-stream state
                // longer than this.
                ctx.spawn(
                    Timer::after(Duration::from_secs(20)),
                    move |view, _, mctx| {
                        if view.voice_gen == gen && view.voice_ui == VoiceUiState::Connecting {
                            log::warn!("[voice] no Live event within 20s — engine never started");
                            if let Some(s) = view.voice_session.take() {
                                s.stop();
                            }
                            view.voice_clear_state();
                            view.show_error_notice((
                                "Dictation failed".to_string(),
                                "The dictation engine never connected. Check your network, or \
                             open System Settings → Privacy & Security → Microphone and Speech \
                             Recognition, then enable Dragent for both."
                                    .to_string(),
                            ), mctx);
                            mctx.notify();
                        }
                    },
                );
                ctx.notify();
            }
            Err(msg) => {
                log::warn!("[voice] {msg}");
                // Voice failures belong in the same dismissible right-side
                // error popover as commit failures. Keeping permission details
                // out of the inline composer preserves the input's compact UI.
                self.show_error_notice(("Dictation failed".to_string(), msg), ctx);
                ctx.notify();
            }
        }
    }

    /// Self-rescheduling spinner tick. Stops on its own as soon as the session
    /// leaves `Connecting` (or is superseded), so no timer outlives its state.
    pub(crate) fn tick_voice_spinner(&mut self, gen: u64, ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(Duration::from_millis(80)),
            move |view, _, mctx| {
                if view.voice_gen != gen || view.voice_ui != crate::voice::VoiceUiState::Connecting
                {
                    return;
                }
                view.voice_spin = (view.voice_spin + 1) % crate::icons::SPINNER_FRAMES;
                view.tick_voice_spinner(gen, mctx);
                mctx.notify();
            },
        );
    }

    pub(crate) fn voice_apply_snapshot(&mut self, snapshot: &str, is_final: bool, ctx: &mut ViewContext<Self>) {
        let editor = self.voice_target_editor(ctx);
        let current = editor.read(ctx, |e, _| e.get_text().to_string());
        let force = std::mem::take(&mut self.voice_rebase_pending);
        let full = if is_final {
            Some(self.voice_fold.apply(&current, snapshot, force))
        } else {
            self.voice_fold.apply_partial(&current, snapshot, force)
        };
        if let Some(full) = full {
            ctx.update_view(&editor, |e, ectx| e.set_text(&full, ectx));
        } else {
            self.input_notice = Some(format!("Dictating: {snapshot}"));
            ctx.notify();
        }
    }

    pub(crate) fn voice_clear_state(&mut self) {
        self.voice_ui = crate::voice::VoiceUiState::Idle;
        // Invalidate any in-flight events from the session being cleared —
        // they'd otherwise inject text or re-light the mic after the fact.
        self.voice_gen = self.voice_gen.wrapping_add(1);
        self.voice_session = None;
        self.voice_fold = Default::default();
        self.voice_pending_confirm = None;
        self.voice_rebase_pending = false;
    }

    pub(crate) fn on_voice_event(
        &mut self,
        ev: crate::voice::VoiceEvent,
        gen: u64,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::voice::VoiceEvent;
        // Ghost guard: events from a session that was cleared (stopped while
        // connecting, force-cleared, or superseded by a new session).
        if gen != self.voice_gen {
            return;
        }
        match ev {
            VoiceEvent::Live => {
                // Capture is actually running now (mic + engine + socket up).
                if self.voice_session.is_some() {
                    self.voice_ui = crate::voice::VoiceUiState::Recording;
                    ctx.notify();
                }
            }
            VoiceEvent::Partial(text) => {
                self.voice_apply_snapshot(&text, false, ctx);
            }
            VoiceEvent::Final(text) => {
                match self.voice_fold.final_disposition(text) {
                    crate::voice::FinalDisposition::Insert(text) => {
                        self.voice_apply_snapshot(&text, true, ctx);
                        self.voice_clear_state();
                        self.input_notice = None;
                    }
                    crate::voice::FinalDisposition::AwaitingConfirm(text) => {
                        self.voice_session = None;
                        self.voice_gen = self.voice_gen.wrapping_add(1);
                        self.voice_ui = crate::voice::VoiceUiState::Idle;
                        self.input_notice = Some(crate::voice::review_notice(&text));
                        self.voice_pending_confirm = Some(text);
                    }
                }
                ctx.notify();
            }
            VoiceEvent::Error(msg) => {
                log::warn!("[voice] {msg}");
                // A real failure, often with a permissions path to follow —
                // the toast keeps it readable and copyable.
                self.show_error_notice(("Dictation failed".to_string(), msg), ctx);
                self.voice_clear_state();
                ctx.notify();
            }
        }
    }
}
