//! Terminal git and branches actions.
//!
//! handle_terminal_git_action: moved verbatim; same behavior.

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
    pub(crate) fn handle_terminal_git_action(
        &mut self,
        action: &TerminalAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            TerminalAction::ToggleGitDiff => {
                // Header click = close (the card auto-reopens when the agent
                // touches new files; see the agent_controller subscription).
                self.git_diff_dismissed = true;
                ctx.notify();
            }
            TerminalAction::ToggleBranchDiff => {
                self.branch_diff_expanded = !self.branch_diff_expanded;
                ctx.notify();
            }
            TerminalAction::StageBranchFile { rel_path } => {
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let path = rel_path.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                // The source-control sidebar shares this action with the
                // terminal's branch-diff panel. Decide from the repository's
                // live index rather than the panel-local selection set.
                let unstaging = self.git_repo_status.read(ctx, |model, _| {
                    model.status.as_ref().is_some_and(|status| {
                        status.entries.iter().any(|entry| {
                            entry.rel_path == *rel_path && entry.index != ' ' && entry.index != '?'
                        })
                    })
                });
                if unstaging {
                    self.branch_diff_staged.remove(rel_path);
                } else {
                    self.branch_diff_staged.insert(rel_path.clone());
                }
                crate::bg::detach(move || {
                    // From the ROOT, not the terminal cwd: `rel_path` is
                    // root-relative but a pathspec resolves against the process
                    // cwd, so running this in a subdirectory matched nothing and
                    // the file silently stayed unstaged.
                    let root = crate::git_status::repo_root(&cwd);
                    let args: Vec<&str> = if unstaging {
                        vec!["restore", "--staged", "--", path.as_str()]
                    } else {
                        vec!["add", "--", path.as_str()]
                    };
                    let _ = tx.send_blocking(run_git(&root, &args));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        // A failed index command used to be discarded, so the next
                        // refresh just silently reverted the click. Say so instead.
                        if let Err(err) = result {
                            log::warn!("[git] stage/unstage failed: {err}");
                            view.show_error_notice(("Git index failed".to_string(), err.to_string()), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::StageAllBranchFiles => {
                // Optimistic UI update — mark all current diff files as staged immediately.
                let all_paths: std::collections::HashSet<String> =
                    self.git_repo_status.read(ctx, |m, _| {
                        m.status
                            .as_ref()
                            .map(|s| s.diff_files.iter().map(|f| f.rel_path.clone()).collect())
                            .unwrap_or_default()
                    });
                self.branch_diff_staged = all_paths;
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    // From the root: `git add .` in a subdirectory stages only
                    // that subdirectory, so "Stage all" quietly missed every
                    // file outside it.
                    let root = crate::git_status::repo_root(&cwd);
                    let _ = tx.send_blocking(run_git(&root, &["add", "."]));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        if let Err(err) = result {
                            log::warn!("[git] stage all failed: {err}");
                            view.show_error_notice(("Git operation failed".to_string(), format!("Could not stage all files: {err}")), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::UnstageAllBranchFiles => {
                self.branch_diff_staged.clear();
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let root = crate::git_status::repo_root(&cwd);
                    let _ = tx.send_blocking(run_git(&root, &["restore", "--staged", "."]));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        if let Err(err) = result {
                            log::warn!("[git] unstage all failed: {err}");
                            view.show_error_notice(("Git operation failed".to_string(), format!("Could not unstage all files: {err}")), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::DiscardBranchFile { rel_path } => {
                self.pending_confirm = Some(crate::confirm::ConfirmRequest::destructive(
                    "Discard changes to this file?",
                    format!("'{rel_path}' will be restored to its last committed state. This cannot be undone."),
                    "Discard",
                    TerminalAction::DiscardBranchFileConfirmed { rel_path: rel_path.clone() },
                ));
                ctx.notify();
                self.poke_workspace_confirm(ctx);
            }
            TerminalAction::DiscardBranchFileConfirmed { rel_path } => {
                // Optimistic: the file returns to HEAD, so it's no longer
                // "staged" once the operation lands (and the diff panel will
                // stop listing it at all on the next refresh).
                self.branch_diff_staged.remove(rel_path);
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let path = rel_path.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let root = crate::git_status::repo_root(&cwd);
                    let _ = tx.send_blocking(run_git(
                        &root,
                        &[
                            "restore",
                            "--source=HEAD",
                            "--staged",
                            "--worktree",
                            "--",
                            path.as_str(),
                        ],
                    ));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        if let Err(err) = result {
                            log::warn!("[git] discard file failed: {err}");
                            view.show_error_notice(("Git operation failed".to_string(), format!("Could not discard file: {err}")), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::DiscardAllBranchChanges => {
                let has_changes = self.git_repo_status.read(ctx, |m, _| {
                    m.status.as_ref().is_some_and(|s| !s.diff_files.is_empty())
                });
                if !has_changes {
                    return;
                }
                self.pending_confirm = Some(crate::confirm::ConfirmRequest::destructive(
                    "Discard all changes?",
                    "Every tracked file will be restored to its last committed state. New (untracked) files are left alone. This cannot be undone.",
                    "Discard All",
                    TerminalAction::DiscardAllBranchChangesConfirmed,
                ));
                ctx.notify();
                self.poke_workspace_confirm(ctx);
            }
            TerminalAction::DiscardAllStagedChanges => {
                let staged_paths = self.git_repo_status.read(ctx, |m, _| {
                    m.status
                        .as_ref()
                        .map(|status| status.staged_rel_paths.clone())
                        .unwrap_or_default()
                });
                if staged_paths.is_empty() {
                    return;
                }
                self.pending_confirm = Some(crate::confirm::ConfirmRequest::destructive(
                    "Discard all staged changes?",
                    format!(
                        "{} staged file{} will be restored to its last committed state. This cannot be undone.",
                        staged_paths.len(),
                        if staged_paths.len() == 1 { "" } else { "s" },
                    ),
                    "Discard All",
                    TerminalAction::DiscardAllStagedChangesConfirmed,
                ));
                ctx.notify();
                self.poke_workspace_confirm(ctx);
            }
            TerminalAction::DiscardAllStagedChangesConfirmed => {
                let staged_paths = self.git_repo_status.read(ctx, |m, _| {
                    m.status
                        .as_ref()
                        .map(|status| status.staged_rel_paths.clone())
                        .unwrap_or_default()
                });
                for path in &staged_paths {
                    self.branch_diff_staged.remove(path);
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let root = crate::git_status::repo_root(&cwd);
                    let mut args = vec!["restore", "--source=HEAD", "--staged", "--worktree", "--"];
                    args.extend(staged_paths.iter().map(String::as_str));
                    let _ = tx.send_blocking(run_git(&root, &args));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        if let Err(err) = result {
                            log::warn!("[git] discard staged failed: {err}");
                            view.show_error_notice(("Git operation failed".to_string(), format!("Could not discard staged changes: {err}")), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::DiscardAllBranchChangesConfirmed => {
                self.branch_diff_staged.clear();
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let git_status_handle = self.git_repo_status.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let root = crate::git_status::repo_root(&cwd);
                    let _ = tx.send_blocking(run_git(
                        &root,
                        &["restore", "--source=HEAD", "--staged", "--worktree", "."],
                    ));
                });
                ctx.spawn_stream_local(
                    rx,
                    move |view, result, vctx| {
                        if let Err(err) = result {
                            log::warn!("[git] discard all failed: {err}");
                            view.show_error_notice(("Git operation failed".to_string(), format!("Could not discard all changes: {err}")), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
                ctx.notify();
            }
            TerminalAction::GenerateBranchCommitMsg => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let staged: Vec<String> = self.branch_diff_staged.iter().cloned().collect();
                if staged.is_empty() {
                    return;
                }

                // Commit AI toggle gates generation.
                if !crate::settings::git_consumers::commit_ai_enabled() {
                    return;
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let agent_api = self.agent_api.clone();
                let selected = crate::settings::AppSettings::load().selected_model.clone();
                let model = crate::settings::git_consumers::commit_ai_model(&selected);
                let editor = self.branch_diff_commit_editor.clone();

                self.branch_diff_op = BranchDiffOp::Generating;
                ctx.notify();

                let (tx, rx) = async_channel::bounded::<String>(1);
                crate::bg::detach(move || {
                    let result = generate_commit_message_llm(&cwd, &staged, &agent_api, &model);
                    let _ = tx.send_blocking(result);
                });

                ctx.spawn_stream_local(
                    rx,
                    move |view, msg, vctx| {
                        view.branch_diff_op = BranchDiffOp::None;
                        vctx.update_view(&editor, move |e, ectx| e.set_text(&msg, ectx));
                        vctx.notify();
                    },
                    |_, _| {},
                );
            }
            TerminalAction::CommitBranchChanges => {
                let staged: Vec<String> = self.branch_diff_staged.iter().cloned().collect();
                if staged.is_empty() || self.branch_diff_op.is_busy() {
                    return;
                }
                let msg = self
                    .branch_diff_commit_editor
                    .read(ctx, |e, _| e.get_text().trim().to_string());
                if msg.is_empty() {
                    return;
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());

                self.branch_diff_op = BranchDiffOp::Committing;
                log::info!(
                    "[source-control] commit started: staged_files={}",
                    staged.len()
                );
                ctx.update_model(&self.git_repo_status, |model, model_ctx| {
                    model.begin_operation(crate::git_status::GitOperation::Committing, model_ctx);
                });
                ctx.notify();

                // Channel carries the error string on failure, empty string on success.
                let (tx, rx) = async_channel::bounded::<String>(1);
                let staged_thread = staged.clone();
                let msg_thread = msg.clone();
                crate::bg::detach(move || {
                    let cwd_str = cwd.to_str().unwrap_or(".");
                    // Resolve repo root so rel_paths (always root-relative) are found
                    // even if the terminal cwd is a subdirectory.
                    let repo_root = crate::git_cmd::GitCommand::new()
                        .no_window()
                        .args(["-C", cwd_str, "rev-parse", "--show-toplevel"])
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| cwd_str.to_string());
                    for path in &staged_thread {
                        let _ = crate::git_cmd::GitCommand::new()
                            .no_window()
                            .args(["-C", &repo_root, "add", "--", path])
                            .output();
                    }
                    let err = match crate::git_cmd::GitCommand::new()
                        .no_window()
                        .args(["-C", &repo_root, "commit", "-m", &msg_thread])
                        .output()
                    {
                        Ok(out) if out.status.success() => String::new(),
                        Ok(out) => {
                            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                            format!("{}{}", stdout, stderr)
                        }
                        Err(e) => e.to_string(),
                    };
                    let _ = tx.send_blocking(err);
                });

                let git_status_handle = self.git_repo_status.clone();
                let editor = self.branch_diff_commit_editor.clone();
                ctx.spawn_stream_local(
                    rx,
                    move |view, err, vctx| {
                        view.branch_diff_op = BranchDiffOp::None;
                        let operation_result = if err.is_empty() {
                            Ok(())
                        } else {
                            Err(err.trim().to_string())
                        };
                        if err.is_empty() {
                            log::info!("[source-control] commit completed successfully");
                            // Commit succeeded — clear staged set, commit message, and any prior error.
                            view.branch_diff_staged.clear();
                            view.error_toast = None;
                            vctx.update_view(&editor, |e, ectx| e.set_text("", ectx));
                        } else {
                            log::error!("[commit] git commit failed: {}", err.trim());
                            view.show_error_notice(("Commit failed".to_string(), err.trim().to_string()), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.finish_operation(
                                crate::git_status::GitOperation::Committing,
                                operation_result,
                                mctx,
                            );
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
            }
            TerminalAction::DismissCommitError => {
                self.error_toast = None;
                ctx.notify();
            }
            TerminalAction::CopyCommitError { text } => {
                self.write_output_clipboard(text.clone(), ctx);
                self.commit_error_copied = true;
                ctx.notify();
                ctx.spawn(
                    Timer::after(Duration::from_millis(1500)),
                    move |view, _, mut ctx| {
                        view.commit_error_copied = false;
                        ctx.notify();
                    },
                );
            }
            TerminalAction::CommitAndPushBranch => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let staged: Vec<String> = self.branch_diff_staged.iter().cloned().collect();
                let msg = self
                    .branch_diff_commit_editor
                    .read(ctx, |e, _| e.get_text().trim().to_string());
                // Committing needs a message; with nothing staged this is a pure push.
                if !staged.is_empty() && msg.is_empty() {
                    return;
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());

                self.branch_diff_op = BranchDiffOp::Pushing;
                log::info!(
                    "[source-control] commit & push started: staged_files={}",
                    staged.len()
                );
                ctx.update_model(&self.git_repo_status, |model, model_ctx| {
                    model.begin_operation(crate::git_status::GitOperation::Pushing, model_ctx);
                });
                ctx.notify();

                // Channel carries the error string on failure, empty on success.
                let (tx, rx) = async_channel::bounded::<String>(1);
                crate::bg::detach(move || {
                    let cwd_str = cwd.to_str().unwrap_or(".");
                    let repo_root = crate::git_cmd::GitCommand::new()
                        .no_window()
                        .args(["-C", cwd_str, "rev-parse", "--show-toplevel"])
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| cwd_str.to_string());

                    let mut err = String::new();
                    if !staged.is_empty() {
                        for path in &staged {
                            let _ = crate::git_cmd::GitCommand::new()
                                .no_window()
                                .args(["-C", &repo_root, "add", "--", path])
                                .output();
                        }
                        err = match crate::git_cmd::GitCommand::new()
                            .no_window()
                            .args(["-C", &repo_root, "commit", "-m", &msg])
                            .output()
                        {
                            Ok(out) if out.status.success() => String::new(),
                            Ok(out) => format!(
                                "{}{}",
                                String::from_utf8_lossy(&out.stdout),
                                String::from_utf8_lossy(&out.stderr)
                            ),
                            Err(e) => e.to_string(),
                        };
                    }
                    // Push only when the commit step succeeded (or was skipped).
                    if err.is_empty() {
                        let branch = current_branch(&repo_root);
                        err = match crate::git_cmd::GitCommand::new()
                            .no_window()
                            .args(["-C", &repo_root, "push", "origin", &branch])
                            .output()
                        {
                            Ok(out) if out.status.success() => String::new(),
                            Ok(out) => format!(
                                "{}{}",
                                String::from_utf8_lossy(&out.stdout),
                                String::from_utf8_lossy(&out.stderr)
                            ),
                            Err(e) => e.to_string(),
                        };
                    }
                    let _ = tx.send_blocking(err);
                });

                let git_status_handle = self.git_repo_status.clone();
                let editor = self.branch_diff_commit_editor.clone();
                ctx.spawn_stream_local(
                    rx,
                    move |view, err: String, vctx| {
                        view.branch_diff_op = BranchDiffOp::None;
                        let operation_result = if err.is_empty() {
                            Ok(())
                        } else {
                            Err(err.trim().to_string())
                        };
                        if err.is_empty() {
                            log::info!("[source-control] commit & push completed successfully");
                            view.branch_diff_staged.clear();
                            view.error_toast = None;
                            vctx.update_view(&editor, |e, ectx| e.set_text("", ectx));
                        } else {
                            view.show_error_notice(("Commit & push failed".to_string(), err), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.finish_operation(
                                crate::git_status::GitOperation::Pushing,
                                operation_result,
                                mctx,
                            );
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
            }
            TerminalAction::ToggleCheckpointPreview { block_id } => {
                self.open_checkpoint_preview = if self.open_checkpoint_preview == Some(*block_id) {
                    None
                } else {
                    Some(*block_id)
                };
                ctx.notify();
            }
            TerminalAction::CloseCheckpointPreview => {
                if self.open_checkpoint_preview.take().is_some() {
                    ctx.notify();
                }
            }
            TerminalAction::PullBranchChanges => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                self.branch_diff_op = BranchDiffOp::Pulling;
                ctx.update_model(&self.git_repo_status, |model, model_ctx| {
                    model.begin_operation(crate::git_status::GitOperation::Pulling, model_ctx);
                });
                ctx.notify();

                let (tx, rx) = async_channel::bounded::<String>(1);
                crate::bg::detach(move || {
                    let cwd_str = cwd.to_str().unwrap_or(".");
                    let branch = current_branch(cwd_str);
                    let err = match crate::git_cmd::GitCommand::new()
                        .no_window()
                        .args(["-C", cwd_str, "pull", "origin", &branch])
                        .output()
                    {
                        Ok(out) if out.status.success() => String::new(),
                        Ok(out) => format!(
                            "{}{}",
                            String::from_utf8_lossy(&out.stdout),
                            String::from_utf8_lossy(&out.stderr)
                        ),
                        Err(e) => e.to_string(),
                    };
                    let _ = tx.send_blocking(err);
                });

                let git_status_handle = self.git_repo_status.clone();
                ctx.spawn_stream_local(
                    rx,
                    move |view, err: String, vctx| {
                        view.branch_diff_op = BranchDiffOp::None;
                        let result = if err.is_empty() {
                            Ok(())
                        } else {
                            Err(err.clone())
                        };
                        vctx.update_model(&git_status_handle, |model, model_ctx| {
                            model.finish_operation(
                                crate::git_status::GitOperation::Pulling,
                                result,
                                model_ctx,
                            );
                        });
                        if !err.is_empty() {
                            // Surfaced in the branch panel's error line (e.g. a
                            // merge conflict needs the user's hands anyway).
                            view.show_error_notice(("Branch operation failed".to_string(), err), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
            }
            TerminalAction::PushBranchChanges => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                self.branch_diff_op = BranchDiffOp::Pushing;
                ctx.update_model(&self.git_repo_status, |model, model_ctx| {
                    model.begin_operation(crate::git_status::GitOperation::Pushing, model_ctx);
                });
                ctx.notify();

                let (tx, rx) = async_channel::bounded::<String>(1);
                crate::bg::detach(move || {
                    let cwd_str = cwd.to_str().unwrap_or(".");
                    let branch = current_branch(cwd_str);
                    let err = match crate::git_cmd::GitCommand::new()
                        .no_window()
                        .args(["-C", cwd_str, "push", "origin", &branch])
                        .output()
                    {
                        Ok(out) if out.status.success() => String::new(),
                        Ok(out) => format!(
                            "{}{}",
                            String::from_utf8_lossy(&out.stdout),
                            String::from_utf8_lossy(&out.stderr)
                        ),
                        Err(error) => error.to_string(),
                    };
                    let _ = tx.send_blocking(err);
                });

                let git_status_handle = self.git_repo_status.clone();
                ctx.spawn_stream_local(
                    rx,
                    move |view, err, vctx| {
                        view.branch_diff_op = BranchDiffOp::None;
                        let result = if err.is_empty() {
                            Ok(())
                        } else {
                            Err(err.clone())
                        };
                        vctx.update_model(&git_status_handle, |model, model_ctx| {
                            model.finish_operation(
                                crate::git_status::GitOperation::Pushing,
                                result,
                                model_ctx,
                            );
                        });
                        if !err.is_empty() {
                            view.show_error_notice(("Branch operation failed".to_string(), err), vctx);
                        }
                        let refresh_cwd = view.git_repo_status.read(vctx, |m, _| m.cwd.clone());
                        vctx.update_model(&git_status_handle, |model, mctx| {
                            model.refresh_bypass_throttle(refresh_cwd, mctx);
                        });
                        vctx.notify();
                    },
                    |_, _| {},
                );
            }
            // ── Branch popover ────────────────────────────────────────────
            TerminalAction::ToggleBranchMenu => {
                self.branch_menu_open = !self.branch_menu_open;
                self.folder_menu_open = false; // one popover at a time
                self.mode_menu_open = false;
                self.model_menu_open = false;
                if self.branch_menu_open {
                    // Re-list every time: branches change outside the app.
                    self.branch_list = None;
                    let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
                    crate::bg::detach(move || {
                        let _ = tx.send_blocking(list_local_branches(&cwd));
                    });
                    ctx.spawn_stream_local(
                        rx,
                        move |view, branches, vctx| {
                            view.branch_list = Some(branches);
                            vctx.notify();
                        },
                        |_, _| {},
                    );
                    ctx.update_view(&self.new_branch_editor, |e, ectx| {
                        e.set_text("", ectx);
                        ectx.focus_self();
                    });
                }
                ctx.notify();
            }
            TerminalAction::CloseBranchMenu => {
                self.branch_menu_open = false;
                ctx.notify();
            }
            TerminalAction::BranchListLoaded { branches } => {
                self.branch_list = Some(branches.clone());
                ctx.notify();
            }
            TerminalAction::CreateBranchFromName { name } => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let raw = name.trim().to_string();
                if raw.is_empty() {
                    return;
                }
                if let Err(why) = validate_branch_name(&raw) {
                    self.show_error_notice(("Invalid branch name".to_string(), why), ctx);
                    ctx.notify();
                    return;
                }
                self.branch_diff_op = BranchDiffOp::Committing;
                ctx.notify();
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let _ = tx.send_blocking(run_git_switch(&cwd, &raw, true));
                });
                self.finish_branch_op(rx, ctx);
            }
            TerminalAction::SwitchBranch { name, force } => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let current = self
                    .git_repo_status
                    .read(ctx, |m, _| m.status.as_ref().map(|s| s.branch.clone()))
                    .unwrap_or_default();
                if *name == current {
                    self.branch_menu_open = false;
                    ctx.notify();
                    return;
                }
                // Uncommitted work is the one way this loses data. git itself
                // refuses when a switch would clobber a modified file, but it
                // happily carries changes across otherwise — which surprises
                // people. Ask first, and let the user opt in.
                let dirty = self.git_repo_status.read(ctx, |m, _| {
                    m.status
                        .as_ref()
                        .map(|s| s.staged + s.modified)
                        .unwrap_or(0)
                });
                if dirty > 0 && !force {
                    self.branch_menu_open = false;
                    self.pending_confirm = Some(crate::confirm::ConfirmRequest::destructive(
                        "Switch branch with uncommitted changes?",
                        format!(
                            "{dirty} uncommitted change(s) will be carried over to '{name}'. \
                             If any of them conflict with that branch, git will refuse and \
                             nothing will move. Commit or stash first to keep them on '{current}'."
                        ),
                        "Switch anyway",
                        TerminalAction::SwitchBranch {
                            name: name.clone(),
                            force: true,
                        },
                    ));
                    ctx.notify();
                    self.poke_workspace_confirm(ctx);
                    return;
                }

                self.branch_menu_open = false;
                self.branch_diff_op = BranchDiffOp::Committing;
                ctx.notify();
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let target = name.clone();
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let _ = tx.send_blocking(run_git_switch(&cwd, &target, false));
                });
                self.finish_branch_op(rx, ctx);
            }
            TerminalAction::CreateBranchFromInput => {
                if self.branch_diff_op.is_busy() {
                    return;
                }
                let raw = self
                    .new_branch_editor
                    .read(ctx, |e, _| e.get_text().trim().to_string());
                if raw.is_empty() {
                    return;
                }
                if let Err(why) = validate_branch_name(&raw) {
                    self.show_error_notice(("Invalid branch name".to_string(), why), ctx);
                    ctx.notify();
                    return;
                }
                self.branch_menu_open = false;
                self.branch_diff_op = BranchDiffOp::Committing;
                ctx.notify();
                ctx.update_view(&self.new_branch_editor, |e, ectx| e.set_text("", ectx));
                let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                let (tx, rx) = async_channel::bounded::<Result<(), String>>(1);
                crate::bg::detach(move || {
                    let _ = tx.send_blocking(run_git_switch(&cwd, &raw, true));
                });
                self.finish_branch_op(rx, ctx);
            }
            // ── Folder popover ────────────────────────────────────────────
            TerminalAction::ToggleFolderMenu => {
                self.folder_menu_open = !self.folder_menu_open;
                self.branch_menu_open = false;
                self.mode_menu_open = false;
                self.model_menu_open = false;
                if self.folder_menu_open {
                    self.folder_list = None;
                    let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());
                    let (tx, rx) = async_channel::bounded::<Vec<String>>(1);
                    crate::bg::detach(move || {
                        let _ = tx.send_blocking(list_subdirectories(&cwd));
                    });
                    ctx.spawn_stream_local(
                        rx,
                        move |view, dirs, vctx| {
                            view.folder_list = Some(dirs);
                            vctx.notify();
                        },
                        |_, _| {},
                    );
                }
                ctx.notify();
            }
            TerminalAction::CloseFolderMenu => {
                self.folder_menu_open = false;
                ctx.notify();
            }
            TerminalAction::FolderListLoaded { dirs } => {
                self.folder_list = Some(dirs.clone());
                ctx.notify();
            }
            TerminalAction::ChangeDirectory { path } => {
                self.folder_menu_open = false;
                // Through the PTY rather than an internal cwd assignment: the
                // shell owns cwd (we only ever learn it back via the Precmd
                // hook), and this way the `cd` lands in the user's own history
                // and scrollback exactly as if they had typed it.
                self.submit_shell_command_inner(format!("cd {}", shell_quote(&path)), ctx);
            }
            _ => {}
        }
    }
}
