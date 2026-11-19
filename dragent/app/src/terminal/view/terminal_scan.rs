//! Terminal watchers, tickers, and proactive analysis.
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
    /// (Re)arm the real-time git watcher when the pane's cwd moves to a
    /// different directory. The watcher resolves the git toplevel itself.
    pub(crate) fn arm_git_file_watcher(&mut self, cwd: PathBuf) {
        let already_watching =
            self.git_watch_cwd.as_deref() == Some(cwd.as_path()) && self.git_watch_stop.is_some();
        self.git_watch_cwd = Some(cwd.clone());

        if !self.workspace_active {
            self.stop_git_file_watcher();
            return;
        }
        if already_watching {
            return;
        }
        self.stop_git_file_watcher();
        self.git_watch_stop = Some(crate::git_status::spawn_git_file_watcher(
            cwd,
            self.git_watch_tx.clone(),
        ));
    }

    pub(crate) fn stop_git_file_watcher(&mut self) {
        if let Some(stop) = self.git_watch_stop.take() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Keep repainting once a second while the agent is busy, an agent
    /// command is running, or a plain shell block is running, so elapsed
    /// counters and spinners tick. Self-arming from agent/block events;
    /// disarms itself when everything is idle.
    pub(crate) fn arm_agent_ticker(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workspace_active || self.agent_ticker_armed {
            return;
        }
        self.agent_ticker_armed = true;
        Self::spawn_agent_ticker(ctx);
    }

    pub(crate) fn spawn_agent_ticker(ctx: &mut ViewContext<Self>) {
        ctx.spawn(
            Timer::after(std::time::Duration::from_secs(1)),
            |view: &mut Self, _, ctx| {
                if !view.workspace_active {
                    view.agent_ticker_armed = false;
                    return;
                }
                let busy = view.agent_controller.read(ctx, |c, _| c.is_streaming())
                    || !crate::agent::local_tools::running_agent_commands_for(view.pane_uuid)
                        .is_empty()
                    || view.model.read(ctx, |m, _| {
                        m.with_core(|core| core.blocks().blocks().iter().any(|b| b.is_running()))
                    });
                if busy {
                    ctx.notify();
                    Self::spawn_agent_ticker(ctx);
                } else {
                    view.agent_ticker_armed = false;
                    ctx.notify();
                }
            },
        );
    }

    /// Current blink phase for grid cursors: always visible when blinking is
    /// disabled in settings, otherwise the timer-toggled phase.
    pub(crate) fn alt_cursor_blink_phase(&self) -> bool {
        !EditorSettings::global().terminal.cursor_blink || self.alt_cursor_blink_on
    }

    /// Start the cursor blink timer for the alt-screen grid (no-op when
    /// already running or blinking is disabled).
    pub(crate) fn arm_alt_cursor_blink(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.workspace_active || self.alt_blink_timer_armed {
            return;
        }
        if !EditorSettings::global().terminal.cursor_blink {
            self.alt_cursor_blink_on = true;
            return;
        }
        self.alt_blink_timer_armed = true;
        Self::spawn_alt_cursor_blink(ctx);
    }

    pub(crate) fn spawn_alt_cursor_blink(ctx: &mut ViewContext<Self>) {
        // Shared blink-rate setting (Appearance -> Cursor), re-read every
        // tick so rate changes apply without re-arming the timer.
        let rate_ms = EditorSettings::global()
            .terminal
            .clamped_cursor_blink_rate_ms() as u64;
        ctx.spawn(
            Timer::after(std::time::Duration::from_millis(rate_ms)),
            |view: &mut Self, _, ctx| {
                let alt_active = view.model.read(ctx, |m, _| m.alt_screen_active());
                let blink = EditorSettings::global().terminal.cursor_blink;
                if view.workspace_active && alt_active && blink {
                    view.alt_cursor_blink_on = !view.alt_cursor_blink_on;
                    ctx.notify();
                    Self::spawn_alt_cursor_blink(ctx);
                } else {
                    view.alt_cursor_blink_on = true;
                    view.alt_blink_timer_armed = false;
                    ctx.notify();
                }
            },
        );
    }

    /// Proactive error scanning (Settings → Agent → General): when a user-run
    /// command exits non-zero, analyze it in the background without waiting
    /// for an Analyze click. Skips agent-run commands (their output already
    /// goes back to the LLM), interrupts/pipe closures, and blocks that
    /// already have an analysis in flight or done.
    pub(crate) fn maybe_proactive_scan(
        &mut self,
        context: &crate::terminal::model::block_context::BlockContext,
        was_agent_command: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if was_agent_command || !context.failed {
            return;
        }
        // 130 = SIGINT (user ^C), 141 = SIGPIPE (e.g. `foo | head`),
        // 143 = SIGTERM — expected terminations, not errors worth an LLM call.
        if matches!(context.exit_code, Some(130) | Some(141) | Some(143)) {
            return;
        }
        if !crate::settings::AppSettings::global()
            .agent
            .proactive_error_scanning
        {
            return;
        }
        let idle = self.self_heal_store.read(ctx, |store, _| {
            matches!(
                store.state(context.block_id),
                crate::agent::self_heal::BlockHealState::Idle
            )
        });
        if !idle {
            return;
        }
        log::info!(
            "[self-heal] proactive scan triggered — block {} (`{}` exited {:?})",
            context.block_id,
            context.command,
            context.exit_code,
        );
        self.auto_analyzed_blocks.insert(context.block_id);
        self.revealed_fix_blocks.remove(&context.block_id);
        self.trigger_block_analysis(context.block_id, ctx);
    }

    /// Kick off a self-heal analysis for a failed block. Shared by the manual
    /// "Analyze" action and the proactive error-scanning trigger.
    pub(crate) fn trigger_block_analysis(&mut self, block_id: usize, ctx: &mut ViewContext<Self>) {
        let Some(snapshot) = self.build_block_snapshot(block_id, ctx) else {
            return;
        };
        use crate::agent::project_context::{AnalyzeBlockRequest, ProjectContext};
        let git_status = self.git_repo_status.read(ctx, |m, _| m.status.clone());
        let git_info = git_status
            .as_ref()
            .map(|s| dragent_agent::project::GitInfo {
                branch: s.branch.clone(),
                changed_files: s.changed_files,
                untracked_files: s.untracked,
            });
        let git_cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
        let project = ProjectContext::build(&git_cwd, git_info, &snapshot.output_text);
        use warpui::SingletonEntity;
        let auth = crate::auth::AuthManager::handle(ctx);
        let (refresh_token, access_token) =
            ctx.read_model(&auth, |a: &crate::auth::AuthManager, _| {
                (
                    a.refresh_token().map(|s| s.to_string()),
                    a.cached_access_token().map(|s| s.to_string()),
                )
            });
        let request = AnalyzeBlockRequest {
            block: snapshot,
            project,
            file_context: None,
            refresh_token,
            access_token,
        };
        ctx.update_model(&self.self_heal_store, move |store, mctx| {
            store.analyze_block(block_id, request, mctx);
        });
    }

    pub(crate) fn build_block_snapshot(
        &self,
        block_id: usize,
        ctx: &ViewContext<Self>,
    ) -> Option<crate::agent::api::session::BlockContextSnapshot> {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .iter()
                    .find(|b| b.id == block_id)
                    .and_then(|b| b.completion.as_deref())
                    .map(crate::agent::api::session::snapshot_from_block_context)
            })
        })
    }
}
