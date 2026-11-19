//! Sidebar project search renders.
//!
//! Moved verbatim; same behavior.

use super::detect;
use super::detect::QuickAction;
use super::model::{ErrorLevel, ProjectInfo, ProjectSidebarModel, SidebarEvent};
use super::sidebar_explorer::project_search_hit_range;
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

use super::sidebar_explorer::{
    buffer_search_tooltip, global_search_icon_button, project_search_sidebar_sticky_file,
    render_sidebar_search_file_header,
};
use super::sidebar_explorer::{
    render_documents_section, render_explorer_context_menu, render_search_file_identity,
};
use super::sidebar_gitdiff_file::render_git_diff_source;
use super::sidebar_gitdiff_multi::GitMultiViewportHeader;
use super::view::{GitHistoryState, GitPanelMode, ProjectSidebarView};
use super::view::{SidebarAction, MULTI_FILE_SEARCH_OVERSCAN_ROWS};

pub(crate) fn project_search_body_height(
    file: &crate::workspace::search_panel::SearchFileResult,
    collapsed: bool,
) -> f32 {
    if collapsed {
        0.0
    } else {
        file.hits.len() as f32 * crate::theme::scaled(24.0)
    }
}

pub(crate) fn project_search_sticky_file<'a>(
    results: &'a crate::workspace::search_panel::SearchResults,
    collapsed: &std::collections::HashSet<PathBuf>,
    scroll_top: f32,
) -> Option<&'a crate::workspace::search_panel::SearchFileResult> {
    let mut top = crate::theme::scaled(64.0);
    let mut active = None;
    for file in &results.files {
        if scroll_top < top {
            break;
        }
        active = Some(file);
        top += crate::theme::scaled(43.0)
            + project_search_body_height(file, collapsed.contains(&file.relative_path));
    }
    active
}

pub(crate) fn render_project_search_source(
    line: &str,
    match_range: Option<(usize, usize)>,
    language: dragent_editor::Language,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let text_size = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE);
    let valid_range = match_range.and_then(|(start, end)| {
        (start < end
            && end <= line.len()
            && line.is_char_boundary(start)
            && line.is_char_boundary(end))
        .then_some(start..end)
    });
    render_git_diff_source(
        line,
        valid_range.as_ref(),
        theme.accent,
        language,
        mono,
        text_size,
        theme,
    )
}

pub(crate) fn render_project_search_file_header(
    file: &crate::workspace::search_panel::SearchFileResult,
    root: &Path,
    collapsed: bool,
    sticky: bool,
    workspace_view_id: EntityId,
    sidebar_view_id: EntityId,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let path = file.relative_path.clone();
    let collapse_path = path.clone();
    let open_path = path.clone();
    let open_root = root.to_path_buf();
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned());
    let hit_count = file.hits.len();
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
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_background_color(theme.surface_2())
        .finish(),
    )
    .on_left_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenReadOnlyDocument {
                root: open_root.clone(),
                relative_path: open_path.clone(),
            },
        );
        DispatchEventResult::StopPropagation
    })
    .finish();

    let identity =
        render_search_file_identity(filename, parent, mono, crate::theme::text_size(12.0), theme);
    let header = Container::new(
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
            .with_child(identity)
            .with_child(
                Container::new(
                    Text::new(hit_count.to_string(), ui, crate::theme::text_size(10.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_padding_left(crate::theme::scaled(6.0))
                .with_padding_right(crate::theme::scaled(6.0))
                .with_padding_top(crate::theme::scaled(1.0))
                .with_padding_bottom(crate::theme::scaled(1.0))
                .with_background_color(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
                .finish(),
            )
            .with_child(open)
            .finish(),
    )
    .with_padding_left(if sticky {
        crate::theme::scaled(22.0)
    } else {
        crate::theme::scaled(10.0)
    })
    .with_padding_right(if sticky {
        crate::theme::scaled(20.0)
    } else {
        crate::theme::scaled(8.0)
    })
    .with_padding_top(crate::theme::scaled(7.0))
    .with_padding_bottom(crate::theme::scaled(7.0))
    .with_background_color(theme.panel_background())
    .with_border(if sticky {
        Border::bottom(1.0).with_border_color(theme.border())
    } else {
        Border::all(1.0).with_border_color(theme.border())
    });
    let header = header.finish();
    EventHandler::new(header)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(
                sidebar_view_id,
                SidebarAction::ToggleGlobalSearchFileCollapse {
                    path: collapse_path.clone(),
                },
            );
            DispatchEventResult::StopPropagation
        })
        .finish()
}

pub(crate) fn render_project_search_multi(
    results: &crate::workspace::search_panel::SearchResults,
    root: &Path,
    scroll: ClippedScrollStateHandle,
    horizontal_scroll: ClippedScrollStateHandle,
    workspace_view_id: EntityId,
    sidebar_view_id: EntityId,
    collapsed: &std::collections::HashSet<PathBuf>,
    active_match: Option<&(PathBuf, usize)>,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    if results.files.is_empty() {
        return Align::new(
            Text::new(
                format!("No results for “{}”", results.query),
                ui,
                crate::theme::text_size(13.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .finish();
    }
    let sticky_file =
        project_search_sticky_file(results, collapsed, scroll.scroll_start().as_f32().max(0.0));
    let aggregate_content_width = (crate::theme::scaled(112.0)
        + results.max_line_chars as f32
            * crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE)
            * 0.62)
        .max(horizontal_scroll.viewport_width())
        .max(crate::theme::scaled(900.0));
    let scroll_top = scroll.scroll_start().as_f32().max(0.0);
    let viewport_height = if scroll.viewport_height().is_finite() && scroll.viewport_height() > 0.0
    {
        scroll.viewport_height()
    } else {
        crate::theme::scaled(900.0)
    };
    let overscan = crate::theme::scaled(180.0);
    let render_start = (scroll_top - overscan).max(0.0);
    let render_end = scroll_top + viewport_height + overscan;
    let mut file_top = crate::theme::scaled(64.0);
    let subtitle = if results.truncated {
        format!(
            "{} matches in {} files · limited",
            results.total_hits,
            results.files.len()
        )
    } else {
        format!(
            "{} matches in {} files",
            results.total_hits,
            results.files.len()
        )
    };
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
                        Text::new(
                            format!("Search results for “{}”", results.query),
                            ui,
                            crate::theme::text_size(15.0),
                        )
                        .with_color(theme.main_text())
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .finish(),
                    )
                    .with_child(
                        Text::new(subtitle, ui, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text())
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

    for file in &results.files {
        let file_card_top = file_top;
        let is_collapsed = collapsed.contains(&file.relative_path);
        let body_height = project_search_body_height(file, is_collapsed);
        let card_height = crate::theme::scaled(43.0) + body_height;
        let file_bottom = file_top + card_height;
        if file_bottom < render_start || file_top > render_end {
            content = content.with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(card_height)
                    .finish(),
            );
            file_top = file_bottom;
            continue;
        }
        file_top = file_bottom;
        let header = render_project_search_file_header(
            file,
            root,
            is_collapsed,
            false,
            workspace_view_id,
            sidebar_view_id,
            ui,
            mono,
            theme,
        );
        let header = GitMultiViewportHeader::new(
            ConstrainedBox::new(header)
                .with_height(crate::theme::scaled(34.0))
                .finish(),
            horizontal_scroll.clone(),
            crate::theme::scaled(24.0),
        )
        .finish();
        let mut rows = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);
        if !is_collapsed {
            let row_height = crate::theme::scaled(24.0);
            let body_top = file_card_top + crate::theme::scaled(43.0);
            let visible_rows = editor_render::viewport_row_range(
                file.hits.len(),
                scroll_top - body_top,
                viewport_height,
                row_height,
                MULTI_FILE_SEARCH_OVERSCAN_ROWS,
            );
            if visible_rows.start > 0 {
                rows = rows.with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_height(visible_rows.start as f32 * row_height)
                        .finish(),
                );
            }
            let language = dragent_editor::Language::from_path(&file.relative_path);
            for hit_index in visible_rows.clone() {
                let hit = &file.hits[hit_index];
                let path = file.relative_path.clone();
                let open_path = path.clone();
                let open_root = root.to_path_buf();
                let select_path = path.clone();
                let open_range = project_search_hit_range(hit);
                let is_active =
                    active_match.is_some_and(|active| active.0 == path && active.1 == hit_index);
                let source = render_project_search_source(
                    &hit.line_text,
                    hit.match_range,
                    language,
                    mono,
                    theme,
                );
                let row = Hoverable::new(MouseStateHandle::default(), move |state| {
                    Container::new(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(
                                ConstrainedBox::new(
                                    Align::new(
                                        Text::new(
                                            hit.line.to_string(),
                                            mono,
                                            crate::theme::text_size(10.5),
                                        )
                                        .with_color(theme.disabled_text())
                                        .finish(),
                                    )
                                    .top_right()
                                    .finish(),
                                )
                                .with_width(crate::theme::scaled(52.0))
                                .finish(),
                            )
                            .with_child(
                                Container::new(source)
                                    .with_padding_left(crate::theme::scaled(12.0))
                                    .with_padding_right(crate::theme::scaled(20.0))
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_background_color(if is_active {
                        theme.accent_overlay()
                    } else if state.is_hovered() {
                        theme.surface_1()
                    } else {
                        theme.background
                    })
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_mouse_down(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(
                        sidebar_view_id,
                        SidebarAction::SelectGlobalSearchMatch {
                            path: select_path.clone(),
                            hit: hit_index,
                        },
                    );
                    ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::OpenLanguageLocation {
                            root: open_root.clone(),
                            relative_path: open_path.clone(),
                            range: open_range,
                        },
                    );
                })
                .finish();
                rows = rows.with_child(ConstrainedBox::new(row).with_height(row_height).finish());
            }
            if visible_rows.end < file.hits.len() {
                rows = rows.with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_height((file.hits.len() - visible_rows.end) as f32 * row_height)
                        .finish(),
                );
            }
        }
        let card = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(header)
            .with_child(
                ConstrainedBox::new(rows.finish())
                    .with_height(body_height)
                    .finish(),
            )
            .finish();
        content = content.with_child(
            Container::new(card)
                .with_padding_left(crate::theme::scaled(12.0))
                .with_padding_right(crate::theme::scaled(12.0))
                .with_padding_top(crate::theme::scaled(9.0))
                .finish(),
        );
    }
    content = content.with_child(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(crate::theme::scaled(120.0))
            .finish(),
    );
    let content = ConstrainedBox::new(content.finish())
        .with_width(aggregate_content_width)
        .finish();
    let scrollable = warpui::elements::NewScrollable::horizontal_and_vertical(
        DualAxisConfig::Clipped {
            horizontal: ClippedAxisConfiguration {
                handle: horizontal_scroll,
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
        let sticky = render_project_search_file_header(
            file,
            root,
            collapsed.contains(&file.relative_path),
            true,
            workspace_view_id,
            sidebar_view_id,
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
impl ProjectSidebarView {
    pub(crate) fn schedule_global_search(&mut self, ctx: &mut ViewContext<Self>) {
        self.global_search_epoch = self.global_search_epoch.wrapping_add(1);
        let epoch = self.global_search_epoch;
        self.global_search_generation
            .store(epoch, Ordering::Release);
        let query = self
            .global_search_query_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        if query.trim().is_empty() {
            self.global_search_results = None;
            self.global_search_results_root = None;
            self.global_search_in_progress = false;
            self.global_search_error = None;
            self.global_search_active = None;
            self.global_search_scroll.scroll_to(Pixels::zero());
            let editor_open = self
                .model
                .read(ctx, |model, _| model.global_search_results.is_some());
            if editor_open {
                ctx.update_model(&self.model, |model, model_ctx| {
                    let root = model.workspace_root().to_path_buf();
                    model.set_global_search_results(
                        root,
                        PathBuf::from(".search/Search Results"),
                        Arc::new(crate::workspace::search_panel::SearchResults::empty(
                            String::new(),
                        )),
                        model_ctx,
                    );
                });
            }
            ctx.notify();
            return;
        }
        let include = self
            .global_search_include_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let exclude = self
            .global_search_exclude_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let options = self.global_search_options;
        let root = self.model.read(ctx, |model, _| {
            model
                .projects
                .first()
                .map(|project| project.git_root.clone())
                .or_else(|| model.active_cwd.clone())
        });
        let Some(root) = root else {
            self.global_search_in_progress = false;
            self.global_search_error = Some("Open a project folder to search".to_string());
            ctx.notify();
            return;
        };

        self.global_search_in_progress = true;
        self.global_search_replacing = false;
        self.global_search_error = None;
        ctx.notify();
        let query_for_error = query.clone();
        let results_root = root.clone();
        let generation = self.global_search_generation.clone();
        ctx.spawn(
            async move {
                Timer::after(std::time::Duration::from_millis(140)).await;
                if generation.load(Ordering::Acquire) != epoch {
                    return None;
                }
                Some(
                    tokio::task::spawn_blocking(move || {
                        crate::workspace::search_panel::run_search(
                            &root, &query, &include, &exclude, options,
                        )
                    })
                    .await
                    .unwrap_or_else(|error| {
                        crate::workspace::search_panel::SearchResults {
                            query: query_for_error,
                            files: Vec::new(),
                            total_hits: 0,
                            max_line_chars: 0,
                            truncated: false,
                            error: Some(format!("Search failed: {error}")),
                        }
                    }),
                )
            },
            move |view, results, ctx| {
                if view.global_search_epoch != epoch {
                    return;
                }
                let Some(results) = results else {
                    return;
                };
                let results = Arc::new(results);
                view.global_search_error = results.error.clone();
                view.global_search_results = Some(results.clone());
                view.global_search_results_root = Some(results_root.clone());
                view.global_search_in_progress = false;
                view.global_search_active = None;
                view.global_search_collapsed.clear();
                view.global_search_scroll.scroll_to(Pixels::zero());
                let editor_open = view
                    .model
                    .read(ctx, |model, _| model.global_search_results.is_some());
                if editor_open {
                    ctx.update_model(&view.model, |model, model_ctx| {
                        model.set_global_search_results(
                            results_root.clone(),
                            PathBuf::from(".search/Search Results"),
                            results,
                            model_ctx,
                        );
                    });
                    view.global_search_editor_scroll.scroll_to(Pixels::zero());
                    view.global_search_editor_horizontal_scroll
                        .scroll_to(Pixels::zero());
                }
                ctx.notify();
            },
        );
    }

    /// Invalidate folder-scoped sidebar state when the selected workspace tab
    /// changes. Keep the user's query and filters, then run them against the
    /// new folder so the right sidebar never presents results from the tab we
    /// just left.
    pub fn refresh_workspace_context(
        &mut self,
        root: Option<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.global_search_context_root == root {
            return;
        }

        self.global_search_context_root = root;
        self.global_search_epoch = self.global_search_epoch.wrapping_add(1);
        self.global_search_generation
            .store(self.global_search_epoch, Ordering::Release);
        self.global_search_results = None;
        self.global_search_results_root = None;
        self.global_search_in_progress = false;
        self.global_search_replacing = false;
        self.global_search_error = None;
        self.global_search_active = None;
        self.global_search_collapsed.clear();
        self.global_search_scroll.scroll_to(Pixels::zero());
        self.global_search_editor_scroll.scroll_to(Pixels::zero());
        self.global_search_editor_horizontal_scroll
            .scroll_to(Pixels::zero());
        self.git_history = GitHistoryState::Loading;
        self.git_history_scroll.scroll_to(Pixels::zero());

        let has_query = self
            .global_search_query_editor
            .read(ctx, |editor, _| !editor.get_text().trim().is_empty());
        if has_query {
            self.schedule_global_search(ctx);
        }
        if self.git_panel_mode == GitPanelMode::History {
            self.load_git_history(ctx);
        } else {
            ctx.notify();
        }
    }

    pub fn global_search_snapshot(
        &self,
    ) -> Option<(PathBuf, Arc<crate::workspace::search_panel::SearchResults>)> {
        Some((
            self.global_search_results_root.clone()?,
            self.global_search_results.clone()?,
        ))
    }

    pub fn prepare_global_search_editor(&mut self) {
        self.global_search_editor_scroll.scroll_to(Pixels::zero());
        self.global_search_editor_horizontal_scroll
            .scroll_to(Pixels::zero());
    }

    pub(crate) fn activate_global_search_match(
        &mut self,
        path: PathBuf,
        hit_index: usize,
        open: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(hit) = self.global_search_results.as_ref().and_then(|results| {
            results
                .files
                .iter()
                .find(|file| file.relative_path == path)
                .and_then(|file| file.hits.get(hit_index))
                .cloned()
        }) else {
            return;
        };
        self.global_search_collapsed.remove(&path);
        self.global_search_active = Some((path.clone(), hit_index));

        // Only the result tree scrolls. Keep the selected line below the
        // pinned file header with a small amount of surrounding context.
        let mut offset = 0.0;
        if let Some(results) = self.global_search_results.as_ref() {
            for file in &results.files {
                offset += 28.0;
                if file.relative_path == path {
                    offset += hit_index as f32 * 24.0;
                    break;
                }
                if !self.global_search_collapsed.contains(&file.relative_path) {
                    offset += file.hits.len() as f32 * 24.0;
                }
            }
        }
        self.global_search_scroll
            .scroll_to(Pixels::new((offset - 72.0).max(0.0)));

        if open {
            if let Some(root) = self.global_search_results_root.clone() {
                ctx.dispatch_typed_action_deferred(WorkspaceAction::OpenLanguageLocation {
                    root,
                    relative_path: path,
                    range: project_search_hit_range(&hit),
                });
            }
        }
        ctx.notify();
    }

    pub(crate) fn navigate_global_search_match(
        &mut self,
        previous: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(results) = self.global_search_results.as_ref() else {
            return;
        };
        let matches = results
            .files
            .iter()
            .flat_map(|file| {
                file.hits
                    .iter()
                    .enumerate()
                    .map(|(index, _)| (file.relative_path.clone(), index))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return;
        }
        let current = self
            .global_search_active
            .as_ref()
            .and_then(|active| matches.iter().position(|candidate| candidate == active));
        let next = if previous {
            current
                .and_then(|index| index.checked_sub(1))
                .unwrap_or(matches.len() - 1)
        } else {
            current.map_or(0, |index| (index + 1) % matches.len())
        };
        let (path, hit) = matches[next].clone();
        self.activate_global_search_match(path, hit, true, ctx);
    }

    pub(crate) fn replace_all_global_search(&mut self, ctx: &mut ViewContext<Self>) {
        if self.global_search_in_progress {
            return;
        }
        let query = self
            .global_search_query_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        if query.is_empty() {
            return;
        }
        let replacement = self
            .global_search_replace_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let include = self
            .global_search_include_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let exclude = self
            .global_search_exclude_editor
            .read(ctx, |editor, _| editor.get_text().to_string());
        let Some(root) = self.model.read(ctx, |model, _| {
            model
                .projects
                .first()
                .map(|project| project.git_root.clone())
                .or_else(|| model.active_cwd.clone())
        }) else {
            self.global_search_error = Some("Open a project folder to replace text".to_string());
            ctx.notify();
            return;
        };
        let options = self.global_search_options;
        let active_clean_path = self.model.read(ctx, |model, _| {
            model
                .buffer
                .as_ref()
                .filter(|buffer| !buffer.is_dirty())
                .map(|buffer| buffer.document().relative_path.clone())
                .filter(|path| {
                    self.global_search_results.as_ref().is_some_and(|results| {
                        results.files.iter().any(|file| file.relative_path == *path)
                    })
                })
        });
        self.global_search_in_progress = true;
        self.global_search_replacing = true;
        self.global_search_error = None;
        ctx.notify();
        ctx.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    let summary = crate::workspace::search_panel::replace_search_results(
                        &root,
                        &query,
                        &replacement,
                        &include,
                        &exclude,
                        options,
                        None,
                    )?;
                    // Re-read changed files on the worker so applying Replace
                    // All never performs file I/O in the WarpUI update path.
                    let preview = active_clean_path
                        .as_ref()
                        .map(|path| crate::workspace::project_documents::read_preview(&root, path));
                    Ok::<_, String>((summary, preview))
                })
                .await
                .unwrap_or_else(|error| Err(format!("Replace worker failed: {error}")))
            },
            |view, result, ctx| {
                view.global_search_in_progress = false;
                view.global_search_replacing = false;
                match result {
                    Ok((_summary, preview)) => {
                        if let Some(preview) = preview {
                            let text = preview.text.clone();
                            let reloaded = ctx.update_model(&view.model, |model, model_ctx| {
                                model.reload_clean_document(&preview, model_ctx)
                            });
                            if reloaded {
                                view.reload_saved_document_text(&text, ctx);
                            }
                        }
                        view.global_search_active = None;
                        view.schedule_global_search(ctx);
                    }
                    Err(error) => {
                        view.global_search_error = Some(error);
                        ctx.notify();
                    }
                }
            },
        );
    }
}
