//! Project sidebar view — right panel styled to match the React reference layout.

use super::sidebar_explorer::buffer_search_tooltip;
use super::sidebar_explorer::{project_search_hit_range, toggle_project_search_file};
use super::sidebar_explorer::{
    render_documents_section, render_explorer_context_menu, render_explorer_header,
    render_search_file_identity,
};
use super::sidebar_githistory::render_sidebar_tab_header;
use super::sidebar_panels::{
    render_errors_section, render_language_diagnostics_section, render_project_section,
};
use super::sidebar_review::render_review_panel;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use web_time::Instant;

use warpui::color::ColorU;
use warpui::elements::new_scrollable::{
    ClippedAxisConfiguration, DualAxisConfig, ScrollableAppearance,
};
use warpui::elements::{
    Align, ChildAnchor, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CrossAxisAlignment, DispatchEventResult, DraggableState, Element, Empty,
    EventHandler, Expanded, Fill, Flex, Highlight, Image, MainAxisAlignment, MainAxisSize,
    OffsetPositioning, ParentAnchor, ParentElement, ParentOffsetBounds, Point, Resizable,
    ResizableStateHandle, SavePosition, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::r#async::Timer;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EntityId, EventContext, LayoutContext, ModelHandle,
    PaintContext, SizeConstraint, TypedActionView, UpdateModel, UpdateView, View, ViewContext,
    ViewHandle,
};
use warpui_core::assets::asset_cache::{AssetSource, AsyncAssetId, AsyncAssetType};
use warpui_core::elements::{ui_line_height_ratio, Clipped, Hoverable, MouseStateHandle};
use warpui_core::event::DispatchedEvent;
use warpui_core::image_cache::CacheOption;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;
use warpui_core::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui_core::ui_components::tool_tip::Tooltip;
use warpui_core::units::Pixels;

use crate::icons::{render_icon, AppIcon, ICON_SIZE_SM};
use crate::process_ext::NoWindow;
use crate::terminal::render::markdown::render_markdown_document_with_links;
use crate::theme::Theme;
use crate::workspace::{
    editor_render,
    share_sheet::{avatar_sized, avatar_sized_from_source},
    EditorContextMenuCommand, InlineRenameState, InputEditor, InputEditorEvent, InputMode,
    LanguageNavigationCapabilities, LanguageNavigationKind, WorkspaceAction,
};
use dragent_editor::{
    find_buffer_matches, lsp_range_to_char_range, BufferSearchOptions, LineChangeKind, TextRange,
};

use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};

pub(crate) const SECTION_SIZE: f32 = 9.0;
pub(crate) const ROW_SIZE: f32 = 10.0;
pub(crate) const NAME_SIZE: f32 = 12.0;
pub(crate) const PATH_SIZE: f32 = 9.5;
pub(crate) const SCRIPT_CMD_SIZE: f32 = 10.0;
pub(crate) const META_SIZE: f32 = 9.0;
pub(crate) const FILE_TREE_SIZE: f32 = 14.0;
pub(crate) const MULTI_FILE_SEARCH_OVERSCAN_ROWS: usize = 8;
/// Safety ceiling for the shared source editor. The normal editor is bounded
/// by its pane, while an active multi-diff file receives its complete projected
/// height so the aggregate diff remains the only vertical scroll surface.
/// 64k px comfortably covers the same ~2,000-row budget used by inactive diff
/// previews without laying out an unbounded generated file in one frame.
pub(crate) const MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT: f32 = 64_000.0;

pub(crate) fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "md" | "markdown" | "mdown" | "mkd" | "mkdn"
    )
}

pub(crate) fn source_line_changes(
    diff: &dragent_document::DocumentDiff,
    git_changes: Option<&std::collections::HashMap<usize, LineChangeKind>>,
    git_file_changed: bool,
    current_line_count: usize,
) -> std::collections::HashMap<usize, LineChangeKind> {
    use dragent_document::DiffLineKind;

    let mut markers = std::collections::HashMap::new();
    for hunk in &diff.hunks {
        let mut index = 0;
        let mut previous_new_line = 1usize;
        while index < hunk.lines.len() {
            if hunk.lines[index].kind == DiffLineKind::Context {
                if let Some(line) = hunk.lines[index].new_line {
                    previous_new_line = line;
                }
                index += 1;
                continue;
            }

            let start = index;
            while index < hunk.lines.len() && hunk.lines[index].kind != DiffLineKind::Context {
                index += 1;
            }
            let group = &hunk.lines[start..index];
            let has_removed = group.iter().any(|line| line.kind == DiffLineKind::Removed);
            let added = group
                .iter()
                .filter_map(|line| {
                    (line.kind == DiffLineKind::Added)
                        .then_some(line.new_line)
                        .flatten()
                })
                .collect::<Vec<_>>();

            if !added.is_empty() {
                for line in added {
                    markers.insert(
                        line,
                        if has_removed {
                            LineChangeKind::Modified
                        } else {
                            LineChangeKind::Added
                        },
                    );
                    previous_new_line = line;
                }
            } else if has_removed {
                let next_new_line = hunk.lines[index..]
                    .iter()
                    .find_map(|line| line.new_line)
                    .unwrap_or(previous_new_line);
                markers.insert(next_new_line.max(1), LineChangeKind::Deleted);
            }
        }
    }
    if let Some(git_changes) = git_changes {
        for (line, kind) in git_changes {
            markers.entry(*line).or_insert(*kind);
        }
    } else if git_file_changed {
        // `git diff` has no hunks for untracked files. Treat every visible
        // line as added so a newly-created file gets the same gutter signal
        // as a tracked file with additions.
        for line in 1..=current_line_count {
            markers.entry(line).or_insert(LineChangeKind::Added);
        }
    }
    markers
}

pub(crate) fn source_line_changes_for_model(
    model: &ProjectSidebarModel,
) -> std::collections::HashMap<usize, LineChangeKind> {
    let Some(buffer) = model.buffer.as_ref() else {
        return std::collections::HashMap::new();
    };
    // Large-file mode deliberately omits whole-document Git gutter maps. An
    // untracked generated file can otherwise allocate one HashMap entry per
    // line before the editor has painted its first viewport.
    if buffer.capabilities().large_file {
        return std::collections::HashMap::new();
    }
    let path = buffer.document().relative_path.clone();
    let line_count = buffer.capabilities().line_count.max(1);
    source_line_changes(
        &model.document_diff,
        model.git_changed_line_ranges.get(&path),
        model.git_path_changed(&path, false),
        line_count,
    )
}
pub(crate) const BADGE_SIZE: f32 = 8.5;

#[derive(Clone, Debug)]
pub struct LanguageLocationItem {
    pub relative_path: std::path::PathBuf,
    pub range: TextRange,
    /// Text of the line containing this reference — powers the VS Code-style
    /// peek preview rows. `None` when the file could not be read.
    pub line_text: Option<String>,
    /// Char range of the match within `line_text`, for match highlighting.
    pub match_range: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub enum SidebarAction {
    DiffCommentStart { path: PathBuf, target: crate::git_status::diff_head::GitDiffTarget, line: usize, deleted: bool },
    DiffCommentExtend { path: PathBuf, target: crate::git_status::diff_head::GitDiffTarget, line: usize, deleted: bool },
    DiffCommentFinish,
    DiffCommentCancel,
    /// Open or close one of the composer's menus (agent or placement).
    DiffCommentMenu(super::sidebar_diff_comments::ComposerMenu),
    DiffCommentPickerMove(i32),
    DiffCommentPickerAccept,
    DiffCommentPickerDismiss,
    /// Empty the agent menu's search field.
    DiffCommentClearSearch,
    DiffCommentChoose(String),
    DiffCommentPlacement(super::sidebar_diff_comments::Placement),
    DiffCommentSend,
    OpenExplorerContextMenu {
        relative_path: PathBuf,
        directory: bool,
        position: Vector2F,
    },
    CloseExplorerContextMenu,
    ToggleGitViewOptions,
    ToggleGitDiffStats,
    ToggleGitFilter,
    ClearGitFilter,
    ToggleReviewChecks,
    ToggleReviewComments,
    ToggleReviewResolved,
    CopyReviewComment {
        id: String,
    },
    CopyAllReviewComments,
    ResolveReviewThread {
        thread_id: String,
        resolve: bool,
    },
    ToggleGitGroup {
        staged: bool,
    },
    SetGitPanelMode(GitPanelMode),
    ToggleGitDiffLayout,
    ToggleGitDiffFullFile,
    ExpandGitDiffRegion(usize),
    NavigateGitDiffHunk(i32),
    ToggleGitMultiFile {
        path: PathBuf,
        target: crate::git_status::diff_head::GitDiffTarget,
    },
    ToggleGitMultiHunk {
        path: PathBuf,
        target: crate::git_status::diff_head::GitDiffTarget,
        hunk: usize,
    },
    ActivateGitMultiFile {
        path: PathBuf,
        line: usize,
        target: crate::git_status::diff_head::GitDiffTarget,
    },
    // Global project search actions
    ToggleGlobalSearchReplace,
    ToggleGlobalSearchOption {
        option: String,
    },
    ToggleGlobalSearchFileCollapse {
        path: PathBuf,
    },
    ActivateGlobalSearchMatch {
        path: PathBuf,
        hit: usize,
    },
    SelectGlobalSearchMatch {
        path: PathBuf,
        hit: usize,
    },
    NavigateGlobalSearchMatch {
        previous: bool,
    },
    ReplaceAllGlobalSearchMatches,
    ToggleAllGlobalSearchFiles,
    ToggleGlobalSearchDetails,
}

#[derive(Clone, Debug)]
pub(crate) struct ExplorerContextMenuState {
    pub(crate) relative_path: PathBuf,
    pub(crate) directory: bool,
    pub(crate) position: Vector2F,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GitPanelMode {
    #[default]
    Changes,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHistoryEntry {
    pub(crate) sha: String,
    pub(crate) short_sha: String,
    pub(crate) author: String,
    pub(crate) author_email: String,
    pub(crate) relative_time: String,
    pub(crate) subject: String,
    pub(crate) avatar: Option<GitCommitAvatar>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitCommitAvatar {
    GitHub(String),
    GitLab {
        host: String,
        project: String,
        sha: String,
        email: String,
    },
}

pub(crate) struct GitLabAvatarAsset;
impl AsyncAssetType for GitLabAvatarAsset {}

#[derive(Clone, Debug, Default)]
pub enum GitHistoryState {
    #[default]
    Loading,
    Loaded(Vec<GitHistoryEntry>),
    Error(String),
}

pub struct ProjectSidebarView {
    pub(crate) diff_comment: Option<super::sidebar_diff_comments::DiffComment>,
    pub(crate) diff_comment_drag: Option<super::sidebar_diff_comments::DiffCommentAnchor>,
    pub(crate) diff_comment_editor: ViewHandle<InputEditor>,
    pub(crate) diff_comment_search: ViewHandle<InputEditor>,
    /// Hover state for diff lines and composer controls, keyed so it
    /// survives redraws (a fresh handle per draw never reports hover).
    pub(crate) diff_comment_mouse:
        std::cell::RefCell<std::collections::HashMap<String, warpui_core::elements::MouseStateHandle>>,
    pub model: ModelHandle<ProjectSidebarModel>,
    pub(crate) workspace_view_id: EntityId,
    pub theme: Theme,
    pub(crate) ui_font: FamilyId,
    pub(crate) mono_font: FamilyId,
    pub(crate) scroll: ClippedScrollStateHandle,
    pub(crate) bottom_panel_scroll: ClippedScrollStateHandle,
    pub(crate) markdown_preview_scroll: ClippedScrollStateHandle,
    pub(crate) git_diff_scroll: ClippedScrollStateHandle,
    pub(crate) git_multi_horizontal_scroll: ClippedScrollStateHandle,
    pub(crate) git_diff_side_by_side: bool,
    pub(crate) git_diff_show_full_file: bool,
    pub(crate) git_diff_expanded_regions: std::collections::HashSet<usize>,
    pub(crate) git_diff_active_hunk: usize,
    pub(crate) git_multi_collapsed:
        std::collections::HashSet<(PathBuf, crate::git_status::diff_head::GitDiffTarget)>,
    pub(crate) git_multi_hunks_collapsed:
        std::collections::HashSet<(PathBuf, crate::git_status::diff_head::GitDiffTarget, usize)>,
    pub(crate) pending_git_edit_line: Option<(PathBuf, usize)>,
    pub(crate) pending_git_edit_scroll_anchor: Option<f32>,
    pub(crate) language_locations_scroll: ClippedScrollStateHandle,
    pub(crate) language_locations_panel_state: ResizableStateHandle,
    pub(crate) error_panel_state: ResizableStateHandle,
    pub(crate) quick_action_mouse_states:
        RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) error_mouse_states: RefCell<std::collections::HashMap<usize, MouseStateHandle>>,
    pub(crate) document_mouse_states: RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) document_action_mouse_states:
        RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) explorer_menu_mouse_states:
        RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) explorer_context_menu: Option<ExplorerContextMenuState>,
    pub(crate) explorer_can_paste: bool,
    pub(crate) document_tab_drag_states:
        RefCell<std::collections::HashMap<std::path::PathBuf, DraggableState>>,
    pub(crate) explorer_file_drag_states:
        RefCell<std::collections::HashMap<std::path::PathBuf, DraggableState>>,
    pub(crate) document_editor: ViewHandle<InputEditor>,
    pub(crate) language_rename_editor: ViewHandle<InputEditor>,
    pub(crate) buffer_search_editor: ViewHandle<InputEditor>,
    pub(crate) buffer_replace_editor: ViewHandle<InputEditor>,
    pub(crate) buffer_search_open: bool,
    pub(crate) buffer_replace_open: bool,
    pub(crate) buffer_search_options: BufferSearchOptions,
    /// Which surface the sidebar shows: file explorer or source control.
    pub sidebar_tab: super::SidebarTab,
    /// Commit message editor for the source-control panel.
    pub git_commit_editor: ViewHandle<InputEditor>,
    /// File filter editor + state for the source-control panel. Filters
    /// staged and unstaged rows by path substring.
    pub(crate) git_filter_editor: ViewHandle<InputEditor>,
    pub(crate) git_filter_text: String,
    pub(crate) git_filter_open: bool,
    pub git_changes_scroll: ClippedScrollStateHandle,
    pub git_history_scroll: ClippedScrollStateHandle,
    pub git_panel_mode: GitPanelMode,
    pub git_view_options_open: bool,
    pub git_show_diff_stats: bool,
    pub git_staged_collapsed: bool,
    pub git_changes_collapsed: bool,
    pub git_history: GitHistoryState,
    pub git_commit_generating: bool,
    pub git_commit_generation_error: Option<String>,
    /// Review tab data (pull request + comments), pushed by the
    /// workspace's 10s `gh` poll. Rendering only — fetching lives in the
    /// workspace view next to the other background polls.
    pub review_pr: super::ReviewPrState,
    pub(crate) review_checks_open: bool,
    pub(crate) review_comments_open: bool,
    pub(crate) review_resolved_open: bool,
    pub(crate) review_action_error: Option<String>,
    /// Transient copy-success feedback (checkmark ~1.5s): copy-all button
    /// and the id of the last per-comment copy. Timers clear them.
    pub(crate) review_copy_all_copied: bool,
    pub(crate) review_copied_comment: Option<String>,
    /// Thread id with a resolve/unresolve mutation in flight. The button
    /// shows a pending label and ignores clicks until it clears.
    pub(crate) review_resolving_thread: Option<String>,
    /// Live git snapshot shared from the workspace render (the workspace
    /// writes it from the active session's git model; this panel only reads).
    pub git_panel: std::rc::Rc<std::cell::RefCell<super::GitPanelState>>,
    /// This view's own entity id — used to dispatch sidebar-local actions.
    pub(crate) view_id: EntityId,
    pub(crate) sidebar_tab_mouse_states:
        RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) git_panel_mouse_states: RefCell<std::collections::HashMap<String, MouseStateHandle>>,
    pub(crate) buffer_search_matches: Vec<std::ops::Range<usize>>,
    pub(crate) buffer_search_active: Option<usize>,
    pub(crate) buffer_search_error: Option<String>,
    // Global project search (VS Code style)
    pub(crate) global_search_query_editor: ViewHandle<InputEditor>,
    pub(crate) global_search_replace_editor: ViewHandle<InputEditor>,
    pub(crate) global_search_include_editor: ViewHandle<InputEditor>,
    pub(crate) global_search_exclude_editor: ViewHandle<InputEditor>,
    pub global_search_results: Option<Arc<crate::workspace::search_panel::SearchResults>>,
    pub global_search_in_progress: bool,
    pub(crate) global_search_replacing: bool,
    pub global_search_scroll: ClippedScrollStateHandle,
    pub(crate) global_search_editor_scroll: ClippedScrollStateHandle,
    pub(crate) global_search_editor_horizontal_scroll: ClippedScrollStateHandle,
    pub global_search_collapsed: std::collections::HashSet<PathBuf>,
    pub global_search_error: Option<String>,
    pub global_search_epoch: u64,
    pub(crate) global_search_generation: Arc<AtomicU64>,
    /// Folder whose results are currently represented by the shared search
    /// controls. Workspace tabs can point at different folders, so results
    /// must never survive a tab/root transition merely because the query text
    /// itself did not change.
    pub(crate) global_search_context_root: Option<PathBuf>,
    /// Exact root used to produce `global_search_results`. It can differ from
    /// the terminal CWD when search is scoped to the enclosing project.
    pub(crate) global_search_results_root: Option<PathBuf>,
    pub(crate) global_search_active: Option<(PathBuf, usize)>,
    pub(crate) global_search_options: crate::workspace::search_panel::SearchOptions,
    pub(crate) global_search_replace_open: bool,
    pub(crate) global_search_details_open: bool,
    pub(crate) editor_document_path: Option<std::path::PathBuf>,
    pub(crate) editor_document_key: Option<PathBuf>,
    pub(crate) editor_view_states:
        HashMap<PathBuf, crate::workspace::input_editor::model::EditorViewState>,
    pub(crate) markdown_preview_open: bool,
    pub(crate) language_rename_open: bool,
    pub(crate) language_locations_title: Option<String>,
    pub(crate) language_locations_root: Option<PathBuf>,
    pub(crate) language_locations: Vec<LanguageLocationItem>,
    pub(crate) language_code_actions: Vec<(String, serde_json::Value)>,
    /// Debounced autosave generation.
    pub(crate) autosave_generation: Arc<AtomicU64>,
}

impl Entity for ProjectSidebarView {
    type Event = ();
}

impl View for ProjectSidebarView {
    fn ui_name() -> &'static str {
        "ProjectSidebarView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let theme = self.theme;
        let ui = self.ui_font;
        let mono = self.mono_font;
        let scroll = self.scroll.clone();
        let bottom_panel_scroll = self.bottom_panel_scroll.clone();
        let error_panel_state = self.error_panel_state.clone();

        self.model.read(app, |m, _| {
            let active_git_root = m
                .active_cwd
                .as_ref()
                .and_then(|cwd| detect::find_git_root(cwd));

            let active_project = active_git_root
                .as_ref()
                .and_then(|root| m.projects.iter().find(|p| &p.git_root == root));

            let mut sections: Vec<Box<dyn Element>> = Vec::new();

            if self.sidebar_tab == super::SidebarTab::Files {
                // File names are part of the code workspace, so they follow the
                // user's selected editor family and size rather than the fixed UI
                // font used by surrounding sidebar chrome.
                sections.push(render_documents_section(m, ui, mono, &theme, self));
            }

            let changed_file_count = self
                .git_panel
                .borrow()
                .git_model
                .as_ref()
                .and_then(|model| {
                    model.read(app, |model, _| {
                        model.status.as_ref().map(|status| status.total_changes())
                    })
                })
                .unwrap_or(0);
            // Tab strip: Files (explorer) | Changes (git workspace).
            let tab_header = render_sidebar_tab_header(
                self.sidebar_tab,
                changed_file_count,
                self.workspace_view_id,
                ui,
                &theme,
                &self.sidebar_tab_mouse_states,
            );

            let middle: Box<dyn Element> = if self.sidebar_tab == super::SidebarTab::SourceControl {
                self.render_git_panel(app, &theme, ui, mono)
            } else if self.sidebar_tab == super::SidebarTab::Search {
                self.render_search_panel(ui, mono, &theme, app)
            } else if self.sidebar_tab == super::SidebarTab::Review {
                let review_root = self.model.read(app, |model, _| model.active_cwd.clone());
                render_review_panel(self, ui, &theme, review_root)
            } else {
                let mut content_col = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Min);
                for section in sections {
                    content_col = content_col.with_child(section);
                }
                ClippedScrollable::vertical(
                    scroll,
                    content_col.finish(),
                    ScrollbarWidth::Auto,
                    theme.disabled_text().into(),
                    theme.sub_text().into(),
                    theme.panel_background().into(),
                )
                .with_overlayed_scrollbar()
                .finish()
            };

            // Error + language diagnostics share the resizable lower tray
            // instead of competing with the file tree for the primary scroll
            // area. Quick actions were removed from the right sidebar.
            let mut bottom_sections = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min);
            bottom_sections =
                bottom_sections.with_child(render_errors_section(m, ui, mono, &theme, self));
            let language_diagnostics = m.active_language_diagnostics();
            if !language_diagnostics.is_empty() {
                bottom_sections = bottom_sections.with_child(render_language_diagnostics_section(
                    &language_diagnostics,
                    m.workspace_root().to_path_buf(),
                    m.active_relative_path(),
                    self.workspace_view_id,
                    &self.document_action_mouse_states,
                    ui,
                    mono,
                    &theme,
                ));
            }

            let bottom_content = ClippedScrollable::vertical(
                bottom_panel_scroll,
                bottom_sections.finish(),
                ScrollbarWidth::Auto,
                theme.disabled_text().into(),
                theme.sub_text().into(),
                theme.panel_background().into(),
            )
            .finish();
            // Default tray height raised 40% (0.30 → 0.42). With quick
            // actions gone the error list itself gets roughly 3x the visible
            // rows it had before. Max raised so the tray can still be dragged
            // to ~3x the old size when a long error list needs it.
            let error_panel = Resizable::new(error_panel_state, bottom_content)
                .with_dragbar_side(warpui::elements::DragBarSide::Top)
                .with_initial_fraction(0.42)
                .with_dragbar_color(theme.accent_overlay().into())
                .on_resize(|ctx, _| ctx.notify())
                .with_bounds_callback(Box::new(|window_size| {
                    let min_height = 90.0;
                    let max_height = (window_size.y() * 0.8).max(min_height);
                    (min_height, max_height)
                }))
                .finish();

            let mut body = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Max);
            body = body.with_child(tab_header);
            if let Some(project) = active_project {
                if self.sidebar_tab == super::SidebarTab::Files {
                    // Project identity and runtime metadata remain pinned while
                    // the document tree below it scrolls independently. The
                    // source-control tab hides it: the git panel header carries
                    // branch/repo identity instead.
                    body = body.with_child(render_project_section(project, true, ui, mono, &theme));
                }
            }
            if self.sidebar_tab == super::SidebarTab::Files {
                body = body.with_child(render_explorer_header(m, ui, &theme, self));
            }
            body = body
                .with_child(Expanded::new(1.0, middle).finish())
                .with_child(error_panel);

            let body = Container::new(body.finish())
                .with_background_color(theme.panel_background())
                .with_border(Border::left(1.0).with_border_color(theme.border()))
                .finish();
            let body = SavePosition::new(body, "project_sidebar_context_root").finish();
            if let Some(menu_state) = self.explorer_context_menu.as_ref() {
                let menu = render_explorer_context_menu(
                    menu_state,
                    self.explorer_can_paste,
                    m.active_cwd
                        .clone()
                        .unwrap_or_else(|| m.workspace_root().to_path_buf()),
                    self.view_id,
                    self.workspace_view_id,
                    ui,
                    &theme,
                    &self.explorer_menu_mouse_states,
                );
                let view_id = self.view_id;
                let menu = warpui_core::elements::Dismiss::new(menu)
                    .on_dismiss(move |event_ctx, _app| {
                        event_ctx.dispatch_typed_action_for(
                            view_id,
                            SidebarAction::CloseExplorerContextMenu,
                        );
                    })
                    .prevent_interaction_with_other_elements()
                    .finish();
                let offset = OffsetPositioning::offset_from_parent(
                    menu_state.position,
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                );
                Stack::new()
                    .with_child(body)
                    .with_positioned_overlay_child(menu, offset)
                    .finish()
            } else {
                body
            }
        })
    }
}
