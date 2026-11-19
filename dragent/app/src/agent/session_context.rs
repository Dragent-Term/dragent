use crate::agent::api::session::{AgentSessionContext, AttachedBlock, BlockContextSnapshot};
use crate::agent::project_context::ProjectContext;
use crate::terminal::TerminalModel;
use std::path::{Path, PathBuf};
use warpui::ModelHandle;
use warpui::ReadModel;

pub struct AgentSessionContextBuilder {
    cwd: PathBuf,
    is_autodetected: bool,
    project: Option<ProjectContext>,
    attached_blocks: Vec<AttachedBlock>,
}

impl AgentSessionContextBuilder {
    pub fn new(cwd: PathBuf, is_autodetected: bool) -> Self {
        Self {
            cwd,
            is_autodetected,
            project: None,
            attached_blocks: Vec::new(),
        }
    }

    pub fn with_project(mut self, project: Option<ProjectContext>) -> Self {
        self.project = project;
        self
    }

    pub fn with_attached_blocks(mut self, attached: Vec<AttachedBlock>) -> Self {
        self.attached_blocks = attached;
        self
    }

    pub fn build(
        self,
        model: &ModelHandle<TerminalModel>,
        ctx: &impl ReadModel,
    ) -> AgentSessionContext {
        let mut session = crate::agent::api::session::session_from_terminal(
            model,
            ctx,
            self.is_autodetected,
            self.project,
        );

        session.cwd = self.cwd;
        session.attached_blocks = self.attached_blocks;

        session
    }
}
