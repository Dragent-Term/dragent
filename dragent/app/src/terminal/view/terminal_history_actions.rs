//! Terminal history and workflow search actions.
//!
//! handle_terminal_history_action: moved verbatim; same behavior.

use super::terminal_helpers::{
    checkpoint_preview_body, current_branch, fetch_conversation_for_resume,
    fetch_conversation_summaries, generate_commit_message_llm, list_local_branches,
    list_subdirectories, run_git, run_git_switch, shell_quote, shorten_dir,
    slash_command_candidates, validate_branch_name,
};
use super::terminal_view::{BranchDiffOp, TerminalView, CMD_HIST_MIN_HEIGHT_PX};
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
    pub(crate) fn handle_terminal_history_action(
        &mut self,
        action: &TerminalAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            TerminalAction::CloseCmdHistory => {
                self.close_cmd_history(true, ctx);
            }
            TerminalAction::CmdHistResizeBy { dy } => {
                // Dragging up = negative dy = taller popover (its bottom is pinned
                // to the input; the top edge moves).
                let new_h = (self.cmd_hist_height_px - *dy).clamp(CMD_HIST_MIN_HEIGHT_PX, 640.0);
                if (new_h - self.cmd_hist_height_px).abs() > f32::EPSILON {
                    self.cmd_hist_height_px = new_h;
                    ctx.notify();
                }
            }
            TerminalAction::CmdHistoryClick { index } => {
                if *index < self.cmd_hist_items.len() {
                    self.cmd_hist_selected = *index;
                    self.preview_cmd_history(ctx);
                    // Click = pick and run — into whichever input the popover targets.
                    let rich = self.cmd_hist_rich;
                    self.close_cmd_history(false, ctx);
                    if rich {
                        self.handle_action(&TerminalAction::SubmitRichInput, ctx);
                    } else {
                        self.submit_command_editor(ctx);
                    }
                }
            }
            TerminalAction::OpenSideCmdHistory => {
                self.open_side_cmd_history(ctx);
            }
            TerminalAction::HistorySearchOpen => {
                self.open_history_search(ctx);
            }
            TerminalAction::HistorySearchClose => {
                self.close_history_search(ctx, true);
            }
            TerminalAction::HistorySearchUp => {
                if self.history_search.open {
                    self.history_search.move_up();
                    ctx.notify();
                }
            }
            TerminalAction::HistorySearchDown => {
                if self.history_search.open {
                    self.history_search.move_down();
                    ctx.notify();
                }
            }
            TerminalAction::HistorySearchAccept => {
                self.accept_history_search(ctx);
            }
            TerminalAction::HistorySearchClick { index } => {
                if self.history_search.open {
                    self.history_search.selected = *index;
                    self.accept_history_search(ctx);
                }
            }
            TerminalAction::HistorySearchBackspace => {
                self.history_search_backspace(ctx);
            }
            TerminalAction::HistorySearchQueryAppend(ch) => {
                self.history_search_append(ch, ctx);
            }
            TerminalAction::WorkflowSearchOpen => {
                self.open_workflow_search(ctx);
            }
            TerminalAction::WorkflowSearchClose => {
                self.close_workflow_search(ctx, true);
            }
            TerminalAction::WorkflowSearchUp => {
                if self.workflow_search.open {
                    self.workflow_search.move_up();
                    ctx.notify();
                }
            }
            TerminalAction::WorkflowSearchDown => {
                if self.workflow_search.open {
                    self.workflow_search.move_down();
                    ctx.notify();
                }
            }
            TerminalAction::WorkflowSearchAccept => {
                self.accept_workflow_search(ctx);
            }
            TerminalAction::WorkflowSearchClick { index } => {
                if self.workflow_search.open {
                    self.workflow_search.selected = *index;
                    self.accept_workflow_search(ctx);
                }
            }
            TerminalAction::WorkflowSearchBackspace => {
                self.workflow_search_backspace(ctx);
            }
            TerminalAction::WorkflowSearchQueryAppend(ch) => {
                self.workflow_search_append(ch, ctx);
            }
            TerminalAction::WorkflowArgPrev => {
                self.cycle_workflow_arg(true, ctx);
            }
            TerminalAction::WorkflowArgNext => {
                self.cycle_workflow_arg(false, ctx);
            }
            TerminalAction::ToggleHistoryPanel => {
                self.history_panel_open = !self.history_panel_open;
                if self.history_panel_open {
                    let pane_uuid = self.pane_uuid;
                    let cwd = self.cwd(ctx);
                    ctx.spawn(
                        async move { fetch_conversation_summaries(pane_uuid, &cwd) },
                        |view, summaries, ctx| {
                            view.conversation_summaries = summaries;
                            ctx.notify();
                        },
                    );
                }
                ctx.notify();
            }
            _ => {}
        }
    }
}
