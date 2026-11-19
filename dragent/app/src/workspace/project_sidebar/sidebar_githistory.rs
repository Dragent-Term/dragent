//! Sidebar git history and panel view.
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
use super::sidebar_gitstatus::{
    git_changes_sticky_group, render_git_file_row, render_git_group_header, render_git_history,
    render_git_sticky_group_header,
};
use super::sidebar_panels::{panel_section, section_title_inline, with_alpha};
use super::view::{
    GitCommitAvatar, GitHistoryEntry, GitHistoryState, GitLabAvatarAsset, GitPanelMode,
    ProjectSidebarView, SidebarAction,
};
use super::view::{META_SIZE, NAME_SIZE, PATH_SIZE, ROW_SIZE, SCRIPT_CMD_SIZE, SECTION_SIZE};

pub(crate) fn render_sidebar_tab_header(
    active: super::SidebarTab,
    changed_file_count: u32,
    workspace_view_id: EntityId,
    ui: FamilyId,
    theme: &Theme,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let tab_button = |key: &str,
                      label: &'static str,
                      icon: AppIcon,
                      tab: super::SidebarTab,
                      is_active: bool,
                      count: Option<u32>| {
        let mouse = states
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone();
        Hoverable::new(mouse, move |state| {
            let hot = is_active || state.is_hovered();
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(crate::icons::render_icon(
                    icon,
                    if hot { theme.accent } else { theme.sub_text() },
                    12.0,
                ))
                .with_child(
                    Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(if is_active {
                            theme.main_text()
                        } else {
                            theme.sub_text()
                        })
                        .with_style(Properties::default().weight(if is_active {
                            Weight::Medium
                        } else {
                            Weight::Normal
                        }))
                        .finish(),
                );
            if let Some(count) = count {
                row = row.with_child(
                    Container::new(
                        Text::new(count.to_string(), ui, crate::theme::text_size(10.0))
                            .with_color(if is_active {
                                theme.main_text()
                            } else {
                                theme.sub_text()
                            })
                            .finish(),
                    )
                    .with_padding_left(5.0)
                    .with_padding_right(5.0)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                    .with_background_color(theme.surface_2())
                    .finish(),
                );
            }
            let row = row.finish();
            // Vertically centered content with the active underline pinned
            // to the bar's bottom divider line. Every height is fixed and
            // every width is content-driven: no flex expansion anywhere,
            // so no unbounded-constraint risk.
            Container::new(
                ConstrainedBox::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_child(
                            ConstrainedBox::new(
                                Flex::column()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                    .with_main_axis_size(MainAxisSize::Max)
                                    .with_main_axis_alignment(MainAxisAlignment::Center)
                                    .with_child(
                                        Container::new(row)
                                            .with_padding_left(10.0)
                                            .with_padding_right(10.0)
                                            .finish(),
                                    )
                                    .finish(),
                            )
                            .with_height(crate::theme::PANEL_HEADER_HEIGHT - 2.0)
                            .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Container::new(warpui::elements::Empty::new().finish())
                                    .with_background_color(if is_active {
                                        theme.accent
                                    } else {
                                        ColorU::new(0, 0, 0, 0)
                                    })
                                    .finish(),
                            )
                            .with_height(2.0)
                            .finish(),
                        )
                        .finish(),
                )
                .with_height(crate::theme::PANEL_HEADER_HEIGHT)
                .finish(),
            )
            .with_background_color(if is_active {
                theme.accent_overlay()
            } else if state.is_hovered() {
                theme.surface_1()
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::SetProjectSidebarTab { tab },
            );
        })
        .finish()
    };

    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(tab_button(
            "sidebar_tab_files",
            "Files",
            AppIcon::Folder,
            super::SidebarTab::Files,
            active == super::SidebarTab::Files,
            None,
        ))
        .with_child(tab_button(
            "sidebar_tab_search",
            "Search",
            AppIcon::Search,
            super::SidebarTab::Search,
            active == super::SidebarTab::Search,
            None,
        ))
        .with_child(tab_button(
            "sidebar_tab_git",
            "Changes",
            AppIcon::GitBranch,
            super::SidebarTab::SourceControl,
            active == super::SidebarTab::SourceControl,
            Some(changed_file_count),
        ))
        .finish();

    // Shared `nav_header_bar` geometry: same outer height, padding, and
    // border as every other navbar, so this divider joins their line.
    crate::workspace::ide_chrome::nav_header_bar(
        header,
        theme,
        0.0,
        0.0,
        crate::theme::PANEL_HEADER_HEIGHT,
    )
}

pub(crate) fn fetch_git_history(cwd: &Path) -> Result<Vec<GitHistoryEntry>, String> {
    let root = crate::git_status::repo_root(cwd);
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args([
            "log",
            "-n",
            "200",
            "--date=relative",
            "--pretty=format:%H%x1f%h%x1f%an%x1f%ae%x1f%ar%x1f%s",
        ])
        .current_dir(&root)
        .output()
        .map_err(|error| format!("git log failed: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // An unborn repository has no history, but that is a valid empty state.
        if error.contains("does not have any commits yet") || error.contains("your current branch")
        {
            return Ok(Vec::new());
        }
        return Err(error);
    }
    let mut entries = parse_git_history(&String::from_utf8_lossy(&output.stdout));
    if let Some(remote) = fetch_git_remote(&root) {
        for entry in &mut entries {
            entry.avatar = remote.avatar_for(entry);
        }
    }
    Ok(entries)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitHostingRemote {
    GitHub,
    GitLab { host: String, project: String },
}

impl GitHostingRemote {
    pub(crate) fn avatar_for(&self, entry: &GitHistoryEntry) -> Option<GitCommitAvatar> {
        let email = entry
            .author_email
            .trim()
            .trim_matches(['<', '>'])
            .to_lowercase();
        if email.is_empty() {
            return None;
        }
        match self {
            Self::GitHub if !email.ends_with("[bot]@users.noreply.github.com") => {
                let email =
                    url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>();
                Some(GitCommitAvatar::GitHub(format!(
                    "https://avatars.githubusercontent.com/u/e?email={email}&s=128"
                )))
            }
            Self::GitHub => None,
            Self::GitLab { host, project } => Some(GitCommitAvatar::GitLab {
                host: host.clone(),
                project: project.clone(),
                sha: entry.sha.clone(),
                email,
            }),
        }
    }
}

fn fetch_git_remote(root: &Path) -> Option<GitHostingRemote> {
    let output = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout))
        .and_then(|remote| parse_git_hosting_remote(remote.trim()))
}

pub(crate) fn parse_git_hosting_remote(remote: &str) -> Option<GitHostingRemote> {
    let (host, path) = if remote.contains("://") {
        let url = url::Url::parse(remote).ok()?;
        (
            url.host_str()?.to_lowercase(),
            url.path().trim_matches('/').to_string(),
        )
    } else {
        let (_, location) = remote.rsplit_once('@')?;
        let (host, path) = location.split_once(':')?;
        (host.to_lowercase(), path.trim_matches('/').to_string())
    };
    let project = path.strip_suffix(".git").unwrap_or(&path).to_string();
    if host == "github.com" {
        Some(GitHostingRemote::GitHub)
    } else if host == "gitlab.com" || host.contains("gitlab") {
        Some(GitHostingRemote::GitLab { host, project })
    } else {
        None
    }
}

pub(crate) fn gitlab_avatar_source(
    host: &str,
    project: &str,
    sha: &str,
    email: &str,
) -> AssetSource {
    let key = format!("gitlab:{host}:{project}:{sha}:{email}");
    let host = host.to_string();
    let email = email.to_string();
    AssetSource::Async {
        id: AsyncAssetId::new::<GitLabAvatarAsset>(&key),
        fetch: Arc::new(move || {
            let host = host.clone();
            let email = email.clone();
            Box::pin(fetch_gitlab_avatar(host, email))
        }),
    }
}

#[cfg(not(target_family = "wasm"))]
async fn fetch_gitlab_avatar(host: String, email: String) -> anyhow::Result<bytes::Bytes> {
    let (tx, rx) = futures::channel::oneshot::channel();
    crate::bg::detach(move || {
        let email = url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>();
        let endpoint = format!("https://{host}/api/v4/avatar?email={email}");
        let result = (|| {
            let response = ureq::get(&endpoint)
                .timeout(std::time::Duration::from_secs(10))
                .call()?;
            let payload: serde_json::Value = response.into_json()?;
            let avatar_url = payload
                .get("avatar_url")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("GitLab returned no avatar"))?;
            crate::settings::pages::account::fetch_avatar_bytes(avatar_url)
        })();
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| anyhow::anyhow!("GitLab avatar fetch thread dropped"))?
}

#[cfg(target_family = "wasm")]
async fn fetch_gitlab_avatar(_host: String, _email: String) -> anyhow::Result<bytes::Bytes> {
    anyhow::bail!("GitLab avatars are unavailable in the web viewer")
}

pub(crate) fn parse_git_history(output: &str) -> Vec<GitHistoryEntry> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(6, '\u{1f}');
            Some(GitHistoryEntry {
                sha: fields.next()?.to_string(),
                short_sha: fields.next()?.to_string(),
                author: fields.next()?.to_string(),
                author_email: fields.next()?.to_string(),
                relative_time: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
                avatar: None,
            })
        })
        .collect()
}

/// Source-control workspace: Changes/History modes, aggregate diff toolbar,
/// staged/unstaged groups, and the commit area. All state comes from the
/// active session's git model via the shared snapshot; every action
/// dispatches the session's existing git operations — one git backend.
///
/// Changes layout: fixed toolbar → scrollable changes list → fixed composer.
/// History owns a separate scroll surface and opens commit patches on demand.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_git_panel_view(
    state: &super::GitPanelState,
    git_commit_editor: &ViewHandle<InputEditor>,
    git_filter_editor: &ViewHandle<InputEditor>,
    git_filter_text: &str,
    git_filter_open: bool,
    workspace_view_id: EntityId,
    view_id: EntityId,
    mouse_states: &RefCell<HashMap<String, MouseStateHandle>>,
    changes_scroll: ClippedScrollStateHandle,
    history_scroll: ClippedScrollStateHandle,
    panel_mode: GitPanelMode,
    view_options_open: bool,
    show_diff_stats: bool,
    staged_collapsed: bool,
    changes_collapsed: bool,
    history: &GitHistoryState,
    commit_generating: bool,
    _commit_generation_error: Option<&str>,
    ui: FamilyId,
    mono: FamilyId,
    theme: &Theme,
    app: &AppContext,
) -> Box<dyn Element> {
    let Some((status, operation, operation_error)) = state.git_model.as_ref().and_then(|model| {
        model.read(app, |m, _| {
            m.status
                .clone()
                .map(|status| (status, m.operation, m.operation_error.clone()))
        })
    }) else {
        return panel_section(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(6.0)
                .with_child(section_title_inline("Changes", ui, theme))
                .with_child(
                    Text::new(
                        "No Git repository detected in this workspace.".to_string(),
                        ui,
                        crate::theme::text_size(META_SIZE),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .finish(),
            None,
        );
    };
    let terminal_id = state.terminal_view_id;
    // Two-click discard arms without a terminal: armed if the stored key is
    // fresh (6s). Expired arms render unarmed; the next click re-arms.
    let armed_fresh = |key: &str| {
        state
            .armed_discard
            .as_ref()
            .is_some_and(|(armed_key, armed_at)| {
                armed_key == key && armed_at.elapsed() < std::time::Duration::from_secs(6)
            })
    };

    let staged_entries: Vec<&crate::console::git_status::GitStatusEntry> = status
        .entries
        .iter()
        .filter(|e| e.index != ' ' && e.index != '?')
        .collect();
    let change_entries: Vec<&crate::console::git_status::GitStatusEntry> =
        status.entries.iter().filter(|e| e.work != ' ').collect();
    let has_staged = !staged_entries.is_empty();
    let has_changes = !change_entries.is_empty();
    let total_added = status.added;
    let total_deleted = status.deleted;
    // Path-substring filter. Unfiltered flags
    // above keep driving commit enablement and the clean-tree empty state;
    // only rows and header counts below use the filtered views.
    let filter = git_filter_text.trim().to_lowercase();
    let matches_filter =
        |rel_path: &str| filter.is_empty() || rel_path.to_lowercase().contains(&filter);
    // Source-control view mode + group order from Version Control.
    // List keeps status grouping; Tree sorts by path. Group order selects the sort key.
    fn sort_entries<'a>(
        mut list: Vec<&'a crate::console::git_status::GitStatusEntry>,
    ) -> Vec<&'a crate::console::git_status::GitStatusEntry> {
        match crate::settings::git_consumers::sc_group_order() {
            crate::settings::ScGroupOrder::Path => {
                list.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
            }
            crate::settings::ScGroupOrder::FileType => {
                list.sort_by(|a, b| {
                    let ext = |p: &str| p.rsplit('.').next().unwrap_or("").to_lowercase();
                    ext(&a.rel_path)
                        .cmp(&ext(&b.rel_path))
                        .then_with(|| a.rel_path.cmp(&b.rel_path))
                });
            }
            crate::settings::ScGroupOrder::Status => {
                if matches!(
                    crate::settings::git_consumers::sc_view_mode(),
                    crate::settings::ScViewMode::Tree
                ) {
                    list.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
                } else {
                    list.sort_by(|a, b| (a.index, a.work, &a.rel_path).cmp(&(b.index, b.work, &b.rel_path)));
                }
            }
        }
        list
    }
    let staged_visible: Vec<_> = sort_entries(
        staged_entries
            .iter()
            .copied()
            .filter(|entry| matches_filter(&entry.rel_path))
            .collect(),
    );
    let changes_visible: Vec<_> = sort_entries(
        change_entries
            .iter()
            .copied()
            .filter(|entry| matches_filter(&entry.rel_path))
            .collect(),
    );
    // Standard panel modes: history is a sibling of the live changes list,
    // not a branch-menu side effect or a separate application surface.
    let mode_tab = |key: &str, label: &'static str, mode: GitPanelMode, active: bool| {
        let mouse = mouse_states
            .borrow_mut()
            .entry(key.to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone();
        Hoverable::new(mouse, move |state| {
            // Vertically centered label with the active underline pinned to
            // the bar's bottom divider line. All fixed heights, content
            // widths: no flex expansion, no unbounded-constraint risk.
            Container::new(
                ConstrainedBox::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_child(
                            ConstrainedBox::new(
                                Flex::row()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                                    .with_child(
                                        Container::new(
                                            Text::new(
                                                label.to_string(),
                                                ui,
                                                crate::theme::text_size(12.0),
                                            )
                                            .with_color(if active {
                                                theme.main_text()
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
                                        .with_padding_left(crate::theme::scaled(9.0))
                                        .with_padding_right(crate::theme::scaled(9.0))
                                        .finish(),
                                    )
                                    .finish(),
                            )
                            .with_height(crate::theme::PANEL_HEADER_HEIGHT - 2.0)
                            .finish(),
                        )
                        .with_child(
                            ConstrainedBox::new(
                                Container::new(warpui::elements::Empty::new().finish())
                                    .with_background_color(if active {
                                        theme.accent
                                    } else {
                                        ColorU::transparent_black()
                                    })
                                    .finish(),
                            )
                            .with_height(2.0)
                            .finish(),
                        )
                        .finish(),
                )
                .with_height(crate::theme::PANEL_HEADER_HEIGHT)
                .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action_for(view_id, SidebarAction::SetGitPanelMode(mode));
        })
        .finish()
    };
    // Shared `nav_header_bar` geometry, so the Changes/History divider is
    // the same straight line as the editor nav and terminal navbar.
    let mode_header = crate::workspace::ide_chrome::nav_header_bar(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(mode_tab(
                "git_mode_changes",
                "Changes",
                GitPanelMode::Changes,
                panel_mode == GitPanelMode::Changes,
            ))
            .with_child(mode_tab(
                "git_mode_history",
                "History",
                GitPanelMode::History,
                panel_mode == GitPanelMode::History,
            ))
            .finish(),
        theme,
        0.0,
        0.0,
        crate::theme::PANEL_HEADER_HEIGHT,
    );

    let diff_mouse = mouse_states
        .borrow_mut()
        .entry("git_view_aggregate_diff".to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let diff_button = Hoverable::new(diff_mouse, move |state| {
        let mut content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(crate::theme::scaled(5.0))
            .with_child(render_icon(
                AppIcon::Code,
                if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
                crate::theme::scaled(12.0),
            ))
            .with_child(
                Text::new("View".to_string(), ui, crate::theme::text_size(11.0))
                    .with_color(if state.is_hovered() {
                        theme.main_text()
                    } else {
                        theme.sub_text()
                    })
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
            );
        if show_diff_stats && (total_added > 0 || total_deleted > 0) {
            content = content
                .with_child(
                    Text::new(format!("+{total_added}"), mono, crate::theme::text_size(10.5))
                        .with_color(theme.ansi_green())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        format!("−{total_deleted}"),
                        mono,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(theme.ansi_red())
                    .finish(),
                );
        }
        Container::new(content.finish())
            .with_padding_left(crate::theme::scaled(7.0))
            .with_padding_right(crate::theme::scaled(7.0))
            .with_padding_top(crate::theme::scaled(4.0))
            .with_padding_bottom(crate::theme::scaled(4.0))
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                crate::theme::scaled(5.0),
            )))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(workspace_view_id, WorkspaceAction::OpenGitWorktreeDiff);
    })
    .finish();

    // File filter toggle: reveals the "Search files…"
    // input that filters staged and unstaged rows by path substring.
    let search_mouse = mouse_states
        .borrow_mut()
        .entry("git_filter_toggle".to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let search_button = Hoverable::new(search_mouse, move |state| {
        let button = Container::new(render_icon(
            AppIcon::Search,
            if git_filter_open || state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
            crate::theme::scaled(12.0),
        ))
        .with_uniform_padding(crate::theme::scaled(5.0))
        .with_background_color(if git_filter_open || state.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
            crate::theme::scaled(5.0),
        )))
        .finish();
        buffer_search_tooltip(button, state.is_hovered(), "Search files", theme, ui)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _app, _pos| {
        ctx.dispatch_typed_action_for(view_id, SidebarAction::ToggleGitFilter);
    })
    .finish();

    let repo_header_base = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, diff_button).finish())
            .with_child(search_button)
            .finish(),
    )
    .with_padding_left(crate::theme::scaled(4.0))
    .with_padding_right(crate::theme::scaled(5.0))
    .with_padding_top(crate::theme::scaled(4.0))
    .with_padding_bottom(crate::theme::scaled(4.0))
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .finish();

    let view_options_panel: Option<Box<dyn Element>> = if view_options_open {
        let option_mouse = mouse_states
            .borrow_mut()
            .entry("git_option_diff_stats".to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone();
        let panel = Hoverable::new(option_mouse, move |state| {
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Text::new(
                            "Show Diff Statistics".to_string(),
                            ui,
                            crate::theme::text_size(11.0),
                        )
                        .with_color(theme.main_text())
                        .finish(),
                    )
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(render_icon(
                        if show_diff_stats {
                            AppIcon::Check
                        } else {
                            AppIcon::XClose
                        },
                        if show_diff_stats {
                            theme.accent
                        } else {
                            theme.disabled_text()
                        },
                        crate::theme::scaled(11.0),
                    ))
                    .finish(),
            )
            .with_padding_left(crate::theme::scaled(10.0))
            .with_padding_right(crate::theme::scaled(10.0))
            .with_padding_top(crate::theme::scaled(6.0))
            .with_padding_bottom(crate::theme::scaled(6.0))
            .with_background_color(if state.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                crate::theme::scaled(6.0),
            )))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _app, _pos| {
            ctx.dispatch_typed_action_for(view_id, SidebarAction::ToggleGitDiffStats);
        })
        .finish();
        Some(
            ConstrainedBox::new(panel)
                .with_width(crate::theme::scaled(190.0))
                .finish(),
        )
    } else {
        None
    };
    let mut repo_header_stack = Stack::new().with_child(repo_header_base);
    if let Some(view_options_panel) = view_options_panel {
        repo_header_stack = repo_header_stack.with_positioned_overlay_child(
            view_options_panel,
            OffsetPositioning::offset_from_parent(
                vec2f(-4.0, 4.0),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::BottomRight,
                ChildAnchor::TopRight,
            ),
        );
    }
    let repo_header = repo_header_stack.finish();

    // ── Changes list (SCROLL OWNER) ───────────────────────────────────
    let sticky_group = git_changes_sticky_group(
        changes_scroll.scroll_start().as_f32(),
        staged_visible.len(),
        staged_collapsed,
        changes_visible.len(),
        changes_collapsed,
    );
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);
    if has_staged {
        list = list.with_child(render_git_group_header(
            "Staged",
            staged_visible.len(),
            theme,
            ui,
            mouse_states,
            terminal_id,
            view_id,
            workspace_view_id,
            true,
            staged_collapsed,
            armed_fresh("all:staged"),
        ));
        if !staged_collapsed {
            for entry in &staged_visible {
                let arm_key = format!("file:{}", entry.rel_path);
                list = list.with_child(render_git_file_row(
                    entry,
                    true,
                    &status.diff_files,
                    state.cwd.as_deref(),
                    terminal_id,
                    workspace_view_id,
                    mouse_states,
                    show_diff_stats,
                    armed_fresh(&arm_key),
                    ui,
                    mono,
                    theme,
                ));
            }
        }
    }
    if has_changes {
        list = list.with_child(render_git_group_header(
            "Unstaged",
            changes_visible.len(),
            theme,
            ui,
            mouse_states,
            terminal_id,
            view_id,
            workspace_view_id,
            false,
            changes_collapsed,
            armed_fresh("all:unstaged"),
        ));
        if !changes_collapsed {
            for entry in &changes_visible {
                let arm_key = format!("file:{}", entry.rel_path);
                list = list.with_child(render_git_file_row(
                    entry,
                    false,
                    &status.diff_files,
                    state.cwd.as_deref(),
                    terminal_id,
                    workspace_view_id,
                    mouse_states,
                    show_diff_stats,
                    armed_fresh(&arm_key),
                    ui,
                    mono,
                    theme,
                ));
            }
        }
    }
    if !filter.is_empty() && staged_visible.is_empty() && changes_visible.is_empty() {
        list = list.with_child(
            Container::new(
                Text::new(format!("No files match “{filter}”"), ui, crate::theme::text_size(META_SIZE))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .finish(),
        );
    }
    if !has_staged && !has_changes {
        list = list.with_child(
            Container::new(
                Text::new("Working tree clean".to_string(), ui, crate::theme::text_size(META_SIZE))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .finish(),
        );
    }

    // ── Commit composer (FIXED) ───────────────────────────────────────
    let commit_message =
        git_commit_editor.read(app, |editor, _| editor.get_text().trim().to_string());
    let operation_busy = operation != crate::git_status::GitOperation::Idle;
    let composer_busy = operation_busy || commit_generating;
    let can_commit = has_staged && !commit_message.is_empty() && !composer_busy;
    // Compare-against-upstream gates the ahead/behind push affordance.
    let effective_ahead = if crate::settings::git_consumers::sc_compare_upstream() {
        status.ahead
    } else {
        0
    };
    let can_commit_and_push = !composer_busy
        && ((has_staged && !commit_message.is_empty()) || (!has_staged && effective_ahead > 0));
    let commit_label = if operation == crate::git_status::GitOperation::Committing {
        "Committing…"
    } else {
        "Commit"
    };
    let push_label = if operation == crate::git_status::GitOperation::Pushing {
        "Commit & Push…"
    } else {
        "Commit & Push"
    };

    let commit_mouse = mouse_states
        .borrow_mut()
        .entry("git_commit_btn".to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let push_mouse = mouse_states
        .borrow_mut()
        .entry("git_commit_push_btn".to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    let commit_message_for_commit = commit_message.clone();
    let commit_btn = Hoverable::new(commit_mouse, move |state| {
        Container::new(
            Text::new(commit_label.to_string(), ui, crate::theme::text_size(11.0))
                .with_color(if can_commit {
                    theme.background
                } else {
                    theme.disabled_text()
                })
                .with_style(Properties::default().weight(Weight::Medium))
                .finish(),
        )
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(if can_commit {
            if state.is_hovered() {
                theme.accent
            } else {
                with_alpha(theme.accent, 0.84)
            }
        } else {
            theme.surface_2()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .with_cursor(if can_commit {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |ctx, _app, _pos| {
        if can_commit {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::CommitGitStaged {
                    message: commit_message_for_commit.clone(),
                    push: false,
                },
            );
        }
    })
    .finish();
    let push_btn = Hoverable::new(push_mouse, move |state| {
        Container::new(
            Text::new(push_label.to_string(), ui, crate::theme::text_size(11.0))
                .with_color(if can_commit_and_push && state.is_hovered() {
                    theme.main_text()
                } else if can_commit_and_push {
                    theme.sub_text()
                } else {
                    theme.disabled_text()
                })
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(if can_commit_and_push && state.is_hovered() {
            theme.surface_2()
        } else {
            theme.surface_1()
        })
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    })
    .with_cursor(if can_commit_and_push {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |ctx, _app, _pos| {
        if can_commit_and_push {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::CommitGitStaged {
                    message: commit_message.clone(),
                    push: true,
                },
            );
        }
    })
    .finish();

    let commit_editor_el = crate::shared_input::shared_input(
        theme,
        git_commit_editor,
        crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 6.0,
            background: Some(theme.background),
            border: Some(theme.outline()),
            pad_left: 8.0,
            pad_right: 8.0,
            pad_top: 6.0,
            pad_bottom: 6.0,
            ..Default::default()
        },
    );

    let generate_mouse = mouse_states
        .borrow_mut()
        .entry("git_generate_commit_btn".to_string())
        .or_insert_with(MouseStateHandle::default)
        .clone();
    // Commit-message AI toggle gates the Generate button.
    let can_generate = (has_staged || has_changes) && !composer_busy
        && crate::settings::git_consumers::commit_ai_enabled();
    let generate_btn = Hoverable::new(generate_mouse, move |state| {
        let tint = if commit_generating {
            theme.accent
        } else if can_generate {
            if state.is_hovered() {
                theme.main_text()
            } else {
                theme.accent
            }
        } else {
            theme.disabled_text()
        };
        let button = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(5.0)
                .with_child(render_icon(AppIcon::Agent, tint, 12.0))
                .with_child(
                    Text::new(
                        if commit_generating {
                            "Generating…"
                        } else {
                            "AI"
                        }
                        .to_string(),
                        ui,
                        crate::theme::text_size(10.5),
                    )
                    .with_color(tint)
                    .with_style(Properties::default().weight(Weight::Medium))
                    .finish(),
                )
                .finish(),
        )
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(if can_generate && state.is_hovered() {
            with_alpha(theme.accent, 0.10)
        } else {
            ColorU::new(0, 0, 0, 0)
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .finish();
        buffer_search_tooltip(
            button,
            state.is_hovered(),
            "Generate from all Git changes",
            theme,
            ui,
        )
    })
    .with_cursor(if can_generate {
        Cursor::PointingHand
    } else {
        Cursor::Arrow
    })
    .on_click(move |ctx, _app, _pos| {
        if can_generate {
            ctx.dispatch_typed_action_for(
                workspace_view_id,
                WorkspaceAction::GenerateGitCommitMessage,
            );
        }
    })
    .finish();

    let mut composer_content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new("Commit message".to_string(), ui, crate::theme::text_size(META_SIZE))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(Box::new(Expanded::new(1.0, Empty::new().finish())))
                .with_child(generate_btn)
                .finish(),
        )
        .with_child(commit_editor_el)
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(commit_btn)
                .with_child(push_btn)
                .finish(),
        );
    if operation_busy {
        let message = match operation {
            crate::git_status::GitOperation::Committing => "Committing staged changes…",
            crate::git_status::GitOperation::Pushing => "Committing and pushing to origin…",
            crate::git_status::GitOperation::Pulling => "Pulling from origin…",
            crate::git_status::GitOperation::Idle => "",
        };
        composer_content = composer_content.with_child(
            Text::new(message.to_string(), ui, crate::theme::text_size(10.5))
                .with_color(theme.accent)
                .finish(),
        );
    } else if commit_generating {
        composer_content = composer_content.with_child(
            Text::new(
                "Reading the complete change set and drafting a message…".to_string(),
                ui,
                crate::theme::text_size(10.5),
            )
            .with_color(theme.accent)
            .finish(),
        );
    }

    let composer = Container::new(composer_content.finish())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(12.0)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish();

    // Keep the scroll gutter inside the file rows. The list itself must remain
    // full-width so row hover surfaces and section borders reach the scrollbar
    // edge rather than stopping at the content inset.
    let scroll_content = list.finish();
    let changes_scrollable = ClippedScrollable::vertical(
        changes_scroll,
        scroll_content,
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.sub_text().into(),
        theme.surface_1().into(),
    )
    .with_overlayed_scrollbar()
    .finish();
    let changes_surface: Box<dyn Element> = if let Some(staged) = sticky_group {
        let (title, count, collapsed) = if staged {
            ("Staged", staged_visible.len(), staged_collapsed)
        } else {
            ("Unstaged", changes_visible.len(), changes_collapsed)
        };
        let sticky_header = render_git_sticky_group_header(
            title,
            count,
            theme,
            ui,
            mouse_states,
            terminal_id,
            view_id,
            workspace_view_id,
            staged,
            collapsed,
            armed_fresh(if staged { "all:staged" } else { "all:unstaged" }),
        );
        Stack::new()
            .with_child(changes_scrollable)
            .with_positioned_child(
                sticky_header,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, 0.0),
                    ParentOffsetBounds::ParentBySize,
                    ParentAnchor::TopLeft,
                    ChildAnchor::TopLeft,
                ),
            )
            .finish()
    } else {
        changes_scrollable
    };
    let changes_body = Expanded::new(1.0, changes_surface).finish();
    let history_body = render_git_history(
        history,
        history_scroll,
        workspace_view_id,
        mouse_states,
        ui,
        mono,
        theme,
    );

    // Changes and History are sibling panel surfaces, matching the standard source
    // control model. Only Changes owns the live worktree toolbar/composer;
    // History is an independently scrollable commit timeline.
    let mut panel = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(mode_header);
    panel = match panel_mode {
        GitPanelMode::Changes => {
            panel = panel.with_child(repo_header);
            if git_filter_open {
                let clear_mouse = mouse_states
                    .borrow_mut()
                    .entry("git_filter_clear".to_string())
                    .or_insert_with(MouseStateHandle::default)
                    .clone();
                panel = panel.with_child(
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(crate::theme::scaled(6.0))
                            .with_child(
                                Expanded::new(1.0, ChildView::new(git_filter_editor).finish())
                                    .finish(),
                            )
                            .with_child(
                                Hoverable::new(clear_mouse, move |state| {
                                    Container::new(render_icon(
                                        AppIcon::XClose,
                                        if state.is_hovered() {
                                            theme.main_text()
                                        } else {
                                            theme.sub_text()
                                        },
                                        crate::theme::scaled(11.0),
                                    ))
                                    .with_uniform_padding(crate::theme::scaled(4.0))
                                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                                        crate::theme::scaled(4.0),
                                    )))
                                    .finish()
                                })
                                .with_cursor(Cursor::PointingHand)
                                .on_click(move |ctx, _app, _pos| {
                                    ctx.dispatch_typed_action_for(
                                        view_id,
                                        SidebarAction::ClearGitFilter,
                                    );
                                })
                                .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(crate::theme::scaled(10.0))
                    .with_padding_right(crate::theme::scaled(8.0))
                    .with_padding_top(crate::theme::scaled(6.0))
                    .with_padding_bottom(crate::theme::scaled(6.0))
                    .with_background_color(theme.pane_surface())
                    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                    .finish(),
                );
            }
            if crate::git_features::worktree_manager_enabled() {
                panel = panel.with_child(
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(6.0)
                            .with_child(render_icon(AppIcon::GitBranch, theme.sub_text(), 12.0))
                            .with_child(
                                Text::new("Worktrees".to_string(), ui, crate::theme::text_size(10.5))
                                    .with_color(theme.sub_text())
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(4.0)
                    .finish(),
                );
            }
            panel = panel.with_child(changes_body).with_child(composer);
            if crate::git_features::multi_diff_editor_enabled() {
                panel = panel.with_child(
                    Container::new(
                        Text::new(
                            "Open in multi-diff editor".to_string(),
                            ui,
                            crate::theme::text_size(10.0),
                        )
                        .with_color(theme.accent)
                        .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(6.0)
                    .finish(),
                );
            }
            panel
        }
        GitPanelMode::History => {
            if crate::git_features::graph_history_enabled() {
                panel = panel.with_child(
                    Container::new(
                        Text::new("Graph".to_string(), ui, crate::theme::text_size(10.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .with_padding_left(10.0)
                    .with_padding_top(6.0)
                    .with_padding_bottom(4.0)
                    .finish(),
                );
            }
            panel.with_child(history_body)
        }
    };
    panel.finish()
}

// Source-control row measurements are design-time values at the default app
// typography. They are passed through `theme::scaled` while rendering so the
// row follows Settings -> Typography instead of becoming a tiny fixed island.
pub(crate) const GIT_STATUS_COLUMN_WIDTH: f32 = 28.0;
pub(crate) const GIT_STATUS_BADGE_SIZE: f32 = 22.0;
pub(crate) const GIT_SECONDARY_ACTION_WIDTH: f32 = 20.0;
pub(crate) const GIT_TRAILING_GROUP_WIDTH: f32 = 60.0;
pub(crate) const GIT_GROUP_HEADER_HEIGHT: f32 = 31.0;
pub(crate) const GIT_FILE_ROW_HEIGHT: f32 = 37.0;
