//! Review selectors use the same card, search and rows as the agent picker.
use super::*;
use crate::automations::widgets::one_line;
use crate::icons::{render_icon, AppIcon};
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Empty,
    EventHandler, Expanded, Flex, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Stack,
};
use warpui::geometry::vector::vec2f;
use warpui_core::elements::{Dismiss, Hoverable};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

pub(super) fn tag(picker: Picker) -> &'static str {
    match picker {
        Picker::Project => "review-project-picker",
        Picker::FormProject => "review-form-project-picker",
        Picker::State => "review-state-picker",
        Picker::Scope => "review-scope-picker",
        Picker::Base => "review-base-picker",
        Picker::Source => "review-source-picker",
        Picker::Template => "review-template-picker",
        Picker::Merge => "review-merge-picker",
    }
}

pub(super) fn trigger(
    v: &PullRequestsView,
    picker: Picker,
    title: &str,
    icon: AppIcon,
) -> Box<dyn Element> {
    let theme = v.theme;
    let ui = v.ui;
    let title = title.to_string();
    let active = v.picker == Some(picker);
    let id = v.id;
    let button = Hoverable::new(v.mouse(tag(picker)), move |mouse| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(7.0)
                .with_child(render_icon(icon, theme.sub_text(), 13.0))
                .with_child(
                    ConstrainedBox::new(one_line(ui, &title, 12.0, theme.main_text()))
                        .with_max_width(180.0)
                        .finish(),
                )
                .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 11.0))
                .finish(),
        )
        .with_background_color(if active || mouse.is_hovered() {
            theme.surface_2()
        } else {
            theme.surface_1()
        })
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(10.0)
        .with_padding_right(9.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, Action::TogglePicker(picker)))
    .finish();
    SavePosition::new(button, tag(picker)).finish()
}

fn menu(v: &PullRequestsView, picker: Picker) -> Box<dyn Element> {
    use crate::workspace::{picker_caption, picker_card, picker_list, picker_row, picker_search};
    let mut entries: Vec<(String, AppIcon, bool, Action)> = vec![];
    // Branch pickers say what each branch is, in line with `entries`.
    let mut hints: Vec<String> = vec![];
    let caption = match picker {
        Picker::Project | Picker::FormProject => {
            if picker == Picker::Project {
                entries.push((
                    "All projects".into(),
                    AppIcon::Layers,
                    v.project.is_none(),
                    Action::Project(None),
                ));
            }
            entries.extend(v.targets.iter().enumerate().map(|(i, t)| {
                (
                    t.label.clone(),
                    AppIcon::FolderGit,
                    v.project == Some(i)
                        || (picker == Picker::FormProject && v.project.is_none() && i == 0),
                    Action::Project(Some(i)),
                )
            }));
            "PROJECTS"
        }
        Picker::Base => {
            // The default branch first, as the usual place to merge into.
            let default = v.branches.iter().filter(|b| **b == v.form_default);
            for b in default.chain(v.branches.iter().filter(|b| **b != v.form_default)) {
                entries.push((b.clone(), AppIcon::GitBranch, *b == v.base, Action::Base(b.clone())));
                hints.push(if *b == v.form_default { "default".into() } else { String::new() });
            }
            "MERGE INTO"
        }
        Picker::Source => {
            for b in source_branches(v) {
                hints.push(source_hint(v, &b));
                entries.push((b.clone(), AppIcon::GitBranch, b == v.form_head, Action::Source(b)));
            }
            "CHANGES FROM"
        }
        Picker::Template => {
            for (i, template) in v.templates.iter().enumerate() {
                entries.push((template.name.clone(), AppIcon::File, v.template == Some(i), Action::Template(Some(i))));
                hints.push(if template.source == "organisation" { "organisation".into() } else { "repository".into() });
            }
            entries.push(("No template".into(), AppIcon::XClose, v.template.is_none(), Action::Template(None)));
            hints.push(String::new());
            "DESCRIPTION TEMPLATE"
        }
        Picker::State => {
            entries.extend(
                [
                    ("open", "Open", AppIcon::PullRequest),
                    ("merged", "Merged", AppIcon::GitCompare),
                    ("closed", "Closed", AppIcon::CircleX),
                    ("all", "All states", AppIcon::Layers),
                ]
                .into_iter()
                .map(|(key, title, icon)| {
                    (
                        title.into(),
                        icon,
                        v.state == key,
                        Action::State(key.into()),
                    )
                }),
            );
            "REVIEW STATE"
        }
        Picker::Scope => {
            entries.extend(
                [
                    (Filter::All, "All reviews", AppIcon::Inbox),
                    (Filter::Mine, "Created by me", AppIcon::User),
                    (
                        Filter::ReviewRequested,
                        "Review requested",
                        AppIcon::MessageSquare,
                    ),
                    (Filter::Attention, "Needs attention", AppIcon::Activity),
                    (Filter::Ready, "Ready to merge", AppIcon::CircleCheck),
                ]
                .into_iter()
                .map(|(filter, title, icon)| {
                    (
                        title.into(),
                        icon,
                        v.filter == filter,
                        Action::Filter(filter),
                    )
                }),
            );
            "SHOW REVIEWS"
        }
        Picker::Merge => {
            for (method, title) in [
                (MergeMethod::Squash, "Squash and merge"),
                (
                    MergeMethod::Merge,
                    if v.detail.as_ref().is_some_and(|p| is_gitlab(&p.data)) {
                        "Project merge method"
                    } else {
                        "Create a merge commit"
                    },
                ),
                (MergeMethod::Rebase, "Rebase and merge"),
            ] {
                if merge_allowed(&v.repository, method) {
                    entries.push((
                        title.into(),
                        AppIcon::GitCompare,
                        v.merge_method == method,
                        Action::MergeMethod(method),
                    ));
                }
            }
            "MERGE METHOD"
        }
    };
    let query = v.base_query.trim().to_lowercase();
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    let mut count = 0;
    let mut keyboard_actions = Vec::new();
    for (i, (title, icon, checked, action)) in entries.into_iter().enumerate() {
        if !query.is_empty() && !title.to_lowercase().contains(&query) {
            continue;
        }
        count += 1;
        if keyboard_actions.len() < 9 {
            keyboard_actions.push(action.clone());
        }
        let key = format!("review-picker-{picker:?}-{i}");
        rows = rows.with_child(if matches!(picker, Picker::Base | Picker::Source | Picker::Template) {
            hint_row(v, &key, icon, &title, hints.get(i).map_or("", String::as_str), checked, action)
        } else {
            picker_row(v.id, &v.theme, v.ui, &v.mouse, &key, icon, &title, checked, action)
        });
    }
    if count == 0 {
        rows = rows.with_child(picker_caption(
            &v.theme,
            v.ui,
            if matches!(picker, Picker::Base | Picker::Source)
                && v.branches.is_empty()
                && v.local_branches.is_empty()
            {
                "Loading branches…"
            } else {
                "No matches"
            },
        ));
    }
    let id = v.id;
    let search = picker_search(
        &v.theme,
        &v.mouse,
        &v.base_search,
        &v.base_query,
        std::rc::Rc::new(move |ctx| ctx.dispatch_typed_action_for(id, Action::ClearPickerSearch)),
    );
    let card = picker_card(
        &v.theme,
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(search)
            .with_child(picker_caption(&v.theme, v.ui, caption))
            .with_child(picker_list(
                &v.theme,
                &v.picker_scroll,
                rows.finish(),
                260.0,
            ))
            .finish(),
        if matches!(picker, Picker::Base | Picker::Source | Picker::Template) { 340.0 } else { 280.0 },
    );
    EventHandler::new(card)
        .on_keydown(move |ctx, _, key| {
            let action = if key.key == "escape" {
                Some(Action::ClosePicker)
            } else if key.key == "enter" {
                keyboard_actions.first().cloned()
            } else if (key.cmd || key.ctrl) && !key.alt {
                key.key
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .and_then(|n| keyboard_actions.get(n - 1))
                    .cloned()
            } else {
                None
            };
            if let Some(action) = action {
                ctx.dispatch_typed_action_for(id, action);
                DispatchEventResult::StopPropagation
            } else {
                DispatchEventResult::PropagateToParent
            }
        })
        .finish()
}

/// Every branch a pull request can come from: the checked-out one, the
/// checkout's own (most recent first), then those only on the remote.
fn source_branches(v: &PullRequestsView) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let current = (!v.form_current.is_empty()).then(|| v.form_current.clone());
    let local = v.local_branches.iter().map(|b| b.name.clone());
    for name in current.into_iter().chain(local).chain(v.branches.iter().cloned()) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// "checked out", "not pushed", "2 to push": what creating from it means.
pub(super) fn source_hint(v: &PullRequestsView, name: &str) -> String {
    let mut parts: Vec<String> = vec![];
    if name == v.form_current {
        parts.push("checked out".into());
    }
    if let Some(local) = v.local_branches.iter().find(|b| b.name == name) {
        if !v.branches.iter().any(|b| b == name) {
            parts.push("not pushed".into());
        } else if local.unpushed > 0 {
            parts.push(format!("{} to push", local.unpushed));
        }
    }
    parts.join(" · ")
}

/// A menu row with a muted note on the right ("default", "not pushed").
fn hint_row(
    v: &PullRequestsView,
    key: &str,
    icon: AppIcon,
    name: &str,
    hint: &str,
    checked: bool,
    action: Action,
) -> Box<dyn Element> {
    let (theme, ui, id) = (v.theme, v.ui, v.id);
    let (name, hint) = (name.to_string(), hint.to_string());
    Hoverable::new(v.mouse(key), move |state| {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(icon, theme.sub_text(), 13.0))
            .with_child(Expanded::new(1.0, one_line(ui, &name, 12.5, theme.main_text())).finish());
        if !hint.is_empty() {
            row = row.with_child(one_line(ui, &hint, 11.0, theme.sub_text()));
        }
        row = row.with_child(if checked {
            render_icon(AppIcon::Check, theme.accent, 13.0)
        } else {
            ConstrainedBox::new(Empty::new().finish()).with_width(13.0).finish()
        });
        Container::new(row.finish())
            .with_background_color(if state.is_hovered() { theme.surface_3() } else { theme.surface_2() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
    .finish()
}

pub(super) fn overlay(v: &PullRequestsView, body: Box<dyn Element>) -> Box<dyn Element> {
    let Some(picker) = v.picker else {
        return body;
    };
    let id = v.id;
    let menu = Dismiss::new(menu(v, picker))
        .on_dismiss(move |ctx, _| ctx.dispatch_typed_action_for(id, Action::ClosePicker))
        .prevent_interaction_with_other_elements()
        .finish();
    Stack::new()
        .with_child(body)
        .with_positioned_overlay_child(
            menu,
            OffsetPositioning::offset_from_save_position_element(
                tag(picker),
                vec2f(0.0, 6.0),
                PositionedElementOffsetBounds::WindowByPosition,
                PositionedElementAnchor::BottomLeft,
                ChildAnchor::TopLeft,
            ),
        )
        .finish()
}
