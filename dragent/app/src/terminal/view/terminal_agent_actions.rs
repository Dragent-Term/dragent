//! Terminal agent flows actions.
//!
//! handle_terminal_agent_action: moved verbatim; same behavior.

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
    pub(crate) fn handle_terminal_agent_action(
        &mut self,
        action: &TerminalAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            TerminalAction::ToggleModeMenu => {
                self.mode_menu_open = !self.mode_menu_open;
                self.model_menu_open = false; // one popover at a time
                ctx.notify();
            }
            TerminalAction::ToggleModelMenu => {
                self.model_menu_open = !self.model_menu_open;
                self.mode_menu_open = false;
                // The model list comes from /api/v1/account, which nothing in a
                // terminal-only session fetches. Kick a fetch on first open so
                // the picker fills in (async; the panel shows the current model
                // meanwhile).
                self.branch_menu_open = false;
                self.folder_menu_open = false;
                // A stale query from last time would hide most of the list on
                // reopen, looking exactly like the bug this replaced.
                self.model_search.clear();
                ctx.update_view(&self.model_search_editor, |e, ectx| e.set_text("", ectx));
                if self.model_menu_open {
                    ctx.update_view(&self.model_search_editor, |_e, ectx| ectx.focus_self());
                    use warpui::SingletonEntity;
                    let account = crate::settings::AccountStore::handle(ctx);
                    let needs_fetch =
                        ctx.read_model(&account, |s, _| s.data.is_none() && !s.loading);
                    if needs_fetch {
                        ctx.update_model(&account, |s, mctx| s.fetch(mctx));
                    }
                }
                ctx.notify();
            }
            TerminalAction::SetAgentModel { model_id, .. } if self.cli_model_picker(ctx).is_some() => {
                self.model_menu_open = false;
                self.switch_cli_model(model_id.clone(), ctx);
            }
            TerminalAction::SetAgentModel { model_id, .. } if self.acp_model_picker(ctx).is_some() => {
                self.model_menu_open = false;
                self.acp_choose_model(model_id.clone(), ctx);
            }
            TerminalAction::SetChatEffort { level } => {
                // The menu stays open: effort is a slider under the models.
                self.set_chat_effort(level.clone(), ctx);
            }
            TerminalAction::EffortPointer { x, phase } => {
                use crate::terminal::view::actions::EffortPointerPhase;
                let dragging = self.effort_drag.is_some();
                match phase {
                    EffortPointerPhase::Press => self.effort_drag = self.effort_level_at(*x, ctx),
                    EffortPointerPhase::Drag if dragging => {
                        if let Some(level) = self.effort_level_at(*x, ctx) {
                            self.effort_drag = Some(level);
                        }
                    }
                    EffortPointerPhase::Release => {
                        if let Some(level) = self.effort_drag.take() {
                            self.set_chat_effort(level, ctx);
                        }
                    }
                    EffortPointerPhase::Drag => {}
                }
                if self.effort_drag.is_some() {
                    // Let go outside the slider (no release reaches it): set
                    // the level once the pointer has rested a moment.
                    self.effort_drag_moves += 1;
                    let moves = self.effort_drag_moves;
                    ctx.spawn(warpui::r#async::Timer::after(std::time::Duration::from_millis(900)), move |view, _, ctx| {
                        if view.effort_drag_moves == moves {
                            if let Some(level) = view.effort_drag.take() {
                                view.set_chat_effort(level, ctx);
                            }
                        }
                    });
                }
                ctx.notify();
            }
            TerminalAction::SetAgentModel { model_id, effort } => {
                // Persist through the same path Settings uses; save() refreshes
                // the global snapshot that submit_agent_query reads.
                let mut settings = crate::settings::AppSettings::load();
                settings.selected_model = model_id.clone();
                if self.agent_model_override.is_some() { self.agent_model_override = Some(model_id.clone()); }
                // Written together: the picker chose both in one click, and a
                // model with no effort support must not inherit a stale level.
                settings.selected_effort = *effort;
                settings.save();
                self.model_menu_open = false;
                // The new model may not accept an effort level; close the picker
                // so it can't linger over a control that just disappeared.
                ctx.notify();
            }
            TerminalAction::SetChatMode { mode } => {
                self.chat_mode = *mode;
                self.mode_menu_open = false;
                self.model_menu_open = false;
                // Selecting an agent mode implies agent input — lock to Agent so
                // the next prompt runs the mode rather than being shell-detected.
                self.input_config.input_type = DetectedInputType::Agent;
                self.input_config.is_locked = true;
                self.sync_input_placeholder(ctx);
                ctx.notify();
            }
            TerminalAction::SlashPaletteMove { forward } => {
                let project_root = self.cwd(ctx);
                let count = self.command_editor.read(ctx, |e, _| {
                    slash_command_candidates(e.get_text(), &project_root)
                        .map(|c| c.len())
                        .unwrap_or(0)
                });
                if count > 0 {
                    self.slash_palette_selected = if *forward {
                        (self.slash_palette_selected + 1) % count
                    } else if self.slash_palette_selected == 0 {
                        count - 1
                    } else {
                        self.slash_palette_selected - 1
                    };
                    ctx.notify();
                }
            }
            TerminalAction::RunSlashCommand { name } => {
                let project_root = self.cwd(ctx);
                if let Some(cmd) = slash_command_candidates(name, &project_root)
                    .and_then(|commands| commands.into_iter().find(|command| command.name == *name))
                {
                    self.run_slash_command(cmd.kind, ctx);
                }
            }
            TerminalAction::ResumeConversation { conversation_id } => {
                self.history_panel_open = false;
                self.open_checkpoint_preview = None;
                let pane_uuid = self.pane_uuid;
                let conversation_id = *conversation_id;
                let cwd = self.cwd(ctx);
                ctx.spawn(
                    async move { fetch_conversation_for_resume(pane_uuid, conversation_id, &cwd) },
                    move |view, (persisted, true_max_sort_order), ctx| {
                        ctx.update_model(&view.agent_controller, |c, mctx| {
                            c.switch_to_conversation(
                                persisted,
                                conversation_id,
                                true_max_sort_order,
                                mctx,
                            );
                        });
                        ctx.notify();
                    },
                );
                ctx.notify();
            }
            TerminalAction::ApproveAgentAction { action_id } => {
                let is_streaming = ctx.read_model(&self.agent_controller, |c, _| c.is_streaming());
                if is_streaming {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.resolve_streaming_approval(*action_id, true, None, mctx);
                    });
                } else {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.approve_action_background(*action_id, mctx);
                    });
                }
                ctx.update_view(&self.feedback_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::AllowToolAlways { action_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.allow_tool_always(*action_id, mctx);
                });
                ctx.update_view(&self.feedback_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::RejectAgentAction {
                action_id,
                feedback,
            } => {
                let is_streaming = ctx.read_model(&self.agent_controller, |c, _| c.is_streaming());
                if is_streaming {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.resolve_streaming_approval(*action_id, false, feedback.clone(), mctx);
                    });
                } else {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.reject_action(*action_id, feedback.clone(), mctx);
                    });
                }
                ctx.update_view(&self.feedback_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::PickAgentQuestionOption { action_id, option } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.pick_user_question_option(*action_id, option.clone(), mctx);
                });
                // Reset the "Other" field so a new question starts blank.
                ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::ConfirmAgentQuestion { action_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.confirm_user_question(*action_id, mctx);
                });
                ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
            }
            TerminalAction::MoveAgentQuestionCursor { action_id, delta } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.move_question_cursor(*action_id, *delta, mctx);
                });
            }
            TerminalAction::PickAgentQuestionCursor { action_id } => {
                let picked = ctx.read_model(&self.agent_controller, |c, _| {
                    c.question_cursor_pick(*action_id)
                });
                if let Some(option) = picked {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.pick_user_question_option(*action_id, option, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::PickAgentQuestionDigit { action_id, index } => {
                let picked = ctx.read_model(&self.agent_controller, |c, _| {
                    c.question_digit_pick(*action_id, *index)
                });
                if let Some(option) = picked {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.pick_user_question_option(*action_id, option, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::SubmitAgentQuestionText { action_id } => {
                let text = self
                    .question_editor
                    .read(ctx, |e, _| e.get_text().to_string());
                if !text.trim().is_empty() {
                    ctx.update_model(&self.agent_controller, |c, mctx| {
                        c.answer_user_question_text(*action_id, text, mctx);
                    });
                    ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                }
            }
            TerminalAction::CancelQueuedMessage { queue_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.cancel_queued_message(*queue_id, mctx);
                });
            }
            TerminalAction::ApprovePlan { plan_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.approve_plan(*plan_id, mctx);
                });
            }
            TerminalAction::RefinePlan { plan_id } => {
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.mark_plan_refining(*plan_id, mctx);
                });
                // Return focus to the input bar so the user can type their refinement.
                self.ensure_command_input_focused(ctx);
            }
            TerminalAction::ClearAgentConversation => {
                self.open_checkpoint_preview = None;
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.clear_conversation(mctx);
                });
            }
            TerminalAction::StopAgentStream => {
                if self.is_remote_controller() {
                    use warpui::SingletonEntity;
                    let manager = crate::realtime::manager::SessionManager::handle(ctx);
                    ctx.update_model(&manager, |session, _| session.stop_agent());
                    return;
                }
                // Kill only the commands THIS turn is blocked on (foreground).
                // Backgrounded processes (e.g. a running dev server) survive —
                // Stop interrupts the agent, it does not tear down your servers.
                for cmd in crate::agent::local_tools::foreground_agent_commands_for(self.pane_uuid)
                {
                    if crate::agent::local_tools::kill_command(&cmd.id) {
                        log::info!(
                            "[agent] stop button killed foreground command task {}",
                            cmd.id
                        );
                    }
                }
                // Abort the current turn; per the user's choice, the next queued
                // prompt (if any) starts automatically inside cancel_stream.
                ctx.update_model(&self.agent_controller, |c, mctx| {
                    c.cancel_stream(mctx);
                });
                ctx.notify();
            }
            TerminalAction::ContinueAgent { error_block_id } => {
                let prepared = ctx.update_model(&self.agent_controller, |controller, _| {
                    controller.prepare_failed_turn_resume(Some(*error_block_id))
                });
                if prepared {
                    self.submit_agent_query(
                        "Resume the interrupted task from its saved execution checkpoint."
                            .to_string(),
                        ctx,
                    );
                } else {
                    self.show_error_notice(("Agent request failed".to_string(), "There is no interrupted agent task to continue.".to_string()), ctx);
                    ctx.notify();
                }
            }
            TerminalAction::ReviewAgentChanges => {
                self.submit_agent_query(
                    "Review the current agent changes for correctness, regressions, and missing verification."
                        .to_string(),
                    ctx,
                );
            }
            TerminalAction::AnalyzeBlock { block_id } => {
                // Explicit user request — always shows the full card flow.
                self.auto_analyzed_blocks.remove(block_id);
                self.trigger_block_analysis(*block_id, ctx);
            }
            TerminalAction::RevealBlockFix { block_id } => {
                self.revealed_fix_blocks.insert(*block_id);
                ctx.notify();
            }
            TerminalAction::AutoFixBlock { block_id } => {
                let bid = *block_id;
                let Some(analysis) =
                    self.self_heal_store
                        .read(ctx, |store, _| match store.state(bid) {
                            crate::agent::self_heal::BlockHealState::Ready(analysis) => {
                                Some(analysis.clone())
                            }
                            _ => None,
                        })
                else {
                    log::warn!(
                        "[self-heal] fix requested for block {bid} without completed analysis"
                    );
                    return;
                };
                let Some(snapshot) = self.build_block_snapshot(bid, ctx) else {
                    return;
                };

                let analysis_context = format!(
                    "Failed command: `{}`\nWorking directory: `{}`\nExit code: {:?}\nCommand output:\n```text\n{}\n```\n\nDiagnosis:\n{}\n\nSuggested command from the analyzer: {}",
                    snapshot.command,
                    snapshot.cwd.display(),
                    snapshot.exit_code,
                    snapshot.output_text,
                    analysis.explanation,
                    analysis.suggested_command.as_deref().unwrap_or("none; investigate and implement the required change"),
                );

                self.set_tab_session_kind(TabSessionKind::Agent);
                self.input_config.input_type = DetectedInputType::Agent;
                self.input_config.is_locked = true;
                self.submit_agent_query_with_system_context(
                    format!("Fix the failed command from block {bid}. Analyze the workspace, make the necessary changes, run verification, and report the result."),
                    Some(analysis_context),
                    None,
                    ctx,
                );
            }
            TerminalAction::CancelAgentCommand { task_id } => {
                if crate::agent::local_tools::kill_command(task_id) {
                    log::info!("[agent] user cancelled command task {task_id}");
                }
                ctx.notify();
            }
            TerminalAction::OpenInAgent { block_id } => {
                let snapshot = self.build_block_snapshot(*block_id, ctx);
                if let Some(snapshot) = snapshot {
                    let mut attached = self.attached_blocks.clone();
                    if !attached.iter().any(|b| b.block_id == *block_id) {
                        attached.push(crate::agent::api::session::AttachedBlock {
                            block_id: *block_id,
                            snapshot,
                        });
                    }
                    self.attached_blocks = attached;
                    self.set_tab_session_kind(TabSessionKind::Agent);
                    self.input_config.input_type = DetectedInputType::Agent;
                    ctx.notify();
                }
            }
            TerminalAction::RunBlockFix { block_id } => {
                // The diagnosis is "rerun this corrected command". Submit it
                // through the same path as pressing Enter in the shell input.
                let cmd = self.self_heal_store.read(ctx, |store, _| {
                    store.suggested_command(*block_id).map(|s| s.to_string())
                });
                if let Some(command) = cmd {
                    self.submit_shell_command_inner(command, ctx);
                }
            }
            TerminalAction::DismissBlockFix { block_id } => {
                let bid = *block_id;
                ctx.update_model(&self.self_heal_store, move |store, mctx| {
                    store.dismiss(bid, mctx);
                });
            }
            TerminalAction::AcceptAllAgentChanges => {
                ctx.update_model(&self.agent_controller, |c, _| {
                    c.agent_modified_files.clear();
                });
                self.git_diff_dismissed = false;
                self.last_agent_file_count = 0;
                ctx.notify();
            }
            TerminalAction::RestoreToMessageConfirmed { block_id } => {
                self.open_checkpoint_preview = None;
                let bid = *block_id;
                let summary =
                    ctx.update_model(&self.agent_controller, move |c, _| c.restore_to_block(bid));
                log::info!("[checkpoint] restore-to-message: {summary}");
                let refresh_cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                ctx.update_model(&self.git_repo_status, |model, mctx| {
                    model.refresh_bypass_throttle(refresh_cwd, mctx);
                });
                ctx.notify();
            }
            TerminalAction::RevertLastAgentTurn => {
                let body = self
                    .agent_controller
                    .read(ctx, |c, _| {
                        c.checkpoints_latest_message()
                            .and_then(|block_id| c.checkpoint_restore_preview(block_id))
                    })
                    .map(|(stats, removed_user_turns)| {
                        checkpoint_preview_body(&stats, removed_user_turns)
                    })
                    .unwrap_or_else(|| {
                        "This restores tracked files to the snapshot taken before the \
                         agent's last change. Files created since are left in place."
                            .to_string()
                    });
                self.pending_confirm = Some(crate::confirm::ConfirmRequest::destructive(
                    "Revert agent edits?",
                    body,
                    "Revert",
                    TerminalAction::RevertLastAgentTurnConfirmed,
                ));
                ctx.notify();
                self.poke_workspace_confirm(ctx);
            }
            TerminalAction::RevertLastAgentTurnConfirmed => {
                let summary = ctx.update_model(&self.agent_controller, |c, _| c.revert_last_turn());
                log::info!("[checkpoint] {summary}");
                let refresh_cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                ctx.update_model(&self.git_repo_status, |model, mctx| {
                    model.refresh_bypass_throttle(refresh_cwd, mctx);
                });
                ctx.notify();
            }
            _ => {}
        }
    }
}
