//! Top-level workspace tabs: Worktree (= workspace) > Tab > Panes.
//!
//! A `WorkspaceTab` owns one complete pane layout: the pane-tabs, the split
//! tree, the active pane-tab/pane selection, the transient per-divider
//! drag/mouse state, and the close-confirmation flow state.
//!
//! `WorkspaceView` keeps the ACTIVE tab's fields live (`tabs`, `split_tree`,
//! `active_tab_id`, …) so every existing pane code path keeps working
//! untouched. Inactive tabs park here verbatim (all `ViewHandle`s are handle
//! clones — parking never destroys views, it only stops rendering them) and
//! are swapped back into the live fields on selection. Pane-tab ids come
//! from the workspace-global `next_tab_id` allocator, so parked tabs can
//! never collide with live ones in `host_owned_tabs` or the surface registry.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use warpui::elements::DragResizeHandle;
use warpui::EntityId;
use warpui_core::elements::MouseStateHandle;

use super::pane_layout::{PaneLayout, SplitId};
use super::view::{PaneTab, WorkspaceView};

/// One top-level workspace tab: a named, independent pane layout.
#[derive(Clone)]
pub(crate) struct WorkspaceTab {
    pub(crate) id: usize,
    pub(crate) name: String,
    pub(crate) ui: SurfaceUiState,
    /// The tab's pane-tabs (live in `WorkspaceView::tabs` while active).
    pub(crate) panes: Vec<PaneTab>,
    pub(crate) split_tree: Option<PaneLayout>,
    pub(crate) active_tab_id: usize,
    pub(crate) active_pane_id: Option<EntityId>,
    pub(crate) editor_pane_focused: bool,
    pub(crate) split_drag_handles: RefCell<HashMap<(SplitId, usize), DragResizeHandle>>,
    pub(crate) split_close_mouse_states: RefCell<HashMap<usize, MouseStateHandle>>,
    pub(crate) split_action_mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    pub(crate) pending_close_tab: Option<(usize, String)>,
    pub(crate) close_tab_error: Option<String>,
    pub(crate) closing_worktree_tab: Option<usize>,
    pub(crate) closed_worktree_main_checkout: Option<PathBuf>,
    pub(crate) close_other_tabs_keep_id: Option<usize>,
}

impl WorkspaceTab {
    /// A fresh, empty surface. The caller creates its first pane-tab.
    pub(crate) fn fresh(id: usize, name: String) -> Self {
        Self {
            id,
            name,
            ui: SurfaceUiState {
                right_visible: true,
                ..SurfaceUiState::default()
            },
            panes: Vec::new(),
            split_tree: None,
            active_tab_id: 0,
            active_pane_id: None,
            editor_pane_focused: false,
            split_drag_handles: RefCell::new(HashMap::new()),
            split_close_mouse_states: RefCell::new(HashMap::new()),
            split_action_mouse_states: RefCell::new(HashMap::new()),
            pending_close_tab: None,
            close_tab_error: None,
            closing_worktree_tab: None,
            closed_worktree_main_checkout: None,
            close_other_tabs_keep_id: None,
        }
    }
}

impl WorkspaceView {
    pub(crate) fn workspace_tab_header_visible(&self) -> bool {
        !self.settings_open
            && !self.automations_open
            && !self.teams_open
            && !self.tasks_open
            && !self.pull_requests_open
            && workspace_tab_header_visible(self.project_start_visible, self.tabs.is_empty())
    }

    pub(crate) fn terminal_focus_can_activate(&self, tab_id: usize) -> bool {
        terminal_focus_can_activate(
            self.project_start_visible || self.settings_open,
            self.tabs.iter().any(|tab| tab.id == tab_id),
            self.active_tab_id == tab_id
                || self
                    .split_tree
                    .as_ref()
                    .is_some_and(|layout| layout.contains(tab_id)),
        )
    }

    /// Snapshot the live (active-surface) fields into a parked tab.
    /// Pure clone — views stay alive under their `ViewHandle`s.
    pub(crate) fn park_active_workspace_tab(&self) -> WorkspaceTab {
        WorkspaceTab {
            id: self.active_workspace_tab_id,
            name: self.active_workspace_tab_name.clone(),
            ui: self.capture_surface_ui(),
            panes: self.tabs.clone(),
            split_tree: self.split_tree.clone(),
            active_tab_id: self.active_tab_id,
            active_pane_id: self.active_pane_id,
            editor_pane_focused: self.editor_pane_focused,
            split_drag_handles: self.split_drag_handles.clone(),
            split_close_mouse_states: self.split_close_mouse_states.clone(),
            split_action_mouse_states: self.split_action_mouse_states.clone(),
            pending_close_tab: self.pending_close_tab.clone(),
            close_tab_error: self.close_tab_error.clone(),
            closing_worktree_tab: self.closing_worktree_tab,
            closed_worktree_main_checkout: self.closed_worktree_main_checkout.clone(),
            close_other_tabs_keep_id: self.close_other_tabs_keep_id,
        }
    }

    /// Install a parked tab into the live fields (it leaves the parked list).
    pub(crate) fn restore_workspace_tab(&mut self, tab: WorkspaceTab) {
        self.reveal_workspace_tab(tab.id);
        for workspace in &mut self.workspaces {
            workspace.select_tab(tab.id);
        }
        self.apply_surface_ui(&tab.ui);
        self.active_workspace_tab_id = tab.id;
        self.active_workspace_tab_name = tab.name;
        self.tabs = tab.panes;
        self.split_tree = tab.split_tree;
        self.active_tab_id = tab.active_tab_id;
        self.active_pane_id = tab.active_pane_id;
        self.editor_pane_focused = tab.editor_pane_focused;
        self.split_drag_handles = tab.split_drag_handles;
        self.split_close_mouse_states = tab.split_close_mouse_states;
        self.split_action_mouse_states = tab.split_action_mouse_states;
        self.pending_close_tab = tab.pending_close_tab;
        self.close_tab_error = tab.close_tab_error;
        self.closing_worktree_tab = tab.closing_worktree_tab;
        self.closed_worktree_main_checkout = tab.closed_worktree_main_checkout;
        self.close_other_tabs_keep_id = tab.close_other_tabs_keep_id;
    }

    /// Upsert the parked entry for the currently live surface.
    pub(crate) fn store_active_workspace_tab(&mut self) {
        let parked = self.park_active_workspace_tab();
        if let Some(slot) = self
            .parked_workspace_tabs
            .iter_mut()
            .find(|tab| tab.id == parked.id)
        {
            *slot = parked;
        } else {
            self.parked_workspace_tabs.push(parked);
        }
    }

    /// Strip rows in stable order: (surface id, name, is_active).
    /// Ids missing from both live and parked state (closed races) are skipped.
    pub(crate) fn ordered_workspace_tab_rows(&self) -> Vec<(usize, String, bool)> {
        let mut rows = Vec::with_capacity(self.workspace_tab_order.len());
        // The header belongs to one worktree. Other workspaces remain
        // reachable in the sidebar, but never contribute their tabs here.
        let owner = self
            .workspaces
            .iter()
            .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id));
        let owned_ids = owner
            .map(|workspace| workspace.tab_ids.as_slice())
            .unwrap_or(&[]);
        for id in owned_ids {
            if *id == self.active_workspace_tab_id {
                rows.push((*id, self.active_workspace_tab_name.clone(), true));
            } else if let Some(tab) = self.parked_workspace_tabs.iter().find(|tab| tab.id == *id) {
                if owner.is_some_and(|workspace| workspace.tab_ids.contains(id)) {
                    rows.push((tab.id, tab.name.clone(), false));
                }
            }
        }
        // The active surface is always shown, even if ordering lags.
        if !rows
            .iter()
            .any(|(id, _, _)| *id == self.active_workspace_tab_id)
        {
            rows.push((
                self.active_workspace_tab_id,
                self.active_workspace_tab_name.clone(),
                true,
            ));
        }
        rows
    }
}

fn workspace_tab_header_visible(creating_workspace: bool, empty: bool) -> bool {
    !creating_workspace && !empty
}

fn terminal_focus_can_activate(
    creating_workspace: bool,
    belongs_to_active_tab: bool,
    visible: bool,
) -> bool {
    !creating_workspace && belongs_to_active_tab && visible
}

impl WorkspaceView {
    /// Attach a newly created worktree surface to its workspace. Membership
    /// is explicit; it does not change when a shell changes cwd or agent state.
    pub(crate) fn assign_surface_to_workspace(&mut self, root: PathBuf, title: String) {
        crate::remote_fs::ensure_stand_in(&root);
        let root = root.canonicalize().unwrap_or(root);
        self.closed_workspace_roots.retain(|closed| closed != &root);
        let surface = self.active_workspace_tab_id;
        for workspace in &mut self.workspaces {
            workspace.tab_ids.retain(|id| *id != surface);
        }
        if let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.root == root)
        {
            workspace.tab_ids.push(surface);
            workspace.active_tab_id = surface;
        } else {
            let id = self
                .workspaces
                .iter()
                .map(|workspace| workspace.id)
                .max()
                .unwrap_or(0)
                + 1;
            self.workspaces
                .push(dragent_persistence::SerializedWorkspace {
                    organisation_id: self.organisation_id.clone(),
                    id,
                    root,
                    title,
                    tab_ids: vec![surface],
                    active_tab_id: surface,
                });
        }
        self.workspaces
            .retain(|workspace| !workspace.tab_ids.is_empty());
        for workspace in &mut self.workspaces {
            if !workspace.tab_ids.contains(&workspace.active_tab_id) {
                workspace.active_tab_id = workspace.tab_ids[0];
            }
        }
        if let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.tab_ids.contains(&surface))
        {
            self.persistence_writer
                .send(dragent_persistence::ModelEvent::RecordLocalWorkspace(
                    workspace.clone(),
                ));
        }
    }

    /// Migrate older snapshots once and remove references to closed surfaces.
    /// Runtime navigation afterwards uses these durable memberships directly.
    pub(crate) fn reconcile_workspace_ownership(&mut self) {
        let mut surfaces = vec![(self.active_workspace_tab_id, self.tabs.clone())];
        surfaces.extend(
            self.parked_workspace_tabs
                .iter()
                .map(|tab| (tab.id, tab.panes.clone())),
        );
        for workspace in &mut self.workspaces {
            workspace
                .tab_ids
                .retain(|id| surfaces.iter().any(|(surface, _)| surface == id));
            if !workspace.tab_ids.contains(&workspace.active_tab_id) {
                if let Some(id) = workspace.tab_ids.first() {
                    workspace.active_tab_id = *id;
                }
            }
        }
        self.workspaces
            .retain(|workspace| !workspace.tab_ids.is_empty());
        for (id, panes) in surfaces {
            if panes.is_empty()
                || self
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.tab_ids.contains(&id))
            {
                continue;
            }
            let root = panes
                .iter()
                .find_map(|pane| pane.project_root.clone())
                .unwrap_or_else(super::project_naming::default_pane_cwd);
            let root = root.canonicalize().unwrap_or(root);
            if let Some(workspace) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.root == root)
            {
                workspace.tab_ids.push(id);
            } else {
                let title = panes
                    .iter()
                    .find_map(|pane| pane.custom_label.clone())
                    .unwrap_or_else(|| {
                        root.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Workspace".into())
                    });
                let workspace_id = self
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.id)
                    .max()
                    .unwrap_or(0)
                    + 1;
                self.workspaces
                    .push(dragent_persistence::SerializedWorkspace {
                        organisation_id: self.organisation_id.clone(),
                        id: workspace_id,
                        root,
                        title,
                        tab_ids: vec![id],
                        active_tab_id: id,
                    });
            }
        }
        if let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.tab_ids.contains(&self.active_workspace_tab_id))
        {
            workspace.active_tab_id = self.active_workspace_tab_id;
        }
    }
}

/// Presentation belongs to the tab layout, not to whichever editor was last
/// mounted in the window. Buffer contents use the existing working-copy store.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct SurfaceUiState {
    pub layout: Option<PaneLayout>,
    pub editor_open: bool,
    pub editor_focused: bool,
    pub owner_tab: Option<usize>,
    pub root: Option<PathBuf>,
    pub active_file: Option<PathBuf>,
    pub open_files: Vec<PathBuf>,
    pub preview_file: Option<PathBuf>,
    pub dock_visible: bool,
    pub dock_maximized: bool,
    pub dock_height: Option<f32>,
    pub dock_active_id: Option<usize>,
    pub dock_split_id: Option<usize>,
    pub right_visible: bool,
    pub right_closed: bool,
}

impl WorkspaceView {
    pub(crate) fn capture_surface_ui(&self) -> SurfaceUiState {
        SurfaceUiState {
            layout: self.split_tree.clone(),
            editor_open: self.document_center_open,
            editor_focused: self.editor_pane_focused,
            owner_tab: self.document_tab_id,
            root: self.document_owner_root.clone(),
            active_file: self.document_requested_path.clone(),
            open_files: self.document_open_paths.clone(),
            preview_file: self.document_preview_path.clone(),
            dock_visible: self.document_terminal_visible,
            dock_maximized: self.document_terminal_maximized,
            dock_height: self
                .document_terminal_state
                .lock()
                .ok()
                .map(|state| state.size()),
            dock_active_id: self.document_terminal_active_id,
            dock_split_id: self.document_terminal_split_id,
            right_visible: self.right_panel_visible,
            right_closed: self.right_panel_user_closed,
        }
    }

    pub(crate) fn apply_surface_ui(&mut self, state: &SurfaceUiState) {
        self.document_center_open = state.editor_open;
        self.document_restore_pending = state.active_file.is_some();
        self.document_tab_id = state.owner_tab;
        self.document_owner_root = state.root.clone();
        self.document_requested_path = state.active_file.clone();
        self.document_open_paths = state.open_files.clone();
        self.document_preview_path = state.preview_file.clone();
        self.document_terminal_visible = state.dock_visible;
        self.document_terminal_maximized = state.dock_maximized;
        self.document_terminal_active_id = state.dock_active_id;
        self.document_terminal_split_id = state.dock_split_id;
        if let Some(height) = state.dock_height {
            self.document_terminal_state = warpui::elements::resizable_state_handle(height);
        }
        self.right_panel_visible = state.right_visible;
        self.right_panel_user_closed = state.right_closed;
    }
}

#[cfg(test)]
mod restoration_tests {
    use super::*;
    use crate::workspace::pane_layout::{
        SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT, MIN_PANE_WIDTH,
    };

    #[test]
    fn creation_screen_hides_previous_workspace_tabs() {
        assert!(!workspace_tab_header_visible(true, false));
        assert!(!workspace_tab_header_visible(true, true));
        assert!(workspace_tab_header_visible(false, false));
        assert!(!workspace_tab_header_visible(false, true));
    }

    #[test]
    fn background_terminal_focus_cannot_replace_restored_workspace() {
        // A stale focus flag from a parked workspace cannot navigate, even
        // when that terminal still considers itself visible during adoption.
        assert!(!terminal_focus_can_activate(false, false, true));
        // Neither a hidden pane nor the previous workspace under the launcher
        // may activate itself when asynchronous terminal events arrive.
        assert!(!terminal_focus_can_activate(false, true, false));
        assert!(!terminal_focus_can_activate(true, true, true));
        // Clicking another visible split pane remains supported.
        assert!(terminal_focus_can_activate(false, true, true));
    }

    #[test]
    fn saved_surface_keeps_editor_split_geometry_focus_and_dock() {
        let mut layout = PaneLayout::new(17);
        assert!(layout.split_pane(17, EDITOR_PANE_ID, SplitSide::Right));
        assert!(layout.split_pane(17, 23, SplitSide::Down));
        layout.focus(EDITOR_PANE_ID);
        let state = SurfaceUiState {
            layout: Some(layout.clone()),
            editor_open: true,
            editor_focused: true,
            owner_tab: Some(17),
            root: Some(PathBuf::from("/project/worktree-a")),
            active_file: Some(PathBuf::from("src/main.rs")),
            open_files: vec![PathBuf::from("README.md"), PathBuf::from("src/main.rs")],
            dock_visible: true,
            dock_maximized: true,
            dock_height: Some(280.0),
            dock_active_id: Some(42),
            right_closed: true,
            ..SurfaceUiState::default()
        };
        let decoded: SurfaceUiState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let restored = decoded.layout.unwrap();
        assert_eq!(restored.root(), layout.root());
        assert_eq!(restored.active(), Some(EDITOR_PANE_ID));
        assert_eq!(
            restored.pane_rects(1200.0, 900.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT),
            layout.pane_rects(1200.0, 900.0, MIN_PANE_WIDTH, MIN_PANE_HEIGHT)
        );
        assert_eq!(decoded.open_files, state.open_files);
        assert_eq!(decoded.active_file, state.active_file);
        assert!(
            decoded.editor_focused
                && decoded.dock_visible
                && decoded.dock_maximized
                && decoded.right_closed
        );
        assert_eq!(decoded.dock_active_id, Some(42));
    }
}
