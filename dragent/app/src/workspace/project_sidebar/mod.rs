pub(crate) mod detect;
mod model;
mod sidebar_actions;
pub(crate) mod sidebar_diff_comments;
mod sidebar_construct;
mod sidebar_document;
mod sidebar_documents;
mod sidebar_explorer;
#[cfg(test)]
pub(crate) use sidebar_explorer::render_documents_section;
mod sidebar_gitdiff_file;
mod sidebar_gitdiff_multi;
mod sidebar_githistory;
mod sidebar_gitstatus;
mod sidebar_panels;
mod sidebar_review;
mod sidebar_search;
mod sidebar_search_panel;
mod view;

pub use crate::console::git_status::GitStatus;
pub(crate) use detect::find_git_root;
pub use model::{
    BlockSnapshot, ProjectSidebarModel, ReviewCheck, ReviewCheckStatus, ReviewComment,
    ReviewPrState, ReviewPullRequest, SidebarEvent,
};
pub use view::{LanguageLocationItem, ProjectSidebarView};
pub(crate) use sidebar_documents::render_buffer_search_bar;

use warpui::EntityId;

/// Which surface the right sidebar is showing. The sidebar is the workspace's
/// primary dock, so it hosts both the file explorer and the source-control
/// workspace behind one tab strip — the same information hierarchy VS Code
/// uses for its explorer/scm switch, without a second dock column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SidebarTab {
    #[default]
    Files,
    Search,
    SourceControl,
    Review,
}

/// Live wiring for the sidebar's source-control panel. The git status model
/// is owned by the active terminal session (single source of truth), falling
/// back to a workspace-owned model in folder-only state; the panel reads it
/// at render time and subscribes to it so diagnostics and file states update
/// the moment the model refreshes.
#[derive(Clone, Default)]
pub struct GitPanelState {
    pub git_model: Option<warpui::ModelHandle<crate::git_status::GitRepoStatusModel>>,
    /// Active session that owns the git model — panel actions dispatch here.
    /// `None` in folder-only state, where actions route to the workspace.
    pub terminal_view_id: Option<EntityId>,
    /// Session cwd — branch listing and diff commands run against it.
    pub cwd: Option<std::path::PathBuf>,
    /// Two-click discard confirm without a terminal: armed path (or group
    /// key) plus arming time. Rendered as "click again to confirm".
    pub armed_discard: Option<(String, std::time::Instant)>,
}
