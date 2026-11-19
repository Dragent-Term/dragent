//! Sidebar git status rows and sticky groups.
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
use super::sidebar_githistory::gitlab_avatar_source;
use super::sidebar_githistory::{
    GIT_FILE_ROW_HEIGHT, GIT_GROUP_HEADER_HEIGHT, GIT_SECONDARY_ACTION_WIDTH,
    GIT_STATUS_BADGE_SIZE, GIT_STATUS_COLUMN_WIDTH, GIT_TRAILING_GROUP_WIDTH,
};
use super::view::{GitCommitAvatar, GitHistoryState, ProjectSidebarView, SidebarAction, META_SIZE};

/// The group whose header owns the top of the Source Control viewport.
/// `true` is Staged Changes and `false` is unstaged Changes. Measurements
/// mirror the intentionally compact, fixed-rhythm row design below.
pub(crate) fn git_changes_sticky_group(
    scroll_top: f32,
    staged_count: usize,
    staged_collapsed: bool,
    changes_count: usize,
    _changes_collapsed: bool,
) -> Option<bool> {
    if scroll_top <= 0.5 {
        return None;
    }
    if staged_count > 0 {
        let staged_height = crate::theme::scaled(GIT_GROUP_HEADER_HEIGHT)
            + if staged_collapsed {
                0.0
            } else {
                staged_count as f32 * crate::theme::scaled(GIT_FILE_ROW_HEIGHT)
            };
        if scroll_top < staged_height || changes_count == 0 {
            return Some(true);
        }
    }
    (changes_count > 0).then_some(false)
}

#[cfg(test)]
use super::sidebar_explorer::{project_search_hit_range, toggle_project_search_file};
mod project_search_navigation_tests {
    use super::*;

    #[test]
    fn search_hit_maps_to_zero_based_editor_selection() {
        let line = "const token = await refreshToken();";
        let start = line.find("refreshToken").unwrap();
        let end = start + "refreshToken".len();
        let range = project_search_hit_range(&crate::workspace::search_panel::SearchHit {
            line: 82,
            col: start + 1,
            line_text: line.to_string(),
            match_range: Some((start, end)),
        });

        assert_eq!(range.start.line, 81);
        assert_eq!(range.end.line, 81);
        assert_eq!(range.start.character, start as u32);
        assert_eq!(range.end.character, end as u32);
    }

    #[test]
    fn search_hit_converts_unicode_byte_offsets_to_utf16_columns() {
        let line = "😀 const café = refreshToken();";
        let start = line.find("refreshToken").unwrap();
        let end = start + "refreshToken".len();
        let range = project_search_hit_range(&crate::workspace::search_panel::SearchHit {
            line: 500,
            col: start + 1,
            line_text: line.to_string(),
            match_range: Some((start, end)),
        });

        assert_eq!(range.start.line, 499);
        assert_eq!(
            range.start.character,
            line[..start].encode_utf16().count() as u32
        );
        assert_eq!(
            range.end.character,
            line[..end].encode_utf16().count() as u32
        );
    }

    #[test]
    fn file_groups_toggle_independently() {
        let auth = PathBuf::from("src/auth.ts");
        let user = PathBuf::from("src/user.ts");
        let profile = PathBuf::from("src/profile.ts");
        let mut collapsed = std::collections::HashSet::from([user.clone()]);

        assert!(!toggle_project_search_file(&mut collapsed, &user));
        assert!(!collapsed.contains(&user));

        assert!(toggle_project_search_file(&mut collapsed, &auth));
        assert!(collapsed.contains(&auth));
        assert!(!collapsed.contains(&user));
        assert!(!collapsed.contains(&profile));

        assert!(toggle_project_search_file(&mut collapsed, &profile));
        assert!(collapsed.contains(&auth));
        assert!(collapsed.contains(&profile));
    }
}

#[cfg(test)]
mod git_changes_sticky_tests {
    use super::*;

    #[test]
    fn staged_header_hands_off_to_changes_at_its_section_boundary() {
        let staged_end = crate::theme::scaled(GIT_GROUP_HEADER_HEIGHT)
            + 3.0 * crate::theme::scaled(GIT_FILE_ROW_HEIGHT);
        assert_eq!(git_changes_sticky_group(0.0, 3, false, 4, false), None);
        assert_eq!(
            git_changes_sticky_group(1.0, 3, false, 4, false),
            Some(true)
        );
        assert_eq!(
            git_changes_sticky_group(staged_end + 1.0, 3, false, 4, false),
            Some(false)
        );
    }

    #[test]
    fn collapsed_staged_group_hands_off_after_only_its_header() {
        let header_end = crate::theme::scaled(GIT_GROUP_HEADER_HEIGHT);
        assert_eq!(
            git_changes_sticky_group(header_end + 1.0, 8, true, 2, false),
            Some(false)
        );
    }

    #[test]
    fn only_changes_group_becomes_sticky_when_scrolled() {
        assert_eq!(
            git_changes_sticky_group(5.0, 0, false, 12, false),
            Some(false)
        );
    }
}

/// Return the repository-relative display path as `(filename, parent, full)`.
/// Porcelain output is normally root-relative; stripping legacy `./` / `../`
/// prefixes keeps older snapshots and subdirectory-originated state quiet in
/// the UI without changing the path used by Git actions.
pub(crate) fn git_file_display_identity(rel_path: &str) -> (String, String, String) {
    let mut display = rel_path;
    while let Some(stripped) = display.strip_prefix("../") {
        display = stripped;
    }
    display = display.strip_prefix("./").unwrap_or(display);
    let (parent, filename) = display.rsplit_once('/').unwrap_or(("", display));
    (
        filename.to_string(),
        parent.to_string(),
        display.to_string(),
    )
}

pub(crate) fn git_status_marker(
    entry: &crate::console::git_status::GitStatusEntry,
    staged: bool,
) -> String {
    if !staged && entry.work == '?' {
        "??".to_string()
    } else {
        let marker = if staged { entry.index } else { entry.work };
        marker.to_string()
    }
}

/// One changed file as a three-region grid row:
///
/// ```text
/// │ STATUS │ FILE (name over path, flexible) │ STATS / ACTIONS │
/// ```
///
/// The middle column is the only main shrink target. The trailing area is one
/// compact two-line visual cluster: additions/deletions occupy its first line,
/// while the stage/unstage affordance occupies its second. This mirrors the
/// file block's rhythm and avoids spending a quarter of the row on tiny cells.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_git_file_row(
    entry: &crate::console::git_status::GitStatusEntry,
    staged: bool,
    diff_files: &[crate::console::git_status::GitDiffFileStat],
    git_root: Option<&Path>,
    terminal_id: Option<EntityId>,
    workspace_view_id: EntityId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    show_diff_stats: bool,
    discard_armed: bool,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let rel_path = entry.rel_path.clone();
    let status_marker = git_status_marker(entry, staged);
    let letter_color = match status_marker.chars().next().unwrap_or(' ') {
        'A' | '?' => theme.ansi_green(),
        'D' => theme.ansi_red(),
        'M' | 'R' => theme.ui_yellow(),
        _ => theme.sub_text(),
    };
    // Repository-relative identity: filename on top, parent path underneath.
    let (filename, parent_dir, display_path) = git_file_display_identity(&rel_path);
    let stats = show_diff_stats
        .then(|| {
            diff_files
                .iter()
                .find(|stat| stat.rel_path == rel_path)
                .map(|stat| (stat.added, stat.deleted))
        })
        .flatten();

    let row_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_file_{staged}_{rel_path}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let stage_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_stage_{staged}_{rel_path}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let discard_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_discard_{staged}_{rel_path}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let file_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_file_identity_{staged}_{rel_path}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();

    // Row data captured by value; the Hoverable closure rebuilds the grid so
    // hover state never changes the row's geometry.
    let status_marker_c = status_marker.clone();
    let filename_c = filename.clone();
    let parent_dir_c = parent_dir.clone();
    let display_path_c = display_path.clone();
    let rel_path_open = rel_path.clone();
    let git_root_open = git_root.map(Path::to_path_buf);
    let rel_path_stage = rel_path.clone();
    let rel_path_discard = rel_path.clone();

    let row = Hoverable::new(row_mouse, move |state| {
        let status_column_width = crate::theme::scaled(GIT_STATUS_COLUMN_WIDTH);
        let status_badge_size = crate::theme::scaled(GIT_STATUS_BADGE_SIZE);
        let secondary_action_width = crate::theme::scaled(GIT_SECONDARY_ACTION_WIDTH);
        let trailing_group_width = crate::theme::scaled(GIT_TRAILING_GROUP_WIDTH);

        // ── STATUS column (fixed gutter) ──────────────────────────────
        let status_col = ConstrainedBox::new(
            Align::new(
                Container::new(
                    ConstrainedBox::new(
                        Align::new(
                            Text::new(status_marker_c.clone(), mono, crate::theme::text_size(11.25))
                                .with_color(letter_color)
                                .with_style(Properties::default().weight(Weight::Semibold))
                                .soft_wrap(false)
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_width(status_badge_size)
                    .with_height(status_badge_size)
                    .finish(),
                )
                .with_background_color(ColorU::new(
                    letter_color.r,
                    letter_color.g,
                    letter_color.b,
                    if state.is_hovered() { 30 } else { 20 },
                ))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                    crate::theme::scaled(5.0),
                )))
                .finish(),
            )
            .left()
            .finish(),
        )
        .with_width(status_column_width)
        .finish();

        // ── FILE column (flexible): filename over muted repo path ─────
        let filename_for_file = filename_c.clone();
        let parent_for_file = parent_dir_c.clone();
        let path_for_tooltip = display_path_c.clone();
        let file_info = Hoverable::new(file_mouse.clone(), move |file_state| {
            let mut content = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(crate::theme::scaled(1.5))
                .with_child(
                    Text::new(filename_for_file.clone(), ui, crate::theme::text_size(13.0))
                        .with_clip(ClipConfig::ellipsis())
                        .with_color(theme.main_text())
                        .with_style(Properties::default().weight(Weight::Medium))
                        .soft_wrap(false)
                        .finish(),
                );
            if !parent_for_file.is_empty() {
                content = content.with_child(
                    Text::new(parent_for_file.clone(), ui, crate::theme::text_size(11.0))
                        .with_clip(ClipConfig::ellipsis())
                        .with_color(theme.sub_text())
                        .soft_wrap(false)
                        .finish(),
                );
            }
            buffer_search_tooltip(
                Shrinkable::new(1.0, content.finish()).finish(),
                file_state.is_hovered(),
                &path_for_tooltip,
                theme,
                ui,
            )
        })
        .finish();
        let file_col = Expanded::new(1.0, file_info).finish();

        // ── GIT cluster: one metadata group, then compact action group ─
        let (added_label, deleted_label) = stats
            .map(|(added, deleted)| (format!("+{added}"), format!("−{deleted}")))
            .unwrap_or_else(|| {
                if show_diff_stats && status_marker_c == "??" {
                    ("new".to_string(), String::new())
                } else {
                    (String::new(), String::new())
                }
            });
        let stats_group = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(crate::theme::scaled(7.0))
            .with_child(
                Text::new(added_label, mono, crate::theme::text_size(10.75))
                    .with_color(theme.ansi_green())
                    .with_style(Properties::default().weight(Weight::Medium))
                    .soft_wrap(false)
                    .finish(),
            )
            .with_child(
                Text::new(deleted_label, mono, crate::theme::text_size(10.75))
                    .with_color(theme.ansi_red())
                    .with_style(Properties::default().weight(Weight::Medium))
                    .soft_wrap(false)
                    .finish(),
            )
            .finish();
        let stage_tooltip = if staged {
            "Unstage Changes"
        } else {
            "Stage Changes"
        };
        let stage_action = crate::terminal::view::actions::TerminalAction::StageBranchFile {
            rel_path: rel_path_stage.clone(),
        };
        let stage_btn = Hoverable::new(stage_mouse.clone(), move |state| {
            let tint = if state.is_hovered() {
                theme.ansi_green()
            } else {
                theme.sub_text()
            };
            let glyph: Box<dyn Element> = if staged {
                Text::new("−".to_string(), ui, crate::theme::text_size(14.0))
                    .with_color(tint)
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish()
            } else {
                crate::icons::render_icon(AppIcon::Plus, tint, crate::theme::scaled(12.0))
            };
            let button = Container::new(glyph)
                .with_padding_left(crate::theme::scaled(3.0))
                .with_padding_right(crate::theme::scaled(3.0))
                .with_padding_top(crate::theme::scaled(2.0))
                .with_padding_bottom(crate::theme::scaled(2.0))
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                    crate::theme::scaled(5.0),
                )))
                .finish();
            buffer_search_tooltip(button, state.is_hovered(), stage_tooltip, theme, ui)
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| {
            if let Some(terminal_id) = terminal_id {
                ctx.dispatch_typed_action_for(terminal_id, stage_action.clone());
            } else {
                ctx.dispatch_typed_action_for(
                    workspace_view_id,
                    WorkspaceAction::StageGitFile {
                        rel_path: rel_path_stage.clone(),
                    },
                );
            }
        })
        .finish();
        let discard_action = crate::terminal::view::actions::TerminalAction::DiscardBranchFile {
            rel_path: rel_path_discard.clone(),
        };
        let discard_slot = ConstrainedBox::new(
            Align::new(
                Hoverable::new(discard_mouse.clone(), move |state| {
                    let button = Container::new(crate::icons::render_icon(
                        AppIcon::Undo2,
                        if state.is_hovered() || discard_armed {
                            theme.ansi_red()
                        } else {
                            theme.disabled_text()
                        },
                        crate::theme::scaled(11.5),
                    ))
                    .with_padding_left(crate::theme::scaled(3.0))
                    .with_padding_right(crate::theme::scaled(3.0))
                    .with_padding_top(crate::theme::scaled(2.0))
                    .with_padding_bottom(crate::theme::scaled(2.0))
                    .with_background_color(if state.is_hovered() || discard_armed {
                        theme.surface_2()
                    } else {
                        ColorU::new(0, 0, 0, 0)
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                        crate::theme::scaled(5.0),
                    )))
                    .finish();
                    buffer_search_tooltip(
                        button,
                        state.is_hovered(),
                        if discard_armed {
                            "Click again to confirm discard"
                        } else {
                            "Discard Changes"
                        },
                        theme,
                        ui,
                    )
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _app, _pos| {
                    if let Some(terminal_id) = terminal_id {
                        ctx.dispatch_typed_action_for(terminal_id, discard_action.clone());
                    } else {
                        ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::DiscardGitFile {
                                rel_path: rel_path_discard.clone(),
                            },
                        );
                    }
                })
                .finish(),
            )
            .right()
            .finish(),
        )
        .with_width(secondary_action_width)
        .finish();

        let action_group = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(crate::theme::scaled(1.0))
            .with_child(stage_btn)
            .with_child(discard_slot)
            .finish();

        let git_col = ConstrainedBox::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(crate::theme::scaled(1.5))
                .with_child(stats_group)
                .with_child(action_group)
                .finish(),
        )
        .with_width(trailing_group_width)
        .finish();

        let grid = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(status_col)
            .with_child(file_col)
            .with_child(git_col);

        ConstrainedBox::new(
            Container::new(grid.finish())
                .with_background_color(if state.is_hovered() {
                    theme.accent_overlay()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                // Insets belong to the row's content, not an outer wrapper. The
                // hover surface therefore paints wall-to-wall across the panel.
                .with_padding_left(crate::theme::scaled(8.0))
                .with_padding_right(crate::theme::scaled(16.0))
                .with_padding_top(crate::theme::scaled(5.0))
                .with_padding_bottom(crate::theme::scaled(5.0))
                .finish(),
        )
        .with_min_height(crate::theme::scaled(GIT_FILE_ROW_HEIGHT))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    // Stage/discard are nested hit targets. Let them consume the pointer event
    // so clicking an action never also opens the file diff from the row.
    .with_defer_events_to_children()
    .on_click(move |ctx, _app, _pos| {
        // `git status -unormal` can collapse an untracked tree into a directory
        // record. It has no file contents to compare.
        if rel_path_open.ends_with('/') {
            return;
        }
        if git_root_open.is_some() {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::OpenGitDiff {
                    rel_path: rel_path_open.clone(),
                    staged,
                },
            );
        }
    })
    .finish();

    row
}

pub(crate) fn render_git_history(
    history: &GitHistoryState,
    scroll: ClippedScrollStateHandle,
    workspace_view_id: EntityId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    let placeholder = |message: String, color: ColorU| {
        Container::new(
            Text::new(message, ui, crate::theme::text_size(11.5))
                .with_color(color)
                .finish(),
        )
        .with_padding_left(crate::theme::scaled(12.0))
        .with_padding_right(crate::theme::scaled(12.0))
        .with_padding_top(crate::theme::scaled(14.0))
        .finish()
    };

    let content: Box<dyn Element> = match history {
        GitHistoryState::Loading => placeholder("Loading history…".to_string(), theme.sub_text()),
        GitHistoryState::Error(error) => placeholder(error.clone(), theme.ansi_red()),
        GitHistoryState::Loaded(entries) if entries.is_empty() => {
            placeholder("No commits yet".to_string(), theme.sub_text())
        }
        GitHistoryState::Loaded(entries) => {
            let mut list = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min);
            for entry in entries {
                let mouse = mouse_states
                    .borrow_mut()
                    .entry(format!("git_history_{}", entry.sha))
                    .or_insert_with(MouseStateHandle::default)
                    .clone();
                let entry = entry.clone();
                let click_entry = entry.clone();
                let avatar_size = crate::theme::scaled(14.0);
                let border_width = crate::theme::scaled(1.0);
                let avatar = match &entry.avatar {
                    Some(GitCommitAvatar::GitHub(url)) => avatar_sized(
                        &entry.author,
                        &entry.author_email,
                        url,
                        theme,
                        ui,
                        avatar_size,
                    ),
                    Some(GitCommitAvatar::GitLab {
                        host,
                        project,
                        sha,
                        email,
                    }) => avatar_sized_from_source(
                        &entry.author,
                        &entry.author_email,
                        gitlab_avatar_source(host, project, sha, email),
                        theme,
                        ui,
                        avatar_size,
                    ),
                    None => avatar_sized(
                        &entry.author,
                        &entry.author_email,
                        "",
                        theme,
                        ui,
                        avatar_size,
                    ),
                };
                list = list.with_child(
                    Hoverable::new(mouse, move |state| {
                        Container::new(
                            Flex::column()
                                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                                .with_main_axis_size(MainAxisSize::Min)
                                .with_spacing(crate::theme::scaled(4.0))
                                .with_child(
                                    Text::new(
                                        entry.subject.clone(),
                                        ui,
                                        crate::theme::text_size(12.0),
                                    )
                                    .with_color(theme.main_text())
                                    .with_style(Properties::default().weight(Weight::Medium))
                                    .with_clip(ClipConfig::ellipsis())
                                    .soft_wrap(false)
                                    .finish(),
                                )
                                .with_child(
                                    Flex::row()
                                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                        .with_spacing(crate::theme::scaled(5.0))
                                        .with_child(
                                            // Avatar with border (matching the standard commit-avatar style)
                                            Container::new(avatar)
                                                .with_border(
                                                    Border::all(border_width)
                                                        .with_border_color(theme.outline()),
                                                )
                                                .with_corner_radius(CornerRadius::with_all(
                                                    Radius::Percentage(50.0),
                                                ))
                                                .finish(),
                                        )
                                        .with_child(
                                            Text::new(
                                                entry.author.clone(),
                                                ui,
                                                crate::theme::text_size(10.5),
                                            )
                                            .with_color(theme.sub_text())
                                            .with_clip(ClipConfig::ellipsis())
                                            .soft_wrap(false)
                                            .finish(),
                                        )
                                        .with_child(
                                            Text::new(
                                                entry.relative_time.clone(),
                                                ui,
                                                crate::theme::text_size(10.5),
                                            )
                                            .with_color(theme.disabled_text())
                                            .soft_wrap(false)
                                            .finish(),
                                        )
                                        .with_child(
                                            Text::new(
                                                "•".to_string(),
                                                ui,
                                                crate::theme::text_size(9.0),
                                            )
                                            .with_color(theme.disabled_text())
                                            .finish(),
                                        )
                                        .with_child(
                                            Expanded::new(1.0, Empty::new().finish()).finish(),
                                        )
                                        .with_child(
                                            Text::new(
                                                entry.short_sha.clone(),
                                                mono,
                                                crate::theme::text_size(10.25),
                                            )
                                            .with_color(theme.disabled_text())
                                            .soft_wrap(false)
                                            .finish(),
                                        )
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_padding_left(crate::theme::scaled(12.0))
                        .with_padding_right(crate::theme::scaled(10.0))
                        .with_padding_top(crate::theme::scaled(7.0))
                        .with_padding_bottom(crate::theme::scaled(7.0))
                        .with_background_color(if state.is_hovered() {
                            theme.surface_1()
                        } else {
                            ColorU::transparent_black()
                        })
                        .with_border(Border::all(1.0).with_border_color(if state.is_hovered() {
                            theme.border()
                        } else {
                            ColorU::transparent_black()
                        }))
                        .finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _app, _pos| {
                        ctx.dispatch_typed_action_for(
                            workspace_view_id,
                            WorkspaceAction::OpenGitCommitDiff {
                                sha: click_entry.sha.clone(),
                                short_sha: click_entry.short_sha.clone(),
                            },
                        );
                    })
                    .finish(),
                );
            }
            list.finish()
        }
    };

    Expanded::new(
        1.0,
        ClippedScrollable::vertical(
            scroll,
            content,
            ScrollbarWidth::Auto,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish(),
    )
    .finish()
}

pub(crate) fn render_git_group_header(
    title: &str,
    count: usize,
    theme: &Theme,
    ui: FamilyId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    terminal_id: Option<EntityId>,
    view_id: EntityId,
    workspace_view_id: EntityId,
    staged: bool,
    collapsed: bool,
    discard_all_armed: bool,
) -> Box<dyn Element> {
    let group_accent = if staged {
        theme.ansi_green()
    } else {
        theme.ui_yellow()
    };
    let header_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_group_{title}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let first_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_group_first_{title}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let second_mouse = mouse_states
        .borrow_mut()
        .entry(format!("git_group_second_{title}"))
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let title = title.to_string();
    // Section actions live at the far right with tooltips: staged gets
    // unstage-all plus discard-all, unstaged gets stage-all plus discard-all.
    // Destructive discards route through the confirm modal first — or, with
    // no terminal to host the modal, through a two-click arm in the panel.
    let (
        first_is_minus,
        first_tooltip,
        first_action,
        first_fallback,
        second_tooltip,
        second_action,
        second_fallback,
    ): (
        bool,
        &'static str,
        crate::terminal::view::actions::TerminalAction,
        WorkspaceAction,
        &'static str,
        crate::terminal::view::actions::TerminalAction,
        WorkspaceAction,
    ) = if staged {
        (
            true,
            "Unstage all",
            crate::terminal::view::actions::TerminalAction::UnstageAllBranchFiles,
            WorkspaceAction::UnstageAllGitFiles,
            "Discard all",
            crate::terminal::view::actions::TerminalAction::DiscardAllStagedChanges,
            WorkspaceAction::DiscardAllGitFiles { staged_only: true },
        )
    } else {
        (
            false,
            "Stage all",
            crate::terminal::view::actions::TerminalAction::StageAllBranchFiles,
            WorkspaceAction::StageAllGitFiles,
            "Discard all",
            crate::terminal::view::actions::TerminalAction::DiscardAllBranchChanges,
            WorkspaceAction::DiscardAllGitFiles { staged_only: false },
        )
    };
    let build_icon_button = |mouse: MouseStateHandle,
                             minus_glyph: bool,
                             use_plus: bool,
                             tooltip: &'static str,
                             color: ColorU,
                             terminal_action: crate::terminal::view::actions::TerminalAction,
                             fallback: WorkspaceAction,
                             armed: bool,
                             armed_tooltip: &'static str| {
        Hoverable::new(mouse, move |state| {
            let glyph: Box<dyn Element> = if minus_glyph {
                // AppIcon has no minus glyph; a tinted bar reads the same
                // and tints on hover like the icon buttons.
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(crate::theme::scaled(10.0))
                        .with_height(crate::theme::scaled(2.0))
                        .finish(),
                )
                .with_background_color(if state.is_hovered() {
                    color
                } else {
                    theme.sub_text()
                })
                .finish()
            } else if use_plus {
                render_icon(
                    AppIcon::Plus,
                    if state.is_hovered() {
                        color
                    } else {
                        theme.sub_text()
                    },
                    crate::theme::scaled(12.0),
                )
            } else {
                render_icon(
                    AppIcon::Undo2,
                    if state.is_hovered() || armed {
                        color
                    } else {
                        theme.sub_text()
                    },
                    crate::theme::scaled(12.0),
                )
            };
            let button = Container::new(glyph)
                .with_uniform_padding(crate::theme::scaled(4.0))
                .with_background_color(if state.is_hovered() || armed {
                    theme.surface_2()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                    crate::theme::scaled(4.0),
                )))
                .finish();
            buffer_search_tooltip(
                button,
                state.is_hovered(),
                if armed { armed_tooltip } else { tooltip },
                theme,
                ui,
            )
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| {
            if let Some(terminal_id) = terminal_id {
                ctx.dispatch_typed_action_for(terminal_id, terminal_action.clone());
            } else {
                ctx.dispatch_typed_action_for(workspace_view_id, fallback.clone());
            }
        })
        .finish()
    };
    Hoverable::new(header_mouse, move |header_state| {
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(crate::theme::scaled(5.0))
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
                Text::new(format!("{title}  {count}"), ui, crate::theme::text_size(12.25))
                    .with_color(group_accent)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(build_icon_button(
                first_mouse.clone(),
                first_is_minus,
                !first_is_minus,
                first_tooltip,
                theme.accent,
                first_action.clone(),
                first_fallback.clone(),
                false,
                "",
            ))
            .with_child(build_icon_button(
                second_mouse.clone(),
                false,
                false,
                second_tooltip,
                theme.ui_red(),
                second_action.clone(),
                second_fallback.clone(),
                discard_all_armed,
                "Click again to confirm discard",
            ))
            .finish();

        ConstrainedBox::new(
            Container::new(row)
                .with_background_color(if header_state.is_hovered() {
                    theme.surface_2()
                } else {
                    theme.surface_1()
                })
                .with_border(
                    Border::new(1.0)
                        .with_sides(true, false, true, false)
                        .with_border_color(theme.outline()),
                )
                .with_padding_left(12.0)
                // The scrollbar is overlaid, so do not inset the header's
                // background or separator from the scrollbar edge.
                .with_padding_right(crate::theme::scaled(8.0))
                .with_padding_top(crate::theme::scaled(7.0))
                .with_padding_bottom(crate::theme::scaled(6.0))
                .finish(),
        )
        .with_min_height(crate::theme::scaled(GIT_GROUP_HEADER_HEIGHT))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .with_defer_events_to_children()
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(view_id, SidebarAction::ToggleGitGroup { staged });
    })
    .finish()
}

/// Pinned group identity rendered above the scroll surface.
///
/// This intentionally does not use `Hoverable`. The original section header
/// still exists inside the clipped list, and mounting a second Hoverable with
/// the same semantic identity caused both elements to synthesize competing
/// MouseMoved transitions on every wheel tick. A plain click handler keeps the
/// sticky header collapsible without participating in hover-state tracking.
pub(crate) fn render_git_sticky_group_header(
    title: &str,
    count: usize,
    theme: &Theme,
    ui: FamilyId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    terminal_id: Option<EntityId>,
    view_id: EntityId,
    workspace_view_id: EntityId,
    staged: bool,
    collapsed: bool,
    discard_all_armed: bool,
) -> Box<dyn Element> {
    let group_accent = if staged {
        theme.ansi_green()
    } else {
        theme.ui_yellow()
    };
    // Unstaged sticky actions mirror the in-list header (unique hover keys
    // so the two never share hover identity). Always visible here: the
    // sticky header has no hover tracking of its own by design.
    let mut header_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(crate::theme::scaled(5.0))
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
            Text::new(format!("{title}  {count}"), ui, crate::theme::text_size(12.25))
                .with_color(group_accent)
                .with_style(Properties::default().weight(Weight::Semibold))
                .finish(),
        )
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    {
        let key_prefix = if staged {
            "git_sticky_staged"
        } else {
            "git_sticky_unstaged"
        };
        let sticky_action = |key: String,
                             minus_glyph: bool,
                             use_plus: bool,
                             tooltip: &'static str,
                             color: ColorU,
                             terminal_action: crate::terminal::view::actions::TerminalAction,
                             fallback: WorkspaceAction,
                             armed: bool| {
            let mouse = mouse_states
                .borrow_mut()
                .entry(key)
                .or_insert_with(MouseStateHandle::default)
                .clone();
            Hoverable::new(mouse, move |state| {
                let glyph: Box<dyn Element> = if minus_glyph {
                    Container::new(
                        ConstrainedBox::new(Empty::new().finish())
                            .with_width(crate::theme::scaled(10.0))
                            .with_height(crate::theme::scaled(2.0))
                            .finish(),
                    )
                    .with_background_color(if state.is_hovered() {
                        color
                    } else {
                        theme.sub_text()
                    })
                    .finish()
                } else if use_plus {
                    render_icon(
                        AppIcon::Plus,
                        if state.is_hovered() {
                            color
                        } else {
                            theme.sub_text()
                        },
                        crate::theme::scaled(12.0),
                    )
                } else {
                    render_icon(
                        AppIcon::Undo2,
                        if state.is_hovered() || armed {
                            color
                        } else {
                            theme.sub_text()
                        },
                        crate::theme::scaled(12.0),
                    )
                };
                let button = Container::new(glyph)
                    .with_uniform_padding(crate::theme::scaled(4.0))
                    .with_background_color(if state.is_hovered() || armed {
                        theme.surface_2()
                    } else {
                        ColorU::transparent_black()
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                        crate::theme::scaled(4.0),
                    )))
                    .finish();
                buffer_search_tooltip(
                    button,
                    state.is_hovered(),
                    if armed {
                        "Click again to confirm discard"
                    } else {
                        tooltip
                    },
                    theme,
                    ui,
                )
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _app, _pos| {
                if let Some(terminal_id) = terminal_id {
                    ctx.dispatch_typed_action_for(terminal_id, terminal_action.clone());
                } else {
                    ctx.dispatch_typed_action_for(workspace_view_id, fallback.clone());
                }
            })
            .finish()
        };
        if staged {
            header_row = header_row
                .with_child(sticky_action(
                    format!("{key_prefix}_unstage"),
                    true,
                    false,
                    "Unstage all",
                    theme.accent,
                    crate::terminal::view::actions::TerminalAction::UnstageAllBranchFiles,
                    WorkspaceAction::UnstageAllGitFiles,
                    false,
                ))
                .with_child(sticky_action(
                    format!("{key_prefix}_discard"),
                    false,
                    false,
                    "Discard all",
                    theme.ui_red(),
                    crate::terminal::view::actions::TerminalAction::DiscardAllStagedChanges,
                    WorkspaceAction::DiscardAllGitFiles { staged_only: true },
                    discard_all_armed,
                ));
        } else {
            header_row = header_row
                .with_child(sticky_action(
                    format!("{key_prefix}_stage"),
                    false,
                    true,
                    "Stage all",
                    theme.accent,
                    crate::terminal::view::actions::TerminalAction::StageAllBranchFiles,
                    WorkspaceAction::StageAllGitFiles,
                    false,
                ))
                .with_child(sticky_action(
                    format!("{key_prefix}_discard"),
                    false,
                    false,
                    "Discard all",
                    theme.ui_red(),
                    crate::terminal::view::actions::TerminalAction::DiscardAllBranchChanges,
                    WorkspaceAction::DiscardAllGitFiles { staged_only: false },
                    discard_all_armed,
                ));
        }
    }
    // As a Stack overlay this header is laid out against the WHOLE panel
    // height (finite), and the `Empty` spacer in the row sizes to its max
    // constraint — so without a fixed height the header inflated to cover
    // every file row, with the title centered in an opaque sheet. In the
    // scrolling list the max height is unbounded, which is why the in-flow
    // header never showed the problem. Pin the height instead of a minimum.
    //
    // The pinned height must still fit the content: 12px action buttons
    // with 8px vertical padding make the row 20 units tall, so 5 units of
    // padding top and bottom (30 total) leave breathing room. The previous
    // 7+6 padding needed 33 units inside a 31-unit pin and clipped the
    // (tallest) title text out while the smaller icons survived.
    let content = ConstrainedBox::new(
        Container::new(header_row.finish())
            .with_background_color(theme.surface_1())
            .with_border(
                Border::new(1.0)
                    .with_sides(true, false, true, false)
                    .with_border_color(theme.outline()),
            )
            .with_padding_left(12.0)
            // Keep the action buttons clear of the overlaid scrollbar.
            .with_padding_right(crate::theme::scaled(8.0))
            .with_padding_top(crate::theme::scaled(5.0))
            .with_padding_bottom(crate::theme::scaled(5.0))
            .finish(),
    )
    .with_height(crate::theme::scaled(30.0))
    .finish();

    EventHandler::new(content)
        .on_left_mouse_down(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action_for(view_id, SidebarAction::ToggleGitGroup { staged });
            DispatchEventResult::StopPropagation
        })
        .finish()
}
