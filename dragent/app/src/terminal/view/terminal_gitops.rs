//! Terminal git status accessors.
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
    /// Called on `BlockCompleted` to trigger a bypass-throttle git refresh.
    pub(crate) fn on_block_completed_git(
        &mut self,
        _context: &crate::terminal::model::block_context::BlockContext,
        ctx: &mut ViewContext<Self>,
    ) {
        let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
        ctx.update_model(&self.git_repo_status, |model, mctx| {
            model.refresh_bypass_throttle(cwd, mctx);
        });
    }

    /// Keep agent tool roots aligned with the terminal cwd and project root.
    pub(crate) fn sync_agent_allowed_roots(&mut self, ctx: &mut ViewContext<Self>) {
        let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
        let git_status = self.git_repo_status.read(ctx, |m, _| m.status.clone());
        let git_info = git_status
            .as_ref()
            .map(|s| dragent_agent::project::GitInfo {
                branch: s.branch.clone(),
                changed_files: s.changed_files,
                untracked_files: s.untracked,
            });
        let project_root =
            crate::agent::project_context::ProjectContext::build(&cwd, git_info, "").project_root;

        let sync_key = (cwd.clone(), project_root.clone());
        if self.agent_roots_sync_key.as_ref() == Some(&sync_key) {
            return;
        }
        // Set this before launching background work. Any status event arriving
        // while Git discovery is in flight must observe the synchronization as
        // already scheduled rather than launch a duplicate OS thread.
        self.agent_roots_sync_key = Some(sync_key);

        // Update model roots synchronously — these are cheap in-memory writes.
        let roots =
            crate::agent::permissions::AllowedRoots::for_session(&cwd, project_root.as_deref());
        ctx.update_model(&self.agent_controller, |controller, _| {
            controller.set_allowed_roots(roots.clone());
        });
        ctx.update_model(&self.self_heal_store, |store, _| {
            store.set_allowed_roots(roots);
        });

        // Pre-build the static context (file tree + config files) for the
        // current CWD so the first query doesn't pay for it.
        crate::agent::context_builder::warm_static_context(&cwd);
        crate::agent::skill_discovery::warm_skills_prompt(&cwd);
    }

    /// Expose current git status for Layer 6b (pane header chip) without exposing model internals.
    pub fn git_status_for_pane(
        &self,
        ctx: &ViewContext<Self>,
    ) -> Option<crate::git_status::GitStatusMetadata> {
        self.git_repo_status.read(ctx, |m, _| m.status.clone())
    }

    /// Accessor for the git repo status model handle (used by workspace subscriptions).
    /// Bottom-bar git chips need the panel/popover toggle states so the
    /// chips can render their active styling.
    pub fn git_ui_state(&self) -> (bool, bool) {
        (self.branch_menu_open, self.branch_diff_expanded)
    }

    /// Source-control panel commit flow: the panel owns its own message
    /// editor, so the message is handed to the session's commit editor before
    /// `CommitBranchChanges` runs (which reads text + staged set from here).
    pub fn set_commit_message(&mut self, message: &str, ctx: &mut ViewContext<Self>) {
        ctx.update_view(&self.branch_diff_commit_editor, |editor, editor_ctx| {
            editor.set_text(message, editor_ctx);
        });
    }

    pub fn git_repo_status(&self) -> ModelHandle<crate::git_status::GitRepoStatusModel> {
        self.git_repo_status.clone()
    }

    /// Queued view events can outlive the focus that produced them. Selection
    /// must follow the framework's current keyboard owner, not a cached hint.
    pub fn pane_has_focus(&self, app: &AppContext) -> bool {
        app.check_view_focused(self.window_id, &self.pane_view_id)
            || app.check_view_or_child_focused(self.window_id, &self.pane_view_id)
    }
}
