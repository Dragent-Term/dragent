//! Settings → Projects: every project in the organisation, grouped by the
//! machine it's on, beside the one picked — where it lives, how new
//! workspaces start in it, and the ways to open, hide or delete it.
//! Backed by the workspace catalogue.
use super::widgets::{card_rows, danger_button, page_header, setting_row};
use super::{SettingsAction, SettingsView};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::theme::Theme;
use std::path::PathBuf;
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize,
    ParentElement, Text, Wrap,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

#[derive(Clone, Debug)]
pub enum ProjectSettingsAction {
    Refresh,
    Add,
    Open(PathBuf),
    ToggleVisibility(PathBuf),
    Remove(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectSettingsEntry {
    pub root: PathBuf,
    pub name: String,
    pub path: String,
    pub machine: String,
    pub remote: bool,
    pub workspaces: usize,
    pub groups: usize,
    pub hidden: bool,
    pub in_sidebar: bool,
}

/// The project list's width beside the details.
const LIST_WIDTH: f32 = 272.0;
/// The narrowest page that still fits the list beside the details; narrower
/// pages stack them.
const SIDE_BY_SIDE_MIN: f32 = 760.0;
/// Project initials sit on one of these, picked by name, so a project keeps
/// its colour.
const AVATAR_COLORS: [ColorU; 6] = [
    ColorU { r: 0x60, g: 0xa5, b: 0xfa, a: 0xff },
    ColorU { r: 0xa7, g: 0x8b, b: 0xfa, a: 0xff },
    ColorU { r: 0x34, g: 0xd3, b: 0x99, a: 0xff },
    ColorU { r: 0xfb, g: 0xbf, b: 0x24, a: 0xff },
    ColorU { r: 0xf4, g: 0x72, b: 0xb6, a: 0xff },
    ColorU { r: 0x22, g: 0xd3, b: 0xee, a: 0xff },
];

fn act(action: ProjectSettingsAction) -> SettingsAction {
    SettingsAction::Project(action)
}

pub fn render_projects(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    // Never paint a previous organisation's snapshot during an account switch.
    let entries: &[ProjectSettingsEntry] = if view.projects_org == dragent_paths::organisation_id() {
        &view.projects
    } else {
        &[]
    };
    let add = |key: &str| {
        mui::button(
            theme,
            ui,
            id,
            view.hover(key),
            Some(AppIcon::Plus),
            "Add project",
            Variant::Primary,
            Size::Md,
            true,
            act(ProjectSettingsAction::Add),
        )
    };
    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(
            Expanded::new(
                1.0,
                page_header(
                    theme,
                    ui,
                    "Projects",
                    "Every project in this organisation, on this computer and on your machines.",
                ),
            )
            .finish(),
        )
        .with_child(mui::button(
            theme,
            ui,
            id,
            view.hover("projects_refresh"),
            Some(AppIcon::RefreshCw),
            "",
            Variant::Ghost,
            Size::Md,
            true,
            act(ProjectSettingsAction::Refresh),
        ))
        .with_child(add("projects_add"))
        .finish();
    let page = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header);
    let Some(first) = entries.first() else {
        return page
            .with_child(mui::empty_state(
                theme,
                ui,
                AppIcon::FolderGit,
                "No projects yet",
                "Add a folder on this computer, clone a repository, or open one on a connected machine.",
                Some(add("projects_add_empty")),
            ))
            .finish();
    };
    let selected = view
        .selected_repo_project
        .as_ref()
        .and_then(|root| entries.iter().find(|project| &project.root == root))
        .unwrap_or(first);
    let list = project_list(view, theme, entries, &selected.root);
    let detail = project_detail(view, theme, selected);
    let body = if super::settings_view::content_width(view.content_pane_width) >= SIDE_BY_SIDE_MIN {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(16.0)
            .with_child(ConstrainedBox::new(list).with_width(LIST_WIDTH).finish())
            .with_child(Expanded::new(1.0, detail).finish())
            .finish()
    } else {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(16.0)
            .with_child(list)
            .with_child(detail)
            .finish()
    };
    page.with_child(stats(theme, ui, entries))
        .with_child(Container::new(body).with_padding_top(20.0).finish())
        .finish()
}

/// Projects, workspaces and machines, counted.
fn stats(theme: &Theme, ui: warpui::fonts::FamilyId, entries: &[ProjectSettingsEntry]) -> Box<dyn Element> {
    let workspaces: usize = entries.iter().map(|project| project.workspaces).sum();
    let mut machines: Vec<&str> = entries.iter().map(|project| project.machine.as_str()).collect();
    machines.sort_unstable();
    machines.dedup();
    let card = |label: &str, value: usize| {
        Expanded::new(
            1.0,
            Container::new(
                Flex::column()
                    .with_spacing(4.0)
                    .with_child(mui::text(ui, label, 11.5, theme.sub_text()))
                    .with_child(mui::semibold(ui, value.to_string(), 20.0, theme.main_text()))
                    .finish(),
            )
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .finish(),
        )
        .finish()
    };
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_child(card("Projects", entries.len()))
        .with_child(card("Workspaces", workspaces))
        .with_child(card(if machines.len() == 1 { "Machine" } else { "Machines" }, machines.len()))
        .finish()
}

/// A project's initial on its colour.
fn avatar(ui: warpui::fonts::FamilyId, name: &str, size: f32) -> Box<dyn Element> {
    let initial = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into());
    let hash = name.bytes().fold(0usize, |hash, byte| hash.wrapping_mul(31).wrapping_add(byte as usize));
    let color = AVATAR_COLORS[hash % AVATAR_COLORS.len()];
    Container::new(mui::sized(
        warpui::elements::Align::new(mui::semibold(ui, initial, (size * 0.42).round(), color)).finish(),
        size,
        size,
    ))
    .with_background_color(mui::alpha(color, 0x24))
    .with_border(Border::all(1.0).with_border_color(mui::alpha(color, 0x40)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels((size * 0.28).round())))
    .finish()
}

/// `/Users/ada/code/app` as `~/code/app`.
fn short_path(path: &str) -> String {
    match dirs::home_dir().map(|home| home.to_string_lossy().into_owned()) {
        Some(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// The searchable list, grouped by machine, this computer first.
fn project_list(
    view: &SettingsView,
    theme: &Theme,
    entries: &[ProjectSettingsEntry],
    selected: &PathBuf,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let search = crate::shared_input::shared_input(
        theme,
        &view.projects_search_editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 13.0,
            }),
            ..Default::default()
        },
    );
    let query = view.projects_search.trim().to_lowercase();
    let visible: Vec<&ProjectSettingsEntry> = entries
        .iter()
        .filter(|project| {
            format!("{} {} {}", project.name, project.path, project.machine)
                .to_lowercase()
                .contains(&query)
        })
        .collect();
    let mut machines: Vec<(&str, bool)> = Vec::new();
    for project in &visible {
        if !machines.iter().any(|(machine, _)| *machine == project.machine) {
            machines.push((&project.machine, project.remote));
        }
    }
    machines.sort_by_key(|(_, remote)| *remote);

    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(Container::new(search).with_uniform_padding(10.0).finish());
    if visible.is_empty() {
        column = column.with_child(
            Container::new(mui::text(ui, "No project matches.", 12.5, theme.sub_text()))
                .with_padding_left(16.0)
                .with_padding_top(6.0)
                .with_padding_bottom(14.0)
                .finish(),
        );
    }
    for (machine, remote) in machines {
        let label = if remote { machine.to_string() } else { "This computer".to_string() };
        column = column.with_child(
            Container::new(mui::eyebrow(ui, theme, &label))
                .with_padding_left(16.0)
                .with_padding_top(10.0)
                .with_padding_bottom(6.0)
                .finish(),
        );
        for project in visible.iter().filter(|project| project.machine == machine) {
            column = column.with_child(project_row(view, theme, project, &project.root == selected));
        }
    }
    Container::new(column.finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
        .with_padding_bottom(8.0)
        .finish()
}

fn project_row(
    view: &SettingsView,
    theme: &Theme,
    project: &ProjectSettingsEntry,
    selected: bool,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let theme = *theme;
    let name = project.name.clone();
    let path = short_path(&project.path);
    let hidden = project.hidden;
    let workspaces = project.workspaces;
    let root = project.root.clone();
    let view_id = view.view_id;
    let row = Hoverable::new(
        view.hover(&format!("project_row:{}", project.root.display())),
        move |state| {
            let mut labels = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(2.0)
                .with_child(mui::line(
                    ui,
                    name.clone(),
                    13.0,
                    if hidden { theme.sub_text() } else { theme.main_text() },
                ))
                .with_child(mui::line(ui, path.clone(), 11.5, theme.disabled_text()));
            if hidden {
                labels = labels.with_child(mui::line(ui, "Hidden from the sidebar", 11.0, theme.disabled_text()));
            }
            let mut row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(avatar(ui, &name, 28.0))
                .with_child(Expanded::new(1.0, labels.finish()).finish());
            if workspaces > 0 {
                row = row.with_child(
                    Container::new(mui::medium(ui, workspaces.to_string(), 11.0, theme.sub_text()))
                        .with_background_color(theme.surface_2())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                        .with_padding_left(6.0)
                        .with_padding_right(6.0)
                        .with_padding_top(1.0)
                        .with_padding_bottom(1.0)
                        .finish(),
                );
            }
            Container::new(row.finish())
                .with_background_color(if selected {
                    theme.surface_2()
                } else if state.is_hovered() {
                    theme.surface_1()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(view_id, SettingsAction::SelectRepoProject(root.clone()))
    })
    .finish();
    Container::new(row)
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .finish()
}

/// The picked project: who it is, where it lives, what can be done with it.
fn project_detail(view: &SettingsView, theme: &Theme, project: &ProjectSettingsEntry) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let key = project.root.display().to_string();

    let title = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(14.0)
        .with_child(avatar(ui, &project.name, 46.0))
        .with_child(
            Expanded::new(
                1.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(4.0)
                    .with_child(mui::semibold(ui, project.name.clone(), 19.0, theme.main_text()))
                    .with_child(mui::line(ui, short_path(&project.path), 12.5, theme.sub_text()))
                    .finish(),
            )
            .finish(),
        )
        .finish();

    let meta_item = |icon: AppIcon, text: String| {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(icon, theme.sub_text(), 13.0))
            .with_child(mui::text(ui, text, 12.5, theme.sub_text()))
            .finish()
    };
    let (status, tone) = if project.hidden {
        ("Hidden", Tone::Neutral)
    } else if project.in_sidebar {
        ("In sidebar", Tone::Success)
    } else {
        ("Saved", Tone::Neutral)
    };
    let meta = Wrap::row()
        .with_spacing(16.0)
        .with_run_spacing(8.0)
        .with_child(mui::pill(theme, ui, status, tone))
        .with_child(meta_item(
            if project.remote { AppIcon::Server } else { AppIcon::Monitor },
            if project.remote { project.machine.clone() } else { "This computer".into() },
        ))
        .with_child(meta_item(
            AppIcon::FolderGit,
            mui::count(project.workspaces, "workspace", "workspaces"),
        ))
        .finish();

    let mut actions = Wrap::row()
        .with_spacing(8.0)
        .with_run_spacing(8.0)
        .with_child(mui::button(
            theme,
            ui,
            id,
            view.hover(&format!("project_open:{key}")),
            Some(AppIcon::FolderOpen),
            "Open",
            Variant::Primary,
            Size::Md,
            true,
            act(ProjectSettingsAction::Open(project.root.clone())),
        ));
    for app in view.app_settings.git.open_in_apps.iter().take(2) {
        actions = actions.with_child(mui::button(
            theme,
            ui,
            id,
            view.hover(&format!("open_in:{key}:{app}")),
            None,
            &format!("Open in {app}"),
            Variant::Secondary,
            Size::Md,
            true,
            SettingsAction::OpenInApp {
                app: app.clone(),
                path: key.clone(),
            },
        ));
    }
    actions = actions.with_child(mui::button(
        theme,
        ui,
        id,
        view.hover(&format!("project_hide:{key}")),
        Some(AppIcon::Eye),
        if project.hidden { "Show in sidebar" } else { "Hide from sidebar" },
        Variant::Ghost,
        Size::Md,
        true,
        act(ProjectSettingsAction::ToggleVisibility(project.root.clone())),
    ));

    let field = |field: &str, label: &str, hint: &str| {
        let control: Box<dyn Element> = match view.repo_editors.get(&format!("repo_{field}:{key}")) {
            Some(editor) => crate::shared_input::shared_input(
                theme,
                editor,
                crate::shared_input::SharedInputOpts {
                    height: Some(mui::CONTROL_HEIGHT),
                    radius: mui::RADIUS_CONTROL,
                    pad_left: 11.0,
                    pad_right: 11.0,
                    ..Default::default()
                },
            ),
            None => mui::text(ui, "…", 12.5, theme.sub_text()),
        };
        mui::field(ui, theme, label, control, Some(hint))
    };
    let workspaces = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(20.0)
            .with_child(field(
                "base_ref",
                "Start from",
                "The branch new workspaces start from. Empty uses the branch you pick, or the current one.",
            ))
            .with_child(field(
                "base_path",
                "Worktree folder",
                "Where this project's worktrees are created. Empty uses Settings → General.",
            ))
            .with_child(field(
                "identity",
                "Commit as",
                "Set in each new worktree. Empty uses your usual git identity.",
            ))
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
    .with_uniform_padding(18.0)
    .finish();
    let danger = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui,
                "Reset project settings",
                Some("Forget the overrides above for this project."),
                danger_button(
                    theme,
                    ui,
                    id,
                    view.hover(&format!("repo_clear:{key}")),
                    "Reset",
                    SettingsAction::ClearRepoOverride(project.root.clone()),
                ),
            ),
            setting_row(
                theme,
                ui,
                "Delete project",
                Some("Removes it and its worktrees from Dragent. You're asked to confirm first."),
                danger_button(
                    theme,
                    ui,
                    id,
                    view.hover(&format!("project_remove:{key}")),
                    "Delete…",
                    act(ProjectSettingsAction::Remove(project.root.clone())),
                ),
            ),
        ],
    );
    let group = |title: &str, body: Box<dyn Element>| {
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(mui::eyebrow(ui, theme, title))
            .with_child(body)
            .finish()
    };

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(title)
            .with_child(Container::new(meta).with_padding_top(14.0).finish())
            .with_child(Container::new(actions.finish()).with_padding_top(18.0).finish())
            .with_child(
                Container::new(group("New workspaces", workspaces))
                    .with_padding_top(28.0)
                    .finish(),
            )
            .with_child(
                Container::new(group("Danger zone", danger))
                    .with_padding_top(28.0)
                    .finish(),
            )
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
    .with_uniform_padding(24.0)
    .finish()
}
