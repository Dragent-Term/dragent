//! Terminal session construction (new_inner).
//!
//! The main TerminalView constructor. Moved verbatim; same behavior.

use super::actions::TerminalAction;
use super::terminal_helpers::*;
use super::terminal_side_shell::*;
use super::terminal_view::{BranchDiffOp, TerminalView, RESIZE_PADDING_X, RESIZE_PADDING_Y};
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

/// What backs a terminal view's PTY side.
pub(crate) enum TerminalBackend {
    /// Classic in-process shell. Spawns via `TerminalManager::spawn`.
    #[cfg(any(test, feature = "performance-harness"))]
    Local { preferred_shell: Option<String> },
    /// Shared-session mirror: no local shell; output arrives via
    /// `feed_remote_output`, input forwards through `sink`.
    Remote {
        sink: crate::terminal::manager::pty_writer::RemoteInputSink,
    },
    /// Host-OWNED tab: no local shell; same feed path as a mirror, but the
    /// view keeps local layout/input behavior while input/resize route to
    /// the daemon's PTY through the binding slot (buffered until bound).
    Host {
        binding: std::sync::Arc<crate::terminal::manager::pty_writer::HostPtyBinding>,
    },
}

/// What a terminal reports to apps asking about its colours under `theme`.
fn terminal_colors(theme: &Theme) -> dragent_host::screen_protocol::TerminalColors {
    let rgb = |c: warpui::color::ColorU| (c.r, c.g, c.b);
    dragent_host::screen_protocol::TerminalColors {
        foreground: rgb(theme.foreground),
        background: rgb(theme.background),
        is_dark: theme.is_dark,
    }
}

impl TerminalView {
    pub(crate) fn new_inner(
        cwd: PathBuf,
        backend: TerminalBackend,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let theme = UiSettings::load().theme();
        let mono_family = load_mono_font(ctx);
        let cell_metrics = compute_cell_metrics(ctx, mono_family);
        log::info!(
            "[fonts] session cell metrics: {:.3}x{:.3} px (family id {mono_family:?})",
            cell_metrics.width,
            cell_metrics.height,
        );

        let size = SizeInfo::from_pane(
            760.0,
            480.0,
            cell_metrics.width,
            cell_metrics.height,
            RESIZE_PADDING_X,
            RESIZE_PADDING_Y,
        );
        // Precedence: new-tab picker choice > Settings default shell > $SHELL.
        let settings_shell = crate::console::editor_settings::EditorSettings::global()
            .terminal
            .default_shell
            .clone();
        let preferred_shell = match &backend {
            #[cfg(any(test, feature = "performance-harness"))]
            TerminalBackend::Local { preferred_shell } => Some(resolve_shell(
                preferred_shell
                    .clone()
                    .or(settings_shell)
                    .or_else(|| std::env::var("SHELL").ok()),
            )),
            TerminalBackend::Remote { .. } | TerminalBackend::Host { .. } => None,
        };
        let shell = preferred_shell.clone();
        let is_remote = matches!(backend, TerminalBackend::Remote { .. });
        // Local shells only: mirrors and host-owned tabs have no local PTY.
        #[cfg(any(test, feature = "performance-harness"))]
        let drive_automation = matches!(backend, TerminalBackend::Local { .. });
        #[cfg(not(any(test, feature = "performance-harness")))]
        let drive_automation = false;
        // Mirrors join mid-session, so they never saw the host's first
        // `precmd` and must not reconstruct blocks from the byte stream
        // (the host's snapshots are authoritative instead). Host-owned
        // tabs are present from spawn: their stream is complete, so
        // blocks parse locally exactly like an in-process shell — without
        // this, no blocks ever form and the submit passthrough never
        // clears, leaving the input bar permanently inert.
        // A mirror joins mid-session, so it never saw the host's first `precmd`
        // and would otherwise FREEZE whatever partial output it accumulated
        // before that first hook fired — the stray shell prompt (`\$`, `%`) that
        // leaks above the first block. Suppressing startup output makes the first
        // precmd it sees CLEAR that pre-join junk (same as a fresh terminal drops
        // shell-startup noise), so the mirror shows only real block content.
        let suppress_startup_output = is_remote
            || shell
                .as_deref()
                .map(ShellKind::from_program)
                .is_some_and(|kind| kind != ShellKind::Other);
        let session_core = {
            let mut core = crate::terminal::model::TerminalSessionCore::new(
                cwd.clone(),
                size,
                suppress_startup_output,
            );
            // A mirror's blocks come from the host's snapshots, so it must not also
            // build them from the byte stream — that rendered every command twice.
            core.set_suppress_local_blocks(is_remote);
            core.set_host_resume_pending(matches!(backend, TerminalBackend::Host { .. }));
            Arc::new(FairMutex::new(core))
        };
        let model = ctx.add_model(|_| TerminalModel::from_core(session_core.clone()));
        let event_listener = TerminalEventListener::new();
        let theme_for_feed = theme.clone();
        let feed_core = session_core.clone();
        let feed: dragent_pty::FeedCallback =
            Box::new(move |bytes| feed_core.lock().feed_pty_bytes(bytes, &theme_for_feed));
        let sync_buffer_len: dragent_pty::SyncBufferLenCallback = Box::new(|| None);
        let listener_for_wakeup = event_listener.clone();
        let on_wakeup: dragent_pty::WakeupCallback =
            Box::new(move || listener_for_wakeup.send_wakeup());

        let (manager, pty_rx) = match backend {
            // Shared-session mirror: no local shell. Output is fed via
            // feed_remote_output; input forwards to the host through `sink`. A
            // never-firing channel stands in for the PTY lifecycle receiver.
            TerminalBackend::Remote { sink } => {
                let (_tx, rx) = async_channel::unbounded::<dragent_pty::PtyEvent>();
                (TerminalManager::remote(sink), rx)
            }
            // Host-owned tab: no local shell either. Same feed path as a
            // mirror, but the view keeps LOCAL layout/input behavior
            // (`is_remote` false) and input/resize route to the daemon's
            // PTY through the binding slot.
            TerminalBackend::Host { binding } => {
                let (_tx, rx) = async_channel::unbounded::<dragent_pty::PtyEvent>();
                (TerminalManager::host(binding), rx)
            }
            // Web viewer: there is no local PTY. A no-op input sink + never-firing
            // lifecycle channel stand in so the terminal still renders (output
            // arrives via the session mirror). Must come before the native spawn
            // path — on wasm the fallback's `std::env::temp_dir()` panics ("no
            // filesystem") and the PTY `.expect()` would abort the whole module.
            #[cfg(all(target_family = "wasm", any(test, feature = "performance-harness")))]
            TerminalBackend::Local { .. } => {
                let (_tx, rx) = async_channel::unbounded::<dragent_pty::PtyEvent>();
                let noop_sink: crate::terminal::manager::pty_writer::RemoteInputSink =
                    std::sync::Arc::new(|_: &[u8]| {});
                (TerminalManager::remote(noop_sink), rx)
            }
            #[cfg(all(
                not(target_family = "wasm"),
                any(test, feature = "performance-harness")
            ))]
            TerminalBackend::Local { .. } => {
                TerminalManager::spawn(shell, &cwd, feed, sync_buffer_len, on_wakeup)
                    .unwrap_or_else(|e| {
                        log::error!("Failed to spawn shell: {e:?}");
                        model.update(ctx, |m, mctx| {
                            m.set_link_status(ShellLinkStatus::Error, mctx);
                        });
                        let empty_feed: dragent_pty::FeedCallback =
                            Box::new(|_| dragent_pty::PtyFeedStats { bytes_processed: 0 });
                        let sync_len: dragent_pty::SyncBufferLenCallback = Box::new(|| None);
                        let noop_wakeup: dragent_pty::WakeupCallback = Box::new(|| {});
                        TerminalManager::spawn(
                            None,
                            std::env::temp_dir(),
                            empty_feed,
                            sync_len,
                            noop_wakeup,
                        )
                        .expect("fallback PTY spawn")
                    })
            }
        };
        // E2E automation (env-gated, first session only): drives this session
        // through the real PTY + hooks + session core. See app/src/automation.rs.
        // Local shells only: mirrors and host-owned tabs have no local PTY.
        if drive_automation {
            crate::automation::maybe_start(session_core.clone(), manager.writer().clone());
        }
        let command_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Ask AI or run a command",
                mono_family,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::TerminalCommand,
                editor_ctx.focus_handle(),
            )
        });
        let rich_input_editor = ctx.add_typed_action_view(|editor_ctx| {
            // The rich input is an AI-tool prompt (Claude Code, cursor-agent, …),
            // never a shell command — so it suppresses shell syntax highlighting.
            // It uses the ACTIVE theme's colors so the text follows light/dark:
            // dark text on a light (image) background, light text on a dark one.
            let mut editor = InputEditor::new(
                "Enter your prompt, or ! to run a shell command...",
                mono_family,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::TerminalCommand,
                editor_ctx.focus_handle(),
            );
            editor.set_suppress_command_highlight(true);
            editor.enable_native_slash_commands();
            editor
        });
        let feedback_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Provide feedback to agent...",
                mono_family,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "agent-approval-feedback".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let question_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Type your own answer…",
                mono_family,
                crate::theme::text_size(INPUT_FONT_SIZE),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "agent-question-answer".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let block_filter_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Filter output…",
                mono_family,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                // Plain single-line: emits `Changed` so the filter query syncs
                // live, and propagates Enter/Escape to the pane (Search mode does
                // neither, which is why the query never reached the filter).
                InputMode::Plain {
                    key: "block_filter".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let branch_diff_commit_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Commit message…",
                mono_family,
                crate::theme::text_size(12.5),
                theme.main_text(),
                theme.sub_text(),
                theme.accent,
                theme,
                InputMode::MultiLine {
                    key: "branch_diff_commit_msg".to_string(),
                    max_height: 80,
                },
                editor_ctx.focus_handle(),
            )
        });
        // Plain (not Search) so Enter propagates to the pane handler, which is
        // how "type a name, press Enter" creates the branch.
        let new_branch_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "New branch name…",
                mono_family,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "new_branch_name".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let model_search_editor = ctx.add_typed_action_view(|editor_ctx| {
            InputEditor::new(
                "Search models…",
                mono_family,
                crate::theme::text_size(12.0),
                theme.main_text(),
                theme.disabled_text(),
                theme.accent,
                theme,
                InputMode::Plain {
                    key: "model_search".to_string(),
                },
                editor_ctx.focus_handle(),
            )
        });
        let git_repo_status =
            ctx.add_model(|mctx| crate::git_status::GitRepoStatusModel::new(cwd.clone(), mctx));
        let path_picker = ctx.add_model(PathPicker::new);
        let project_root =
            crate::agent::project_context::ProjectContext::build(&cwd, None, "").project_root;
        let allowed_roots =
            crate::agent::permissions::AllowedRoots::for_session(&cwd, project_root.as_deref());
        let agent_controller = {
            let api = crate::agent::default_agent_api();
            let cwd_for_ctrl = cwd.clone();
            ctx.add_model(move |ctx| {
                let mut ctrl = crate::agent::AgentController::with_allowed_roots(
                    api,
                    allowed_roots.clone(),
                    cwd_for_ctrl,
                );
                ctrl.start_background_watcher(ctx);
                ctrl
            })
        };
        let allowed_roots_sh =
            crate::agent::permissions::AllowedRoots::for_session(&cwd, project_root.as_deref());
        let self_heal_store = {
            let api = crate::agent::default_agent_api();
            ctx.add_model(|_| {
                crate::agent::self_heal::SelfHealStore::with_allowed_roots(api, allowed_roots_sh)
            })
        };
        let editor_settings = EditorSettings::load();
        let vim_enabled = editor_settings.vim_mode_enabled;
        let autodetect_enabled = editor_settings.input_autodetection_enabled;

        model.update(ctx, |m, mctx| {
            m.set_link_status(ShellLinkStatus::Running, mctx);
        });
        manager.writer().set_terminal_colors(terminal_colors(&theme));

        ctx.subscribe_to_model(&model, move |view, _, event, ctx| {
            if let TerminalEvent::PtyWrite(bytes) = event {
                // A shared-session mirror re-parses the HOST's output, and its
                // emulator auto-generates replies to queries in that stream
                // (cursor-position/device-attribute/mouse reports). On a local
                // terminal those go back to the PTY, but on a mirror the writer is
                // a remote sink → they'd be forwarded to the HOST's shell as if
                // the viewer typed them (`zsh: command not found: 31`, …). Only the
                // host answers its own queries; the mirror must stay silent.
                if !view.is_remote {
                    view.manager.writer().write(bytes);
                }
            }
            view.on_model_event(event, ctx);
        });
        // Real-time git chips: a filesystem watcher on the repo pushes the
        // changed paths into this channel; each batch refreshes git status
        // scoped to those paths (no throttle — the watcher already
        // debounces, and scoped fetches stay cheap). Capacity absorbs
        // bursts; the model coalesces anything arriving mid-fetch.
        let (git_watch_tx, git_watch_rx) = async_channel::bounded::<Vec<PathBuf>>(16);
        ctx.spawn_stream_local(
            git_watch_rx,
            |view, paths: Vec<PathBuf>, ctx| {
                // A stopped watcher may already have queued a notification.
                // Hidden panes do not render Git state, so discard it instead
                // of starting duplicate Git subprocesses in background tabs.
                if !view.workspace_active {
                    return;
                }
                log::info!(
                    "[git-watch] file change → refreshing git status ({} paths)",
                    paths.len()
                );
                let cwd = view.model.read(ctx, |m, _| m.cwd().to_path_buf());
                ctx.update_model(&view.git_repo_status, |model, mctx| {
                    model.refresh_scoped(cwd, paths, mctx);
                });
            },
            |_, _| {},
        );

        // A phone taking or leaving a terminal changes this pane's cover.
        ctx.spawn_stream_local(
            crate::realtime::phone_control::subscribe(),
            |_view, _unit: (), ctx| ctx.notify(),
            |_, _| {},
        );

        // Running commands live outside the UI model, so their registry emits
        // an explicit per-pane wakeup. This makes the activity strip appear and
        // disappear immediately without relying on subsequent model/tool calls.
        let initial_pane_uuid = Uuid::new_v4();
        // A phone's prompts and answers for this pane's built-in agent.
        ctx.spawn_stream_local(
            crate::realtime::phone_agent::register(initial_pane_uuid),
            |view, command, ctx| view.handle_phone_agent_command(command, ctx),
            |_, _| {},
        );
        let (agent_activity_tx, agent_activity_rx) = async_channel::bounded::<()>(8);
        crate::agent::local_tools::register_activity_sender(initial_pane_uuid, agent_activity_tx);
        ctx.spawn_stream_local(
            agent_activity_rx,
            |view, _unit: (), ctx| {
                view.arm_agent_ticker(ctx);
                ctx.notify();
            },
            |_, _| {},
        );

        ctx.subscribe_to_model(&git_repo_status, |view, _, event, ctx| {
            use crate::git_status::GitRepoStatusEvent;
            // Keep branch_diff_staged in sync with the actual git index so that
            // files staged outside the app (or before the panel was opened) are
            // automatically reflected without requiring the user to re-stage them.
            if let GitRepoStatusEvent::Updated {
                status: Some(ref status),
                ..
            } = event
            {
                view.branch_diff_staged = status.staged_rel_paths.iter().cloned().collect();
            }
            ctx.notify();
        });
        // Repaint when account data (the model list for the input-bar model
        // picker) arrives — the fetch is kicked lazily when the picker opens.
        {
            use warpui::SingletonEntity;
            let account = crate::settings::AccountStore::handle(ctx);
            ctx.subscribe_to_model(&account, |_, _, _, ctx| {
                ctx.notify();
            });
        }
        ctx.subscribe_to_model(&agent_controller, |view, handle, event, ctx| {
            use crate::agent::AgentControllerEvent;
            view.arm_agent_ticker(ctx);
            view.notify_agent_moments(&handle, event, ctx);
            // New agent edits re-surface a dismissed Agent Changes card.
            let file_count = ctx.read_model(&handle, |c: &crate::agent::AgentController, _| {
                c.agent_modified_files.len()
            });
            if file_count != view.last_agent_file_count {
                if file_count > view.last_agent_file_count {
                    view.git_diff_dismissed = false;
                }
                view.last_agent_file_count = file_count;
            }
            match event {
                AgentControllerEvent::BlocksChanged => {
                    view.phone_agent_changed(ctx);
                    view.rebuild_height_index(ctx);
                    ctx.notify();
                    view.follow_output_if_auto_scroll(ctx);
                    let block_count = ctx.read_model(
                        &handle,
                        |c: &crate::agent::AgentController, _| {
                            crate::agent::agent_share::agent_block_count(c.agent_blocks())
                        },
                    );
                    if block_count != view.last_shared_agent_block_count {
                        view.last_shared_agent_block_count = block_count;
                        view.broadcast_shared_agent_blocks(ctx);
                    } else {
                        view.broadcast_shared_agent_blocks_throttled(ctx);
                    }
                }
                AgentControllerEvent::ApprovalResolved { .. } => {
                    ctx.notify();
                }
                AgentControllerEvent::Error { message } => {
                    log::warn!("[agent] {message}");
                    ctx.notify();
                }
                AgentControllerEvent::BudgetUpdated { turns, input_tokens, output_tokens } => {
                    log::debug!(
                        "[agent] budget: {turns} turns, {input_tokens} in, {output_tokens} out tokens"
                    );
                    // Invalidate usage after every agent response so the account page stays current.
                    use warpui::SingletonEntity;
                    let account = crate::settings::AccountStore::handle(ctx);
                    ctx.update_model(&account, |store, mctx| store.fetch(mctx));
                    ctx.notify();
                }
                AgentControllerEvent::Agent(_) => {}
                // Embedded hosts consume this; the terminal already reacts to
                // BlocksChanged / BudgetUpdated for the same moment.
                AgentControllerEvent::TurnCompleted { .. }
                | AgentControllerEvent::ConsentRequested { .. }
                | AgentControllerEvent::QuestionOpened { .. }
                | AgentControllerEvent::QuestionAnswered { .. }
                | AgentControllerEvent::HostToolCall { .. } => {}
                AgentControllerEvent::RunCommandInTerminal {
                    task_id,
                    call_id,
                    command,
                    args_json,
                    allow_terminal_cwd_change,
                } => {
                    // Compatibility with older controller events: never lend
                    // the visible shell to an agent command.
                    let _ = (command, allow_terminal_cwd_change);
                    let task_id = task_id.clone();
                    let call_id = call_id.clone();
                    let args_json = args_json.clone();
                    let pane_uuid = view.pane_uuid;
                    crate::bg::detach(move || {
                        let (output, is_error) = crate::agent::local_tools::execute_with_call_id(
                            &call_id, "run_command", &args_json, pane_uuid,
                        );
                        if let Err(error) = crate::agent::pipeline::post_tool_result(
                            &task_id, &call_id, &output, is_error,
                        ) {
                            log::warn!("[agent-stream] post command result {call_id}: {error}");
                        }
                    });
                }

                AgentControllerEvent::SessionExpired => {
                    log::warn!("[agent] session expired — logging out");
                    use warpui::SingletonEntity;
                    let auth = crate::auth::AuthManager::handle(ctx);
                    ctx.update_model(&auth, |a: &mut crate::auth::AuthManager, auth_ctx| a.log_out(auth_ctx));
                    ctx.notify();
                }
            }
        });

        // Env-gated agent automation bridge (first pane only): lets the E2E
        // server submit agent queries, observe controller state, and resolve
        // approval prompts headlessly. No-op without DRAGENT_AUTOMATION_PORT.
        if let Some(bridge) = crate::automation::claim_agent_bridge() {
            let state_sink = bridge.state.clone();
            ctx.subscribe_to_model(&agent_controller, move |view, _, _event, ctx| {
                let snap = view.agent_controller.read(ctx, |c, _| {
                    serde_json::json!({
                        "streaming": c.is_streaming(),
                        "pending_approval": c
                            .frontmost_pending_approval()
                            .map(|u| u.to_string()),
                        "blocks": serde_json::to_value(c.agent_blocks())
                            .unwrap_or_default(),
                    })
                });
                if let Ok(mut guard) = state_sink.lock() {
                    *guard = snap;
                }
            });
            ctx.spawn_stream_local(
                bridge.query_rx,
                |view, query: String, ctx| {
                    log::info!("[automation] agent query: {query}");
                    view.submit_agent_query(query, ctx);
                },
                |_, _| {},
            );
            ctx.spawn_stream_local(
                bridge.sync_rx,
                |view, _unit: (), ctx| {
                    log::info!("[automation] sync_editor_settings requested");
                    view.sync_editor_settings(ctx);
                },
                |_, _| {},
            );
            // Named input-bar UI actions for E2E — drives the same handlers the
            // mouse does, so screenshots exercise the real render paths without
            // synthetic clicks (which race the user's own pointer).
            ctx.spawn_stream_local(
                bridge.ui_rx,
                |view, action: String, ctx| {
                    log::info!("[automation] ui action: {action}");
                    match action.as_str() {
                        "agent_input" => {
                            view.input_config.input_type = DetectedInputType::Agent;
                            view.input_config.is_locked = true;
                            view.sync_input_placeholder(ctx);
                            ctx.notify();
                        }
                        "toggle_mode_menu" => {
                            view.handle_action(&TerminalAction::ToggleModeMenu, ctx)
                        }
                        "toggle_model_menu" => {
                            view.handle_action(&TerminalAction::ToggleModelMenu, ctx)
                        }
                        "source_control" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::SetProjectSidebarTab {
                                tab: crate::workspace::project_sidebar::SidebarTab::SourceControl,
                            },
                        ),
                        "open_worktree_diff" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::OpenGitWorktreeDiff,
                        ),
                        "toggle_document_terminal" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::ToggleDocumentTerminal,
                        ),
                        "layout_shape" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::SelectWorkspaceLayout(
                                dragent_workspace_model::BuiltInLayout::Shape,
                            ),
                        ),
                        "layout_coordinate" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::SelectWorkspaceLayout(
                                dragent_workspace_model::BuiltInLayout::Coordinate,
                            ),
                        ),
                        "layout_develop" => ctx.dispatch_typed_action_deferred(
                            crate::workspace::WorkspaceAction::SelectWorkspaceLayout(
                                dragent_workspace_model::BuiltInLayout::Develop,
                            ),
                        ),
                        other => {
                            if let Some(relative_path) = other.strip_prefix("open_working_diff:") {
                                ctx.dispatch_typed_action_deferred(
                                    crate::workspace::WorkspaceAction::OpenGitDiff {
                                        rel_path: relative_path.to_string(),
                                        staged: false,
                                    },
                                );
                            } else {
                                log::warn!("[automation] unknown ui action: {other}");
                            }
                        }
                    }
                },
                |_, _| {},
            );
            ctx.spawn_stream_local(
                bridge.auto_fix_rx,
                |view, _unit: (), ctx| {
                    // Newest failed shell block → autonomous fix loop.
                    let target = view.model.read(ctx, |m, _| {
                        m.with_core(|core| {
                            core.blocks()
                                .blocks()
                                .iter()
                                .rev()
                                .find(|b| b.completion.as_ref().map(|c| c.failed).unwrap_or(false))
                                .map(|b| b.id)
                        })
                    });
                    if let Some(block_id) = target {
                        log::info!("[automation] auto_fix_last → block {block_id}");
                        view.handle_action(&TerminalAction::AutoFixBlock { block_id }, ctx);
                    } else {
                        log::warn!("[automation] auto_fix_last: no failed block found");
                    }
                },
                |_, _| {},
            );
            ctx.spawn_stream_local(
                bridge.resolve_rx,
                |view, approve: bool, ctx| {
                    let pending = view
                        .agent_controller
                        .read(ctx, |c, _| c.frontmost_pending_approval());
                    let Some(action_id) = pending else {
                        log::warn!("[automation] resolve_approval with nothing pending");
                        return;
                    };
                    log::info!("[automation] resolving approval {action_id} approve={approve}");
                    ctx.update_model(&view.agent_controller, move |c, mctx| {
                        c.resolve_streaming_approval(action_id, approve, None, mctx);
                    });
                },
                |_, _| {},
            );
        }

        ctx.subscribe_to_model(&self_heal_store, |view, _, event, ctx| {
            use crate::agent::self_heal::SelfHealEvent;
            use warpui::SingletonEntity;
            match event {
                SelfHealEvent::BlocksChanged => {
                    // Observability: log store state vs what the auto-fix
                    // gate will actually display.
                    let auto_fix = crate::settings::AppSettings::global()
                        .agent
                        .auto_fix_on_failure;
                    let states = view
                        .self_heal_store
                        .read(ctx, |store, _| store.all_states().clone());
                    for (bid, state) in &states {
                        let display = state.projected_for_display(
                            auto_fix,
                            view.auto_analyzed_blocks.contains(bid),
                            view.revealed_fix_blocks.contains(bid),
                        );
                        log::info!(
                            "[self-heal] block {bid}: store={} display={} (auto_fix={auto_fix})",
                            state.tag(),
                            display.tag(),
                        );
                    }
                    view.rebuild_height_index(ctx);
                    ctx.notify();
                }
                SelfHealEvent::Agent(agent_event) => {
                    log::info!("[self-heal] {:?}", agent_event);
                }
                SelfHealEvent::TokenRotated {
                    refresh_token,
                    access_token,
                    expires_in,
                } => {
                    let auth = crate::auth::AuthManager::handle(ctx);
                    let rt = refresh_token.clone();
                    let at = access_token.clone();
                    let exp = *expires_in;
                    ctx.update_model(&auth, move |a, _| {
                        a.update_refresh_token(rt);
                        a.cache_access_token(at, exp);
                    });
                }
            }
        });
        {
            use warpui::SingletonEntity;
            let session_manager = crate::realtime::manager::SessionManager::handle(ctx);
            ctx.subscribe_to_model(&session_manager, |view, _, event, ctx| {
                use crate::realtime::manager::SessionManagerEvent;
                // A new participant joined the shared session: replay this pane's
                // full current state so the late joiner isn't looking at a blank
                // terminal. No-op unless this pane is the one being shared.
                if matches!(event, SessionManagerEvent::ResyncViewers) {
                    view.resync_shared_state(ctx);
                }
            });
        }
        ctx.observe(&path_picker, |_this, _handle, ctx| ctx.notify());
        ctx.subscribe_to_view(&command_editor, |view, _, event, ctx| {
            view.on_command_editor_event(event, ctx);
        });
        ctx.subscribe_to_view(&rich_input_editor, |view, _, event, ctx| {
            view.on_rich_input_editor_event(event, ctx);
        });
        ctx.subscribe_to_view(&model_search_editor, |view, _, event, ctx| {
            if let InputEditorEvent::Changed { text, .. } = event {
                view.model_search = text.clone();
                ctx.notify();
            }
        });
        ctx.subscribe_to_view(&block_filter_editor, |view, _, event, ctx| {
            view.on_block_filter_editor_event(event, ctx);
        });
        if vim_enabled {
            ctx.update_view(&command_editor, |editor, ectx| {
                editor.set_vim_enabled(true, ectx);
            });
        }

        // While a foreground command runs the input bar is collapsed but can
        // still hold focus — this flag makes the editor propagate every key so
        // the pane forwards them to the PTY (raw-mode prompts need them).
        let command_input_passthrough =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = command_input_passthrough.clone();
            ctx.update_view(&command_editor, move |editor, _| {
                editor.set_passthrough_flag(flag);
            });
        }

        // Output wakeups, lifecycle, sizes and heights are polled on the main
        // thread: a background-polled stream waits for a free background
        // thread, and slow git work once held all of them, freezing every pane.
        ctx.spawn_stream_on_main_thread(
            event_listener.wakeup_receiver(),
            move |view, _, ctx| {
                view.handle_terminal_wakeup(ctx);
            },
            |_, _| {},
        );

        ctx.spawn_stream_on_main_thread(
            pty_rx,
            move |view, event, ctx| {
                view.on_pty_lifecycle_event(event, ctx);
            },
            |_, _| {},
        );

        let (resize_tx, resize_rx) = async_channel::unbounded::<Vector2F>();
        let pending_sizes = resize_rx.clone();
        ctx.spawn_stream_on_main_thread(
            resize_rx,
            move |view, mut size, ctx| {
                while let Ok(latest) = pending_sizes.try_recv() {
                    size = latest;
                }
                view.after_terminal_view_layout(size, ctx);
            },
            |_, _| {},
        );

        let (heights_tx, heights_rx) = async_channel::unbounded::<()>();
        ctx.spawn_stream_on_main_thread(
            heights_rx,
            move |view, _, ctx| {
                view.rebuild_height_index(ctx);
                ctx.notify();
            },
            |_, _| {},
        );

        ctx.spawn(
            Timer::after(Duration::from_millis(0)),
            move |view, _, mut ctx| {
                view.focus_command_editor(&mut ctx);
            },
        );

        let mut view = Self {
            launch_shell: preferred_shell,
            theme,
            mono_family,
            cell_metrics,
            model,
            manager,
            session_core,
            is_remote,
            // A shared-session mirror starts view-only; the host grants control
            // explicitly. A local terminal is never read-only.
            remote_read_only: is_remote,
            links_open_in_system_browser: false,
            pending_shell_setup: None,
            remote_ai_tool: None,
            remote_is_host_account: false,
            remote_rich_input_closed: false,
            remote_alt_h_scroll: ClippedScrollStateHandle::new(),
            alt_pan_drag: Arc::new(Mutex::new(None)),
            last_local_input_edit: Instant::now(),
            remote_dims: None,
            remote_requested_dims: None,
            resize_tx,
            heights_tx,
            pane_has_focus: false,
            shift_held: false,
            ctrl_held: false,
            alt_held: false,
            cmd_held: false,
            drag_selecting: false,
            alt_left_gesture: None,
            alt_press: None,
            last_mouse_pos: Vector2F::zero(),
            last_dims: None,
            last_alt_active: false,
            last_rich_input_active: false,
            block_scroll_state: ClippedScrollStateHandle::new(),
            agent_transcript_scroll: ClippedScrollStateHandle::new(),
            agent_transcript_follow: std::cell::Cell::new(true),
            agent_transcript_last_scroll: std::cell::Cell::new(0.0),
            block_scroll_pos: Arc::new(Mutex::new(ScrollPosition::follow_bottom())),
            block_scroll_handle: Arc::new(Mutex::new(Default::default())),
            block_height_index: BlockHeightIndex::new(),
            auto_scroll: true, // Default to parsing follow mode
            command_input_passthrough,
            command_editor,
            rich_input_editor,
            feedback_editor,
            question_editor,
            block_filter_editor,
            command_history: Vec::new(),
            rich_input_history_idx: None,
            command_history_idx: None,
            history_recall_prefix: None,
            previous_command: None,
            shell_aliases: ShellAliases::load(),
            suggest_engine: SuggestEngine::new(),
            git_repo_status,
            current_running_command: None,
            rich_input_active: false,
            rich_input_proxy_chars: 0,
            pane_view_id: ctx.view_id(),
            window_id: ctx.window_id(),
            output_viewport_width_px: 760.0,
            output_viewport_height_px: 480.0,
            entry_hint: None,
            link_hint: None,
            entry_mouse_states: RefCell::new(HashMap::new()),
            shimmer_states: RefCell::new(HashMap::new()),
            diagram_states: RefCell::new(HashMap::new()),
            pending_attachments: Vec::new(),
            side_shell: None,
            side_card_open: false,
            side_poll_armed: false,
            side_cmd_history: Vec::new(),
            side_card_height_px: 220.0,
            side_card_drag: warpui_core::elements::drag_resize_handle(),
            side_scroll: ClippedScrollStateHandle::new(),
            side_selectable: SelectionHandle::default(),
            side_follow_tail: true,
            side_activity_seen: 0,
            // In the past so the Stop button isn't shown until real output arrives.
            side_activity_at: web_time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(60))
                .unwrap_or_else(web_time::Instant::now),
            voice_ui: Default::default(),
            voice_gen: 0,
            voice_spin: 0,
            voice_session: None,
            voice_fold: Default::default(),
            voice_pending_confirm: None,
            voice_rebase_pending: false,
            input_notice: None,
            host_notice_active: false,
            shell_starting_notice: false,
            host_error_notice: None,
            host_recovery_actions: Vec::new(),
            host_restart_command: None,
            image_viewer: None,
            diagram_viewer: None,
            diagram_viewer_zoom: 1.0,
            diagram_horizontal_scroll: ClippedScrollStateHandle::new(),
            diagram_drag_last: Arc::new(Mutex::new(None)),
            block_selectable_handle: SelectionHandle::default(),
            block_selected_text: Arc::new(RwLock::new(None)),
            path_picker,
            history_search: HistorySearchState::default(),
            cmd_hist_open: false,
            cmd_hist_items: Vec::new(),
            cmd_hist_selected: 0,
            cmd_hist_saved: String::new(),
            cmd_hist_previous_input_config: None,
            cmd_hist_rich: false,
            cmd_hist_height_px: 240.0,
            cmd_hist_drag: warpui_core::elements::drag_resize_handle(),
            editor_settings,
            workflow_store: WorkflowStore::load(),
            workflow_search: WorkflowSearchState::default(),
            workflow_session: None,
            workflow_scroll_state: ClippedScrollStateHandle::new(),
            completion_scroll_state: ClippedScrollStateHandle::new(),
            dir_sort: DirSort::default(),
            dir_view: DirView::default(),
            pane_uuid: initial_pane_uuid,
            event_listener,
            sticky_header: None,
            sticky_headers_enabled: true,
            docked_backdrop_enabled: false,
            open_block_menu: None,
            block_menu_anchor: None,
            copy_confirmation: Default::default(),
            open_link_menu: None,
            link_menu_anchor: None,
            hovered_link: None,
            alt_hovered_link: None,
            dir_hover: Default::default(),
            open_checkpoint_preview: None,
            block_filter: None,
            block_filter_match_index: 0,
            sticky_push_offset: 0.0,
            measured_block_heights: Arc::new(Mutex::new(HashMap::new())),
            measured_agent_block_heights: Arc::new(Mutex::new(HashMap::new())),
            glyph_cache: Arc::new(Mutex::new(CellGlyphCache::new())),
            block_text_cache: Arc::new(Mutex::new(Default::default())),
            live_prompt_height_px: 0.0,
            input_config: InputConfig::for_tab_kind(TabSessionKind::Shell, autodetect_enabled),
            chat_mode: crate::agent::chat_mode::ChatMode::default(),
            mode_menu_open: false,
            model_menu_open: false,
            agent_model_override: None,
            slash_palette_selected: 0,
            slash_scroll_state: ClippedScrollStateHandle::new(),
            history_panel_open: false,
            chat_open: std::collections::HashSet::new(),
            cli_chat_on: false,
            cli_chat: None,
            cli_chat_generation: 0,
            cli_chat_sent: None,
            acp_chat: None,
            chat_window: super::terminal_cli_chat::CHAT_WINDOW,
            cli_chat_model_pick: None,
            cli_chat_effort_pick: None,
            effort_drag: None,
            effort_drag_moves: 0,
            conversation_summaries: Vec::new(),
            tab_session_kind: TabSessionKind::Shell,
            // Bound at launch (rail, composer, agent tab) or on restore;
            // plain shells keep `None` and fall back to detection.
            agent_id: None,
            classify_generation: 0,
            last_submit_was_agent: false,
            automation_submit: false,
            agent_turn_busy: false,
            automation_service_writes: false,
            agent_api: crate::agent::default_agent_api(),
            agent_controller,
            self_heal_store,
            auto_analyzed_blocks: std::collections::HashSet::new(),
            revealed_fix_blocks: std::collections::HashSet::new(),
            agent_ticker_armed: false,
            pending_confirm: None,
            showing_close_confirm: false,
            git_watch_tx,
            git_watch_cwd: None,
            git_watch_stop: None,
            agent_roots_sync_generation: 0,
            agent_roots_sync_key: None,
            alt_cursor_blink_on: true,
            alt_blink_timer_armed: false,
            attached_blocks: Vec::new(),
            last_height_rebuild_at: None,
            output_rebuild_pending: false,
            workspace_active: true,
            pending_agent_run_command: None,
            git_diff_dismissed: false,
            last_agent_file_count: 0,
            last_shared_agent_block_count: 0,
            phone_agent_published_at: None,
            phone_agent_timer: false,
            branch_diff_expanded: false,
            branch_diff_staged: std::collections::HashSet::new(),
            branch_diff_scroll: ClippedScrollStateHandle::new(),
            checkpoint_preview_scroll: ClippedScrollStateHandle::new(),
            branch_diff_commit_editor,
            branch_diff_op: BranchDiffOp::None,
            error_toast: None,
            branch_diff_error_scroll: ClippedScrollStateHandle::new(),
            commit_error_copied: false,
            branch_menu_open: false,
            branch_list: None,
            new_branch_editor,
            folder_menu_open: false,
            folder_list: None,
            branch_scroll: ClippedScrollStateHandle::new(),
            folder_scroll: ClippedScrollStateHandle::new(),
            model_search_editor,
            model_search: String::new(),
            model_scroll: ClippedScrollStateHandle::new(),
        };
        crate::terminal::output_activity::register(
            view.pane_uuid,
            std::sync::Arc::downgrade(&view.session_core),
        );
        view.rebuild_height_index(ctx);
        view
    }
}

impl TerminalView {
    #[cfg(any(test, feature = "performance-harness"))]
    pub fn new_with_restore(
        cwd: PathBuf,
        pane_uuid: Uuid,
        shell_title: Option<String>,
        clear_gap_height_px: f32,
        clear_gap_before_block_id: Option<i32>,
        clear_gap_after_all: bool,
        restored_blocks: Vec<PersistedBlock>,
        restored_agent_blocks: Vec<dragent_persistence::PersistedAgentBlock>,
        preferred_shell: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let mut view = Self::new(cwd.clone(), preferred_shell, ctx);
        view.restore_persisted_state(
            cwd,
            pane_uuid,
            shell_title,
            clear_gap_height_px,
            clear_gap_before_block_id,
            clear_gap_after_all,
            restored_blocks,
            restored_agent_blocks,
            ctx,
        );
        view
    }

    pub(crate) fn restore_persisted_state(
        &mut self,
        cwd: PathBuf,
        pane_uuid: Uuid,
        shell_title: Option<String>,
        clear_gap_height_px: f32,
        clear_gap_before_block_id: Option<i32>,
        clear_gap_after_all: bool,
        mut restored_blocks: Vec<PersistedBlock>,
        mut restored_agent_blocks: Vec<dragent_persistence::PersistedAgentBlock>,
        ctx: &mut ViewContext<Self>,
    ) {
        // Per-root restore: migrate legacy rows once, then prefer the
        // per-root file (None when empty so bulk-loaded state still applies).
        let org = dragent_persistence::current_org();
        if restored_blocks.is_empty() && restored_agent_blocks.is_empty() {
            if let Some((blocks, agent_blocks)) =
                dragent_persistence::restore_pane_blocks(&cwd, &org, pane_uuid)
            {
                restored_blocks = blocks;
                restored_agent_blocks = agent_blocks;
            }
        } else {
            let _ = dragent_persistence::migrate_pane_to_root(&cwd, &org, pane_uuid);
        }
        crate::agent::local_tools::rebind_activity_sender(self.pane_uuid, pane_uuid);
        crate::terminal::output_activity::rebind(self.pane_uuid, pane_uuid);
        crate::realtime::phone_agent::rebind(self.pane_uuid, pane_uuid);
        self.pane_uuid = pane_uuid;
        if self.manager.writer().is_host() {
            // Running state comes exclusively from the daemon checkpoint. A
            // persisted Running block cannot prove that a process survived.
            let running = dragent_persistence::PersistedRunStatus::Running;
            self.host_restart_command = restored_blocks
                .iter()
                .rev()
                .find(|block| block.status == running)
                .map(|block| block.command.clone());
            restored_blocks.retain(|block| block.status != running);
        }
        self.model.update(ctx, |model, model_ctx| {
            if let Some(title) = shell_title.filter(|t| !t.is_empty()) {
                model.set_restored_shell_title(title);
            }
            if !restored_blocks.is_empty() {
                let blocks = restored_blocks
                    .into_iter()
                    .enumerate()
                    .map(|(i, b)| crate::workspace::persistence::persisted_block_to_block(b, i))
                    .collect();
                model.restore_blocks(blocks, model_ctx);
            }
            if clear_gap_height_px > 0.0 {
                let before_id = clear_gap_before_block_id.map(|id| id as usize);
                model.restore_clear_gap(
                    clear_gap_height_px,
                    before_id,
                    clear_gap_after_all,
                    model_ctx,
                );
            }
            model_ctx.notify();
        });

        // Restore agent blocks from SQLite into the agent controller.
        if !restored_agent_blocks.is_empty() {
            // Older conversations for this pane can exist unloaded in SQLite
            // (only the latest is ever auto-restored) — the true pane-wide
            // max sort_order guards the renumbering below against colliding
            // with them. Fetch once, at startup, alongside the already-blocking
            // session load.
            let true_max_sort_order = (|| -> anyhow::Result<Option<i32>> {
                let org = dragent_persistence::current_org();
                let _ = dragent_persistence::migrate_pane_to_root(&cwd, &org, pane_uuid);
                let path = dragent_persistence::db_path_for_read(&cwd)?;
                let mut conn = dragent_persistence::setup_connection(&path)?;
                dragent_persistence::max_sort_order_for_pane(&mut conn, pane_uuid)
            })()
            .unwrap_or_default();
            ctx.update_model(&self.agent_controller, |ctrl, mctx| {
                ctrl.restore_blocks_from_sql(restored_agent_blocks, true_max_sort_order);
                mctx.emit(crate::agent::AgentControllerEvent::BlocksChanged);
            });
        }

        self.rebuild_height_index(ctx);
    }

    /// Wire up SQLite persistence for agent blocks produced by this pane.
    pub fn set_agent_persistence(
        &mut self,
        pane_uuid: Uuid,
        sender: std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>,
        ctx: &mut ViewContext<Self>,
    ) {
        ctx.update_model(&self.agent_controller, |ctrl, _| {
            ctrl.set_persistence(pane_uuid, sender);
        });
    }

    pub fn active_clear_gap<C: ReadModel>(
        &self,
        ctx: &C,
    ) -> Option<crate::terminal::model::terminal_session_core::ActiveClearGap> {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| core.active_clear_gap().cloned())
        })
    }

    /// Persist the current clear-gap state to SQLite (via the persistence writer).
    /// Call this whenever terminal events change the gap height so it survives restart.
    pub fn persist_clear_gap_if_changed(
        &self,
        writer: &std::sync::mpsc::SyncSender<dragent_persistence::ModelEvent>,
        ctx: &mut ViewContext<Self>,
    ) {
        let gap = self.active_clear_gap(ctx);
        let (height_px, before_block_id, after_all) = match gap {
            Some(g) => (
                g.current_height_px,
                g.before_block_id.map(|id| id as i32),
                g.after_all,
            ),
            None => (0.0, None, false),
        };
        let _ = writer.send(dragent_persistence::ModelEvent::UpdateClearGap {
            pane_uuid: self.pane_uuid,
            height_px,
            before_block_id,
            after_all,
        });
    }

    pub fn pane_uuid(&self) -> Uuid {
        self.pane_uuid
    }

    /// Feed broadcast output from a shared session into this MIRROR view's core,
    /// exactly as if it came from a local PTY — so the same block/alt-screen
    /// rendering runs and the mirror looks identical to the host. Then drain the
    /// resulting effects to repaint.
    /// Show (or clear, with `None`) the host-connection notice under the
    /// input. Other notices are never overwritten by a clear.
    pub fn set_host_notice(&mut self, notice: Option<String>, ctx: &mut ViewContext<Self>) {
        match notice {
            Some(text) => {
                self.shell_starting_notice = false;
                self.input_notice = Some(text);
                self.host_notice_active = true;
            }
            // Clearing the connection notice (every streamed output batch does)
            // must not lift the starting notice: submit stays blocked until the
            // shell's first prompt, or early input queues and runs later.
            None if self.shell_starting_notice
                && self.model.read(ctx, |m, _| m.host_shell_starting().is_some()) =>
            {
                return;
            }
            None if self.host_notice_active => {
                self.shell_starting_notice = false;
                self.input_notice = None;
                self.host_notice_active = false;
                self.host_error_notice = None;
                self.host_recovery_actions.clear();
            }
            None => return,
        }
        ctx.notify();
    }

    pub(crate) fn set_host_error_notice(&mut self, notice: String, ctx: &mut ViewContext<Self>) {
        if self.host_error_notice.as_ref() != Some(&notice) {
            self.show_error_notice(
                ("Terminal connection failed".to_string(), notice.clone()),
                ctx,
            );
            self.host_error_notice = Some(notice);
        }
        self.shell_starting_notice = false;
        self.input_notice = None;
        self.host_notice_active = true;
        ctx.notify();
    }

    /// Show the coloured "Starting <shell>…" line while the host shell is
    /// still starting (submit is blocked meanwhile, as for any host notice),
    /// and clear it at its first prompt.
    pub(crate) fn sync_shell_starting_notice(&mut self, ctx: &mut ViewContext<Self>) {
        match self.model.read(ctx, |m, _| m.host_shell_starting()) {
            Some(shell) if !self.shell_starting_notice => {
                let name = shell_display_name(&shell);
                self.set_host_notice(Some(format!("Starting {name}…")), ctx);
                self.shell_starting_notice = true;
                ctx.spawn(
                    warpui::r#async::Timer::after(Duration::from_secs(10)),
                    move |view, _, ctx| {
                        let still = view.model.read(ctx, |m, _| m.host_shell_starting().is_some());
                        if view.shell_starting_notice && still {
                            view.input_notice = Some(format!(
                                "Starting {name}… it is slow to start on this machine. Input is held until it's ready."
                            ));
                            ctx.notify();
                        }
                    },
                );
            }
            None if self.shell_starting_notice => self.set_host_notice(None, ctx),
            _ => {}
        }
    }

    /// The pane's host session cannot be used. `actions` (empty while a
    /// replacement is starting) offer resuming or replacing it in this pane.
    pub fn set_host_unavailable(
        &mut self,
        notice: String,
        actions: Vec<super::terminal_view::HostRecoveryAction>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.set_host_error_notice(notice, ctx);
        self.host_recovery_actions = actions;
        ctx.notify();
    }

    /// Command running in this pane as last known: the daemon's checkpoint
    /// first, then the saved session.
    pub(crate) fn last_running_command(&self, app: &warpui::AppContext) -> Option<String> {
        self.model
            .read(app, |model, _| {
                model.with_core(|core| core.running_command())
            })
            .or_else(|| self.current_running_command.clone())
            .or_else(|| self.host_restart_command.clone())
    }

    /// A host-owned shell exited: same status a local PTY exit sets.
    pub fn mark_remote_exited(&mut self, code: i32, ctx: &mut ViewContext<Self>) {
        // Parsed output effects must be observed before the final lifecycle event.
        self.handle_terminal_wakeup(ctx);
        self.on_pty_lifecycle_event(dragent_pty::PtyEvent::ProcessExited(code), ctx);
    }

    /// Apply daemon-parsed frames on the stream worker. Each frame is bounded
    /// by the daemon, releasing the display lock between replay frames.
    pub(crate) fn host_output_sink(
        &self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> impl Fn(&dragent_host::TerminalChunk) -> bool + Send + 'static {
        let core = self.session_core.clone();
        let theme = self.theme.clone();
        let listener = self.event_listener.clone();
        move |chunk| {
            let waiting = std::time::Instant::now();
            let mut session = core.lock();
            crate::terminal::performance::FEED_LOCK_WAIT.record(waiting.elapsed());
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return false;
            }
            let started = std::time::Instant::now();
            if let Some(resume) = &chunk.resume {
                if let Err(error) = session.restore_host_resume(resume, &theme) {
                    log::error!("[host-tab] cannot restore terminal checkpoint: {error}");
                    return false;
                }
            }
            if let Some(operations) = chunk.operations.as_deref() {
                session.feed_operations(&chunk.data, operations, &theme);
            }
            crate::terminal::performance::HOST_FEED.record(started.elapsed());
            drop(session);
            listener.send_wakeup();
            true
        }
    }

    pub fn feed_remote_output(&mut self, bytes: &[u8], ctx: &mut ViewContext<Self>) {
        {
            let theme = self.theme.clone();
            self.session_core.lock().feed_pty_bytes(bytes, &theme);
        }
        self.handle_terminal_wakeup(ctx);
    }

    pub fn terminal_model(&self) -> ModelHandle<TerminalModel> {
        self.model.clone()
    }
    pub fn cwd<C: ReadModel>(&self, ctx: &C) -> PathBuf {
        self.model.read(ctx, |m, _| m.cwd().clone())
    }
    /// Whether a full-screen program owns the terminal (vim, htop, an AI TUI).
    /// The workspace hides the right sidebar while this is true.
    /// Ids of the blocks already in this terminal. Captured when a share starts
    /// so a "clean screen" share can mark them as already-sent (see
    /// `SessionManager::suppress_scrollback`) and never replay them.
    pub fn current_block_ids<C: ReadModel>(&self, ctx: &C) -> Vec<usize> {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| core.blocks().blocks().iter().map(|b| b.id).collect())
        })
    }

    pub fn is_alt_screen_active<C: ReadModel>(&self, ctx: &C) -> bool {
        self.model.read(ctx, |m, _| m.alt_screen_active())
    }
    pub fn shell_title<C: ReadModel>(&self, ctx: &C) -> Option<String> {
        self.model.read(ctx, |m, _| m.shell_title())
    }
    pub fn last_command<C: ReadModel>(&self, ctx: &C) -> Option<String> {
        self.model.read(ctx, |m, _| {
            m.with_core(|core| {
                core.blocks()
                    .blocks()
                    .last()
                    .map(|block| block.command.clone())
            })
        })
    }

    pub fn display_title<C: ReadModel>(&self, ctx: &C) -> String {
        self.model.read(ctx, |m, _| {
            if let Some(title) = m.shell_title() {
                if !title.is_empty() {
                    return title;
                }
            }
            m.cwd()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| DEFAULT_SESSION_TITLE.to_string())
        })
    }

    pub fn is_running<C: ReadModel>(&self, ctx: &C) -> bool {
        self.model.read(ctx, |m, _| m.is_command_running())
    }

    /// Keep the global foreground-command registry in step with this pane's
    /// live command state, so tab-close and app-quit guards can see it.
    pub(crate) fn sync_foreground_registry<C: ReadModel>(&self, ctx: &C) {
        let running = self.model.read(ctx, |m, _| m.is_command_running());
        let cmd = if running {
            self.current_running_command
                .clone()
                .or(Some("a command".to_string()))
        } else {
            None
        };
        crate::confirm::set_foreground_command(self.pane_uuid, cmd);
    }
    /// Tell a daemon-owned terminal the colours it should answer queries with.
    pub(crate) fn send_terminal_colors(&self) -> bool {
        self.manager
            .writer()
            .set_terminal_colors(terminal_colors(&self.theme))
    }

    pub fn set_theme(&mut self, theme: Theme, ctx: &mut ViewContext<Self>) {
        let scheme_flipped = self.theme.is_dark != theme.is_dark;
        let previous = self.theme;
        self.theme = theme;
        // Update the reply first: an app told about the change asks again for
        // the background and picks its own colours from the answer. A
        // daemon-owned terminal does both on its side.
        let announce = {
            let mut core = self.session_core.lock();
            if scheme_flipped {
                core.note_scheme_flipped_from(&previous);
            }
            core.set_reply_theme(theme);
            scheme_flipped && !self.is_remote && core.color_scheme_updates_enabled()
        };
        if !self.send_terminal_colors() && announce {
            self.manager
                .writer()
                .write(&dragent_terminal::color_scheme_report(theme.is_dark));
        }
        // `InputEditor::set_theme` takes its OWN ctx and must call notify() on
        // it — passing `_` here (as this used to) updates the editor's color
        // fields correctly but schedules no repaint, so the old colors stayed
        // on screen until some unrelated event (focus, hover, a keystroke)
        // forced that editor to redraw anyway.
        ctx.update_view(&self.command_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        // The rich input follows the ACTIVE theme now (its background reveals the
        // theme image, so its text must match: dark on a light background, light
        // on a dark one). set_theme resets text_color but not is_agent_mode, so
        // shell highlighting stays off.
        ctx.update_view(&self.rich_input_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        // These three were missing entirely before — each is its own
        // InputEditor with its own internally-cached theme, so switching
        // themes left them showing stale colors until something else forced
        // them to rebuild: the approval-feedback box, the block output filter
        // box, and the branch-diff commit message box.
        ctx.update_view(&self.feedback_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        ctx.update_view(&self.question_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        ctx.update_view(&self.block_filter_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        ctx.update_view(&self.branch_diff_commit_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        ctx.update_view(&self.new_branch_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        ctx.update_view(&self.model_search_editor, |editor, ectx| {
            editor.set_theme(theme, ectx)
        });
        // If this pane is shared, push the new theme id to viewers so their
        // mirror re-themes live (no-op when not hosting a session).
        self.broadcast_surface_state(ctx);
        ctx.notify();
    }
    pub fn active_ai_tool<C: ReadModel>(&self, ctx: &C) -> Option<AiTool> {
        self.active_ai_tool_inner(ctx)
    }

    /// True when this view is a shared-session mirror that currently HOLDS
    /// control (Edit) — i.e. a controlling viewer whose actions should drive the
    /// host desktop.
    pub(crate) fn is_remote_controller(&self) -> bool {
        self.is_remote && !self.remote_read_only
    }

    /// Controller → host: push a live input/scroll surface up so the host desktop
    /// reflects what this participant is doing. No-op unless this view is a
    /// controlling mirror (the core also role-gates it server-side).
    pub(crate) fn forward_control_surface(
        &self,
        surface: dragent_session_protocol::ControlSurface,
        ctx: &mut ViewContext<Self>,
    ) {
        if !self.is_remote_controller() {
            return;
        }
        use warpui::SingletonEntity;
        let sm = crate::realtime::manager::SessionManager::handle(ctx);
        ctx.update_model(&sm, move |m, _| m.send_control(surface));
    }

    /// Publish the complete input-layer presentation state to a shared-session
    /// viewer. PTY output alone cannot reconstruct ghost text or AI-tool input.
    pub(crate) fn broadcast_surface_state(&self, ctx: &mut ViewContext<Self>) {
        use crate::realtime::manager::SessionManager;
        use dragent_session_protocol::TerminalSurfaceState;
        use warpui::SingletonEntity;

        // Only the shared pane publishes its surface — otherwise a theme change or
        // keystroke in a DIFFERENT pane would clobber the viewer's mirror with the
        // wrong pane's input state.
        {
            let sm = SessionManager::handle(ctx);
            let pane = self.pane_uuid;
            if !ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane)) {
                return;
            }
        }

        // The host's grid dimensions travel with every surface update so a
        // mirror sizes its emulator to the host, not its own browser window.
        let (rows, cols) = self.model.read(ctx, |m, _| {
            m.with_core(|c| {
                let s = c.size();
                (Some(s.rows), Some(s.cols))
            })
        });

        let Some(tool) = self.active_ai_tool(ctx) else {
            let sm = SessionManager::handle(ctx);
            let state = self
                .command_editor
                .read(ctx, |editor, _| TerminalSurfaceState {
                    command_input: editor.get_text().to_string(),
                    command_ghost: editor.ghost_suffix().map(str::to_string),
                    command_cursor: editor.cursor_char_index(),
                    command_running: self.current_running_command.is_some(),
                    theme_id: Some(self.theme.id.to_string()),
                    rows,
                    cols,
                    scroll_fraction: self.scroll_fraction_for_share(),
                    ..Default::default()
                });
            ctx.update_model(&sm, |m, _| m.broadcast_surface_state(state));
            return;
        };

        let sm = SessionManager::handle(ctx);
        let state = self
            .rich_input_editor
            .read(ctx, |editor, _| TerminalSurfaceState {
                command_running: self.current_running_command.is_some(),
                ai_tool: Some(tool.label().to_string()),
                ai_input: editor.get_text().to_string(),
                ai_ghost: editor.ghost_suffix().map(str::to_string),
                ai_cursor: editor.cursor_char_index(),
                theme_id: Some(self.theme.id.to_string()),
                rows,
                cols,
                scroll_fraction: self.scroll_fraction_for_share(),
                ..Default::default()
            });
        ctx.update_model(&sm, |m, _| m.broadcast_surface_state(state));
    }

    /// Host: broadcast the shared pane's block structure (command cards + styled
    /// output) to viewers. No-op unless this pane is the shared one. Only blocks
    /// not yet finalized to viewers are snapshotted, so this stays cheap to call
    /// per output chunk and on block lifecycle changes.
    pub(crate) fn broadcast_shared_blocks(&self, ctx: &mut ViewContext<Self>) {
        use warpui::SingletonEntity;
        let sm = crate::realtime::manager::SessionManager::handle(ctx);
        let pane = self.pane_uuid;
        let sharing = ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane));
        if !sharing {
            return;
        }
        let theme = self.theme;
        let skip = ctx.read_model(&sm, |m, _| m.shared_finalized_blocks());
        let snapshot = self
            .model
            .read(ctx, |m, _| m.block_share_snapshot(&theme, &skip));
        ctx.update_model(&sm, |m, _| m.broadcast_blocks(snapshot));
    }

    /// Host: broadcast the agent-block list (AI conversation) to viewers. No-op
    /// unless this pane is the shared one.
    pub(crate) fn broadcast_shared_agent_blocks(&self, ctx: &mut ViewContext<Self>) {
        use warpui::SingletonEntity;
        let sm = crate::realtime::manager::SessionManager::handle(ctx);
        let pane = self.pane_uuid;
        if !ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane)) {
            return;
        }
        let blocks = self.agent_controller.read(ctx, |c, _| {
            crate::agent::agent_share::wire_agent_blocks(c.agent_blocks())
        });
        ctx.update_model(&sm, |m, _| m.broadcast_agent_blocks(blocks));
    }

    /// Host: throttled (leading + trailing) wrapper over
    /// [`broadcast_shared_blocks`](Self::broadcast_shared_blocks) for the hot
    /// per-output-chunk path. A trailing timer guarantees the final state, so a
    /// long-running-then-idle command's last output isn't dropped by the throttle.
    pub(crate) fn broadcast_shared_blocks_throttled(&self, ctx: &mut ViewContext<Self>) {
        use crate::realtime::manager::{SessionManager, ThrottleTick};
        use warpui::SingletonEntity;
        let sm = SessionManager::handle(ctx);
        if !ctx.read_model(&sm, |m, _| m.shared_pane() == Some(self.pane_uuid)) {
            return;
        }
        match ctx.update_model(&sm, |m, _| m.block_throttle_tick(Duration::from_millis(50))) {
            ThrottleTick::Now => self.broadcast_shared_blocks(ctx),
            ThrottleTick::Arm(remaining) => {
                ctx.spawn(Timer::after(remaining), |view, _, ctx| {
                    use warpui::SingletonEntity;
                    let sm = crate::realtime::manager::SessionManager::handle(ctx);
                    ctx.update_model(&sm, |m, _| m.block_throttle_fire());
                    view.broadcast_shared_blocks(ctx);
                });
            }
            ThrottleTick::Skip => {}
        }
    }

    /// Host: throttled (leading + trailing) wrapper over
    /// [`broadcast_shared_agent_blocks`](Self::broadcast_shared_agent_blocks) for
    /// per-token streaming. The whole agent block list is re-serialized on each
    /// call, so this coalesces the flood; the trailing timer flushes the final
    /// text/state so the viewer never shows a stuck "streaming" spinner.
    pub(crate) fn broadcast_shared_agent_blocks_throttled(&self, ctx: &mut ViewContext<Self>) {
        use crate::realtime::manager::{SessionManager, ThrottleTick};
        use warpui::SingletonEntity;
        let sm = SessionManager::handle(ctx);
        if !ctx.read_model(&sm, |m, _| m.shared_pane() == Some(self.pane_uuid)) {
            return;
        }
        match ctx.update_model(&sm, |m, _| {
            m.agent_throttle_tick(Duration::from_millis(100))
        }) {
            ThrottleTick::Now => self.broadcast_shared_agent_blocks(ctx),
            ThrottleTick::Arm(remaining) => {
                ctx.spawn(Timer::after(remaining), |view, _, ctx| {
                    use warpui::SingletonEntity;
                    let sm = crate::realtime::manager::SessionManager::handle(ctx);
                    ctx.update_model(&sm, |m, _| m.agent_throttle_fire());
                    view.broadcast_shared_agent_blocks(ctx);
                });
            }
            ThrottleTick::Skip => {}
        }
    }

    /// Host: replay this pane's full current state to viewers (used when a new
    /// participant joins). The manager has already reset the block cursor, so a
    /// block snapshot re-announces every block. Also pushes agent blocks, the
    /// input surface, and the alt-screen grid if a full-screen program is active.
    pub(crate) fn resync_shared_state(&self, ctx: &mut ViewContext<Self>) {
        use warpui::SingletonEntity;
        let sm = crate::realtime::manager::SessionManager::handle(ctx);
        let pane = self.pane_uuid;
        if !ctx.read_model(&sm, |m, _| m.shared_pane() == Some(pane)) {
            return;
        }
        // DIMENSIONS FIRST. The surface state carries the host's rows×cols, and a
        // mirror re-parses the raw byte stream through its own emulator — so it
        // must know the host's WIDTH before any content arrives. Sent last (as it
        // was), a viewer joining mid-command parses the output still streaming
        // from the running command at its own default width; those lines wrap at
        // the wrong column and a terminal cannot re-wrap scrollback afterwards,
        // so the block stays visibly mangled for the rest of the session.
        self.broadcast_surface_state(ctx);
        self.broadcast_shared_blocks(ctx);
        self.broadcast_shared_agent_blocks(ctx);
        if self.model.read(ctx, |m, _| m.alt_screen_active()) {
            let theme = self.theme;
            let snap = self.model.read(ctx, |m, _| m.alt_screen_snapshot(&theme));
            ctx.update_model(&sm, |m, _| m.broadcast_alt_screen(snap));
        }
    }
}

/// What the user calls the shell at `path` ("Git Bash", "PowerShell", "zsh").
fn shell_display_name(path: &str) -> String {
    if let Some(choice) = dragent_pty::available_shells()
        .iter()
        .find(|choice| choice.path.eq_ignore_ascii_case(path))
    {
        return choice.label.clone();
    }
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path);
    let in_git = path.to_ascii_lowercase().replace('/', r"\").contains(r"\git\");
    match stem.to_ascii_lowercase().as_str() {
        "bash" if in_git => "Git Bash".into(),
        "pwsh" => "PowerShell".into(),
        "powershell" => "Windows PowerShell".into(),
        "cmd" => "Command Prompt".into(),
        _ => stem.to_string(),
    }
}
