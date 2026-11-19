//! Sidebar review panel.
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

use super::sidebar_githistory::render_sidebar_tab_header;
use super::sidebar_panels::{
    render_errors_section, render_language_diagnostics_section, render_project_section,
    section_title_inline,
};
use super::view::{ProjectSidebarView, SidebarAction};

/// Files | Source Control tab strip at the top of the right sidebar. The
/// sidebar is one dock hosting two workspaces; this is the switcher.
/// Review surface: the linked pull request's
/// status, checks, and comments, fed by the workspace's `gh` poll. States
/// mirror the reference — loading, error, empty ("Open a pull request…"),
/// then PR header / checks / comments sections.
pub(crate) fn render_review_panel(
    view: &ProjectSidebarView,
    ui: FamilyId,
    theme: &Theme,
    root: Option<PathBuf>,
) -> Box<dyn Element> {
    let Some(pr) = view.review_pr.pr.as_ref() else {
        return render_review_empty(view, ui, theme);
    };
    // Borrow everything: this renders on every notify and the PR payload is
    // re-cloned nowhere. Only short-lived labels allocate below.
    let pr_number = pr.number;
    let comments = &view.review_pr.comments;
    let error = view.review_pr.error.clone();
    let loading_comments = view.review_pr.loading_comments;
    let action_error = view.review_action_error.clone();
    let checks_open = view.review_checks_open;
    let comments_open = view.review_comments_open;
    let resolved_open = view.review_resolved_open;
    let workspace_view_id = view.workspace_view_id;

    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0)
        .with_child(review_pr_header_view(
            ui,
            theme,
            workspace_view_id,
            &pr.title,
            &pr.url,
            &pr.state,
            &pr.review_decision,
            &pr.base_branch,
            &pr.head_branch,
            pr_number,
            review_mouse(&view.sidebar_tab_mouse_states, "review_title".to_string()),
        ))
        .with_child(review_divider(theme));

    let relevant: Vec<&super::ReviewCheck> = pr
        .checks
        .iter()
        .filter(|check| !matches!(check.status, super::ReviewCheckStatus::Skipped))
        .collect();
    let passing = relevant
        .iter()
        .filter(|check| matches!(check.status, super::ReviewCheckStatus::Success))
        .count();
    let checks_summary = if relevant.is_empty() {
        "No checks reported".to_string()
    } else {
        format!("{passing}/{} passing", relevant.len())
    };
    column = column.with_child(review_section_view(
        ui,
        theme,
        "Checks",
        relevant.len().to_string(),
        Some((checks_summary, checks_status_color(theme, &pr.checks))),
        checks_open,
        review_mouse(&view.sidebar_tab_mouse_states, "review_checks".to_string()),
        SidebarAction::ToggleReviewChecks,
        None,
        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(1.0)
                .with_child(review_check_rows(ui, theme, &pr.checks))
                .finish(),
        ),
    ));
    column = column.with_child(review_divider(theme));

    let active: Vec<&super::ReviewComment> = comments
        .iter()
        .filter(|comment| !comment.is_resolved)
        .collect();
    let resolved: Vec<&super::ReviewComment> = comments
        .iter()
        .filter(|comment| comment.is_resolved)
        .collect();
    let comments_label = if loading_comments {
        "...".to_string()
    } else {
        comments.len().to_string()
    };
    // Single count display (the label doubles as the "..." loading state);
    // Copy All trails the header instead of sitting in the body.
    let comments_trailing = if active.is_empty() {
        None
    } else {
        Some(review_copy_all_button(
            ui,
            theme,
            &view.sidebar_tab_mouse_states,
            view.review_copy_all_copied,
        ))
    };
    let comments_body: Box<dyn Element> = if active.is_empty() {
        Container::new(
            Text::new(
                "No comments yet.".to_string(),
                ui,
                crate::theme::text_size(11.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .with_padding_left(22.0)
        .with_padding_top(2.0)
        .with_padding_bottom(4.0)
        .finish()
    } else {
        review_comment_rows(
            ui,
            theme,
            workspace_view_id,
            &view.sidebar_tab_mouse_states,
            &active,
            view.review_copied_comment.as_deref(),
            view.review_resolving_thread.as_deref(),
            root.clone(),
        )
    };
    column = column.with_child(review_section_view(
        ui,
        theme,
        "Comments",
        comments_label,
        None,
        comments_open,
        review_mouse(
            &view.sidebar_tab_mouse_states,
            "review_comments".to_string(),
        ),
        SidebarAction::ToggleReviewComments,
        comments_trailing,
        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(1.0)
                .with_child(comments_body)
                .finish(),
        ),
    ));
    if !resolved.is_empty() {
        column = column.with_child(review_section_view(
            ui,
            theme,
            "Resolved",
            resolved.len().to_string(),
            None,
            resolved_open,
            review_mouse(
                &view.sidebar_tab_mouse_states,
                "review_resolved".to_string(),
            ),
            SidebarAction::ToggleReviewResolved,
            None,
            Some(review_comment_rows(
                ui,
                theme,
                workspace_view_id,
                &view.sidebar_tab_mouse_states,
                &resolved,
                view.review_copied_comment.as_deref(),
                view.review_resolving_thread.as_deref(),
                root.clone(),
            )),
        ));
    }
    if let Some(message) = error.or(action_error) {
        column = column.with_child(
            Container::new(
                Text::new(message, ui, crate::theme::text_size(10.5))
                    .with_color(theme.ui_red())
                    .soft_wrap(true)
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish(),
        );
    }

    Container::new(column.finish())
        .with_padding_top(4.0)
        .with_padding_bottom(12.0)
        .finish()
}

/// Centered empty/error state. The copy is split into two balanced lines
/// because the Text element exposes no text-align.
fn render_review_empty(view: &ProjectSidebarView, ui: FamilyId, theme: &Theme) -> Box<dyn Element> {
    let body: Box<dyn Element> = if view.review_pr.non_git_folder {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(0.0)
            .with_child(
                Text::new(
                    "Review requires a Git repository.".to_string(),
                    ui,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(
                Text::new(
                    "Initialize Git to review changes.".to_string(),
                    ui,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .finish()
    } else if view.review_pr.error.is_some() {
        Text::new(
            "Unable to load review status".to_string(),
            ui,
            crate::theme::text_size(12.0),
        )
        .with_color(theme.ui_red())
        .finish()
    } else if view.review_pr.loading_pr {
        Text::new(
            "Loading review...".to_string(),
            ui,
            crate::theme::text_size(12.0),
        )
        .with_color(theme.sub_text())
        .finish()
    } else {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(0.0)
            .with_child(
                Text::new(
                    "Open a pull request to view".to_string(),
                    ui,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(
                Text::new(
                    "review status, checks, and comments.".to_string(),
                    ui,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .finish()
    };
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(render_icon(
                AppIcon::PullRequest,
                theme.disabled_text(),
                28.0,
            ))
            .with_child(body)
            .finish(),
    )
    .with_padding_left(20.0)
    .with_padding_right(20.0)
    .with_padding_top(12.0)
    .with_padding_bottom(12.0)
    .finish()
}

/// 1px horizontal divider between review sections.
fn review_divider(theme: &Theme) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(1.0)
            .finish(),
    )
    .with_background_color(theme.outline())
    .with_margin_top(4.0)
    .with_margin_bottom(4.0)
    .finish()
}

/// Rollup color for the checks summary:
/// failure wins over pending wins over success.
fn checks_status_color(theme: &Theme, checks: &[super::ReviewCheck]) -> ColorU {
    use super::ReviewCheckStatus;
    let mut relevant = checks
        .iter()
        .filter(|check| !matches!(check.status, ReviewCheckStatus::Skipped));
    if relevant.clone().any(|check| {
        matches!(
            check.status,
            ReviewCheckStatus::Failure | ReviewCheckStatus::Cancelled
        )
    }) {
        return theme.ui_red();
    }
    if relevant
        .clone()
        .any(|check| matches!(check.status, ReviewCheckStatus::Pending))
    {
        return theme.ui_yellow();
    }
    if relevant.any(|check| matches!(check.status, ReviewCheckStatus::Success)) {
        return theme.ui_green();
    }
    theme.sub_text()
}

fn review_check_icon(status: super::ReviewCheckStatus) -> AppIcon {
    use super::ReviewCheckStatus;
    match status {
        ReviewCheckStatus::Success => AppIcon::Check,
        ReviewCheckStatus::Failure | ReviewCheckStatus::Cancelled => AppIcon::XClose,
        ReviewCheckStatus::Pending => AppIcon::Clock,
        ReviewCheckStatus::Skipped | ReviewCheckStatus::Unknown => AppIcon::DotsVertical,
    }
}

fn review_check_color(theme: &Theme, status: super::ReviewCheckStatus) -> ColorU {
    use super::ReviewCheckStatus;
    match status {
        ReviewCheckStatus::Success => theme.ui_green(),
        ReviewCheckStatus::Failure | ReviewCheckStatus::Cancelled => theme.ui_red(),
        ReviewCheckStatus::Pending => theme.ui_yellow(),
        ReviewCheckStatus::Skipped | ReviewCheckStatus::Unknown => theme.disabled_text(),
    }
}

pub(crate) fn review_comment_clipboard_text(comment: &super::ReviewComment) -> String {
    let mut text = format!("{}:\n{}", comment.author, comment.body);
    if let Some(url) = comment.url.as_deref() {
        text.push('\n');
        text.push_str(url);
    }
    text
}

/// Compact relative age ("5m", "2h", "3d") for comment timestamps.
fn review_short_age(created_at: &str) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return String::new();
    };
    let seconds = chrono::Utc::now()
        .signed_duration_since(time.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

fn review_mouse(
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    key: String,
) -> MouseStateHandle {
    states
        .borrow_mut()
        .entry(key)
        .or_insert_with(MouseStateHandle::default)
        .clone()
}

/// PR header: state icon + title (opens the PR on the pull requests page),
/// decision pill,
/// and base/head branches.
#[allow(clippy::too_many_arguments)]
fn review_pr_header_view(
    ui: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    title: &str,
    url: &str,
    state: &str,
    decision: &str,
    base: &str,
    head: &str,
    number: u64,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let state_color = match state {
        "merged" => theme.accent,
        "closed" => theme.ui_red(),
        _ => theme.ui_green(),
    };
    let open_url = url.to_string();
    let title_text = if title.is_empty() {
        format!("Pull request #{number}")
    } else {
        title.to_string()
    };
    let title_row = Hoverable::new(mouse, move |hovered| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::PullRequest, state_color, 14.0))
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(title_text.clone(), ui, crate::theme::text_size(12.0))
                            .with_color(theme.main_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(if hovered.is_hovered() {
            theme.sidebar_row_hover()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenPullRequest {
                url: open_url.clone(),
            },
        );
    })
    .finish();

    let (decision_label, decision_color) = match decision {
        "approved" => ("Approved", theme.ui_green()),
        "changes_requested" => ("Changes requested", theme.ui_red()),
        _ => ("Review pending", theme.ui_yellow()),
    };
    let branches = if base.is_empty() || head.is_empty() {
        format!("#{number}")
    } else {
        format!("#{number} · {head} → {base}")
    };
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(title_row)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(
                        Container::new(
                            Text::new(decision_label.to_string(), ui, crate::theme::text_size(10.0))
                                .with_color(decision_color)
                                .finish(),
                        )
                        .with_border(Border::all(1.0).with_border_color(decision_color))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(2.0)
                        .with_padding_bottom(2.0)
                        .finish(),
                    )
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(branches, ui, crate::theme::text_size(10.0))
                                .with_color(theme.sub_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(8.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish()
}

/// Collapsible review section header: chevron,
/// label, count, and an optional right-aligned summary.
#[allow(clippy::too_many_arguments)]
fn review_section_view(
    ui: FamilyId,
    theme: &Theme,
    label: &'static str,
    count_label: String,
    summary: Option<(String, ColorU)>,
    open: bool,
    mouse: MouseStateHandle,
    toggle: SidebarAction,
    trailing: Option<Box<dyn Element>>,
    body: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(render_icon(
            if open {
                AppIcon::ChevronDown
            } else {
                AppIcon::ChevronRight
            },
            theme.disabled_text(),
            10.0,
        ))
        .with_child(
            Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(count_label, ui, crate::theme::text_size(10.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    if let Some((summary, color)) = summary {
        // The summary itself expands (right-aligned): a spacer plus a bare
        // text would both report full width and force the panel wider at
        // narrow sizes.
        header = header.with_child(
            Expanded::new(
                1.0,
                Align::new(
                    Text::new(summary, ui, crate::theme::text_size(10.0))
                        .with_color(color)
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .right()
                .finish(),
            )
            .finish(),
        );
    }
    // The toggle area fills the row; an optional trailing element (e.g. the
    // Copy All button) sits outside it so its click never toggles the section.
    let header_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Expanded::new(
                1.0,
                Hoverable::new(mouse, move |hovered| {
                    Container::new(header.finish())
                        .with_background_color(if hovered.is_hovered() {
                            theme.sidebar_row_hover()
                        } else {
                            ColorU::transparent_black()
                        })
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(8.0)
                        .with_padding_top(6.0)
                        .with_padding_bottom(6.0)
                        .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(toggle.clone());
                })
                .finish(),
            )
            .finish(),
        )
        .with_child(trailing.unwrap_or_else(|| Empty::new().finish()))
        .finish();

    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0)
        .with_child(header_row);
    if open {
        if let Some(body) = body {
            column = column.with_child(body);
        }
    }
    column.finish()
}

/// One CI check row: status icon, name, duration.
fn review_check_rows(
    ui: FamilyId,
    theme: &Theme,
    checks: &[super::ReviewCheck],
) -> Box<dyn Element> {
    if checks.is_empty() {
        return Container::new(
            Text::new(
                "No checks reported.".to_string(),
                ui,
                crate::theme::text_size(11.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
        .with_padding_left(22.0)
        .with_padding_top(2.0)
        .with_padding_bottom(4.0)
        .finish();
    }
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(1.0);
    for check in checks {
        let status = check.status;
        let name = check.name.clone();
        let duration = check.duration_text.clone().unwrap_or_default();
        column = column.with_child(
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(
                        review_check_icon(status),
                        review_check_color(theme, status),
                        11.0,
                    ))
                    .with_child(
                        Expanded::new(
                            1.0,
                            Text::new(name, ui, crate::theme::text_size(11.5))
                                .with_color(theme.main_text())
                                .soft_wrap(false)
                                .with_clip(ClipConfig::ellipsis())
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(
                        Text::new(duration, ui, crate::theme::text_size(10.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(22.0)
            .with_padding_right(8.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .finish(),
        );
    }
    column.finish()
}

/// "Copy all" button trailing the Comments section header. Shows a checkmark
/// briefly after a successful copy.
fn review_copy_all_button(
    ui: FamilyId,
    theme: &Theme,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    copied: bool,
) -> Box<dyn Element> {
    let mouse = review_mouse(states, "review_copy_all".to_string());
    let (icon, color) = if copied {
        (AppIcon::Check, theme.ui_green())
    } else {
        (AppIcon::Copy, theme.sub_text())
    };
    Hoverable::new(mouse, move |hovered| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(
                    icon,
                    if copied || hovered.is_hovered() {
                        color
                    } else {
                        theme.sub_text()
                    },
                    11.0,
                ))
                .with_child(
                    Text::new("Copy all".to_string(), ui, crate::theme::text_size(11.0))
                        .with_color(if hovered.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        })
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(4.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(SidebarAction::CopyAllReviewComments);
    })
    .finish()
}

/// Comment rows: author + kind + age, body preview, path, and Done / Copy /
/// Comment actions are always visible (no hover tracking per row).
#[allow(clippy::too_many_arguments)]
fn review_comment_rows(
    ui: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    comments: &[&super::ReviewComment],
    copied_id: Option<&str>,
    resolving_id: Option<&str>,
    root: Option<PathBuf>,
) -> Box<dyn Element> {
    if comments.is_empty() {
        return Empty::new().finish();
    }
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    for comment in comments {
        let id = comment.id.clone();
        let thread_id = comment.thread_id.clone();
        let url = comment.url.clone();
        let author = comment.author.clone();
        let avatar_url = comment.avatar_url.clone().unwrap_or_default();
        let comment_path = comment.path.clone();
        let comment_line = comment.line;
        let kind = if comment.kind == "review" {
            "review".to_string()
        } else {
            "comment".to_string()
        };
        let age = review_short_age(&comment.created_at);
        let body: String = comment.body.chars().take(280).collect();
        let location = match (comment.path.as_deref(), comment.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.to_string(),
            _ => String::new(),
        };
        let is_resolved = comment.is_resolved;

        // Resolve sits far left, Copy + Open group far right
        // (SpaceBetween) — never clustered together.
        let mut trailing = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(2.0);
        let mut resolve_el: Option<Box<dyn Element>> = None;
        if thread_id.is_some() {
            let resolve_id = thread_id.clone().unwrap_or_default();
            let resolve_mouse = review_mouse(states, format!("review_resolve_{id}"));
            let resolving = resolving_id == thread_id.as_deref();
            let resolve_label = if resolving {
                if is_resolved {
                    "Unresolving…"
                } else {
                    "Resolving…"
                }
            } else if is_resolved {
                "Unresolve"
            } else {
                "Resolve"
            };
            let resolve_icon = if is_resolved {
                AppIcon::Undo2
            } else {
                AppIcon::Check
            };
            resolve_el = Some(
                Hoverable::new(resolve_mouse, move |hovered| {
                    Container::new(
                        Text::new(resolve_label.to_string(), ui, crate::theme::text_size(10.0))
                            .with_color(if resolving {
                                theme.disabled_text()
                            } else if hovered.is_hovered() {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            })
                            .finish(),
                    )
                    .with_padding_left(6.0)
                    .with_padding_right(6.0)
                    .with_padding_top(3.0)
                    .with_padding_bottom(3.0)
                    .finish()
                })
                .with_cursor(if resolving {
                    Cursor::Arrow
                } else {
                    Cursor::PointingHand
                })
                .on_click(move |event_ctx, _app, _pos| {
                    if resolving {
                        return;
                    }
                    event_ctx.dispatch_typed_action(SidebarAction::ResolveReviewThread {
                        thread_id: resolve_id.clone(),
                        resolve: !is_resolved,
                    });
                })
                .finish(),
            );
            let _ = resolve_icon;
        }
        {
            let copy_id = id.clone();
            let copy_mouse = review_mouse(states, format!("review_copy_{id}"));
            let just_copied = copied_id == Some(id.as_str());
            let (copy_icon, copy_color) = if just_copied {
                (AppIcon::Check, theme.ui_green())
            } else {
                (AppIcon::Copy, theme.sub_text())
            };
            trailing = trailing.with_child(
                Hoverable::new(copy_mouse, move |hovered| {
                    Container::new(render_icon(
                        copy_icon,
                        if just_copied || hovered.is_hovered() {
                            copy_color
                        } else {
                            theme.sub_text()
                        },
                        11.0,
                    ))
                    .with_padding_left(5.0)
                    .with_padding_right(5.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0)
                    .finish()
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(SidebarAction::CopyReviewComment {
                        id: copy_id.clone(),
                    });
                })
                .finish(),
            );
        }
        if let Some(link) = url.clone() {
            trailing = trailing.with_child(
                Hoverable::new(
                    review_mouse(states, format!("review_open_{id}")),
                    move |hovered| {
                        Container::new(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_spacing(5.0)
                                .with_child(render_icon(
                                    AppIcon::GitHub,
                                    if hovered.is_hovered() {
                                        theme.main_text()
                                    } else {
                                        theme.sub_text()
                                    },
                                    12.0,
                                ))
                                .with_child(
                                    Text::new("Open".to_string(), ui, crate::theme::text_size(10.0))
                                        .with_color(if hovered.is_hovered() {
                                            theme.main_text()
                                        } else {
                                            theme.sub_text()
                                        })
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(3.0)
                        .with_padding_bottom(3.0)
                        .finish()
                    },
                )
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action_for(
                        workspace_view_id,
                        WorkspaceAction::OpenExternalUrl { url: link.clone() },
                    );
                })
                .finish(),
            );
        }

        column = column.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(3.0)
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(6.0)
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    Flex::row()
                                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                        .with_main_axis_size(MainAxisSize::Max)
                                        .with_spacing(6.0)
                                        .with_child(avatar_sized(
                                            &author,
                                            "",
                                            &avatar_url,
                                            theme,
                                            ui,
                                            crate::theme::scaled(15.0),
                                        ))
                                        .with_child(
                                            Expanded::new(
                                                1.0,
                                                Text::new(author, ui, crate::theme::text_size(11.5))
                                                    .with_color(theme.main_text())
                                                    .soft_wrap(false)
                                                    .with_clip(ClipConfig::ellipsis())
                                                    .finish(),
                                            )
                                            .finish(),
                                        )
                                        .finish(),
                                )
                                .finish(),
                            )
                            .with_child(
                                Container::new(
                                    Text::new(kind, ui, crate::theme::text_size(9.0))
                                        .with_color(theme.sub_text())
                                        .finish(),
                                )
                                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                                .with_padding_left(4.0)
                                .with_padding_right(4.0)
                                .with_padding_top(1.0)
                                .with_padding_bottom(1.0)
                                .finish(),
                            )
                            .with_child(
                                Text::new(age, ui, crate::theme::text_size(10.0))
                                    .with_color(theme.sub_text())
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_child(
                        Text::new(body, ui, crate::theme::text_size(11.5))
                            .with_color(theme.sub_text())
                            .soft_wrap(true)
                            .finish(),
                    )
                    .with_child(
                        // Row wrapper: Expanded is only legal inside a
                        // horizontal flex here — the card column scrolls
                        // vertically (infinite height), so a direct Expanded
                        // child would abort layout.
                        review_location_row(
                            ui,
                            theme,
                            workspace_view_id,
                            &location,
                            comment_path.clone(),
                            comment_line,
                            root.clone(),
                            review_mouse(states, format!("review_location_{id}")),
                        ),
                    )
                    .with_child(
                        Container::new(
                            Flex::row()
                                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                .with_main_axis_size(MainAxisSize::Max)
                                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                                .with_child(resolve_el.unwrap_or_else(|| Empty::new().finish()))
                                .with_child(trailing.finish())
                                .finish(),
                        )
                        .with_background_color(theme.surface_2())
                        .with_border(Border::all(1.0).with_border_color(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
                        .with_padding_left(4.0)
                        .with_padding_right(4.0)
                        .with_padding_top(2.0)
                        .with_padding_bottom(2.0)
                        .finish(),
                    )
                    .finish(),
            )
            .with_background_color(theme.surface_1())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_margin_left(8.0)
            .with_margin_right(8.0)
            .with_margin_top(2.0)
            .with_margin_bottom(2.0)
            .finish(),
        );
    }
    column.finish()
}

/// File path + line under a review comment. Clicking opens the file at
/// that position in the editor; plain text when the project root is
/// unknown (or the comment has no path).
#[allow(clippy::too_many_arguments)]
fn review_location_row(
    ui: FamilyId,
    theme: &Theme,
    workspace_view_id: EntityId,
    location: &str,
    path: Option<String>,
    line: Option<u32>,
    root: Option<PathBuf>,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    // Plain text when there is nowhere to navigate to.
    let (Some(root), Some(path)) = (root, path) else {
        return Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(location.to_string(), ui, crate::theme::text_size(10.0))
                        .with_color(theme.disabled_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            )
            .finish();
    };
    // GitHub lines are 1-based; editor positions are 0-based.
    let position = dragent_editor::TextPosition {
        line: line.unwrap_or(1).saturating_sub(1),
        character: 0,
    };
    let range = dragent_editor::TextRange {
        start: position,
        end: position,
    };
    let relative_path = PathBuf::from(path);
    let location_text = location.to_string();
    Hoverable::new(mouse, move |hovered| {
        // Link affordance on hover: accent text (no underline API on Text)
        // plus the row highlight and pointing-hand cursor.
        let hot = hovered.is_hovered();
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Expanded::new(
                        1.0,
                        Text::new(location_text.clone(), ui, crate::theme::text_size(10.0))
                            .with_color(if hot {
                                theme.accent
                            } else {
                                theme.disabled_text()
                            })
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis())
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(if hot {
            theme.sidebar_row_hover()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action_for(
            workspace_view_id,
            WorkspaceAction::OpenLanguageLocation {
                root: root.clone(),
                relative_path: relative_path.clone(),
                range,
            },
        );
    })
    .finish()
}
