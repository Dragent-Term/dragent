//! Workspace pane wrapper — terminal sessions, browsers, and settings.

use std::path::PathBuf;

use uuid::Uuid;

use warpui::fonts::FamilyId;
use warpui::{EntityId, ReadView, UpdateView, ViewContext, ViewHandle};

use crate::agent::TabSessionKind;
use crate::browser::BrowserView;
use crate::console::ai_toolbar::AiTool;
use crate::settings::SettingsView;
use crate::terminal::TerminalView;
use crate::theme::Theme;

use super::view::WorkspaceView;
use dragent_persistence::{PersistedAgentBlock, PersistedBlock};
use dragent_workspace_model::{Project, SurfaceDescriptor, SurfaceId, SurfaceKind};

/// Fixed UUID — settings panes are not persisted to session storage.
const SETTINGS_PANE_UUID: Uuid = Uuid::from_u128(0);

#[derive(Clone)]
pub enum WorkspacePane {
    /// Production terminal stack — no extra chrome wrapper.
    Terminal(ViewHandle<TerminalView>),
    /// In-tab settings (sidebar nav + content). One shared view per workspace.
    Settings(ViewHandle<SettingsView>),
    /// A web page, opened and arranged like a terminal.
    Browser(ViewHandle<BrowserView>),
}

impl WorkspacePane {
    pub fn id(&self) -> EntityId {
        match self {
            WorkspacePane::Terminal(pane) => pane.id(),
            WorkspacePane::Settings(pane) => pane.id(),
            WorkspacePane::Browser(pane) => pane.id(),
        }
    }

    /// Stable product identity for this live surface. Terminal panes retain
    /// their existing durable UUID so adopting generic layouts does not create
    /// a second terminal or lose restored scrollback/process state.
    pub fn surface_id<C: ReadView>(&self, ctx: &C) -> SurfaceId {
        match self {
            WorkspacePane::Terminal(_) => SurfaceId::from_terminal_pane(self.pane_uuid(ctx)),
            WorkspacePane::Settings(_) => {
                SurfaceDescriptor::application_singleton(SurfaceKind::Settings).id
            }
            WorkspacePane::Browser(pane) => pane.read(ctx, |p, _| p.surface_descriptor().id),
        }
    }

    pub fn surface_descriptor<C: ReadView>(&self, ctx: &C) -> SurfaceDescriptor {
        match self {
            WorkspacePane::Terminal(_) => {
                let pane_id = self.pane_uuid(ctx);
                let project_id = Project::new_for_organisation(&dragent_persistence::current_org(), self.cwd(ctx), 0).id;
                SurfaceDescriptor::migrated_terminal(project_id, pane_id)
            }
            WorkspacePane::Settings(_) => {
                SurfaceDescriptor::application_singleton(SurfaceKind::Settings)
            }
            WorkspacePane::Browser(pane) => pane.read(ctx, |p, _| p.surface_descriptor()),
        }
    }

    pub fn is_settings(&self) -> bool {
        matches!(self, WorkspacePane::Settings(_))
    }

    pub fn is_browser(&self) -> bool {
        matches!(self, WorkspacePane::Browser(_))
    }

    #[cfg(test)]
    pub fn spawn_new(
        ctx: &mut ViewContext<WorkspaceView>,
        _theme: Theme,
        _ui_font: FamilyId,
        cwd: PathBuf,
    ) -> Self {
        Self::spawn_terminal(
            ctx,
            cwd,
            Uuid::new_v4(),
            None,
            0.0,
            None,
            false,
            Vec::new(),
            Vec::new(),
            TabSessionKind::Shell,
            None,
        )
    }

    /// Like [`spawn_new`] but runs a specific shell (full path) chosen from the
    /// new-tab picker instead of the resolved default.
    #[cfg(test)]
    pub fn spawn_new_with_shell(
        ctx: &mut ViewContext<WorkspaceView>,
        _theme: Theme,
        _ui_font: FamilyId,
        cwd: PathBuf,
        shell: String,
    ) -> Self {
        Self::spawn_terminal(
            ctx,
            cwd,
            Uuid::new_v4(),
            None,
            0.0,
            None,
            false,
            Vec::new(),
            Vec::new(),
            TabSessionKind::Shell,
            Some(shell),
        )
    }

    #[cfg(test)]
    pub fn spawn_new_as_agent(
        ctx: &mut ViewContext<WorkspaceView>,
        _theme: Theme,
        _ui_font: FamilyId,
        cwd: PathBuf,
    ) -> Self {
        Self::spawn_terminal(
            ctx,
            cwd,
            Uuid::new_v4(),
            None,
            0.0,
            None,
            false,
            Vec::new(),
            Vec::new(),
            TabSessionKind::Agent,
            None,
        )
    }

    pub fn spawn_settings(settings: ViewHandle<SettingsView>) -> Self {
        Self::Settings(settings)
    }

    #[cfg(test)]
    pub fn spawn_restore(
        ctx: &mut ViewContext<WorkspaceView>,
        _theme: Theme,
        _ui_font: FamilyId,
        cwd: PathBuf,
        uuid: Uuid,
        shell_title: Option<String>,
        clear_gap_height_px: f32,
        clear_gap_before_block_id: Option<i32>,
        clear_gap_after_all: bool,
        restored_blocks: Vec<PersistedBlock>,
        restored_agent_blocks: Vec<PersistedAgentBlock>,
    ) -> Self {
        Self::spawn_terminal(
            ctx,
            cwd,
            uuid,
            shell_title,
            clear_gap_height_px,
            clear_gap_before_block_id,
            clear_gap_after_all,
            restored_blocks,
            restored_agent_blocks,
            TabSessionKind::Shell,
            None,
        )
    }

    #[cfg(test)]
    fn spawn_terminal(
        ctx: &mut ViewContext<WorkspaceView>,
        cwd: PathBuf,
        uuid: Uuid,
        shell_title: Option<String>,
        clear_gap_height_px: f32,
        clear_gap_before_block_id: Option<i32>,
        clear_gap_after_all: bool,
        restored_blocks: Vec<PersistedBlock>,
        restored_agent_blocks: Vec<PersistedAgentBlock>,
        tab_kind: TabSessionKind,
        preferred_shell: Option<String>,
    ) -> Self {
        let pane = ctx.add_typed_action_view(move |terminal_ctx| {
            let mut view = TerminalView::new_with_restore(
                cwd,
                uuid,
                shell_title,
                clear_gap_height_px,
                clear_gap_before_block_id,
                clear_gap_after_all,
                restored_blocks,
                restored_agent_blocks,
                preferred_shell,
                terminal_ctx,
            );
            if tab_kind == TabSessionKind::Agent {
                view.set_tab_session_kind(TabSessionKind::Agent);
            }
            view
        });
        WorkspacePane::Terminal(pane)
    }

    /// A shared-session MIRROR pane: a remote `TerminalView` (no local shell) fed
    /// by the host's broadcast output, with input forwarded to the host via `sink`.
    pub fn spawn_remote(
        ctx: &mut ViewContext<WorkspaceView>,
        cwd: PathBuf,
        sink: crate::terminal::manager::pty_writer::RemoteInputSink,
    ) -> Self {
        let pane = ctx.add_typed_action_view(move |terminal_ctx| {
            TerminalView::new_remote(cwd, sink, terminal_ctx)
        });
        WorkspacePane::Terminal(pane)
    }

    /// A host-OWNED tab pane: no local shell is ever spawned. Output arrives
    /// via the workspace's host-tab pump (`feed_remote_output`); input and
    /// resize route to the daemon's PTY through `binding`. The session — and
    /// any servers it runs — outlives the app process.
    pub fn spawn_host_pending(
        ctx: &mut ViewContext<WorkspaceView>,
        cwd: PathBuf,
        binding: std::sync::Arc<crate::terminal::manager::pty_writer::HostPtyBinding>,
    ) -> Self {
        let pane = ctx.add_typed_action_view(move |terminal_ctx| {
            TerminalView::new_host(cwd, binding, terminal_ctx)
        });
        WorkspacePane::Terminal(pane)
    }

    pub fn focus(&self, ctx: &mut ViewContext<WorkspaceView>) {
        match self {
            WorkspacePane::Terminal(pane) => ctx.focus(pane),
            WorkspacePane::Settings(pane) => ctx.focus(pane),
            WorkspacePane::Browser(pane) => ctx.focus(pane),
        }
    }

    pub fn set_theme(&self, theme: Theme, ctx: &mut ViewContext<WorkspaceView>) {
        match self {
            WorkspacePane::Terminal(pane) => {
                ctx.update_view(pane, |p, pane_ctx| p.set_theme(theme, pane_ctx));
            }
            WorkspacePane::Browser(pane) => {
                ctx.update_view(pane, |p, pane_ctx| p.set_theme(theme, pane_ctx));
            }
            WorkspacePane::Settings(_) => {}
        }
    }

    /// Tell a pane whether it is currently rendered by the workspace. This
    /// changes visual scheduling only; terminal processes and agent tasks keep
    /// running regardless of tab visibility.
    pub fn set_workspace_active(&self, active: bool, ctx: &mut ViewContext<WorkspaceView>) {
        match self {
            WorkspacePane::Terminal(pane) => {
                ctx.update_view(pane, |view, pane_ctx| {
                    view.set_workspace_active(active, pane_ctx)
                });
            }
            WorkspacePane::Browser(pane) => {
                ctx.update_view(pane, |view, pane_ctx| view.set_active(active, pane_ctx));
            }
            WorkspacePane::Settings(_) => {}
        }
    }

    pub fn pane_uuid<C: ReadView>(&self, ctx: &C) -> Uuid {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, _| p.pane_uuid()),
            WorkspacePane::Settings(_) => SETTINGS_PANE_UUID,
            WorkspacePane::Browser(pane) => pane.read(ctx, |p, _| p.pane_uuid()),
        }
    }

    pub fn cwd<C: ReadView>(&self, ctx: &C) -> PathBuf {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.cwd(c)),
            WorkspacePane::Settings(_) => PathBuf::new(),
            // A browser has no working directory; its project scope stands in.
            WorkspacePane::Browser(pane) => {
                pane.read(ctx, |p, _| p.project_root().unwrap_or_default())
            }
        }
    }

    pub fn shell_title<C: ReadView>(&self, ctx: &C) -> Option<String> {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.shell_title(c)),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        }
    }

    pub fn last_command<C: ReadView>(&self, ctx: &C) -> Option<String> {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.last_command(c)),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        }
    }

    /// A browser's site icon on disk, for its tab.
    pub fn browser_icon<C: ReadView>(&self, ctx: &C) -> Option<String> {
        match self {
            WorkspacePane::Browser(pane) => pane.read(ctx, |p, _| {
                p.favicon_path().map(|path| path.to_string_lossy().into_owned())
            }),
            _ => None,
        }
    }

    pub fn display_title<C: ReadView>(&self, ctx: &C) -> String {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.display_title(c)),
            WorkspacePane::Settings(_) => "Settings".to_string(),
            WorkspacePane::Browser(pane) => pane.read(ctx, |p, _| p.title()),
        }
    }

    pub fn is_running<C: ReadView>(&self, _ctx: &C) -> bool {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(_ctx, |p, c| p.is_running(c)),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => false,
        }
    }

    pub fn active_ai_tool<C: ReadView>(&self, ctx: &C) -> Option<AiTool> {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.active_ai_tool(c)),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        }
    }

    /// Shell vs agent launch kind. Terminal panes launched as agent tabs
    /// (or promoted by running an agent command) report `Agent` even when
    /// no specific CLI tool is currently detected.
    pub fn tab_session_kind<C: ReadView>(&self, ctx: &C) -> TabSessionKind {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, _| p.tab_session_kind),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => TabSessionKind::Shell,
        }
    }

    /// Stable agent-registry identity for this session, if launched as an
    /// agent. Survives restart via pane persistence (unlike detection,
    /// which would have to re-derive it from the process).
    pub fn agent_id<C: ReadView>(&self, ctx: &C) -> Option<String> {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, _| p.agent_id.clone()),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        }
    }

    pub fn set_agent_id<C: UpdateView>(&self, ctx: &mut C, agent_id: Option<String>) {
        if let WorkspacePane::Terminal(pane) = self {
            pane.update(ctx, |p, _| p.set_agent_id(agent_id));
        }
    }

    pub fn active_clear_gap<C: ReadView>(
        &self,
        ctx: &C,
    ) -> Option<crate::terminal::model::terminal_session_core::ActiveClearGap> {
        match self {
            WorkspacePane::Terminal(pane) => pane.read(ctx, |p, c| p.active_clear_gap(c)),
            WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
        }
    }
}
