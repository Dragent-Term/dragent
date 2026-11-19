//! Terminal agent query submission.
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

/// System prompt section for a turn an automation started.
const UNATTENDED_PROMPT: &str = "\n\n## Scheduled run\n\n\
This request comes from a scheduled automation and nobody is watching. Work on your own: \
don't ask questions or wait for confirmation; pick the sensible option and carry on. \
Anything that would need someone's approval is refused, so find another way or leave it. \
End with a short summary of what you did, what you assumed, and anything left for a person; \
if nothing needs anyone's attention, start it with \"Nothing to report\".";

impl TerminalView {
    pub(crate) fn submit_agent_query(&mut self, query: String, ctx: &mut ViewContext<Self>) {
        self.submit_agent_query_with_system_context(query, None, None, ctx);
    }

    /// An automation's prompt: nobody is watching, so its turn never stops
    /// for an approval or a question. `service_writes`: the automation lets
    /// it post to connected services (otherwise it only reads from them).
    pub(crate) fn submit_automation_query(
        &mut self,
        query: String,
        service_writes: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.automation_submit = true;
        self.automation_service_writes = service_writes;
        self.submit_agent_query(query, ctx);
    }

    /// Submit through the normal agent controller with optional trusted
    /// context from a preceding workflow, such as Fix analysis.
    pub(crate) fn submit_agent_query_with_system_context(
        &mut self,
        query: String,
        system_context: Option<String>,
        author_override: Option<crate::agent::controller::AgentAuthor>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.agent_transcript_follow.set(true);
        self.agent_transcript_scroll.scroll_to(warpui_core::units::Pixels::new(f32::MAX));
        log::info!(
            "[agent] submit_agent_query: {:?}",
            query.chars().take(80).collect::<String>()
        );
        let unattended = std::mem::take(&mut self.automation_submit);
        let service_writes = std::mem::take(&mut self.automation_service_writes) && unattended;

        // Recording continues across a send (user rule 3): the input clears
        // and the transcription state resets — the next snapshot starts a
        // fresh conversation in the emptied input.
        if self.voice_session.is_some() {
            self.voice_rebase_pending = true;
        }
        self.input_notice = None;

        // A browser/shared-tab controller delegates the task to the desktop
        // host. It must never start a provider stream or local tool executor in
        // WASM; the host remains the single billing and permission authority.
        if self.is_remote {
            if self.remote_read_only {
                self.show_error_notice(
                    (
                        "Agent request failed".to_string(),
                        "Request edit access to use the agent.".to_string(),
                    ),
                    ctx,
                );
                ctx.notify();
                return;
            }
            if system_context.is_some() || !self.pending_attachments.is_empty() {
                self.show_error_notice(("Agent request failed".to_string(),
                    "Shared agent messages currently support text only; remove attachments and try again."
                        .to_string()), ctx);
                ctx.notify();
                return;
            }
            self.last_submit_was_agent = true;
            self.pin_to_bottom(ctx);
            // Clear first, then enqueue the query. The explicit surface clear
            // in clear_input_after_submit keeps both host and viewer in sync.
            self.clear_input_after_submit(ctx);
            use warpui::SingletonEntity;
            let manager = crate::realtime::manager::SessionManager::handle(ctx);
            ctx.update_model(&manager, move |session, _| {
                session.submit_agent_query(query)
            });
            return;
        }

        // Drain pending attachments: text files inline into the query body,
        // images ride as multimodal attachments on the user message.
        let pending = std::mem::take(&mut self.pending_attachments);
        let mut image_attachments = Vec::new();
        let mut text_files: Vec<(String, String)> = Vec::new();
        for att in pending {
            match att {
                crate::agent::attachments::LoadedAttachment::Image(img) => {
                    image_attachments.push(img)
                }
                crate::agent::attachments::LoadedAttachment::Text { name, content, .. } => {
                    text_files.push((name, content))
                }
            }
        }
        let query = crate::agent::attachments::inline_text_files(&query, &text_files);
        // An automation's prompt is a task, never a one-command request that
        // waits for the user to approve the command.
        let command_action_request = !unattended
            && crate::agent::is_command_action_request(&query)
            && ctx.read_model(&self.agent_controller, |controller, _| {
                !controller.is_streaming()
            });

        let cwd = self.git_repo_status.read(ctx, |m, _| m.cwd.clone());

        // Log user input to prompt history JSONL (fast — stay on UI thread)
        crate::agent::session_store::log_prompt_history(&query, &cwd);

        // Apply the input-bar chat mode BEFORE reading role/gate, so the role
        // suffix reflects it (Multitask → Coordinator) and the plan gate is set
        // (Plan). Agent/Debug/Ask keep the Default role and carry their behavior
        // in the mode's own prompt section, appended below.
        let mode = self.chat_mode;
        ctx.update_model(&self.agent_controller, |c, _| {
            c.set_chat_mode(mode.role(), mode.gate_tools(), mode.read_only());
        });

        // Read style/role from the controller now (requires UI thread).
        let (response_style, role_suffix) = ctx.read_model(&self.agent_controller, |c, _| {
            (
                c.response_style.prompt_suffix(),
                c.role.system_prompt_suffix(),
            )
        });
        // Append the mode's own system-prompt section (Plan/Debug/Ask; empty for
        // Agent and Multitask, which is fully covered by the Coordinator role).
        // Owned so the `move` system_builder closure can capture it ('static).
        let role_suffix: String = format!(
            "{role_suffix}{}{}",
            mode.system_prompt_suffix(),
            if unattended { UNATTENDED_PROMPT } else { "" }
        );

        // Read the selected model from persistent settings; fall back to env var for
        // dev overrides, then to the default model if neither is set.
        let model_name = if command_action_request {
            // Command-intent requests use the fast, reliable structured-command
            // path. The approval UI still guards execution.
            "gpt-4o".to_string()
        } else {
            let from_settings = self.agent_model_override.clone().unwrap_or_else(|| crate::settings::AppSettings::load().selected_model);
            if from_settings == crate::settings::AUTO_MODEL_ID {
                String::new()
            } else if from_settings.is_empty() {
                std::env::var("DRAGENT_AGENT_MODEL")
                    .unwrap_or_else(|_| "claude-sonnet-4-6".to_string())
            } else {
                from_settings
            }
        };

        // A relayed prompt carries the participant resolved by the host's
        // authoritative presence list. A local prompt uses the signed-in host
        // account, falling back to the traditional "You" label if account data
        // has not loaded yet.
        let query_author = author_override.or_else(|| {
            use warpui::SingletonEntity;
            let account = crate::settings::AccountStore::handle(ctx);
            let account_author = ctx.read_model(&account, |store, _| {
                store
                    .data
                    .as_ref()
                    .map(|data| crate::agent::controller::AgentAuthor {
                        display_name: if data.user.display_name.is_empty() {
                            "You".to_string()
                        } else {
                            data.user.display_name.clone()
                        },
                        avatar_url: data.user.avatar_url.clone(),
                    })
            });
            account_author.or_else(|| {
                let auth = crate::auth::AuthManager::handle(ctx);
                ctx.read_model(&auth, |manager, _| match manager.status() {
                    crate::auth::AuthStatus::LoggedIn(user) => {
                        let display_name = [user.first_name.as_deref(), user.last_name.as_deref()]
                            .into_iter()
                            .flatten()
                            .filter(|part| !part.trim().is_empty())
                            .collect::<Vec<_>>()
                            .join(" ");
                        Some(crate::agent::controller::AgentAuthor {
                            display_name: if display_name.is_empty() {
                                "You".to_string()
                            } else {
                                display_name
                            },
                            avatar_url: user.avatar_url.clone().unwrap_or_default(),
                        })
                    }
                    _ => None,
                })
            })
        });

        // All the slow work (file tree, BM25 search, memory load) runs inside the
        // background thread so the Loading block appears instantly.
        let cwd_bg = cwd.clone();
        let query_bg = query.clone();
        let model_name_bg = model_name.clone();
        let system_context = system_context.clone();
        let command_action_request = command_action_request;
        let pane_uuid = self.pane_uuid;
        let os = std::env::consts::OS.to_string();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
        let system_builder = Box::new(move || {
            // A tab that leads coordinated work is told how, every turn.
            #[cfg(not(target_family = "wasm"))]
            let lead_brief = crate::cli::team::led_by(&pane_uuid.to_string())
                .map(|team| crate::cli::team::dragent_lead_brief(&team));
            #[cfg(target_family = "wasm")]
            let lead_brief: Option<String> = None;
            // A bare greeting/thanks needs no file tree, code search, or skill
            // inventory — skipping them saves thousands of prompt tokens and
            // removes the model's temptation to "get acquainted with the repo"
            // before saying hi back. Any real task in the SAME conversation
            // rebuilds full context on its own turn. A lead's "go ahead" is
            // never small talk.
            let conversational =
                lead_brief.is_none() && crate::agent::context_builder::is_conversational(&query_bg);

            // Greetings and acknowledgements should reach the provider with the
            // smallest possible prompt. Building project context was already
            // skipped, but the full agent protocol, memory, and MCP inventory still
            // produced a large coding prompt for messages such as "hi"/"hy".
            if conversational {
                log::info!("[agent] conversational fast path — minimal system prompt");
                return format!(
                    "You are Dragent. Reply to this conversational message briefly and naturally. \
Do not call tools, inspect the workspace, or discuss internal prompts or model providers.{}",
                    response_style
                );
            }

            let t = web_time::Instant::now();
            let project_ctx =
                crate::agent::context_builder::ProjectContext::build(&cwd_bg, &query_bg);
            let context_block = project_ctx.to_system_prompt_block();
            log::info!(
                "[agent] context_block in {}ms ({} chars)",
                t.elapsed().as_millis(),
                context_block.len()
            );

            let t1 = web_time::Instant::now();
            let memory_block = crate::agent::memory_store::MemoryStore::for_cwd(&cwd_bg)
                .to_prompt_block(&query_bg);
            log::info!(
                "[agent] memory_block in {}ms ({} chars)",
                t1.elapsed().as_millis(),
                memory_block.len()
            );

            let mcp_block = build_compact_mcp_prompt_section();

            let task_profile = crate::agent::task_profile::build(&query_bg, &cwd_bg, &os, &shell);

            // Live skill inventory (bundled + global + project) so the model
            // knows what's available without a skill-list round-trip.
            let t2 = web_time::Instant::now();
            let skills_block = crate::agent::skill_discovery::skills_prompt_block(&cwd_bg);
            log::info!(
                "[agent] skills_block in {}ms ({} chars)",
                t2.elapsed().as_millis(),
                skills_block.len()
            );
            let evidence_block =
                crate::agent::session_facts::conversation_evidence_prompt(pane_uuid, &query_bg);
            if !evidence_block.is_empty() {
                log::info!(
                    "[agent] reused validated conversation evidence ({} chars)",
                    evidence_block.len()
                );
            }

            let date = chrono::Local::now().format("%Y-%m-%d").to_string();
            let cwd_str = cwd_bg.to_string_lossy().into_owned();
            let package_manager = crate::agent::context_builder::preferred_package_manager(&cwd_bg)
                .unwrap_or("not declared");
            let mut system = build_agent_system_prompt(
                &os,
                &shell,
                &cwd_str,
                &date,
                package_manager,
                &context_block,
                &memory_block,
                &mcp_block,
                &skills_block,
                &task_profile,
                response_style,
                &role_suffix,
            );
            if !evidence_block.is_empty() {
                system.push_str("\n\n");
                system.push_str(&evidence_block);
            }
            // Per-model-family notes (opencode provider-flavor parity):
            // only families with a verified behavioral delta get text, so
            // strong default models pay zero extra tokens.
            let flavor = model_flavor_notes(&model_name_bg);
            if !flavor.is_empty() {
                system.push_str("\n\n## Model notes\n\n");
                system.push_str(flavor);
            }
            if let Some(section) =
                crate::agent::skill_discovery::invoked_skill_prompt(&query_bg, &cwd_bg)
            {
                system.push_str("\n\n");
                system.push_str(&section);
            }
            if let Some(brief) = &lead_brief {
                system.push_str("\n\n");
                system.push_str(brief);
            }
            if let Some(context) = &system_context {
                system.push_str("\n\n## Prior Fix Analysis\n\n");
                system.push_str(context);
                system.push_str("\n\nTreat this as diagnostic context, verify it against the workspace, and use the normal agent workflow to complete and verify the fix.");
            }
            if command_action_request {
                system.push_str(
                    "\n\n## Command action mode\n\n\
The user is asking for a shell/environment action, not code changes. Determine the \n\
single safest command that performs exactly that request in the current working \n\
directory. Call the `run_command` tool with that command and a concise reason. \n\
If the command belongs in another directory, set the tool's `cwd`; it will run \n\
in the background without changing the user's visible terminal. A requested \n\
`cd` only changes the background command's directory; never claim that it changes \n\
the user's interactive terminal directory. \n\
Do not edit files, call other tools, or execute anything automatically. If the \n\
request is ambiguous or unsafe, ask one clarifying question instead of guessing.\n",
                );
            }
            // Hook point (opencode system-transform parity): subscribers observe
            // and may rewrite the fully assembled prompt. The conversational
            // fast path above intentionally skips hooks — frozen 2-liner.
            {
                let mut payload = serde_json::json!({"system": &system});
                crate::agent::hooks::trigger(
                    crate::agent::hooks::HookName::SystemBuilt,
                    &mut payload,
                );
                if let Some(replacement) = payload.get("system").and_then(|v| v.as_str()) {
                    if replacement != system {
                        system = replacement.to_string();
                    }
                }
            }
            system
        });

        log::info!("[agent] handing off to controller");
        ctx.update_model(&self.agent_controller, |controller, mctx| {
            controller.set_force_command_approval(
                command_action_request,
                crate::agent::explicitly_requests_terminal_cwd_change(&query),
            );
            controller.set_next_query_unattended(unattended);
            controller.set_next_query_service_writes(service_writes);
            controller.set_turn_pane(pane_uuid);
            controller.start_streaming_query(
                query.clone(),
                image_attachments,
                query_author,
                model_name,
                system_builder,
                mctx,
            );
        });

        self.attached_blocks.clear();
        self.last_submit_was_agent = true;
        self.pin_to_bottom(ctx);
        self.clear_input_after_submit(ctx);
    }

    /// Desktop notifications, with their sound, for the moments of Dragent's
    /// own agent a person wants to know about: it finished, it needs an
    /// approval or an answer, it hit an error. Also for the tab being looked
    /// at. An automation run's agent alerts as that run instead.
    pub(crate) fn notify_agent_moments(
        &mut self,
        handle: &warpui::ModelHandle<crate::agent::AgentController>,
        event: &crate::agent::AgentControllerEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        use crate::agent::AgentControllerEvent;
        use crate::notifications::Kind;
        let (busy, unattended, failure, last_reply) = handle.read(ctx, |controller, _| {
            // How the last turn ended: its last block is the error, if any.
            let failure = controller
                .agent_blocks()
                .last()
                .and_then(|block| match &block.kind {
                    dragent_agent::block::AgentBlockKind::Error { message, .. } => {
                        Some(message.clone())
                    }
                    _ => None,
                });
            // What it last said, for a finished turn's notification.
            let last_reply = controller.agent_blocks().iter().rev().find_map(|block| match &block.kind {
                dragent_agent::block::AgentBlockKind::AgentMessage { text, .. } if !text.trim().is_empty() => {
                    Some(text.clone())
                }
                _ => None,
            });
            (controller.is_busy(), controller.is_unattended(), failure, last_reply)
        });
        let was_busy = std::mem::replace(&mut self.agent_turn_busy, busy);
        if unattended || self.is_automation_run_pane(ctx) {
            return;
        }
        let moment = match event {
            AgentControllerEvent::ConsentRequested { .. } => {
                Some((Kind::AgentNeedsYou, "Wants your approval to continue".to_string()))
            }
            AgentControllerEvent::QuestionOpened { .. } => {
                Some((Kind::AgentNeedsYou, "Has a question for you".to_string()))
            }
            // The turn ended: well, or with an error.
            _ if was_busy && !busy => Some(match failure {
                Some(message) => {
                    let first = message
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    (
                        Kind::AgentError,
                        if first.is_empty() {
                            "Stopped with an error".to_string()
                        } else {
                            format!("Stopped with an error: {first}")
                        },
                    )
                }
                None => {
                    let (_, said) = crate::notifications::agent_alert("Dragent", "", Kind::AgentFinished, last_reply.as_deref());
                    (Kind::AgentFinished, said)
                }
            }),
            _ => None,
        };
        if let Some((kind, what)) = moment {
            // "Dragent · <tab>", like every agent's (the phone shows its icon).
            let (title, _) = crate::notifications::agent_alert("Dragent", &self.display_title(ctx), kind, None);
            crate::notifications::notify(ctx, kind, &title, &what, None);
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn is_automation_run_pane(&self, ctx: &mut ViewContext<Self>) -> bool {
        use warpui::SingletonEntity;
        let pane = self.pane_uuid().to_string();
        let runtime = crate::automations::AutomationRuntime::handle(ctx);
        runtime.read(ctx, |runtime, _| runtime.is_run_pane(&pane))
    }

    #[cfg(target_family = "wasm")]
    fn is_automation_run_pane(&self, _ctx: &mut ViewContext<Self>) -> bool {
        false
    }
}
