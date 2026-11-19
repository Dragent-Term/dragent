//! Picker and menu rendering for workspace tabs.
//!
//! Responsible for: new-tab picker, add-project menu, picker items.
//! Blocks here are free functions and impl WorkspaceView: same behavior.

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

pub(crate) fn picker_mouse(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: String,
) -> MouseStateHandle {
    states.borrow_mut().entry(key).or_default().clone()
}

/// What leads a row of a `+` menu.
#[derive(Clone, Copy)]
enum MenuMark {
    Icon(AppIcon),
    /// A bundled image (the Dragent logo).
    Image(&'static str),
}

fn menu_mark(mark: MenuMark, theme: &Theme) -> Box<dyn Element> {
    let mark: Box<dyn Element> = match mark {
        MenuMark::Icon(icon) => render_icon(icon, theme.main_text(), 14.0),
        MenuMark::Image(path) => ConstrainedBox::new(
            warpui::elements::Image::new(
                warpui_core::assets::asset_cache::AssetSource::Bundled { path },
                warpui_core::image_cache::CacheOption::BySize,
            )
            .contain()
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish(),
        )
        .with_width(16.0)
        .with_height(16.0)
        .finish(),
    };
    ConstrainedBox::new(Align::new(mark).finish()).with_width(16.0).with_height(16.0).finish()
}

/// One row of a `+` menu, built exactly like a row of the agent popover:
/// filled row that lightens under the pointer, 16 px mark, title, a muted
/// second line when there is one, and a muted note (a shortcut) at the end.
fn popover_row(
    theme: &Theme,
    ui_font: FamilyId,
    mouse_state: MouseStateHandle,
    mark: MenuMark,
    title: &str,
    detail: Option<&str>,
    note: Option<&str>,
    workspace_view_id: EntityId,
    action: WorkspaceAction,
    then: Option<WorkspaceAction>,
) -> Box<dyn Element> {
    let theme = *theme;
    let (title, detail, note) = (title.to_string(), detail.map(str::to_string), note.map(str::to_string));
    Hoverable::new(mouse_state, move |state| {
        let mut words = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0)
            .with_child(
                Text::new(title.clone(), ui_font, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .soft_wrap(false)
                    .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                    .finish(),
            );
        if let Some(detail) = &detail {
            words = words.with_child(
                Text::new(detail.clone(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(warpui_core::text_layout::ClipConfig::ellipsis())
                    .finish(),
            );
        }
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(menu_mark(mark, &theme))
            .with_child(Expanded::new(1.0, words.finish()).finish());
        if let Some(note) = &note {
            row = row.with_child(
                Text::new(note.clone(), ui_font, crate::theme::text_size(11.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            );
        }
        Container::new(row.finish())
            .with_background_color(if state.is_hovered() { theme.surface_3() } else { theme.surface_2() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
        if let Some(then) = &then {
            event_ctx.dispatch_typed_action_for(workspace_view_id, then.clone());
        }
    })
    .finish()
}

/// Groups of rows under the agent popover's captions, in its card.
fn popover_card(theme: &Theme, ui_font: FamilyId, groups: Vec<(&'static str, Vec<Box<dyn Element>>)>, width: f32) -> Box<dyn Element> {
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    for (caption, rows) in groups.into_iter().filter(|(_, rows)| !rows.is_empty()) {
        col = col.with_child(super::project_prompt::menu_caption(theme, ui_font, caption));
        for row in rows {
            col = col.with_child(row);
        }
    }
    super::project_prompt::menu_card(theme, col.finish(), width)
}

pub(crate) fn render_new_tab_picker(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui_font: FamilyId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    // The earlier work items stay out of here; a team starts from the
    // project's prompt screen, like any agent.
    let row = |key: &str, mark: MenuMark, title: &str, note: Option<&str>, action: WorkspaceAction| {
        popover_row(theme, ui_font, picker_mouse(mouse_states, format!("ntpicker_{key}")), mark, title, None, note, workspace_view_id, action, None)
    };
    let agents = vec![
        row("dragent", MenuMark::Image("icons/dragent-icon.png"), "Dragent", Some("⌘T"), WorkspaceAction::NewAgentTab),
        row("coordination", MenuMark::Icon(AppIcon::Layers), "Agent Coordination", None, WorkspaceAction::OpenTeamPrompt),
        row("past", MenuMark::Icon(AppIcon::Asset("icons/history.svg")), "Past Sessions", None, WorkspaceAction::OpenPastSessions),
        row("cloud", MenuMark::Icon(AppIcon::Agent), "Cloud Agent", None, WorkspaceAction::NewAgentTab),
    ];
    let tabs = vec![
        row("terminal", MenuMark::Icon(AppIcon::Terminal), "Terminal", None, WorkspaceAction::NewTab),
        row("browser", MenuMark::Icon(AppIcon::Globe), "Browser", None, WorkspaceAction::NewBrowserTab { url: None }),
    ];
    let project = vec![row("open_folder", MenuMark::Icon(AppIcon::FolderOpen), "Open Folder…", None, WorkspaceAction::OpenFolder)];
    popover_card(theme, ui_font, vec![("AGENTS", agents), ("NEW TAB", tabs), ("PROJECT", project)], 250.0)
}

/// Add-project popover under the sidebar add button: Open project, Create
/// new project, Clone repository. Same overlay mechanics and card language
/// as the new-tab picker above.
pub(crate) fn render_add_project_menu(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui_font: FamilyId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let rows = [
        (
            "addproj_open",
            AppIcon::FolderOpen,
            "Open project",
            "Choose a folder on this computer",
            WorkspaceAction::OpenFolder,
        ),
        (
            "addproj_create",
            AppIcon::Plus,
            "Create new project",
            "Blank folder, initialized as a Git repository",
            WorkspaceAction::OpenCreateProject,
        ),
        (
            "addproj_clone",
            AppIcon::GitBranch,
            "Clone repository",
            "HTTPS or Git SSH repository URL",
            WorkspaceAction::ShowAddProjectCloneForm,
        ),
    ];
    let rows = rows
        .into_iter()
        .map(|(key, icon, title, detail, action)| {
            // Every choice but the clone form closes the menu.
            let then = (!matches!(action, WorkspaceAction::ShowAddProjectCloneForm)).then_some(WorkspaceAction::CloseAddProjectMenu);
            popover_row(theme, ui_font, picker_mouse(mouse_states, key.to_string()), MenuMark::Icon(icon), title, Some(detail), None, workspace_view_id, action, then)
        })
        .collect();
    popover_card(theme, ui_font, vec![("ADD A PROJECT", rows)], 264.0)
}

/// One row of the header's new-tab menu.
type NewTabRow = (&'static str, MenuMark, &'static str, &'static str, WorkspaceAction);

/// The menu's terminal rows. Windows lists its shells by name, Git Bash (when
/// installed) and PowerShell, each opening a tab in that shell; elsewhere a
/// single row opens the default shell.
fn terminal_rows() -> Vec<NewTabRow> {
    let terminal = (
        "newtab_terminal",
        MenuMark::Icon(AppIcon::Terminal),
        "Terminal",
        "A shell in this workspace",
        WorkspaceAction::NewWorkspaceTab,
    );
    if !cfg!(windows) {
        return vec![terminal];
    }
    let shells = dragent_pty::available_shells();
    let find = |label: &str| {
        shells
            .iter()
            .find(|shell| shell.label == label)
            .map(|shell| shell.path.clone())
    };
    let mut rows = Vec::new();
    if let Some(shell) = find("Git Bash") {
        rows.push((
            "newtab_git_bash",
            MenuMark::Icon(AppIcon::Terminal),
            "Git Bash",
            "A Git Bash shell in this workspace",
            WorkspaceAction::NewShellWorkspaceTab { shell },
        ));
    }
    // PowerShell 7 when installed, else the Windows PowerShell every
    // machine has.
    if let Some(shell) = find("PowerShell 7").or_else(|| find("Windows PowerShell")) {
        rows.push((
            "newtab_powershell",
            MenuMark::Icon(AppIcon::Terminal),
            "PowerShell",
            "A PowerShell shell in this workspace",
            WorkspaceAction::NewShellWorkspaceTab { shell },
        ));
    }
    if rows.is_empty() {
        rows.push(terminal);
    }
    rows
}

/// The header `+` menu: a plain terminal, or one already running the
/// Dragent command line's help. Same card language as the add-project
/// menu.
pub(crate) fn render_header_new_tab_menu(
    workspace_view_id: EntityId,
    theme: &Theme,
    ui_font: FamilyId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    // The earlier work item workflow is switched on (Settings → Agent).
    work_items: bool,
) -> Box<dyn Element> {
    let mut rows = terminal_rows();
    rows.extend([
        (
            "newtab_browser",
            MenuMark::Icon(AppIcon::Globe),
            "Browser",
            "Preview your app or open any site",
            WorkspaceAction::NewBrowserWorkspaceTab,
        ),
        (
            "newtab_browser_side",
            MenuMark::Icon(AppIcon::SplitHorizontal),
            "Browser to the side",
            "Open a browser next to the current pane",
            WorkspaceAction::SplitActiveTabWithBrowser,
        ),
        (
            "newtab_dragent_cli",
            MenuMark::Image("icons/dragent-icon.png"),
            "Dragent CLI",
            "Script workspaces, agents and automations",
            WorkspaceAction::NewDragentCliTab,
        ),
        (
            "newtab_past_sessions",
            MenuMark::Icon(AppIcon::Asset("icons/history.svg")),
            "Past sessions",
            "Continue an earlier agent conversation",
            WorkspaceAction::OpenPastSessions,
        ),
        (
            "newtab_team",
            MenuMark::Icon(AppIcon::Layers),
            "Agent coordination",
            "A lead agent plans it; workers build it in parallel",
            WorkspaceAction::OpenTeamPrompt,
        ),
    ]);
    if work_items {
        rows.push((
            "newtab_work_item",
            MenuMark::Icon(AppIcon::TaskList),
            "Work item",
            "The earlier Shape → Coordinate → Review workflow",
            WorkspaceAction::NewWorkItemTab,
        ));
    }
    let row = |(key, mark, title, detail, action): NewTabRow| {
        popover_row(theme, ui_font, picker_mouse(mouse_states, key.to_string()), mark, title, Some(detail), None, workspace_view_id, action, None)
    };
    let (agents, tabs): (Vec<NewTabRow>, Vec<NewTabRow>) = rows.into_iter().partition(|(key, ..)| {
        matches!(*key, "newtab_past_sessions" | "newtab_team" | "newtab_work_item")
    });
    popover_card(
        theme,
        ui_font,
        vec![("NEW TAB", tabs.into_iter().map(row).collect()), ("AGENTS", agents.into_iter().map(row).collect())],
        264.0,
    )
}
