//! Host-owned terminal tabs: daemon PTYs that outlive the app process.
//!
//! A host tab is created instantly with a pending binding slot (shell-hook
//! bootstrap plus early input buffer inside), then bound asynchronously once
//! the host chain lands. Output streams back through the per-tab pump below.
//! Host failures keep the pane host-backed and retry the binding so the
//! session remains owned by the daemon; there is no silent local fallback.
//!
//! Quit safety: view teardown only DETACHES the slot. The session itself is
//! closed explicitly on tab close, so quitting the app leaves servers
//! running in the daemon and relaunch re-attaches to them.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use warpui::r#async::Timer;
use warpui::{UpdateView, ViewContext, ViewHandle};

use super::host_mirror::HostMirror;
use super::pane::WorkspacePane;
use super::project_naming::default_pane_cwd;
use super::view::{PaneTab, WorkspaceView};
use super::DEFAULT_SESSION_TITLE;
use crate::agent::TabSessionKind;
use crate::terminal::manager::pty_writer::HostPtyBinding;
use crate::terminal::view::terminal_view::HostRecoveryAction;
use crate::terminal::TerminalView;
use dragent_workspace_model::WorkItemId;

/// One host-backed pane. The mirror retains its durable identity even while
/// the transport is unavailable; `session_id` describes the bound transport.
pub(crate) struct HostOwnedTab {
    pub client: Option<dragent_host::HostClient>,
    pub remote_host: Option<String>,
    pub pane: ViewHandle<TerminalView>,
    pub workspace_id: String,
    pub home_id: String,
    pub session_id: Option<String>,
    pub mirror: HostMirror,
    pub slot: Arc<HostPtyBinding>,
    pub root: PathBuf,
    pub cwd: PathBuf,
    /// Consecutive failed attempts to (re)bind this tab; drives the backoff.
    pub bind_attempts: u32,
    /// Command to submit once a restarted session binds (restart only).
    pub pending_launch_command: Option<String>,
    /// Cancel flag of the output-stream worker for the current session;
    /// `None` when no worker runs.
    pub stream: Option<Arc<AtomicBool>>,
    /// An unused conversation was already replaced automatically in this
    /// app run; a second failure is shown to the user instead of looping.
    pub started_over: bool,
}

impl HostOwnedTab {
    pub(crate) fn persisted_session_id(&self) -> Option<&str> {
        self.session_id.as_deref().or_else(|| {
            (!self.mirror.session_id.is_empty()).then_some(self.mirror.session_id.as_str())
        })
    }
}

/// Progress messages in flight between stream workers and the UI. Bounded on
/// purpose: when the UI falls behind, workers block, stop reading their
/// streams, and the host pauses the shells (flow control end to end).
const STREAM_CHANNEL_CHUNKS: usize = 64;

/// Only progress and lifecycle cross into the UI; bytes are parsed by the
/// stream worker before progress is published.
pub(crate) enum HostStreamMsg {
    Parsed {
        next_offset: u64,
        exit_code: Option<i32>,
    },
    Gone,
    RestartedForUpdate,
    RestoreFailed,
    AgentResumeFailed,
    Unavailable,
    /// The stream found its host again under a new endpoint (it restarted):
    /// the tab's other requests use this client from now on.
    Rebound(dragent_host::HostClient),
}

/// (tab id, session id, message) from a stream worker to the view.
pub(crate) type HostStreamSender = async_channel::Sender<(usize, String, HostStreamMsg)>;

/// Re-attach bundle for one restored tab: pane born host-pending, daemon
/// binding from the last session, and roots for the attach-or-spawn chain.
pub(crate) struct HostReattach {
    pub pane: ViewHandle<TerminalView>,
    pub slot: Arc<HostPtyBinding>,
    pub binding: dragent_persistence::SerializedHostTerminalBinding,
    pub cwd: PathBuf,
    pub project_root: Option<PathBuf>,
    /// The pane ran an agent. Without a saved session it is reported
    /// unavailable instead of silently becoming a plain shell.
    pub expect_agent: bool,
    /// Live update recovery also reopens plain terminal shells.
    pub restart_after_update: bool,
}

impl WorkspaceView {
    /// Create a host-owned tab: instant pane (connecting state) + async bind.
    /// Mirrors `create_tab_with_cwd_focus` shape so call sites swap 1:1.
    pub(crate) fn create_host_tab(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
        focus: bool,
    ) -> PaneTab {
        self.build_host_tab(
            ctx,
            custom_label,
            shell,
            cwd,
            project_root,
            focus,
            DEFAULT_SESSION_TITLE.to_string(),
            None,
            TabSessionKind::Shell,
            true,
            None,
            None,
        )
    }

    /// Host-owned variant of `create_agent_tab_at`.
    pub(crate) fn create_host_agent_tab_at(
        &mut self,
        ctx: &mut ViewContext<Self>,
        project_root: Option<PathBuf>,
    ) -> PaneTab {
        let cwd = project_root.clone().unwrap_or_else(default_pane_cwd);
        self.build_host_tab(
            ctx,
            Some("Dragent".to_string()),
            None,
            cwd,
            project_root,
            true,
            "Dragent".to_string(),
            Some("Dragent".to_string()),
            TabSessionKind::Agent,
            true,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_host_tab(
        &mut self,
        ctx: &mut ViewContext<Self>,
        custom_label: Option<String>,
        shell: Option<String>,
        cwd: PathBuf,
        project_root: Option<PathBuf>,
        focus: bool,
        title: String,
        title_custom_label: Option<String>,
        tab_kind: TabSessionKind,
        in_navigation: bool,
        client: Option<dragent_host::HostClient>,
        existing: Option<dragent_host::protocol::TerminalSession>,
    ) -> PaneTab {
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;

        // Remote project folders (stand-in paths) run on their machine.
        let stand_in = crate::remote_fs::resolve(project_root.as_deref().unwrap_or(&cwd));
        let is_remote = stand_in.is_some()
            || client
                .as_ref()
                .is_some_and(|c| c.remote_connection().is_some());
        let shell = if is_remote {
            stand_in
                .as_ref()
                .and_then(|remote| crate::remote_fs::shell(&remote.machine_id))
                .or(shell)
                .unwrap_or_else(|| "/bin/sh".into())
        } else {
            dragent_pty::resolve_shell(
                shell
                    .or_else(|| {
                        crate::console::editor_settings::EditorSettings::global()
                            .terminal
                            .default_shell
                            .clone()
                    })
                    .or_else(|| std::env::var("SHELL").ok()),
            )
        };
        let slot = Arc::new(HostPtyBinding::new());
        if let Some(client) = &client {
            slot.set_client(client.clone());
        }
        let pane = WorkspacePane::spawn_host_pending(ctx, cwd.clone(), slot.clone());
        if let WorkspacePane::Terminal(handle) = &pane {
            let machine = stand_in.as_ref().map(|remote| remote.machine_id.clone());
            ctx.update_view(handle, |view, _| {
                view.launch_shell = Some(shell.clone());
                view.set_remote_machine(machine);
            });
        }
        if tab_kind == TabSessionKind::Agent {
            if let WorkspacePane::Terminal(handle) = &pane {
                let handle = handle.clone();
                ctx.update_view(&handle, |view: &mut TerminalView, _| {
                    view.set_tab_session_kind(TabSessionKind::Agent);
                });
            }
        }
        if in_navigation {
            super::tabs_panes::subscribe_workspace_pane(
                tab_id,
                &pane,
                self.persistence_writer.sender(),
                ctx,
            );
        }
        if focus {
            pane.focus(ctx);
            self.active_pane_id = Some(pane.id());
        }
        let surface_id = if in_navigation {
            self.register_surface(&pane, ctx)
        } else {
            pane.surface_id(ctx)
        };
        let terminal_handle = match &pane {
            WorkspacePane::Terminal(handle) => Some(handle.clone()),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        };

        let anchor = if is_remote {
            project_root.clone().unwrap_or_else(|| cwd.clone())
        } else {
            project_root
                .clone()
                .filter(|root| root.is_dir())
                .or_else(|| cwd.is_dir().then(|| cwd.clone()))
                .unwrap_or_else(default_pane_cwd)
        };
        self.host_owned_tabs.insert(
            tab_id,
            HostOwnedTab {
                remote_host: client
                    .as_ref()
                    .and_then(|c| c.remote_connection())
                    .and_then(|r| serde_json::to_string(&r.host).ok()),
                client,
                pane: terminal_handle
                    .clone()
                    .expect("host tab is always a terminal"),
                workspace_id: String::new(),
                home_id: String::new(),
                session_id: None,
                mirror: HostMirror::new(String::new()),
                slot: slot.clone(),
                root: anchor.clone(),
                cwd: cwd.clone(),
                bind_attempts: 0,
                pending_launch_command: None,
                stream: None,
                started_over: false,
            },
        );

        if let Some(session) = existing {
            slot.bind(&session.workspace_id, &session.id);
            if let Some(tab) = self.host_owned_tabs.get_mut(&tab_id) {
                tab.workspace_id = session.workspace_id;
                tab.home_id = session.worktree_id;
                tab.session_id = Some(session.id.clone());
                tab.mirror = HostMirror::new(session.id);
            }
            self.ensure_host_tabs_pump(ctx);
        } else {
            self.bind_new_host_tab(ctx, tab_id, slot, anchor, cwd, 0);
        }

        PaneTab {
            id: tab_id,
            surface_id,
            title,
            custom_label: title_custom_label.or(custom_label),
            color: None,
            work_item_id: Some(WorkItemId::new()),
            project_root,
            hidden_from_sidebar: false,
            pane,
        }
    }

    /// Allocate a daemon-backed terminal outside main-tab navigation. The
    /// registry id uses the global sequence, never the dock's separate ids.
    pub(crate) fn create_host_auxiliary_pane(
        &mut self,
        cwd: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> WorkspacePane {
        self.build_host_tab(
            ctx,
            None,
            None,
            cwd,
            None,
            false,
            DEFAULT_SESSION_TITLE.to_string(),
            None,
            TabSessionKind::Shell,
            false,
            None,
            None,
        )
        .pane
    }

    pub(crate) fn close_host_pane_session(&mut self, pane_id: warpui::EntityId) {
        let id = self
            .host_owned_tabs
            .iter()
            .find(|(_, owned)| owned.pane.id() == pane_id)
            .map(|(id, _)| *id);
        if let Some(id) = id {
            self.close_host_tab_session(id);
        }
    }

    pub(crate) fn host_binding_for_pane(
        &self,
        pane: &WorkspacePane,
        app: &warpui::AppContext,
    ) -> Option<dragent_persistence::SerializedHostTerminalBinding> {
        let owned = self
            .host_owned_tabs
            .values()
            .find(|owned| owned.pane.id() == pane.id())?;
        Some(dragent_persistence::SerializedHostTerminalBinding {
            remote_host: owned.remote_host.clone(),
            pane_uuid: pane.pane_uuid(app),
            workspace_id: owned.workspace_id.clone(),
            home_id: owned.home_id.clone(),
            session_id: owned.persisted_session_id()?.to_owned(),
            agent: owned.pane.read(app, |view, _| {
                view.tab_session_kind == TabSessionKind::Agent
            }),
        })
    }

    fn bind_new_host_tab(
        &mut self,
        ctx: &mut ViewContext<Self>,
        tab_id: usize,
        slot: Arc<HostPtyBinding>,
        anchor: PathBuf,
        cwd: PathBuf,
        attempt: u32,
    ) {
        let anchor_str = anchor.to_string_lossy().into_owned();
        let cwd_str = cwd.to_string_lossy().into_owned();
        let remote_client = self
            .host_owned_tabs
            .get(&tab_id)
            .and_then(|tab| tab.client.clone());
        let remote_host = self
            .host_owned_tabs
            .get(&tab_id)
            .and_then(|tab| tab.remote_host.clone());
        let shell = self
            .host_owned_tabs
            .get(&tab_id)
            .and_then(|tab| tab.pane.read(ctx, |view, _| view.launch_shell.clone()));
        let stand_in = crate::remote_fs::resolve(&anchor);
        ctx.spawn(
            async move {
                let mut anchor_str = anchor_str;
                let mut cwd_str = cwd_str;
                let mut shell = shell;
                let client = match (stand_in, remote_client, remote_host) {
                    // A remote project folder: its machine, in its own paths,
                    // with the machine's login shell.
                    (Some(remote), _, _) => {
                        let client = crate::remote_fs::client(&remote.machine_id)?;
                        cwd_str = crate::remote_fs::resolve(std::path::Path::new(&cwd_str))
                            .map(|cwd| cwd.path)
                            .unwrap_or_else(|| remote.path.clone());
                        anchor_str = remote.path;
                        shell = None;
                        client
                    }
                    (None, Some(client), _) => client,
                    (None, None, Some(host)) => dragent_host::HostClient::connect_remote(serde_json::from_str(&host).map_err(|e| e.to_string())?)?,
                    (None, None, None) => dragent_host::HostClient::ensure_host()?,
                };
                let workspace_id = if client.remote_connection().is_some() { client.create_workspace(&anchor_str)? } else { client.create_local_workspace(&anchor_str)? };
                let home = client.ensure_terminal_home(&workspace_id, &anchor_str, &cwd_str)?;
                let session = client.create_terminal_with_shell(
                    &workspace_id,
                    &home.id,
                    None,
                    Some(&cwd_str),
                    80,
                    24,
                    shell.as_deref(),
                )?;
                Ok::<_, String>((workspace_id, home.id, session.id, client))
            },
            move |view, result, ctx| {
                match result {
                    Ok((workspace_id, home_id, session_id, client)) => {
                        if !view.host_owned_tabs.contains_key(&tab_id) {
                            slot.detach();
                            crate::bg::detach(move || {
                                 {
                                    let _ = client.close_terminal(&workspace_id, &session_id);
                                }
                            });
                            return;
                        }
                        // No bind here: the stream binds once the checkpoint is
                        // applied, so input typed or submitted meanwhile reaches
                        // the shell after that checkpoint, never before it.
                        slot.set_client(client.clone());
                        if let Some(owned) = view.host_owned_tabs.get_mut(&tab_id) {
                            owned.client = Some(client);
                            owned.workspace_id = workspace_id;
                            owned.home_id = home_id;
                            owned.session_id = Some(session_id.clone());
                            owned.mirror.session_id = session_id;
                            let pane = owned.pane.clone();
                            let launch = owned.pending_launch_command.take();
                            ctx.update_view(&pane, |tv, tctx| {
                                tv.set_host_notice(None, tctx);
                                if let Some(command) = launch {
                                    tv.submit_shell_command(&command, tctx);
                                }
                            });
                        }
                        view.ensure_host_tabs_pump(ctx);
                        view.save_session(ctx);
                    }
                    Err(error) if is_missing_folder(&error) => {
                        log::warn!("[host-tab] bind stopped tab={tab_id}: {error}");
                        view.mark_host_unavailable(tab_id, &missing_folder_notice(&anchor), ctx);
                    }
                    Err(error) => {
                        let delay = host_retry_delay_for(attempt, &error);
                        log::warn!(
                            "[host-tab] bind retry tab={tab_id} attempt={attempt} in {delay:?}: {error}"
                        );
                        if view.host_owned_tabs.contains_key(&tab_id) {
                            if let Some(pane) =
                                view.host_owned_tabs.get(&tab_id).map(|owned| owned.pane.clone())
                            {
                                ctx.update_view(&pane, |tv, tctx| {
                                    show_host_retry(tv, delay, &error, tctx)
                                });
                            }
                            ctx.spawn(Timer::after(delay), move |view, _, ctx| {
                                if view
                                    .host_owned_tabs
                                    .get(&tab_id)
                                    .is_some_and(|tab| tab.session_id.is_none())
                                {
                                    view.bind_new_host_tab(
                                        ctx,
                                        tab_id,
                                        slot,
                                        anchor,
                                        cwd,
                                        attempt.saturating_add(1),
                                    );
                                }
                            });
                        }
                    }
                }
                ctx.notify();
            },
        );
    }

    /// Stop mirroring a host session without ending it (a remote project
    /// was removed from this device, or its machine disconnected).
    pub(crate) fn detach_host_tab(&mut self, tab_id: usize) {
        let Some(owned) = self.host_owned_tabs.remove(&tab_id) else {
            return;
        };
        if let Some(cancel) = owned.stream.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        owned.slot.detach();
    }

    /// What the E2E automation server reports about each host-owned tab.
    pub(crate) fn automation_agent_panes(&self, ctx: &ViewContext<Self>) -> serde_json::Value {
        let panes: Vec<_> = self
            .host_owned_tabs
            .iter()
            .map(|(tab_id, owned)| {
                owned.pane.read(ctx, |view, view_ctx| {
                    view.model.read(view_ctx, |model, _| {
                        model.with_core(|core| {
                            serde_json::json!({
                                "tab": tab_id,
                                "pane": view.pane_uuid.to_string(),
                                "session": owned.session_id,
                                "running": core.running_command(),
                                "alt": core.alt_screen_active(),
                                "cwd": core.cwd().display().to_string(),
                                "theme_dark": view.theme.is_dark,
                                "stale": core.stale_app_colors(&view.theme).is_some(),
                                "needs_restart": core.needs_restart_for_theme(&view.theme),
                                "screen": core.alt_screen_emulator().alt_screen_all_lines()
                                    .iter().map(|line| line.plain_text()).collect::<Vec<_>>()
                                    .join("\n"),
                            })
                        })
                    })
                })
            })
            .collect();
        serde_json::json!({
            "panes": panes,
            "workspaces": self.workspaces.iter().map(|workspace| serde_json::json!({
                "root": workspace.root.display().to_string(),
                "title": workspace.title,
            })).collect::<Vec<_>>(),
        })
    }

    /// Some agents read the terminal's colours once at startup and cannot hear
    /// about a change, so the only way they follow a dark/light switch is to
    /// start again. Wait until each has finished its turn, then restart it on
    /// the same conversation. Until then its pane keeps the colours it was told.
    pub(crate) fn schedule_agent_theme_restarts(&mut self, ctx: &mut ViewContext<Self>) {
        if self.agent_theme_restarts_scheduled {
            return;
        }
        self.agent_theme_restarts_scheduled = true;
        ctx.spawn(Timer::after(AGENT_THEME_RESTART_POLL), |view, _, ctx| {
            view.agent_theme_restarts_scheduled = false;
            view.restart_agents_for_theme(ctx);
        });
    }

    #[cfg(target_family = "wasm")]
    fn restart_agents_for_theme(&mut self, _ctx: &mut ViewContext<Self>) {}

    #[cfg(not(target_family = "wasm"))]
    fn restart_agents_for_theme(&mut self, ctx: &mut ViewContext<Self>) {
        let waiting: Vec<(usize, uuid::Uuid, bool)> = self
            .host_owned_tabs
            .iter()
            .filter(|(_, owned)| owned.session_id.is_some() && owned.remote_host.is_none())
            .filter_map(|(tab_id, owned)| {
                owned.pane.read(ctx, |view, view_ctx| {
                    let needs_restart = view.model.read(view_ctx, |model, _| {
                        model.with_core(|core| core.needs_restart_for_theme(&view.theme))
                    });
                    let quiet = view
                        .manager
                        .writer()
                        .input_quiet_for()
                        .is_some_and(|quiet| quiet >= AGENT_THEME_RESTART_QUIET);
                    needs_restart.then_some((*tab_id, view.pane_uuid, quiet))
                })
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            // Agents without a launch record can't be restarted: stop polling
            // for them, and their panes keep the colours they were told.
            let waiting: Vec<(usize, bool)> = waiting
                .into_iter()
                .filter(|(_, pane, _)| {
                    dragent_host::agent_sessions::load(*pane).is_some_and(|r| r.active)
                })
                .map(|(tab_id, pane, quiet)| {
                    (
                        tab_id,
                        quiet && dragent_host::agent_sessions::is_between_turns(pane),
                    )
                })
                .collect();
            let _ = tx.send(waiting);
        });
        ctx.spawn(rx, |view, waiting, ctx| {
            let Ok(waiting) = waiting else {
                return;
            };
            let mut still_waiting = false;
            for (tab_id, ready) in waiting {
                if ready {
                    view.restart_agent_for_theme(tab_id, ctx);
                } else {
                    still_waiting = true;
                }
            }
            if still_waiting {
                view.schedule_agent_theme_restarts(ctx);
            }
        });
    }

    /// Replace a running agent with a new process on the same conversation, in
    /// the same pane. One never given a prompt has no conversation to return to.
    #[cfg(not(target_family = "wasm"))]
    fn restart_agent_for_theme(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) {
        let Some(pane) = self
            .host_owned_tabs
            .get(&tab_id)
            .map(|owned| owned.pane.clone())
        else {
            return;
        };
        let used = pane.read(ctx, |view, _| {
            dragent_host::agent_sessions::load(view.pane_uuid).is_some_and(|record| record.used)
        });
        let plan = self.host_recovery_plan(&pane, ctx);
        let Some(command) = (if used { plan.resume } else { plan.fresh }) else {
            log::warn!("[host-tab] cannot restart {} for the new theme", plan.name);
            return;
        };
        let notice = format!("Restarting {} for the new theme…", plan.name);
        let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
            return;
        };
        let Some(session_id) = owned.session_id.take() else {
            return;
        };
        if let Some(cancel) = owned.stream.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        owned.slot.unbind();
        let workspace_id = owned.workspace_id.clone();
        let client = owned.client.clone();
        // The old process must be gone before the new one claims the record.
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            match client
                .map(Ok)
                .unwrap_or_else(dragent_host::HostClient::ensure_host)
                .and_then(|client| client.close_terminal(&workspace_id, &session_id))
            {
                Ok(_) => {}
                Err(error) => log::warn!("[host-tab] closing for theme restart failed: {error}"),
            }
            let _ = tx.send(());
        });
        ctx.spawn(rx, move |view, _, ctx| {
            ctx.update_view(&pane, |terminal, terminal_ctx| {
                let theme = terminal.theme;
                terminal.model.update(terminal_ctx, |model, model_ctx| {
                    model.end_running_command(&theme, model_ctx);
                });
                terminal.set_host_notice(Some(notice), terminal_ctx);
            });
            view.replace_pane_session(tab_id, Some(command), ctx);
        });
    }

    /// Close one host session after its tab is gone: detach the slot and
    /// dispose the daemon PTY. Quit never calls this — sessions survive it.
    pub(crate) fn close_host_tab_session(&mut self, tab_id: usize) {
        let Some(owned) = self.host_owned_tabs.remove(&tab_id) else {
            return;
        };
        if let Some(cancel) = owned.stream.as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
        owned.slot.detach();
        let (Some(workspace_id), Some(session_id)) = (
            (!owned.workspace_id.is_empty()).then(|| owned.workspace_id.clone()),
            owned.session_id.clone(),
        ) else {
            return;
        };
        let bound_client = owned.client;
        let remote_host = owned.remote_host;
        crate::bg::detach(move || {
            let Ok(client) = bound_client.map(Ok).unwrap_or_else(|| {
                if remote_host.is_some() {
                    Err("Remote session has no connected host authority".into())
                } else {
                    dragent_host::HostClient::ensure_host()
                }
            }) else {
                return;
            };
            crate::realtime::phone_control::forget(&session_id);
            if let Err(e) = client.close_terminal(&workspace_id, &session_id) {
                log::warn!("[host-tab] close session failed: {e}");
            }
        });
    }

    /// Re-attach restored tabs to their daemon sessions. Live sessions bind
    /// only after the daemon's terminal checkpoint has been applied. A
    /// missing agent PTY can be replaced only by an exact persisted provider
    /// conversation resume. Recovery never focuses or switches a surface.
    pub(crate) fn attach_restored_host_tabs(
        &mut self,
        ctx: &mut ViewContext<Self>,
        pending: Vec<(usize, HostReattach)>,
    ) {
        for (tab_id, reattach) in pending {
            if self
                .host_owned_tabs
                .get(&tab_id)
                .is_some_and(|tab| tab.session_id.is_some())
            {
                continue;
            }
            ctx.update_view(&reattach.pane, |view, ctx| {
                view.set_host_notice(
                    Some("Reconnecting to the original terminal session…".into()),
                    ctx,
                )
            });
            // Carry the failure count across retries of this same restore.
            let attempt = self
                .host_owned_tabs
                .get(&tab_id)
                .map_or(0, |tab| tab.bind_attempts);
            let started_over = self
                .host_owned_tabs
                .get(&tab_id)
                .is_some_and(|tab| tab.started_over);
            self.host_owned_tabs.insert(
                tab_id,
                HostOwnedTab {
                    client: None,
                    remote_host: reattach.binding.remote_host.clone(),
                    pane: reattach.pane.clone(),
                    workspace_id: reattach.binding.workspace_id.clone(),
                    home_id: reattach.binding.home_id.clone(),
                    session_id: None,
                    mirror: HostMirror::new(reattach.binding.session_id.clone()),
                    slot: reattach.slot.clone(),
                    root: reattach
                        .project_root
                        .clone()
                        .filter(|root| root.is_dir())
                        .unwrap_or_else(|| reattach.cwd.clone()),
                    cwd: reattach.cwd.clone(),
                    bind_attempts: attempt,
                    pending_launch_command: None,
                    stream: None,
                    started_over,
                },
            );
            let slot = reattach.slot.clone();
            let binding = reattach.binding.clone();
            let expect_agent = reattach.expect_agent;
            let restart_after_update = reattach.restart_after_update;
            let builtin_agent = reattach
                .pane
                .read(ctx, |view, _| view.agent_id.as_deref() == Some("dragent"));
            let cwd_str = reattach.cwd.to_string_lossy().into_owned();
            let shell = reattach.pane.read(ctx, |view, _| view.launch_shell.clone());
            let root_str = reattach
                .project_root
                .clone()
                .filter(|root| root.is_dir())
                .unwrap_or_else(|| reattach.cwd.clone())
                .to_string_lossy()
                .into_owned();
            // A remote project's terminal re-attaches on its machine once it
            // is connected (retries cover the reconnect).
            let stand_in = crate::remote_fs::resolve(std::path::Path::new(&root_str));
            if let Some(remote) = &stand_in {
                let machine = remote.machine_id.clone();
                reattach
                    .pane
                    .update(ctx, |view, _| view.set_remote_machine(Some(machine)));
            }
            ctx.spawn(
                async move {
                    let mut root_str = root_str;
                    let client = if let Some(remote) = &stand_in {
                        root_str = remote.path.clone();
                        crate::remote_fs::client(&remote.machine_id)?
                    } else if let Some(remote) = &binding.remote_host {
                        dragent_host::HostClient::connect_remote(serde_json::from_str(remote).map_err(|e| e.to_string())?)?
                    } else { dragent_host::HostClient::ensure_host()? };
                    let remote_session = stand_in.is_some() || binding.remote_host.is_some();
                    // A terminal is identified by its session id; the daemon
                    // keeps the workspace id it was created under across app
                    // and host restarts. Look the saved session up under that
                    // saved id — minting a fresh local workspace here would
                    // gate the lookup on an id no session has, so every
                    // restored tab came back unbound. A fresh workspace is
                    // only needed when there is no saved binding to adopt.
                    let workspace_id = if binding.workspace_id.is_empty() {
                        if remote_session { client.create_workspace(&root_str)? } else { client.create_local_workspace(&root_str)? }
                    } else {
                        binding.workspace_id.clone()
                    };
                    if restart_after_update && !remote_session {
                        // This RPC drives daemon replacement; do not launch a
                        // resumed process in the predecessor while it drains.
                        client.list_terminals(&workspace_id)?;
                        if !dragent_host::pty_supervisor::update_replacement_ready(&binding.session_id) {
                            return Err("Waiting for the updated terminal service".into());
                        }
                    }
                    let restored = if remote_session {
                        let sessions = client.list_terminals(&workspace_id)?;
                        let session = sessions.iter().find(|s| s.id == binding.session_id && matches!(s.state,dragent_host::TerminalState::Running));
                        dragent_host::agent_sessions::Restored {
                            home_id: session.map(|s| s.worktree_id.clone()).unwrap_or_else(|| binding.home_id.clone()),
                            terminal_id: session.map(|s| s.id.clone()).unwrap_or_default(),
                            resumed: false,
                            asleep: false,
                        }
                    } else { dragent_host::agent_sessions::restore_terminal(
                        &client,
                        dragent_host::agent_sessions::RestoreTerminal {
                            pane_uuid: binding.pane_uuid,
                            workspace_id: &workspace_id,
                            home_id: &binding.home_id,
                            terminal_id: &binding.session_id,
                            root: &root_str,
                            cwd: &cwd_str,
                            shell: shell.as_deref(),
                            // A saved id may point to a shell lost on reboot.
                            // Plain terminal panes always reopen a shell then.
                            allow_new_shell: builtin_agent || !expect_agent,
                            helper: dragent_host::agent_sessions::helper_executable().ok().as_deref(),
                        },
                    )? };
                    Ok::<_, String>((workspace_id, restored, client))
                },
                move |view, result, ctx| {
                    match result {
                        Ok((workspace_id, restored, client)) => {
                            slot.set_client(client.clone());
                            if let Some(tab) = view.host_owned_tabs.get_mut(&tab_id) { tab.client = Some(client.clone()); }
                            let dragent_host::agent_sessions::Restored {
                                home_id,
                                terminal_id: session_id,
                                resumed,
                                asleep,
                            } = restored;
                            if !view.host_owned_tabs.contains_key(&tab_id) {
                                slot.detach();
                                crate::bg::detach(move || {
                                    let _ = client.close_terminal(&workspace_id, &session_id);
                                });
                                return;
                            }
                            if asleep {
                                // Put to sleep before the restart: it stays
                                // asleep until its tab is opened.
                                if let Some(owned) = view.host_owned_tabs.get_mut(&tab_id) {
                                    owned.workspace_id = workspace_id;
                                    owned.home_id = home_id;
                                }
                                view.mark_tab_asleep(tab_id, ctx);
                                view.wake_visible_sleepers(ctx);
                                ctx.notify();
                                return;
                            }
                            if session_id.is_empty() {
                                log::warn!(
                                    "[host-tab] saved session is gone; leaving tab unbound tab={tab_id}"
                                );
                                if let Some(owned) = view.host_owned_tabs.get_mut(&tab_id) {
                                    owned.workspace_id = workspace_id;
                                    owned.home_id = home_id;
                                }
                                let pane = reattach.pane.clone();
                                let notice = view.lost_session_notice(&pane, ctx);
                                view.mark_host_unavailable(tab_id, &notice, ctx);
                                if !view.start_over_if_unused(tab_id, ctx) {
                                    view.start_missing_host_agent(tab_id, ctx);
                                }
                                ctx.notify();
                                return;
                            }
                            if let Some(owned) = view.host_owned_tabs.get_mut(&tab_id) {
                                owned.workspace_id = workspace_id;
                                owned.home_id = home_id;
                                owned.session_id = Some(session_id.clone());
                                owned.mirror = HostMirror::new(session_id);
                                owned.bind_attempts = 0;
                                let pane = owned.pane.clone();
                                // A reboot left no process: a new one is resuming
                                // the pane's recorded conversation.
                                let notice = if resumed {
                                    format!("Resuming {}…", view.host_recovery_plan(&pane, ctx).name)
                                } else {
                                    "Restoring terminal state…".to_string()
                                };
                                ctx.update_view(&pane, |tv, tctx| tv.set_host_notice(Some(notice), tctx));
                            }
                            view.ensure_host_tabs_pump(ctx);
                            view.save_session(ctx);
                        }
                        Err(error) if is_missing_folder(&error) => {
                            log::warn!("[host-tab] restore stopped tab={tab_id}: {error}");
                            let folder = reattach.project_root.as_ref().unwrap_or(&reattach.cwd);
                            view.mark_host_unavailable(tab_id, &missing_folder_notice(folder), ctx);
                        }
                        Err(error) => {
                            let delay = host_retry_delay_for(attempt, &error);
                            log::warn!(
                                "[host-tab] restore re-attach retry tab={tab_id} attempt={attempt} in {delay:?}: {error}"
                            );
                            if view.host_owned_tabs.contains_key(&tab_id) {
                                if let Some(owned) = view.host_owned_tabs.get_mut(&tab_id) {
                                    owned.bind_attempts = attempt.saturating_add(1);
                                    let pane = owned.pane.clone();
                                    ctx.update_view(&pane, |tv, tctx| {
                                        show_host_retry(tv, delay, &error, tctx)
                                    });
                                }
                                ctx.spawn(
                                    Timer::after(delay),
                                    move |view, _, ctx| {
                                        if view.host_owned_tabs.contains_key(&tab_id) {
                                            view.attach_restored_host_tabs(ctx, vec![(tab_id, reattach)]);
                                        }
                                    },
                                );
                            }
                        }
                    }
                    ctx.notify();
                },
            );
        }
    }

    /// Start an output-stream worker for every bound tab that lacks one.
    fn ensure_host_tabs_pump(&mut self, ctx: &mut ViewContext<Self>) {
        let tx = self.host_stream_sender(ctx);
        for (tab_id, owned) in self.host_owned_tabs.iter_mut() {
            if owned.stream.is_some() || owned.mirror.exited.is_some() || owned.mirror.ended {
                continue;
            }
            let Some(session_id) = owned.session_id.clone() else {
                continue;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            owned.stream = Some(cancel.clone());
            spawn_host_tab_stream(
                *tab_id,
                owned.pane.read(ctx, |view, _| view.pane_uuid),
                owned.workspace_id.clone(),
                session_id,
                owned.mirror.next_offset,
                owned.slot.clone(),
                owned.client.clone(),
                owned
                    .pane
                    .read(ctx, |view, _| view.host_output_sink(cancel.clone())),
                cancel,
                tx.clone(),
            );
        }
    }

    fn host_stream_sender(&mut self, ctx: &mut ViewContext<Self>) -> HostStreamSender {
        if let Some(tx) = self.host_stream_tx.as_ref() {
            return tx.clone();
        }
        let (tx, rx) = async_channel::bounded(STREAM_CHANNEL_CHUNKS);
        // Drained on the main thread: if it waited for a background thread,
        // busy background work would stall every terminal's output.
        ctx.spawn_stream_on_main_thread(
            rx,
            |view, (tab_id, session_id, msg): (usize, String, HostStreamMsg), ctx| {
                view.handle_host_tab_msg(tab_id, &session_id, msg, ctx)
            },
            |_, _| {},
        );
        self.host_stream_tx = Some(tx.clone());
        tx
    }

    /// The pane keeps its identity and history but has no usable session.
    /// Nothing replaces it until the user chooses an action.
    fn mark_host_unavailable(&mut self, tab_id: usize, notice: &str, ctx: &mut ViewContext<Self>) {
        let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
            return;
        };
        if let Some(cancel) = owned.stream.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        owned.mirror.ended = true;
        owned.session_id = None;
        owned.pending_launch_command = None;
        owned.slot.unbind();
        let pane = owned.pane.clone();
        let actions = self.host_recovery_plan(&pane, ctx).actions();
        let notice = notice.to_string();
        ctx.update_view(&pane, |tv, tctx| {
            tv.set_host_unavailable(notice, actions, tctx)
        });
    }

    /// Why a restored pane has no process: explicit per agent capability,
    /// never a silent replacement.
    fn lost_session_notice(
        &self,
        pane: &ViewHandle<TerminalView>,
        app: &warpui::AppContext,
    ) -> String {
        let plan = self.host_recovery_plan(pane, app);
        match plan.agent {
            Some(AgentResume::Unsupported) => format!(
                "{} can't resume a previous conversation after its process is lost. Start a new session to continue.",
                plan.name
            ),
            Some(AgentResume::Supported) if plan.resume.is_none() => format!(
                "No resumable {} conversation was recorded for this pane.",
                plan.name
            ),
            _ => "Session unavailable: its original process has ended.".to_string(),
        }
    }

    /// What can replace an unavailable session: resuming the pane's recorded
    /// agent conversation, and starting a new session (the recorded launch
    /// options without the conversation, else the agent's launch command,
    /// else what was running in the pane).
    pub(super) fn host_recovery_plan(
        &self,
        pane: &ViewHandle<TerminalView>,
        app: &warpui::AppContext,
    ) -> HostRecoveryPlan {
        let (running, agent_id) = pane.read(app, |view, app| {
            (view.last_running_command(app), view.agent_id.clone())
        });
        let settings = crate::setup::UiSettings::load();
        let definition = agent_id.as_deref().and_then(|id| {
            super::agent_definition::definition_by_id(id, &settings.custom_agents, false)
        });
        let launch = definition.as_ref().and_then(|definition| {
            // Recovery labels/actions run on the UI thread. Build the saved
            // launch line without probing npm or starting a login shell;
            // the owning host resolves it when the process is launched.
            super::action_workspace_tabs::agent_launch_line(definition, &settings, false)
        });
        #[cfg(not(target_family = "wasm"))]
        let (resume, recorded_fresh) = pane.read(app, |view, _| {
            let Some(mut record) = dragent_host::agent_sessions::load(view.pane_uuid) else {
                return (None, None);
            };
            let shell = view.launch_shell.as_deref().unwrap_or("sh");
            let fresh = dragent_host::agent_sessions::fresh_command(&record, shell);
            // A manual retry may resume a conversation whose previous attempt exited.
            record.active = true;
            let resume = dragent_host::agent_sessions::resume_command(&record, &record.cwd, shell);
            (resume, fresh)
        });
        #[cfg(target_family = "wasm")]
        let (resume, recorded_fresh): (Option<String>, Option<String>) = (None, None);
        let fresh = recorded_fresh.or(launch).or(running);
        let name = definition
            .as_ref()
            .map(|d| d.name.clone())
            .or_else(|| {
                fresh
                    .as_deref()
                    .and_then(|c| c.split_whitespace().next())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "agent".to_string());
        let agent = definition
            .as_ref()
            .filter(|d| d.builtin != Some(super::action::PromptAgent::Dragent))
            .map(|d| {
                if d.supports_resume() {
                    AgentResume::Supported
                } else {
                    AgentResume::Unsupported
                }
            });
        HostRecoveryPlan {
            name,
            agent,
            resume,
            fresh,
        }
    }

    /// Replace an unavailable session in the same pane with a new one in the
    /// same directory: resume the recorded conversation, or with `fresh`
    /// start a new one.
    pub(crate) fn restart_host_session(
        &mut self,
        pane_uuid: uuid::Uuid,
        fresh: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(tab_id) = self.host_owned_tabs.iter().find_map(|(id, owned)| {
            (owned.pane.read(ctx, |view, _| view.pane_uuid) == pane_uuid
                && owned.session_id.is_none()
                && owned.mirror.ended)
                .then_some(*id)
        }) else {
            return;
        };
        let Some(pane) = self
            .host_owned_tabs
            .get(&tab_id)
            .map(|owned| owned.pane.clone())
        else {
            return;
        };
        let plan = self.host_recovery_plan(&pane, ctx);
        let (notice, command) = match plan.resume {
            Some(resume) if !fresh => (format!("Resuming {}…", plan.name), Some(resume)),
            _ if plan.agent.is_some() => {
                (format!("Starting a new {} session…", plan.name), plan.fresh)
            }
            _ => ("Starting a new session…".to_string(), plan.fresh),
        };
        if let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) {
            owned.started_over = false;
        }
        self.relaunch_in_pane(tab_id, command, notice, ctx);
    }

    /// The pane's agent could not be resumed, but its conversation never got
    /// a prompt: start the agent again rather than report a failure. Once per
    /// pane per app run, so a new session that also fails is shown instead.
    fn start_over_if_unused(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) -> bool {
        let Some(pane) = self
            .host_owned_tabs
            .get(&tab_id)
            .filter(|owned| !owned.started_over && owned.session_id.is_none())
            .map(|owned| owned.pane.clone())
        else {
            return false;
        };
        #[cfg(not(target_family = "wasm"))]
        let command = pane.read(ctx, |view, _| {
            dragent_host::agent_sessions::load(view.pane_uuid)
                .filter(|record| record.nothing_to_resume())
                .and_then(|record| {
                    dragent_host::agent_sessions::start_over_command(
                        &record,
                        view.launch_shell.as_deref().unwrap_or("sh"),
                    )
                })
        });
        #[cfg(target_family = "wasm")]
        let command: Option<String> = None;
        let Some(command) = command else {
            return false;
        };
        let name = self.host_recovery_plan(&pane, ctx).name;
        if let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) {
            owned.started_over = true;
        }
        log::info!("[host-tab] previous conversation was never used; starting over tab={tab_id}");
        self.relaunch_in_pane(
            tab_id,
            Some(command),
            format!(
                "Starting a new {name} session: the previous one had no conversation to resume."
            ),
            ctx,
        );
        true
    }

    /// An agent pane without a resumable conversation reopens the saved
    /// agent command once. A failed exact resume keeps its recovery actions.
    fn start_missing_host_agent(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) -> bool {
        let Some(pane) = self
            .host_owned_tabs
            .get(&tab_id)
            .filter(|owned| !owned.started_over && owned.session_id.is_none())
            .map(|owned| owned.pane.clone())
        else {
            return false;
        };
        let plan = self.host_recovery_plan(&pane, ctx);
        if plan.agent.is_none() || plan.resume.is_some() {
            return false;
        }
        let Some(command) = plan.fresh else {
            return false;
        };
        if let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) {
            owned.started_over = true;
        }
        log::info!("[host-tab] restarting agent without a saved conversation tab={tab_id}");
        self.relaunch_in_pane(
            tab_id,
            Some(command),
            format!("Starting {}…", plan.name),
            ctx,
        );
        true
    }

    /// Bind a new session in the pane's directory and submit `command` there.
    fn relaunch_in_pane(
        &mut self,
        tab_id: usize,
        command: Option<String>,
        notice: String,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(pane) = self.host_owned_tabs.get(&tab_id).map(|owned| owned.pane.clone()) else {
            return;
        };
        ctx.update_view(&pane, |tv, tctx| {
            tv.set_host_unavailable(notice, Vec::new(), tctx)
        });
        self.replace_pane_session(tab_id, command, ctx);
    }

    pub(super) fn replace_pane_session(
        &mut self,
        tab_id: usize,
        command: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
            return;
        };
        owned.pending_launch_command = command;
        // A pane that lost its session drops input (`unbind`). The command
        // is submitted as soon as the new session exists, which is before it
        // binds, so without this it was dropped too: the pane showed it
        // running while the shell sat at an empty prompt.
        owned.slot.await_bind();
        owned.mirror = HostMirror::new(String::new());
        owned.bind_attempts = 0;
        let (slot, root, cwd) = (owned.slot.clone(), owned.root.clone(), owned.cwd.clone());
        self.bind_new_host_tab(ctx, tab_id, slot, root, cwd, 0);
        self.save_session(ctx);
    }

    /// Reuse startup recovery in-place for panes whose transport was lost.
    /// Keep pane UUID, directory, saved conversation, layout, and focus.
    fn recover_host_tab(
        &mut self,
        tab_id: usize,
        restart_after_update: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
            return;
        };
        let pane = owned.pane.clone();
        let Some(session_id) = owned.session_id.take() else {
            return;
        };
        if let Some(cancel) = owned.stream.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        owned.slot.unbind();
        let reattach = HostReattach {
            pane: pane.clone(),
            slot: owned.slot.clone(),
            binding: dragent_persistence::SerializedHostTerminalBinding {
                remote_host: owned.remote_host.clone(),
                pane_uuid: pane.read(ctx, |view, _| view.pane_uuid),
                workspace_id: owned.workspace_id.clone(),
                home_id: owned.home_id.clone(),
                session_id,
                agent: pane.read(ctx, |view, _| {
                    view.tab_session_kind == TabSessionKind::Agent
                }),
            },
            cwd: owned.cwd.clone(),
            project_root: Some(owned.root.clone()),
            expect_agent: pane.read(ctx, |view, _| {
                view.agent_id.is_some() || view.tab_session_kind == TabSessionKind::Agent
            }),
            restart_after_update,
        };
        log::info!(
            "[host-tab] recovering terminal session tab={tab_id} update={restart_after_update}"
        );
        self.attach_restored_host_tabs(ctx, vec![(tab_id, reattach)]);
    }

    /// Missing sessions normally stay unavailable; update shutdowns use the
    /// same exact-conversation recovery path as application startup.
    fn handle_host_tab_msg(
        &mut self,
        tab_id: usize,
        session_id: &str,
        msg: HostStreamMsg,
        ctx: &mut ViewContext<Self>,
    ) {
        let stale = self
            .host_owned_tabs
            .get(&tab_id)
            .is_none_or(|owned| owned.session_id.as_deref() != Some(session_id));
        if stale {
            return;
        }
        match msg {
            HostStreamMsg::Parsed {
                next_offset,
                exit_code,
            } => {
                let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
                    return;
                };
                owned.mirror.next_offset = next_offset;
                owned.mirror.failures = 0;
                let pane = owned.pane.clone();
                ctx.update_view(&pane, |tv, tctx| tv.set_host_notice(None, tctx));
                if let Some(code) = exit_code {
                    owned.mirror.exited = Some(code);
                    owned.stream = None;
                    let pane = owned.pane.clone();
                    ctx.update_view(&pane, |tv, tctx| tv.mark_remote_exited(code, tctx));
                }
            }
            HostStreamMsg::Rebound(client) => {
                if let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) {
                    owned.client = Some(client);
                }
            }
            HostStreamMsg::RestartedForUpdate => self.recover_host_tab(tab_id, true, ctx),
            // Confirmed by the host; the worker has stopped.
            HostStreamMsg::Gone => self.recover_host_tab(tab_id, false, ctx),
            HostStreamMsg::AgentResumeFailed => {
                let plain_terminal = self.host_owned_tabs.get(&tab_id).is_some_and(|owned| {
                    owned.pane.read(ctx, |view, _| view.agent_id.is_none()
                        && view.tab_session_kind != TabSessionKind::Agent)
                });
                if plain_terminal {
                    // A rejected command must not leave an ordinary terminal
                    // unusable. Recovery opens a shell for its inactive record.
                    self.recover_host_tab(tab_id, false, ctx);
                    ctx.notify();
                    return;
                }
                self.mark_host_unavailable(
                    tab_id,
                    "Unable to resume previous session. Check the error above, then retry or start a new session.",
                    ctx,
                );
                self.start_over_if_unused(tab_id, ctx);
            }
            HostStreamMsg::RestoreFailed => self.mark_host_unavailable(
                tab_id,
                "Session could not be reattached safely because its terminal state is unavailable. No replacement was started.",
                ctx,
            ),
            // The worker keeps retrying on its own.
            HostStreamMsg::Unavailable => {
                if let Some(owned) = self.host_owned_tabs.get(&tab_id) {
                    let pane = owned.pane.clone();
                    ctx.update_view(&pane, |tv, tctx| tv.set_host_notice(Some("Reconnecting to the original terminal session…".into()), tctx));
                }
            }
        }
        ctx.notify();
    }
}

/// One tab's output stream: apply decoded operations off the UI thread, and
/// reconnect from the last offset (with backoff) when the stream drops.
/// Stops after the exit message, when the host confirms the session is gone,
/// when cancelled, or when the view is gone.
fn spawn_host_tab_stream(
    tab_id: usize,
    pane_uuid: uuid::Uuid,
    workspace_id: String,
    session_id: String,
    mut offset: u64,
    slot: Arc<HostPtyBinding>,
    authority: Option<dragent_host::HostClient>,
    feed: impl Fn(&dragent_host::TerminalChunk) -> bool + Send + 'static,
    cancel: Arc<AtomicBool>,
    tx: HostStreamSender,
) {
    crate::bg::detach(move || {
        let mut mirror = HostMirror::new(session_id.clone());
        mirror.next_offset = offset;
        let mut attempt = 0u32;
        // Set once the stream dropped: a local host may have restarted under
        // a new endpoint, which the client the tab was bound with doesn't know.
        let mut dropped = false;
        let mut authority = authority;
        let send = |msg: HostStreamMsg| tx.send_blocking((tab_id, session_id.clone(), msg)).is_ok();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let found = match authority.as_ref() {
                Some(bound) if !dropped => Ok(bound.clone()),
                Some(bound) => bound.reconnected().inspect(|fresh| {
                    if fresh.endpoint().token != bound.endpoint().token {
                        // Input and the tab's other requests follow it too.
                        slot.set_client(fresh.clone());
                        let _ = send(HostStreamMsg::Rebound(fresh.clone()));
                    }
                }),
                None => dragent_host::HostClient::ensure_host(),
            };
            if let (Ok(fresh), Some(_)) = (&found, authority.as_ref()) {
                authority = Some(fresh.clone());
            }
            let client = match found {
                Ok(client) => client,
                Err(_) => {
                    if !send(HostStreamMsg::Unavailable) {
                        return;
                    }
                    std::thread::sleep(stream_retry_delay(attempt));
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            };
            let mut exited = false;
            let mut view_gone = false;
            let mut restore_failed = false;
            let mut resume_failed = false;
            let result =
                client.stream_terminal_operations(&workspace_id, &session_id, offset, |chunk| {
                    if cancel.load(Ordering::Relaxed) {
                        return false;
                    }
                    attempt = 0;
                    if chunk.resume.is_some() {
                        if !feed(&chunk) { restore_failed = true; return false; }
                        mirror.next_offset = chunk.base;
                        slot.bind(&workspace_id, &session_id);
                    }
                    let unseen = mirror.apply_chunk(chunk.base, chunk.total, &chunk.data);
                    if unseen.gap {
                        log::error!("[host-tab] gap without checkpoint tab={tab_id} session={session_id}");
                        return false;
                    }
                    if !unseen.bytes.is_empty() {
                        // Operation frames are atomic. Never apply a whole decoded
                        // batch to only the unseen suffix of its source bytes.
                        let Some(_operations) = chunk.operations.as_deref() else {
                            return false;
                        };
                        if unseen.bytes.len() != chunk.data.len() {
                            log::error!("[host-tab] unaligned operation replay tab={tab_id}");
                            return false;
                        }
                        if !feed(&chunk) {
                            return false;
                        }
                    }
                    // Read the durable exit result only at attach or a shell
                    // completion boundary, never on every output/scroll frame.
                    let completed = chunk.operations.as_ref().is_some_and(|checkpoint| {
                        checkpoint.iter().any(|op| {
                            matches!(op, dragent_terminal::operations::Operation::Osc(bytes)
                                if bytes.starts_with(b"7979;precmd;") || bytes.starts_with(b"133;D"))
                        })
                    });
                    if (chunk.resume.is_some() || completed)
                        && dragent_host::agent_sessions::load(pane_uuid).is_some_and(|r|
                            r.terminal_id == session_id && r.workspace_id == workspace_id && r.resume_failed()) {
                        resume_failed = true;
                        let _ = client.close_terminal(&workspace_id, &session_id);
                        let _ = send(HostStreamMsg::AgentResumeFailed);
                        return false;
                    }
                    offset = mirror.next_offset;
                    exited = chunk.exit_code.is_some();
                    if exited && dragent_host::pty_supervisor::session_interrupted_by_update(&session_id) {
                        let _ = send(HostStreamMsg::RestartedForUpdate);
                        return false;
                    }
                    if !send(HostStreamMsg::Parsed {
                        next_offset: offset,
                        exit_code: chunk.exit_code,
                    }) {
                        view_gone = true;
                        return false;
                    }
                    true
                });
            if restore_failed
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.contains("terminal state unavailable"))
            {
                let _ = send(HostStreamMsg::RestoreFailed);
                return;
            }
            if resume_failed || exited || view_gone || cancel.load(Ordering::Relaxed) {
                return;
            }
            // The stream ended without an exit: is the session gone for good,
            // or was the host just unreachable?
            let gone = result.is_err()
                && client.list_terminals(&workspace_id).is_ok_and(|sessions| {
                    !sessions.iter().any(|s| {
                        s.id == session_id
                            && matches!(s.state, dragent_host::TerminalState::Running)
                    })
                });
            if gone {
                let message =
                    if dragent_host::pty_supervisor::session_interrupted_by_update(&session_id) {
                        HostStreamMsg::RestartedForUpdate
                    } else {
                        HostStreamMsg::Gone
                    };
                let _ = send(message);
                return;
            }
            if !send(HostStreamMsg::Unavailable) {
                return;
            }
            dropped = true;
            std::thread::sleep(stream_retry_delay(attempt));
            attempt = attempt.saturating_add(1);
        }
    });
}

/// Reconnect backoff for a dropped stream: 250 ms doubling to 5 s.
fn stream_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis((250u64 << attempt.min(5)).min(5_000))
}

/// Backoff between attempts to bind a tab to the host: 1 s, 2 s, 4 s, 8 s,
/// then every 15 s. The tab stays host-backed the whole time.
fn host_retry_delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(4)).min(15))
}

/// While a remote tab's machine isn't connected, look again every two
/// seconds: it's a local lookup, so the tab attaches as soon as the machine
/// connects (it never connects the machine itself).
fn host_retry_delay_for(attempt: u32, error: &str) -> Duration {
    if error == crate::remote_fs::NOT_CONNECTED {
        Duration::from_secs(2)
    } else {
        host_retry_delay(attempt)
    }
}

/// Retrying can't bring a deleted folder back, and a tab that keeps retrying
/// never takes input. It stops and offers its recovery actions instead.
fn is_missing_folder(error: &str) -> bool {
    error.starts_with("project root unavailable")
}

fn missing_folder_notice(folder: &std::path::Path) -> String {
    format!(
        "This terminal's folder can't be found: {}. Restore it and start a new session, or close this tab.",
        folder.display()
    )
}

/// The tab's notice while it retries: waiting for its machine is not a
/// failure, so it isn't shown as one.
fn show_host_retry(
    tv: &mut crate::terminal::TerminalView,
    delay: Duration,
    error: &str,
    ctx: &mut warpui::ViewContext<crate::terminal::TerminalView>,
) {
    if error == crate::remote_fs::NOT_CONNECTED {
        tv.set_host_notice(
            Some(
                "Waiting for this machine to connect… If it doesn't, check it in Settings → Machines."
                    .into(),
            ),
            ctx,
        );
    } else {
        tv.set_host_error_notice(host_retry_notice(delay, error), ctx);
    }
}

fn host_retry_notice(delay: Duration, error: &str) -> String {
    format!(
        "Terminal connection failed: {}. Retrying in {}s…",
        error.trim_end_matches('.'),
        delay.as_secs().max(1)
    )
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn stream_retry_backs_off_and_caps() {
        let ms: Vec<u128> = (0..8).map(|n| stream_retry_delay(n).as_millis()).collect();
        assert_eq!(ms, vec![250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
    }

    #[test]
    fn host_retry_backs_off_and_caps() {
        let secs: Vec<u64> = (0..7).map(|n| host_retry_delay(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 15, 15, 15]);
        // Waiting for a remote tab's machine polls a local lookup, briefly.
        let waiting: Vec<u64> = (0..7)
            .map(|n| host_retry_delay_for(n, crate::remote_fs::NOT_CONNECTED).as_secs())
            .collect();
        assert_eq!(waiting, vec![2; 7]);
        assert_eq!(host_retry_delay_for(3, "refused").as_secs(), 8);
        assert_eq!(
            host_retry_notice(Duration::from_secs(4), "It went away."),
            "Terminal connection failed: It went away. Retrying in 4s…"
        );
    }

    #[test]
    fn only_a_missing_folder_stops_retrying() {
        assert!(is_missing_folder(
            "project root unavailable: The system cannot find the path specified. (os error 3)"
        ));
        assert!(!is_missing_folder("refused"));
        assert!(!is_missing_folder(crate::remote_fs::NOT_CONNECTED));
    }
}

/// Whether the pane's agent can resume its conversation exactly.
/// How often agents waiting to restart for a new theme are checked.
const AGENT_THEME_RESTART_POLL: Duration = Duration::from_secs(1);
/// Nothing sent to the terminal for this long: the user isn't typing in it.
const AGENT_THEME_RESTART_QUIET: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentResume {
    Supported,
    Unsupported,
}

/// Replacement options for a pane whose host session is unavailable.
pub(super) struct HostRecoveryPlan {
    /// Agent (or command) name for labels.
    pub(super) name: String,
    /// `None` for plain terminals and the built-in Dragent agent.
    pub(super) agent: Option<AgentResume>,
    /// Command resuming the recorded conversation.
    pub(super) resume: Option<String>,
    /// Command starting a new session in the pane.
    pub(super) fresh: Option<String>,
}

impl HostRecoveryPlan {
    fn actions(&self) -> Vec<HostRecoveryAction> {
        let mut actions = Vec::new();
        if self.resume.is_some() {
            actions.push(HostRecoveryAction {
                label: "Retry Resume".into(),
                fresh: false,
            });
        }
        let label = match (&self.agent, &self.fresh) {
            (Some(_), _) => "Start New Session".to_string(),
            (None, Some(command)) => format!(
                "Restart {}",
                command
                    .split_whitespace()
                    .next()
                    .unwrap_or(command.as_str())
            ),
            (None, None) => "Start a new session".to_string(),
        };
        actions.push(HostRecoveryAction { label, fresh: true });
        actions
    }
}
