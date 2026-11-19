//! Terminal input and terminal I/O actions.
//!
//! handle_terminal_input_action: moved verbatim; same behavior.

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
    pub(crate) fn handle_terminal_input_action(
        &mut self,
        action: &TerminalAction,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.host_notice_active { return; }
        match action {
            TerminalAction::FocusOutput => {
                // Any click outside an open input-bar dropdown dismisses it.
                if self.close_input_bar_menus() {
                    ctx.notify();
                }
                // A click on the raw REPL output area must not steal focus
                // away from an open rich input panel — see `wrap_terminal_input`
                // and `ensure_command_input_focused` for the rest of this.
                if self.rich_input_active {
                    ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                        ectx.focus_self();
                    });
                } else {
                    self.pane_has_focus = true;
                    ctx.focus_self();
                }
                ctx.emit(());
            }
            TerminalAction::FocusInput => {
                self.close_input_bar_menus();
                self.pane_has_focus = true;
                self.clear_block_hints(ctx);
                self.clear_block_selection();
                ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
                self.close_history_search(ctx, true);
                self.ensure_command_input_focused(ctx);
                ctx.emit(());
            }
            TerminalAction::ClearBlockHints => {
                self.clear_block_hints(ctx);
            }
            TerminalAction::AttachDroppedFiles { paths } => {
                let paths: Vec<std::path::PathBuf> =
                    paths.iter().map(std::path::PathBuf::from).collect();
                self.receive_context_files(paths, ctx);
            }
            TerminalAction::RemovePendingAttachment { index } => {
                if *index < self.pending_attachments.len() {
                    self.pending_attachments.remove(*index);
                    // Indices shifted — the viewer's ordinal may now be wrong.
                    self.image_viewer = None;
                    ctx.notify();
                }
            }
            TerminalAction::ClearInputEditorSelection => {
                self.clear_command_editor_selection(ctx);
            }
            TerminalAction::HistoryUp => {
                self.recall_command_history(true, ctx);
                ctx.notify();
            }
            TerminalAction::HistoryDown => {
                self.recall_command_history(false, ctx);
                ctx.notify();
            }
            TerminalAction::RichHistoryUp => self.recall_rich_history(true, ctx),
            TerminalAction::RichHistoryDown => self.recall_rich_history(false, ctx),
            TerminalAction::AcceptGhost => {
                self.accept_command_ghost(ctx);
                ctx.notify();
            }
            TerminalAction::Submit => {
                let is_question_focused = self.question_editor.read(ctx, |e, cx| e.is_focused(cx));
                let is_feedback_focused = self.feedback_editor.read(ctx, |e, cx| e.is_focused(cx));
                if is_question_focused {
                    // Enter in the "Other" field submits the typed answer to the
                    // open question card.
                    let text = self
                        .question_editor
                        .read(ctx, |e, _| e.get_text().to_string());
                    let pending_id = ctx
                        .read_model(&self.agent_controller, |c, _| c.first_pending_question_id());
                    if let (Some(action_id), false) = (pending_id, text.trim().is_empty()) {
                        ctx.update_model(&self.agent_controller, |c, mctx| {
                            c.answer_user_question_text(action_id, text, mctx);
                        });
                        ctx.update_view(&self.question_editor, |e, ectx| e.set_text("", ectx));
                    }
                } else if is_feedback_focused {
                    let feedback = self
                        .feedback_editor
                        .read(ctx, |e, _| e.get_text().to_string())
                        .trim()
                        .to_string();
                    let feedback_opt = if feedback.is_empty() {
                        None
                    } else {
                        Some(feedback)
                    };
                    // Find active pending approval request block ID to reject
                    let pending_id =
                        ctx.read_model(&self.agent_controller, |c, _| c.first_pending_action_id());
                    if let Some(action_id) = pending_id {
                        let is_streaming =
                            ctx.read_model(&self.agent_controller, |c, _| c.is_streaming());
                        if is_streaming {
                            ctx.update_model(&self.agent_controller, |c, mctx| {
                                c.resolve_streaming_approval(
                                    action_id,
                                    false,
                                    feedback_opt.clone(),
                                    mctx,
                                );
                            });
                        } else {
                            ctx.update_model(&self.agent_controller, |c, mctx| {
                                c.reject_action(action_id, feedback_opt.clone(), mctx);
                            });
                        }
                        ctx.update_view(&self.feedback_editor, |e, ectx| {
                            e.set_text("", ectx);
                        });
                        self.ensure_command_input_focused(ctx);
                    }
                } else {
                    self.submit_command_editor(ctx);
                    // SMART SCROLL STATE RE-ARM: Force active tracking on submission
                    self.auto_scroll = true;
                    self.scroll_to_bottom();
                }
                ctx.notify();
            }
            TerminalAction::InsertNewline => {
                ctx.update_view(&self.command_editor, |editor, ectx| {
                    editor.insert_text_at_cursor("\n", ectx);
                });
                ctx.notify();
            }
            TerminalAction::Interrupt => {
                // Shell integration drives the presentation model, but it can be
                // unavailable (or briefly stale while hosting a shared session).
                // The PTY foreground process group is authoritative for whether
                // Ctrl+C must also issue SIGINT rather than only write 0x03.
                let running = self.model.read(ctx, |m, _| m.is_command_running())
                    || self.manager.writer().foreground_running();
                if running {
                    // A running command must be interrupted, never have Ctrl+C
                    // hijacked into a copy. Send the tty interrupt char AND a
                    // direct SIGINT to the process group (reliably stops
                    // `npm run …` whose children a bare 0x03 may not reach).
                    self.manager.writer().write(&[0x03]);
                    self.manager.writer().interrupt();
                    ctx.notify();
                } else if self.copy_selected_output(ctx) {
                    ctx.notify();
                } else {
                    self.manager.writer().write(&[0x03]);
                    ctx.notify();
                }
            }
            TerminalAction::ToggleRichInput => {
                self.rich_input_active = !self.rich_input_active;
                // Mirror: remember the choice so the host's surface updates don't
                // immediately reopen a panel the viewer just closed.
                if self.is_remote {
                    self.remote_rich_input_closed = !self.rich_input_active;
                }
                self.set_rich_input_proxy_chars(0, ctx);
                self.last_dims = None;
                if self.rich_input_active {
                    ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                        ectx.focus_self();
                    });
                } else {
                    ctx.focus_self();
                }
                ctx.notify();
            }
            TerminalAction::SubmitRichInput => {
                // If the `!` history popover is open, the previewed `!cmd` is
                // already in the editor — close the popover (keep the preview)
                // before submitting so it doesn't linger.
                if self.cmd_hist_open {
                    self.close_cmd_history(false, ctx);
                }
                // Shouldn't normally be nonzero here — the proxy-trigger path
                // bypasses the rich editor's buffer entirely, so a real
                // blob-submit only happens from genuine compose-mode typing —
                // but zero it defensively in case of a missed edge case.
                self.set_rich_input_proxy_chars(0, ctx);
                self.rich_input_history_idx = None; // fresh history position next time
                if self.voice_session.is_some() {
                    self.voice_rebase_pending = true;
                }
                let text = self
                    .rich_input_editor
                    .read(ctx, |editor, _| editor.get_text().to_string())
                    .trim()
                    .to_string();
                let tool = self.active_ai_tool(ctx);
                // `!<command>` runs in the persistent scratch shell shown in a card
                // above the input INSTEAD of going to the AI tool — state (cd/env)
                // persists across commands. Bare `!` just reopens the card.
                if let Some(cmd) = text.strip_prefix('!') {
                    self.send_side_command(cmd.trim().to_string(), ctx);
                } else if !text.is_empty() && self.acp_chat_shown(ctx) {
                    self.acp_send(text.clone(), ctx);
                } else if !text.is_empty() {
                    if self.showing_cli_chat(ctx) {
                        self.note_cli_chat_sent(text.clone(), ctx);
                    }
                    // Send the whole (possibly multi-line) blob as a BRACKETED
                    // PASTE, then — after a delay — a single CR to submit.
                    //
                    // Two bugs this fixes together:
                    //  1. The old path wrote raw text + a bare CR, so the wrapped
                    //     CLI saw each embedded newline as its own line-submit and
                    //     the trailing CR only sent the last line. Bracketed paste
                    //     (ESC[200~…ESC[201~, which Claude Code and Codex enable)
                    //     makes the newlines DATA, not submits.
                    //  2. These are Ink/React apps that process a paste
                    //     asynchronously. A CR sent in the same burst is absorbed
                    //     INTO the paste rather than submitting it — which is why
                    //     the text landed in the tool's input but needed a second
                    //     Enter. The CR is now ALWAYS delayed (not just for REPLs)
                    //     so the paste settles first and the CR lands as a clean
                    //     submit keystroke.
                    let mode = self.model.read(ctx, |m, _| {
                        m.with_core(|core| core.active_emulator().mode())
                    });
                    self.manager.writer().write(&encode_paste(&text, mode));
                    let delay = tool.map(|t| t.submit_cr_delay_ms()).unwrap_or(160);
                    ctx.spawn(
                        Timer::after(Duration::from_millis(delay)),
                        move |view, _, _| {
                            view.manager.writer().write(b"\r");
                        },
                    );
                }
                ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                    editor.set_text("", ectx);
                });
                // Controller → host: `set_text` fires no Changed event, so forward
                // the cleared AI input explicitly — otherwise the host keeps the
                // stale text this controller just submitted to the AI tool.
                self.forward_control_surface(
                    dragent_session_protocol::ControlSurface {
                        ai_input: Some(String::new()),
                        ai_cursor: Some(0),
                        ..Default::default()
                    },
                    ctx,
                );
                ctx.update_view(&self.rich_input_editor, |_editor, ectx| {
                    ectx.focus_self();
                });
                ctx.notify();
            }
            TerminalAction::ToggleVoiceInput => {
                self.toggle_voice_input(ctx);
            }
            TerminalAction::OpenFilePicker => {
                self.attach_context_files(ctx);
            }
            TerminalAction::ShiftHeld(held) => {
                self.shift_held = *held;
            }
            TerminalAction::CtrlHeld(held) => {
                self.ctrl_held = *held;
            }
            TerminalAction::AltHeld(held) => {
                self.alt_held = *held;
            }
            TerminalAction::CmdHeld(held) => {
                self.cmd_held = *held;
            }
            TerminalAction::Copy => {
                if self.copy_selected_output(ctx) {
                    ctx.notify();
                }
            }
            TerminalAction::Paste => {
                let content = ctx.clipboard().read().plain_text;
                if content.is_empty() {
                    return;
                }
                if self.ai_rich_input_panel_open(ctx) && self.rich_input_proxy_chars > 0 {
                    let mode = self.active_term_mode(ctx);
                    self.manager.writer().write(&encode_paste(&content, mode));
                    self.set_rich_input_proxy_chars(
                        self.rich_input_proxy_chars + content.chars().count(),
                        ctx,
                    );
                    ctx.notify();
                    return;
                }
                if self.ai_rich_input_panel_open(ctx) {
                    self.paste_to_rich_input_editor(&content, ctx);
                    return;
                }
                if self.local_input_active(ctx) {
                    ctx.update_view(&self.command_editor, |editor, ectx| {
                        editor.insert_text_at_cursor(&content, ectx);
                    });
                } else {
                    let mode = self.model.read(ctx, |m, _| {
                        m.with_core(|core| core.active_emulator().mode())
                    });
                    let bytes = encode_paste(&content, mode);
                    self.manager.writer().write(&bytes);
                }
                ctx.notify();
            }
            TerminalAction::SendPty(bytes) => {
                self.manager.writer().write(bytes);
            }
            TerminalAction::TerminalMouse {
                position,
                button,
                kind,
                modifiers,
            } => {
                self.handle_terminal_mouse(*position, *button, *kind, *modifiers, ctx);
            }
            TerminalAction::TerminalWheel {
                position,
                delta_y,
                precise,
                modifiers,
            } => {
                self.handle_terminal_wheel(*position, *delta_y, *precise, *modifiers, ctx);
            }
            TerminalAction::BlockScroll { .. } => {
                // Physical scroll already applied by BlockListScrollAdapter::scroll().
                // Sync auto_scroll from the updated logical position.
                let pos = self.block_scroll_pos.lock().unwrap();
                self.auto_scroll = matches!(pos.mode, ScrollMode::FollowBottom);
                drop(pos);
                self.schedule_sync_sticky_header(ctx);
                if self.is_remote {
                    // Controller → host: forward the scroll so the host desktop
                    // scrolls where the web participant scrolls (no-op unless this
                    // is a controlling mirror).
                    let fraction = self.scroll_fraction_for_share();
                    self.forward_control_surface(
                        dragent_session_protocol::ControlSurface {
                            scroll: Some(dragent_session_protocol::ScrollUpdate { fraction }),
                            ..Default::default()
                        },
                        ctx,
                    );
                } else {
                    // Host → viewers: mirror the host's scroll position.
                    self.broadcast_surface_state(ctx);
                }
                ctx.notify();
            }
            TerminalAction::UserScrollInteraction => {
                self.schedule_sync_auto_scroll(ctx);
                self.schedule_sync_sticky_header(ctx);
            }
            TerminalAction::DragSelectScrollCheck { cursor_y } => {
                let id = format!("terminal_output_viewport_{}", self.pane_uuid);
                let Some(viewport) = ctx.element_position_by_id(&id) else {
                    return;
                };
                let vp_top = viewport.origin_y();
                let vp_bottom = vp_top + viewport.height();
                // scroll_delta > 0 = scroll DOWN; < 0 = scroll UP
                let scroll_delta = if *cursor_y < vp_top {
                    -((vp_top - cursor_y).min(40.0))
                } else if *cursor_y > vp_bottom {
                    (cursor_y - vp_bottom).min(40.0)
                } else {
                    return;
                };
                let total = self.block_height_index.total_height_px();
                let new_pos = self.block_scroll_pos.lock().unwrap().scroll_by(
                    scroll_delta,
                    total,
                    self.output_viewport_height_px,
                );
                self.auto_scroll = matches!(new_pos.mode, ScrollMode::FollowBottom);
                *self.block_scroll_pos.lock().unwrap() = new_pos;
                self.schedule_sync_sticky_header(ctx);
                ctx.notify();
            }
            TerminalAction::TabComplete => {
                if self.workflow_session.is_some() {
                    self.cycle_workflow_arg(false, ctx);
                    return;
                }
                let completion_open = self.path_picker.read(ctx, |m, _| m.open);
                if completion_open {
                    ctx.update_model(&self.path_picker, |m, model_ctx| {
                        m.select_next(model_ctx);
                    });
                } else {
                    let buffer = self
                        .command_editor
                        .read(ctx, |editor, _| editor.get_text().to_string());
                    if buffer == "cd" {
                        self.apply_path_completion("cd ".to_string(), ctx);
                        return;
                    }
                    let cwd = self.model.read(ctx, |m, _| m.cwd().to_path_buf());
                    if should_suggest_paths(&buffer) {
                        let with_help = self.input_config.input_type == DetectedInputType::Agent;
                        ctx.update_model(&self.path_picker, |m, model_ctx| {
                            m.try_open_for_buffer(&buffer, &cwd, with_help, model_ctx);
                        });
                        // Exactly one candidate — whether from a typed prefix
                        // (`cd dr` → dragent) or a directory with a single
                        // entry (`cd ` + Tab) — completes immediately instead
                        // of opening a one-item picker.
                        let unique = self
                            .path_picker
                            .read(ctx, |m, _| m.open && m.entries().len() == 1);
                        if unique {
                            if let Some(new_buffer) = self
                                .path_picker
                                .read(ctx, |m, _| m.accept_selection(&buffer))
                            {
                                ctx.update_model(&self.path_picker, |m, model_ctx| {
                                    m.close(model_ctx)
                                });
                                self.apply_path_completion(new_buffer, ctx);
                                ctx.notify();
                                return;
                            }
                        }
                        ctx.update_view(&self.command_editor, |editor, ectx| {
                            editor.set_ghost_suffix(None, ectx);
                        });
                    }
                }
                ctx.notify();
            }
            TerminalAction::TabCompletePrev => {
                if self.workflow_session.is_some() {
                    self.cycle_workflow_arg(true, ctx);
                    return;
                }
                let completion_open = self.path_picker.read(ctx, |m, _| m.open);
                if completion_open {
                    ctx.update_model(&self.path_picker, |m, model_ctx| {
                        m.select_prev(model_ctx);
                    });
                    ctx.notify();
                }
            }
            TerminalAction::AcceptCompletion => {
                let completion_open = self.path_picker.read(ctx, |m, _| m.open);
                if completion_open {
                    let buffer = self
                        .command_editor
                        .read(ctx, |editor, _| editor.get_text().to_string());
                    if let Some(new_buffer) = self
                        .path_picker
                        .read(ctx, |m, _| m.accept_selection(&buffer))
                    {
                        self.apply_path_completion(new_buffer, ctx);
                    }
                    ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
                    ctx.notify();
                }
            }
            TerminalAction::DismissCompletion => {
                ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
                let text = self
                    .command_editor
                    .read(ctx, |editor, _| editor.get_text().to_string());
                self.refresh_command_ghost(&text, ctx);
                ctx.notify();
            }
            TerminalAction::ClickPathCompletion { index } => {
                let completion_open = self.path_picker.read(ctx, |m, _| m.open);
                if completion_open {
                    let buffer = self
                        .command_editor
                        .read(ctx, |editor, _| editor.get_text().to_string());
                    if let Some(new_buffer) = self
                        .path_picker
                        .read(ctx, |m, _| m.accept_index(&buffer, *index))
                    {
                        self.apply_path_completion(new_buffer, ctx);
                    }
                    ctx.update_model(&self.path_picker, |m, model_ctx| m.close(model_ctx));
                    ctx.notify();
                }
            }
            TerminalAction::ToggleVimMode => {
                self.toggle_vim_mode(ctx);
            }
            TerminalAction::ToggleInputTypeLock => {
                self.toggle_input_type_lock(ctx);
            }
            TerminalAction::ForwardRichInputProxyKey { bytes, delta } => {
                self.manager.writer().write(bytes);
                let chars = (self.rich_input_proxy_chars as i32 + delta).max(0) as usize;
                self.set_rich_input_proxy_chars(chars, ctx);
                if chars == 0 {
                    ctx.update_view(&self.rich_input_editor, |editor, ectx| {
                        editor.set_text("", ectx);
                    });
                    self.rich_input_history_idx = None;
                    self.forward_control_surface(
                        dragent_session_protocol::ControlSurface {
                            ai_input: Some(String::new()),
                            ai_cursor: Some(0),
                            ..Default::default()
                        },
                        ctx,
                    );
                }
                ctx.notify();
            }
            _ => {}
        }
    }
}
