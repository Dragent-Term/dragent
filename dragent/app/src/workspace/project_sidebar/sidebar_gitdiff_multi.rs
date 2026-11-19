//! Git multi-diff renders.
//!
//! Moved verbatim; same behavior.

use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};
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
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
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
use web_time::Instant;

use super::sidebar_explorer::buffer_search_tooltip;
use super::sidebar_gitdiff_file::git_multi_hunk_action_button;
use super::sidebar_panels::with_alpha;
use super::view::{ProjectSidebarView, SidebarAction, MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT};

fn git_status_color(status: char, theme: &Theme) -> ColorU {
    match status {
        'A' | '?' => theme.ui_green(),
        'D' => theme.ui_red(),
        'R' | 'C' => theme.ansi_blue(),
        _ => theme.ui_yellow(),
    }
}

pub(crate) fn render_git_multi_diff(
    diff: &crate::git_status::diff_head::GitMultiDiff,
    comments: &ProjectSidebarView,
    root: &Path,
    loading: bool,
    scroll: ClippedScrollStateHandle,
    horizontal_scroll: ClippedScrollStateHandle,
    workspace_view_id: EntityId,
    sidebar_view_id: EntityId,
    collapsed_files: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
    )>,
    collapsed_hunks: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
        usize,
    )>,
    active_file_diff: Option<&crate::git_status::diff_head::GitFileDiff>,
    active_projected_rows: Option<usize>,
    document_editor: &ViewHandle<InputEditor>,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let sticky_file = git_multi_sticky_file(
        diff,
        collapsed_files,
        collapsed_hunks,
        active_file_diff,
        active_projected_rows,
        comments,
        scroll.scroll_start().as_f32(),
    );
    let widest_changed_line = diff
        .files
        .iter()
        .filter_map(|file| file.diff.as_ref().ok())
        // Rows never lay out more than the display cap (plus its marker), so
        // one minified line must not make every card millions of pixels wide.
        .map(|file| {
            file.max_changed_line_chars
                .min(crate::workspace::editor_render::DIFF_ROW_MAX_CHARS + 32)
        })
        .max()
        .unwrap_or(0);
    let aggregate_content_width = (crate::theme::scaled(150.0)
        + widest_changed_line as f32
            * crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE)
            * 0.62)
        // Preserve the last measured viewport so short diffs still fill the
        // editor instead of shrinking their headers to the text's natural
        // width. The baseline also makes the very first layout finite.
        .max(horizontal_scroll.viewport_width())
        .max(crate::theme::scaled(900.0));
    let scroll_top = scroll.scroll_start().as_f32().max(0.0);
    let measured_viewport_height = scroll.viewport_height();
    let viewport_height = if measured_viewport_height.is_finite() && measured_viewport_height > 0.0
    {
        measured_viewport_height
    } else {
        crate::theme::scaled(900.0)
    };
    // Keep enough nearby content ready for a smooth wheel tick without
    // rebuilding every file card in a large worktree on each editor frame.
    let render_overscan = crate::theme::scaled(180.0);
    let render_window_start = (scroll_top - render_overscan).max(0.0);
    let render_window_end = scroll_top + viewport_height + render_overscan;
    let mut file_top = crate::theme::scaled(64.0);
    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(crate::theme::scaled(3.0))
                    .with_child(
                        Text::new(diff.title.clone(), ui, crate::theme::text_size(15.0))
                            .with_color(theme.main_text())
                            .with_style(Properties::default().weight(Weight::Semibold))
                            .finish(),
                    )
                    .with_child(
                        Text::new(
                            diff.subtitle
                                .clone()
                                .unwrap_or_else(|| format!("{} changed files", diff.files.len())),
                            ui,
                            crate::theme::text_size(11.0),
                        )
                        .with_color(theme.sub_text())
                        .with_clip(ClipConfig::ellipsis())
                        .soft_wrap(false)
                        .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(18.0))
            .with_padding_right(crate::theme::scaled(18.0))
            .with_padding_top(crate::theme::scaled(14.0))
            .with_padding_bottom(crate::theme::scaled(12.0))
            .with_background_color(theme.panel_background())
            .with_border(Border::bottom(1.0).with_border_color(theme.border()))
            .finish(),
        );
    if diff.files.is_empty() {
        return Align::new(
            Text::new(
                if loading {
                    "Loading changed files…"
                } else if diff.read_only {
                    "No changed files in this commit."
                } else {
                    "Working tree is clean."
                }
                .to_string(),
                ui,
                crate::theme::text_size(13.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .finish();
    }
    let active_file_index = active_file_diff.and_then(|active| {
        diff.files.iter().position(|file| {
            file.change.relative_path == active.relative_path && file.change.target == active.target
        })
    });
    for (file_index, file) in diff.files.iter().enumerate() {
        let change = &file.change;
        let file_key = (change.relative_path.clone(), change.target);
        let comment = comments.comment_for_file(&change.relative_path, change.target);
        let collapsed = collapsed_files.contains(&file_key) && comment.is_none();
        let edit_inline = !diff.read_only
            && change.target == crate::git_status::diff_head::GitDiffTarget::Unstaged;
        let active_inline_editor = active_file_diff.is_some_and(|active| {
            active.relative_path == change.relative_path
                && active.target == change.target
                && edit_inline
                && comment.is_none()
        });
        let stable_body_height = if collapsed {
            0.0
        } else if let Some(comment) = comment {
            file.diff.as_ref().map_or(crate::theme::scaled(34.0), |file| comment_preview_height(file, comment))
        } else {
            git_multi_stable_body_height(
                file,
                collapsed_hunks,
                active_inline_editor
                    .then_some(active_projected_rows)
                    .flatten(),
            )
        };
        let stable_body_height = stable_body_height + comment.map_or(0.0, |c| c.height());
        let estimated_card_height = crate::theme::scaled(43.0) + stable_body_height;
        let body_top = file_top + crate::theme::scaled(43.0);
        let file_bottom = file_top + estimated_card_height;
        let outside_render_window =
            file_bottom < render_window_start || file_top > render_window_end;
        if git_multi_should_cull_file(file_index, active_file_index, outside_render_window) {
            content = content.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(estimated_card_height)
                    .finish(),
            );
            file_top = file_bottom;
            continue;
        }
        file_top = file_bottom;
        let path = change.relative_path.to_string_lossy().into_owned();
        let filename = change
            .relative_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let parent = change
            .relative_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.to_string_lossy().into_owned());
        let open_path = change.relative_path.clone();
        let edit_target = change.target;
        let edit_line = file
            .diff
            .as_ref()
            .ok()
            .and_then(|file| {
                file.diff
                    .hunks
                    .iter()
                    .flat_map(|hunk| hunk.lines.iter())
                    .find_map(|line| line.new_line)
            })
            .unwrap_or(1);
        let normal_open_path = open_path.clone();
        let normal_open_root = root.to_path_buf();
        let activate_path = open_path.clone();
        let body_activate_path = open_path.clone();
        let status = change.status;
        // Collapse is an explicit per-view choice. Inactive files remain
        // expanded as lightweight diff previews; selecting one promotes that
        // file to the single live editor without changing any other header.
        let collapse_path = change.relative_path.clone();
        let collapse_target = change.target;
        let stats = file
            .diff
            .as_ref()
            .map(|file| (file.additions(), file.deletions()))
            .unwrap_or_default();
        let open_button = Hoverable::new(MouseStateHandle::default(), move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(crate::theme::scaled(5.0))
                    .with_child(render_icon(
                        AppIcon::Eye,
                        if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        },
                        crate::theme::scaled(11.0),
                    ))
                    .with_child(
                        Text::new("Open File".to_string(), ui, crate::theme::text_size(10.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(7.0))
            .with_padding_right(crate::theme::scaled(7.0))
            .with_padding_top(crate::theme::scaled(4.0))
            .with_padding_bottom(crate::theme::scaled(4.0))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::transparent_black()
            })
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .finish();
        let open_button = EventHandler::new(open_button)
            .on_left_mouse_down(move |ctx, _app, _pos| {
                ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::OpenReadOnlyDocument {
                        // Resolve against the owning worktree root; this opens
                        // the live worktree file rather than a diff snapshot.
                        root: normal_open_root.clone(),
                        relative_path: normal_open_path.clone(),
                    },
                );
                DispatchEventResult::StopPropagation
            })
            .finish();
        let collapse_button = EventHandler::new(
            Container::new(render_icon(
                if collapsed {
                    AppIcon::ChevronRight
                } else {
                    AppIcon::ChevronDown
                },
                theme.disabled_text(),
                crate::theme::scaled(10.0),
            ))
            .with_padding_left(crate::theme::scaled(3.0))
            .with_padding_right(crate::theme::scaled(3.0))
            .with_padding_top(crate::theme::scaled(4.0))
            .with_padding_bottom(crate::theme::scaled(4.0))
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                sidebar_view_id,
                SidebarAction::ToggleGitMultiFile {
                    path: collapse_path.clone(),
                    target: collapse_target,
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish();
        let file_identity = parent
            .map(|parent| format!("{filename}  —  {parent}"))
            .unwrap_or(filename);
        let identity = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(crate::theme::scaled(6.0))
            .with_child(collapse_button)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new(file_identity, mono, crate::theme::text_size(12.0))
                        .with_color(theme.main_text())
                        .with_style(Properties::default().weight(Weight::Medium))
                        .with_clip(ClipConfig::ellipsis())
                        .soft_wrap(false)
                        .finish(),
                )
                .finish(),
            );
        let header = Hoverable::new(MouseStateHandle::default(), move |state| {
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(crate::theme::scaled(8.0))
                    .with_child(Expanded::new(1.0, identity.finish()).finish())
                    .with_child(
                        Text::new(
                            format!("+{} −{}", stats.0, stats.1),
                            mono,
                            crate::theme::text_size(10.0),
                        )
                        .with_color(theme.sub_text())
                        .finish(),
                    )
                    .with_child(
                        Text::new(status.to_string(), mono, crate::theme::text_size(10.5))
                            .with_color(git_status_color(status, theme))
                            .with_style(Properties::default().weight(Weight::Semibold))
                            .finish(),
                    )
                    .with_child(open_button)
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(10.0))
            .with_padding_right(crate::theme::scaled(8.0))
            .with_padding_top(crate::theme::scaled(7.0))
            .with_padding_bottom(crate::theme::scaled(7.0))
            .with_background_color(if state.is_hovered() {
                theme.surface_1()
            } else {
                theme.panel_background()
            })
            .with_border(Border::all(1.0).with_border_color(theme.border()))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .with_defer_events_to_children()
        .on_click(move |ctx, _app, _pos| {
            if edit_inline {
                ctx.dispatch_typed_action_for(
                    sidebar_view_id,
                    SidebarAction::ActivateGitMultiFile {
                        path: activate_path.clone(),
                        line: edit_line,
                        target: edit_target,
                    },
                );
            }
        })
        .finish();
        let body = if collapsed {
            Empty::new().finish()
        } else if active_inline_editor {
            ConstrainedBox::new(
                Container::new(ChildView::new(document_editor).finish())
                    .with_background_color(theme.background)
                    .finish(),
            )
            .with_height(stable_body_height)
            .finish()
        } else {
            match &file.diff {
                Ok(file) => render_git_multi_file_rows(
                    file,
                    comments,
                    &change.relative_path,
                    change.target,
                    !diff.read_only,
                    collapsed_hunks,
                    workspace_view_id,
                    sidebar_view_id,
                    ui,
                    mono,
                    theme,
                    horizontal_scroll.clone(),
                    (render_window_start - body_top, render_window_end - body_top),
                ),
                Err(error) => Container::new(
                    Text::new(error.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(if loading {
                            theme.sub_text()
                        } else {
                            theme.ui_red()
                        })
                        .finish(),
                )
                .with_padding_left(crate::theme::scaled(12.0))
                .with_padding_top(crate::theme::scaled(10.0))
                .with_padding_bottom(crate::theme::scaled(10.0))
                .finish(),
            }
        };
        let body = if edit_inline && !active_inline_editor && !collapsed && comment.is_none() {
            EventHandler::new(body)
                .on_left_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(
                        sidebar_view_id,
                        SidebarAction::ActivateGitMultiFile {
                            path: body_activate_path.clone(),
                            line: edit_line,
                            target: edit_target,
                        },
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish()
        } else {
            body
        };
        let body = if collapsed {
            body
        } else {
            ConstrainedBox::new(body)
                .with_height(stable_body_height)
                .finish()
        };
        let header_height = crate::theme::scaled(34.0);
        let header = GitMultiViewportHeader::new(
            ConstrainedBox::new(header)
                .with_height(header_height)
                .finish(),
            horizontal_scroll.clone(),
            crate::theme::scaled(24.0),
        )
        .finish();
        let card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(header)
            .with_child(body)
            .finish();
        content = content.with_child(
            Container::new(card)
                .with_padding_left(crate::theme::scaled(12.0))
                .with_padding_right(crate::theme::scaled(12.0))
                .with_padding_top(crate::theme::scaled(9.0))
                .finish(),
        );
    }
    let content = ConstrainedBox::new(content.finish())
        // Propagate an exact finite width to every nested expanding header.
        // A minimum alone still lets ConstrainedBox preserve an infinite
        // incoming maximum from a clipped horizontal layout.
        .with_width(aggregate_content_width)
        .finish();
    let scrollable = warpui::elements::NewScrollable::horizontal_and_vertical(
        DualAxisConfig::Clipped {
            horizontal: ClippedAxisConfiguration {
                handle: horizontal_scroll,
                // A clipped horizontal child is normally offered infinity.
                // File and hunk headers intentionally contain expanding flex
                // rows, so give the aggregate document a finite measured width
                // while retaining horizontal overflow for long source lines.
                max_size: Some(aggregate_content_width),
                stretch_child: false,
            },
            vertical: ClippedAxisConfiguration {
                handle: scroll,
                max_size: None,
                stretch_child: false,
            },
            child: content,
        },
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.background.into(),
    )
    .with_horizontal_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
    .with_vertical_scrollbar(ScrollableAppearance::new(ScrollbarWidth::Custom(6.0), true))
    .with_scrollbar_padding(0.0, 0.0)
    .with_wheel_locked_to_dominant_axis(true)
    .finish();
    if let Some(file) = sticky_file {
        let sticky = render_git_multi_sticky_header(
            file,
            root,
            workspace_view_id,
            sidebar_view_id,
            collapsed_files.contains(&(file.change.relative_path.clone(), file.change.target)),
            ui,
            mono,
            theme,
        );
        Stack::new()
            .with_child(scrollable)
            .with_positioned_child(
                sticky,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, 0.0),
                    ParentOffsetBounds::ParentBySize,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            )
            .finish()
    } else {
        scrollable
    }
}

/// Keeps a multi-file diff header pinned to the editor viewport while the
/// source rows beneath it participate in the aggregate horizontal scroll.
///
/// The scrollable publishes its real viewport before laying out its child, so
/// this wrapper follows the pane width on the first frame and every resize. At
/// paint time it cancels the parent's horizontal translation for the header
/// only; the source rows retain the shared horizontal scrollbar.
pub(crate) struct GitMultiViewportHeader {
    child: Box<dyn Element>,
    horizontal_scroll: ClippedScrollStateHandle,
    horizontal_inset: f32,
    laid_out_size: Option<Vector2F>,
}

impl GitMultiViewportHeader {
    pub(crate) fn new(
        child: Box<dyn Element>,
        horizontal_scroll: ClippedScrollStateHandle,
        horizontal_inset: f32,
    ) -> Self {
        Self {
            child,
            horizontal_scroll,
            horizontal_inset,
            laid_out_size: None,
        }
    }
}

impl Element for GitMultiViewportHeader {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let available_width = self.horizontal_scroll.viewport_width() - self.horizontal_inset;
        let width = if available_width.is_finite() && available_width > 0.0 {
            available_width.min(constraint.max.x())
        } else {
            constraint.max.x()
        }
        .max(1.0);
        let child_size = self.child.layout(
            SizeConstraint::new(
                vec2f(width, constraint.min.y()),
                vec2f(width, constraint.max.y()),
            ),
            ctx,
            app,
        );
        let size = vec2f(width, child_size.y());
        self.laid_out_size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let horizontal_offset = self.horizontal_scroll.scroll_start().as_f32().max(0.0);
        self.child
            .paint(origin + vec2f(horizontal_offset, 0.0), ctx, app);
    }

    fn size(&self) -> Option<Vector2F> {
        self.laid_out_size
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }
}

fn git_multi_sticky_file<'a>(
    diff: &'a crate::git_status::diff_head::GitMultiDiff,
    collapsed: &std::collections::HashSet<(PathBuf, crate::git_status::diff_head::GitDiffTarget)>,
    collapsed_hunks: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
        usize,
    )>,
    active_file_diff: Option<&crate::git_status::diff_head::GitFileDiff>,
    active_projected_rows: Option<usize>,
    comments: &ProjectSidebarView,
    scroll_top: f32,
) -> Option<&'a crate::git_status::diff_head::GitMultiDiffFile> {
    let mut top = crate::theme::scaled(60.0);
    let mut active = None;
    for file in &diff.files {
        if scroll_top < top {
            break;
        }
        active = Some(file);
        let body_height =
            if collapsed.contains(&(file.change.relative_path.clone(), file.change.target)) && comments.comment_for_file(&file.change.relative_path, file.change.target).is_none() {
                0.0
            } else {
                let is_active = active_file_diff.is_some_and(|active| {
                    active.relative_path == file.change.relative_path
                        && active.target == file.change.target
                        && comments.comment_for_file(&file.change.relative_path, file.change.target).is_none()
                });
                if let Some(comment) = comments.comment_for_file(&file.change.relative_path, file.change.target) {
                    file.diff.as_ref().map_or(crate::theme::scaled(34.0), |file| comment_preview_height(file, comment))
                } else {
                    git_multi_stable_body_height(file, collapsed_hunks, is_active.then_some(active_projected_rows).flatten())
                }
            };
        top += crate::theme::scaled(43.0) + body_height + comments.comment_for_file(&file.change.relative_path, file.change.target).map_or(0.0, |c| c.height());
    }
    active
}

/// Exact height of one preview diff row: 15px source text at the app's
/// line-height ratio plus the 1px top/bottom row padding. Must match
/// `render_git_multi_file_rows` exactly — the virtualization bookkeeping
/// (culling window, sticky offsets) treats this as truth, and any drift
/// makes files get culled while still on screen (blank where content is).
pub(crate) fn git_multi_preview_row_height() -> f32 {
    crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE) * ui_line_height_ratio()
        + crate::theme::scaled(1.0)
        + crate::theme::scaled(1.0)
}

pub(crate) fn git_multi_file_body_height(
    file: &crate::git_status::diff_head::GitMultiDiffFile,
    collapsed_hunks: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
        usize,
    )>,
) -> f32 {
    file.diff
        .as_ref()
        .map_or(crate::theme::scaled(34.0), |file| {
            let mut rendered_lines = 0usize;
            let mut height = 0.0;
            let mut hit_render_limit = false;
            for (hunk_index, hunk) in file.diff.hunks.iter().enumerate() {
                if rendered_lines >= 2_000 {
                    hit_render_limit = true;
                    break;
                }
                height += crate::theme::scaled(21.0);
                if collapsed_hunks.contains(&(file.relative_path.clone(), file.target, hunk_index))
                {
                    continue;
                }
                let remaining = 2_000 - rendered_lines;
                let shown = hunk.lines.len().min(remaining);
                rendered_lines += shown;
                height += shown as f32 * git_multi_preview_row_height();
                if shown < hunk.lines.len() {
                    hit_render_limit = true;
                    break;
                }
            }
            if hit_render_limit {
                height += crate::theme::scaled(34.0);
            }
            height
        })
}

fn git_multi_editor_height(
    file: &crate::git_status::diff_head::GitMultiDiffFile,
    projected_rows_override: Option<usize>,
) -> f32 {
    let projected_rows = projected_rows_override.unwrap_or_else(|| {
        file.diff
            .as_ref()
            .map(|file| {
                file.diff
                    .display_rows_with_expansions(
                        file.old_text.lines().count(),
                        file.new_text.lines().count(),
                        1,
                        &std::collections::HashMap::new(),
                    )
                    .len()
            })
            .unwrap_or(8)
    });
    git_multi_projected_editor_height(projected_rows)
}

pub(crate) fn git_multi_projected_editor_height(projected_rows: usize) -> f32 {
    // Keep the active editor and its outer multi-diff card on one geometry.
    // This prevents the old 2,000px editor viewport from clipping at roughly
    // 82 rows while the aggregate document continued reserving empty space.
    let line_height = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE * 1.618);
    // Aggregate diff scrolling supplies the surrounding context. Unlike a
    // standalone editor, this embedded editor must not add bottom overscroll:
    // it would appear as a blank region before the following file card.
    (projected_rows.max(1) as f32 * line_height).min(MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT)
}

/// Keep the active editor and the card immediately after it mounted while the
/// shared editor changes projections. Their heights can change between two
/// layout passes; replacing the following card with a same-frame placeholder
/// lets a short file be skipped before the next paint catches up.
pub(crate) fn git_multi_should_cull_file(
    file_index: usize,
    active_file_index: Option<usize>,
    outside_render_window: bool,
) -> bool {
    outside_render_window
        && !active_file_index
            .is_some_and(|active| file_index == active || file_index == active.saturating_add(1))
}

pub(crate) fn git_multi_stable_body_height(
    file: &crate::git_status::diff_head::GitMultiDiffFile,
    collapsed_hunks: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
        usize,
    )>,
    active_projected_rows: Option<usize>,
) -> f32 {
    let preview = git_multi_file_body_height(file, collapsed_hunks);
    match active_projected_rows {
        // Once active, the live editor is the body. Its exact projected height
        // replaces the preview height; taking max(preview, editor) can reserve
        // blank space whenever hunk chrome makes the preview taller.
        Some(_) => git_multi_editor_height(file, active_projected_rows),
        // Inactive cards paint exactly the preview rows. Reserving the full
        // editor projection height here leaves page after page of blank space
        // and pushes every later card's estimated offset far past its real
        // position, so following files get culled while still on screen.
        None => preview,
    }
}

fn render_git_multi_sticky_header(
    file: &crate::git_status::diff_head::GitMultiDiffFile,
    root: &Path,
    workspace_view_id: EntityId,
    sidebar_view_id: EntityId,
    collapsed: bool,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let path = file.change.relative_path.clone();
    let collapse_path = path.clone();
    let collapse_target = file.change.target;
    let file_identity = path.to_string_lossy().into_owned();
    let open_path = path.clone();
    let open_root = root.to_path_buf();
    let status = file.change.status;
    Hoverable::new(MouseStateHandle::default(), move |state| {
        let target_path = open_path.clone();
        let open = EventHandler::new(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(crate::theme::scaled(5.0))
                    .with_child(render_icon(
                        AppIcon::Eye,
                        theme.sub_text(),
                        crate::theme::scaled(11.0),
                    ))
                    .with_child(
                        Text::new("Open File".to_string(), ui, crate::theme::text_size(10.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(7.0))
            .with_padding_right(crate::theme::scaled(7.0))
            .with_padding_top(crate::theme::scaled(4.0))
            .with_padding_bottom(crate::theme::scaled(4.0))
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish(),
        )
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::OpenReadOnlyDocument {
                    root: open_root.clone(),
                    relative_path: target_path.clone(),
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish();
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(crate::theme::scaled(7.0))
                .with_child(render_icon(
                    if collapsed {
                        AppIcon::ChevronRight
                    } else {
                        AppIcon::ChevronDown
                    },
                    theme.sub_text(),
                    crate::theme::scaled(10.0),
                ))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(file_identity.clone(), mono, crate::theme::text_size(12.0))
                            .with_color(theme.main_text())
                            .with_style(Properties::default().weight(Weight::Medium))
                            .with_clip(ClipConfig::ellipsis())
                            .soft_wrap(false)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    Text::new(status.to_string(), mono, crate::theme::text_size(10.5))
                        .with_color(git_status_color(status, theme))
                        .finish(),
                )
                .with_child(open)
                .finish(),
        )
        .with_padding_left(crate::theme::scaled(22.0))
        .with_padding_right(crate::theme::scaled(20.0))
        .with_padding_top(crate::theme::scaled(7.0))
        .with_padding_bottom(crate::theme::scaled(7.0))
        .with_background_color(if state.is_hovered() {
            theme.surface_1()
        } else {
            theme.panel_background()
        })
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .with_defer_events_to_children()
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(
            sidebar_view_id,
            SidebarAction::ToggleGitMultiFile {
                path: collapse_path.clone(),
                target: collapse_target,
            },
        );
    })
    .finish()
}

fn render_git_multi_file_rows(
    file: &crate::git_status::diff_head::GitFileDiff,
    comments: &ProjectSidebarView,
    path: &Path,
    target: crate::git_status::diff_head::GitDiffTarget,
    allow_hunk_actions: bool,
    collapsed_hunks: &std::collections::HashSet<(
        PathBuf,
        crate::git_status::diff_head::GitDiffTarget,
        usize,
    )>,
    workspace_view_id: EntityId,
    sidebar_view_id: EntityId,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    horizontal_scroll: ClippedScrollStateHandle,
    visible_body_range: (f32, f32),
) -> Box<dyn Element> {
    use dragent_document::DiffLineKind;
    if file.binary {
        return Container::new(
            Text::new(
                "Binary file not shown".to_string(),
                mono,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .with_padding_left(crate::theme::scaled(12.0))
        .with_padding_top(crate::theme::scaled(10.0))
        .with_padding_bottom(crate::theme::scaled(10.0))
        .finish();
    }
    let comment = comments.comment_for_file(path, target);
    let hunks = comment_preview_hunks(file, comment);
    let line_limit = comment_preview_limit(&hunks, comment);
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    let mut activation_rows = Vec::new();
    let mut body_offset = 0.0_f32;
    let mut skipped_height = 0.0_f32;
    let row_height = git_multi_preview_row_height();

    let language = dragent_editor::Language::from_path(path);
    let mut rendered_lines = 0usize;
    let mut hit_render_limit = false;
    'hunks: for (hunk_index, hunk) in hunks {
        if rendered_lines >= line_limit {
            hit_render_limit = true;
            break;
        }
        let old_start = hunk
            .lines
            .iter()
            .find_map(|line| line.old_line)
            .unwrap_or(0);
        let new_start = hunk
            .lines
            .iter()
            .find_map(|line| line.new_line)
            .unwrap_or(0);
        let hunk_key = (path.to_path_buf(), target, hunk_index);
        let collapsed = comment.is_none() && collapsed_hunks.contains(&hunk_key);
        let header_height = crate::theme::scaled(21.0);
        if body_offset + header_height < visible_body_range.0
            || body_offset > visible_body_range.1
        {
            skipped_height += header_height;
        } else {
            if skipped_height > 0.0 {
                rows = rows.with_child(ConstrainedBox::new(Empty::new().finish())
                    .with_height(skipped_height).finish());
                skipped_height = 0.0;
            }
            let toggle_path = path.to_path_buf();
            let action_path = path.to_string_lossy().into_owned();
            let stage_action = WorkspaceAction::StageGitHunk {
                rel_path: action_path.clone(),
                hunk_index,
                old_start,
                new_start,
                staged: target == crate::git_status::diff_head::GitDiffTarget::Staged,
            };
            let primary = if allow_hunk_actions && hunk_index != usize::MAX {
                git_multi_hunk_action_button(
                    if target == crate::git_status::diff_head::GitDiffTarget::Staged {
                        AppIcon::Undo2
                    } else {
                        AppIcon::Plus
                    },
                    if target == crate::git_status::diff_head::GitDiffTarget::Staged {
                        "Unstage hunk"
                    } else {
                        "Stage hunk"
                    },
                    if target == crate::git_status::diff_head::GitDiffTarget::Staged {
                        theme.sub_text()
                    } else {
                        theme.ui_green()
                    },
                    stage_action,
                    workspace_view_id,
                    ui,
                    theme,
                )
            } else {
                Empty::new().finish()
            };
            let secondary = if allow_hunk_actions && hunk_index != usize::MAX
                && target == crate::git_status::diff_head::GitDiffTarget::Unstaged
            {
                git_multi_hunk_action_button(
                    AppIcon::Undo2,
                    "Discard hunk",
                    theme.ui_red(),
                    WorkspaceAction::RequestDiscardGitHunk {
                        rel_path: action_path,
                        hunk_index,
                        old_start,
                        new_start,
                    },
                    workspace_view_id,
                    ui,
                    theme,
                )
            } else {
                Empty::new().finish()
            };
            let header = Hoverable::new(MouseStateHandle::default(), move |state| {
                Container::new(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(crate::theme::scaled(5.0))
                        .with_child(render_icon(
                            if collapsed {
                                AppIcon::ChevronRight
                            } else {
                                AppIcon::ChevronDown
                            },
                            theme.accent,
                            crate::theme::scaled(9.0),
                        ))
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(
                                    format!("@@ -{old_start} +{new_start} @@"),
                                    mono,
                                    crate::theme::text_size(10.0),
                                )
                                .with_color(theme.accent)
                                .finish(),
                            )
                            .finish(),
                        )
                        .with_child(primary)
                        .with_child(secondary)
                        .finish(),
                )
                .with_padding_left(crate::theme::scaled(8.0))
                .with_padding_top(crate::theme::scaled(4.0))
                .with_padding_bottom(crate::theme::scaled(4.0))
                .with_background_color(if state.is_hovered() {
                    with_alpha(theme.accent, 0.14)
                } else {
                    with_alpha(theme.accent, 0.08)
                })
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .with_defer_events_to_children()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(
                    sidebar_view_id,
                    SidebarAction::ToggleGitMultiHunk {
                        path: toggle_path.clone(),
                        target,
                        hunk: hunk_index,
                    },
                );
            })
            .finish();
            rows = rows.with_child(
                GitMultiViewportHeader::new(
                    ConstrainedBox::new(header)
                        .with_height(crate::theme::scaled(21.0))
                        .finish(),
                    horizontal_scroll.clone(),
                    crate::theme::scaled(24.0),
                )
                .finish(),
            );
        }
        body_offset += crate::theme::scaled(21.0);
        if collapsed {
            continue;
        }
        for line in &hunk.lines {
            if rendered_lines >= line_limit {
                hit_render_limit = true;
                break 'hunks;
            }
            rendered_lines += 1;
            let anchor_row = comment.is_some_and(|c| c.anchor.end == line.new_line.or(line.old_line).unwrap_or(1)
                && c.anchor.end_deleted == (line.kind == DiffLineKind::Removed));
            let annotation_height = if anchor_row { comment.map_or(0.0, |c| c.height()) } else { 0.0 };
            if body_offset + row_height + annotation_height < visible_body_range.0 || body_offset > visible_body_range.1 {
                skipped_height += row_height + annotation_height;
                body_offset += row_height + annotation_height;
                continue;
            }
            if skipped_height > 0.0 {
                rows = rows.with_child(ConstrainedBox::new(Empty::new().finish())
                    .with_height(skipped_height).finish());
                skipped_height = 0.0;
            }

            let (marker, foreground, background) = match line.kind {
                DiffLineKind::Added => ("+", theme.ui_green(), with_alpha(theme.ui_green(), 0.10)),
                DiffLineKind::Removed => ("−", theme.ui_red(), with_alpha(theme.ui_red(), 0.10)),
                DiffLineKind::Context => (" ", theme.main_text(), theme.background),
            };
            let syntax_spans = match line.kind {
                DiffLineKind::Removed => line.old_line.zip(file.old_syntax.as_ref()),
                DiffLineKind::Context | DiffLineKind::Added => {
                    line.new_line.zip(file.new_syntax.as_ref())
                }
            }
            .and_then(|(number, syntax)| syntax.lines.get(number.saturating_sub(1)));
            let mut comment_state = match line.kind {
                DiffLineKind::Removed => line
                    .old_line
                    .and_then(|number| file.old_highlight_states.get(number.saturating_sub(1))),
                DiffLineKind::Context | DiffLineKind::Added => line
                    .new_line
                    .and_then(|number| file.new_highlight_states.get(number.saturating_sub(1))),
            }
            .copied()
            .unwrap_or_default();
            let show_whitespace = crate::console::editor_settings::EditorSettings::global()
                .editor
                .show_whitespace;
            let font_size = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE);
            // Lay out only a bounded prefix of very long lines: shaping a
            // whole minified line blocks the UI thread for seconds. Spans are
            // byte ranges of the full line; ones past the cut are skipped.
            let (shown_text, hidden_chars) = crate::workspace::editor_render::display_prefix(
                &line.text,
                crate::workspace::editor_render::DIFF_ROW_MAX_CHARS,
            );
            let code = match syntax_spans {
                Some(spans) => crate::workspace::editor_render::highlighted_source_from_spans(
                    shown_text,
                    spans,
                    theme,
                    mono,
                    font_size,
                    Weight::Normal,
                    show_whitespace,
                ),
                None => crate::workspace::editor_render::highlighted_source_with_state(
                    shown_text,
                    language,
                    &mut comment_state,
                    theme,
                    mono,
                    font_size,
                    Weight::Normal,
                    show_whitespace,
                ),
            }
            .soft_wrap(false)
            .finish();
            let comment_line = line.new_line.or(line.old_line).unwrap_or(1);
            let deleted = line.kind == DiffLineKind::Removed;
            let row = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(crate::theme::scaled(7.0))
                    .with_child(comments.diff_comment_gutter(path, target, comment_line, deleted))
                    .with_child(
                        ConstrainedBox::new(
                            Text::new(
                                line.old_line
                                    .map(|line| line.to_string())
                                    .unwrap_or_default(),
                                mono,
                                crate::theme::text_size(9.5),
                            )
                            .with_color(theme.disabled_text())
                            .finish(),
                        )
                        .with_width(crate::theme::scaled(32.0))
                        .finish(),
                    )
                    .with_child(
                        ConstrainedBox::new(
                            Text::new(
                                line.new_line
                                    .map(|line| line.to_string())
                                    .unwrap_or_default(),
                                mono,
                                crate::theme::text_size(9.5),
                            )
                            .with_color(theme.disabled_text())
                            .finish(),
                        )
                        .with_width(crate::theme::scaled(32.0))
                        .finish(),
                    )
                    .with_child(
                        Text::new(
                            marker.to_string(),
                            mono,
                            crate::theme::text_size(crate::workspace::SOURCE_EDITOR_FONT_SIZE),
                        )
                        .with_color(foreground)
                        .finish(),
                    )
                    .with_child(
                        Expanded::new(
                            1.0,
                            if hidden_chars == 0 {
                                code
                            } else {
                                Flex::row()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_spacing(crate::theme::scaled(8.0))
                                    .with_child(code)
                                    .with_child(
                                        Text::new(
                                            format!(
                                                "… {} more characters (open the file to see the full line)",
                                                thousands(hidden_chars)
                                            ),
                                            ui,
                                            crate::theme::text_size(10.5),
                                        )
                                        .with_color(theme.disabled_text())
                                        .soft_wrap(false)
                                        .finish(),
                                    )
                                    .finish()
                            },
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(8.0))
            .with_padding_right(crate::theme::scaled(10.0))
            .with_padding_top(crate::theme::scaled(1.0))
            .with_padding_bottom(crate::theme::scaled(1.0))
            .with_background_color(background)
            .finish();
            // Removed rows have no current-buffer line. Place their caret at
            // the hunk's current insertion point instead.
            activation_rows.push((
                body_offset,
                body_offset + row_height,
                line.new_line.unwrap_or(new_start.max(1)),
            ));
            body_offset += row_height;
            rows = rows.with_child(comments.diff_comment_row(row, path, target, comment_line, deleted));
            if let Some(composer) = comments.render_comment_after(path, target, comment_line, deleted) {
                body_offset += comments.comment_for_file(path, target).map_or(0.0, |c| c.height());
                rows = rows.with_child(composer);
            }
        }
    }
    if skipped_height > 0.0 {
        rows = rows.with_child(ConstrainedBox::new(Empty::new().finish())
            .with_height(skipped_height).finish());
    }
    if hit_render_limit {
        rows = rows.with_child(
            Container::new(
                Text::new(
                    format!(
                        "Showing the first {line_limit} changed/context lines. Open the file to review the rest."
                    ),
                    mono,
                    crate::theme::text_size(10.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_padding_left(crate::theme::scaled(12.0))
            .with_padding_top(crate::theme::scaled(8.0))
            .with_padding_bottom(crate::theme::scaled(8.0))
            .with_background_color(theme.panel_background())
            .finish(),
        );
    }
    let body = Container::new(rows.finish())
        .with_border(Border::left(1.0).with_border_color(theme.border()))
        .finish();
    if !allow_hunk_actions || comments.comment_for_file(path, target).is_some() {
        return body;
    }
    // One stable hit surface maps pointer Y to source lines. This avoids a
    // responder per preview row, which can leave stale offscreen responders
    // behind when the body is replaced by the live editor.
    let position_id = format!("git_multi_preview:{}:{target:?}", path.to_string_lossy());
    let hit_position_id = position_id.clone();
    let activate_path = path.to_path_buf();
    EventHandler::new(SavePosition::new(body, &position_id).finish())
        .on_left_mouse_down(move |ctx, _, position| {
            let Some(bounds) = ctx.element_position_by_id(&hit_position_id) else {
                return DispatchEventResult::PropagateToParent;
            };
            if position.x() - bounds.origin().x() < crate::theme::scaled(30.0) {
                return DispatchEventResult::PropagateToParent;
            }
            let local_y = position.y() - bounds.origin().y();
            let Some((_, _, line)) = activation_rows
                .iter()
                .find(|(top, bottom, _)| local_y >= *top && local_y < *bottom)
            else {
                return DispatchEventResult::PropagateToParent;
            };
            ctx.dispatch_typed_action_for(
                sidebar_view_id,
                SidebarAction::ActivateGitMultiFile {
                    path: activate_path.clone(),
                    line: *line,
                    target,
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// `1234567` → `"1,234,567"`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod long_line_tests {
    use super::thousands;
    use crate::workspace::editor_render::{display_prefix, DIFF_ROW_MAX_CHARS};

    #[test]
    fn long_lines_are_cut_on_char_boundaries() {
        assert_eq!(display_prefix("short", DIFF_ROW_MAX_CHARS), ("short", 0));
        let exact = "x".repeat(DIFF_ROW_MAX_CHARS);
        assert_eq!(display_prefix(&exact, DIFF_ROW_MAX_CHARS), (exact.as_str(), 0));
        // Multi-byte characters: the cut counts characters, never splits one.
        let long = "é".repeat(DIFF_ROW_MAX_CHARS + 5);
        let (shown, hidden) = display_prefix(&long, DIFF_ROW_MAX_CHARS);
        assert_eq!(shown.chars().count(), DIFF_ROW_MAX_CHARS);
        assert_eq!(hidden, 5);
        let minified = "a".repeat(458_345);
        let (shown, hidden) = display_prefix(&minified, DIFF_ROW_MAX_CHARS);
        assert_eq!((shown.len(), hidden), (DIFF_ROW_MAX_CHARS, 458_345 - DIFF_ROW_MAX_CHARS));
    }

    #[test]
    fn hidden_counts_are_readable() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(456_345), "456,345");
        assert_eq!(thousands(12_345_678), "12,345,678");
    }
}

fn comment_preview_hunks<'a>(file: &'a crate::git_status::diff_head::GitFileDiff,
    comment: Option<&'a super::sidebar_diff_comments::DiffComment>) -> Vec<(usize, &'a dragent_document::DiffHunk)> {
    let mut hunks: Vec<_> = file.diff.hunks.iter().enumerate().collect();
    if let Some(context) = comment.and_then(|c| c.context_hunk.as_ref()) {
        let start = context.lines.first().and_then(|line| line.new_line).unwrap_or(0);
        let index = hunks.iter().position(|(_, hunk)| hunk.lines.iter().find_map(|line| line.new_line).unwrap_or(usize::MAX) > start).unwrap_or(hunks.len());
        hunks.insert(index, (usize::MAX, context));
    }
    hunks
}

fn comment_preview_limit(hunks: &[(usize, &dragent_document::DiffHunk)], comment: Option<&super::sidebar_diff_comments::DiffComment>) -> usize {
    let Some(comment) = comment else { return 2000; };
    hunks.iter().flat_map(|(_, hunk)| &hunk.lines).position(|line| {
        line.new_line.or(line.old_line) == Some(comment.anchor.end)
            && (line.kind == dragent_document::DiffLineKind::Removed) == comment.anchor.end_deleted
    }).map_or(2000, |index| 2000.max(index + 1))
}

fn comment_preview_height(file: &crate::git_status::diff_head::GitFileDiff, comment: &super::sidebar_diff_comments::DiffComment) -> f32 {
    let hunks = comment_preview_hunks(file, Some(comment));
    let limit = comment_preview_limit(&hunks, Some(comment));
    let mut count = 0;
    let mut height = 0.0;
    for (_, hunk) in hunks {
        if count >= limit { height += crate::theme::scaled(34.0); break; }
        height += crate::theme::scaled(21.0);
        let shown = hunk.lines.len().min(limit - count);
        count += shown;
        height += shown as f32 * git_multi_preview_row_height();
        if shown < hunk.lines.len() { height += crate::theme::scaled(34.0); break; }
    }
    height
}
