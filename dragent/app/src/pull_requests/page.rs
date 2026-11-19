use super::*;
use crate::automations::widgets::{button, field, label, one_line, ButtonKind};
use crate::icons::{render_icon, AppIcon};
use crate::machine_ui::{medium, semibold};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Empty, EventHandler, Expanded, Flex, MainAxisSize, ParentElement,
    SavePosition, ScrollbarWidth, Stack, Text,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

fn pad(e: Box<dyn Element>, x: f32, y: f32) -> Box<dyn Element> {
    Container::new(e)
        .with_padding_left(x)
        .with_padding_right(x)
        .with_padding_top(y)
        .with_padding_bottom(y)
        .finish()
}
fn col() -> Flex {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
}
fn row() -> Flex {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
}
fn rule(t: &Theme, vertical: bool) -> Box<dyn Element> {
    let e = Container::new(Empty::new().finish())
        .with_background_color(t.outline())
        .finish();
    if vertical {
        ConstrainedBox::new(e).with_width(1.0).finish()
    } else {
        ConstrainedBox::new(e).with_height(1.0).finish()
    }
}
fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU::new(c.r, c.g, c.b, a)
}
fn gitlab(v: &PullRequestsView) -> bool {
    (v.form.is_some() && v.form_is_gitlab)
        || v.detail.as_ref().is_some_and(|p| is_gitlab(&p.data))
        || is_gitlab(&v.repository)
}
fn user_avatar(v: &PullRequestsView, user: &Value, review: &Value, size: f32) -> Box<dyn Element> {
    let name = user["login"]
        .as_str()
        .or_else(|| user["name"].as_str())
        .unwrap_or("?");
    super::avatars::render(
        v.theme,
        v.ui,
        name,
        &super::avatars::photo_url(user, review),
        size,
    )
}
fn btn(
    v: &PullRequestsView,
    key: &str,
    title: &str,
    kind: ButtonKind,
    action: Option<Action>,
) -> Box<dyn Element> {
    button(
        &v.theme,
        v.ui,
        v.id,
        v.mouse(key),
        None,
        title,
        kind,
        if v.busy { None } else { action },
    )
}
fn chip(
    v: &PullRequestsView,
    key: &str,
    title: &str,
    selected: bool,
    action: Action,
) -> Box<dyn Element> {
    let theme = v.theme;
    let ui = v.ui;
    let text = title.to_string();
    let id = v.id;
    Hoverable::new(v.mouse(key), move |mouse| {
        Container::new(medium(
            ui,
            &text,
            12.0,
            if selected {
                theme.main_text()
            } else {
                theme.sub_text()
            },
        ))
        .with_background_color(if selected {
            theme.surface_3()
        } else if mouse.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
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

fn icon_button(
    v: &PullRequestsView,
    key: &str,
    icon: AppIcon,
    tooltip: &str,
    action: Option<Action>,
) -> Box<dyn Element> {
    let theme = v.theme;
    let ui = v.ui;
    let hint = tooltip.to_string();
    let enabled = action.is_some() && !v.busy;
    let id = v.id;
    Hoverable::new(v.mouse(key), move |mouse| {
        let content = Container::new(
            Align::new(render_icon(
                icon,
                if enabled {
                    theme.sub_text()
                } else {
                    theme.disabled_text()
                },
                15.0,
            ))
            .finish(),
        )
        .with_background_color(if enabled && mouse.is_hovered() {
            theme.surface_2()
        } else {
            ColorU::transparent_black()
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_uniform_padding(8.0)
        .finish();
        crate::machine_ui::with_tooltip(&theme, ui, content, mouse.is_hovered(), &hint)
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        if enabled {
            if let Some(action) = &action {
                ctx.dispatch_typed_action_for(id, action.clone());
            }
        }
    })
    .finish()
}
fn tab_button(
    v: &PullRequestsView,
    key: &str,
    title: &str,
    selected: bool,
    action: Action,
) -> Box<dyn Element> {
    let theme = v.theme;
    let ui = v.ui;
    let text = title.to_string();
    let id = v.id;
    Hoverable::new(v.mouse(key), move |mouse| {
        Container::new(medium(
            ui,
            &text,
            12.0,
            if selected || mouse.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            },
        ))
        .with_border(
            Border::bottom(if selected { 2.0 } else { 0.0 }).with_border_color(theme.accent),
        )
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(11.0)
        .with_padding_bottom(10.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
    .finish()
}
fn since(value: &str) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(value) else {
        return value.to_string();
    };
    let minutes = (chrono::Utc::now().signed_duration_since(at).num_minutes()).max(0);
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 1440 {
        format!("{}h ago", minutes / 60)
    } else {
        format!("{}d ago", minutes / 1440)
    }
}
fn scope_title(filter: Filter) -> &'static str {
    match filter {
        Filter::All => "All reviews",
        Filter::Mine => "Created by me",
        Filter::ReviewRequested => "Review requested",
        Filter::Attention => "Needs attention",
        Filter::Ready => "Ready to merge",
    }
}

fn scroll(
    v: &PullRequestsView,
    handle: &ClippedScrollStateHandle,
    e: Box<dyn Element>,
) -> Box<dyn Element> {
    ClippedScrollable::vertical(
        handle.clone(),
        e,
        ScrollbarWidth::Auto,
        v.theme.outline().into(),
        v.theme.sub_text().into(),
        v.theme.background.into(),
    )
    .with_overlayed_scrollbar()
    .finish()
}
fn pill(v: &PullRequestsView, title: &str, color: ColorU) -> Box<dyn Element> {
    Container::new(medium(v.ui, title, 11.0, color))
        .with_background_color(tint(color, 24))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish()
}
fn card(v: &PullRequestsView, e: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(pad(e, 16.0, 14.0))
        .with_background_color(v.theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(v.theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .finish()
}
fn state_color(v: &PullRequestsView, p: &Value) -> ColorU {
    match text(p, "state") {
        "MERGED" => v.theme.accent,
        "CLOSED" => v.theme.ui_red(),
        _ if p["isDraft"].as_bool().unwrap_or(false) => v.theme.sub_text(),
        _ => v.theme.ui_green(),
    }
}
fn ready_color(v: &PullRequestsView, state: &str) -> ColorU {
    match state {
        "Ready to merge" | "Merged" => v.theme.ui_green(),
        "Conflicts" | "Checks failing" | "Changes requested" => v.theme.ui_red(),
        _ => v.theme.ui_yellow(),
    }
}
fn empty(v: &PullRequestsView, title: &str, body: &str) -> Box<dyn Element> {
    Align::new(
        ConstrainedBox::new(
            col()
                .with_child(render_icon(AppIcon::PullRequest, v.theme.accent, 32.0))
                .with_child(semibold(v.ui, title, 22.0, v.theme.main_text()))
                .with_child(label(v.ui, body, 13.0, v.theme.sub_text()))
                .finish(),
        )
        .with_max_width(440.0)
        .finish(),
    )
    .finish()
}

pub(super) fn render(v: &PullRequestsView, app: &AppContext) -> Box<dyn Element> {
    let title = row()
        .with_child(render_icon(AppIcon::PullRequest, v.theme.accent, 20.0))
        .with_child(
            Expanded::new(
                1.0,
                col()
                    .with_spacing(3.0)
                    .with_child(
                        row()
                            .with_child(semibold(v.ui, "Code reviews", 20.0, v.theme.main_text()))
                            .with_child(pill(v, &v.items.len().to_string(), v.theme.sub_text()))
                            .finish(),
                    )
                    .with_child(label(
                        v.ui,
                        "Pull requests and merge requests across your projects",
                        12.0,
                        v.theme.sub_text(),
                    ))
                    .finish(),
            )
            .finish(),
        )
        .with_child(icon_button(
            v,
            "connections",
            AppIcon::Plug,
            "Repository connections",
            (!v.targets.is_empty()).then_some(Action::Connections),
        ))
        .with_child(icon_button(
            v,
            "refresh",
            AppIcon::RefreshCw,
            if v.loading {
                "Refreshing reviews…"
            } else {
                "Refresh reviews"
            },
            (!v.loading).then_some(Action::Refresh),
        ))
        .with_child(button(
            &v.theme,
            v.ui,
            v.id,
            v.mouse("create"),
            Some(AppIcon::Plus),
            if gitlab(v) {
                "New merge request"
            } else {
                "New pull request"
            },
            ButtonKind::Primary,
            (!v.targets.is_empty() && !v.busy).then_some(Action::New),
        ))
        .finish();
    let header = Container::new(pad(title, 20.0, 16.0))
        .with_background_color(v.theme.surface_1())
        .finish();
    let controls = row()
        .with_child(super::menus::trigger(
            v,
            Picker::Project,
            v.project
                .and_then(|i| v.targets.get(i))
                .map(|t| t.label.as_str())
                .unwrap_or("All projects"),
            AppIcon::FolderGit,
        ))
        .with_child(super::menus::trigger(
            v,
            Picker::State,
            match v.state.as_str() {
                "open" => "Open",
                "closed" => "Closed",
                "merged" => "Merged",
                _ => "All states",
            },
            AppIcon::PullRequest,
        ))
        .with_child(super::menus::trigger(
            v,
            Picker::Scope,
            scope_title(v.filter),
            AppIcon::Inbox,
        ))
        .finish();
    let search = crate::shared_input::shared_input(
        &v.theme,
        &v.search,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 13.0,
            }),
            width: crate::shared_input::SharedInputWidth::Fill,
            ..Default::default()
        },
    );
    let search = row()
        .with_child(Expanded::new(1.0, search).finish())
        .with_child(icon_button(
            v,
            "search-server",
            AppIcon::ArrowRight,
            "Search on the repository provider",
            Some(Action::Search),
        ))
        .finish();
    let toolbar = if v.width.get() < 1100.0 {
        col()
            .with_spacing(8.0)
            .with_child(search)
            .with_child(controls)
            .finish()
    } else {
        row()
            .with_child(controls)
            .with_child(Expanded::new(1.0, search).finish())
            .finish()
    };
    let body = if v.targets.is_empty() {
        empty(v,"Your review inbox starts here","Add a GitHub or GitLab project from the sidebar. Sign in with its CLI on the machine that owns the project.")
    } else if v.loading && v.items.is_empty() {
        empty(v,"Loading code reviews…","Fetching pull and merge requests from your project machines. Your inbox will appear here when the first refresh completes.")
    } else if v.items.is_empty() && !v.errors.is_empty() {
        Align::new(ConstrainedBox::new(col()
            .with_child(semibold(v.ui,"Couldn't load code reviews",21.0,v.theme.main_text()))
            .with_child(label(v.ui,"Check your connection and provider sign-in on the project machine. Open Details above for the exact error.",13.0,v.theme.sub_text()))
            .with_child(row().with_child(btn(v,"connect-provider","Set up connection",ButtonKind::Secondary,Some(Action::Connections)))
                .with_child(btn(v,"retry-inbox","Try again",ButtonKind::Primary,Some(Action::Refresh))).finish()).finish())
            .with_max_width(520.0).finish()).finish()
    } else if v.items.is_empty() {
        empty(v,"No reviews found","This request completed successfully. Try another project or state, or create a pull or merge request.")
    } else if v.width.get() < 1050.0 {
        if v.list_only || v.detail.is_none() {
            inbox(v)
        } else {
            detail(v, app)
        }
    } else {
        let list_width = if v.width.get() < 1100.0 { 280.0 } else { 340.0 };
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                ConstrainedBox::new(inbox(v))
                    .with_width(list_width)
                    .finish(),
            )
            .with_child(rule(&v.theme, true))
            .with_child(Expanded::new(1.0, detail(v, app)).finish())
            .finish()
    };
    let mut content = col()
        .with_spacing(0.0)
        .with_child(header)
        .with_child(pad(toolbar, 20.0, 12.0));
    if !v.errors.is_empty() {
        content=content.with_child(pad(row().with_child(render_icon(AppIcon::TriangleAlert,v.theme.ui_yellow(),14.0)).with_child(Expanded::new(1.0,label(v.ui,"Some review data could not be refreshed. Other results remain available.",12.0,v.theme.sub_text())).finish()).with_child(btn(v,"errors","Details",ButtonKind::Ghost,Some(Action::ShowError))).finish(),24.0,5.0));
    }
    if let Some(message) = &v.message {
        content = content.with_child(pad(label(v.ui, message, 12.0, v.theme.accent), 24.0, 5.0));
    }
    content = content
        .with_child(rule(&v.theme, false))
        .with_child(Expanded::new(1.0, body).finish());
    let body = Container::new(content.finish())
        .with_background_color(v.theme.solid_bg())
        .finish();
    if let Some(log) = &v.check_log {
        let catcher = EventHandler::new(
            Container::new(Empty::new().finish())
                .with_background_color(ColorU::new(0, 0, 0, 150))
                .finish(),
        )
        .on_left_mouse_down({
            let id = v.id;
            move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, Action::CloseLog);
                DispatchEventResult::StopPropagation
            }
        })
        .finish();
        let dialog = EventHandler::new(check_log_dialog(v, log))
            .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
            .finish();
        let mut stack = Stack::new().with_child(body);
        stack.add_overlay_child(catcher);
        stack.add_overlay_child(
            Align::new(ConstrainedBox::new(dialog).with_width((v.width.get() - 80.0).clamp(320.0, 1000.0)).finish()).finish(),
        );
        return super::menus::overlay(v, stack.finish());
    }
    if v.form.is_none() && v.confirmation.is_none() {
        return super::menus::overlay(v, body);
    }
    if v.confirmation.is_none()
        && matches!(v.form, Some(Form::Create | Form::Edit | Form::Metadata(_)))
    {
        let mut stack = Stack::new().with_child(body);
        stack.add_overlay_child(pr_dialog(v));
        return super::menus::overlay(v, stack.finish());
    }
    let catcher = EventHandler::new(
        Container::new(Empty::new().finish())
            .with_background_color(ColorU::new(0, 0, 0, 150))
            .finish(),
    )
    .on_left_mouse_down({
        let id = v.id;
        move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, Action::Cancel);
            DispatchEventResult::StopPropagation
        }
    })
    .finish();
    let modal = EventHandler::new(if v.confirmation.is_some() {
        confirmation(v)
    } else {
        form(v)
    })
    .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
    .finish();
    let mut stack = Stack::new().with_child(body);
    stack.add_overlay_child(catcher);
    stack.add_overlay_child(
        Align::new(
            ConstrainedBox::new(modal)
                .with_width((v.width.get() - 40.0).min(660.0))
                .with_max_height(700.0)
                .finish(),
        )
        .finish(),
    );
    super::menus::overlay(v, stack.finish())
}
fn inbox(v: &PullRequestsView) -> Box<dyn Element> {
    let visible = v.visible_items();
    let header = pad(
        row()
            .with_child(semibold(v.ui, "Inbox", 12.0, v.theme.main_text()))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(label(
                v.ui,
                &format!("{} reviews", visible.len()),
                11.0,
                v.theme.sub_text(),
            ))
            .finish(),
        16.0,
        13.0,
    );
    let mut list = col().with_spacing(0.0);
    if visible.is_empty() {
        list = list.with_child(pad(
            label(
                v.ui,
                if v.loading {
                    "Loading reviews…"
                } else {
                    "No reviews match these filters."
                },
                12.0,
                v.theme.sub_text(),
            ),
            16.0,
            24.0,
        ));
    }
    for p in visible {
        let key = p.key();
        let chosen = v.selected.as_deref() == Some(&key);
        let theme = v.theme;
        let ui = v.ui;
        let title = text(&p.data, "title").to_string();
        let reference = format!(
            "{} · {}{}",
            p.target.label,
            if is_gitlab(&p.data) { "!" } else { "#" },
            p.number()
        );
        let author = login(&p.data).to_string();
        let author_avatar = super::avatars::photo_url(&p.data["author"], &p.data);
        let age = since(text(&p.data, "updatedAt"));
        let (status, _) = attention(&p.data);
        let status = status.to_string();
        let color = if status == "Check requirements" {
            theme.sub_text()
        } else {
            ready_color(v, &status)
        };
        let state = state_color(v, &p.data);
        let id = v.id;
        let action = key.clone();
        let item = Hoverable::new(v.mouse(&format!("pr-{key}")), move |mouse| {
            Container::new(pad(
                col()
                    .with_spacing(7.0)
                    .with_child(
                        row()
                            .with_child(render_icon(AppIcon::PullRequest, state, 14.0))
                            .with_child(
                                Expanded::new(1.0, semibold(ui, &title, 13.0, theme.main_text()))
                                    .finish(),
                            )
                            .finish(),
                    )
                    .with_child(one_line(ui, &reference, 11.0, theme.sub_text()))
                    .with_child(
                        row()
                            .with_child(super::avatars::render(
                                theme,
                                ui,
                                &author,
                                &author_avatar,
                                18.0,
                            ))
                            .with_child(
                                Expanded::new(1.0, one_line(ui, &author, 11.0, theme.sub_text()))
                                    .finish(),
                            )
                            .with_child(label(ui, &age, 11.0, theme.disabled_text()))
                            .finish(),
                    )
                    .with_child(
                        row()
                            .with_child(render_icon(
                                if status == "Ready to merge" {
                                    AppIcon::CircleCheck
                                } else if status == "Checks failing" || status == "Conflicts" {
                                    AppIcon::CircleX
                                } else {
                                    AppIcon::Clock
                                },
                                color,
                                11.0,
                            ))
                            .with_child(one_line(ui, &status, 11.0, color))
                            .finish(),
                    )
                    .finish(),
                14.0,
                13.0,
            ))
            .with_background_color(if chosen {
                theme.surface_3()
            } else if mouse.is_hovered() {
                theme.surface_2()
            } else {
                theme.surface_1()
            })
            .with_border(Border::left(2.0).with_border_color(if chosen {
                theme.accent
            } else {
                ColorU::transparent_black()
            }))
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(id, Action::Select(action.clone()))
        })
        .finish();
        list = list.with_child(item).with_child(rule(&v.theme, false));
    }
    if v.limit < 500 {
        list = list.with_child(pad(
            btn(
                v,
                "more",
                "Load more reviews",
                ButtonKind::Ghost,
                (!v.loading).then_some(Action::LoadMore),
            ),
            12.0,
            12.0,
        ));
    }
    Container::new(
        col()
            .with_spacing(0.0)
            .with_child(header)
            .with_child(rule(&v.theme, false))
            .with_child(Expanded::new(1.0, scroll(v, &v.list_scroll, list.finish())).finish())
            .finish(),
    )
    .with_background_color(v.theme.surface_1())
    .finish()
}
fn detail(v: &PullRequestsView, app: &AppContext) -> Box<dyn Element> {
    let Some(p) = &v.detail else {
        return empty(v,if v.detail_loading{"Loading review context…"}else{"Choose a review"},"Open a pull or merge request to see its description, code, checks, conversations and merge requirements.");
    };
    let d = &p.data;
    let pr_number = p.number();
    let open = text(d, "state") == "OPEN";
    let top = row()
        .with_child(pill(
            v,
            if d["isDraft"].as_bool().unwrap_or(false) {
                "Draft"
            } else {
                match text(d, "state") {
                    "OPEN" => "Open",
                    "MERGED" => "Merged",
                    _ => "Closed",
                }
            },
            state_color(v, d),
        ))
        .with_child(
            Expanded::new(
                1.0,
                one_line(
                    v.ui,
                    &format!(
                        "{} / {}{}",
                        p.target.label,
                        if is_gitlab(d) { "!" } else { "#" },
                        pr_number
                    ),
                    12.0,
                    v.theme.sub_text(),
                ),
            )
            .finish(),
        )
        .with_child(icon_button(
            v,
            "copy-link",
            AppIcon::Copy,
            "Copy review link",
            Some(Action::Copy(text(d, "url").into())),
        ))
        .with_child(icon_button(
            v,
            "open-link",
            AppIcon::Asset("icons/external-link.svg"),
            &format!("Open on {}", if is_gitlab(d) { "GitLab" } else { "GitHub" }),
            Some(Action::Link(text(d, "url").into())),
        ));
    let mut header_col = col().with_spacing(10.0);
    if v.width.get() < 1050.0 {
        header_col = header_col.with_child(
            row()
                .with_child(btn(
                    v,
                    "back-inbox",
                    "← Inbox",
                    ButtonKind::Ghost,
                    Some(Action::BackToInbox),
                ))
                .finish(),
        );
    }
    let header = pad(
        header_col
            .with_child(top.finish())
            .with_child(semibold(v.ui, text(d, "title"), 21.0, v.theme.main_text()))
            .with_child(
                row()
                    .with_child(render_icon(AppIcon::GitBranch, v.theme.sub_text(), 13.0))
                    .with_child(one_line(
                        v.ui,
                        &format!("{}  →  {}", text(d, "headRefName"), text(d, "baseRefName")),
                        12.0,
                        v.theme.sub_text(),
                    ))
                    .with_child(user_avatar(v, &d["author"], d, 20.0))
                    .with_child(label(
                        v.ui,
                        &format!("by {}", login(d)),
                        11.0,
                        v.theme.sub_text(),
                    ))
                    .finish(),
            )
            .with_child(
                row()
                    .with_child(button(
                        &v.theme,
                        v.ui,
                        v.id,
                        v.mouse("agent-review"),
                        Some(AppIcon::Agent),
                        "Review with agent",
                        ButtonKind::Secondary,
                        (!v.busy).then_some(Action::Handoff(
                            "Review the code and identify actionable findings".into(),
                        )),
                    ))
                    .with_child(button(
                        &v.theme,
                        v.ui,
                        v.id,
                        v.mouse("workspace"),
                        Some(AppIcon::Terminal),
                        "Open workspace",
                        ButtonKind::Ghost,
                        (!v.busy).then_some(Action::OpenWorkspace),
                    ))
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(icon_button(
                        v,
                        "edit-pr",
                        AppIcon::Edit,
                        "Edit review details",
                        open.then_some(Action::Edit),
                    ))
                    .finish(),
            )
            .finish(),
        20.0,
        16.0,
    );
    let mut tabs = row();
    for (tab, title) in [
        (Tab::Overview, "Overview"),
        (Tab::Files, "Files"),
        (Tab::Commits, "Commits"),
        (Tab::Checks, "Checks"),
        (Tab::Activity, "Activity"),
    ] {
        let count = match tab {
            Tab::Files => number(d, "changedFiles") as usize,
            Tab::Commits => array(d, "commits").len(),
            Tab::Checks => array(d, "statusCheckRollup").len(),
            Tab::Activity => array(d, "comments").len() + v.threads.len(),
            _ => 0,
        };
        let title = if count > 0 {
            format!("{title} {count}")
        } else {
            title.into()
        };
        tabs = tabs.with_child(tab_button(
            v,
            &format!("tab-{tab:?}"),
            &title,
            v.tab == tab,
            Action::Tab(tab),
        ));
    }
    let body = match v.tab {
        Tab::Overview => scroll(v, &v.detail_scroll, overview(v, p, app)),
        Tab::Files => files(v, p),
        Tab::Commits => scroll(v, &v.detail_scroll, commits(v, p)),
        Tab::Checks => scroll(v, &v.detail_scroll, checks(v, p)),
        Tab::Activity => scroll(v, &v.detail_scroll, activity(v, p, app)),
    };
    let mut area = col()
        .with_spacing(0.0)
        .with_child(header)
        .with_child(pad(tabs.finish(), 8.0, 0.0))
        .with_child(rule(&v.theme, false))
        .with_child(Expanded::new(1.0, body).finish());
    if matches!(v.tab, Tab::Activity | Tab::Files) && (v.tab == Tab::Activity || v.commit.is_none())
    {
        area = area
            .with_child(rule(&v.theme, false))
            .with_child(composer(v, p));
    }
    area.finish()
}
fn overview(v: &PullRequestsView, p: &Item, app: &AppContext) -> Box<dyn Element> {
    let d = &p.data;
    let n = p.number();
    let (status, why) = attention(d);
    let color = ready_color(v, status);
    let unresolved = v
        .threads
        .iter()
        .filter(|t| !t["isResolved"].as_bool().unwrap_or(false))
        .count();
    let mut status_row = row()
        .with_child(render_icon(
            if status == "Ready to merge" {
                AppIcon::CircleCheck
            } else if status == "Checks failing" || status == "Conflicts" {
                AppIcon::CircleX
            } else {
                AppIcon::Clock
            },
            color,
            18.0,
        ))
        .with_child(
            Expanded::new(
                1.0,
                col()
                    .with_spacing(4.0)
                    .with_child(semibold(v.ui, status, 14.0, v.theme.main_text()))
                    .with_child(label(v.ui, why, 12.0, v.theme.sub_text()))
                    .finish(),
            )
            .finish(),
        );
    if status == "Checks failing" || status == "Conflicts" {
        status_row = status_row.with_child(btn(
            v,
            "resolve-blocker",
            if status == "Conflicts" {
                "Resolve with agent"
            } else {
                "Investigate"
            },
            ButtonKind::Secondary,
            Some(Action::Handoff(
                if status == "Conflicts" {
                    "Resolve the merge conflicts, preserving intended changes on both sides"
                } else {
                    "Investigate and fix the failing checks"
                }
                .into(),
            )),
        ));
    }
    let mut next = col().with_spacing(8.0).with_child(status_row.finish());
    if unresolved > 0 {
        next = next.with_child(
            row()
                .with_child(render_icon(
                    AppIcon::MessageSquare,
                    v.theme.sub_text(),
                    12.0,
                ))
                .with_child(label(
                    v.ui,
                    &format!(
                        "{unresolved} unresolved thread{}",
                        if unresolved == 1 { "" } else { "s" }
                    ),
                    12.0,
                    v.theme.sub_text(),
                ))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(btn(
                    v,
                    "fix-feedback",
                    "Address feedback",
                    ButtonKind::Ghost,
                    Some(Action::Handoff("Address unresolved review findings".into())),
                ))
                .finish(),
        );
    }
    let mut content = col().with_spacing(14.0).with_child(card(v, next.finish()));
    let description = if text(d, "body").trim().is_empty() {
        label(
            v.ui,
            if v.detail_loading && d.get("body").is_none() {
                "Loading description…"
            } else {
                "No description provided."
            },
            13.0,
            v.theme.sub_text(),
        )
    } else {
        crate::terminal::render::markdown::render_markdown_document_with_links(
            text(d, "body"),
            v.theme,
            v.ui,
            v.mono,
            13.0,
            1.45,
            v.id,
            Action::Link,
        )
    };
    content = content.with_child(card(
        v,
        col()
            .with_child(semibold(v.ui, "Description", 14.0, v.theme.main_text()))
            .with_child(rule(&v.theme, false))
            .with_child(description)
            .finish(),
    ));
    let stats = row()
        .with_child(pill(
            v,
            &format!("{} files", number(d, "changedFiles")),
            v.theme.sub_text(),
        ))
        .with_child(pill(
            v,
            &format!("+{}", number(d, "additions")),
            v.theme.ui_green(),
        ))
        .with_child(pill(
            v,
            &format!("−{}", number(d, "deletions")),
            v.theme.ui_red(),
        ))
        .finish();
    let mut sidebar=col().with_spacing(16.0)
        .with_child(row().with_child(semibold(v.ui,"Details",13.0,v.theme.main_text()))
            .with_child(Expanded::new(1.0,Empty::new().finish()).finish())
            .with_child(icon_button(v,"copy-review-summary",AppIcon::Copy,"Copy review summary",Some(Action::Copy(format!("{} #{}: {}\n{}\nHead: {}\nStatus: {}\n{} files · +{} / -{}\n{} unresolved review threads",p.target.label,n,text(d,"title"),text(d,"url"),text(d,"headRefOid"),status,number(d,"changedFiles"),number(d,"additions"),number(d,"deletions"),unresolved))))).finish())
        .with_child(stats)
        .with_child(label(v.ui,&format!("Updated {}",since(text(d,"updatedAt"))),11.0,v.theme.sub_text()))
        .with_child(rule(&v.theme,false));
    for (field, key, title) in [
        (MetadataField::Reviewer, "reviewRequests", "Reviewers"),
        (MetadataField::Assignee, "assignees", "Assignees"),
        (MetadataField::Label, "labels", "Labels"),
    ] {
        let mut c = col().with_spacing(8.0).with_child(
            row()
                .with_child(semibold(v.ui, title, 13.0, v.theme.main_text()))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(icon_button(
                    v,
                    &format!("add-{key}"),
                    AppIcon::Plus,
                    &format!("Add {}", title.to_lowercase()),
                    Some(Action::Metadata(field)),
                ))
                .finish(),
        );
        let values = array(d, key);
        if values.is_empty() {
            c = c.with_child(label(v.ui, "None yet", 12.0, v.theme.sub_text()));
        }
        for (i, value) in values.iter().enumerate() {
            let name = if field == MetadataField::Label {
                text(value, "name")
            } else {
                value["login"]
                    .as_str()
                    .or_else(|| value["name"].as_str())
                    .or_else(|| value["requestedReviewer"]["login"].as_str())
                    .unwrap_or("Reviewer")
            };
            let mut person_row = row();
            if field != MetadataField::Label {
                let person = if value["requestedReviewer"].is_object() {
                    &value["requestedReviewer"]
                } else {
                    value
                };
                person_row = person_row.with_child(user_avatar(v, person, d, 22.0));
            }
            c = c.with_child(
                person_row
                    .with_child(
                        Expanded::new(1.0, one_line(v.ui, name, 12.0, v.theme.main_text()))
                            .finish(),
                    )
                    .with_child(icon_button(
                        v,
                        &format!("remove-{key}-{i}"),
                        AppIcon::XClose,
                        "Remove",
                        Some(Action::RemoveMetadata(field, name.into())),
                    ))
                    .finish(),
            );
        }
        sidebar = sidebar
            .with_child(c.finish())
            .with_child(rule(&v.theme, false));
    }
    if text(d, "state") == "OPEN" {
        let method = match v.merge_method {
            MergeMethod::Squash => "Squash and merge",
            MergeMethod::Rebase => "Rebase and merge",
            MergeMethod::Merge => {
                if is_gitlab(d) {
                    "Project merge method"
                } else {
                    "Merge commit"
                }
            }
        };
        let mut merging = col()
            .with_spacing(12.0)
            .with_child(
                row()
                    .with_child(render_icon(AppIcon::GitCompare, v.theme.sub_text(), 15.0))
                    .with_child(semibold(v.ui, "Merge options", 14.0, v.theme.main_text()))
                    .finish(),
            )
            .with_child(label(
                v.ui,
                "Repository rules apply. Only the commit you reviewed can be merged.",
                12.0,
                v.theme.sub_text(),
            ))
            .with_child(
                row()
                    .with_child(super::menus::trigger(
                        v,
                        Picker::Merge,
                        method,
                        AppIcon::GitCompare,
                    ))
                    .finish(),
            );
        let draft = d["isDraft"].as_bool().unwrap_or(false);
        let mut actions = row();
        if draft {
            actions = actions.with_child(btn(
                v,
                "ready",
                "Mark ready for review",
                ButtonKind::Primary,
                Some(Action::Run(Operation::Draft {
                    number: n,
                    draft: false,
                })),
            ));
        } else {
            actions = actions
                .with_child(btn(
                    v,
                    "merge",
                    if is_gitlab(d) {
                        "Merge request…"
                    } else {
                        "Merge pull request…"
                    },
                    ButtonKind::Primary,
                    (status == "Ready to merge" && merge_allowed(&v.repository, v.merge_method))
                        .then_some(Action::Ask(Operation::Merge {
                            number: n,
                            head: text(d, "headRefOid").into(),
                            method: v.merge_method,
                            auto: false,
                        })),
                ))
                .with_child(btn(
                    v,
                    "auto-merge",
                    if d["autoMergeRequest"].is_object() {
                        "Disable auto-merge"
                    } else {
                        "Enable auto-merge…"
                    },
                    ButtonKind::Secondary,
                    Some(if d["autoMergeRequest"].is_object() {
                        Action::Run(Operation::DisableAutoMerge { number: n })
                    } else {
                        Action::Ask(Operation::Merge {
                            number: n,
                            head: text(d, "headRefOid").into(),
                            method: v.merge_method,
                            auto: true,
                        })
                    }),
                ));
        }
        merging = merging.with_child(actions.finish());
        if !is_gitlab(d)
            && text(&v.repository, "viewerPermission") == "ADMIN"
            && !draft
            && status != "Ready to merge"
        {
            merging = merging.with_child(
                row()
                    .with_child(btn(
                        v,
                        "override-merge",
                        "Administrator merge…",
                        ButtonKind::Danger,
                        Some(Action::Ask(Operation::OverrideMerge {
                            number: n,
                            head: text(d, "headRefOid").into(),
                            method: v.merge_method,
                            acknowledgement: String::new(),
                        })),
                    ))
                    .finish(),
            );
        }

        merging = merging.with_child(
            row()
                .with_child(btn(
                    v,
                    "update-branch",
                    if is_gitlab(d) {
                        "Rebase onto target…"
                    } else {
                        "Update from base…"
                    },
                    ButtonKind::Ghost,
                    Some(Action::Ask(Operation::UpdateBranch {
                        number: n,
                        head: text(d, "headRefOid").into(),
                    })),
                ))
                .with_child(btn(
                    v,
                    "draft",
                    "Convert to draft",
                    ButtonKind::Ghost,
                    (!draft).then_some(Action::Run(Operation::Draft {
                        number: n,
                        draft: true,
                    })),
                ))
                .with_child(btn(
                    v,
                    "close-pr",
                    if is_gitlab(d) {
                        "Close merge request…"
                    } else {
                        "Close pull request…"
                    },
                    ButtonKind::Danger,
                    Some(Action::Ask(Operation::SetState {
                        number: n,
                        open: false,
                    })),
                ))
                .finish(),
        );
        content = content.with_child(card(v, merging.finish()));
    } else if text(d, "state") == "CLOSED" {
        content = content.with_child(btn(
            v,
            "reopen",
            if is_gitlab(d) {
                "Reopen merge request"
            } else {
                "Reopen pull request"
            },
            ButtonKind::Secondary,
            Some(Action::Run(Operation::SetState {
                number: n,
                open: true,
            })),
        ));
    }
    let sidebar = card(v, sidebar.finish());
    let layout = if v.width.get() >= 1300.0 {
        row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(16.0)
            .with_child(Expanded::new(1.0, content.finish()).finish())
            .with_child(ConstrainedBox::new(sidebar).with_width(232.0).finish())
            .finish()
    } else {
        content.with_child(sidebar).finish()
    };
    pad(layout, 20.0, 18.0)
}
fn files(v: &PullRequestsView, p: &Item) -> Box<dyn Element> {
    if v.detail_loading && v.files.is_empty() {
        return empty(v,"Loading changed files…","The review overview is ready. File diffs will appear as soon as the provider returns them.");
    }
    let values = v.file_values();
    let mut list = col().with_spacing(3.0).with_child(pad(
        col()
            .with_spacing(6.0)
            .with_child(semibold(v.ui, "CHANGED FILES", 11.0, v.theme.sub_text()))
            .with_child(
                row()
                    .with_child(chip(v, "tree", "Tree", v.tree, Action::Tree))
                    .with_child(chip(
                        v,
                        "hide-viewed",
                        "Hide viewed",
                        v.hide_viewed,
                        Action::HideViewed,
                    ))
                    .finish(),
            )
            .finish(),
        10.0,
        10.0,
    ));
    let mut ordered = values
        .iter()
        .filter(|f| {
            !v.hide_viewed
                || !v.viewed.contains(&format!(
                    "{}:{}:{}",
                    p.key(),
                    v.pending_head,
                    text(f, "filename")
                ))
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|f| text(f, "filename"));
    if v.tree {
        list = list.with_child(file_tree(v, p, &ordered, "", 0));
    } else {
        for f in ordered {
            list = list.with_child(file_button(v, p, f, text(f, "filename")));
        }
    }
    let selected = values
        .iter()
        .find(|f| Some(text(f, "filename")) == v.selected_file.as_deref());
    let code = if let Some(f) = selected {
        diff(v, p, f)
    } else {
        empty(v,"No file selected","Choose a changed file to review its diff. Binary or oversized diffs can be opened on the provider.")
    };
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            ConstrainedBox::new(scroll(v, &v.files_scroll, list.finish()))
                .with_width(if v.width.get() < 1150.0 { 170.0 } else { 220.0 })
                .finish(),
        )
        .with_child(rule(&v.theme, true))
        .with_child(Expanded::new(1.0, code).finish())
        .finish()
}
fn file_button(v: &PullRequestsView, p: &Item, f: &Value, title: &str) -> Box<dyn Element> {
    let path = text(f, "filename");
    let viewed = v
        .viewed
        .contains(&format!("{}:{}:{}", p.key(), v.pending_head, path));
    chip(
        v,
        &format!("file-{path}"),
        &format!("{}{}", if viewed { "✓ " } else { "" }, title),
        v.selected_file.as_deref() == Some(path),
        Action::File(path.into()),
    )
}
fn file_tree(
    v: &PullRequestsView,
    p: &Item,
    files: &[&Value],
    parent: &str,
    depth: usize,
) -> Box<dyn Element> {
    let mut column = col().with_spacing(2.0);
    let mut directories = std::collections::BTreeSet::new();
    let prefix = if parent.is_empty() {
        String::new()
    } else {
        format!("{parent}/")
    };
    for f in files {
        if let Some(rest) = text(f, "filename").strip_prefix(&prefix) {
            if let Some((directory, _)) = rest.split_once('/') {
                directories.insert(directory);
            }
        }
    }
    for directory in directories {
        let path = format!("{prefix}{directory}");
        let closed = v.collapsed_dirs.contains(&path);
        column = column.with_child(btn(
            v,
            &format!("dir-{path}"),
            &format!(
                "{}{} {}",
                "  ".repeat(depth),
                if closed { "▸" } else { "▾" },
                directory
            ),
            ButtonKind::Ghost,
            Some(Action::ToggleDirectory(path.clone())),
        ));
        if !closed {
            column = column.with_child(file_tree(v, p, files, &path, depth + 1));
        }
    }
    for f in files {
        let path = text(f, "filename");
        if path.rsplit_once('/').map(|x| x.0).unwrap_or("") == parent {
            let title = format!(
                "{}{}",
                "  ".repeat(depth),
                path.rsplit('/').next().unwrap_or(path)
            );
            column = column.with_child(file_button(v, p, f, &title));
        }
    }
    column.finish()
}
fn diff(v: &PullRequestsView, p: &Item, f: &Value) -> Box<dyn Element> {
    let path = text(f, "filename");
    let viewed = v
        .viewed
        .contains(&format!("{}:{}:{}", p.key(), v.pending_head, path));
    let mut bar = col()
        .with_spacing(5.0)
        .with_child(one_line(v.ui, path, 12.5, v.theme.main_text()))
        .with_child(
            row()
                .with_child(label(
                    v.ui,
                    &format!(
                        "{}  +{} −{}",
                        text(f, "status"),
                        number(f, "additions"),
                        number(f, "deletions")
                    ),
                    11.0,
                    v.theme.sub_text(),
                ))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                .with_child(chip(
                    v,
                    "split",
                    if v.split { "Split" } else { "Unified" },
                    v.split,
                    Action::Split,
                ))
                .with_child(chip(
                    v,
                    "viewed",
                    if viewed { "✓ Viewed" } else { "Mark viewed" },
                    viewed,
                    Action::Viewed(path.into()),
                ))
                .finish(),
        );
    if let Some(sha) = &v.commit {
        bar = bar.with_child(
            row()
                .with_child(label(
                    v.ui,
                    &format!("Commit {} · read-only", &sha[..sha.len().min(8)]),
                    11.0,
                    v.theme.accent,
                ))
                .with_child(btn(
                    v,
                    "whole-pr",
                    "All changes",
                    ButtonKind::Ghost,
                    Some(Action::Commit(None)),
                ))
                .finish(),
        );
    }
    if !text(f, "previous_filename").is_empty() {
        bar = bar.with_child(label(
            v.ui,
            &format!("Renamed from {}", text(f, "previous_filename")),
            11.0,
            v.theme.sub_text(),
        ));
    }
    if !text(f, "blob_url").is_empty() {
        bar = bar.with_child(
            row()
                .with_child(btn(
                    v,
                    "open-file-github",
                    "Open full file ↗",
                    ButtonKind::Ghost,
                    Some(Action::Link(text(f, "blob_url").into())),
                ))
                .finish(),
        );
    }
    let patch = text(f, "patch");
    let body = if patch.is_empty() && super::images::supported(path) && !text(f, "sha").is_empty() {
        let mut content = col()
            .with_child(label(
                v.ui,
                "Image change · read-only",
                13.0,
                v.theme.sub_text(),
            ))
            .with_child(
                row()
                    .with_child(btn(
                        v,
                        "image-preview",
                        if v.image_preview {
                            "Hide image"
                        } else {
                            "Preview image"
                        },
                        ButtonKind::Secondary,
                        Some(Action::PreviewImage),
                    ))
                    .with_child(btn(
                        v,
                        "image-github",
                        "Open full file ↗",
                        ButtonKind::Ghost,
                        Some(Action::Link(text(f, "blob_url").into())),
                    ))
                    .finish(),
            );
        if v.image_preview {
            content = content.with_child(
                ConstrainedBox::new(
                    warpui::elements::Image::new(
                        super::images::source(p.target.clone(), text(f, "sha")),
                        warpui_core::image_cache::CacheOption::Original,
                    )
                    .contain()
                    .before_load(label(
                        v.ui,
                        "Loading image from the project machine…",
                        12.0,
                        v.theme.sub_text(),
                    ))
                    .on_load_failure(label(
                        v.ui,
                        "This image could not be previewed. Open the full file on the provider.",
                        12.0,
                        v.theme.sub_text(),
                    ))
                    .finish(),
                )
                .with_height(380.0)
                .finish(),
            );
        }
        pad(content.finish(), 18.0, 20.0)
    } else if patch.is_empty() {
        empty(v,"No text diff available","This file is binary, unchanged in this comparison, or too large for the provider's patch response. Open the full file to inspect it.")
    } else {
        let mut lines = col().with_spacing(0.0);
        let parsed = patch_lines(patch);
        if v.split {
            lines = lines.with_child(pad(
                row()
                    .with_child(
                        Expanded::new(1.0, label(v.ui, "Original", 11.0, v.theme.sub_text()))
                            .finish(),
                    )
                    .with_child(
                        Expanded::new(1.0, label(v.ui, "Proposed", 11.0, v.theme.sub_text()))
                            .finish(),
                    )
                    .finish(),
                10.0,
                6.0,
            ));
            for (left, right) in split_patch(&parsed).into_iter().take(v.diff_limit) {
                if left.as_ref().is_some_and(|l| l.kind == '@') {
                    lines = lines.with_child(diff_cell(v, p, path, left.as_ref(), "", false));
                    continue;
                }
                lines = lines.with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(
                            Expanded::new(1.0, diff_cell(v, p, path, left.as_ref(), "LEFT", false))
                                .finish(),
                        )
                        .with_child(rule(&v.theme, true))
                        .with_child(
                            Expanded::new(
                                1.0,
                                diff_cell(v, p, path, right.as_ref(), "RIGHT", false),
                            )
                            .finish(),
                        )
                        .finish(),
                );
            }
        } else {
            for line in parsed.iter().take(v.diff_limit) {
                lines = lines.with_child(diff_cell(
                    v,
                    p,
                    path,
                    Some(line),
                    if line.new.is_some() { "RIGHT" } else { "LEFT" },
                    true,
                ));
            }
        }
        if parsed.len() > v.diff_limit {
            lines = lines.with_child(btn(
                v,
                "more-diff",
                "Show 500 more lines",
                ButtonKind::Secondary,
                Some(Action::MoreDiff),
            ));
        }

        scroll(v, &v.diff_scroll, lines.finish())
    };
    bar = bar.with_child(label(
        v.ui,
        if v.commit.is_some() {
            "Commit comparison is read-only. Choose All changes to comment."
        } else {
            "Click + or a diff line to comment. Select a line range in the editor below."
        },
        11.0,
        v.theme.sub_text(),
    ));
    col()
        .with_spacing(0.0)
        .with_child(pad(bar.finish(), 12.0, 10.0))
        .with_child(rule(&v.theme, false))
        .with_child(Expanded::new(1.0, body).finish())
        .finish()
}
fn diff_cell(
    v: &PullRequestsView,
    p: &Item,
    path: &str,
    line: Option<&DiffLine>,
    side: &str,
    unified: bool,
) -> Box<dyn Element> {
    let Some(line) = line else {
        return Container::new(Empty::new().finish())
            .with_background_color(v.theme.surface_1())
            .finish();
    };
    let n = if side == "LEFT" { line.old } else { line.new };
    let selected = v.line.as_ref().is_some_and(|c| {
        c.path == path
            && c.side == side
            && n.is_some_and(|n| n >= c.start_line.unwrap_or(c.line) && n <= c.line)
    });
    let bg = if selected {
        tint(v.theme.accent, 36)
    } else {
        match line.kind {
            '+' => tint(v.theme.ui_green(), 17),
            '-' => tint(v.theme.ui_red(), 18),
            '@' => tint(v.theme.accent, 15),
            _ => ColorU::transparent_black(),
        }
    };
    let numbers = if unified {
        format!(
            "{:>4} {:>4}",
            line.old.map(|n| n.to_string()).unwrap_or_default(),
            line.new.map(|n| n.to_string()).unwrap_or_default()
        )
    } else {
        format!("{:>4}", n.map(|n| n.to_string()).unwrap_or_default())
    };
    let clipped = line
        .text
        .char_indices()
        .nth(2000)
        .map(|(cut, _)| format!("{} … [open full file for the rest]", &line.text[..cut]));
    let source_text = clipped.as_deref().unwrap_or(&line.text);
    let source: Box<dyn Element> = if line.kind == '@' {
        label(v.mono, &line.text, 11.5, v.theme.accent)
    } else {
        crate::workspace::highlighted_source(
            source_text,
            dragent_editor::Language::from_path(std::path::Path::new(path)),
            &v.theme,
            v.mono,
            crate::theme::text_size(11.5),
            warpui::fonts::Weight::Normal,
            false,
        )
        .finish()
    };
    let commentable = line.kind != '@' && v.commit.is_none() && n.is_some();
    let marker = if commentable {
        let theme = v.theme;
        let ui = v.ui;
        Hoverable::new(
            v.mouse(&format!("comment-line-{path}-{side}-{}", n.unwrap())),
            move |mouse| {
                crate::machine_ui::with_tooltip(
                    &theme,
                    ui,
                    render_icon(
                        AppIcon::Plus,
                        if mouse.is_hovered() {
                            theme.accent
                        } else {
                            theme.sub_text()
                        },
                        12.0,
                    ),
                    mouse.is_hovered(),
                    "Add line comment",
                )
            },
        )
        .with_cursor(Cursor::PointingHand)
        .finish()
    } else {
        ConstrainedBox::new(Empty::new().finish())
            .with_width(12.0)
            .finish()
    };
    let element = Container::new(
        row()
            .with_spacing(8.0)
            .with_child(marker)
            .with_child(label(v.mono, &numbers, 10.5, v.theme.sub_text()))
            .with_child(label(
                v.mono,
                &line.kind.to_string(),
                11.5,
                v.theme.sub_text(),
            ))
            .with_child(Expanded::new(1.0, source).finish())
            .finish(),
    )
    .with_background_color(bg)
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .finish();
    if commentable {
        let id = v.id;
        let action = Action::Line {
            path: path.into(),
            line: n.unwrap(),
            side: side.into(),
        };
        EventHandler::new(element)
            .on_left_mouse_down(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    } else {
        element
    }
}
fn commits(v: &PullRequestsView, p: &Item) -> Box<dyn Element> {
    if v.detail_loading && array(&p.data, "commits").is_empty() {
        return empty(
            v,
            "Loading commit history…",
            "Fetching commits for this review.",
        );
    }
    let mut c = col().with_child(
        row()
            .with_child(semibold(v.ui, "Commit history", 16.0, v.theme.main_text()))
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(btn(
                v,
                "all-commits",
                "All changes",
                ButtonKind::Secondary,
                Some(Action::Commit(None)),
            ))
            .finish(),
    );
    for commit in array(&p.data, "commits") {
        let sha = text(commit, "oid");
        let title = text(commit, "messageHeadline");
        c = c.with_child(card(
            v,
            col()
                .with_spacing(8.0)
                .with_child(semibold(v.ui, title, 13.0, v.theme.main_text()))
                .with_child(label(
                    v.ui,
                    &format!(
                        "{} · {}",
                        &sha[..sha.len().min(8)],
                        text(commit, "committedDate")
                    ),
                    11.5,
                    v.theme.sub_text(),
                ))
                .with_child(
                    row()
                        .with_child(btn(
                            v,
                            &format!("commit-{sha}"),
                            "Review this commit",
                            ButtonKind::Secondary,
                            Some(Action::Commit(Some(sha.into()))),
                        ))
                        .with_child(btn(
                            v,
                            &format!("copy-{sha}"),
                            "Copy SHA",
                            ButtonKind::Ghost,
                            Some(Action::Copy(sha.into())),
                        ))
                        .finish(),
                )
                .finish(),
        ));
    }
    pad(c.finish(), 24.0, 20.0)
}
fn checks(v: &PullRequestsView, p: &Item) -> Box<dyn Element> {
    let checks = array(&p.data, "statusCheckRollup");
    let mut c = col()
        .with_spacing(14.0)
        .with_child(semibold(v.ui, "Checks", 16.0, v.theme.main_text()))
        .with_child(label(
            v.ui,
            "Results for the latest commit. Skipped checks are kept separate from passing checks.",
            12.0,
            v.theme.sub_text(),
        ));
    if checks.is_empty() {
        return pad(
            c.with_child(card(
                v,
                label(
                    v.ui,
                    "No checks have been reported for this commit.",
                    13.0,
                    v.theme.sub_text(),
                ),
            ))
            .finish(),
            20.0,
            18.0,
        );
    }
    let mut counts = row();
    for (state, title, color) in [
        (CheckState::Pass, "passed", v.theme.ui_green()),
        (CheckState::Fail, "failed", v.theme.ui_red()),
        (CheckState::Running, "running", v.theme.ui_yellow()),
        (CheckState::Skipped, "skipped", v.theme.sub_text()),
    ] {
        let count = checks
            .iter()
            .filter(|check| check_state(check) == state)
            .count();
        if count > 0 {
            counts = counts.with_child(pill(v, &format!("{count} {title}"), color));
        }
    }
    c = c.with_child(counts.finish());
    let mut checks = checks.iter().collect::<Vec<_>>();
    checks.sort_by_key(|check| match check_state(check) {
        CheckState::Fail => 0,
        CheckState::Running => 1,
        CheckState::Pass => 2,
        CheckState::Skipped => 3,
    });
    let mut rows = col().with_spacing(0.0);
    for (i, check) in checks.iter().enumerate() {
        let (state, color, icon) = match check_state(check) {
            CheckState::Pass => ("Passed", v.theme.ui_green(), AppIcon::CircleCheck),
            CheckState::Fail => ("Failed", v.theme.ui_red(), AppIcon::CircleX),
            CheckState::Running => ("Running", v.theme.ui_yellow(), AppIcon::Clock),
            CheckState::Skipped => ("Skipped", v.theme.sub_text(), AppIcon::CircleX),
        };
        let name = if text(check, "name").is_empty() {
            text(check, "context")
        } else {
            text(check, "name")
        };
        let url = if text(check, "detailsUrl").is_empty() {
            text(check, "targetUrl")
        } else {
            text(check, "detailsUrl")
        };
        let run = check["runId"].as_u64().or_else(|| {
            url.split("/actions/runs/")
                .nth(1)
                .and_then(|s| s.split('/').next())
                .and_then(|s| s.parse::<u64>().ok())
        });
        let mut line = row()
            .with_child(render_icon(icon, color, 16.0))
            .with_child(
                Expanded::new(1.0, one_line(v.ui, name, 13.0, v.theme.main_text())).finish(),
            )
            .with_child(pill(v, state, color));
        if let Some(run) = run.filter(|_| check_state(check) == CheckState::Fail) {
            line = line
                .with_child(btn(
                    v,
                    &format!("logs-{i}"),
                    "View log",
                    ButtonKind::Ghost,
                    Some(Action::Run(Operation::Logs { run })),
                ))
                .with_child(btn(
                    v,
                    &format!("rerun-{i}"),
                    if is_gitlab(&p.data) {
                        "Retry job…"
                    } else {
                        "Rerun…"
                    },
                    ButtonKind::Secondary,
                    Some(Action::Ask(Operation::Rerun {
                        run,
                        failed_only: true,
                    })),
                ));
        }
        line = line.with_child(icon_button(
            v,
            &format!("check-{i}"),
            AppIcon::Asset("icons/external-link.svg"),
            "Open check details",
            (!url.is_empty()).then_some(Action::Link(url.into())),
        ));
        let mut item = col().with_spacing(4.0).with_child(line.finish());
        let timestamp = if text(check, "completedAt").is_empty() {
            text(check, "startedAt")
        } else {
            text(check, "completedAt")
        };
        if !timestamp.is_empty() {
            item = item.with_child(label(v.ui, &since(timestamp), 11.0, v.theme.sub_text()));
        }
        rows = rows.with_child(pad(item.finish(), 0.0, 12.0));
        if i + 1 < checks.len() {
            rows = rows.with_child(rule(&v.theme, false));
        }
    }
    pad(c.with_child(card(v, rows.finish())).finish(), 20.0, 18.0)
}
fn activity(v: &PullRequestsView, p: &Item, app: &AppContext) -> Box<dyn Element> {
    if v.detail_loading
        && array(&p.data, "comments").is_empty()
        && array(&p.data, "reviews").is_empty()
        && v.threads.is_empty()
    {
        return empty(
            v,
            "Loading discussion…",
            "Comments and review threads will appear as they arrive.",
        );
    }
    let mut c = col().with_child(semibold(
        v.ui,
        "Conversation & review threads",
        16.0,
        v.theme.main_text(),
    ));
    for comment in array(&p.data, "comments") {
        c = c.with_child(comment_card(v, comment, None, None, app));
    }
    for review in array(&p.data, "reviews") {
        c = c.with_child(card(
            v,
            col()
                .with_spacing(7.0)
                .with_child(
                    row()
                        .with_child(user_avatar(v, &review["author"], &p.data, 24.0))
                        .with_child(semibold(v.ui, login(review), 12.5, v.theme.main_text()))
                        .with_child(pill(v, text(review, "state"), v.theme.sub_text()))
                        .finish(),
                )
                .with_child(label(v.ui, text(review, "body"), 13.0, v.theme.main_text()))
                .finish(),
        ));
    }
    for thread in &v.threads {
        let resolved = thread["isResolved"].as_bool().unwrap_or(false);
        let outdated = thread["isOutdated"].as_bool().unwrap_or(false);
        let title = format!(
            "{}:{}{}",
            text(thread, "path"),
            number(thread, "line"),
            if outdated { " · outdated" } else { "" }
        );
        let mut tc = col().with_spacing(10.0).with_child(
            row()
                .with_child(
                    Expanded::new(1.0, one_line(v.ui, &title, 12.5, v.theme.accent)).finish(),
                )
                .with_child(pill(
                    v,
                    if resolved { "Resolved" } else { "Unresolved" },
                    if resolved {
                        v.theme.sub_text()
                    } else {
                        v.theme.ui_yellow()
                    },
                ))
                .finish(),
        );
        let nodes = thread["comments"]["nodes"].as_array();
        for comment in nodes.into_iter().flatten() {
            tc = tc.with_child(comment_card(
                v,
                comment,
                Some(number(comment, "databaseId")),
                Some(text(thread, "id")),
                app,
            ));
        }
        tc = tc.with_child(
            row()
                .with_child(btn(
                    v,
                    &format!("thread-file-{}", text(thread, "id")),
                    "View file",
                    ButtonKind::Ghost,
                    Some(Action::File(text(thread, "path").into())),
                ))
                .with_child(btn(
                    v,
                    &format!("resolve-{}", text(thread, "id")),
                    if resolved {
                        "Reopen thread"
                    } else {
                        "Resolve thread"
                    },
                    ButtonKind::Secondary,
                    Some(Action::Resolve {
                        thread: text(thread, "id").into(),
                        resolved: !resolved,
                    }),
                ))
                .finish(),
        );
        c = c.with_child(card(v, tc.finish()));
    }
    if array(&p.data, "comments").is_empty()
        && array(&p.data, "reviews").is_empty()
        && v.threads.is_empty()
    {
        c = c.with_child(label(
            v.ui,
            if text(&p.data, "state") == "OPEN" {
                "Start the conversation with a comment, or select a line in Files to draft a review."
            } else {
                "No comments or reviews yet. You can still add a conversation comment below."
            },
            13.0,
            v.theme.sub_text(),
        ));
    }
    pad(c.finish(), 24.0, 20.0)
}
fn comment_card(
    v: &PullRequestsView,
    c: &Value,
    reply: Option<u64>,
    thread: Option<&str>,
    app: &AppContext,
) -> Box<dyn Element> {
    let body = crate::terminal::render::markdown::render_markdown_document_with_links(
        text(c, "body"),
        v.theme,
        v.ui,
        v.mono,
        12.5,
        1.45,
        v.id,
        Action::Link,
    );
    let mut content = col()
        .with_spacing(9.0)
        .with_child(
            row()
                .with_child(user_avatar(
                    v,
                    &c["author"],
                    &v.detail.as_ref().map(|p| &p.data).unwrap_or(&Value::Null),
                    24.0,
                ))
                .with_child(semibold(v.ui, login(c), 12.5, v.theme.main_text()))
                .with_child(label(v.ui, text(c, "createdAt"), 10.5, v.theme.sub_text()))
                .finish(),
        )
        .with_child(body);
    if let Some(reply) = reply {
        content = content.with_child(btn(
            v,
            &format!("reply-{reply}"),
            "Reply",
            ButtonKind::Ghost,
            Some(Action::Reply { comment: reply }),
        ));
    }
    card(v, content.finish())
}
fn composer(v: &PullRequestsView, p: &Item) -> Box<dyn Element> {
    let mut c = col().with_spacing(8.0);
    if let Some(line) = &v.line {
        c = c.with_child(
            row()
                .with_child(one_line(
                    v.ui,
                    &format!("Comment on {}:{} ({})", line.path, line.line, line.side),
                    12.0,
                    v.theme.accent,
                ))
                .with_child(btn(
                    v,
                    "extend-line",
                    if v.extend_line {
                        "Click the last line…"
                    } else {
                        "Select a line range"
                    },
                    ButtonKind::Ghost,
                    Some(Action::ExtendLine),
                ))
                .with_child(btn(
                    v,
                    "cancel-line",
                    "Cancel",
                    ButtonKind::Ghost,
                    Some(Action::CancelReply),
                ))
                .finish(),
        );
    } else if v.reply.is_some() {
        c = c.with_child(
            row()
                .with_child(label(
                    v.ui,
                    "Replying to review thread",
                    12.0,
                    v.theme.accent,
                ))
                .with_child(btn(
                    v,
                    "cancel-reply",
                    "Cancel reply",
                    ButtonKind::Ghost,
                    Some(Action::CancelReply),
                ))
                .finish(),
        );
    }
    if !v.outdated_comments.is_empty() {
        c = c.with_child(
            row()
                .with_child(label(
                    v.ui,
                    &format!(
                        "{} draft comment(s) on an earlier head",
                        v.outdated_comments.len()
                    ),
                    11.5,
                    v.theme.ui_yellow(),
                ))
                .with_child(btn(
                    v,
                    "copy-outdated",
                    "Copy earlier drafts",
                    ButtonKind::Ghost,
                    Some(Action::Copy(
                        v.outdated_comments
                            .iter()
                            .map(|c| format!("{}:{} ({})\n{}", c.path, c.line, c.side, c.body))
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    )),
                ))
                .finish(),
        );
    }
    if !v.pending.is_empty() {
        let mut pending = row().with_child(pill(
            v,
            &format!("{} pending line comment(s)", v.pending.len()),
            v.theme.accent,
        ));
        pending = pending.with_child(btn(
            v,
            "review-drafts",
            "Review drafts…",
            ButtonKind::Ghost,
            Some(Action::ShowDrafts),
        ));
        c = c.with_child(pending.finish());
    }
    c = c.with_child(field(&v.theme, &v.composer, 24.0));
    let mut actions = row();
    if v.line.is_some() {
        actions = actions.with_child(btn(
            v,
            "add-line",
            "Add to review",
            ButtonKind::Primary,
            (!v.body.trim().is_empty()).then_some(Action::AddLine),
        ));
    } else if v.tab == Tab::Activity && v.pending.is_empty() {
        actions = actions.with_child(btn(
            v,
            "post-comment",
            if v.reply.is_some() {
                "Post reply"
            } else {
                "Post comment"
            },
            ButtonKind::Primary,
            (!v.body.trim().is_empty()).then_some(Action::SendComment),
        ));
    } else {
        for (kind, title) in [
            (ReviewKind::Comment, "Comment"),
            (ReviewKind::Approve, "Approve"),
            (ReviewKind::RequestChanges, "Request changes"),
        ] {
            if text(&p.data, "state") != "OPEN" && kind != ReviewKind::Comment {
                continue;
            }
            actions = actions.with_child(chip(
                v,
                &format!("review-{kind:?}"),
                title,
                v.review == kind,
                Action::ReviewKind(kind),
            ));
        }
        let valid = v.can_submit_review();
        actions = actions.with_child(btn(
            v,
            "submit-review",
            "Submit review",
            ButtonKind::Primary,
            valid.then_some(Action::SubmitReview),
        ));
    }
    if v.busy {
        actions = actions.with_child(label(
            v.ui,
            if is_gitlab(&p.data) {
                "Updating GitLab…"
            } else {
                "Updating GitHub…"
            },
            11.0,
            v.theme.sub_text(),
        ));
    }
    c = c.with_child(actions.finish());
    pad(c.finish(), 18.0, 12.0)
}
fn connections(v: &PullRequestsView) -> Box<dyn Element> {
    let target = v.connection_target();
    let remote = target.and_then(|t| crate::remote_fs::resolve(&t.root));
    let mut body = col()
        .with_child(row()
            .with_child(Expanded::new(1.0, semibold(v.ui,"Repository connections",21.0,v.theme.main_text())).finish())
            .with_child(btn(v,"close-connections","Close",ButtonKind::Ghost,Some(Action::Cancel))).finish())
        .with_child(label(v.ui,"Choose the project whose machine you want to connect. Sign-in runs there and credentials stay there.",13.0,v.theme.sub_text()));
    body = body.with_child(super::menus::trigger(
        v,
        Picker::FormProject,
        target.map(|t| t.label.as_str()).unwrap_or("Choose project"),
        AppIcon::FolderGit,
    ));
    body = body.with_child(label(v.ui,
        if remote.is_some() { "SSH machine · For GitLab browser sign-in, choose Device and open the displayed URL on this computer." }
        else { "This computer · Follow the sign-in prompts in the embedded terminal." },12.0,v.theme.sub_text()));
    for provider in GitProvider::ALL {
        body = body.with_child(btn(
            v,
            &format!("connect-{}", provider.cli()),
            &format!("Sign in to {}", provider.label()),
            ButtonKind::Secondary,
            target.map(|_| Action::ConnectProvider(provider)),
        ));
    }
    body = body.with_child(label(v.ui,"Need gh or glab? Install it from this machine's System tools, then return here to sign in. Refresh Code reviews after signing in.",12.0,v.theme.sub_text()))
        .with_child(btn(v,"provider-tools","Manage machine tools",ButtonKind::Ghost,
            target.map(|_|Action::InstallProviderTools)));
    card(v, scroll(v, &v.form_scroll, pad(body.finish(), 8.0, 8.0)))
}
fn form(v: &PullRequestsView) -> Box<dyn Element> {
    let form = v.form.unwrap();
    if form == Form::Connections {
        return connections(v);
    }
    if form == Form::ReviewDrafts {
        let mut contents = col()
            .with_child(semibold(v.ui, "Draft review", 21.0, v.theme.main_text()))
            .with_child(label(
                v.ui,
                if gitlab(v) { "These comments are local drafts. GitLab posts each discussion separately when you submit." } else { "These comments are local drafts until you submit the review." },
                12.0,
                v.theme.sub_text(),
            ));
        for (i, c) in v.pending.iter().enumerate() {
            contents = contents.with_child(card(
                v,
                col()
                    .with_child(
                        row()
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    one_line(
                                        v.ui,
                                        &format!("{}:{} ({})", c.path, c.line, c.side),
                                        12.0,
                                        v.theme.accent,
                                    ),
                                )
                                .finish(),
                            )
                            .with_child(btn(
                                v,
                                &format!("remove-draft-{i}"),
                                "Remove",
                                ButtonKind::Ghost,
                                Some(Action::RemoveLine(i)),
                            ))
                            .finish(),
                    )
                    .with_child(label(v.ui, &c.body, 13.0, v.theme.main_text()))
                    .finish(),
            ));
        }
        contents = contents.with_child(
            row()
                .with_child(btn(
                    v,
                    "close-drafts",
                    "Back to diff",
                    ButtonKind::Secondary,
                    Some(Action::Cancel),
                ))
                .with_child(btn(
                    v,
                    "send-drafts",
                    "Submit review",
                    ButtonKind::Primary,
                    v.can_submit_review().then_some(Action::SubmitReview),
                ))
                .finish(),
        );
        return card(
            v,
            scroll(v, &v.form_scroll, pad(contents.finish(), 8.0, 8.0)),
        );
    }
    // New, edit and metadata forms are dialogs of their own: `pr_dialog`.
    Empty::new().finish()
}

/// New or edited pull request, and the reviewer, assignee and label
/// prompts, in the dialog the project and machine screens use.
fn pr_dialog(v: &PullRequestsView) -> Box<dyn Element> {
    let form = v.form.unwrap();
    let noun = if gitlab(v) { "merge request" } else { "pull request" };
    let provider = if gitlab(v) { "GitLab" } else { "GitHub" };
    let reference = v
        .detail
        .as_ref()
        .map(|p| format!("{} #{}", p.target.label, p.number()))
        .unwrap_or_default();
    let (icon, title, subtitle) = match form {
        Form::Create => (
            AppIcon::PullRequest,
            format!("New {noun}"),
            "Choose the branch with your changes and the branch to merge them into.".to_string(),
        ),
        Form::Edit => (AppIcon::Edit, format!("Edit {noun}"), reference.clone()),
        Form::Metadata(MetadataField::Reviewer) => (AppIcon::User, "Request a reviewer".into(), reference.clone()),
        Form::Metadata(MetadataField::Assignee) => (AppIcon::User, "Assign someone".into(), reference.clone()),
        _ => (AppIcon::Layers, "Add a label".into(), reference.clone()),
    };
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0);
    let mut footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0);
    if let Form::Metadata(kind) = form {
        let hint = match (gitlab(v), kind) {
            (true, MetadataField::Label) => "The label's exact name in this GitLab project.",
            (true, _) => "Their exact GitLab username. GitLab checks they have access.",
            (false, MetadataField::Label) => "The label's exact name in this repository.",
            (false, _) => "Their exact GitHub username, or a team as org/team. GitHub checks they have access.",
        };
        body = body
            .with_child(field_label(v, if kind == MetadataField::Label { "Label" } else { "Username" }, None))
            .with_child(title_input(v))
            .with_child(label(v.ui, hint, 12.0, v.theme.sub_text()));
        footer = footer
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(btn(v, "cancel-form", "Cancel", ButtonKind::Ghost, Some(Action::Cancel)))
            .with_child(btn(
                v,
                "submit-metadata",
                if v.busy { "Adding…" } else { "Add" },
                ButtonKind::Primary,
                (!v.title.trim().is_empty()).then_some(Action::SubmitMetadata),
            ));
    } else {
        if form == Form::Create {
            let project = v.project.and_then(|i| v.targets.get(i)).or(v.targets.first());
            if let Some(project) = project {
                body = body.with_child(
                    row()
                        .with_child(label(v.ui, "Project", 12.0, v.theme.sub_text()))
                        .with_child(super::menus::trigger(v, Picker::FormProject, &project.label, AppIcon::FolderGit))
                        .finish(),
                );
            }
        }
        body = body
            .with_child(compare_card(v, form, noun, provider))
            .with_child(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(6.0)
                    .with_child(field_label(v, "Title", None))
                    .with_child(title_input(v))
                    .finish(),
            )
            .with_child(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(6.0)
                    .with_child(field_label(v, "Description", None))
                    .with_child(description_editor(v, form))
                    .finish(),
            );
        if form == Form::Create {
            footer = footer
                .with_child(crate::machine_ui::switch(&v.theme, v.id, v.mouse("pr-draft"), v.draft, Action::ToggleDraft))
                .with_child(
                    Flex::column()
                        .with_child(medium(v.ui, "Draft", 12.5, v.theme.main_text()))
                        .with_child(label(v.ui, "Not ready for review yet", 11.0, v.theme.sub_text()))
                        .finish(),
                );
        }
        let submit = match form {
            Form::Create if v.busy => "Creating…".to_string(),
            Form::Create => {
                let verb = if v.source_needs_push() { "Push and create" } else { "Create" };
                let what = if v.draft { format!("draft {noun}") } else { noun.to_string() };
                format!("{verb} {what}")
            }
            _ if v.busy => "Saving…".into(),
            _ => "Save changes".into(),
        };
        let ready = if form == Form::Create {
            v.can_create()
        } else {
            !v.title.trim().is_empty() && !v.base.is_empty()
        };
        footer = footer
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(btn(v, "cancel-form", "Cancel", ButtonKind::Ghost, Some(Action::Cancel)))
            .with_child(btn(v, "submit-form", &submit, ButtonKind::Primary, ready.then_some(Action::SubmitForm)));
    }
    crate::machine_ui::dialog(
        &v.theme,
        v.ui,
        v.id,
        v.mouse("pr-dialog-close"),
        &v.form_scroll,
        crate::machine_ui::dialog_title(&v.theme, v.ui, icon, &title, (!subtitle.is_empty()).then_some(subtitle.as_str())),
        body.finish(),
        Some(footer.finish()),
        (v.width.get() - 48.0).clamp(360.0, 640.0),
        (!v.busy).then_some(Action::Cancel),
    )
}

/// The single-line input the app's dialogs use.
fn title_input(v: &PullRequestsView) -> Box<dyn Element> {
    crate::shared_input::shared_input(
        &v.theme,
        &v.title_editor,
        crate::shared_input::SharedInputOpts {
            height: Some(crate::machine_ui::CONTROL_HEIGHT),
            radius: crate::machine_ui::RADIUS_CONTROL,
            pad_left: 11.0,
            pad_right: 11.0,
            ..Default::default()
        },
    )
}

/// The description as one editor: Write/Preview, the team's template and
/// AI on top, what it is made from and Undo below.
fn description_editor(v: &PullRequestsView, form: Form) -> Box<dyn Element> {
    let t = v.theme;
    let creating = form == Form::Create;
    let mut toolbar = row()
        .with_spacing(4.0)
        .with_child(chip(v, "description-write", "Write", !v.description_preview, Action::DescriptionTab(false)))
        .with_child(chip(v, "description-preview", "Preview", v.description_preview, Action::DescriptionTab(true)))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if creating && !v.templates.is_empty() {
        let name = v.template.and_then(|i| v.templates.get(i)).map_or("None", |t| t.name.as_str());
        toolbar = toolbar.with_child(toolbar_menu(v, Picker::Template, AppIcon::File, &format!("Template: {name}")));
    }
    if creating {
        let ready = !v.form_head.is_empty() && v.form_head != v.base && !v.describing;
        toolbar = toolbar.with_child(toolbar_button(
            v,
            "describe-with-ai",
            AppIcon::Sparkle,
            if v.describing { "Writing…" } else { "Describe with AI" },
            ready.then_some(Action::Describe),
        ));
    }
    let content = if v.description_preview {
        let preview = if v.description.trim().is_empty() {
            label(v.ui, "Nothing to preview yet.", 13.0, t.sub_text())
        } else {
            crate::terminal::render::markdown::render_markdown_document_with_links(
                &v.description, t, v.ui, v.mono, 13.0, 1.45, v.id, Action::Link,
            )
        };
        ConstrainedBox::new(pad(preview, 14.0, 12.0)).with_min_height(180.0).finish()
    } else {
        // Clicks anywhere in the area reach the editor, as in a text box.
        let editor_id = v.description_editor.id();
        let area = Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(ChildView::new(&v.description_editor).finish())
                .finish(),
        )
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(12.0)
        .finish();
        EventHandler::new(ConstrainedBox::new(area).with_min_height(180.0).finish())
            .on_left_mouse_down(move |ctx, _, position| {
                ctx.dispatch_typed_action_for(editor_id, crate::workspace::InputEditorAction::ClickAt(position));
                DispatchEventResult::StopPropagation
            })
            .finish()
    };
    let note = if v.describing {
        status_note(v, AppIcon::RefreshCw, t.sub_text(), "Writing the description from the branch's changes…")
    } else if let Some(error) = &v.describe_error {
        status_note(v, AppIcon::TriangleAlert, t.ui_red(), error)
    } else if let Some(template) = v.template.and_then(|i| v.templates.get(i)).filter(|_| creating) {
        let whose = if template.source == "organisation" { "your organisation's" } else { "the repository's" };
        status_note(v, AppIcon::File, t.sub_text(), &format!("Started from {whose} “{}” template · Markdown", template.name))
    } else {
        status_note(v, AppIcon::Code, t.sub_text(), "Markdown supported")
    };
    let mut footer = row().with_child(Expanded::new(1.0, note).finish());
    if v.can_undo_description() {
        footer = footer.with_child(toolbar_button(v, "undo-description", AppIcon::Undo2, "Undo", Some(Action::UndoDescription)));
    }
    let bar = |e: Box<dyn Element>, y: f32| {
        Container::new(e)
            .with_padding_left(8.0)
            .with_padding_right(8.0)
            .with_padding_top(y)
            .with_padding_bottom(y)
            .finish()
    };
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(bar(toolbar.finish(), 6.0))
            .with_child(rule(&t, false))
            .with_child(content)
            .with_child(rule(&t, false))
            .with_child(bar(footer.finish(), 5.0))
            .finish(),
    )
    .with_background_color(t.surface_2())
    .with_border(Border::all(1.0).with_border_color(t.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(crate::machine_ui::RADIUS_CONTROL)))
    .finish()
}

fn status_note(v: &PullRequestsView, icon: AppIcon, color: ColorU, text: &str) -> Box<dyn Element> {
    Container::new(
        row()
            .with_spacing(6.0)
            .with_child(render_icon(icon, color, 12.0))
            .with_child(Expanded::new(1.0, label(v.ui, text, 11.5, color)).finish())
            .finish(),
    )
    .with_padding_left(4.0)
    .finish()
}

/// A quiet toolbar button: icon and label, filled on hover.
fn toolbar_button(v: &PullRequestsView, key: &str, icon: AppIcon, title: &str, action: Option<Action>) -> Box<dyn Element> {
    let (theme, ui, id) = (v.theme, v.ui, v.id);
    let title = title.to_string();
    let enabled = action.is_some() && !v.busy;
    let mut button = Hoverable::new(v.mouse(key), move |mouse| {
        let color = if enabled { theme.main_text() } else { theme.disabled_text() };
        Container::new(
            row()
                .with_spacing(6.0)
                .with_child(render_icon(icon, if enabled { theme.accent } else { color }, 13.0))
                .with_child(medium(ui, &title, 12.0, color))
                .finish(),
        )
        .with_background_color(if enabled && mouse.is_hovered() { theme.surface_3() } else { ColorU::transparent_black() })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(9.0)
        .with_padding_right(9.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    });
    if let (Some(action), true) = (action, enabled) {
        button = button
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()));
    }
    button.finish()
}

/// A toolbar button that opens one of the page's pickers below it.
fn toolbar_menu(v: &PullRequestsView, picker: Picker, icon: AppIcon, title: &str) -> Box<dyn Element> {
    let (theme, ui, id) = (v.theme, v.ui, v.id);
    let title = title.to_string();
    let open = v.picker == Some(picker);
    let tag = super::menus::tag(picker);
    let button = Hoverable::new(v.mouse(tag), move |mouse| {
        Container::new(
            row()
                .with_spacing(6.0)
                .with_child(render_icon(icon, theme.sub_text(), 13.0))
                .with_child(medium(ui, &title, 12.0, theme.main_text()))
                .with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 11.0))
                .finish(),
        )
        .with_background_color(if open || mouse.is_hovered() { theme.surface_3() } else { ColorU::transparent_black() })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_padding_left(9.0)
        .with_padding_right(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, Action::TogglePicker(picker)))
    .finish();
    SavePosition::new(button, tag).finish()
}

/// A field's name, with an optional note on the right.
fn field_label(v: &PullRequestsView, name: &str, note: Option<&str>) -> Box<dyn Element> {
    let mut line = row().with_child(Expanded::new(1.0, medium(v.ui, name, 12.0, v.theme.main_text())).finish());
    if let Some(note) = note {
        line = line.with_child(label(v.ui, note, 11.0, v.theme.sub_text()));
    }
    line.finish()
}

/// Base ← source, as GitHub's compare bar, over what the pull request would
/// hold. An edited pull request keeps its source.
fn compare_card(v: &PullRequestsView, form: Form, noun: &str, provider: &str) -> Box<dyn Element> {
    let creating = form == Form::Create;
    let half = |caption: &str, field: Box<dyn Element>| {
        Expanded::new(
            1.0,
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(6.0)
                .with_child(label(v.ui, caption, 11.5, v.theme.sub_text()))
                .with_child(field)
                .finish(),
        )
        .finish()
    };
    let pickers = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_spacing(10.0)
        .with_child(half("Base · merge into", branch_field(v, Some(Picker::Base), &v.base, "Choose a branch")))
        .with_child(
            Container::new(render_icon(AppIcon::ArrowLeft, v.theme.sub_text(), 15.0))
                .with_padding_bottom(10.0)
                .finish(),
        )
        .with_child(half(
            "Source · your changes",
            branch_field(v, creating.then_some(Picker::Source), &v.form_head, "Choose a branch"),
        ));
    let mut card = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(pad(pickers.finish(), 14.0, 14.0));
    if creating {
        card = card
            .with_child(rule(&v.theme, false))
            .with_child(pad(compare_status(v, noun, provider), 14.0, 12.0));
    }
    Container::new(card.finish())
        .with_background_color(v.theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(v.theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .finish()
}

/// A branch select; without a picker it only shows the branch.
fn branch_field(v: &PullRequestsView, picker: Option<Picker>, value: &str, placeholder: &str) -> Box<dyn Element> {
    let (theme, ui, id) = (v.theme, v.ui, v.id);
    let empty = value.is_empty();
    let text = if empty { placeholder } else { value }.to_string();
    let open = picker.is_some() && v.picker == picker;
    let render = move |hovered: bool| -> Box<dyn Element> {
        let mut line = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(AppIcon::GitBranch, if empty { theme.sub_text() } else { theme.accent }, 13.0))
            .with_child(
                Expanded::new(1.0, one_line(ui, &text, 12.5, if empty { theme.sub_text() } else { theme.main_text() }))
                    .finish(),
            );
        if picker.is_some() {
            line = line.with_child(render_icon(AppIcon::ChevronDown, theme.sub_text(), 11.0));
        }
        Container::new(line.finish())
            .with_background_color(if picker.is_none() {
                theme.surface_1()
            } else if open || hovered {
                theme.surface_3()
            } else {
                theme.surface_2()
            })
            .with_border(Border::all(1.0).with_border_color(if open { theme.accent } else { theme.outline() }))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
    };
    let Some(picker) = picker else {
        return render(false);
    };
    let tag = super::menus::tag(picker);
    let field = Hoverable::new(v.mouse(tag), move |mouse| render(mouse.is_hovered()))
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, Action::TogglePicker(picker)))
        .finish();
    SavePosition::new(field, tag).finish()
}

/// What the chosen branches would make: GitHub's "able to merge" line,
/// worked out from the checkout, and what gets pushed first.
fn compare_status(v: &PullRequestsView, noun: &str, provider: &str) -> Box<dyn Element> {
    let t = v.theme;
    let (base, source) = (&v.base, &v.form_head);
    if source.is_empty() {
        let mut line = row()
            .with_child(render_icon(AppIcon::Info, t.sub_text(), 14.0))
            .with_child(Expanded::new(1.0, label(v.ui, "Choose the branch with your changes.", 12.5, t.sub_text())).finish());
        if !v.form_current.is_empty() && v.form_current != *base {
            line = line.with_child(btn(
                v,
                "use-current-branch",
                &format!("Use {}", v.form_current),
                ButtonKind::Secondary,
                Some(Action::Source(v.form_current.clone())),
            ));
        }
        return line.finish();
    }
    if source == base {
        return status_line(v, AppIcon::TriangleAlert, t.ui_yellow(), "Base and source are the same branch. Choose two different branches.");
    }
    let mut lines = col().with_spacing(10.0);
    if v.comparing {
        lines = lines.with_child(status_line(v, AppIcon::RefreshCw, t.sub_text(), &format!("Comparing {source} with {base}…")));
    } else if let Some(c) = &v.comparison {
        let ahead = number(c, "ahead");
        if ahead == 0 {
            lines = lines.with_child(status_line(
                v,
                AppIcon::TriangleAlert,
                t.ui_yellow(),
                &format!("Nothing to merge: {source} has no commits that {base} doesn't."),
            ));
        } else {
            let files = number(c, "files");
            let plural = |n: u64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
            let mut summary = row()
                .with_child(render_icon(AppIcon::CircleCheck, t.ui_green(), 14.0))
                .with_child(medium(v.ui, &plural(ahead, "commit", "commits"), 12.5, t.main_text()))
                .with_child(label(v.ui, &format!("· {} changed", plural(files, "file", "files")), 12.5, t.sub_text()))
                .with_child(medium(v.ui, &format!("+{}", number(c, "additions")), 12.0, t.ui_green()))
                .with_child(medium(v.ui, &format!("−{}", number(c, "deletions")), 12.0, t.ui_red()))
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
            summary = match c["mergeable"].as_bool() {
                Some(true) => summary.with_child(label(v.ui, "No conflicts", 12.0, t.sub_text())),
                Some(false) => summary
                    .with_child(render_icon(AppIcon::TriangleAlert, t.ui_yellow(), 13.0))
                    .with_child(label(v.ui, &format!("Conflicts with {base}"), 12.0, t.ui_yellow())),
                None => summary,
            };
            lines = lines.with_child(summary.finish());
        }
    } else {
        lines = lines.with_child(status_line(
            v,
            AppIcon::Info,
            t.sub_text(),
            &format!("This checkout doesn't have both branches, so there's no summary. {provider} checks them when you create the {noun}."),
        ));
    }
    if v.source_needs_push() {
        let unpushed = v.source_unpushed();
        let text = if !v.source_on_remote() {
            format!("{source} isn't on {provider} yet. It's pushed when you create the {noun}.")
        } else if unpushed == 1 {
            format!("1 commit on {source} isn't on {provider} yet. It's pushed first.")
        } else {
            format!("{unpushed} commits on {source} aren't on {provider} yet. They're pushed first.")
        };
        lines = lines.with_child(status_line(v, AppIcon::ArrowUp, t.accent, &text));
    }
    lines.finish()
}

fn status_line(v: &PullRequestsView, icon: AppIcon, color: ColorU, text: &str) -> Box<dyn Element> {
    row()
        .with_child(render_icon(icon, color, 14.0))
        .with_child(Expanded::new(1.0, label(v.ui, text, 12.5, if color == v.theme.sub_text() { color } else { v.theme.main_text() })).finish())
        .finish()
}
/// A failed check's log: one monospaced row per line, only the rows on
/// screen drawn, opening on the end where it failed.
fn check_log_dialog(v: &PullRequestsView, log: &super::check_log::CheckLog) -> Box<dyn Element> {
    use super::check_log::LineKind;
    use warpui::elements::ScrollableElement as _;
    let theme = v.theme;
    let header = row()
        .with_child(render_icon(AppIcon::TriangleAlert, theme.ui_red(), 14.0))
        .with_child(
            Expanded::new(1.0, semibold(v.ui, format!("Failed check log · run {}", log.run), 13.0, theme.main_text())).finish(),
        )
        .with_child(btn(v, "log-copy", "Copy log", ButtonKind::Secondary, (!log.raw.is_empty()).then_some(Action::CopyLog)))
        .with_child(icon_button(v, "log-close", AppIcon::XClose, "Close", Some(Action::CloseLog)))
        .finish();
    let body: Box<dyn Element> = if log.loading {
        Align::new(label(v.ui, "Loading the log…", 12.5, theme.sub_text())).finish()
    } else if let Some(error) = &log.error {
        Align::new(label(v.ui, error, 12.5, theme.ui_red())).finish()
    } else if log.lines.is_empty() {
        Align::new(label(v.ui, "The log is empty.", 12.5, theme.sub_text())).finish()
    } else {
        let lines = log.lines.clone();
        let mono = v.mono;
        let rows = warpui::elements::UniformList::new(log.list.clone(), lines.len(), move |range, _| {
            range
                .map(|index| {
                    let line = &lines[index];
                    let color = match line.kind {
                        LineKind::Plain => theme.main_text(),
                        LineKind::Step => theme.accent,
                        LineKind::Error => theme.ui_red(),
                        LineKind::Warning => theme.ui_yellow(),
                    };
                    ConstrainedBox::new(
                        Container::new(one_line(mono, &line.text, 11.5, color))
                            .with_padding_left(12.0)
                            .with_padding_right(12.0)
                            .finish(),
                    )
                    .with_height(19.0)
                    .finish()
                })
                .collect::<Vec<_>>()
                .into_iter()
        });
        warpui::elements::Scrollable::vertical(
            log.scroll.clone(),
            rows.finish_scrollable(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.pane_surface().into(),
        )
        .finish()
    };
    let mut column = col().with_spacing(10.0).with_child(header);
    if log.skipped > 0 {
        column = column.with_child(label(
            v.ui,
            &format!("Showing the last {} lines; Copy log has all of it.", log.lines.len()),
            11.5,
            theme.sub_text(),
        ));
    }
    column = column.with_child(
        ConstrainedBox::new(
            Container::new(body)
                .with_background_color(theme.pane_surface())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_top(6.0)
                .with_padding_bottom(6.0)
                .finish(),
        )
        .with_height(520.0)
        .finish(),
    );
    Container::new(column.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(16.0)
        .finish()
}

fn confirmation(v: &PullRequestsView) -> Box<dyn Element> {
    let op = v.confirmation.as_ref().unwrap();
    let (title, body) = if gitlab(v) {
        match op {
            Operation::Merge { auto: true, .. } => ("Enable auto-merge?", "GitLab will merge this request when project rules permit it. The reviewed head commit must still match."),
            Operation::Merge { .. } => ("Merge this request?", "GitLab project rules still apply. New commits require a refresh before merging."),
            Operation::SetState { .. } => ("Close this merge request?", "The merge request will close on GitLab. The source branch and local workspace stay in place."),
            Operation::UpdateBranch { .. } => ("Rebase onto target?", "GitLab will rebase the source branch, changing its head and restarting checks. Refresh afterward."),
            Operation::Rerun { .. } => ("Retry this failed job?", "GitLab CI will retry the selected job."),
            _ => ("Confirm this change?", "This updates the merge request on GitLab."),
        }
    } else {
        match op {
            Operation::OverrideMerge { .. } => ("Bypass branch requirements?", "Administrator merge can bypass required reviews, checks and other repository protections. The exact reviewed head commit is still required."),
            Operation::Merge { auto: true, .. } => ("Enable auto-merge?", "GitHub will merge this pull request when its rules and checks permit it. Verify the repository, base branch, and head commit below."),
            Operation::Merge { .. } => ("Merge this pull request?", "This changes the base branch on GitHub. Repository rules still apply. If the head changed since you reviewed it, the merge will be rejected."),
            Operation::SetState { .. } => ("Close this pull request?", "The pull request will be closed on GitHub. Its branch and local workspaces are kept."),
            Operation::UpdateBranch { .. } => ("Update the PR branch?", "GitHub will merge the latest base branch into this pull request's branch. This may add a commit and restart checks."),
            Operation::Rerun { .. } => ("Rerun failed checks?", "GitHub Actions will restart failed jobs. This uses the repository's Actions resources."),
            _ => ("Confirm this change?", "This updates the pull request on GitHub."),
        }
    };
    let mut c = col()
        .with_spacing(18.0)
        .with_child(semibold(v.ui, title, 22.0, v.theme.main_text()))
        .with_child(label(v.ui, body, 13.0, v.theme.sub_text()));
    if let Some(p) = &v.detail {
        c = c.with_child(card(
            v,
            col()
                .with_spacing(7.0)
                .with_child(semibold(
                    v.ui,
                    &format!("{} #{}", p.target.label, p.number()),
                    13.0,
                    v.theme.main_text(),
                ))
                .with_child(label(
                    v.ui,
                    text(&p.data, "title"),
                    13.0,
                    v.theme.main_text(),
                ))
                .with_child(label(
                    v.ui,
                    &format!(
                        "{} → {}",
                        text(&p.data, "headRefName"),
                        text(&p.data, "baseRefName")
                    ),
                    12.0,
                    v.theme.sub_text(),
                ))
                .with_child(label(
                    v.mono,
                    text(&p.data, "headRefOid"),
                    11.0,
                    v.theme.sub_text(),
                ))
                .finish(),
        ));
    }
    if let Operation::OverrideMerge {
        acknowledgement, ..
    } = op
    {
        c = c.with_child(chip(
            v,
            "ack-override",
            "I understand this bypasses required checks and reviews",
            !acknowledgement.is_empty(),
            Action::AcknowledgeOverride,
        ));
    }
    c = c.with_child(
        row()
            .with_child(btn(
                v,
                "cancel-confirm",
                "Cancel",
                ButtonKind::Secondary,
                Some(Action::Cancel),
            ))
            .with_child(btn(
                v,
                "confirm",
                "Confirm",
                ButtonKind::Primary,
                (!matches!(op,Operation::OverrideMerge{acknowledgement,..} if acknowledgement.is_empty())).then_some(Action::Confirm),
            ))
            .finish(),
    );
    card(v, pad(c.finish(), 8.0, 8.0))
}
