//! Sidebar search panel rendering.
//!
//! The global search results panel. Moved verbatim; same behavior.

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

use super::sidebar_explorer::{
    buffer_search_tooltip, global_search_icon_button, project_search_hit_range,
    project_search_sidebar_sticky_file, render_sidebar_search_file_header,
};
use super::view::{ProjectSidebarView, SidebarAction, MULTI_FILE_SEARCH_OVERSCAN_ROWS};

impl ProjectSidebarView {
    pub(crate) fn render_search_panel(
        &self,
        ui: FamilyId,
        mono: FamilyId,
        theme: &Theme,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        // Search results can contain tens of thousands of owned line strings.
        // Rendering only needs to borrow the immutable snapshot; cloning it on
        // every scroll/update would otherwise dwarf the work saved by row
        // virtualization.
        let results = self.global_search_results.as_ref();
        let in_progress = self.global_search_in_progress;
        let replacing = self.global_search_replacing;
        let error = self.global_search_error.clone();
        let collapsed = self.global_search_collapsed.clone();
        let options = self.global_search_options;
        let active_match = self.global_search_active.clone();
        let search_root = self
            .global_search_results_root
            .clone()
            .or_else(|| self.global_search_context_root.clone());
        let view_id = self.view_id;
        let workspace_view_id = self.workspace_view_id;

        // ── Helper: toggle button (Aa, ab, .* ) ────────────────────────
        let make_toggle = |key: String, label: &str, tooltip: &str, active: bool| {
            let mouse = MouseStateHandle::default();
            let label_owned = label.to_string();
            let tooltip_owned = tooltip.to_string();
            let view_id_copy = view_id;
            let button = Hoverable::new(mouse, move |state| {
                let bg = if active {
                    theme.accent_overlay()
                } else if state.is_hovered() {
                    theme.surface_1()
                } else {
                    ColorU::transparent_black()
                };
                let button = Container::new(
                    Text::new(label_owned.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(if active {
                            theme.accent
                        } else {
                            theme.sub_text()
                        })
                        .with_style(Properties::default().weight(if active {
                            Weight::Semibold
                        } else {
                            Weight::Normal
                        }))
                        .finish(),
                )
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .with_background_color(bg)
                .with_border(Border::all(1.0).with_border_color(if active {
                    theme.accent
                } else {
                    theme.border()
                }))
                .finish();
                buffer_search_tooltip(button, state.is_hovered(), &tooltip_owned, theme, ui)
            })
            .with_cursor(Cursor::PointingHand)
            .finish();
            EventHandler::new(button)
                .on_left_mouse_down(move |ctx, _app, _pos| {
                    ctx.dispatch_typed_action_for(
                        view_id_copy,
                        SidebarAction::ToggleGlobalSearchOption {
                            option: key.to_string(),
                        },
                    );
                    DispatchEventResult::StopPropagation
                })
                .finish()
        };

        let mut header_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(0.0);

        // ════════════════════════════════════════════════════════════════
        // SEARCH WIDGET (VS Code: div.search-widgets-container)
        // ════════════════════════════════════════════════════════════════

        // Search query and matching semantics are always visible.
        let search_input_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(
                Expanded::new(
                    1.0,
                    crate::shared_input::shared_input(
                        theme,
                        &self.global_search_query_editor,
                        crate::shared_input::SharedInputOpts {
                            width: crate::shared_input::SharedInputWidth::Fill,
                            radius: 5.0,
                            background: Some(theme.background),
                            border: Some(theme.accent),
                            pad_left: crate::theme::scaled(8.0),
                            pad_right: crate::theme::scaled(6.0),
                            pad_top: crate::theme::scaled(3.0),
                            pad_bottom: crate::theme::scaled(3.0),
                            ..Default::default()
                        },
                    ),
                )
                .finish(),
            )
            // Inline option toggles (VS Code: controls inside SearchFindInput)
            .with_child(make_toggle(
                "case_sensitive".to_string(),
                "Aa",
                "Match case",
                options.case_sensitive,
            ))
            .with_child(make_toggle(
                "whole_word".to_string(),
                "ab",
                "Match whole word",
                options.whole_word,
            ))
            .with_child(make_toggle(
                "regex".to_string(),
                ".*",
                "Use regular expression",
                options.regex,
            ))
            .finish();

        header_col = header_col.with_child(
            Container::new(search_input_row)
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
        );

        let can_replace = results
            .as_ref()
            .is_some_and(|results| results.total_hits > 0)
            && !in_progress;
        let replace_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(
                Expanded::new(
                    1.0,
                    crate::shared_input::shared_input(
                        theme,
                        &self.global_search_replace_editor,
                        crate::shared_input::SharedInputOpts {
                            ..Default::default()
                        },
                    ),
                )
                .finish(),
            )
            .with_child(global_search_icon_button(
                AppIcon::ReplaceAll,
                "Replace all results",
                SidebarAction::ReplaceAllGlobalSearchMatches,
                can_replace,
                theme,
                ui,
                view_id,
            ))
            .finish();
        header_col = header_col.with_child(
            Container::new(replace_row)
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .with_border(Border::top(1.0).with_border_color(theme.accent))
                .finish(),
        );

        // Include and exclude filters stay visible so scope is always clear.
        {
            let include_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(3.0)
                .with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(4.0)
                        .finish(),
                )
                .with_child(
                    Text::new("include:".to_string(), ui, crate::theme::text_size(10.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        crate::shared_input::shared_input(
                            theme,
                            &self.global_search_include_editor,
                            crate::shared_input::SharedInputOpts {
                                ..Default::default()
                            },
                        ),
                    )
                    .finish(),
                )
                .finish();

            let exclude_row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(3.0)
                .with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(4.0)
                        .finish(),
                )
                .with_child(
                    Text::new("exclude:".to_string(), ui, crate::theme::text_size(10.5))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        crate::shared_input::shared_input(
                            theme,
                            &self.global_search_exclude_editor,
                            crate::shared_input::SharedInputOpts {
                                ..Default::default()
                            },
                        ),
                    )
                    .finish(),
                )
                .finish();

            header_col = header_col.with_child(
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_main_axis_size(MainAxisSize::Min)
                        .with_child(
                            Container::new(include_row)
                                .with_padding_left(2.0)
                                .with_padding_right(8.0)
                                .with_padding_top(5.0)
                                .with_padding_bottom(3.0)
                                .finish(),
                        )
                        .with_child(
                            Container::new(exclude_row)
                                .with_padding_left(2.0)
                                .with_padding_right(8.0)
                                .with_padding_top(3.0)
                                .with_padding_bottom(5.0)
                                .finish(),
                        )
                        .finish(),
                )
                .with_background_color(theme.panel_background())
                .with_border(Border::top(1.0).with_border_color(theme.accent))
                .finish(),
            );
        }

        if in_progress {
            header_col = header_col.with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(render_icon(AppIcon::Search, theme.accent, 11.0))
                        .with_child(
                            Text::new(
                                if replacing {
                                    "Applying replacements…".to_string()
                                } else {
                                    "Searching project…".to_string()
                                },
                                ui,
                                crate::theme::text_size(10.5),
                            )
                            .with_color(theme.sub_text())
                            .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(8.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .finish(),
            );
        }

        // ════════════════════════════════════════════════════════════════
        // RESULTS (VS Code: div.results with virtual tree)
        // 22px fixed row height, no sticky headers
        // ════════════════════════════════════════════════════════════════

        const MATCH_ROW_HEIGHT: f32 = 24.0;
        const FILE_ROW_HEIGHT: f32 = 28.0;
        let results_scroll_top = self.global_search_scroll.scroll_start().as_f32().max(0.0);
        let results_viewport_height = {
            let measured = self.global_search_scroll.viewport_height();
            if measured.is_finite() && measured > 0.0 {
                measured
            } else {
                700.0
            }
        };
        let results_overscan = 8.0 * MATCH_ROW_HEIGHT;
        let results_render_start = (results_scroll_top - results_overscan).max(0.0);
        let results_render_end = results_scroll_top + results_viewport_height + results_overscan;
        let mut result_file_top = 0.0;
        let mut results_col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);

        if let Some(err) = &error {
            header_col = header_col.with_child(
                Container::new(
                    Text::new(err.clone(), ui, crate::theme::text_size(11.0))
                        .with_color(theme.ui_red())
                        .finish(),
                )
                .with_padding_left(10.0)
                .with_padding_right(8.0)
                .with_padding_top(8.0)
                .finish(),
            );
        } else if let Some(results) = results {
            if results.files.is_empty() && !results.query.is_empty() {
                header_col = header_col.with_child(
                    Container::new(
                        Text::new(
                            format!("No results for \"{}\"", results.query),
                            ui,
                            crate::theme::text_size(11.5),
                        )
                        .with_color(theme.sub_text())
                        .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_right(8.0)
                    .with_padding_top(12.0)
                    .finish(),
                );
            } else if !results.files.is_empty() {
                // Summary message (VS Code: div.messages)
                let summary = if results.truncated {
                    format!(
                        "{} results in {} files (limited)",
                        results.total_hits,
                        results.files.len()
                    )
                } else {
                    format!(
                        "{} results in {} files",
                        results.total_hits,
                        results.files.len()
                    )
                };
                let active_number = active_match.as_ref().and_then(|active| {
                    results
                        .files
                        .iter()
                        .flat_map(|file| {
                            file.hits
                                .iter()
                                .enumerate()
                                .map(|(index, _)| (&file.relative_path, index))
                        })
                        .position(|candidate| candidate.0 == &active.0 && candidate.1 == active.1)
                        .map(|index| index + 1)
                });
                let result_status = active_number.map_or(summary, |index| {
                    format!(
                        "{index} of {}  ·  {} files",
                        results.total_hits,
                        results.files.len()
                    )
                });
                let summary_row = Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(result_status, ui, crate::theme::text_size(10.5))
                                .with_color(theme.sub_text())
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        EventHandler::new(
                            Container::new(
                                Text::new(
                                    "Open in editor".to_string(),
                                    ui,
                                    crate::theme::text_size(10.5),
                                )
                                .with_color(theme.accent)
                                .with_style(Properties::default().weight(Weight::Medium))
                                .finish(),
                            )
                            .with_padding_left(crate::theme::scaled(5.0))
                            .with_padding_right(crate::theme::scaled(7.0))
                            .with_padding_top(crate::theme::scaled(4.0))
                            .with_padding_bottom(crate::theme::scaled(4.0))
                            .finish(),
                        )
                        .on_left_mouse_down(move |ctx, _, _| {
                            ctx.dispatch_typed_action_for(
                                workspace_view_id,
                                WorkspaceAction::OpenProjectSearchResults,
                            );
                            DispatchEventResult::StopPropagation
                        })
                        .finish(),
                    )
                    .finish();
                header_col = header_col.with_child(
                    Container::new(summary_row)
                        .with_padding_left(10.0)
                        .with_padding_right(8.0)
                        .with_padding_top(3.0)
                        .with_padding_bottom(3.0)
                        .with_border(Border::bottom(1.0).with_border_color(theme.accent))
                        .finish(),
                );

                // Render each file + its matches as flat 22px rows
                for file_result in &results.files {
                    let is_collapsed = collapsed.contains(&file_result.relative_path);
                    let body_height = if is_collapsed {
                        0.0
                    } else {
                        file_result.hits.len() as f32 * MATCH_ROW_HEIGHT
                    };
                    let file_height = FILE_ROW_HEIGHT + body_height;
                    let file_bottom = result_file_top + file_height;
                    if file_bottom < results_render_start || result_file_top > results_render_end {
                        results_col = results_col.with_child(
                            ConstrainedBox::new(Empty::new().finish())
                                .with_height(file_height)
                                .finish(),
                        );
                        result_file_top = file_bottom;
                        continue;
                    }

                    results_col = results_col.with_child(render_sidebar_search_file_header(
                        file_result,
                        is_collapsed,
                        view_id,
                        ui,
                        theme,
                        false,
                    ));

                    // ── Match rows (VS Code: .linematch, 22px each) ────
                    if !is_collapsed {
                        let body_top = result_file_top + FILE_ROW_HEIGHT;
                        let visible_rows = editor_render::viewport_row_range(
                            file_result.hits.len(),
                            results_scroll_top - body_top,
                            results_viewport_height,
                            MATCH_ROW_HEIGHT,
                            MULTI_FILE_SEARCH_OVERSCAN_ROWS,
                        );
                        if visible_rows.start > 0 {
                            results_col = results_col.with_child(
                                ConstrainedBox::new(Empty::new().finish())
                                    .with_height(visible_rows.start as f32 * MATCH_ROW_HEIGHT)
                                    .finish(),
                            );
                        }
                        for hit_index in visible_rows.clone() {
                            let hit = &file_result.hits[hit_index];
                            let line_num = hit.line;
                            // Keep backend byte ranges valid even when a match
                            // includes trailing whitespace.
                            let line_text = hit.line_text.clone();
                            let match_range = hit.match_range;
                            let is_active = active_match.as_ref().is_some_and(|active| {
                                active.0 == file_result.relative_path && active.1 == hit_index
                            });

                            // Build display text with match highlighting
                            // VS Code: span (before) + span.findInFileMatch (match) + span (after)
                            let (before_text, match_text, after_text) =
                                if let Some((start, end)) = match_range {
                                    (
                                        line_text[..start].to_string(),
                                        line_text[start..end].to_string(),
                                        line_text[end..].to_string(),
                                    )
                                } else {
                                    (String::new(), line_text.clone(), String::new())
                                };

                            // Preserve the match itself in narrow docks by
                            // trimming context asymmetrically around it.
                            let max_before_ctx = 24;
                            let max_after_ctx = 48;
                            let before_display = if before_text.chars().count() > max_before_ctx {
                                let truncated: String = before_text
                                    .chars()
                                    .rev()
                                    .take(max_before_ctx)
                                    .collect::<String>()
                                    .chars()
                                    .rev()
                                    .collect();
                                format!("…{}", truncated)
                            } else {
                                before_text
                            };
                            let after_display = if after_text.chars().count() > max_after_ctx {
                                let truncated: String =
                                    after_text.chars().take(max_after_ctx).collect();
                                format!("{}…", truncated)
                            } else {
                                after_text
                            };

                            let match_row_mouse = MouseStateHandle::default();
                            let match_path = file_result.relative_path.clone();
                            let select_path = match_path.clone();
                            let open_path = match_path.clone();
                            let open_root = search_root.clone();
                            let open_range = project_search_hit_range(hit);
                            let match_row = Hoverable::new(match_row_mouse, move |state| {
                                let bg = if is_active {
                                    theme.accent_overlay()
                                } else if state.is_hovered() {
                                    theme.surface_1()
                                } else {
                                    ColorU::transparent_black()
                                };
                                Flex::row()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_main_axis_size(MainAxisSize::Max)
                                    // Line number (VS Code: span.matchLineNum)
                                    .with_child(
                                        Text::new(
                                            format!("{:>4}:", line_num),
                                            ui,
                                            crate::theme::text_size(10.0),
                                        )
                                        .with_color(theme.sub_text())
                                        .finish(),
                                    )
                                    // Before text
                                    .with_child(
                                        Text::new(before_display, mono, crate::theme::text_size(10.5))
                                            .with_color(theme.sub_text())
                                            .finish(),
                                    )
                                    // Match highlight (VS Code: span.findInFileMatch)
                                    .with_child(
                                        Container::new(
                                            Text::new(match_text, mono, crate::theme::text_size(10.5))
                                                .with_color(theme.main_text())
                                                .with_style(
                                                    Properties::default().weight(Weight::Semibold),
                                                )
                                                .finish(),
                                        )
                                        .with_background_color(theme.accent_overlay())
                                        .finish(),
                                    )
                                    // After text
                                    .with_child(
                                        Expanded::new(
                                            1.0,
                                            Text::new(
                                                after_display,
                                                mono,
                                                crate::theme::text_size(10.5),
                                            )
                                            .with_color(theme.sub_text())
                                            .with_clip(ClipConfig::ellipsis())
                                            .finish(),
                                        )
                                        .finish(),
                                    )
                                    .finish()
                            })
                            .with_cursor(Cursor::PointingHand)
                            // Result rows are rebuilt while hover/selection state
                            // changes. Activate on mouse-down so navigation cannot
                            // lose the matching mouse-up to a newly rendered row.
                            .on_mouse_down(move |ctx, _app, _position| {
                                ctx.dispatch_typed_action_for(
                                    view_id,
                                    SidebarAction::SelectGlobalSearchMatch {
                                        path: select_path.clone(),
                                        hit: hit_index,
                                    },
                                );
                                if let Some(root) = open_root.clone() {
                                    ctx.dispatch_typed_action_for(
                                        workspace_view_id,
                                        WorkspaceAction::OpenLanguageLocation {
                                            root,
                                            relative_path: open_path.clone(),
                                            range: open_range,
                                        },
                                    );
                                }
                            })
                            .finish();

                            results_col = results_col.with_child(
                                ConstrainedBox::new(
                                    Container::new(match_row)
                                        .with_padding_left(18.0)
                                        .with_padding_right(8.0)
                                        .finish(),
                                )
                                .with_height(MATCH_ROW_HEIGHT)
                                .finish(),
                            );
                        }
                        if visible_rows.end < file_result.hits.len() {
                            results_col = results_col.with_child(
                                ConstrainedBox::new(Empty::new().finish())
                                    .with_height(
                                        (file_result.hits.len() - visible_rows.end) as f32
                                            * MATCH_ROW_HEIGHT,
                                    )
                                    .finish(),
                            );
                        }
                    }
                    result_file_top = file_bottom;
                }
            }
        } else if !in_progress {
            // Empty state (VS Code shows nothing, just the inputs)
        }

        let results_scroll = ClippedScrollable::vertical(
            self.global_search_scroll.clone(),
            results_col.finish(),
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.panel_background().into(),
        )
        .finish();
        let sticky_file = results.and_then(|results| {
            project_search_sidebar_sticky_file(
                results,
                &collapsed,
                self.global_search_scroll.scroll_start().as_f32(),
            )
        });
        let results_view = if let Some(file) = sticky_file {
            Stack::new()
                .with_child(results_scroll)
                .with_positioned_child(
                    render_sidebar_search_file_header(
                        file,
                        collapsed.contains(&file.relative_path),
                        view_id,
                        ui,
                        theme,
                        true,
                    ),
                    OffsetPositioning::offset_from_parent(
                        vec2f(0.0, 0.0),
                        ParentOffsetBounds::ParentBySize,
                        ParentAnchor::TopLeft,
                        ChildAnchor::TopLeft,
                    ),
                )
                .finish()
        } else {
            results_scroll
        };
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(header_col.finish())
                    .with_background_color(theme.panel_background())
                    .with_border(Border::bottom(1.0).with_border_color(theme.accent))
                    .finish(),
            )
            .with_child(Expanded::new(1.0, results_view).finish())
            .finish()
    }
}
