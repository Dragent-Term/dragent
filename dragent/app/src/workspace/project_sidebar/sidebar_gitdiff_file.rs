//! Git file-diff renders.
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
use super::view::{ProjectSidebarView, SidebarAction};

pub(crate) fn git_multi_hunk_action_button(
    icon: AppIcon,
    label: &'static str,
    color: ColorU,
    action: WorkspaceAction,
    workspace_view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let button = Hoverable::new(MouseStateHandle::default(), move |state| {
        let surface = Container::new(render_icon(icon, color, crate::theme::scaled(19.0)))
            .with_padding_left(crate::theme::scaled(5.0))
            .with_padding_right(crate::theme::scaled(5.0))
            .with_padding_top(crate::theme::scaled(1.0))
            .with_padding_bottom(crate::theme::scaled(1.0))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::transparent_black()
            })
            .finish();
        buffer_search_tooltip(surface, state.is_hovered(), label, theme, ui)
    })
    .with_cursor(Cursor::PointingHand)
    .finish();
    EventHandler::new(button)
        .on_left_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(workspace_view_id, action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

fn git_diff_action_button(
    icon: AppIcon,
    label: &'static str,
    action: SidebarAction,
    view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
) -> Box<dyn Element> {
    Hoverable::new(MouseStateHandle::default(), move |state| {
        let button = Container::new(render_icon(
            icon,
            theme.sub_text(),
            crate::theme::scaled(12.0),
        ))
        .with_padding_left(crate::theme::scaled(5.0))
        .with_padding_right(crate::theme::scaled(5.0))
        .with_padding_top(crate::theme::scaled(4.0))
        .with_padding_bottom(crate::theme::scaled(4.0))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_background_color(if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .finish();
        buffer_search_tooltip(button, state.is_hovered(), label, theme, ui)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(view_id, action.clone());
    })
    .finish()
}

fn git_diff_open_file_button(
    path: PathBuf,
    root: PathBuf,
    workspace_view_id: EntityId,
    theme: &Theme,
    ui: FamilyId,
) -> Box<dyn Element> {
    Hoverable::new(MouseStateHandle::default(), move |state| {
        let button = Container::new(render_icon(
            AppIcon::Code,
            theme.sub_text(),
            crate::theme::scaled(12.0),
        ))
        .with_padding_left(crate::theme::scaled(5.0))
        .with_padding_right(crate::theme::scaled(5.0))
        .with_padding_top(crate::theme::scaled(4.0))
        .with_padding_bottom(crate::theme::scaled(4.0))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_background_color(if state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .finish();
        buffer_search_tooltip(button, state.is_hovered(), "Open editable file", theme, ui)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenReadOnlyDocument {
                root: root.clone(),
                relative_path: path.clone(),
            },
        );
    })
    .finish()
}

pub(crate) fn pair_git_diff_lines(
    hunk: &dragent_document::DiffHunk,
) -> Vec<(
    Option<&dragent_document::DiffLine>,
    Option<&dragent_document::DiffLine>,
)> {
    use dragent_document::DiffLineKind;

    let mut rows = Vec::new();
    let mut index = 0;
    while index < hunk.lines.len() {
        if hunk.lines[index].kind == DiffLineKind::Context {
            rows.push((Some(&hunk.lines[index]), Some(&hunk.lines[index])));
            index += 1;
            continue;
        }
        let start = index;
        while index < hunk.lines.len() && hunk.lines[index].kind != DiffLineKind::Context {
            index += 1;
        }
        let changed = &hunk.lines[start..index];
        let removed = changed
            .iter()
            .filter(|line| line.kind == DiffLineKind::Removed)
            .collect::<Vec<_>>();
        let added = changed
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .collect::<Vec<_>>();
        for row in 0..removed.len().max(added.len()) {
            rows.push((removed.get(row).copied(), added.get(row).copied()));
        }
    }
    rows
}

pub(crate) fn git_intraline_ranges(
    old: &str,
    new: &str,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    if old == new || old.is_empty() || new.is_empty() {
        return None;
    }
    let mut prefix = 0;
    for (old_char, new_char) in old.chars().zip(new.chars()) {
        if old_char != new_char {
            break;
        }
        prefix += old_char.len_utf8();
    }
    let old_remaining = &old[prefix..];
    let new_remaining = &new[prefix..];
    let mut suffix = 0;
    for (old_char, new_char) in old_remaining.chars().rev().zip(new_remaining.chars().rev()) {
        if old_char != new_char {
            break;
        }
        suffix += old_char.len_utf8();
    }
    Some((prefix..old.len() - suffix, prefix..new.len() - suffix))
}

pub(crate) fn render_git_diff_source(
    line: &str,
    intraline: Option<&std::ops::Range<usize>>,
    color: ColorU,
    language: dragent_editor::Language,
    mono: FamilyId,
    text_size: f32,
    theme: &Theme,
) -> Box<dyn Element> {
    // Diff whitespace + word-wrap from Version Control.
    let diff_ws = crate::console::editor_settings::EditorSettings::global()
        .editor
        .show_whitespace
        || std::panic::catch_unwind(crate::settings::git_consumers::diff_show_whitespace).unwrap_or(false);
    let diff_wrap =
        std::panic::catch_unwind(crate::settings::git_consumers::diff_word_wrap).unwrap_or(false);
    let Some(range) = intraline.filter(|range| !range.is_empty()) else {
        return crate::workspace::editor_render::highlighted_source(
            line, language, theme, mono, text_size, Weight::Normal, diff_ws,
        )
        .soft_wrap(diff_wrap)
        .finish();
    };
    let segment = |text: &str| {
        crate::workspace::editor_render::highlighted_source(
            text, language, theme, mono, text_size, Weight::Normal, diff_ws,
        )
        .soft_wrap(diff_wrap)
        .finish()
    };
    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(segment(&line[..range.start]))
        .with_child(
            Container::new(segment(&line[range.clone()]))
                .with_background_color(ColorU { a: 72, ..color })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                .finish(),
        )
        .with_child(segment(&line[range.end..]))
        .finish()
}

pub(crate) fn git_diff_hunk_offset(
    file: &crate::git_status::diff_head::GitFileDiff,
    target: usize,
    side_by_side: bool,
    show_full_file: bool,
) -> f32 {
    git_diff_display(file, show_full_file)
        .hunks
        .iter()
        .take(target)
        .map(|hunk| {
            let rows = if side_by_side {
                pair_git_diff_lines(hunk).len()
            } else {
                hunk.lines.len()
            };
            crate::theme::scaled(25.0 + rows as f32 * 19.0)
        })
        .sum()
}

pub(crate) fn git_diff_display(
    file: &crate::git_status::diff_head::GitFileDiff,
    show_full_file: bool,
) -> &dragent_document::DocumentDiff {
    if show_full_file {
        &file.full_diff
    } else {
        &file.diff
    }
}

fn render_git_split_side(
    line: Option<&dragent_document::DiffLine>,
    intraline: Option<&std::ops::Range<usize>>,
    removed_side: bool,
    language: dragent_editor::Language,
    mono: FamilyId,
    text_size: f32,
    line_number_size: f32,
    theme: &Theme,
) -> Box<dyn Element> {
    use dragent_document::DiffLineKind;

    let kind = line.map(|line| line.kind);
    let changed =
        matches!(kind, Some(DiffLineKind::Added | DiffLineKind::Removed)) || line.is_none();
    let color = if removed_side {
        theme.ui_red()
    } else {
        theme.ui_green()
    };
    let background = if changed {
        ColorU { a: 24, ..color }
    } else {
        theme.background
    };
    let number = line
        .and_then(|line| {
            if removed_side {
                line.old_line
            } else {
                line.new_line
            }
        })
        .map(|line| line.to_string())
        .unwrap_or_default();
    let source: Box<dyn Element> = line.map_or_else(
        || Empty::new().finish(),
        |line| {
            render_git_diff_source(
                &line.text, intraline, color, language, mono, text_size, theme,
            )
        },
    );
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(
                    Align::new(
                        Text::new(number, mono, line_number_size)
                            .with_color(theme.disabled_text())
                            .finish(),
                    )
                    .right()
                    .finish(),
                )
                .with_width(crate::theme::scaled(42.0))
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    Align::new(
                        Text::new(
                            if changed {
                                if removed_side {
                                    "−"
                                } else {
                                    "+"
                                }
                            } else {
                                " "
                            }
                            .to_string(),
                            mono,
                            text_size,
                        )
                        .with_color(if changed {
                            color
                        } else {
                            theme.disabled_text()
                        })
                        .finish(),
                    )
                    .finish(),
                )
                .with_width(crate::theme::scaled(22.0))
                .finish(),
            )
            .with_child(Expanded::new(1.0, source).finish())
            .finish(),
    )
    .with_padding_right(crate::theme::scaled(8.0))
    .with_padding_top(crate::theme::scaled(1.5))
    .with_padding_bottom(crate::theme::scaled(1.5))
    .with_background_color(background)
    .finish()
}

fn render_git_collapsed_region(
    region_id: usize,
    line_count: usize,
    view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let mouse = warpui_core::elements::MouseStateHandle::default();
    let theme = *theme;
    Hoverable::new(mouse, move |state| {
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(Container::new(Empty::new().finish()).finish())
                        .with_width(crate::theme::scaled(106.0))
                        .finish(),
                )
                .with_child(
                    Text::new(
                        format!("Expand {line_count} unchanged lines"),
                        ui,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    })
                    .finish(),
                )
                .finish(),
        )
        .with_padding_top(crate::theme::scaled(6.0))
        .with_padding_bottom(crate::theme::scaled(6.0))
        .with_background_color(if state.is_hovered() {
            theme.surface_1()
        } else {
            theme.panel_background()
        })
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish()
    })
    .with_cursor(warpui_core::platform::Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(view_id, SidebarAction::ExpandGitDiffRegion(region_id));
    })
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn render_git_expanded_context_row(
    source_text: &str,
    old_line: usize,
    new_line: usize,
    side_by_side: bool,
    language: dragent_editor::Language,
    mono: FamilyId,
    text_size: f32,
    line_number_size: f32,
    theme: &Theme,
) -> Box<dyn Element> {
    let line = dragent_document::DiffLine {
        kind: dragent_document::DiffLineKind::Context,
        old_line: Some(old_line),
        new_line: Some(new_line),
        text: source_text.to_string(),
    };
    if side_by_side {
        return Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Expanded::new(
                    1.0,
                    render_git_split_side(
                        Some(&line),
                        None,
                        true,
                        language,
                        mono,
                        text_size,
                        line_number_size,
                        theme,
                    ),
                )
                .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    Container::new(Empty::new().finish())
                        .with_background_color(theme.outline())
                        .finish(),
                )
                .with_width(1.0)
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    render_git_split_side(
                        Some(&line),
                        None,
                        false,
                        language,
                        mono,
                        text_size,
                        line_number_size,
                        theme,
                    ),
                )
                .finish(),
            )
            .finish();
    }

    let number = |value: usize| {
        Text::new(value.to_string(), mono, line_number_size)
            .with_color(theme.disabled_text())
            .finish()
    };
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                ConstrainedBox::new(Align::new(number(old_line)).right().finish())
                    .with_width(crate::theme::scaled(42.0))
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(Align::new(number(new_line)).right().finish())
                    .with_width(crate::theme::scaled(42.0))
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(crate::theme::scaled(22.0))
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.0,
                    render_git_diff_source(
                        source_text,
                        None,
                        theme.disabled_text(),
                        language,
                        mono,
                        text_size,
                        theme,
                    ),
                )
                .finish(),
            )
            .finish(),
    )
    .with_padding_right(crate::theme::scaled(12.0))
    .with_padding_top(crate::theme::scaled(1.5))
    .with_padding_bottom(crate::theme::scaled(1.5))
    .with_background_color(theme.background)
    .finish()
}

pub(crate) fn render_git_file_diff(
    file: &crate::git_status::diff_head::GitFileDiff,
    comments: &ProjectSidebarView,
    root: &Path,
    scroll: ClippedScrollStateHandle,
    view_id: EntityId,
    workspace_view_id: EntityId,
    terminal_id: Option<EntityId>,
    side_by_side: bool,
    show_full_file: bool,
    expanded_regions: &std::collections::HashSet<usize>,
    active_hunk: usize,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    use dragent_document::DiffLineKind;

    let (old_label, new_label) = file.target.labels();
    let language = dragent_editor::Language::from_path(&file.relative_path);
    let text_size = crate::theme::text_size(12.5);
    let line_number_size = crate::theme::text_size(10.5);
    let added = file.additions();
    let deleted = file.deletions();
    let tint = |color: ColorU, alpha| ColorU { a: alpha, ..color };

    let display_diff = git_diff_display(file, show_full_file);
    let hunk_count = display_diff.hunks.len();
    let comparison = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(crate::theme::scaled(8.0))
        .with_child(if file.base_relative_path != file.relative_path {
            Text::new(
                format!(
                    "{}  →  {}",
                    file.base_relative_path.display(),
                    file.relative_path.display()
                ),
                ui,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.main_text())
            .finish()
        } else {
            Empty::new().finish()
        })
        .with_child(
            Text::new(
                format!("{old_label}  →  {new_label}"),
                ui,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .with_child(
            Text::new(format!("+{added}"), mono, crate::theme::text_size(10.5))
                .with_color(theme.ui_green())
                .finish(),
        )
        .with_child(
            Text::new(format!("−{deleted}"), mono, crate::theme::text_size(10.5))
                .with_color(theme.ui_red())
                .finish(),
        )
        .finish();
    let controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(crate::theme::scaled(3.0))
        .with_child(
            Text::new(
                if hunk_count == 0 {
                    "0 of 0".to_string()
                } else {
                    format!("{} of {hunk_count}", active_hunk.min(hunk_count - 1) + 1)
                },
                mono,
                crate::theme::text_size(10.0),
            )
            .with_color(theme.disabled_text())
            .finish(),
        )
        .with_child(git_diff_open_file_button(
            file.relative_path.clone(),
            root.to_path_buf(),
            workspace_view_id,
            theme,
            ui,
        ))
        .with_child(git_diff_action_button(
            AppIcon::ArrowUp,
            "Previous change",
            SidebarAction::NavigateGitDiffHunk(-1),
            view_id,
            theme,
            ui,
        ))
        .with_child(git_diff_action_button(
            AppIcon::ArrowDown,
            "Next change",
            SidebarAction::NavigateGitDiffHunk(1),
            view_id,
            theme,
            ui,
        ))
        .with_child(git_diff_action_button(
            if side_by_side {
                AppIcon::Code
            } else {
                AppIcon::SplitVertical
            },
            if side_by_side {
                "Use unified view"
            } else {
                "Use side-by-side view"
            },
            SidebarAction::ToggleGitDiffLayout,
            view_id,
            theme,
            ui,
        ))
        .with_child(git_diff_action_button(
            if show_full_file {
                AppIcon::ChevronRight
            } else {
                AppIcon::ChevronDown
            },
            if show_full_file {
                "Show changes only"
            } else {
                "Show full file"
            },
            SidebarAction::ToggleGitDiffFullFile,
            view_id,
            theme,
            ui,
        ))
        .finish();
    let summary = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, comparison).finish())
            .with_child(controls)
            .finish(),
    )
    .with_padding_left(crate::theme::scaled(14.0))
    .with_padding_right(crate::theme::scaled(14.0))
    .with_padding_top(crate::theme::scaled(8.0))
    .with_padding_bottom(crate::theme::scaled(8.0))
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .with_background_color(theme.panel_background())
    .finish();

    let content: Box<dyn Element> = if file.binary {
        Align::new(
            Text::new(
                "Binary files cannot be displayed as text.".to_string(),
                ui,
                crate::theme::text_size(13.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .finish()
    } else if display_diff.is_empty() {
        Align::new(
            Text::new(
                "No changes in this comparison.".to_string(),
                ui,
                crate::theme::text_size(13.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .finish()
    } else {
        let mut rows = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);
        // Compact hunks carry their own context. Everything between them is a
        // real equal-line region that the display may reveal independently.
        // This is presentation state only; neither Git snapshot is mutated.
        // Collapse-unchanged toggle from Version Control.
        let collapsed_threshold: usize =
            if std::panic::catch_unwind(crate::settings::git_consumers::diff_collapse_unchanged)
                .unwrap_or(false)
            {
                9
            } else {
                usize::MAX
            };
        let old_line_count = file.old_text.lines().count();
        let new_line_count = file.new_text.lines().count();
        let unchanged_regions = if show_full_file {
            Vec::new()
        } else {
            display_diff.unchanged_regions(old_line_count, new_line_count, collapsed_threshold)
        };
        let new_lines = file.new_text.lines().collect::<Vec<_>>();
        for (hunk_index, hunk) in display_diff.hunks.iter().enumerate() {
            if let Some(region) = unchanged_regions
                .iter()
                .find(|region| region.id == hunk_index)
            {
                if expanded_regions.contains(&region.id) {
                    for offset in comment_region_offsets(region, comments, file) {
                        let source = new_lines
                            .get(region.new_start.saturating_sub(1) + offset)
                            .copied()
                            .unwrap_or_default();
                        rows = rows.with_child(comments.diff_comment_context_row(render_git_expanded_context_row(
                            source,
                            region.old_start + offset,
                            region.new_start + offset,
                            side_by_side,
                            language,
                            mono,
                            text_size,
                            line_number_size,
                            theme,
                        ), file, region.new_start + offset));
                    }
                } else {
                    rows = rows.with_child(render_git_collapsed_region(
                        region.id,
                        region.line_count,
                        view_id,
                        ui,
                        theme,
                    ));
                }
            }
            let old_start = hunk
                .lines
                .iter()
                .find_map(|line| line.old_line)
                .unwrap_or(0);
            let old_count = hunk
                .lines
                .iter()
                .filter(|line| line.old_line.is_some())
                .count();
            let new_start = hunk
                .lines
                .iter()
                .find_map(|line| line.new_line)
                .unwrap_or(0);
            let new_count = hunk
                .lines
                .iter()
                .filter(|line| line.new_line.is_some())
                .count();
            let hunk_header = if crate::git_features::hunk_stage_enabled() {
                let add_mouse = warpui_core::elements::MouseStateHandle::default();
                let discard_mouse = warpui_core::elements::MouseStateHandle::default();
                let staged = file.target == crate::git_status::diff_head::GitDiffTarget::Staged;
                let add_icon = render_icon(
                    if staged {
                        AppIcon::Undo2
                    } else {
                        AppIcon::Plus
                    },
                    if staged {
                        theme.sub_text()
                    } else {
                        theme.ansi_green()
                    },
                    12.0,
                );
                let discard_icon = render_icon(AppIcon::Undo2, theme.ansi_red(), 12.0);
                let rel_for_add = file.relative_path.clone();
                let rel_for_discard = file.relative_path.clone();
                let ws_for_add = workspace_view_id;
                let ws_for_discard = workspace_view_id;
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_child(
                            Expanded::new(
                                1.0,
                                Text::new(
                                    format!(
                                        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
                                    ),
                                    mono,
                                    line_number_size,
                                )
                                .with_color(theme.accent)
                                .finish(),
                            )
                            .finish(),
                        )
                        .with_child(
                            Hoverable::new(add_mouse, move |st| {
                                Container::new(add_icon)
                                    .with_padding_left(4.0)
                                    .with_padding_right(4.0)
                                    .with_padding_top(2.0)
                                    .with_padding_bottom(2.0)
                                    .with_background_color(if st.is_hovered() {
                                        theme.surface_2()
                                    } else {
                                        ColorU::transparent_black()
                                    })
                                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                                    .finish()
                            })
                            .with_cursor(warpui_core::platform::Cursor::PointingHand)
                            .on_click(move |ctx, _app, _pos| {
                                ctx.dispatch_typed_action_for(
                                    ws_for_add,
                                    crate::workspace::WorkspaceAction::StageGitHunk {
                                        rel_path: rel_for_add.to_string_lossy().into_owned(),
                                        hunk_index,
                                        old_start,
                                        new_start,
                                        staged,
                                    },
                                );
                            })
                            .finish(),
                        )
                        .with_child(if staged {
                            Empty::new().finish()
                        } else {
                            Hoverable::new(discard_mouse, move |st| {
                                Container::new(discard_icon)
                                    .with_padding_left(4.0)
                                    .with_padding_right(4.0)
                                    .with_padding_top(2.0)
                                    .with_padding_bottom(2.0)
                                    .with_background_color(if st.is_hovered() {
                                        theme.surface_2()
                                    } else {
                                        ColorU::transparent_black()
                                    })
                                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                                    .finish()
                            })
                            .with_cursor(warpui_core::platform::Cursor::PointingHand)
                            .on_click(move |ctx, _app, _pos| {
                                ctx.dispatch_typed_action_for(
                                    ws_for_discard,
                                    crate::workspace::WorkspaceAction::RequestDiscardGitHunk {
                                        rel_path: rel_for_discard.to_string_lossy().into_owned(),
                                        hunk_index,
                                        old_start,
                                        new_start,
                                    },
                                );
                            })
                            .finish()
                        })
                        .finish(),
                )
                .with_padding_left(crate::theme::scaled(12.0))
                .with_padding_right(crate::theme::scaled(6.0))
                .with_padding_top(crate::theme::scaled(5.0))
                .with_padding_bottom(crate::theme::scaled(5.0))
                .with_border(Border::left(crate::theme::scaled(2.0)).with_border_color(
                    if hunk_index == active_hunk {
                        theme.accent
                    } else {
                        ColorU::transparent_black()
                    },
                ))
                .with_background_color(tint(
                    theme.accent,
                    if hunk_index == active_hunk { 30 } else { 18 },
                ))
                .finish()
            } else {
                Container::new(
                    Text::new(
                        format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
                        mono,
                        line_number_size,
                    )
                    .with_color(theme.accent)
                    .finish(),
                )
                .with_padding_left(crate::theme::scaled(12.0))
                .with_padding_top(crate::theme::scaled(5.0))
                .with_padding_bottom(crate::theme::scaled(5.0))
                .with_border(Border::left(crate::theme::scaled(2.0)).with_border_color(
                    if hunk_index == active_hunk {
                        theme.accent
                    } else {
                        ColorU::transparent_black()
                    },
                ))
                .with_background_color(tint(
                    theme.accent,
                    if hunk_index == active_hunk { 30 } else { 18 },
                ))
                .finish()
            };
            rows = rows.with_child(hunk_header);
            let mut intraline_by_line = HashMap::new();
            for (old_line, new_line) in pair_git_diff_lines(hunk) {
                if let (Some(old), Some(new)) = (old_line, new_line) {
                    if let Some((old_range, new_range)) = git_intraline_ranges(&old.text, &new.text)
                    {
                        intraline_by_line.insert(old as *const _ as usize, old_range);
                        intraline_by_line.insert(new as *const _ as usize, new_range);
                    }
                }
            }
            if side_by_side {
                for (old_line, new_line) in pair_git_diff_lines(hunk) {
                    let intraline = old_line
                        .zip(new_line)
                        .and_then(|(old, new)| git_intraline_ranges(&old.text, &new.text));
                    rows = rows.with_child(
                        Flex::row()
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    comment_split_side(comments, file, old_line, true,
                                        render_git_split_side(old_line, intraline.as_ref().map(|ranges| &ranges.0), true,
                                            language, mono, text_size, line_number_size, theme)),
                                )
                                .finish(),
                            )
                            .with_child(
                                ConstrainedBox::new(
                                    Container::new(Empty::new().finish())
                                        .with_background_color(theme.outline())
                                        .finish(),
                                )
                                .with_width(1.0)
                                .finish(),
                            )
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    comment_split_side(comments, file, new_line, false,
                                        render_git_split_side(new_line, intraline.as_ref().map(|ranges| &ranges.1), false,
                                            language, mono, text_size, line_number_size, theme)),
                                )
                                .finish(),
                            )
                            .finish(),
                    );
                    for (line, deleted) in [(old_line, true), (new_line, false)] {
                        if let Some(line) = line {
                            if let Some(composer) = comments.render_comment_after(&file.relative_path, file.target,
                                if deleted { line.old_line } else { line.new_line }.unwrap_or(1), deleted) {
                                rows = rows.with_child(composer);
                            }
                        }
                    }
                }
            } else {
                for line in &hunk.lines {
                    let (marker, marker_color, background) = match line.kind {
                        DiffLineKind::Added => ("+", theme.ui_green(), tint(theme.ui_green(), 24)),
                        DiffLineKind::Removed => ("−", theme.ui_red(), tint(theme.ui_red(), 24)),
                        DiffLineKind::Context => (" ", theme.disabled_text(), theme.background),
                    };
                    let number = |value: Option<usize>| {
                        Text::new(
                            value.map(|line| line.to_string()).unwrap_or_default(),
                            mono,
                            line_number_size,
                        )
                        .with_color(theme.disabled_text())
                        .finish()
                    };
                    let source = render_git_diff_source(
                        &line.text,
                        intraline_by_line.get(&(line as *const _ as usize)),
                        marker_color,
                        language,
                        mono,
                        text_size,
                        theme,
                    );
                    let comment_line = line.new_line.or(line.old_line).unwrap_or(1);
                    let deleted = line.kind == DiffLineKind::Removed;
                    let row = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(comments.diff_comment_gutter(&file.relative_path, file.target, comment_line, deleted))
                        .with_child(
                            ConstrainedBox::new(Align::new(number(line.old_line)).right().finish())
                                .with_width(crate::theme::scaled(42.0))
                                .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(Align::new(number(line.new_line)).right().finish())
                                .with_width(crate::theme::scaled(42.0))
                                .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Align::new(
                                    Text::new(marker.to_string(), mono, text_size)
                                        .with_color(marker_color)
                                        .finish(),
                                )
                                .finish(),
                            )
                            .with_width(crate::theme::scaled(22.0))
                            .finish(),
                        )
                        .with_child(Expanded::new(1.0, source).finish())
                        .finish();
                    rows = rows.with_child(comments.diff_comment_row(
                        Container::new(row)
                            .with_padding_right(crate::theme::scaled(12.0))
                            .with_padding_top(crate::theme::scaled(1.5))
                            .with_padding_bottom(crate::theme::scaled(1.5))
                            .with_background_color(background)
                            .finish(), &file.relative_path, file.target, comment_line, deleted));
                    if let Some(composer) = comments.render_comment_after(&file.relative_path, file.target, comment_line, deleted) {
                        rows = rows.with_child(composer);
                    }
                }
            }
        }
        if let Some(region) = unchanged_regions
            .iter()
            .find(|region| region.id == display_diff.hunks.len())
        {
            if expanded_regions.contains(&region.id) {
                for offset in comment_region_offsets(region, comments, file) {
                    let source = new_lines
                        .get(region.new_start.saturating_sub(1) + offset)
                        .copied()
                        .unwrap_or_default();
                    rows = rows.with_child(comments.diff_comment_context_row(render_git_expanded_context_row(
                        source,
                        region.old_start + offset,
                        region.new_start + offset,
                        side_by_side,
                        language,
                        mono,
                        text_size,
                        line_number_size,
                        theme,
                    ), file, region.new_start + offset));
                }
            } else {
                rows = rows.with_child(render_git_collapsed_region(
                    region.id,
                    region.line_count,
                    view_id,
                    ui,
                    theme,
                ));
            }
        }
        ClippedScrollable::vertical(
            scroll,
            rows.finish(),
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.background.into(),
        )
        .with_overlayed_scrollbar()
        .finish()
    };

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(summary)
        .with_child(Expanded::new(1.0, content).finish())
        .finish()
}

fn comment_split_side(comments: &ProjectSidebarView, file: &crate::git_status::diff_head::GitFileDiff,
    line: Option<&dragent_document::DiffLine>, deleted: bool, content: Box<dyn Element>) -> Box<dyn Element> {
    let Some(line) = line else { return content; };
    let number = if deleted { line.old_line } else { line.new_line }.unwrap_or(1);
    let row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(comments.diff_comment_gutter(&file.relative_path, file.target, number, deleted))
        .with_child(Expanded::new(1.0, content).finish()).finish();
    comments.diff_comment_row(row, &file.relative_path, file.target, number, deleted)
}

fn comment_region_offsets(region: &dragent_document::DiffUnchangedRegion, comments: &ProjectSidebarView,
    file: &crate::git_status::diff_head::GitFileDiff) -> std::ops::Range<usize> {
    if let Some(comment) = comments.comment_for_file(&file.relative_path, file.target) {
        let end = comment.anchor.end;
        if !comment.anchor.end_deleted && end >= region.new_start && end < region.new_start + region.line_count {
            let end_offset = (end - region.new_start + 4).min(region.line_count);
            let start_offset = comment.anchor.start.saturating_sub(region.new_start).saturating_sub(3).max(end_offset.saturating_sub(100));
            return start_offset..end_offset;
        }
    }
    0..region.line_count
}
