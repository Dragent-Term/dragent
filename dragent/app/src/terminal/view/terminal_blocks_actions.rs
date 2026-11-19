//! Terminal blocks and filtering actions.
//!
//! handle_terminal_blocks_action: moved verbatim; same behavior.

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
use crate::terminal::view::actions::{block_copy_key, EntryHint, LinkHint, TerminalAction};
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
    pub(crate) fn handle_terminal_blocks_action(
        &mut self,
        action: &TerminalAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            TerminalAction::SelectDirEntry {
                block_id,
                name,
                is_dir,
            } => {
                self.clear_block_selection();
                self.link_hint = None;
                self.entry_hint = Some(EntryHint {
                    block_id: *block_id,
                    name: name.clone(),
                    is_dir: *is_dir,
                });
                ctx.notify();
            }
            TerminalAction::OpenDirEntry {
                block_id,
                name,
                is_dir: _,
            } => {
                self.open_dir_entry(*block_id, name, ctx);
            }
            TerminalAction::SelectLink {
                block_id,
                url,
                occurrence,
                absolute_position,
            } => {
                self.entry_hint = None;
                let hint = LinkHint {
                    block_id: *block_id,
                    url: url.clone(),
                    occurrence: *occurrence,
                };
                self.link_hint = Some(hint.clone());
                self.open_link_menu = Some(hint);
                // Anchor relative to the fixed (unscrolled) viewport frame —
                // the same reference point `OpenBlockMenuAtClick` uses for the
                // block options menu, not the block list's own paint-space
                // origin, which moves as virtualized rows scroll through it.
                let local = self
                    .viewport_origin(ctx)
                    .map(|origin| *absolute_position - origin)
                    .unwrap_or(*absolute_position);
                self.link_menu_anchor = Some(local);
                ctx.notify();
            }
            TerminalAction::OpenLink { block_id, url } => {
                self.entry_hint = None;
                self.link_hint = None;
                self.open_link_menu = None;
                self.link_menu_anchor = None;
                if url.starts_with("http://") || url.starts_with("https://") {
                    // The workspace opens it like every link (Dragent's
                    // browser unless the user turned that off).
                    ctx.dispatch_typed_action_deferred(
                        crate::workspace::WorkspaceAction::OpenExternalUrl { url: url.clone() },
                    );
                } else {
                    self.open_terminal_path_link(*block_id, url, ctx);
                }
                ctx.notify();
            }
            TerminalAction::OpenLinkExternal { url, .. } => {
                self.entry_hint = None;
                self.link_hint = None;
                self.open_link_menu = None;
                self.link_menu_anchor = None;
                // Always the system browser, ignoring the in-app-browser
                // preference that `OpenLink` respects.
                ctx.open_url(url);
                ctx.notify();
            }
            TerminalAction::RevealLinkInFinder { block_id, url } => {
                self.entry_hint = None;
                self.link_hint = None;
                self.open_link_menu = None;
                self.link_menu_anchor = None;
                let (raw_path, _, _) =
                    crate::terminal::render::block_output::split_path_suffix(url);
                if !raw_path.is_empty() {
                    let cwd = self.block_cwd(*block_id, ctx);
                    let path = crate::terminal::render::block_output::resolve_terminal_path(
                        raw_path, &cwd,
                    );
                    ctx.open_file_path_in_explorer(&path);
                }
                ctx.notify();
            }
            TerminalAction::CopyLinkTarget { block_id, url } => {
                let text = if url.starts_with("http://") || url.starts_with("https://") {
                    url.clone()
                } else {
                    let (raw_path, _, _) =
                        crate::terminal::render::block_output::split_path_suffix(url);
                    let cwd = self.block_cwd(*block_id, ctx);
                    crate::terminal::render::block_output::resolve_terminal_path(raw_path, &cwd)
                        .to_string_lossy()
                        .into_owned()
                };
                self.write_output_clipboard(text, ctx);
                self.open_link_menu = None;
                self.link_menu_anchor = None;
                self.link_hint = None;
                ctx.notify();
            }
            TerminalAction::CloseLinkMenu => {
                if self.open_link_menu.is_some() {
                    self.open_link_menu = None;
                    self.link_menu_anchor = None;
                    self.link_hint = None;
                    ctx.notify();
                }
            }
            TerminalAction::HoverLink { hint } => {
                if self.hovered_link.as_ref() != Some(hint) {
                    self.hovered_link = Some(hint.clone());
                    ctx.notify();
                }
            }
            TerminalAction::ClearHoverLink => {
                if self.hovered_link.is_some() {
                    self.hovered_link = None;
                    ctx.notify();
                }
            }
            TerminalAction::HoverAltLink { segments } => {
                let new_hover = Some(segments.clone());
                if self.alt_hovered_link != new_hover {
                    self.alt_hovered_link = new_hover;
                    ctx.notify();
                }
            }
            TerminalAction::SetDirHover(hover) => {
                if self.dir_hover != *hover {
                    self.dir_hover = *hover;
                    ctx.notify();
                }
            }
            TerminalAction::ClearHoverAltLink => {
                if self.alt_hovered_link.is_some() {
                    self.alt_hovered_link = None;
                    ctx.notify();
                }
            }
            TerminalAction::ToggleBlockExpansion { block_id } => {
                let is_agent = ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.toggle_activity_group_expanded(*block_id, mctx)
                });
                if !is_agent {
                    self.model.update(ctx, |m, mctx| {
                        m.toggle_block_expansion(*block_id);
                        mctx.notify();
                    });
                }
                ctx.notify();
            }
            TerminalAction::ToggleAgentThinking { block_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.toggle_thinking_expanded(*block_id, mctx);
                });
                ctx.notify();
            }
            TerminalAction::ToggleToolStep { block_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.toggle_tool_step_expanded(*block_id, mctx);
                });
                ctx.notify();
            }
            TerminalAction::SetChatView { chat } => {
                // Chosen from the input's pill menu: close it.
                self.mode_menu_open = false;
                self.set_cli_chat(*chat, ctx);
            }
            TerminalAction::StopChatAgent => {
                self.stop_cli_agent(ctx);
            }
            TerminalAction::JumpToLatestChat => {
                self.agent_transcript_follow.set(true);
                self.agent_transcript_scroll.scroll_to(warpui_core::units::Pixels::new(f32::MAX));
                ctx.notify();
            }
            TerminalAction::ShowEarlierChat => {
                self.chat_window += super::terminal_cli_chat::CHAT_WINDOW;
                // Stay on what was read, not jump to the bottom.
                self.agent_transcript_follow.set(false);
                ctx.notify();
            }
            TerminalAction::AnswerAcpPermission { option } => {
                self.acp_answer_permission(option.clone(), ctx);
            }
            TerminalAction::ToggleChatLine { key } => {
                if !self.chat_open.remove(key) {
                    self.chat_open.insert(key.clone());
                }
                ctx.notify();
            }
            TerminalAction::ToggleDiffExpand { tool_id } => {
                // Expand state lives in the edit_diff registry (a static store),
                // not the model — flip it and repaint so the card re-reads it.
                crate::agent::edit_diff::toggle_expanded(tool_id);
                ctx.update_model(&self.agent_controller, |_c, mctx| {
                    mctx.notify();
                });
                ctx.notify();
            }
            TerminalAction::CopyText { text } => {
                self.write_output_clipboard(text.clone(), ctx);
            }
            TerminalAction::CopyWithConfirmation { key, text } => {
                self.copy_with_confirmation(key.clone(), text.clone(), ctx);
                self.close_block_menu();
                ctx.notify();
            }
            TerminalAction::ToggleBlockMenu { block_id } => {
                if self.open_block_menu == Some(*block_id) {
                    self.close_block_menu();
                } else {
                    self.open_block_menu = Some(*block_id);
                    self.block_menu_anchor = None;
                }
                ctx.notify();
            }
            TerminalAction::CloseBlockMenu => {
                if self.open_block_menu.is_some() {
                    self.close_block_menu();
                    ctx.notify();
                }
            }
            TerminalAction::TerminateBlock { block_id } => {
                let running = self.model.read(ctx, |m, _| {
                    m.with_core(|core| {
                        core.blocks()
                            .blocks()
                            .iter()
                            .find(|block| block.id == *block_id)
                            .is_some_and(|block| block.is_running())
                    })
                });
                if running {
                    self.manager.writer().write(&[0x03]);
                    self.manager.writer().interrupt();
                }
                self.close_block_menu();
                ctx.notify();
            }
            TerminalAction::OpenBlockMenuAtClick { local_x, local_y } => {
                let total = self.block_height_index.total_height_px();
                let vp = self.output_viewport_height_px.max(1.0);
                let scroll_top = self
                    .block_scroll_pos
                    .lock()
                    .unwrap()
                    .scroll_top_px(total, vp);
                let content_y = *local_y + scroll_top;
                if let Some(block_id) = self.block_height_index.block_id_at_content_y(content_y) {
                    self.open_block_menu = Some(block_id);
                    self.block_menu_anchor = Some(Vector2F::new(*local_x, *local_y));
                    ctx.notify();
                }
            }
            TerminalAction::CopyBlockOutput { block_id } => {
                if let Some(text) = self.block_output_text(*block_id, ctx) {
                    self.copy_with_confirmation(block_copy_key(*block_id), text, ctx);
                }
                self.close_block_menu();
                ctx.notify();
            }
            TerminalAction::CopyBlockCommandAndOutput { block_id } => {
                if let Some(text) = self.block_command_and_output_text(*block_id, ctx) {
                    self.copy_with_confirmation(block_copy_key(*block_id), text, ctx);
                }
                self.close_block_menu();
                ctx.notify();
            }
            TerminalAction::ScrollBlockToTop { block_id } => {
                self.scroll_block_to_top(*block_id);
                self.close_block_menu();
                self.schedule_sync_sticky_header(ctx);
                ctx.notify();
            }
            TerminalAction::ScrollBlockToBottom { block_id } => {
                self.scroll_block_to_bottom(*block_id);
                self.close_block_menu();
                self.schedule_sync_sticky_header(ctx);
                ctx.notify();
            }
            TerminalAction::ToggleBlockFilter { block_id } => {
                use crate::console::block_filter::BlockFilterState;
                // Toggle: close if this block's filter is already open, else open it
                // (replacing any other block's filter — one bar at a time).
                self.freeze_block_scroll_position();
                if matches!(self.block_filter, Some((id, _)) if id == *block_id) {
                    self.block_filter = None;
                    self.block_filter_match_index = 0;
                    ctx.update_view(&self.block_filter_editor, |e, ectx| e.set_text("", ectx));
                } else {
                    self.block_filter = Some((*block_id, BlockFilterState::new()));
                    self.block_filter_match_index = 0;
                    ctx.update_view(&self.block_filter_editor, |e, ectx| e.set_text("", ectx));
                    // Focus after the ChildView is mounted in the next render, else
                    // focus_self() targets a view that isn't in the tree yet.
                    let filter_block_id = *block_id;
                    ctx.spawn(
                        Timer::after(Duration::from_millis(0)),
                        move |view, _, ctx| {
                            // Do not steal focus if this filter was closed or replaced
                            // before its deferred mount completed.
                            if view
                                .block_filter
                                .as_ref()
                                .is_some_and(|(active_id, _)| *active_id == filter_block_id)
                            {
                                ctx.update_view(&view.block_filter_editor, |_e, ectx| {
                                    ectx.focus_self()
                                });
                                ctx.notify();
                            }
                        },
                    );
                }
                self.close_block_menu();
                self.rebuild_height_index(ctx);
                ctx.notify();
            }
            TerminalAction::CloseBlockFilter { block_id } => {
                if matches!(self.block_filter, Some((id, _)) if id == *block_id) {
                    self.block_filter = None;
                    self.block_filter_match_index = 0;
                    ctx.update_view(&self.block_filter_editor, |e, ectx| e.set_text("", ectx));
                    self.focus_command_editor(ctx);
                    self.rebuild_height_index(ctx);
                    ctx.notify();
                }
            }
            TerminalAction::ToggleBlockFilterRegex { block_id } => {
                if let Some((id, f)) = self.block_filter.as_mut() {
                    if *id == *block_id {
                        f.toggle_regex();
                        self.block_filter_match_index = 0;
                    }
                }
                self.rebuild_height_index(ctx);
                ctx.notify();
            }
            TerminalAction::BlockFilterMatch { block_id, delta } => {
                self.move_block_filter_match(*block_id, *delta, ctx);
            }
            TerminalAction::SetDirSort(sort) => {
                self.dir_sort = *sort;
                ctx.notify();
            }
            TerminalAction::SetDirView(view) => {
                self.dir_view = *view;
                ctx.notify();
            }
            TerminalAction::DetachBlock { block_id } => {
                self.attached_blocks.retain(|b| b.block_id != *block_id);
                ctx.notify();
            }
            _ => {}
        }
    }
}
