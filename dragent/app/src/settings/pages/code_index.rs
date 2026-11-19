//! Code and Index — controls for the semantic codebase index that gives the
//! agent project context. Status card is live (`CodeIndexManager::index_status`),
//! toggles drive the manager directly, and the ignore-patterns editor writes
//! the user-global `index.ignore` consumed by the walker and watcher.

use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Shrinkable, Text, Wrap,
};
use warpui_core::elements::WrapFill;
use warpui_core::scene::{Border, CornerRadius, Radius};

use dragent_index::{RepoIndexState, RepoIndexStatus};

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::app_settings::EmbeddingsBackend;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    alert_banner, card_rows, danger_button, group_label, page_header, secondary_button,
    select_control, setting_row, toggle_row, SelectItem,
};

pub fn render_code_index(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let settings = &view.app_settings.index;

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui,
            "Code and Index",
            "Manage how Dragent indexes your codebase to give the agent context.",
        ));

    // ── Live status: consolidated accordion ───────────────────────────────────
    if settings.enabled {
        let statuses = view.index_statuses.clone().unwrap_or_default();
        if view.index_statuses.is_none() {
            col = col.with_child(info_card(
                theme,
                ui,
                "Checking the index…",
                "Asking the indexing service for the status of your repositories.",
            ));
        } else if statuses.is_empty() {
            col = col.with_child(info_card(
                theme,
                ui,
                "No repositories indexed yet",
                "Open a project folder in a terminal tab and the index builds automatically.",
            ));
        } else {
            col = col.with_child(index_accordion(view, theme, &statuses));
        }
    } else {
        col = col.with_child(info_card(
            theme,
            ui,
            "Indexing is disabled",
            "The agent falls back to text search only. Enable indexing below to restore semantic context.",
        ));
    }

    if let Some(msg) = &view.index_status_message {
        col = col.with_child(
            Container::new(alert_banner(theme, ui, msg))
                .with_padding_top(12.0)
                .finish(),
        );
    }

    // ── Indexing settings ─────────────────────────────────────────────────────
    let embeddings_select = select_control(
        theme,
        ui,
        id,
        view.hover("embeddings_btn"),
        settings.embeddings.label(),
        190.0,
        view.picker_open("embeddings"),
        SettingsAction::TogglePicker("embeddings".to_string()),
        vec![
            SelectItem {
                label: EmbeddingsBackend::Local.label().to_string(),
                selected: settings.embeddings == EmbeddingsBackend::Local,
                enabled: true,
                hover: view.hover("embeddings_local"),
                action: SettingsAction::SelectEmbeddingsBackend(EmbeddingsBackend::Local),
            },
            SelectItem {
                label: "Cloud (coming soon)".to_string(),
                selected: false,
                enabled: false,
                hover: view.hover("embeddings_cloud"),
                action: SettingsAction::SelectEmbeddingsBackend(EmbeddingsBackend::Cloud),
            },
        ],
    );

    col = col
        .with_child(group_label(theme, ui, "Indexing"))
        .with_child(card_rows(
            theme,
            vec![
            toggle_row(
                theme,
                ui,
                id,
                view.hover("idx_enabled"),
                "Enable codebase indexing",
                Some("Build a semantic index so the agent understands your whole project."),
                settings.enabled,
                SettingsAction::ToggleIndexingEnabled,
            ),
            toggle_row(
                theme,
                ui,
                id,
                view.hover("idx_auto"),
                "Auto-index on changes",
                Some(
                    "Re-index files automatically as you edit. When off, use Re-index to refresh.",
                ),
                settings.auto_index,
                SettingsAction::ToggleAutoIndex,
            ),
            setting_row(
                theme,
                ui,
                "Embeddings",
                Some("Where embeddings are computed. Local keeps code on your machine."),
                embeddings_select,
            ),
            toggle_row(
                theme,
                ui,
                id,
                view.hover("idx_gitignore"),
                "Respect .gitignore",
                Some(
                    "Skip files and folders excluded by your .gitignore. Changing this re-indexes.",
                ),
                settings.respect_gitignore,
                SettingsAction::ToggleRespectGitignore,
            ),
        ],
        ));

    // ── Ignore patterns ───────────────────────────────────────────────────────
    col = col.with_child(group_label(theme, ui, "Ignore patterns"));
    if let Some(editor) = &view.ignore_patterns_editor {
        let editor_box = Container::new(ChildView::new(editor).finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .with_uniform_padding(13.0)
            .finish();

        let footer = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new(
                        "One glob pattern per line. These paths are never indexed or sent to the model."
                            .to_string(),
                        ui,
                        crate::theme::text_size(11.5),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .finish(),
            )
            .with_child(secondary_button(
                theme,
                ui,
                id,
                view.hover("idx_save_ignore"),
                "Save patterns",
                SettingsAction::SaveIgnorePatterns,
            ))
            .finish();

        col = col
            .with_child(editor_box)
            .with_child(Container::new(footer).with_padding_top(8.0).finish());
    }

    col.finish()
}

// ── Status card ───────────────────────────────────────────────────────────────

fn status_card(view: &SettingsView, theme: &Theme, status: &RepoIndexStatus) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;

    let (dot_color, state_label) = match status.state {
        RepoIndexState::Indexing => (theme.accent, "Indexing…"),
        RepoIndexState::Watching => {
            if status.pending > 0 {
                (theme.accent, "Syncing changes…")
            } else {
                (theme.ui_green(), "Index up to date")
            }
        }
        RepoIndexState::Paused => (theme.ui_yellow(), "Paused — heavy churn detected"),
    };

    let repo_name = status
        .root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_string();

    let mut detail_parts: Vec<String> = Vec::new();
    detail_parts.push(format!("{} files", format_count(status.files)));
    detail_parts.push(format!("{} chunks", format_count(status.chunks)));
    if status.pending > 0 {
        detail_parts.push(format!("{} pending", status.pending));
    }
    if let Some(secs) = status.synced_secs_ago {
        detail_parts.push(format!("synced {}", format_ago(secs)));
    }
    if status.capped {
        detail_parts.push("file limit reached".to_string());
    }
    let detail = detail_parts.join(" · ");

    // Colored state dot in a rounded well, echoing the design's check tile.
    let inner_dot = Container::new(
        ConstrainedBox::new(warpui::elements::Empty::new().finish())
            .with_width(9.0)
            .with_height(9.0)
            .finish(),
    )
    .with_background_color(dot_color)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.5)))
    .finish();
    let dot = ConstrainedBox::new(
        Container::new(warpui::elements::Align::new(inner_dot).finish())
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish(),
    )
    .with_width(34.0)
    .with_height(34.0)
    .finish();

    let text_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(
            Text::new(
                format!("{state_label} — {repo_name}"),
                ui,
                crate::theme::text_size(13.5),
            )
            .with_color(theme.main_text())
            .finish(),
        )
        .with_child(
            Container::new(
                Text::new(detail, ui, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_padding_top(3.0)
            .finish(),
        )
        .finish();

    let identity = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(14.0)
        .with_child(dot)
        .with_child(Shrinkable::new(1.0, text_col).finish())
        .finish();

    let reindex_btn = secondary_button(
        theme,
        ui,
        id,
        view.hover(&format!("reindex_{}", status.root.display())),
        "Re-index",
        SettingsAction::ReindexRepo(status.root.clone()),
    );
    let delete_btn = danger_button(
        theme,
        ui,
        id,
        view.hover(&format!("delete_index_{}", status.root.display())),
        "Delete",
        SettingsAction::DeleteIndex(status.root.clone()),
    );
    let buttons = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(reindex_btn)
        .with_child(delete_btn)
        .finish();

    // Same Wrap+WrapFill idiom as setting_row: the button drops to its own run
    // instead of overlapping when the pane is narrow.
    Container::new(
        Wrap::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(16.0)
            .with_run_spacing(12.0)
            .with_child(WrapFill::new(220.0, identity).finish())
            .with_child(buttons)
            .finish(),
    )
    .with_background_color(warpui::color::ColorU::new(0, 0, 0, 0))
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(16.0)
    .with_margin_top(12.0)
    .finish()
}

/// Consolidated indexed-folders view: a header showing the count that toggles
/// open to reveal a searchable, paginated list of per-repo status cards.
const INDEX_PAGE_SIZE: usize = 4;

fn folder_name(status: &RepoIndexStatus) -> String {
    status
        .root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_string()
}

fn index_accordion(
    view: &SettingsView,
    theme: &Theme,
    statuses: &[RepoIndexStatus],
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let expanded = view.index_list_expanded;
    let total = statuses.len();

    // Header row (clickable): count + chevron.
    let chevron = if expanded { "\u{25BE}" } else { "\u{25B8}" };
    let header_label = format!(
        "\u{1F4C1}  {total} indexed folder{}",
        if total == 1 { "" } else { "s" }
    );
    let header =
        warpui_core::elements::Hoverable::new(view.hover("index_accordion_header"), move |st| {
            let bg = if st.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            };
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Text::new(header_label.clone(), ui, crate::theme::text_size(13.5))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .with_child(
                        Text::new(chevron.to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(bg)
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, SettingsAction::ToggleIndexList);
        })
        .finish();

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(header);

    if !expanded {
        return col.finish();
    }

    // Search bar.
    let search = crate::shared_input::shared_input(
        theme,
        &view.index_search_editor,
        crate::shared_input::SharedInputOpts {
            leading_element: Some(
                Text::new("\u{2315}".to_string(), ui, crate::theme::text_size(14.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            ),
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 8.0,
            background: Some(theme.surface_1()),
            border: Some(theme.outline()),
            pad_left: 12.0,
            pad_right: 12.0,
            pad_top: 8.0,
            pad_bottom: 8.0,
            spacing: 8.0,
            ..Default::default()
        },
    );
    col = col.with_child(search);

    // Filter by folder name / path.
    let q = view.index_search_query.trim().to_lowercase();
    let filtered: Vec<&RepoIndexStatus> = statuses
        .iter()
        .filter(|st| {
            q.is_empty()
                || folder_name(st).to_lowercase().contains(&q)
                || st.root.to_string_lossy().to_lowercase().contains(&q)
        })
        .collect();

    if filtered.is_empty() {
        col = col.with_child(
            Container::new(
                Text::new(
                    format!(
                        "No indexed folder matches \u{201c}{}\u{201d}.",
                        view.index_search_query.trim()
                    ),
                    ui,
                    crate::theme::text_size(13.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_padding_left(4.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish(),
        );
        return col.finish();
    }

    // Pagination.
    let page_count = filtered.len().div_ceil(INDEX_PAGE_SIZE).max(1);
    let page = view.index_list_page.min(page_count - 1);
    let start = page * INDEX_PAGE_SIZE;
    let end = (start + INDEX_PAGE_SIZE).min(filtered.len());
    for st in &filtered[start..end] {
        col = col.with_child(status_card(view, theme, st));
    }

    if page_count > 1 {
        let prev_enabled = page > 0;
        let next_enabled = page + 1 < page_count;
        let pager = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(pager_button(
                view,
                theme,
                ui,
                id,
                "\u{2039} Prev",
                prev_enabled,
                if prev_enabled { page - 1 } else { page },
            ))
            .with_child(
                Text::new(
                    format!("Page {} of {page_count}", page + 1),
                    ui,
                    crate::theme::text_size(12.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(pager_button(
                view,
                theme,
                ui,
                id,
                "Next \u{203A}",
                next_enabled,
                if next_enabled { page + 1 } else { page },
            ))
            .finish();
        col = col.with_child(Container::new(pager).with_padding_top(4.0).finish());
    }

    col.finish()
}

fn pager_button(
    view: &SettingsView,
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    view_id: warpui::EntityId,
    label: &str,
    enabled: bool,
    target_page: usize,
) -> Box<dyn Element> {
    let color = if enabled {
        theme.main_text()
    } else {
        theme.disabled_text()
    };
    let build = move |bg: warpui::color::ColorU| -> Box<dyn Element> {
        Container::new(
            Text::new(label.to_string(), ui, crate::theme::text_size(12.5))
                .with_color(color)
                .finish(),
        )
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish()
    };
    if !enabled {
        return build(warpui::color::ColorU::new(0, 0, 0, 0));
    }
    let hover_key = format!("index_pager_{target_page}_{label}");
    warpui_core::elements::Hoverable::new(view.hover(&hover_key), move |st| {
        build(if st.is_hovered() {
            theme.surface_2()
        } else {
            warpui::color::ColorU::new(0, 0, 0, 0)
        })
    })
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(view_id, SettingsAction::IndexListPage(target_page));
    })
    .finish()
}

fn info_card(
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    title: &str,
    body: &str,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Text::new(title.to_string(), ui, crate::theme::text_size(13.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Container::new(
                    Text::new(body.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_padding_top(3.0)
                .finish(),
            )
            .finish(),
    )
    .with_background_color(warpui::color::ColorU::new(0, 0, 0, 0))
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(16.0)
    .with_margin_top(12.0)
    .finish()
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn format_count(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n >= 1000 {
        // 1,284 style for the readable mid range.
        let thousands = n / 1000;
        let rest = n % 1000;
        format!("{thousands},{rest:03}")
    } else {
        n.to_string()
    }
}

fn format_ago(secs: u64) -> String {
    if secs < 10 {
        "just now".to_string()
    } else if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::{format_ago, format_count};

    #[test]
    fn count_formatting() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1284), "1,284");
        assert_eq!(format_count(8200), "8,200");
        assert_eq!(format_count(10_000), "10.0k");
        assert_eq!(format_count(82_000), "82.0k");
    }

    #[test]
    fn ago_formatting() {
        assert_eq!(format_ago(3), "just now");
        assert_eq!(format_ago(45), "45s ago");
        assert_eq!(format_ago(120), "2m ago");
        assert_eq!(format_ago(7200), "2h ago");
        assert_eq!(format_ago(200_000), "2d ago");
    }
}
