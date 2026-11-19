//! Sidebar document breadcrumb, buffer search, and quick actions.
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

#[cfg(test)]
use super::sidebar_gitdiff_file::{git_intraline_ranges, pair_git_diff_lines};
use super::sidebar_gitdiff_multi::GitMultiViewportHeader;
#[cfg(test)]
use super::sidebar_gitdiff_multi::{
    git_multi_file_body_height, git_multi_preview_row_height, git_multi_projected_editor_height,
    git_multi_should_cull_file, git_multi_stable_body_height,
};
#[cfg(test)]
use super::sidebar_githistory::{parse_git_history, parse_git_hosting_remote, GitHostingRemote};
#[cfg(test)]
use super::sidebar_gitstatus::{git_file_display_identity, git_status_marker};
use super::sidebar_panels::{panel_section, render_quick_action_chip, section_title};
use super::view::source_line_changes;
use super::view::ProjectSidebarView;

#[cfg(test)]
use super::sidebar_explorer::project_search_sidebar_sticky_file;
use super::sidebar_explorer::{
    buffer_search_icon_button, buffer_search_option_button, render_language_icon,
};
#[cfg(test)]
use super::sidebar_search::project_search_body_height;
use super::sidebar_search::project_search_sticky_file;
#[cfg(test)]
use super::view::{
    is_markdown_path, GitCommitAvatar, MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT,
    MULTI_FILE_SEARCH_OVERSCAN_ROWS,
};

pub(crate) fn render_document_breadcrumb(
    model: &ProjectSidebarModel,
    relative_path: &Path,
    mono: FamilyId,
    theme: &Theme,
) -> Box<dyn Element> {
    if let Some(diff) = &model.git_multi_diff {
        return Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(crate::theme::scaled(7.0))
            .with_child(render_icon(
                AppIcon::GitBranch,
                theme.sub_text(),
                crate::theme::scaled(12.0),
            ))
            .with_child(
                Text::new(diff.title.clone(), mono, crate::theme::text_size(12.0))
                    .with_color(theme.main_text())
                    .with_clip(ClipConfig::ellipsis())
                    .soft_wrap(false)
                    .finish(),
            )
            .finish();
    }
    let mut segments = Vec::new();
    if let Some(root_name) = model
        .active_cwd
        .as_ref()
        .and_then(|cwd| detect::find_git_root(cwd))
        .and_then(|root| {
            root.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
    {
        segments.push(root_name);
    }
    segments.extend(
        relative_path
            .components()
            .filter_map(|component| component.as_os_str().to_str().map(str::to_owned)),
    );

    let file_name = segments.pop().unwrap_or_default();
    let parent = if segments.is_empty() {
        String::new()
    } else {
        format!("{} >", segments.join(" > "))
    };
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(4.0)
        .with_child(
            Shrinkable::new(
                1.0,
                Text::new(parent, mono, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish(),
        )
        .with_child(render_language_icon(relative_path, theme))
        .with_child(
            Shrinkable::new(
                1.0,
                Text::new(file_name, mono, crate::theme::text_size(12.0))
                    .with_color(theme.main_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish(),
        )
        .finish()
}

pub(crate) fn render_buffer_search_bar(
    view: &ProjectSidebarView,
    theme: &Theme,
    ui: FamilyId,
    mono: FamilyId,
) -> Box<dyn Element> {
    let count = view.buffer_search_matches.len();
    let current = view.buffer_search_active.map_or(0, |index| index + 1);
    let status = if view.buffer_search_error.is_some() {
        "Invalid pattern".to_string()
    } else {
        format!("{current}/{count}")
    };
    let input = crate::shared_input::shared_input(
        theme,
        &view.buffer_search_editor,
        crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 6.0,
            background: Some(theme.background),
            border: Some(if view.buffer_search_error.is_some() {
                theme.ui_red()
            } else {
                theme.outline()
            }),
            pad_left: 10.0,
            pad_right: 8.0,
            pad_top: 5.0,
            pad_bottom: 5.0,
            ..Default::default()
        },
    );

    // Measure the controls at their natural width before giving the input
    // the remaining space. Larger match counts must never wrap or overlap.
    let search_controls = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::End)
        .with_spacing(2.0)
        .with_child(buffer_search_option_button(
            "Aa",
            "Match case",
            "buffer_search_case",
            WorkspaceAction::ToggleBufferSearchCaseSensitive,
            view.buffer_search_options.case_sensitive,
            theme,
            ui,
            view,
        ))
        .with_child(buffer_search_option_button(
            "W",
            "Match whole word",
            "buffer_search_word",
            WorkspaceAction::ToggleBufferSearchWholeWord,
            view.buffer_search_options.whole_word,
            theme,
            ui,
            view,
        ))
        .with_child(buffer_search_option_button(
            ".*",
            "Use regular expression",
            "buffer_search_regex",
            WorkspaceAction::ToggleBufferSearchRegex,
            view.buffer_search_options.regex,
            theme,
            mono,
            view,
        ))
        .with_child(
            ConstrainedBox::new(
                Align::new(
                    Text::new(status, mono, crate::theme::text_size(10.5))
                        .soft_wrap(false)
                        .with_color(if view.buffer_search_error.is_some() {
                            theme.ui_red()
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_min_width(45.0)
            .finish(),
        )
        .with_child(buffer_search_icon_button(
            AppIcon::ArrowUp,
            "Previous match (Shift+Enter)",
            "buffer_search_previous",
            WorkspaceAction::BufferSearchPrevious,
            theme,
            view,
        ))
        .with_child(buffer_search_icon_button(
            AppIcon::ArrowDown,
            "Next match (Enter)",
            "buffer_search_next",
            WorkspaceAction::BufferSearchNext,
            theme,
            view,
        ))
        .with_child(buffer_search_icon_button(
            AppIcon::XClose,
            "Close search (Escape)",
            "buffer_search_close",
            WorkspaceAction::CloseBufferSearch,
            theme,
            view,
        ))
        .finish();

    let search_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(5.0)
        .with_child(buffer_search_icon_button(
            if view.buffer_replace_open {
                AppIcon::ChevronDown
            } else {
                AppIcon::ChevronRight
            },
            if view.buffer_replace_open {
                "Hide replace"
            } else {
                "Show replace"
            },
            "buffer_search_replace_toggle",
            WorkspaceAction::ToggleBufferSearchReplace,
            theme,
            view,
        ))
        .with_child(Expanded::new(1.0, input).finish())
        .with_child(search_controls)
        .finish();

    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(search_row);
    if view.buffer_replace_open {
        let replace_input = crate::shared_input::shared_input(
            theme,
            &view.buffer_replace_editor,
            crate::shared_input::SharedInputOpts {
                ..Default::default()
            },
        );
        rows = rows.with_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                // Align the replacement field with the search field after the
                // disclosure button above.
                .with_child(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(26.0)
                        .finish(),
                )
                .with_child(Expanded::new(1.0, replace_input).finish())
                .with_child(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Min)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_main_axis_alignment(MainAxisAlignment::Start)
                        .with_spacing(2.0)
                        .with_child(buffer_search_icon_button(
                            AppIcon::Replace,
                            "Replace current match (Enter)",
                            "buffer_search_replace_current",
                            WorkspaceAction::BufferSearchReplaceCurrent,
                            theme,
                            view,
                        ))
                        .with_child(buffer_search_icon_button(
                            AppIcon::ReplaceAll,
                            "Replace all matches",
                            "buffer_search_replace_all",
                            WorkspaceAction::BufferSearchReplaceAll,
                            theme,
                            view,
                        ))
                        .finish(),
                )
                .finish(),
        );
    }

    Container::new(rows.finish())
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(theme.panel_background())
        .with_border(Border::bottom(1.0).with_border_color(theme.border()))
        .finish()
}

// ── sections ─────────────────────────────────────────────────────────────────

pub(crate) fn render_quick_actions_section(
    actions: &[QuickAction],
    workspace_view_id: EntityId,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    view: &ProjectSidebarView,
) -> Box<dyn Element> {
    let mut items: Vec<Box<dyn Element>> = Vec::new();
    for action in actions {
        items.push(render_quick_action_chip(
            action,
            workspace_view_id,
            ui,
            mono,
            theme,
            view,
        ));
    }

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(4.0);
    for item in items {
        col = col.with_child(item);
    }

    panel_section(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(section_title("Quick Actions", ui, theme))
            .with_child(col.finish())
            .finish(),
        Some(theme.border()),
    )
}

#[cfg(test)]
mod line_change_tests {
    use super::*;

    #[test]
    fn derives_added_modified_and_deleted_gutter_markers() {
        let added = dragent_document::DocumentDiff::between("one\n", "one\ntwo\n");
        assert_eq!(
            source_line_changes(&added, None, false, 2).get(&2),
            Some(&LineChangeKind::Added)
        );

        let modified = dragent_document::DocumentDiff::between("one\ntwo\n", "one\nchanged\n");
        assert_eq!(
            source_line_changes(&modified, None, false, 2).get(&2),
            Some(&LineChangeKind::Modified)
        );

        let deleted = dragent_document::DocumentDiff::between("one\ntwo\nthree\n", "one\nthree\n");
        assert!(source_line_changes(&deleted, None, false, 2)
            .values()
            .any(|kind| *kind == LineChangeKind::Deleted));
    }

    #[test]
    fn split_diff_pairs_replacements_and_preserves_context() {
        use dragent_document::{DiffHunk, DiffLine, DiffLineKind};

        let hunk = DiffHunk {
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(1),
                    new_line: Some(1),
                    text: "same".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    old_line: Some(2),
                    new_line: None,
                    text: "old".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(2),
                    text: "new".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    old_line: None,
                    new_line: Some(3),
                    text: "extra".to_string(),
                },
            ],
            ..DiffHunk::default()
        };
        let rows = pair_git_diff_lines(&hunk);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0.unwrap().text, "same");
        assert_eq!(rows[0].1.unwrap().text, "same");
        assert_eq!(rows[1].0.unwrap().text, "old");
        assert_eq!(rows[1].1.unwrap().text, "new");
        assert!(rows[2].0.is_none());
        assert_eq!(rows[2].1.unwrap().text, "extra");
    }

    #[test]
    fn intraline_ranges_preserve_utf8_boundaries() {
        let (old, new) = git_intraline_ranges("let café = old;", "let café = new;").unwrap();
        assert_eq!(&"let café = old;"[old], "old");
        assert_eq!(&"let café = new;"[new], "new");
    }

    #[test]
    fn recognizes_markdown_preview_extensions() {
        for path in ["README.md", "guide.markdown", "notes.mdown", "doc.MKD"] {
            assert!(is_markdown_path(Path::new(path)), "{path}");
        }
        assert!(!is_markdown_path(Path::new("component.mdx")));
        assert!(!is_markdown_path(Path::new("README.txt")));
    }

    #[test]
    fn git_file_identity_stacks_repo_relative_name_and_parent() {
        assert_eq!(
            git_file_display_identity("backend/src/config/typeorm-data-source.ts"),
            (
                "typeorm-data-source.ts".to_string(),
                "backend/src/config".to_string(),
                "backend/src/config/typeorm-data-source.ts".to_string(),
            )
        );
        assert_eq!(
            git_file_display_identity("../backend/src/http/http-reply.util.ts"),
            (
                "http-reply.util.ts".to_string(),
                "backend/src/http".to_string(),
                "backend/src/http/http-reply.util.ts".to_string(),
            )
        );
    }

    #[test]
    fn aggregate_diff_editor_height_tracks_projected_rows_without_eighty_line_clip() {
        let line_height = crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE * 1.618);
        let short = git_multi_projected_editor_height(6);
        let taller = git_multi_projected_editor_height(12);

        // Every projected row contributes its line height to the card
        // reservation; embedded diff editors add no standalone-editor tail.
        assert!((taller - short - 6.0 * line_height).abs() < 0.01);
        assert!((short - 6.0 * line_height).abs() < 0.01);

        // A file longer than the former ~82-line/2,000px viewport must keep
        // growing in the aggregate document instead of becoming an internally
        // clipped editor followed by blank card space.
        let long = git_multi_projected_editor_height(200);
        assert!(long > 2_000.0);

        // Truly extreme projections retain a finite safety ceiling.
        assert_eq!(
            git_multi_projected_editor_height(10_000),
            MULTI_DIFF_INLINE_EDITOR_MAX_HEIGHT
        );
    }

    #[test]
    fn aggregate_diff_keeps_active_and_following_file_mounted() {
        assert!(!git_multi_should_cull_file(3, Some(3), true));
        assert!(!git_multi_should_cull_file(4, Some(3), true));
        assert!(git_multi_should_cull_file(5, Some(3), true));
        assert!(git_multi_should_cull_file(3, None, true));
        assert!(!git_multi_should_cull_file(9, Some(3), false));
    }

    #[test]
    fn inactive_multi_diff_card_reserves_preview_height_only() {
        use crate::git_status::diff_head::GitMultiDiffFile;
        use crate::git_status::diff_head::{GitChangedFile, GitDiffTarget, GitFileDiff};

        let old = "one\ntwo\nthree\n";
        let new = "one\nTWO\nthree\n";
        let diff = dragent_document::DocumentDiff::between(old, new);
        let line_count: usize = diff.hunks.iter().map(|hunk| hunk.lines.len()).sum();
        assert!(line_count > 0);
        let file = GitMultiDiffFile {
            change: GitChangedFile {
                relative_path: PathBuf::from("a.rs"),
                base_relative_path: None,
                target: GitDiffTarget::Unstaged,
                status: 'M',
            },
            diff: Ok(GitFileDiff {
                relative_path: PathBuf::from("a.rs"),
                base_relative_path: PathBuf::from("a.rs"),
                target: GitDiffTarget::Unstaged,
                old_text: old.to_string(),
                new_text: new.to_string(),
                diff: diff.clone(),
                full_diff: diff,
                old_highlight_states: Vec::new(),
                new_highlight_states: Vec::new(),
                old_syntax: None,
                new_syntax: None,
                additions: 1,
                deletions: 1,
                max_changed_line_chars: 3,
                binary: false,
            }),
        };
        let collapsed = std::collections::HashSet::new();

        // An inactive card paints exactly its hunk rows: one hunk header plus
        // one exact-height row per rendered line.
        let expected =
            crate::theme::scaled(21.0) + line_count as f32 * git_multi_preview_row_height();
        assert!((git_multi_file_body_height(&file, &collapsed) - expected).abs() < 0.01);

        // …and the reservation matches the paint, not the full editor
        // projection (which would leave blank space and corrupt the
        // virtualization offsets of every later file).
        assert_eq!(
            git_multi_stable_body_height(&file, &collapsed, None),
            git_multi_file_body_height(&file, &collapsed)
        );

        // An active card reserves only its projected editor rows. It does not
        // inherit the standalone editor's bottom overscroll.
        let active = git_multi_stable_body_height(&file, &collapsed, Some(line_count));
        let expected_active = line_count as f32
            * crate::theme::scaled(crate::workspace::SOURCE_EDITOR_FONT_SIZE * 1.618);
        assert!((active - expected_active).abs() < 0.01);
    }

    #[test]
    fn git_status_marker_preserves_untracked_state() {
        let untracked = crate::console::git_status::GitStatusEntry {
            rel_path: "new.ts".to_string(),
            index: '?',
            work: '?',
        };
        assert_eq!(git_status_marker(&untracked, false), "??");

        let modified = crate::console::git_status::GitStatusEntry {
            rel_path: "changed.ts".to_string(),
            index: ' ',
            work: 'M',
        };
        assert_eq!(git_status_marker(&modified, false), "M");
    }

    #[test]
    fn git_history_parser_preserves_subject_spaces() {
        let entries = parse_git_history(
            "0123456789abcdef\u{1f}0123456\u{1f}Ada Lovelace\u{1f}ada@example.com\u{1f}2 hours ago\u{1f}Fix source control history view",
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sha, "0123456789abcdef");
        assert_eq!(entries[0].short_sha, "0123456");
        assert_eq!(entries[0].author, "Ada Lovelace");
        assert_eq!(entries[0].author_email, "ada@example.com");
        assert_eq!(entries[0].relative_time, "2 hours ago");
        assert_eq!(entries[0].subject, "Fix source control history view");
    }

    #[test]
    fn parses_supported_git_hosting_remotes() {
        assert_eq!(
            parse_git_hosting_remote("git@github.com:team/project.git"),
            Some(GitHostingRemote::GitHub)
        );
        assert_eq!(
            parse_git_hosting_remote("https://gitlab.com/team/project.git"),
            Some(GitHostingRemote::GitLab {
                host: "gitlab.com".to_string(),
                project: "team/project".to_string(),
            })
        );
        assert_eq!(
            parse_git_hosting_remote("ssh://git@code.example.com/team/project.git"),
            None
        );
    }

    #[test]
    fn github_avatar_uses_normalized_encoded_email() {
        let mut entry = parse_git_history(
            "0123456789abcdef\u{1f}0123456\u{1f}Ada\u{1f}<Ada+Git@Example.COM>\u{1f}now\u{1f}Commit",
        )
        .remove(0);
        entry.avatar = GitHostingRemote::GitHub.avatar_for(&entry);
        assert_eq!(
            entry.avatar,
            Some(GitCommitAvatar::GitHub(
                "https://avatars.githubusercontent.com/u/e?email=ada%2Bgit%40example.com&s=128"
                    .to_string()
            ))
        );
    }

    #[test]
    fn search_result_card_height_collapses_only_its_body() {
        let file = crate::workspace::search_panel::SearchFileResult {
            relative_path: PathBuf::from("src/main.rs"),
            hits: (0..3)
                .map(|line| crate::workspace::search_panel::SearchHit {
                    line: line + 1,
                    col: 1,
                    line_text: "match".to_string(),
                    match_range: Some((0, 5)),
                })
                .collect(),
        };
        assert_eq!(project_search_body_height(&file, true), 0.0);
        assert_eq!(
            project_search_body_height(&file, false),
            3.0 * crate::theme::scaled(24.0)
        );
    }

    #[test]
    fn multi_file_search_materializes_only_viewport_rows() {
        let range = editor_render::viewport_row_range(
            100_000,
            50_000.0 * crate::theme::scaled(24.0),
            720.0,
            crate::theme::scaled(24.0),
            MULTI_FILE_SEARCH_OVERSCAN_ROWS,
        );

        assert!(range.start > 49_000);
        assert!(range.end < 51_000);
        assert!(range.len() < 64);
    }

    #[test]
    fn search_sticky_header_advances_at_file_boundaries() {
        let file = |path: &str| crate::workspace::search_panel::SearchFileResult {
            relative_path: PathBuf::from(path),
            hits: vec![crate::workspace::search_panel::SearchHit {
                line: 1,
                col: 1,
                line_text: "match".to_string(),
                match_range: Some((0, 5)),
            }],
        };
        let results = crate::workspace::search_panel::SearchResults {
            query: "match".to_string(),
            files: vec![file("src/a.rs"), file("src/b.rs")],
            total_hits: 2,
            max_line_chars: 5,
            truncated: false,
            error: None,
        };
        let collapsed = std::collections::HashSet::new();
        assert!(project_search_sticky_file(&results, &collapsed, 0.0).is_none());
        assert_eq!(
            project_search_sticky_file(
                &results,
                &collapsed,
                crate::theme::scaled(64.0)
                    + crate::theme::scaled(43.0)
                    + crate::theme::scaled(24.0)
            )
            .map(|file| file.relative_path.as_path()),
            Some(Path::new("src/b.rs"))
        );
    }

    #[test]
    fn sidebar_search_sticky_header_tracks_the_scrolled_file() {
        let file = |path: &str, hits: usize| crate::workspace::search_panel::SearchFileResult {
            relative_path: PathBuf::from(path),
            hits: (0..hits)
                .map(|line| crate::workspace::search_panel::SearchHit {
                    line: line + 1,
                    col: 1,
                    line_text: "match".to_string(),
                    match_range: Some((0, 5)),
                })
                .collect(),
        };
        let results = crate::workspace::search_panel::SearchResults {
            query: "match".to_string(),
            files: vec![file("src/a.rs", 2), file("src/b.rs", 1)],
            total_hits: 3,
            max_line_chars: 5,
            truncated: false,
            error: None,
        };
        let collapsed = std::collections::HashSet::new();
        assert!(project_search_sidebar_sticky_file(&results, &collapsed, 0.0).is_none());
        assert_eq!(
            project_search_sidebar_sticky_file(&results, &collapsed, 1.0)
                .map(|file| file.relative_path.as_path()),
            Some(Path::new("src/a.rs"))
        );
        assert_eq!(
            project_search_sidebar_sticky_file(&results, &collapsed, 28.0 + 2.0 * 24.0)
                .map(|file| file.relative_path.as_path()),
            Some(Path::new("src/b.rs"))
        );
    }
}
