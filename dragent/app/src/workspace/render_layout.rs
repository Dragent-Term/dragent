//! Layout-instance builders for workspace rendering.
//!
//! Responsible for: shape/coordinate/review layout instances, split grab
//! areas, editor header tabs. Render support only.

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

impl WorkspaceView {
    /// Document surface (editor tabs, editor, dock terminal).
    /// Rendered inside an editor layout pane — never replaces the
    /// terminal anymore; both live side by side in the split tree.
    /// Tabs for the editor pane header are files only. Work Items and their
    /// agent views belong to the first-class Work pane, never editor chrome.
    pub(crate) fn editor_header_tabs(
        &self,
        app: &AppContext,
    ) -> Vec<super::editor_tabs::HeaderTab> {
        self.project_sidebar_model.read(app, |model, _| {
            self.document_open_paths
                .iter()
                .map(|path| super::editor_tabs::HeaderTab {
                    path: path.clone(),
                    label: Self::git_file_diff_tab_label(path),
                    active: !self.work_item_tab_active
                        && self.document_requested_path.as_ref() == Some(path),
                    dirty: model.document_is_dirty(&self.document_index_root, path),
                    work_item: None,
                    agent: None,
                })
                .collect::<Vec<_>>()
        })
    }
    pub(crate) fn horizontal_split_grab_area(&self, theme: &Theme) -> Box<dyn Element> {
        let line = Container::new(Box::new(Empty::new()))
            .with_background_color(theme.border())
            .finish();
        Container::new(ConstrainedBox::new(line).with_height(1.0).finish())
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .finish()
    }
    /// A single visible separator inside a slightly wider drag target. The
    /// divider owns the seam so adjacent panes never paint duplicate lines.
    pub(crate) fn vertical_split_grab_area(&self, theme: &Theme) -> Box<dyn Element> {
        let line = Container::new(Box::new(Empty::new()))
            .with_background_color(theme.border())
            .finish();
        Container::new(ConstrainedBox::new(line).with_width(1.0).finish())
            .with_padding_left(1.0)
            .with_padding_right(1.0)
            .finish()
    }
}

impl WorkspaceView {
    pub(crate) fn shape_layout_instance(
        &self,
    ) -> Result<LayoutInstance, dragent_workspace_model::LayoutValidationError> {
        let ids = self.work_surface_ids;
        let root = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Vertical,
            children: vec![
                SurfaceLayoutNode::Split {
                    axis: LayoutAxis::Horizontal,
                    children: vec![
                        SurfaceLayoutNode::single(ids.conversation),
                        SurfaceLayoutNode::Stack {
                            surfaces: vec![ids.brief, ids.organization],
                            active_surface_id: ids.brief,
                        },
                    ],
                    preferred_sizes: vec![620.0],
                },
                SurfaceLayoutNode::single(ids.activity),
            ],
            preferred_sizes: vec![560.0],
        };
        LayoutInstance::new_built_in(
            LayoutScope::WorkItem {
                project_id: self.workspace_state.project().id,
                work_item_id: self.workspace_work_item_id,
            },
            BuiltInLayout::Shape,
            root,
            vec![ids.conversation, ids.brief, ids.organization, ids.activity],
            LayoutChromeState::default(),
            Some(ids.conversation),
            false,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    pub(crate) fn coordinate_layout_instance(
        &self,
    ) -> Result<LayoutInstance, dragent_workspace_model::LayoutValidationError> {
        let ids = self.work_surface_ids;
        let root = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Vertical,
            children: vec![
                SurfaceLayoutNode::Split {
                    axis: LayoutAxis::Horizontal,
                    children: vec![
                        SurfaceLayoutNode::single(ids.organization),
                        SurfaceLayoutNode::Stack {
                            surfaces: vec![ids.role_detail, ids.attention, ids.tests, ids.artifact],
                            active_surface_id: ids.role_detail,
                        },
                    ],
                    preferred_sizes: vec![880.0],
                },
                SurfaceLayoutNode::single(ids.activity),
            ],
            preferred_sizes: vec![610.0],
        };
        LayoutInstance::new_built_in(
            LayoutScope::WorkItem {
                project_id: self.workspace_state.project().id,
                work_item_id: self.workspace_work_item_id,
            },
            BuiltInLayout::Coordinate,
            root,
            vec![
                ids.organization,
                ids.role_detail,
                ids.attention,
                ids.tests,
                ids.artifact,
                ids.activity,
            ],
            LayoutChromeState::default(),
            Some(ids.organization),
            false,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    pub(crate) fn review_layout_instance(
        &self,
    ) -> Result<LayoutInstance, dragent_workspace_model::LayoutValidationError> {
        let ids = self.work_surface_ids;
        let root = SurfaceLayoutNode::Split {
            axis: LayoutAxis::Horizontal,
            children: vec![
                SurfaceLayoutNode::single(ids.brief),
                SurfaceLayoutNode::Stack {
                    surfaces: vec![ids.tests, ids.artifact, ids.attention],
                    active_surface_id: ids.tests,
                },
            ],
            preferred_sizes: vec![760.0],
        };
        LayoutInstance::new_built_in(
            LayoutScope::WorkItem {
                project_id: self.workspace_state.project().id,
                work_item_id: self.workspace_work_item_id,
            },
            BuiltInLayout::Review,
            root,
            vec![ids.brief, ids.tests, ids.artifact, ids.attention],
            LayoutChromeState::default(),
            Some(ids.brief),
            false,
            chrono::Utc::now().timestamp_millis(),
        )
    }
}
