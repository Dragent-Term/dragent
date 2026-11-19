//! Layout and session persistence methods for WorkspaceView.
//!
//! Responsible for:
//! - serializing/deserializing pane layouts and capturing layout collections
//! - saving workspace sessions (tabs, panes, working copies)
//!
//! Not responsible for:
//! - the layout tree algorithms themselves (pane_layout)
//! - SQLite writes (persistence writer thread)
//!
//! Blocks here are impl WorkspaceView: same type, same behavior.

use super::share_sheet;
use super::viewer_bar;
use super::{InputEditor, InputEditorEvent, InputMode, LanguageNavigationCapabilities};
use crate::agent::workspace_execution::{
    next_workspace_launch_action, next_workspace_retry_wait_ms, WorkspaceExecutionRuntime,
    WorkspaceLaunchAction, WorkspaceLaunchPolicy, WorkspacePipelineAction,
    WorkspaceResolvedToolAction, WorkspaceToolRequest,
};
use crate::auth::{AuthManager, AuthManagerEvent};
use crate::realtime::manager::{SessionManager, SessionManagerEvent};
use crate::settings::AccountStore;
use crate::settings::{SettingsEvent, SettingsView};
use crate::setup::UiSettings;
use crate::terminal::{TerminalEvent, TerminalView};
use crate::theme::{
    Theme, DEFAULT_LEFT_PANEL_WIDTH, DEFAULT_RIGHT_PANEL_WIDTH, HEADER_BAR_HEIGHT, MIN_PANEL_WIDTH,
    UI_FONT_SIZE, WORKSPACE_PADDING,
};
use dragent_session_protocol::workspace::{
    CorrelationId, WorkspaceCommandEnvelope, WorkspaceCommandId, WorkspaceCommandPayload,
    WorkspaceEventEnvelope, WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
};
use dragent_session_protocol::{AccessLevel, ParticipantId, Role};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use warpui::color::ColorU;
use warpui::elements::{
    drag_resize_handle, resizable_state_handle, resizable_state_handle_initialized, Align,
    ChildAnchor, ChildView, ClippedScrollStateHandle, ConstrainedBox, Container,
    CrossAxisAlignment, DispatchEventResult, DragBarSide, DragResizeAxis, DragResizeElement,
    DragResizeHandle, DropTarget, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds,
    PositionedElementAnchor, PositionedElementOffsetBounds, Resizable, ResizableStateHandle,
    SavePosition, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::r#async::Timer;
use warpui::{
    AppContext, Entity, EntityId, GetSingletonModelHandle, ReadModel, ReadView, SingletonEntity,
    TypedActionView, UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Clipped, Hoverable, MouseStateHandle};
use warpui_core::fonts::Cache as FontCache;
use warpui_core::platform::Cursor;
use warpui_core::platform::FilePickerConfiguration;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_MD};
use crate::process_ext::NoWindow;
use dragent_editor::{
    char_index_to_lsp_position, lsp_position_to_char_index, LanguageRequest, LanguageRequestKind,
    TextPosition, TextRange,
};
use dragent_language_services::{
    LanguageServerCatalog, LanguageServerKey, LanguageServiceClient, LanguageServiceClientConfig,
    LanguageServiceEvent, SyncedDocument,
};

use super::pane::WorkspacePane;
use super::pane_layout::{
    LayoutNode, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
use super::project_sidebar::{
    find_git_root, ProjectSidebarModel, ProjectSidebarView, ReviewPrState,
};
// Re-exported so existing `super::view::ReviewShipState` paths keep working
// during the migration; new code should import from `review_pr_service`.
// Re-exported so existing `super::…` and test paths keep working during the
// migration; new code should import from `project_naming`.
// Glob import during migration: shaping helpers moved to `shape_parse`
// verbatim; call sites below are unchanged.
// Glob import during migration: execution delivery helpers moved verbatim.
use super::execution_delivery::*;
// Glob import during migration: language navigation helpers moved verbatim.
use super::language_nav::*;
pub(crate) use super::project_naming::{
    clone_target_name, default_projects_dir, is_default_work_label,
    should_replace_project_tab_label, valid_project_name,
};
pub(crate) use super::review_pr_service::{
    query_review_pr_state, query_review_ship_state, CachedReviewPr, ReviewShipState,
};
use super::shape_parse::*;
use super::sidebar::color_from_hex;
use super::view_overlays::ExplorerPrompt;
use super::{
    chrome_inset, content_header, header, left_panel, pane_layout, persistence, sidebar,
    split_tab_header, DropSide, PaneDropZone, ProjectFolderIntent, SidebarFocus, WorkItemView,
    WorkspaceAction, DEFAULT_SESSION_TITLE,
};
use dragent_persistence::{SerializedPaneLayout, SerializedSplitDirection};
use dragent_workspace_model::{
    task_coordination_thread_id, ActorKind, ActorRef, AgentMessage as WorkspaceAgentMessage,
    AgentMessageId, AgentMessageKind, AgentMessageRouting, AttentionItem, AttentionItemId,
    AttentionKind, AttentionResolution, AttentionStatus, BuiltInLayout, ChargingAuthorization,
    ChargingMode, CommunicationThread, ExecutionAttemptId, LayoutAxis, LayoutChromeState,
    LayoutCollection, LayoutInstance, LayoutPanelState, LayoutScope, MessageDeliveryState, Project,
    ProjectDocumentIndex, SurfaceDescriptor, SurfaceId, SurfaceKind, SurfaceLayoutNode,
    SurfaceRegistry, ThreadId, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
    WorkspaceState, MAX_LAYOUT_FOCUS_HISTORY,
};
use left_panel::LeftPanelView;
use persistence::{PersistedSession, WindowChrome, WriterHandles};
use uuid::Uuid;

use super::view::WorkspaceView;

/// Restored state shared by every workspace tab rebuilt at launch.
pub(crate) struct SurfaceRestoreContext<'a> {
    pub blocks: &'a persistence::RestoredBlockLists,
    pub agent_blocks: &'a persistence::RestoredAgentBlockLists,
    pub host_bindings: &'a HashMap<Uuid, persistence::SerializedHostTerminalBinding>,
    pub tab_work_items: &'a HashMap<usize, persistence::SerializedWorkspaceTabWorkItem>,
    pub document: &'a persistence::SerializedDocumentSession,
    pub theme: Theme,
    pub ui_font: FamilyId,
}

pub(crate) struct RestoredSurface {
    pub tabs: Vec<super::view::PaneTab>,
    pub split_tree: Option<PaneLayout>,
    pub active_tab_id: usize,
    pub active_pane_id: Option<EntityId>,
}

impl WorkspaceView {
    pub(crate) fn flatten_serialized_layout(
        layout: &SerializedPaneLayout,
    ) -> Vec<SerializedPaneLayout> {
        match layout {
            SerializedPaneLayout::Single { .. } => vec![layout.clone()],
            SerializedPaneLayout::Split {
                left_or_top,
                right_or_bottom,
                ..
            } => {
                let mut panes = Self::flatten_serialized_layout(left_or_top);
                panes.extend(Self::flatten_serialized_layout(right_or_bottom));
                panes
            }
        }
    }

    /// Rebuild one top-level workspace tab's pane-tabs and split tree. Used
    /// for the front tab and every parked tab; only the front tab takes focus.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn restore_surface(
        serialized_tabs: Vec<persistence::SerializedTab>,
        saved_active_tab_id: usize,
        saved_active_pane_uuid: Option<Uuid>,
        focus: bool,
        restore: &SurfaceRestoreContext<'_>,
        next_tab_id: &mut usize,
        pending_host_reattach: &mut Vec<(usize, super::host_tabs::HostReattach)>,
        ctx: &mut ViewContext<Self>,
    ) -> RestoredSurface {
        let mut tabs: Vec<super::view::PaneTab> = Vec::new();
        let mut split_tree = None;
        // Tabs whose root layout holds a split tree carry every member
        // pane (each leaf is a full Single payload). Member tabs stored
        // as lone Singles are redundant with those leaves and are
        // skipped — unless no tree exists at all (legacy sessions), in
        // which case every tab restores exactly as before.
        let tree_tab_idx = serialized_tabs
            .iter()
            .position(|tab| Self::layout_contains_split(&tab.root_layout));
        let tree_member_uuids = tree_tab_idx
            .and_then(|index| serialized_tabs.get(index))
            .map(|tab| {
                Self::flatten_serialized_layout(&tab.root_layout)
                    .into_iter()
                    .filter_map(|leaf| match leaf {
                        SerializedPaneLayout::Single { uuid, .. } => Some(uuid),
                        SerializedPaneLayout::Split { .. } => None,
                    })
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let tab_metadata_by_uuid = serialized_tabs
            .iter()
            .filter_map(|tab| {
                let uuid = tab.owner_pane_uuid.or_else(|| match &tab.root_layout {
                    SerializedPaneLayout::Single { uuid, .. } => Some(*uuid),
                    SerializedPaneLayout::Split { .. } => None,
                })?;
                Some((uuid, tab.clone()))
            })
            .collect::<HashMap<_, _>>();
        for (tab_index, tab) in serialized_tabs.into_iter().enumerate() {
            // Split members stored as lone Singles duplicate the tree
            // tab's leaves — skip them (legacy sessions without any tree
            // keep every tab).
            let own_uuid = tab.owner_pane_uuid.or_else(|| match &tab.root_layout {
                SerializedPaneLayout::Single { uuid, .. } => Some(*uuid),
                SerializedPaneLayout::Split { .. } => None,
            });
            if Some(tab_index) != tree_tab_idx
                && own_uuid.is_some_and(|uuid| tree_member_uuids.contains(&uuid))
            {
                continue;
            }
            let single_layouts = Self::flatten_serialized_layout(&tab.root_layout);
            let mut group_tab_ids = vec![];

            for (i, single_layout) in single_layouts.iter().enumerate() {
                let leaf_uuid = match single_layout {
                    SerializedPaneLayout::Single { uuid, .. } => Some(*uuid),
                    SerializedPaneLayout::Split { .. } => None,
                };
                let metadata = leaf_uuid.and_then(|uuid| tab_metadata_by_uuid.get(&uuid));
                let host_binding = leaf_uuid.and_then(|uuid| restore.host_bindings.get(&uuid));
                let (pane, host_reattach) = Self::reconstruct_pane(
                    single_layout,
                    restore.blocks,
                    restore.agent_blocks,
                    restore.theme,
                    restore.ui_font,
                    host_binding,
                    ctx,
                );
                let tab_id = if let Some(metadata) = metadata {
                    metadata.id
                } else if i == 0 {
                    tab.id
                } else {
                    let id = *next_tab_id;
                    *next_tab_id += 1;
                    id
                };
                group_tab_ids.push(tab_id);
                let work_item = restore.tab_work_items.get(&tab_id);
                tabs.push(super::view::PaneTab {
                    id: tab_id,
                    surface_id: pane.surface_id(ctx),
                    title: metadata
                        .map(|metadata| metadata.title.clone())
                        .unwrap_or_else(|| {
                            if i == 0 {
                                tab.title.clone()
                            } else {
                                DEFAULT_SESSION_TITLE.to_string()
                            }
                        }),
                    custom_label: metadata
                        .and_then(|metadata| metadata.custom_label.clone())
                        .or_else(|| (i == 0).then(|| tab.custom_label.clone()).flatten()),
                    color: metadata
                        .and_then(|metadata| metadata.color.as_deref())
                        .or_else(|| (i == 0).then_some(tab.color.as_deref()).flatten())
                        .and_then(color_from_hex),
                    work_item_id: work_item
                        .map(|item| item.work_item_id)
                        .or_else(|| {
                            (!pane.is_settings() && !pane.is_browser()).then(WorkItemId::new)
                        }),
                    project_root: work_item
                        .and_then(|item| item.project_root.clone())
                        .or_else(|| {
                            (restore.document.owner_tab_id == Some(tab_id)
                                && restore.document.root.is_dir())
                            .then(|| restore.document.root.clone())
                        })
                        .or_else(|| {
                            // Compatibility for sessions written before
                            // project scope was persisted independently.
                            let cwd = pane.cwd(ctx);
                            (cwd.is_dir() && cwd != super::project_naming::default_pane_cwd())
                                .then_some(cwd)
                        }),
                    hidden_from_sidebar: metadata
                        .map(|metadata| metadata.hidden_from_sidebar)
                        .unwrap_or(i != 0 || tab.hidden_from_sidebar),
                    pane,
                });
                // Host-owned tab: carry the re-attach bundle (with the
                // restored project root) to the post-build attach step.
                if let Some(mut reattach) = host_reattach {
                    if let Some(restored) = tabs.last() {
                        reattach.project_root = restored.project_root.clone();
                        if matches!(restored.pane, WorkspacePane::Terminal(_)) {
                            pending_host_reattach.push((restored.id, reattach));
                        }
                    }
                }
                if tab_id >= *next_tab_id {
                    *next_tab_id = tab_id + 1;
                }
            }

            if Some(tab_index) == tree_tab_idx {
                // Sizes are shares of the extent `to_persisted` wrote them in.
                let mut id_iter = group_tab_ids.into_iter();
                let mut layout = PaneLayout::from_persisted(&tab.root_layout, &mut id_iter);
                layout.focus(saved_active_tab_id);
                split_tree = Some(layout);
            }

            if tab.id >= *next_tab_id {
                *next_tab_id = tab.id + 1;
            }
        }

        let mut active_tab_id = saved_active_tab_id;
        let mut active_pane_id = None;
        let active = saved_active_pane_uuid
            .and_then(|uuid| tabs.iter().find(|tab| tab.pane.pane_uuid(ctx) == uuid))
            .or_else(|| tabs.iter().find(|tab| tab.id == saved_active_tab_id));
        if let Some(tab) = active {
            active_tab_id = tab.id;
            active_pane_id = Some(tab.pane.id());
            if focus {
                tab.pane.focus(ctx);
            }
        }
        RestoredSurface {
            tabs,
            split_tree,
            active_tab_id,
            active_pane_id,
        }
    }

    /// True when a persisted layout holds a split tree (whose leaves each
    /// carry a full pane payload).
    pub(crate) fn layout_contains_split(layout: &SerializedPaneLayout) -> bool {
        matches!(layout, SerializedPaneLayout::Split { .. })
    }

    pub(crate) fn reconstruct_pane(
        serialized: &SerializedPaneLayout,
        blocks: &persistence::RestoredBlockLists,
        agent_blocks: &persistence::RestoredAgentBlockLists,
        theme: Theme,
        ui_font: FamilyId,
        host_binding: Option<&persistence::SerializedHostTerminalBinding>,
        ctx: &mut ViewContext<Self>,
    ) -> (WorkspacePane, Option<super::host_tabs::HostReattach>) {
        match serialized {
            SerializedPaneLayout::Single {
                browser_url: Some(url),
                cwd,
                uuid,
                ..
            } => {
                // A browser leaf: restored as a dormant pane that loads its
                // address (a plain GET) once it is shown again.
                let project_root = (!cwd.as_os_str().is_empty()).then(|| cwd.clone());
                let url = dragent_workspace_model::BrowserUrl::parse(url).unwrap_or_else(|_| {
                    dragent_workspace_model::BrowserUrl::parse("about:blank").expect("static URL")
                });
                let owner = super::browser_tabs::browser_owner(
                    &dragent_persistence::current_org(),
                    project_root.as_deref(),
                );
                let session =
                    dragent_workspace_model::BrowserPaneSession::restore(*uuid, owner, url);
                let pane = Self::spawn_browser_pane(session, project_root, theme, ui_font, ctx);
                (pane, None)
            }
            SerializedPaneLayout::Single {
                cwd,
                uuid,
                shell_title,
                is_active: _,
                clear_gap_height_px,
                clear_gap_before_block_id,
                clear_gap_after_all,
                agent_id,
                browser_url: None,
            } => {
                // Host-owned tab: born pending; integration is owned by the
                // daemon. Reattachment never writes setup into a live program.
                #[cfg(not(test))]
                let use_host = true;
                #[cfg(test)]
                let use_host = host_binding.is_some();
                if use_host {
                    use crate::terminal::manager::pty_writer::HostPtyBinding;
                    let slot = std::sync::Arc::new(HostPtyBinding::new());
                    let shell = dragent_pty::resolve_shell(
                        crate::console::editor_settings::EditorSettings::global()
                            .terminal
                            .default_shell
                            .clone()
                            .or_else(|| std::env::var("SHELL").ok()),
                    );
                    let pane = WorkspacePane::spawn_host_pending(ctx, cwd.clone(), slot.clone());
                    // Keep the persisted pane identity (active-tab matching,
                    // surface registry, agent activity routing) exactly like
                    // the classic restore path does post-construction.
                    if let WorkspacePane::Terminal(handle) = &pane {
                        let handle = handle.clone();
                        let persisted = *uuid;
                        let restored_agent_id = agent_id.clone();
                        ctx.update_view(&handle, |view, tctx| {
                            view.launch_shell = Some(shell.clone());
                            if host_binding.is_some_and(|binding| binding.agent) {
                                view.set_tab_session_kind(crate::agent::TabSessionKind::Agent);
                            }
                            // The session keeps its registry identity: tabs,
                            // rail, and input editor resolve the definition
                            // without re-detecting the process.
                            view.set_agent_id(restored_agent_id.clone());
                            // History/conversations and the live PTY are separate.
                            // Reattaching a PTY must not discard the built-in
                            // agent's persisted conversation or completed blocks.
                            {
                                view.restore_persisted_state(
                                    cwd.clone(),
                                    persisted,
                                    shell_title.clone(),
                                    *clear_gap_height_px,
                                    *clear_gap_before_block_id,
                                    *clear_gap_after_all,
                                    blocks.get(uuid).cloned().unwrap_or_default(),
                                    agent_blocks.get(uuid).cloned().unwrap_or_default(),
                                    tctx,
                                );
                            }
                            crate::agent::local_tools::rebind_activity_sender(
                                view.pane_uuid,
                                persisted,
                            );
                            view.pane_uuid = persisted;
                        });
                    }
                    let reattach = match &pane {
                        WorkspacePane::Terminal(handle) => Some(super::host_tabs::HostReattach {
                            pane: handle.clone(),
                            slot,
                            binding: host_binding.cloned().unwrap_or_else(|| {
                                persistence::SerializedHostTerminalBinding {
                                    remote_host: None,
                                    pane_uuid: *uuid,
                                    workspace_id: String::new(),
                                    home_id: String::new(),
                                    session_id: String::new(),
                                    agent: false,
                                }
                            }),
                            cwd: cwd.clone(),
                            project_root: None,
                            restart_after_update: false,
                            expect_agent: host_binding.is_some_and(|binding| binding.agent)
                                || agent_id.is_some(),
                        }),
                        WorkspacePane::Settings(_) | WorkspacePane::Browser(_) => None,
                    };
                    return (pane, reattach);
                }
                #[cfg(not(test))]
                unreachable!("production terminal restoration is host-backed");
                #[cfg(test)]
                {
                    // Prefer the per-root database (migrating legacy rows once);
                    // fall back to the bulk-loaded snapshot when it holds nothing.
                    let org = dragent_persistence::current_org();
                    let per_root = dragent_persistence::restore_pane_blocks(cwd, &org, *uuid);
                    let (restored, restored_agent) = per_root.unwrap_or_else(|| {
                        (
                            blocks.get(uuid).cloned().unwrap_or_default(),
                            agent_blocks.get(uuid).cloned().unwrap_or_default(),
                        )
                    });
                    let pane = WorkspacePane::spawn_restore(
                        ctx,
                        theme,
                        ui_font,
                        cwd.clone(),
                        *uuid,
                        shell_title.clone(),
                        *clear_gap_height_px,
                        *clear_gap_before_block_id,
                        *clear_gap_after_all,
                        restored,
                        restored_agent,
                    );
                    pane.set_agent_id(ctx, agent_id.clone());
                    (pane, None)
                }
            }
            SerializedPaneLayout::Split { .. } => {
                panic!("flatten_serialized_layout should only yield Single nodes");
            }
        }
    }

    pub(crate) fn serialize_tab_pane(
        pane: &WorkspacePane,
        app: &AppContext,
        active_pane_uuid: Option<Uuid>,
    ) -> SerializedPaneLayout {
        let cwd = pane.cwd(app);
        let uuid = pane.pane_uuid(app);
        let shell_title = pane.shell_title(app);
        let gap = pane.active_clear_gap(app);
        let (height, before_id, after_all) = match gap {
            Some(g) => (
                g.current_height_px,
                g.before_block_id.map(|id| id as i32),
                g.after_all,
            ),
            None => (0.0, None, false),
        };
        SerializedPaneLayout::Single {
            cwd,
            uuid,
            shell_title,
            is_active: active_pane_uuid == Some(uuid),
            clear_gap_height_px: height,
            clear_gap_before_block_id: before_id,
            clear_gap_after_all: after_all,
            agent_id: pane.agent_id(app),
            browser_url: match pane {
                WorkspacePane::Browser(browser) => {
                    Some(browser.read(app, |b, _| b.session().url.as_str().to_string()))
                }
                _ => None,
            },
        }
    }

    /// Project one layout-tree node to a surface-layout node for persistence.
    /// Missing tabs (closed without dissolving) prune the node; a split left
    /// with fewer than two live children collapses to the survivor. Sizes
    /// are advisory shares of a nominal extent (ratios, not screen pixels).
    pub(crate) fn surface_node_for_split(&self, node: &LayoutNode) -> Option<SurfaceLayoutNode> {
        match node {
            LayoutNode::Pane { id } => self
                .tabs
                .iter()
                .find(|tab| tab.id == *id)
                .map(|tab| SurfaceLayoutNode::single(tab.surface_id)),
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let children = children
                    .iter()
                    .filter_map(|child| self.surface_node_for_split(child))
                    .collect::<Vec<_>>();
                if children.len() < 2 {
                    return children.into_iter().next();
                }
                let preferred_sizes = node
                    .shares()
                    .into_iter()
                    .take(children.len().saturating_sub(1))
                    .map(|share| share * pane_layout::SURFACE_SHARE_EXTENT)
                    .collect::<Vec<_>>();
                Some(SurfaceLayoutNode::Split {
                    axis: match direction {
                        SplitDirection::Horizontal => LayoutAxis::Horizontal,
                        SplitDirection::Vertical => LayoutAxis::Vertical,
                    },
                    children,
                    preferred_sizes,
                })
            }
        }
    }

    pub(crate) fn capture_layout_collection(&self, app: &AppContext) -> LayoutCollection {
        let Some(active_terminal_surface_id) = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .map(|tab| tab.surface_id)
        else {
            return self.layout_collection.clone();
        };

        let available_surfaces = self
            .tabs
            .iter()
            .map(|tab| tab.surface_id)
            .chain(std::iter::once(self.document_surface_id))
            .chain(
                self.document_terminals
                    .iter()
                    .map(|terminal| terminal.surface_id),
            )
            .fold(Vec::new(), |mut surfaces, surface_id| {
                if !surfaces.contains(&surface_id) {
                    surfaces.push(surface_id);
                }
                surfaces
            });

        // The live pane tree is authoritative whenever it exists. In
        // particular, an editor can be split beside terminals; reducing that
        // state to a single editor surface makes the next launch lose the
        // terminal geometry even though the PTYs survive. A standalone editor
        // (with no pane tree yet) is the only case that uses a single editor
        // surface here.
        let split_root = self
            .split_tree
            .as_ref()
            .and_then(|layout| self.surface_node_for_split(layout.root()));
        let (root, active_surface_id) = if let Some(root) = split_root {
            let active_surface_id = if self.editor_pane_focused && self.document_center_open {
                self.document_surface_id
            } else {
                active_terminal_surface_id
            };
            (root, active_surface_id)
        } else if self.document_center_open {
            (
                SurfaceLayoutNode::single(self.document_surface_id),
                self.document_surface_id,
            )
        } else {
            (
                SurfaceLayoutNode::Stack {
                    surfaces: self.tabs.iter().map(|tab| tab.surface_id).collect(),
                    active_surface_id: active_terminal_surface_id,
                },
                active_terminal_surface_id,
            )
        };

        let chrome = LayoutChromeState {
            left: LayoutPanelState {
                visible: self.left_panel_visible,
                size: self.left_panel_state.lock().ok().map(|state| state.size()),
                maximized: false,
                active_surface_id: None,
            },
            right: LayoutPanelState {
                visible: self.right_panel_visible,
                size: self.right_panel_state.lock().ok().map(|state| state.size()),
                maximized: false,
                active_surface_id: None,
            },
            bottom: LayoutPanelState {
                visible: self.document_terminal_visible,
                size: self
                    .document_terminal_state
                    .lock()
                    .ok()
                    .map(|state| state.size()),
                maximized: self.document_terminal_maximized,
                active_surface_id: self.document_terminal_active_id.and_then(|terminal_id| {
                    self.document_terminals
                        .iter()
                        .find(|terminal| terminal.id == terminal_id)
                        .map(|terminal| terminal.surface_id)
                }),
            },
        };

        let scope = LayoutScope::Project {
            project_id: self.workspace_state.project().id,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let Ok(mut captured) = LayoutInstance::migrate_develop(
            scope,
            root,
            available_surfaces,
            chrome,
            Some(active_surface_id),
            now,
        ) else {
            log::error!("[workspace-layout] refused to persist invalid Develop layout");
            return self.layout_collection.clone();
        };

        let mut layouts = self.layout_collection.clone();
        if let Some(existing) = layouts.instance_for(scope, BuiltInLayout::Develop).cloned() {
            captured.created_at_unix_ms = existing.created_at_unix_ms;
            captured.focus_history = reconciled_focus_history(
                &existing.focus_history,
                &captured.available_surfaces,
                active_surface_id,
            );

            let presentation_changed = captured.root != existing.root
                || captured.available_surfaces != existing.available_surfaces
                || captured.chrome != existing.chrome
                || captured.active_surface_id != existing.active_surface_id
                || captured.focus_history != existing.focus_history;
            if presentation_changed {
                captured.revision = existing.revision.saturating_add(1);
            } else {
                captured.revision = existing.revision;
                captured.updated_at_unix_ms = existing.updated_at_unix_ms;
            }
        }

        let activate_develop = layouts.active_layout_id.is_none()
            || layouts
                .active()
                .is_some_and(|layout| layout.built_in == Some(BuiltInLayout::Develop));
        if let Err(error) = layouts.upsert(captured.clone()) {
            log::error!("[workspace-layout] failed to update Develop layout: {error}");
            return self.layout_collection.clone();
        }
        if activate_develop {
            layouts.active_layout_id = Some(captured.id);
        }
        layouts
    }
}

impl WorkspaceView {
    /// Serialize one top-level workspace tab's pane-tabs and split tree. The
    /// front tab and parked tabs use the same form so restore is identical.
    fn serialize_surface(
        &self,
        tabs: &[super::view::PaneTab],
        split_tree: Option<&PaneLayout>,
        active_pane_id: Option<EntityId>,
        app: &AppContext,
    ) -> (Vec<persistence::SerializedTab>, Option<Uuid>) {
        let active_pane_uuid = active_pane_id.and_then(|pane_id| {
            tabs.iter()
                .find(|tab| tab.pane.id() == pane_id)
                .map(|tab| tab.pane.pane_uuid(app))
        });

        // The split tree is stored once, on the first member tab in visual
        // order (sizes are shares of `PaneLayout::to_persisted`'s extent).
        // Members keep their own Singles; the tree leaves carry full pane
        // payloads, so restore flattens everything into matching tabs.
        // Editor pane stripped: storage holds terminal tabs only; the
        // editor returns from restored documents at build.
        // Mirrors never persist; they re-bind via the restore plan.
        let mirror_tab_id = self.host_mirror_tab_id;
        let save_tree = Self::layout_for_save_of(split_tree);
        let tree_host = save_tree
            .as_ref()
            .filter(|layout| layout.pane_count() > 1)
            .and_then(|layout| {
                let panes = layout.panes();
                panes
                    .iter()
                    .copied()
                    .find(|id| {
                        tabs.iter()
                            .find(|tab| tab.id == *id)
                            .is_some_and(|tab| !tab.hidden_from_sidebar)
                    })
                    .or_else(|| panes.into_iter().next())
            });
        let persisted =
            |tab: &&super::view::PaneTab| !tab.pane.is_settings() && mirror_tab_id != Some(tab.id);
        let member_singles: HashMap<usize, SerializedPaneLayout> = tabs
            .iter()
            .filter(persisted)
            .map(|tab| {
                (
                    tab.id,
                    Self::serialize_tab_pane(&tab.pane, app, active_pane_uuid),
                )
            })
            .collect();
        let serialized_tabs = tabs
            .iter()
            .filter(persisted)
            .map(|tab| {
                let root_layout = match (&save_tree, tree_host) {
                    (Some(layout), Some(host)) if tab.id == host => layout.to_persisted(&|pane| {
                        member_singles.get(&pane).cloned().unwrap_or_else(|| {
                            Self::serialize_tab_pane(&tab.pane, app, active_pane_uuid)
                        })
                    }),
                    _ => member_singles.get(&tab.id).cloned().unwrap_or_else(|| {
                        Self::serialize_tab_pane(&tab.pane, app, active_pane_uuid)
                    }),
                };
                persistence::SerializedTab {
                    id: tab.id,
                    title: tab.title.clone(),
                    custom_label: tab.custom_label.clone(),
                    color: tab.color.map(|c| sidebar::color_to_hex(c)),
                    root_layout,
                    hidden_from_sidebar: tab.hidden_from_sidebar,
                    owner_pane_uuid: Some(tab.pane.pane_uuid(app)),
                }
            })
            .collect();
        (serialized_tabs, active_pane_uuid)
    }

    pub(crate) fn save_session(&mut self, app: &AppContext) {
        if self.organisation_id != dragent_paths::organisation_id() {
            log::warn!(
                "[organisation] skipped saving a workspace of {} after switching away",
                self.organisation_id
            );
            return;
        }
        self.reconcile_workspace_ownership();
        // Extra windows may save pane output, but their empty/new layout must
        // never replace the primary window's durable workspace catalogue.
        if !self.persists_session_snapshot {
            return;
        }
        let left_panel_width = self.left_panel_state.lock().ok().map(|s| s.size());
        let right_panel_width = self.right_panel_state.lock().ok().map(|s| s.size());

        let (serialized_tabs, active_pane_uuid) = self.serialize_surface(
            &self.tabs,
            self.split_tree.as_ref(),
            self.active_pane_id,
            app,
        );
        let parked_workspace_tabs = self
            .parked_workspace_tabs
            .iter()
            .filter(|parked| parked.id != self.active_workspace_tab_id)
            .map(|parked| {
                let (tabs, active_pane_uuid) = self.serialize_surface(
                    &parked.panes,
                    parked.split_tree.as_ref(),
                    parked.active_pane_id,
                    app,
                );
                persistence::SerializedWorkspaceTab {
                    id: parked.id,
                    name: parked.name.clone(),
                    tabs,
                    active_tab_id: parked.active_tab_id,
                    active_pane_uuid,
                }
            })
            .collect::<Vec<_>>();
        let parked_panes = || {
            self.parked_workspace_tabs
                .iter()
                .filter(|parked| parked.id != self.active_workspace_tab_id)
                .flat_map(|parked| parked.panes.iter())
        };

        let (model_active_relative_path, expanded_directories) =
            self.project_sidebar_model.read(app, |model, _| {
                (
                    model.active_relative_path(),
                    model
                        .expanded_directories
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            });
        // A dormant restored session intentionally has no active buffer. Keep
        // its lightweight requested path so another close/restart before a
        // file click does not erase the last active editor tab.
        let is_generated_review = |path: &std::path::Path| {
            path.starts_with(".git-diff")
                || path.starts_with(".git-history")
                || path.starts_with(".work-changes")
        };
        let active_relative_path = model_active_relative_path
            .or_else(|| self.document_requested_path.clone())
            .filter(|path| !is_generated_review(path));
        let dock_height = self.document_terminal_state.lock().ok().map(|s| s.size());
        let dock_terminals = self
            .document_terminals
            .iter()
            .map(|terminal| persistence::SerializedDockedTerminal {
                id: terminal.id,
                owner_root: terminal.owner_root.clone(),
                pane: Self::serialize_tab_pane(&terminal.pane, app, None),
                borrowed: terminal.borrowed,
                host_binding: self.host_binding_for_pane(&terminal.pane, app),
            })
            .collect();
        let layouts = self.capture_layout_collection(app);
        self.layout_collection = layouts.clone();
        let workspace_tab_work_items = self
            .tabs
            .iter()
            .chain(parked_panes())
            .filter_map(|tab| {
                let work_item_id = tab.work_item_id?;
                let (open, active, view, dock_visible, dock_maximized, shaping) =
                    if work_item_id == self.workspace_work_item_id {
                        (
                            self.work_item_tab_open,
                            self.work_item_tab_active,
                            self.work_item_view,
                            self.document_terminal_visible,
                            self.document_terminal_maximized,
                            dragent_persistence::SerializedShapingProgress {
                                requirements_complete: self.shape_requirements_complete,
                                shape_approved: self.shape_approved,
                                turn_memory: self.shape_turn_memory.clone(),
                                interrupted_turn: self.shape_interrupted_turn.clone(),
                            },
                        )
                    } else {
                        let parked = self.parked_workspace_work.get(&work_item_id)?;
                        (
                            parked.tab_open,
                            parked.tab_active,
                            parked.view,
                            parked.dock_visible,
                            parked.dock_maximized,
                            dragent_persistence::SerializedShapingProgress {
                                requirements_complete: parked.requirements_complete,
                                shape_approved: parked.principal_shape_approved,
                                turn_memory: parked.turn_memory.clone(),
                                interrupted_turn: parked.interrupted_turn.clone(),
                            },
                        )
                    };
                Some(dragent_persistence::SerializedWorkspaceTabWorkItem {
                    owner_tab_id: tab.id,
                    work_item_id,
                    project_root: tab.project_root.clone(),
                    open,
                    active,
                    view: Some(view.persistence_key().to_string()),
                    dock_visible: Some(dock_visible),
                    dock_maximized: Some(dock_maximized),
                    shaping,
                })
            })
            .collect();

        // Host-owned tabs persist their daemon binding per pane uuid so
        // relaunch re-attaches instead of spawning fresh shells. Entries
        // retain their original identity even while reconnecting or unavailable.
        let host_terminal_bindings: Vec<persistence::SerializedHostTerminalBinding> = self
            .tabs
            .iter()
            .chain(parked_panes())
            .filter_map(|tab| self.host_binding_for_pane(&tab.pane, app))
            .collect();

        let session = persistence::SerializedSession {
            tabs: serialized_tabs,
            active_tab_id: self.active_tab_id,
            active_pane_uuid,
            chrome: WindowChrome {
                left_panel_open: self.left_panel_visible,
                left_panel_width,
                right_panel_open: self.right_panel_visible,
                right_panel_width,
            },
            document: persistence::SerializedDocumentSession {
                organisation_id: self.organisation_id.clone(),
                root: self
                    .document_owner_root
                    .clone()
                    .unwrap_or_else(|| self.document_index_root.clone()),
                open_project_roots: self.open_project_roots.clone(),
                selected_project_root: self.selected_project_root.clone(),
                project_start_visible: self.project_start_visible,
                prompt_text: self.prompt_text.clone(),
                prompt_links_json: serde_json::to_string(&self.prompt_links).unwrap_or_default(),
                prompt_agent: Some(format!("{:?}", self.prompt_agent)),
                prompt_selected_worktree: self.prompt_selected_worktree.clone(),
                prompt_branch: self.prompt_branch.clone(),
                center_open: self.document_center_open,
                owner_tab_id: self.document_tab_id,
                active_relative_path,
                open_relative_paths: self
                    .document_open_paths
                    .iter()
                    .filter(|path| !is_generated_review(path))
                    .cloned()
                    .collect(),
                work_item_open: self.work_item_tab_open,
                work_item_active: self.work_item_tab_active,
                work_item_view: Some(self.work_item_view.persistence_key().to_string()),
                workspace_tab_work_items,
                expanded_directories,
                dock_visible: self.document_terminal_visible,
                dock_height,
                dock_maximized: self.document_terminal_maximized,
                dock_active_id: self.document_terminal_active_id,
                dock_split_id: self.document_terminal_split_id,
                dock_next_id: self.document_terminal_next_id,
                dock_terminals,
                host_terminal_bindings,
                parked_workspace_tabs,
                workspace_tab_order: self.workspace_tab_order.clone(),
                workspaces: self.workspaces.clone(),
                closed_workspace_roots: self.closed_workspace_roots.clone(),
                settings_open: self.settings_open,
                surface_ui: self
                    .parked_workspace_tabs
                    .iter()
                    .map(|tab| (tab.id, serde_json::to_string(&tab.ui).unwrap_or_default()))
                    .chain(std::iter::once((
                        self.active_workspace_tab_id,
                        serde_json::to_string(&self.capture_surface_ui()).unwrap_or_default(),
                    )))
                    .collect(),
                sidebar_ui: self
                    .left_panel_view
                    .read(app, |panel, _| panel.persist_ui()),
                editor_ui: self
                    .project_sidebar_view
                    .read(app, |sidebar, _| sidebar.persist_editor_ui(app)),
                active_workspace_tab_id: Some(self.active_workspace_tab_id),
                active_workspace_tab_name: self.active_workspace_tab_name.clone(),
            },
            layouts,
        };

        if std::env::var_os("DRAGENT_LOG_PTY").is_some() {
            log::info!(
                "[mem] save_session serialized {} tabs, rss={:.0}MB",
                session.tabs.len(),
                crate::terminal::model::terminal_session_core::current_rss_mb(),
            );
        }

        persistence::persist_snapshot(&self.persistence_writer, session);
    }
}

pub(crate) fn reconciled_focus_history(
    previous: &[SurfaceId],
    available: &[SurfaceId],
    active: SurfaceId,
) -> Vec<SurfaceId> {
    let available = available.iter().copied().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut history = previous
        .iter()
        .copied()
        .filter(|surface| available.contains(surface) && *surface != active)
        .filter(|surface| seen.insert(*surface))
        .collect::<Vec<_>>();
    if available.contains(&active) {
        history.push(active);
    }
    let excess = history.len().saturating_sub(MAX_LAYOUT_FOCUS_HISTORY);
    if excess > 0 {
        history.drain(..excess);
    }
    history
}
pub(crate) fn is_legacy_agentic_fixture(
    project_id: dragent_workspace_model::ProjectId,
    work: WorkItemId,
) -> bool {
    work == WorkItemId(Uuid::new_v5(&project_id.0, b"fixture:organization-billing"))
}
