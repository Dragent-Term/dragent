//! The Automations side panel: creating an automation (with templates),
//! an automation's runs and settings, a teammate's automation (read-only),
//! the prompt history sheet, and the delete confirmation.

use chrono::Local;
use dragent_automations::cron::{clock_label, ordinal, MONTH_LABELS, WEEKDAY_LABELS};
use dragent_automations::{
    Automation, Period, PromptSource, Run, RunFilter, RunStatus, Schedule, Trigger,
};
use std::rc::Rc;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, SavePosition, ScrollbarWidth, Shrinkable, Wrap,
};
use warpui_core::elements::{Clipped, Hoverable};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::executor::describe_error;
use super::page::{
    agent_label, describe_schedule, format_time, project_label, template_card, AGENT_CHIP_TAG,
};
use super::team::TeamAutomation;
use super::templates::TEMPLATES;
use super::view::{AutomationsAction as A, AutomationsView, PanelTab, Picker, WorkspaceKind};
use super::widgets::{
    button, card_section, divider, field, label, menu_card, menu_item, one_line, section_label,
    segmented, select, status_badge, switch, tag_chip, with_menu, ButtonKind, Choice, Segment,
};
use super::AutomationRuntime;
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::agent_chip;

const PANEL_WIDTH: f32 = 480.0;
const FIELD_WIDTH: f32 = PANEL_WIDTH - 36.0;

/// Zones offered besides this computer's and the automation's own.
const ZONES: [&str; 22] = [
    "UTC",
    "America/Los_Angeles",
    "America/Denver",
    "America/Chicago",
    "America/New_York",
    "America/Toronto",
    "America/Mexico_City",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Madrid",
    "Africa/Lagos",
    "Africa/Nairobi",
    "Africa/Johannesburg",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Singapore",
    "Asia/Shanghai",
    "Asia/Tokyo",
    "Australia/Sydney",
    "Pacific/Auckland",
];

fn trigger_label(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Schedule => "On schedule",
        Trigger::Manual => "Run manually",
    }
}

fn panel_header(
    view: &AutomationsView,
    theme: &Theme,
    title: &str,
    subtitle: Option<&str>,
) -> Box<dyn Element> {
    let mut text_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.0)
        .with_child(label(view.ui_font, title, 15.0, theme.main_text()));
    if let Some(subtitle) = subtitle {
        text_col = text_col.with_child(label(view.ui_font, subtitle, 12.0, theme.sub_text()));
    }
    // Opaque chrome, like the history popover: `solid_background` is fully
    // opaque on every theme, so scrolling body content slides *under* this
    // bar instead of showing through it.
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, text_col.finish()).finish())
            .with_child(button(
                theme,
                view.ui_font,
                view.view_id,
                view.mouse("close-panel"),
                Some(AppIcon::XClose),
                "",
                ButtonKind::Ghost,
                Some(A::ClosePanel),
            ))
            .finish(),
    )
    .with_background_color(theme.solid_background())
    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
    .with_padding_left(18.0)
    .with_padding_right(10.0)
    .with_padding_top(12.0)
    .with_padding_bottom(10.0)
    .finish()
}

fn scrolling(view: &AutomationsView, theme: &Theme, body: Box<dyn Element>) -> Box<dyn Element> {
    // Input fields are editor ChildViews with unbounded positioned layers
    // (carets, selections, ghost text). A flex constraint sizes them but does
    // not clip those layers, so they previously painted over the panel header
    // and footer while scrolling. Clip the whole body at the scroll boundary;
    // this is the actual occlusion gate (same as the document editor surface).
    // Floating layers (picker dropdowns, mention popups, context menus) are
    // unclipped by design and would otherwise hover over the header while the
    // body moves beneath them — dismiss them on scroll, like every menu UI.
    let view_id = view.view_id;
    let body = EventHandler::new(Clipped::new(body).finish())
        .on_scroll_wheel(move |event_ctx, _, _, _, _, _| {
            event_ctx.dispatch_typed_action_for(view_id, A::DismissFloatingUi);
            DispatchEventResult::PropagateToParent
        })
        .finish();
    Expanded::new(
        1.0,
        ClippedScrollable::vertical(
            view.panel_scroll.clone(),
            Container::new(body)
                .with_padding_left(18.0)
                .with_padding_right(18.0)
                .with_padding_top(12.0)
                .with_padding_bottom(20.0)
                .finish(),
            ScrollbarWidth::Auto,
            theme.outline().into(),
            theme.sub_text().into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish(),
    )
    .finish()
}

/// Error under the form, with the upgrade when it's the plan's limit.
fn form_error(view: &AutomationsView, theme: &Theme) -> Option<Box<dyn Element>> {
    let error = view.error.as_ref()?;
    let ui = view.ui_font;
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(6.0)
        .with_child(label(ui, error, 12.0, theme.ui_red()));
    if view.error_is_limit {
        column = column.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("upgrade-form"),
            Some(AppIcon::ArrowUp),
            "Upgrade to Pro",
            ButtonKind::Primary,
            Some(A::Upgrade),
        ));
    }
    Some(column.finish())
}

// ── Create ────────────────────────────────────────────────────────────────────

pub(crate) fn create_panel(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(card_section(
            theme,
            ui,
            "Name",
            field(theme, &view.name_editor, 0.0),
        ))
        .with_child(form_fields(view, theme));
    if let Some(error) = form_error(view, theme) {
        body = body.with_child(error);
    }

    let templates_header = Hoverable::new(view.mouse("templates-toggle"), {
        let theme = *theme;
        let open = view.templates_open;
        move |state| {
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(
                            1.0,
                            label(
                                ui,
                                "Start from a template",
                                13.0,
                                if state.is_hovered() {
                                    theme.main_text()
                                } else {
                                    theme.main_text()
                                },
                            ),
                        )
                        .finish(),
                    )
                    .with_child(render_icon(
                        if open {
                            AppIcon::ChevronDown
                        } else {
                            AppIcon::ChevronUp
                        },
                        theme.sub_text(),
                        11.0,
                    ))
                    .finish(),
            )
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish()
        }
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::ToggleTemplates))
    .finish();
    let mut templates_inner = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(templates_header);
    if view.templates_open {
        let mut cards = Wrap::row().with_spacing(8.0).with_run_spacing(8.0);
        for index in 0..TEMPLATES.len() {
            cards = cards.with_child(
                ConstrainedBox::new(template_card(view, theme, index, true))
                    .with_width(213.0)
                    .finish(),
            );
        }
        templates_inner = templates_inner.with_child(cards.finish());
    }
    let templates = Container::new(templates_inner.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(12.0)
        .with_padding_bottom(14.0)
        .finish();

    let footer = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(button(
                theme,
                ui,
                id,
                view.mouse("cancel-create"),
                None,
                "Cancel",
                ButtonKind::Secondary,
                Some(A::ClosePanel),
            ))
            .with_child(button(
                theme,
                ui,
                id,
                view.mouse("create"),
                None,
                "Create automation",
                ButtonKind::Primary,
                Some(A::Save),
            ))
            .finish(),
    )
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .with_background_color(theme.solid_background())
    .with_uniform_padding(12.0)
    .finish();

    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(panel_header(
            view,
            theme,
            "New automation",
            Some("An agent runs this on a schedule, in its own workspace."),
        ))
        .with_child(scrolling(view, theme, body.finish()))
        .with_child(templates)
        .with_child(footer)
        .finish()
}

// ── Detail ────────────────────────────────────────────────────────────────────

pub(crate) fn detail_panel(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automation: &Automation,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;

    let menu_button = button(
        theme,
        ui,
        id,
        view.mouse("menu"),
        Some(AppIcon::DotsVertical),
        "",
        ButtonKind::Ghost,
        Some(A::ToggleMenu),
    );
    let menu = view.menu_open.then(|| {
        menu_card(
            theme,
            vec![
                menu_item(
                    theme,
                    ui,
                    id,
                    view.mouse("menu-history"),
                    AppIcon::History,
                    "Prompt history",
                    false,
                    A::ShowPromptHistory,
                ),
                menu_item(
                    theme,
                    ui,
                    id,
                    view.mouse("menu-link"),
                    AppIcon::Copy,
                    "Copy link",
                    false,
                    A::CopyLink(automation.id.clone()),
                ),
                divider(theme),
                menu_item(
                    theme,
                    ui,
                    id,
                    view.mouse("menu-delete"),
                    AppIcon::XClose,
                    "Delete automation",
                    true,
                    A::RequestDelete,
                ),
            ],
            200.0,
        )
    });

    let title = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(
            Expanded::new(1.0, label(ui, &automation.name, 17.0, theme.main_text())).finish(),
        )
        .with_child(with_menu(menu_button, menu, true))
        .with_child(switch(
            theme,
            id,
            automation.enabled,
            A::ToggleEnabled(automation.id.clone()),
        ))
        .finish();

    let over = runtime.is_over_allowance(&automation.id);
    let next = automation
        .enabled
        .then(|| runtime.scheduled_run(&automation.id))
        .flatten()
        .and_then(|run| run.scheduled_at_ms);
    let (banner_text, banner_color) = if over {
        (
            "Not running: the free plan runs your first 4 automations. Upgrade to Pro to run them all.".to_string(),
            theme.ui_yellow(),
        )
    } else {
        match (automation.enabled, next) {
            (true, Some(next)) => (format!("Next run {}", format_time(next)), theme.accent),
            (true, None) => (
                "No upcoming run: the schedule has no future time.".into(),
                theme.accent,
            ),
            (false, _) => (
                "Paused. Turn it on to schedule runs again.".into(),
                theme.sub_text(),
            ),
        }
    };
    let next_banner = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(render_icon(AppIcon::Clock, banner_color, 12.0))
            .with_child(Shrinkable::new(1.0, label(ui, &banner_text, 12.0, banner_color)).finish())
            .finish(),
    )
    .with_background_color(ColorU::new(
        banner_color.r,
        banner_color.g,
        banner_color.b,
        22,
    ))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(7.0)
    .with_padding_bottom(7.0)
    .finish();

    let tab = |key: &'static str, text: &'static str, which: PanelTab| {
        let selected = view.tab == which;
        let theme = *theme;
        Hoverable::new(view.mouse(key), move |state| {
            Container::new(label(
                ui,
                text,
                12.5,
                if selected || state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.sub_text()
                },
            ))
            .with_background_color(if selected {
                theme.surface_3()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::SetTab(which)))
        .finish()
    };
    let mut tabs_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(tab("tab-runs", "Runs", PanelTab::Runs))
        .with_child(tab("tab-settings", "Settings", PanelTab::Settings))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if view.tab == PanelTab::Runs {
        tabs_row = tabs_row.with_child(button(
            theme,
            ui,
            id,
            view.mouse("run-now"),
            Some(AppIcon::ArrowRight),
            "Run now",
            ButtonKind::Secondary,
            (automation.enabled && !over).then_some(A::RunNow),
        ));
    }

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(title)
        .with_child(next_banner)
        .with_child(tabs_row.finish());
    if let Some(error) = form_error(view, theme) {
        body = body.with_child(error);
    }
    body = match view.tab {
        PanelTab::Runs => body.with_child(run_history(view, theme, runtime, automation)),
        PanelTab::Settings => {
            let mut settings = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(14.0)
                .with_child(card_section(
                    theme,
                    ui,
                    "Name",
                    field(theme, &view.name_editor, 0.0),
                ))
                .with_child(form_fields(view, theme))
                .with_child(
                    Flex::row()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_child(button(
                            theme,
                            ui,
                            id,
                            view.mouse("save"),
                            None,
                            "Save changes",
                            ButtonKind::Primary,
                            Some(A::Save),
                        ))
                        .finish(),
                );
            if let Some(notice) = &view.notice {
                settings = settings.with_child(label(ui, notice, 12.0, theme.sub_text()));
            }
            body.with_child(settings.finish())
        }
    };

    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(panel_header(view, theme, "Automation", None))
        .with_child(scrolling(view, theme, body.finish()))
        .finish()
}

fn run_history(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automation: &Automation,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let counts = runtime.run_counts(&automation.id);
    let mut filters = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(4.0);
    for filter in RunFilter::ALL {
        let selected = view.filter == filter;
        let count = counts.get(&filter).copied().unwrap_or(0);
        let theme = *theme;
        filters = filters.with_child(
            Expanded::new(
                1.0,
                Hoverable::new(
                    view.mouse(format!("filter-{}", filter.label())),
                    move |state| {
                        Container::new(
                            Align::new(
                                Flex::row()
                                    .with_spacing(5.0)
                                    .with_child(label(
                                        ui,
                                        filter.label(),
                                        11.5,
                                        if selected {
                                            theme.main_text()
                                        } else {
                                            theme.sub_text()
                                        },
                                    ))
                                    .with_child(label(
                                        ui,
                                        &count.to_string(),
                                        11.5,
                                        theme.disabled_text(),
                                    ))
                                    .finish(),
                            )
                            .finish(),
                        )
                        .with_background_color(if selected {
                            theme.surface_3()
                        } else if state.is_hovered() {
                            theme.surface_2()
                        } else {
                            ColorU::transparent_black()
                        })
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                        .with_padding_top(5.0)
                        .with_padding_bottom(5.0)
                        .finish()
                    },
                )
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ectx, _, _| {
                    ectx.dispatch_typed_action_for(id, A::SetFilter(filter))
                })
                .finish(),
            )
            .finish(),
        );
    }

    let runs = runtime.runs(&automation.id, view.filter, view.runs_limit);
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if runs.is_empty() {
        list = list.with_child(
            Container::new(Align::new(label(ui, "No runs yet.", 12.5, theme.sub_text())).finish())
                .with_padding_top(28.0)
                .with_padding_bottom(28.0)
                .finish(),
        );
    }
    for (index, run) in runs.iter().enumerate() {
        if index > 0 {
            list = list.with_child(divider(theme));
        }
        list = list.with_child(run_row(view, theme, run));
    }
    if runs.len() == view.runs_limit {
        list = list.with_child(
            Container::new(
                Align::new(button(
                    theme,
                    ui,
                    id,
                    view.mouse("more-runs"),
                    None,
                    "Show more",
                    ButtonKind::Ghost,
                    Some(A::ShowMoreRuns),
                ))
                .finish(),
            )
            .with_uniform_padding(6.0)
            .finish(),
        );
    }

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(filters.finish())
                    .with_uniform_padding(6.0)
                    .finish(),
            )
            .with_child(divider(theme))
            .with_child(list.finish())
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
    .finish()
}

fn run_row(view: &AutomationsView, theme: &Theme, run: &Run) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let openable = run.can_open();
    let in_flight = run.status.is_in_flight() || run.status == RunStatus::Queued;
    let theme_owned = *theme;
    let name = run.branch.clone().unwrap_or_else(|| run.name.clone());
    let when = run
        .display_time_ms()
        .map(format_time)
        .unwrap_or_else(|| "—".into());
    let mut meta = format!("{when} · {}", trigger_label(run.trigger));
    if run.continued {
        meta.push_str(" · sent to the running agent");
    }
    let note = run
        .error
        .as_ref()
        .filter(|_| matches!(run.status, RunStatus::Failed | RunStatus::Skipped))
        .map(describe_error);
    let warning = run.warning.clone();
    let progress = run
        .progress
        .clone()
        .filter(|_| run.status == RunStatus::Started);
    let reply = progress
        .as_ref()
        .and_then(|progress| progress.reply.as_deref())
        .map(super::progress::reply_preview);
    let copy_reply = reply.is_some().then(|| {
        button(
            theme,
            ui,
            id,
            view.mouse(format!("copy-reply-{}", run.id)),
            Some(AppIcon::Copy),
            "Copy reply",
            ButtonKind::Ghost,
            Some(A::CopyRunReply(run.id.clone())),
        )
    });
    // Runs from before agents were followed: what the sidebar says about
    // the run's workspace.
    let activity = (run.status == RunStatus::Started && progress.is_none())
        .then(|| {
            run.workspace_path.as_ref().and_then(|path| {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                view.run_activity
                    .get(&canonical)
                    .or_else(|| view.run_activity.get(path))
                    .cloned()
            })
        })
        .flatten();
    let badge_run = run.clone();
    let cancel = in_flight.then(|| {
        button(
            theme,
            ui,
            id,
            view.mouse(format!("cancel-{}", run.id)),
            None,
            "Cancel",
            ButtonKind::Ghost,
            Some(A::CancelRun(run.id.clone())),
        )
    });
    let row = Hoverable::new(view.mouse(format!("run-{}", run.id)), move |state| {
        let theme = theme_owned;
        let mut top = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, one_line(ui, &name, 12.5, theme.main_text())).finish())
            .with_child(status_badge(&theme, ui, &badge_run));
        if let Some(cancel) = cancel {
            top = top.with_child(cancel);
        }
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(top.finish())
            .with_child(label(ui, &meta, 11.0, theme.disabled_text()));
        if let Some(activity) = &activity {
            column = column.with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(5.0)
                    .with_child(render_icon(AppIcon::Agent, theme.sub_text(), 11.0))
                    .with_child(label(ui, activity, 11.5, theme.sub_text()))
                    .finish(),
            );
        }
        if let Some(reply) = &reply {
            column = column.with_child(
                Container::new(label(ui, reply, 11.5, theme.sub_text()))
                    .with_background_color(theme.surface_1())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_uniform_padding(7.0)
                    .finish(),
            );
        }
        if let Some(copy_reply) = copy_reply {
            column = column.with_child(
                Flex::row()
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_child(copy_reply)
                    .finish(),
            );
        }
        if let Some(note) = &note {
            column = column.with_child(label(ui, note, 11.5, theme.ui_red()));
        }
        if let Some(warning) = &warning {
            column = column.with_child(label(ui, warning, 11.5, theme.ui_yellow()));
        }
        Container::new(column.finish())
            .with_background_color(if openable && state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::transparent_black()
            })
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
    });
    if openable {
        let open = A::OpenRun(run.id.clone());
        row.with_cursor(Cursor::PointingHand)
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, open.clone()))
            .finish()
    } else {
        row.finish()
    }
}

// ── A teammate's automation ───────────────────────────────────────────────────

fn read_only_row(
    view: &AutomationsView,
    theme: &Theme,
    title: &str,
    value: &str,
) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(3.0)
        .with_child(label(view.ui_font, title, 11.0, theme.disabled_text()))
        .with_child(label(view.ui_font, value, 12.5, theme.main_text()))
        .finish()
}

pub(crate) fn team_panel(
    view: &AutomationsView,
    theme: &Theme,
    automation: &TeamAutomation,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let owner = if automation.owner_name.is_empty() {
        automation.owner_email.clone()
    } else {
        format!("{} ({})", automation.owner_name, automation.owner_email)
    };
    let schedule = if automation.schedule_label.is_empty() {
        describe_schedule(&automation.schedule)
    } else {
        automation.schedule_label.clone()
    };
    let last = match (&automation.last_run_status, automation.last_run_at) {
        (Some(status), Some(at)) => format!("{} · {}", status.replace('_', " "), format_time(at)),
        (Some(status), None) => status.replace('_', " "),
        _ => "No runs yet".into(),
    };
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(label(ui, &automation.name, 17.0, theme.main_text()))
        .with_child(label(
            ui,
            "Only its owner can change or run this automation; it runs on their computer.",
            12.0,
            theme.sub_text(),
        ))
        .with_child(read_only_row(view, theme, "Owner", &owner))
        .with_child(read_only_row(
            view,
            theme,
            "Status",
            if automation.enabled {
                "Active"
            } else {
                "Paused"
            },
        ))
        .with_child(read_only_row(view, theme, "Schedule", &schedule))
        .with_child(read_only_row(
            view,
            theme,
            "Works in",
            automation
                .project_label
                .as_deref()
                .unwrap_or("A session (no project)"),
        ))
        .with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(3.0)
                .with_child(label(ui, "Agent", 11.0, theme.disabled_text()))
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(6.0)
                        .with_child(match super::executor::prompt_agent(&automation.agent) {
                            Some(agent) => crate::workspace::prompt_agent_image(agent, theme, 14.0, 1.0),
                            None => crate::icons::render_icon(
                                crate::icons::AppIcon::Agent,
                                theme.sub_text(),
                                14.0,
                            ),
                        })
                        .with_child(label(ui, &agent_label(&automation.agent), 12.5, theme.main_text()))
                        .finish(),
                )
                .finish(),
        )
        .with_child(read_only_row(view, theme, "Last run", &last));
    if let Some(next) = automation.next_run_at.filter(|_| automation.enabled) {
        body = body.with_child(read_only_row(view, theme, "Next run", &format_time(next)));
    }
    if !automation.prompt.is_empty() {
        body = body.with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(5.0)
                .with_child(label(ui, "Prompt", 11.0, theme.disabled_text()))
                .with_child(
                    Container::new(label(ui, &automation.prompt, 12.5, theme.main_text()))
                        .with_background_color(theme.surface_2())
                        .with_border(Border::all(1.0).with_border_color(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                        .with_uniform_padding(10.0)
                        .finish(),
                )
                .finish(),
        );
    }
    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(panel_header(view, theme, "Team automation", None))
        .with_child(scrolling(view, theme, body.finish()))
        .finish()
}

pub(crate) fn missing_panel(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(panel_header(view, theme, "Automation", None))
        .with_child(
            Container::new(label(
                view.ui_font,
                "This automation isn't available. It may have been deleted, or it belongs to another organization.",
                12.5,
                theme.sub_text(),
            ))
            .with_uniform_padding(18.0)
            .finish(),
        )
        .finish()
}

// ── Prompt history ────────────────────────────────────────────────────────────

pub(crate) fn prompt_history(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automation_id: &str,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let versions = runtime.prompt_versions(automation_id);
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0);
    if versions.is_empty() {
        list = list.with_child(label(
            ui,
            "No earlier versions yet.",
            12.5,
            theme.sub_text(),
        ));
    }
    for (index, version) in versions.iter().enumerate() {
        let source = match version.source {
            PromptSource::Human => "Edited in Dragent",
            PromptSource::Agent => "Written by an agent",
            PromptSource::Restore => "Restored",
        };
        let preview: String = version
            .prompt
            .lines()
            .take(6)
            .collect::<Vec<_>>()
            .join("\n");
        let mut header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Expanded::new(
                    1.0,
                    label(
                        ui,
                        &format!("{} · {source}", format_time(version.created_at_ms)),
                        11.5,
                        theme.sub_text(),
                    ),
                )
                .finish(),
            );
        header = if index == 0 {
            header.with_child(label(ui, "Current", 11.5, theme.accent))
        } else {
            header.with_child(button(
                theme,
                ui,
                id,
                view.mouse(format!("restore-{}", version.id)),
                Some(AppIcon::Undo2),
                "Restore",
                ButtonKind::Secondary,
                Some(A::RestorePrompt(version.id)),
            ))
        };
        list = list.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(6.0)
                    .with_child(header.finish())
                    .with_child(label(ui, &preview, 12.0, theme.main_text()))
                    .finish(),
            )
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .with_uniform_padding(10.0)
            .finish(),
        );
    }
    let card = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Expanded::new(1.0, label(ui, "Prompt history", 16.0, theme.main_text()))
                            .finish(),
                    )
                    .with_child(button(
                        theme,
                        ui,
                        id,
                        view.mouse("history-close"),
                        Some(AppIcon::XClose),
                        "",
                        ButtonKind::Ghost,
                        Some(A::ClosePromptHistory),
                    ))
                    .finish(),
            )
            .with_child(
                ConstrainedBox::new(
                    ClippedScrollable::vertical(
                        view.history_scroll.clone(),
                        list.finish(),
                        ScrollbarWidth::Auto,
                        theme.outline().into(),
                        theme.sub_text().into(),
                        theme.surface_1().into(),
                    )
                    .with_overlayed_scrollbar()
                    .finish(),
                )
                .with_max_height(460.0)
                .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(18.0)
    .finish();
    modal(
        view,
        ConstrainedBox::new(card).with_max_width(520.0).finish(),
        A::ClosePromptHistory,
    )
}

fn modal(view: &AutomationsView, card: Box<dyn Element>, dismiss: A) -> Box<dyn Element> {
    let id = view.view_id;
    let card = EventHandler::new(card)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();
    EventHandler::new(
        Container::new(Align::new(card).finish())
            .with_background_color(ColorU::new(0, 0, 0, 140))
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, dismiss.clone());
        DispatchEventResult::StopPropagation
    })
    .finish()
}

// ── Form ──────────────────────────────────────────────────────────────────────

fn labelled(
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    text: &str,
    control: Box<dyn Element>,
) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(section_label(theme, ui, text))
        .with_child(control)
        .finish()
}

fn hint(view: &AutomationsView, theme: &Theme, text: &str) -> Box<dyn Element> {
    label(view.ui_font, text, 11.5, theme.disabled_text())
}

fn word(view: &AutomationsView, theme: &Theme, text: &str) -> Box<dyn Element> {
    label(view.ui_font, text, 12.5, theme.sub_text())
}

fn picker(
    view: &AutomationsView,
    theme: &Theme,
    which: Picker,
    current: &str,
    width: f32,
    options: Vec<(String, bool, A)>,
) -> Box<dyn Element> {
    let choices = options
        .into_iter()
        .enumerate()
        .map(|(index, (label, selected, action))| Choice {
            label,
            selected,
            mouse: view.mouse(format!("pick-{which:?}-{index}")),
            action,
        })
        .collect();
    select(
        theme,
        view.ui_font,
        view.view_id,
        view.mouse(format!("picker-{which:?}")),
        current,
        width,
        view.open_picker == Some(which),
        A::TogglePicker(which),
        choices,
        &view.picker_scroll,
    )
}

/// The same searchable branch menu used on workspace creation.
fn branch_picker(view: &AutomationsView, theme: &Theme, base: bool) -> Box<dyn Element> {
    let which = if base {
        Picker::BaseBranch
    } else {
        Picker::ExistingBranch
    };
    let selected = if base {
        &view.form.base_branch
    } else {
        &view.form.existing_branch
    };
    let current = if selected.is_empty() {
        if base {
            "Project's current branch"
        } else {
            "Choose a branch"
        }
    } else {
        selected.as_str()
    };
    let trigger = select(
        theme,
        view.ui_font,
        view.view_id,
        view.mouse(format!("picker-{which:?}")),
        current,
        FIELD_WIDTH,
        false,
        A::TogglePicker(which),
        Vec::new(),
        &view.picker_scroll,
    );
    let menu = (view.open_picker == Some(which)).then(|| {
        crate::workspace::render_branch_menu(crate::workspace::BranchMenuProps {
            theme,
            ui: view.ui_font,
            view_id: view.view_id,
            states: view.mouse_states(),
            scroll: &view.picker_scroll,
            search_editor: &view.branch_search_editor,
            search: &view.branch_search,
            branches: &view.branches,
            selected: (!selected.is_empty()).then_some(selected.as_str()),
            title: if base {
                "Base branch for new worktrees"
            } else {
                "Branch for automation runs"
            },
            width: FIELD_WIDTH,
            loading: view.branches_loading,
            default_label: base.then_some("Project's current branch"),
            pick: Rc::new(move |branch| A::SelectBranch { base, branch }),
        })
    });
    with_menu(trigger, menu, false)
}

fn segment(
    view: &AutomationsView,
    key: &str,
    text: &str,
    selected: bool,
    action: Option<A>,
) -> Segment {
    Segment {
        mouse: view.mouse(key),
        label: text.to_string(),
        selected,
        action,
    }
}

/// When runs start: on the schedule, or when something new shows up in a
/// connected service (the schedule then says how often to look).
fn schedule_fields(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let form = &view.form;
    let connected: Vec<crate::integrations::Provider> = crate::integrations::Provider::ALL
        .into_iter()
        .filter(|provider| provider.offered() && crate::integrations::is_connected(*provider))
        .collect();
    let first = connected.first().map(|provider| provider.key().to_string());
    let watching = form.watch_service.is_some();
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(segmented(
            theme,
            ui,
            id,
            vec![
                segment(
                    view,
                    "starts-schedule",
                    "On a schedule",
                    !watching,
                    Some(A::SetWatch(None)),
                ),
                segment(
                    view,
                    "starts-watch",
                    "When something new shows up",
                    watching,
                    first.map(|service| A::SetWatch(Some(service))),
                ),
            ],
        ));
    if !watching {
        if connected.is_empty() {
            column = column.with_child(hint(
                view,
                theme,
                "To start runs from what shows up in Linear, Jira, Sentry or another service, connect it in Settings → Integrations.",
            ));
        }
        return column.with_child(schedule_rows(view, theme)).finish();
    }

    let mut services = Wrap::row().with_spacing(6.0).with_run_spacing(6.0);
    for provider in &connected {
        services = services.with_child(service_chip(
            view,
            theme,
            *provider,
            form.watch_service.as_deref() == Some(provider.key()),
        ));
    }
    let service = form
        .watch_service
        .as_deref()
        .and_then(crate::integrations::Provider::from_key);
    let needs_words = service.is_some_and(crate::integrations::items::needs_query);
    let intervals = super::watch::INTERVALS
        .into_iter()
        .map(|minutes| {
            let text = if minutes >= 60 {
                "1 hour".to_string()
            } else {
                format!("{minutes} min")
            };
            segment(
                view,
                &format!("watch-every-{minutes}"),
                &text,
                form.watch_minutes == minutes,
                Some(A::SetWatchInterval(minutes)),
            )
        })
        .collect();
    let label_for = service
        .map(|provider| provider.label())
        .unwrap_or("the service");
    column = column
        .with_child(services.finish())
        .with_child(field(theme, &view.watch_query_editor, 34.0))
        .with_child(hint(
            view,
            theme,
            &if needs_words {
                format!("{label_for} only finds items for words: say what to look for.")
            } else {
                format!("Uses {label_for}'s own search, as its search box takes it. Empty: its recent, open or assigned items.")
            },
        ))
        .with_child(word(view, theme, "Check every"))
        .with_child(segmented(theme, ui, id, intervals))
        .with_child(hint(
            view,
            theme,
            &format!(
                "Each check searches {label_for}. A run starts only for what it hasn't seen before, all of it in one run, and at most {} runs a day. The first check just notes what's already there. Dragent has to be open to check.",
                super::watch::DAILY_LIMIT
            ),
        ));
    column.finish()
}

/// One connected service a watch can use: its mark and name.
fn service_chip(
    view: &AutomationsView,
    theme: &Theme,
    provider: crate::integrations::Provider,
    selected: bool,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let theme_c = *theme;
    let action = A::SetWatch(Some(provider.key().to_string()));
    Hoverable::new(
        view.mouse(format!("watch-service-{}", provider.key())),
        move |state| {
            let content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(6.0)
                .with_child(crate::integrations::marks::mark(
                    &theme_c, ui, provider, 14.0,
                ))
                .with_child(label(
                    ui,
                    provider.label(),
                    12.5,
                    if selected {
                        theme_c.main_text()
                    } else {
                        theme_c.sub_text()
                    },
                ))
                .finish();
            let mut chip = Container::new(content)
                .with_padding_left(10.0)
                .with_padding_right(10.0)
                .with_padding_top(5.0)
                .with_padding_bottom(5.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_border(Border::all(1.0).with_border_color(if selected {
                    theme_c.accent
                } else {
                    theme_c.outline()
                }));
            if selected {
                chip = chip.with_background_color(theme_c.surface_3());
            } else if state.is_hovered() {
                chip = chip.with_background_color(theme_c.surface_2());
            }
            chip.finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone()))
    .finish()
}

fn schedule_rows(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let form = &view.form;
    let repeat = form.repeat;
    let custom = form.custom_schedule.is_some();
    let period_label = if custom {
        "custom".to_string()
    } else {
        repeat.period.label().to_string()
    };
    let mut schedule = Wrap::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_run_spacing(6.0)
        .with_child(word(view, theme, "Every"))
        .with_child(picker(
            view,
            theme,
            Picker::Period,
            &period_label,
            92.0,
            Period::ALL
                .into_iter()
                .map(|period| {
                    (
                        period.label().to_string(),
                        !custom && repeat.period == period,
                        A::SetPeriod(period),
                    )
                })
                .collect(),
        ));
    if !custom {
        let day_options = || {
            (1..=31)
                .map(|day| (ordinal(day), repeat.day == day, A::SetDay(day)))
                .collect::<Vec<_>>()
        };
        match repeat.period {
            Period::Week => {
                schedule = schedule
                    .with_child(word(view, theme, "on"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Weekday,
                        WEEKDAY_LABELS[repeat.weekday as usize % 7],
                        112.0,
                        (0..7)
                            .map(|day| {
                                (
                                    WEEKDAY_LABELS[day as usize].to_string(),
                                    repeat.weekday == day,
                                    A::SetWeekday(day),
                                )
                            })
                            .collect(),
                    ));
            }
            Period::Month => {
                schedule = schedule
                    .with_child(word(view, theme, "on the"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Day,
                        &ordinal(repeat.day),
                        72.0,
                        day_options(),
                    ));
            }
            Period::Year => {
                schedule = schedule
                    .with_child(word(view, theme, "in"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Month,
                        MONTH_LABELS[(repeat.month.max(1) - 1) as usize % 12],
                        112.0,
                        (1..=12)
                            .map(|month| {
                                (
                                    MONTH_LABELS[(month - 1) as usize].to_string(),
                                    repeat.month == month,
                                    A::SetMonth(month),
                                )
                            })
                            .collect(),
                    ))
                    .with_child(word(view, theme, "on the"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Day,
                        &ordinal(repeat.day),
                        72.0,
                        day_options(),
                    ));
            }
            Period::Minute | Period::Hour | Period::Day => {}
        }
        let mut minutes: Vec<u32> = (0..60).step_by(5).collect();
        if !minutes.contains(&repeat.minute) {
            minutes.push(repeat.minute);
            minutes.sort_unstable();
        }
        let minute_options: Vec<(String, bool, A)> = minutes
            .iter()
            .map(|minute| {
                (
                    format!(":{minute:02}"),
                    repeat.minute == *minute,
                    A::SetMinute(*minute),
                )
            })
            .collect();
        match repeat.period {
            Period::Minute => {}
            Period::Hour => {
                schedule = schedule
                    .with_child(word(view, theme, "at minute"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Minute,
                        &format!(":{:02}", repeat.minute),
                        72.0,
                        minute_options,
                    ));
            }
            _ => {
                schedule = schedule
                    .with_child(word(view, theme, "at"))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Hour,
                        &clock_label(repeat.hour, 0).replace(":00", ""),
                        84.0,
                        (0..24)
                            .map(|hour| {
                                (
                                    clock_label(hour, 0).replace(":00", ""),
                                    repeat.hour == hour,
                                    A::SetHour(hour),
                                )
                            })
                            .collect(),
                    ))
                    .with_child(picker(
                        view,
                        theme,
                        Picker::Minute,
                        &format!(":{:02}", repeat.minute),
                        72.0,
                        minute_options,
                    ));
            }
        }
    }

    // Time zone: this computer's, the automation's, and common ones.
    let local = dragent_automations::local_time_zone();
    let mut zones: Vec<String> = Vec::new();
    for zone in [local.as_str(), form.timezone.as_str()]
        .into_iter()
        .chain(ZONES)
    {
        if !zone.is_empty() && !zones.iter().any(|known| known == zone) {
            zones.push(zone.to_string());
        }
    }
    let current_zone = if form.timezone.is_empty() {
        local.clone()
    } else {
        form.timezone.clone()
    };
    let zone_options = zones
        .iter()
        .map(|zone| {
            let text = if *zone == local {
                format!("{zone} (this computer)")
            } else {
                zone.clone()
            };
            (text, *zone == current_zone, A::SetTimeZone(zone.clone()))
        })
        .collect();
    let zone_picker = picker(
        view,
        theme,
        Picker::TimeZone,
        &current_zone,
        FIELD_WIDTH,
        zone_options,
    );

    let expression = form.schedule();
    let summary = match Schedule::parse(&expression) {
        Ok(parsed) => {
            let zone: Option<dragent_automations::Tz> = current_zone.parse().ok();
            let next = match zone {
                Some(zone) => parsed
                    .next_after(&chrono::Utc::now().with_timezone(&zone))
                    .map(|next| next.timestamp_millis()),
                None => parsed
                    .next_after(&Local::now())
                    .map(|next| next.timestamp_millis()),
            }
            // `format_time` shows this computer's clock; say so when the
            // schedule's zone is a different one.
            .map(|next| {
                let yours = if current_zone == local {
                    ""
                } else {
                    " your time"
                };
                format!(" · next {}{yours}", format_time(next))
            })
            .unwrap_or_default();
            if custom {
                format!("Custom schedule “{expression}”{next}. Choosing a period replaces it.")
            } else {
                format!("{}{next}", parsed.describe())
            }
        }
        Err(reason) => format!("Invalid schedule: {reason}"),
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(schedule.finish())
        .with_child(zone_picker)
        .with_child(label(ui, &summary, 11.5, theme.disabled_text()))
        .finish()
}

/// Where each run's agent works: project or session, which workspace,
/// which session, and the sidebar folders it's filed into.
fn workspace_fields(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let form = &view.form;
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0);

    if form.session {
        column = column
            .with_child(segmented(
                theme,
                ui,
                id,
                vec![
                    segment(view, "kind-new", "A new folder each run", form.kind != WorkspaceKind::Pinned, Some(A::SetWorkspaceKind(WorkspaceKind::New))),
                    segment(view, "kind-pinned", "The same folder every run", form.kind == WorkspaceKind::Pinned, Some(A::SetWorkspaceKind(WorkspaceKind::Pinned))),
                ],
            ))
            .with_child(hint(
                view,
                theme,
                "A session has no project: the agent works in a folder of its own, listed under Sessions in the sidebar.",
            ));
    } else {
        column = column.with_child(segmented(
            theme,
            ui,
            id,
            vec![
                segment(
                    view,
                    "kind-new",
                    "New worktree",
                    form.kind == WorkspaceKind::New,
                    Some(A::SetWorkspaceKind(WorkspaceKind::New)),
                ),
                segment(
                    view,
                    "kind-branch",
                    "Existing branch",
                    form.kind == WorkspaceKind::Branch,
                    Some(A::SetWorkspaceKind(WorkspaceKind::Branch)),
                ),
                segment(
                    view,
                    "kind-pinned",
                    "Pinned workspace",
                    form.kind == WorkspaceKind::Pinned,
                    Some(A::SetWorkspaceKind(WorkspaceKind::Pinned)),
                ),
            ],
        ));
        match form.kind {
            WorkspaceKind::New => {
                column = column
                    .with_child(hint(
                        view,
                        theme,
                        "Each run creates a separate checkout and a new branch. Start it from:",
                    ))
                    .with_child(branch_picker(view, theme, true))
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(10.0)
                            .with_child(switch(
                                theme,
                                id,
                                form.push_branch,
                                A::SetPushBranch(!form.push_branch),
                            ))
                            .with_child(label(
                                ui,
                                "Publish each new branch to the remote",
                                12.5,
                                theme.main_text(),
                            ))
                            .finish(),
                    );
            }
            WorkspaceKind::Branch => {
                column = column
                    .with_child(hint(
                        view,
                        theme,
                        "Choose an existing branch. Runs reuse its checkout, or create a checkout if needed. Changes stay on this branch.",
                    ))
                    .with_child(branch_picker(view, theme, false));
            }
            WorkspaceKind::Pinned => {
                let current = view
                    .pinned_options
                    .iter()
                    .find(|(path, _)| match &form.pinned {
                        Some(pinned) => pinned == path,
                        None => {
                            Some(path) == form.project.as_ref()
                                || view.pinned_options.first().map(|(p, _)| p) == Some(path)
                        }
                    })
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| "Project checkout".into());
                let options = view
                    .pinned_options
                    .iter()
                    .enumerate()
                    .map(|(index, (path, text))| {
                        let selected = match &form.pinned {
                            Some(pinned) => pinned == path,
                            None => index == 0,
                        };
                        (text.clone(), selected, A::SetPinned(Some(index)))
                    })
                    .collect();
                column = column
                    .with_child(hint(view, theme, "Every run uses this same checkout and its existing files. No new branch or worktree is created:"))
                    .with_child(picker(view, theme, Picker::Pinned, &current, FIELD_WIDTH, options));
            }
        }
        if form.kind != WorkspaceKind::Pinned {
            column = column.with_child(labelled(
                theme,
                ui,
                "Copy into new worktrees",
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(5.0)
                    .with_child(field(theme, &view.copy_editor, 0.0))
                    .with_child(hint(
                        view,
                        theme,
                        "Gitignored files (comma separated) copied from the project into each new worktree. Applies to every automation in this project.",
                    ))
                    .finish(),
            ));
        }
    }

    // Session: a new agent each run, or the one the last run left running.
    let can_continue = form.can_continue();
    column = column.with_child(labelled(
        theme,
        ui,
        "Agent session",
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(5.0)
            .with_child(segmented(
                theme,
                ui,
                id,
                vec![
                    segment(view, "session-new", "A new session each run", !form.continue_session, Some(A::SetContinueSession(false))),
                    segment(
                        view,
                        "session-continue",
                        "Continue the last session",
                        form.continue_session,
                        can_continue.then_some(A::SetContinueSession(true)),
                    ),
                ],
            ))
            .with_child(hint(
                view,
                theme,
                if can_continue {
                    if form.continue_session {
                        "Each run sends its prompt to the previous live agent in this workspace, keeping that conversation. If the agent has ended, a new session starts."
                    } else {
                        "Each run starts a new agent conversation. Files stay in the same workspace, but the conversation starts fresh."
                    }
                } else {
                    "Each run starts a new agent conversation. Choose Pinned workspace to reuse the previous live conversation between runs."
                },
            ))
            .finish(),
    ));

    // Connected services: read only, unless the automation may post.
    column = column.with_child(labelled(
        theme,
        ui,
        "Connected services",
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(5.0)
            .with_child(segmented(
                theme,
                ui,
                id,
                vec![
                    segment(view, "services-read", "Read only", !form.service_writes, Some(A::SetServiceWrites(false))),
                    segment(
                        view,
                        "services-write",
                        "Can comment and change status",
                        form.service_writes,
                        Some(A::SetServiceWrites(true)),
                    ),
                ],
            ))
            .with_child(hint(
                view,
                theme,
                if form.service_writes {
                    "Runs can comment on items, change their status and create new ones in Linear, Jira, Slack and the other connected services, under your account."
                } else {
                    "Runs can search and read Linear, Jira, Slack and the other connected services, but can't post anything to them."
                },
            ))
            .finish(),
    ));

    // Tags: sidebar folders for each run's workspace.
    let mut chips = Wrap::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_run_spacing(6.0);
    for tag in &form.tags {
        chips = chips.with_child(tag_chip(
            theme,
            ui,
            id,
            view.mouse(format!("tag-{tag}")),
            tag,
            Some(A::RemoveTag(tag.clone())),
        ));
    }
    if form.tags.is_empty() {
        chips = chips.with_child(label(
            ui,
            "No folder: runs appear with the project's other workspaces.",
            11.5,
            theme.disabled_text(),
        ));
    }
    let suggestions: Vec<&String> = view
        .tag_suggestions
        .iter()
        .filter(|tag| {
            !form
                .tags
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(tag))
        })
        .take(6)
        .collect();
    let mut tags = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(6.0)
        .with_child(chips.finish())
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(Expanded::new(1.0, field(theme, &view.tag_editor, 0.0)).finish())
                .with_child(button(
                    theme,
                    ui,
                    id,
                    view.mouse("tag-add"),
                    Some(AppIcon::Plus),
                    "Add",
                    ButtonKind::Secondary,
                    Some(A::AddTagFromField),
                ))
                .finish(),
        );
    if !suggestions.is_empty() {
        let mut row = Wrap::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_run_spacing(6.0)
            .with_child(label(ui, "Existing groups:", 11.5, theme.disabled_text()));
        for tag in suggestions {
            row = row.with_child(button(
                theme,
                ui,
                id,
                view.mouse(format!("tag-suggest-{tag}")),
                None,
                tag,
                ButtonKind::Ghost,
                Some(A::AddTag(tag.clone())),
            ));
        }
        tags = tags.with_child(row.finish());
    }
    column = column.with_child(labelled(theme, ui, "Sidebar groups", tags.finish()))
        .with_child(hint(view, theme, "Group each run's workspace under these names in the sidebar. These are labels, not folders on disk, and do not change where the agent runs."));
    column.finish()
}

fn form_fields(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let form = &view.form;

    // Project, or a session with no project.
    let current_project = if form.session {
        "Session (no project)".to_string()
    } else {
        form.project
            .as_deref()
            .map(|root| project_label(view, root))
            .unwrap_or_else(|| "Choose a project".into())
    };
    let mut project_options: Vec<(String, bool, A)> = view
        .projects
        .iter()
        .enumerate()
        .map(|(index, (root, label))| {
            (
                label.clone(),
                !form.session && form.project.as_ref() == Some(root),
                A::SetProject(Some(index)),
            )
        })
        .collect();
    project_options.push((
        "Session (no project)".into(),
        form.session,
        A::SetProject(None),
    ));
    let project = picker(
        view,
        theme,
        Picker::Project,
        &current_project,
        FIELD_WIDTH,
        project_options,
    );

    // Agent: the prompt screen's chip and menu (the menu opens from the
    // page, under this chip).
    let view_id = view.view_id;
    let agent = agent_chip(
        theme,
        view.mono_font,
        form.agent
            .as_deref()
            .and_then(super::executor::prompt_agent),
        view.mouse("agent_chip"),
        move |ectx| ectx.dispatch_typed_action_for(view_id, A::TogglePicker(Picker::Agent)),
    );
    let agent = Align::new(SavePosition::new(agent, AGENT_CHIP_TAG).finish())
        .left()
        .finish();
    let mut agent_column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(5.0)
        .with_child(agent);
    if form.agent.as_deref() == Some(super::executor::DRAGENT_AGENT) {
        agent_column = agent_column.with_child(hint(
            view,
            theme,
            "Dragent's own agent runs inside a window, so Dragent needs a window open when this runs.",
        ));
    }
    if let Some(agent) = form
        .agent
        .as_deref()
        .and_then(super::executor::prompt_agent)
        .filter(|agent| view.asking_agents.contains(agent))
    {
        let message = format!(
            "{} may stop to ask for approval during a run, and the run waits until someone \
             answers (it's stopped after {} minutes). To let it work on its own, add its \
             skip-approval option to its command in Settings → Agents.",
            agent.label(),
            super::progress::WAIT_LIMIT_MS / 60_000
        );
        agent_column = agent_column.with_child(hint(view, theme, &message));
    }

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(card_section(theme, ui, "Works in", project))
        .with_child(card_section(
            theme,
            ui,
            "Schedule",
            schedule_fields(view, theme),
        ))
        .with_child(card_section(theme, ui, "Agent", agent_column.finish()))
        .with_child(card_section(
            theme,
            ui,
            "Prompt",
            field(theme, &view.prompt_editor, 110.0),
        ))
        .with_child(card_section(
            theme,
            ui,
            "Workspace",
            workspace_fields(view, theme),
        ))
        .finish()
}

// ── Delete confirmation ───────────────────────────────────────────────────────

pub(crate) fn delete_dialog(view: &AutomationsView, theme: &Theme, name: &str) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let card = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.0)
            .with_child(label(ui, "Delete automation?", 16.0, theme.main_text()))
            .with_child(label(
                ui,
                &format!(
                    "“{name}” and its run history will be removed. Agents it already started keep running in their workspaces."
                ),
                12.5,
                theme.sub_text(),
            ))
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_spacing(8.0)
                    .with_child(button(theme, ui, id, view.mouse("delete-cancel"), None, "Cancel", ButtonKind::Secondary, Some(A::CancelDelete)))
                    .with_child(button(theme, ui, id, view.mouse("delete-confirm"), None, "Delete", ButtonKind::Danger, Some(A::ConfirmDelete)))
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(20.0)
    .finish();
    modal(
        view,
        ConstrainedBox::new(card).with_max_width(380.0).finish(),
        A::CancelDelete,
    )
}
