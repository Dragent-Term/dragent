//! Rendering for the Automations page: summary cards, your automations and
//! your team's as a grouped (or sorted) table, the inbox of what runs came
//! back with, the empty state with suggestions, and the overlays (side
//! panel, menus, dialogs).

use std::collections::{HashMap, HashSet};

use chrono::{Local, TimeZone};
use dragent_automations::{AgentState, Automation, Run, RunStatus, Schedule};
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, ScrollbarWidth, Shrinkable, Stack, Wrap,
};
use warpui::geometry::vector::vec2f;
use warpui::{AppContext, SingletonEntity};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::panel;
use super::team::TeamAutomation;
use super::templates::TEMPLATES;
use super::view::{AutomationsAction as A, AutomationsView, Panel, Picker, Scope, SortField};
use super::widgets::{
    button, divider, dot, label, menu_card, menu_item, one_line, status_badge, ButtonKind,
};
use super::{compact_ago, compact_until, AutomationRuntime, Stats};
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::{render_agent_menu, AgentMenuProps, PromptAgent};

const PAGE_WIDTH: f32 = 960.0;
const PANEL_WIDTH: f32 = 480.0;
const NEW_MENU_TAG: &str = "automations_new_menu";
/// The empty state's "Create with AI" button.
const EMPTY_AI_TAG: &str = "automations_empty_create_with_ai";
/// The form's agent chip; the agent menu opens under it.
pub(crate) const AGENT_CHIP_TAG: &str = "automation_agent_chip";

/// `Thu, Oct 2, 9:00 AM` in local time.
pub(crate) fn format_time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| time.format("%a, %b %-d, %-I:%M %p").to_string())
        .unwrap_or_default()
}

pub(crate) fn describe_schedule(expression: &str) -> String {
    Schedule::parse(expression)
        .map(|schedule| schedule.describe())
        .unwrap_or_else(|_| format!("Invalid schedule ({expression})"))
}

/// The schedule, naming its zone when it isn't this computer's.
pub(crate) fn schedule_label(automation: &Automation) -> String {
    if let Some(watch) = &automation.watch {
        return super::watch::describe(watch, &automation.schedule);
    }
    let described = describe_schedule(&automation.schedule);
    let zone = automation.timezone.trim();
    if zone.is_empty() || zone == dragent_automations::local_time_zone() {
        described
    } else {
        format!("{described} ({zone})")
    }
}

pub(crate) fn project_label(view: &AutomationsView, root: &std::path::Path) -> String {
    if root.as_os_str().is_empty() {
        return "Session".into();
    }
    view.projects
        .iter()
        .find(|(known, _)| known == root)
        .map(|(_, label)| label.clone())
        .or_else(|| root.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| root.display().to_string())
}

pub(crate) fn agent_label(key: &str) -> String {
    super::executor::prompt_agent(key)
        .map(|agent| agent.label().to_string())
        .unwrap_or_else(|| key.to_string())
}

/// Dot colour and wording for a latest run, as the list shows it: for a
/// started run, how its agent is getting on.
fn latest_run_style(theme: &Theme, run: &Run) -> (ColorU, &'static str, bool) {
    use dragent_automations::AgentState;
    match (&run.progress, run.status) {
        (Some(progress), RunStatus::Started) => match progress.state {
            AgentState::Working => (theme.accent, "working", false),
            AgentState::NeedsInput => (theme.ui_yellow(), "needs input", false),
            AgentState::Done => (theme.ui_green(), "done", false),
            AgentState::Failed => (theme.ui_red(), "agent failed", true),
            AgentState::Ended => (theme.disabled_text(), "ended", false),
            AgentState::TimedOut => (theme.ui_yellow(), "timed out", true),
        },
        (_, status) => last_run_style(theme, status),
    }
}

/// Dot colour and wording for a latest run, as the list shows it.
fn last_run_style(theme: &Theme, status: RunStatus) -> (ColorU, &'static str, bool) {
    match status {
        RunStatus::Started => (theme.ui_green(), "started", false),
        RunStatus::Failed => (theme.ui_red(), "failed", true),
        RunStatus::Skipped => (theme.disabled_text(), "skipped", false),
        RunStatus::Cancelled => (theme.disabled_text(), "cancelled", false),
        RunStatus::Scheduled
        | RunStatus::Queued
        | RunStatus::PreparingWorkspace
        | RunStatus::StartingAgent => (theme.ui_yellow(), "starting", false),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── Entry ─────────────────────────────────────────────────────────────────────

pub(crate) fn render(view: &AutomationsView, app: &AppContext) -> Box<dyn Element> {
    let theme = view.theme;
    let runtime = AutomationRuntime::as_ref(app);
    let mut stack = Stack::new().with_child(
        Container::new(
            Align::new(
                ConstrainedBox::new(page_body(view, &theme, runtime))
                    .with_max_width(PAGE_WIDTH)
                    .finish(),
            )
            .top_center()
            .finish(),
        )
        .with_background_color(theme.background)
        .finish(),
    );

    // Menus anchored to the page close on any click elsewhere.
    if view.new_menu_open || view.row_menu.is_some() {
        let view_id = view.view_id;
        stack.add_overlay_child(
            EventHandler::new(
                Container::new(Empty::new().finish())
                    .with_background_color(ColorU::transparent_black())
                    .finish(),
            )
            .on_left_mouse_down(move |ectx, _, _| {
                ectx.dispatch_typed_action_for(view_id, A::CloseMenus);
                DispatchEventResult::StopPropagation
            })
            .finish(),
        );
    }
    if view.new_menu_open {
        stack = stack.with_positioned_overlay_child(
            new_menu(view, &theme),
            OffsetPositioning::offset_from_save_position_element(
                NEW_MENU_TAG,
                vec2f(0.0, 6.0),
                PositionedElementOffsetBounds::WindowByPosition,
                PositionedElementAnchor::BottomRight,
                ChildAnchor::TopRight,
            ),
        );
    }
    if let Some(id) = &view.row_menu {
        if let Some(automation) = runtime.automation(id) {
            stack = stack.with_positioned_overlay_child(
                row_menu(view, &theme, runtime, &automation),
                OffsetPositioning::offset_from_save_position_element(
                    row_menu_tag(id),
                    vec2f(0.0, 4.0),
                    PositionedElementOffsetBounds::WindowByPosition,
                    PositionedElementAnchor::BottomRight,
                    ChildAnchor::TopRight,
                ),
            );
        }
    }

    if let Some(tag) = view.ai_project_menu {
        let (anchor, child) = if tag == NEW_MENU_TAG {
            (PositionedElementAnchor::BottomRight, ChildAnchor::TopRight)
        } else {
            (PositionedElementAnchor::BottomLeft, ChildAnchor::TopLeft)
        };
        stack = stack.with_positioned_overlay_child(
            ai_project_menu(view, &theme),
            OffsetPositioning::offset_from_save_position_element(
                tag,
                vec2f(0.0, 6.0),
                PositionedElementOffsetBounds::WindowByPosition,
                anchor,
                child,
            ),
        );
    }
    if let Some(tag) = view.ai_agent_menu {
        // Under the "New" button it lines up with that button's right edge.
        let (anchor, child) = if tag == NEW_MENU_TAG {
            (PositionedElementAnchor::BottomRight, ChildAnchor::TopRight)
        } else {
            (PositionedElementAnchor::BottomLeft, ChildAnchor::TopLeft)
        };
        let selected = Some(view.create_with_ai_agent(app));
        stack = stack.with_positioned_overlay_child(
            agent_menu(view, &theme, selected, A::CreateWithAgentUsing),
            OffsetPositioning::offset_from_save_position_element(
                tag,
                vec2f(0.0, 6.0),
                PositionedElementOffsetBounds::WindowByPosition,
                anchor,
                child,
            ),
        );
    }

    if let Some(open) = &view.panel {
        let view_id = view.view_id;
        let catcher = EventHandler::new(
            Container::new(Empty::new().finish())
                .with_background_color(ColorU::new(0, 0, 0, 90))
                .finish(),
        )
        .on_left_mouse_down(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(view_id, A::ClosePanel);
            DispatchEventResult::StopPropagation
        })
        .finish();
        // The scrim and the panel are normal layers, not overlays: an
        // overlay's layers stack in paint order, so fields painted after an
        // open dropdown (the prompt's text) drew on top of it. As normal
        // layers, every dropdown and menu inside the panel sits above them.
        stack.add_child(catcher);
        let body = match open {
            Panel::Create => panel::create_panel(view, &theme),
            Panel::Detail(id) => match runtime.automation(id) {
                Some(automation) => panel::detail_panel(view, &theme, runtime, &automation),
                None => Empty::new().finish(),
            },
            Panel::Team(id) => match runtime.team().automations.iter().find(|a| &a.id == id) {
                Some(automation) => panel::team_panel(view, &theme, automation),
                None => panel::missing_panel(view, &theme),
            },
        };
        let side = EventHandler::new(
            Container::new(body)
                .with_background_color(theme.surface_1())
                .with_border(Border::left(1.0).with_border_color(theme.outline()))
                .finish(),
        )
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();
        stack.add_child(
            Align::new(ConstrainedBox::new(side).with_width(PANEL_WIDTH).finish())
                .right()
                .finish(),
        );
        if view.open_picker == Some(Picker::Agent) {
            let selected = view.form.agent.as_deref().and_then(super::executor::prompt_agent);
            stack = stack.with_positioned_overlay_child(
                agent_menu(view, &theme, selected, A::SetAgent),
                OffsetPositioning::offset_from_save_position_element(
                    AGENT_CHIP_TAG,
                    vec2f(0.0, 6.0),
                    PositionedElementOffsetBounds::WindowByPosition,
                    PositionedElementAnchor::BottomLeft,
                    ChildAnchor::TopLeft,
                ),
            );
        }
        if view.history_open {
            if let Panel::Detail(id) = open {
                stack.add_overlay_child(panel::prompt_history(view, &theme, runtime, id));
            }
        }
    }
    if let Some(id) = &view.pending_delete {
        let name = runtime
            .automation(id)
            .map(|automation| automation.name)
            .unwrap_or_default();
        stack.add_overlay_child(panel::delete_dialog(view, &theme, &name));
    }
    stack.finish()
}

/// "Create with AI", first step: the project the automation works in (the
/// one the user was in first), or none.
fn ai_project_menu(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let mut order: Vec<usize> = (0..view.projects.len()).collect();
    if let Some(current) = view
        .current_project
        .as_ref()
        .and_then(|current| view.projects.iter().position(|(root, _)| root == current))
    {
        order.retain(|index| *index != current);
        order.insert(0, current);
    }
    let mut rows = vec![Container::new(label(ui, "Where should it work?", 11.0, theme.disabled_text()))
        .with_padding_left(8.0)
        .with_padding_top(6.0)
        .with_padding_bottom(4.0)
        .finish()];
    for index in order {
        let (root, name) = &view.projects[index];
        let text = if view.current_project.as_ref() == Some(root) {
            format!("{name} (current)")
        } else {
            name.clone()
        };
        rows.push(menu_item(
            theme,
            ui,
            id,
            view.mouse(format!("ai-project-{index}")),
            AppIcon::Folder,
            &text,
            false,
            A::CreateWithAiIn(Some(index)),
        ));
    }
    rows.push(divider(theme));
    rows.push(menu_item(
        theme,
        ui,
        id,
        view.mouse("ai-project-none"),
        AppIcon::Terminal,
        "No project (a session)",
        false,
        A::CreateWithAiIn(None),
    ));
    warpui_core::elements::Dismiss::new(menu_card(theme, rows, 280.0))
        .on_dismiss(move |ectx, _| ectx.dispatch_typed_action_for(id, A::ClosePicker))
        .prevent_interaction_with_other_elements()
        .finish()
}

/// The prompt screen's agent menu: `choose` is dispatched for an installed
/// agent, the installer opens for one that isn't.
fn agent_menu(
    view: &AutomationsView,
    theme: &Theme,
    selected: Option<PromptAgent>,
    choose: fn(PromptAgent) -> A,
) -> Box<dyn Element> {
    let view_id = view.view_id;
    let menu = render_agent_menu(AgentMenuProps {
        theme,
        ui: view.ui_font,
        states: view.mouse_states(),
        scroll: &view.picker_scroll,
        search_editor: &view.agent_search_editor,
        search: &view.agent_search,
        selected,
        installed: &view.installed_agents,
        checking: view.agents_checking,
        installed_only: false,
        with_dragent: true,
        pick: std::rc::Rc::new(move |ectx, agent, installed| {
            ectx.dispatch_typed_action_for(
                view_id,
                if installed { choose(agent) } else { A::InstallAgent(agent) },
            );
        }),
        clear_search: std::rc::Rc::new(move |ectx| {
            ectx.dispatch_typed_action_for(view_id, A::ClearAgentSearch);
        }),
    });
    warpui_core::elements::Dismiss::new(menu)
        .on_dismiss(move |ectx, _| ectx.dispatch_typed_action_for(view_id, A::ClosePicker))
        .prevent_interaction_with_other_elements()
        .finish()
}

// ── Page ──────────────────────────────────────────────────────────────────────

fn page_body(view: &AutomationsView, theme: &Theme, runtime: &AutomationRuntime) -> Box<dyn Element> {
    let mine = runtime.automations();
    let me = runtime.account().user_id.clone();
    let team: Vec<&TeamAutomation> = runtime
        .team()
        .automations
        .iter()
        .filter(|item| me.as_deref() != Some(item.owner_user_id.as_str()))
        .collect();
    // Yours, registered from your other computers (they run there).
    let elsewhere: Vec<&TeamAutomation> = runtime
        .team()
        .automations
        .iter()
        .filter(|item| me.as_deref() == Some(item.owner_user_id.as_str()))
        .filter(|item| !mine.iter().any(|local| local.id == item.id))
        .collect();
    let org_empty = mine.is_empty() && team.is_empty() && elsewhere.is_empty();

    let mut top = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0)
        .with_child(header(view, theme, !org_empty));
    if let Some(strip) = allowance_strip(view, theme, runtime) {
        top = top.with_child(strip);
    }
    if let Some(message) = view.error.as_ref().filter(|_| view.panel.is_none()) {
        top = top.with_child(banner(view, theme, message, theme.ui_red(), view.error_is_limit));
    } else if let Some(message) = view.notice.as_ref().filter(|_| view.panel.is_none()) {
        top = top.with_child(banner(view, theme, message, theme.accent, false));
    }
    if let Some(error) = runtime.error() {
        top = top.with_child(banner(view, theme, error, theme.ui_red(), false));
    }

    let content: Box<dyn Element> = if org_empty && view.scope == Scope::Mine {
        empty_state(view, theme)
    } else {
        let failing = runtime.failing();
        let tab_empty = match view.scope {
            Scope::Mine => mine.is_empty() && elsewhere.is_empty(),
            Scope::Team => team.is_empty(),
            // The inbox says when it's empty itself.
            Scope::Inbox => false,
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0);
        if view.scope == Scope::Mine && !tab_empty {
            column = column.with_child(stat_cards(view, theme, runtime.stats(), !failing.is_empty()));
        }
        column = column.with_child(tabs_row(
            view,
            theme,
            mine.len() + elsewhere.len(),
            team.len(),
            runtime.unread_results(),
            &failing,
            tab_empty,
        ));
        column = column.with_child(match view.scope {
            Scope::Mine if tab_empty => empty_state(view, theme),
            Scope::Mine => mine_table(view, theme, runtime, &mine, &elsewhere, &failing),
            Scope::Team => team_table(view, theme, runtime, &team),
            Scope::Inbox => inbox_list(view, theme, runtime),
        });
        if !view.cli_hint_dismissed && !tab_empty && view.scope != Scope::Inbox {
            column = column.with_child(cli_hint(view, theme));
        }
        column.finish()
    };

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(16.0)
            .with_child(top.finish())
            .with_child(
                Expanded::new(
                    1.0,
                    ClippedScrollable::vertical(
                        view.list_scroll.clone(),
                        Container::new(content).with_padding_bottom(40.0).finish(),
                        ScrollbarWidth::Auto,
                        theme.outline().into(),
                        theme.sub_text().into(),
                        theme.background.into(),
                    )
                    .with_overlayed_scrollbar()
                    .finish(),
                )
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(32.0)
    .with_padding_right(32.0)
    .with_padding_top(32.0)
    .finish()
}

fn header(view: &AutomationsView, theme: &Theme, show_new: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Expanded::new(
                1.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(4.0)
                    .with_child(label(ui, "Automations", 20.0, theme.main_text()))
                    .with_child(label(
                        ui,
                        "Agents that start on a schedule, in a project or on their own.",
                        12.5,
                        theme.sub_text(),
                    ))
                    .finish(),
            )
            .finish(),
        );
    if show_new {
        let new = button(
            theme,
            ui,
            view.view_id,
            view.mouse("new"),
            Some(AppIcon::Plus),
            "New automation",
            ButtonKind::Primary,
            Some(A::ToggleNewMenu),
        );
        row = row.with_child(SavePosition::new(new, NEW_MENU_TAG).finish());
    }
    row.finish()
}

fn new_menu(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let option = |key: &'static str, icon: AppIcon, title: &'static str, detail: &'static str, action: A| {
        let theme = *theme;
        Hoverable::new(view.mouse(key), move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(10.0)
                    .with_child(
                        Container::new(render_icon(icon, theme.sub_text(), 14.0))
                            .with_padding_top(2.0)
                            .finish(),
                    )
                    .with_child(
                        Flex::column()
                            .with_spacing(2.0)
                            .with_child(label(ui, title, 12.5, theme.main_text()))
                            .with_child(label(ui, detail, 11.5, theme.sub_text()))
                            .finish(),
                    )
                    .finish(),
            )
            .with_background_color(if state.is_hovered() {
                theme.surface_3()
            } else {
                ColorU::transparent_black()
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_uniform_padding(8.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone()))
        .finish()
    };
    menu_card(
        theme,
        vec![
            option(
                "new-ai",
                AppIcon::Sparkle,
                "Create with AI",
                "Tell an agent what should run and it sets it up",
                A::CreateWithAgent(NEW_MENU_TAG),
            ),
            option(
                "new-manual",
                AppIcon::Edit,
                "Create manually",
                "Pick the project, schedule, and agent yourself",
                A::NewAutomation,
            ),
        ],
        300.0,
    )
}

fn allowance_strip(view: &AutomationsView, theme: &Theme, runtime: &AutomationRuntime) -> Option<Box<dyn Element>> {
    let (limit, owned) = runtime.allowance();
    let limit = limit?;
    let ui = view.ui_font;
    let full = owned >= limit;
    let text = if full {
        format!("You've used all {limit} automations on the Free plan.")
    } else {
        format!("{owned} of {limit} automations used on the Free plan.")
    };
    Some(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(Expanded::new(1.0, label(ui, &text, 12.0, if full { theme.ui_yellow() } else { theme.sub_text() })).finish())
                .with_child(button(
                    theme,
                    ui,
                    view.view_id,
                    view.mouse("upgrade-strip"),
                    Some(AppIcon::ArrowUp),
                    "Upgrade to Pro for unlimited",
                    ButtonKind::Ghost,
                    Some(A::Upgrade),
                ))
                .finish(),
        )
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(12.0)
        .with_padding_right(4.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .finish(),
    )
}

fn banner(view: &AutomationsView, theme: &Theme, message: &str, color: ColorU, upgrade: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(Expanded::new(1.0, label(ui, message, 12.5, theme.main_text())).finish());
    if upgrade {
        row = row.with_child(button(
            theme,
            ui,
            view.view_id,
            view.mouse("upgrade-banner"),
            None,
            "Upgrade",
            ButtonKind::Primary,
            Some(A::Upgrade),
        ));
    }
    row = row.with_child(button(
        theme,
        ui,
        view.view_id,
        view.mouse("dismiss"),
        None,
        "Dismiss",
        ButtonKind::Ghost,
        Some(A::DismissMessage),
    ));
    Container::new(row.finish())
        .with_background_color(ColorU::new(color.r, color.g, color.b, 24))
        .with_border(Border::all(1.0).with_border_color(ColorU::new(color.r, color.g, color.b, 80)))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .with_padding_left(12.0)
        .with_padding_right(6.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .finish()
}

// ── Summary cards ─────────────────────────────────────────────────────────────

fn percent(part: usize, total: usize) -> Option<String> {
    (total > 0).then(|| format!("{:.1}%", part as f64 * 100.0 / total as f64))
}

fn stat_cards(view: &AutomationsView, theme: &Theme, stats: Stats, failing_now: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let total = stats.created_7d + stats.failed_7d;
    let card = |title: String, value: usize, pct: Option<String>, accent: Option<ColorU>| {
        let mut value_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_spacing(6.0)
            .with_child(label(ui, &value.to_string(), 18.0, theme.main_text()));
        if let Some(pct) = pct {
            value_row = value_row.with_child(
                Container::new(label(ui, &pct, 11.5, theme.disabled_text()))
                    .with_padding_bottom(2.0)
                    .finish(),
            );
        }
        let border = accent.unwrap_or_else(|| theme.outline());
        Container::new(
            Flex::column()
                .with_spacing(3.0)
                .with_child(label(ui, &title, 11.5, theme.sub_text()))
                .with_child(value_row.finish())
                .finish(),
        )
        .with_background_color(accent.map_or(ColorU::transparent_black(), |c| ColorU::new(c.r, c.g, c.b, 14)))
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .with_padding_left(14.0)
        .with_padding_right(14.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .finish()
    };
    let failed_title = if view.failed_only {
        "Failed · 7d · filtering".to_string()
    } else {
        "Failed · 7d".to_string()
    };
    let failed_card = card(
        failed_title,
        stats.failed_7d,
        percent(stats.failed_7d, total),
        view.failed_only.then(|| theme.ui_red()),
    );
    let failed: Box<dyn Element> = if failing_now || view.failed_only {
        Hoverable::new(view.mouse("stat-failed"), move |_| failed_card)
            .with_cursor(Cursor::PointingHand)
            .on_click({
                let id = view.view_id;
                move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::ToggleFailedFilter)
            })
            .finish()
    } else {
        failed_card
    };
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(8.0)
        .with_child(Expanded::new(1.0, card("Active".into(), stats.active, None, None)).finish())
        .with_child(
            Expanded::new(
                1.0,
                card(
                    "Started · 7d".into(),
                    stats.created_7d,
                    percent(stats.created_7d, total),
                    None,
                ),
            )
            .finish(),
        )
        .with_child(Expanded::new(1.0, failed).finish())
        .finish()
}

// ── Tabs and table ────────────────────────────────────────────────────────────

fn tabs_row(
    view: &AutomationsView,
    theme: &Theme,
    mine: usize,
    team: usize,
    unread: usize,
    failing: &HashSet<String>,
    tab_empty: bool,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let tab = |key: &'static str, text: &'static str, count: usize, scope: Scope| {
        let selected = view.scope == scope;
        let theme = *theme;
        Hoverable::new(view.mouse(key), move |state| {
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(label(
                        ui,
                        text,
                        13.0,
                        if selected || state.is_hovered() { theme.main_text() } else { theme.sub_text() },
                    ))
                    .with_child(label(ui, &count.to_string(), 11.5, theme.disabled_text()))
                    .finish(),
            )
            .with_background_color(if selected { theme.surface_3() } else { ColorU::transparent_black() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::SetScope(scope)))
        .finish()
    };
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0)
        .with_child(tab("scope-mine", "Mine", mine, Scope::Mine))
        .with_child(tab("scope-team", "Team", team, Scope::Team))
        .with_child(tab("scope-inbox", "Inbox", unread, Scope::Inbox))
        .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if view.scope == Scope::Inbox {
        if unread > 0 && !view.inbox_archived {
            row = row.with_child(button(
                theme,
                ui,
                id,
                view.mouse("inbox-read-all"),
                Some(AppIcon::Check),
                "Mark all read",
                ButtonKind::Ghost,
                Some(A::MarkAllRead),
            ));
        }
        row = row.with_child(button(
            theme,
            ui,
            id,
            view.mouse("inbox-archived"),
            None,
            if view.inbox_archived { "Back to inbox" } else { "Archived" },
            ButtonKind::Secondary,
            Some(A::ToggleInboxArchived),
        ));
    }
    if !tab_empty {
        if view.scope == Scope::Mine && !failing.is_empty() {
            row = row.with_child(button(
                theme,
                ui,
                id,
                view.mouse("retry-all"),
                Some(AppIcon::Undo2),
                &format!("Retry all {}", failing.len()),
                ButtonKind::Secondary,
                Some(A::RetryAll),
            ));
        }
        row = row.with_child(
            ConstrainedBox::new(super::widgets::field(theme, &view.search_editor, 0.0))
                .with_width(200.0)
                .finish(),
        );
    }
    row.finish()
}

/// Fixed column widths after the flexible name column.
struct Columns {
    owner: Option<f32>,
    schedule: f32,
    status: f32,
    last_run: f32,
    actions: f32,
}

fn columns(team: bool) -> Columns {
    Columns {
        owner: team.then_some(130.0),
        schedule: if team { 170.0 } else { 190.0 },
        status: 140.0,
        last_run: 150.0,
        actions: if team { 16.0 } else { 70.0 },
    }
}

fn cell(width: f32, child: Box<dyn Element>) -> Box<dyn Element> {
    ConstrainedBox::new(child).with_width(width).finish()
}

fn header_cell(view: &AutomationsView, theme: &Theme, text: &str, field: Option<SortField>) -> Box<dyn Element> {
    let ui = view.ui_font;
    let Some(field) = field else {
        return label(ui, text, 10.5, theme.disabled_text());
    };
    let arrow = match view.sort {
        Some((current, true)) if current == field => Some(AppIcon::ArrowUp),
        Some((current, false)) if current == field => Some(AppIcon::ArrowDown),
        _ => None,
    };
    let theme = *theme;
    let text = text.to_string();
    let id = view.view_id;
    Hoverable::new(view.mouse(format!("sort-{field:?}")), move |state| {
        let color = if state.is_hovered() || arrow.is_some() { theme.main_text() } else { theme.disabled_text() };
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(label(ui, &text, 10.5, color));
        if let Some(arrow) = arrow {
            row = row.with_child(render_icon(arrow, color, 10.0));
        }
        row.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::Sort(field)))
    .finish()
}

fn table_header(view: &AutomationsView, theme: &Theme, team: bool) -> Box<dyn Element> {
    let cols = columns(team);
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(Expanded::new(1.0, header_cell(view, theme, "NAME", Some(SortField::Name))).finish());
    if let Some(width) = cols.owner {
        row = row.with_child(cell(width, header_cell(view, theme, "OWNER", Some(SortField::Owner))));
    }
    row = row
        .with_child(cell(cols.schedule, header_cell(view, theme, "SCHEDULE", Some(SortField::Schedule))))
        .with_child(cell(cols.status, header_cell(view, theme, "STATUS", Some(SortField::Status))))
        .with_child(cell(cols.last_run, header_cell(view, theme, "LAST RUN", None)))
        .with_child(cell(cols.actions, Empty::new().finish()));
    Container::new(row.finish())
        .with_padding_left(16.0)
        .with_padding_right(12.0)
        .with_padding_top(9.0)
        .with_padding_bottom(9.0)
        .finish()
}

fn section_row(view: &AutomationsView, theme: &Theme, text: &str, alert: bool) -> Box<dyn Element> {
    Container::new(label(
        view.ui_font,
        text,
        10.0,
        if alert { theme.ui_red() } else { theme.disabled_text() },
    ))
    .with_background_color(theme.surface_1())
    .with_padding_left(16.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .finish()
}

fn table(view: &AutomationsView, theme: &Theme, team: bool, rows: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(table_header(view, theme, team));
    if rows.is_empty() {
        column = column.with_child(divider(theme)).with_child(
            Container::new(Align::new(label(view.ui_font, "No automations match.", 12.5, theme.sub_text())).finish())
                .with_padding_top(30.0)
                .with_padding_bottom(30.0)
                .finish(),
        );
    }
    for row in rows {
        column = column.with_child(divider(theme)).with_child(row);
    }
    Container::new(column.finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .finish()
}

fn matches_search(view: &AutomationsView, name: &str, extra: &str) -> bool {
    let query = view.search.trim().to_lowercase();
    query.is_empty() || name.to_lowercase().contains(&query) || extra.to_lowercase().contains(&query)
}

fn mine_table(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automations: &[Automation],
    elsewhere: &[&TeamAutomation],
    failing: &HashSet<String>,
) -> Box<dyn Element> {
    let latest = runtime.latest_runs();
    let next: HashMap<String, Option<i64>> = automations
        .iter()
        .map(|a| {
            let at = a
                .enabled
                .then(|| runtime.scheduled_run(&a.id))
                .flatten()
                .and_then(|run| run.scheduled_at_ms);
            (a.id.clone(), at)
        })
        .collect();
    let visible: Vec<&Automation> = automations
        .iter()
        .filter(|a| !view.failed_only || failing.contains(&a.id))
        .filter(|a| matches_search(view, &a.name, &project_label(view, &a.project_root)))
        .collect();
    let row = |a: &Automation| automation_row(view, theme, runtime, a, latest.get(&a.id), next[&a.id]);

    let mut rows = Vec::new();
    match view.sort {
        Some((field, ascending)) => {
            let mut sorted = visible.clone();
            sorted.sort_by(|a, b| {
                let key = |x: &Automation| match field {
                    SortField::Name | SortField::Owner => x.name.to_lowercase(),
                    SortField::Schedule => schedule_label(x),
                    SortField::Status => if x.enabled { "active" } else { "paused" }.to_string(),
                };
                let order = key(a).cmp(&key(b));
                if ascending { order } else { order.reverse() }
            });
            rows.extend(sorted.into_iter().map(row));
        }
        None => {
            // Failing first, then soonest next run, then paused.
            let attention: Vec<&&Automation> = visible.iter().filter(|a| failing.contains(&a.id)).collect();
            let mut upcoming: Vec<&&Automation> = visible
                .iter()
                .filter(|a| a.enabled && !failing.contains(&a.id))
                .collect();
            upcoming.sort_by_key(|a| next[&a.id].unwrap_or(i64::MAX));
            let paused: Vec<&&Automation> = visible
                .iter()
                .filter(|a| !a.enabled && !failing.contains(&a.id))
                .collect();
            for (title, group, alert) in [
                ("NEEDS ATTENTION", attention, true),
                ("UP NEXT", upcoming, false),
                ("PAUSED", paused, false),
            ] {
                if group.is_empty() {
                    continue;
                }
                rows.push(section_row(view, theme, title, alert));
                rows.extend(group.into_iter().map(|a| row(a)));
            }
        }
    }
    let remote: Vec<&&TeamAutomation> = elsewhere
        .iter()
        .filter(|a| !view.failed_only)
        .filter(|a| matches_search(view, &a.name, a.project_label.as_deref().unwrap_or("")))
        .collect();
    if !remote.is_empty() {
        rows.push(section_row(view, theme, "ON YOUR OTHER COMPUTERS", false));
        rows.extend(remote.into_iter().map(|a| team_row(view, theme, a, false)));
    }
    table(view, theme, false, rows)
}

// ── Inbox ─────────────────────────────────────────────────────────────────────

fn inbox_list(view: &AutomationsView, theme: &Theme, runtime: &AutomationRuntime) -> Box<dyn Element> {
    let ui = view.ui_font;
    let runs: Vec<Run> = runtime
        .inbox(view.inbox_archived)
        .into_iter()
        .filter(|run| {
            let reply = run
                .progress
                .as_ref()
                .and_then(|progress| progress.reply.as_deref())
                .unwrap_or("");
            matches_search(view, &run.snapshot.name, reply)
        })
        .collect();
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if runs.is_empty() {
        let message = if view.inbox_archived {
            "Nothing filed away."
        } else {
            "No results yet. When a run finishes, fails, or needs you, it shows up here."
        };
        column = column.with_child(
            Container::new(Align::new(label(ui, message, 12.5, theme.sub_text())).finish())
                .with_padding_top(30.0)
                .with_padding_bottom(30.0)
                .finish(),
        );
    }
    for (index, run) in runs.iter().enumerate() {
        if index > 0 {
            column = column.with_child(divider(theme));
        }
        column = column.with_child(inbox_row(view, theme, run));
    }
    let list = Container::new(column.finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .finish();
    let note = if view.inbox_archived {
        "Archived results stay for 30 days."
    } else {
        "A run whose agent replies “Nothing to report” is archived on its own."
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(list)
        .with_child(label(ui, note, 11.0, theme.disabled_text()))
        .finish()
}

fn inbox_row(view: &AutomationsView, theme: &Theme, run: &Run) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let theme_owned = *theme;
    let unread = run.is_unread();
    let now = now_ms();
    let mut meta = vec![run
        .result_at_ms()
        .map(|at| compact_ago(at, now))
        .unwrap_or_default()];
    meta.push(if run.snapshot.is_session() {
        "Session".to_string()
    } else {
        project_label(view, &run.snapshot.project_root)
    });
    if let Some(branch) = &run.branch {
        meta.push(branch.clone());
    }
    let meta = meta.into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" · ");
    let reply = run
        .progress
        .as_ref()
        .filter(|_| run.status == RunStatus::Started)
        .and_then(|progress| progress.reply.as_deref())
        .map(super::progress::reply_preview)
        .filter(|reply| !reply.is_empty());
    let note = match (&run.error, run.progress.as_ref().map(|progress| progress.state)) {
        (Some(error), _) if run.status == RunStatus::Failed => {
            Some((super::executor::describe_error(error), theme.ui_red()))
        }
        (_, Some(AgentState::NeedsInput)) => Some(("Waiting for you in its tab.".to_string(), theme.ui_yellow())),
        (_, Some(AgentState::TimedOut)) => Some((
            format!(
                "Stopped after waiting {} minutes for you. Open it to carry on.",
                super::progress::WAIT_LIMIT_MS / 60_000
            ),
            theme.ui_yellow(),
        )),
        _ => None,
    };
    let mut actions = Flex::row()
        .with_main_axis_alignment(MainAxisAlignment::End)
        .with_spacing(4.0);
    if reply.is_some() {
        actions = actions.with_child(button(
            theme,
            ui,
            id,
            view.mouse(format!("inbox-copy-{}", run.id)),
            Some(AppIcon::Copy),
            "Copy reply",
            ButtonKind::Ghost,
            Some(A::CopyRunReply(run.id.clone())),
        ));
    }
    let archived = run.is_archived();
    actions = actions.with_child(button(
        theme,
        ui,
        id,
        view.mouse(format!("inbox-archive-{}", run.id)),
        None,
        if archived { "Move to inbox" } else { "Archive" },
        ButtonKind::Ghost,
        Some(A::ArchiveRun(run.id.clone(), !archived)),
    ));
    let actions = actions.finish();
    let title = run.snapshot.name.clone();
    let badge_run = run.clone();
    let open = A::OpenInboxRun(run.id.clone());
    Hoverable::new(view.mouse(format!("inbox-{}", run.id)), move |state| {
        let theme = theme_owned;
        let marker: Box<dyn Element> = if unread {
            dot(theme.accent, 7.0)
        } else {
            ConstrainedBox::new(Empty::new().finish()).with_width(7.0).finish()
        };
        let top = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(marker)
            .with_child(
                Expanded::new(
                    1.0,
                    one_line(ui, &title, 13.0, if unread { theme.main_text() } else { theme.sub_text() }),
                )
                .finish(),
            )
            .with_child(status_badge(&theme, ui, &badge_run))
            .finish();
        let mut body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(5.0)
            .with_child(label(ui, &meta, 11.0, theme.disabled_text()));
        if let Some(reply) = &reply {
            body = body.with_child(
                Container::new(label(ui, reply, 11.5, theme.sub_text()))
                    .with_background_color(theme.surface_1())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .with_uniform_padding(7.0)
                    .finish(),
            );
        }
        if let Some((note, color)) = &note {
            body = body.with_child(label(ui, note, 11.5, *color));
        }
        body = body.with_child(actions);
        let column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(top)
            // Under the title, past the unread marker.
            .with_child(Container::new(body.finish()).with_padding_left(15.0).finish())
            .finish();
        Container::new(column)
            .with_background_color(if state.is_hovered() { theme.surface_2() } else { ColorU::transparent_black() })
            .with_padding_left(14.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(6.0)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, open.clone()))
    .finish()
}

fn row_menu_tag(id: &str) -> String {
    format!("automation_row_menu_{id}")
}

fn automation_row(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automation: &Automation,
    latest: Option<&Run>,
    next: Option<i64>,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let cols = columns(false);
    let now = now_ms();
    let over = runtime.is_over_allowance(&automation.id);
    let theme_owned = *theme;

    let name = automation.name.clone();
    let enabled = automation.enabled;
    let place = project_label(view, &automation.project_root);
    let session = automation.is_session();
    let schedule = schedule_label(automation);
    let status = if over {
        "Upgrade to run".to_string()
    } else if !enabled {
        "Paused".to_string()
    } else {
        match next {
            Some(at) => format!("Active · {}", compact_until(at, now)),
            None => "Active".to_string(),
        }
    };
    let status_color = if over { theme.ui_yellow() } else { theme.sub_text() };
    let open = A::Open(automation.id.clone());

    let main = Hoverable::new(view.mouse(format!("row-{}", automation.id)), move |state| {
        let theme = theme_owned;
        let name_cell = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Shrinkable::new(
                    1.0,
                    one_line(ui, &name, 13.0, if enabled { theme.main_text() } else { theme.sub_text() }),
                )
                .finish(),
            )
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(render_icon(
                        if session { AppIcon::Terminal } else { AppIcon::Folder },
                        theme.disabled_text(),
                        11.0,
                    ))
                    .with_child(
                        ConstrainedBox::new(one_line(ui, &place, 11.5, theme.disabled_text()))
                            .with_max_width(140.0)
                            .finish(),
                    )
                    .finish(),
            )
            .finish();
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(Expanded::new(1.0, name_cell).finish())
                .with_child(cell(cols.schedule, one_line(ui, &schedule, 12.0, theme.sub_text())))
                .with_child(cell(cols.status, one_line(ui, &status, 12.0, status_color)))
                .finish(),
        )
        .with_background_color(if state.is_hovered() { theme.surface_1() } else { ColorU::transparent_black() })
        .with_padding_left(16.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, open.clone()))
    .finish();

    // Last run: opens the run's workspace when there is one.
    let last: Box<dyn Element> = match latest {
        Some(run) => {
            let (color, word, failed) = latest_run_style(theme, &run);
            let ago = run.display_time_ms().map(|at| compact_ago(at, now)).unwrap_or_default();
            let openable = run.can_open();
            let automation_id = automation.id.clone();
            let theme = *theme;
            let word = word.to_string();
            let content = move |hovered: bool| {
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(dot(color, 6.0))
                    .with_child(label(
                        ui,
                        &word,
                        12.0,
                        if failed { theme.ui_red() } else if hovered { theme.main_text() } else { theme.sub_text() },
                    ))
                    .with_child(Shrinkable::new(1.0, one_line(ui, &ago, 11.5, theme.disabled_text())).finish())
                    .finish()
            };
            if openable {
                Hoverable::new(view.mouse(format!("last-{}", automation.id)), move |state| content(state.is_hovered()))
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ectx, _, _| {
                        ectx.dispatch_typed_action_for(id, A::OpenLastRun(automation_id.clone()))
                    })
                    .finish()
            } else {
                content(false)
            }
        }
        None => label(ui, "—", 12.0, theme.disabled_text()),
    };

    let run_button = button(
        theme,
        ui,
        id,
        view.mouse(format!("run-now-{}", automation.id)),
        Some(AppIcon::ArrowRight),
        "",
        ButtonKind::Ghost,
        (enabled && !over && !runtime.is_active(&automation.id)).then(|| A::RunAutomation(automation.id.clone())),
    );
    let menu_button = SavePosition::new(
        button(
            theme,
            ui,
            id,
            view.mouse(format!("row-menu-{}", automation.id)),
            Some(AppIcon::DotsVertical),
            "",
            ButtonKind::Ghost,
            Some(A::ToggleRowMenu(automation.id.clone())),
        ),
        &row_menu_tag(&automation.id),
    )
    .finish();

    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Expanded::new(1.0, main).finish())
        .with_child(
            Container::new(cell(cols.last_run, last))
                .with_padding_left(10.0)
                .finish(),
        )
        .with_child(
            Container::new(cell(
                cols.actions,
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(run_button)
                    .with_child(menu_button)
                    .finish(),
            ))
            .with_padding_left(10.0)
            .with_padding_right(8.0)
            .finish(),
        )
        .finish()
}

fn row_menu(view: &AutomationsView, theme: &Theme, runtime: &AutomationRuntime, automation: &Automation) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let key = |name: &str| view.mouse(format!("rowmenu-{name}-{}", automation.id));
    let over = runtime.is_over_allowance(&automation.id);
    let mut rows = vec![
        menu_item(theme, ui, id, key("edit"), AppIcon::Edit, "Edit", false, A::Open(automation.id.clone())),
        menu_item(theme, ui, id, key("link"), AppIcon::Copy, "Copy link", false, A::CopyLink(automation.id.clone())),
    ];
    if automation.enabled && !over {
        rows.push(menu_item(
            theme,
            ui,
            id,
            key("run"),
            AppIcon::ArrowRight,
            "Run now",
            false,
            A::RunAutomation(automation.id.clone()),
        ));
    }
    rows.push(menu_item(
        theme,
        ui,
        id,
        key("toggle"),
        AppIcon::Clock,
        if automation.enabled { "Pause" } else { "Resume" },
        false,
        A::ToggleEnabled(automation.id.clone()),
    ));
    rows.push(menu_item(
        theme,
        ui,
        id,
        key("history"),
        AppIcon::History,
        "Run history",
        false,
        A::ShowHistory(automation.id.clone()),
    ));
    rows.push(divider(theme));
    rows.push(menu_item(
        theme,
        ui,
        id,
        key("delete"),
        AppIcon::XClose,
        "Delete",
        true,
        A::RequestDeleteOf(automation.id.clone()),
    ));
    menu_card(theme, rows, 190.0)
}

fn team_table(
    view: &AutomationsView,
    theme: &Theme,
    runtime: &AutomationRuntime,
    automations: &[&TeamAutomation],
) -> Box<dyn Element> {
    let state = runtime.team();
    if crate::api::is_local_mode() {
        return team_message(view, theme, crate::api::NEEDS_ACCOUNT, None);
    }
    if runtime.account().user_id.is_none() {
        return team_message(view, theme, "Sign in to see your team's automations.", None);
    }
    if automations.is_empty() {
        if state.loading {
            return team_message(view, theme, "Loading your team's automations…", None);
        }
        if let Some(error) = &state.error {
            return team_message(view, theme, "Couldn't load your team's automations.", Some(error));
        }
        return team_message(
            view,
            theme,
            "No team automations",
            Some("Nobody else in this organization has automations yet."),
        );
    }
    let visible: Vec<&&TeamAutomation> = automations
        .iter()
        .filter(|a| matches_search(view, &a.name, &a.owner_name))
        .collect();
    let mut sorted = visible.clone();
    if let Some((field, ascending)) = view.sort {
        sorted.sort_by(|a, b| {
            let key = |x: &TeamAutomation| match field {
                SortField::Name => x.name.to_lowercase(),
                SortField::Owner => x.owner_name.to_lowercase(),
                SortField::Schedule => x.schedule_label.clone(),
                SortField::Status => if x.enabled { "active" } else { "paused" }.to_string(),
            };
            let order = key(a).cmp(&key(b));
            if ascending { order } else { order.reverse() }
        });
    }
    let rows = sorted.into_iter().map(|a| team_row(view, theme, a, true)).collect();
    table(view, theme, true, rows)
}

fn team_message(view: &AutomationsView, theme: &Theme, title: &str, detail: Option<&str>) -> Box<dyn Element> {
    let ui = view.ui_font;
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_child(label(ui, title, 14.0, theme.main_text()));
    if let Some(detail) = detail {
        column = column.with_child(label(ui, detail, 12.5, theme.sub_text()));
    }
    Container::new(Align::new(column.finish()).finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_top(48.0)
        .with_padding_bottom(48.0)
        .finish()
}

/// A registry entry (a teammate's, or yours from another computer):
/// read-only, opens its details.
fn team_row(view: &AutomationsView, theme: &Theme, automation: &TeamAutomation, show_owner: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let cols = columns(show_owner);
    let now = now_ms();
    let theme_owned = *theme;
    let name = automation.name.clone();
    let place = automation.project_label.clone().unwrap_or_else(|| "Session".into());
    let session = automation.project_label.is_none();
    let owner = if automation.owner_name.is_empty() {
        automation.owner_email.clone()
    } else {
        automation.owner_name.clone()
    };
    let schedule = if automation.schedule_label.is_empty() {
        describe_schedule(&automation.schedule)
    } else {
        automation.schedule_label.clone()
    };
    let enabled = automation.enabled;
    let status = if !enabled {
        "Paused".to_string()
    } else {
        match automation.next_run_at {
            Some(at) => format!("Active · {}", compact_until(at, now)),
            None => "Active".to_string(),
        }
    };
    let last = automation.last_run_status.clone().map(|status| {
        let parsed = RunStatus::parse(&status).unwrap_or(RunStatus::Started);
        let (color, word, failed) = last_run_style(theme, parsed);
        let ago = automation.last_run_at.map(|at| compact_ago(at, now)).unwrap_or_default();
        (color, word.to_string(), failed, ago)
    });
    let open = A::OpenTeam(automation.id.clone());
    Hoverable::new(view.mouse(format!("team-{}", automation.id)), move |state| {
        let theme = theme_owned;
        let name_cell = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Shrinkable::new(1.0, one_line(ui, &name, 13.0, if enabled { theme.main_text() } else { theme.sub_text() })).finish())
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(4.0)
                    .with_child(render_icon(if session { AppIcon::Terminal } else { AppIcon::Folder }, theme.disabled_text(), 11.0))
                    .with_child(ConstrainedBox::new(one_line(ui, &place, 11.5, theme.disabled_text())).with_max_width(120.0).finish())
                    .finish(),
            )
            .finish();
        let last_cell: Box<dyn Element> = match &last {
            Some((color, word, failed, ago)) => Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(dot(*color, 6.0))
                .with_child(label(ui, word, 12.0, if *failed { theme.ui_red() } else { theme.sub_text() }))
                .with_child(Shrinkable::new(1.0, one_line(ui, ago, 11.5, theme.disabled_text())).finish())
                .finish(),
            None => label(ui, "—", 12.0, theme.disabled_text()),
        };
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(Expanded::new(1.0, name_cell).finish());
        if let Some(width) = cols.owner {
            row = row.with_child(cell(width, one_line(ui, &owner, 12.0, theme.sub_text())));
        }
        Container::new(
            row
                .with_child(cell(cols.schedule, one_line(ui, &schedule, 12.0, theme.sub_text())))
                .with_child(cell(cols.status, one_line(ui, &status, 12.0, theme.sub_text())))
                .with_child(cell(cols.last_run, last_cell))
                .with_child(cell(cols.actions, Empty::new().finish()))
                .finish(),
        )
        .with_background_color(if state.is_hovered() { theme.surface_1() } else { ColorU::transparent_black() })
        .with_padding_left(16.0)
        .with_padding_right(12.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, open.clone()))
    .finish()
}

// ── Empty state and hints ─────────────────────────────────────────────────────

pub(crate) fn template_card(view: &AutomationsView, theme: &Theme, index: usize, compact: bool) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let template = &TEMPLATES[index];
    let theme_owned = *theme;
    let (name, summary, category, icon) = (template.name, template.summary, template.category, template.icon);
    Hoverable::new(view.mouse(format!("template-{index}-{compact}")), move |state| {
        let theme = theme_owned;
        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(7.0)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(8.0)
                        .with_child(render_icon(icon, theme.sub_text(), 14.0))
                        .with_child(Shrinkable::new(1.0, one_line(ui, name, 13.0, theme.main_text())).finish())
                        .finish(),
                )
                .with_child(label(ui, summary, 11.5, theme.sub_text()))
                .with_child(label(ui, category, 10.5, theme.disabled_text()))
                .finish(),
        )
        .with_background_color(if state.is_hovered() { theme.surface_2() } else { theme.surface_1() })
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(12.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, A::UseTemplate(index)))
    .finish()
}

fn empty_state(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let start = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(render_icon(AppIcon::Clock, theme.sub_text(), 22.0))
            .with_child(label(ui, "What should run on a schedule?", 16.0, theme.main_text()))
            .with_child(label(
                ui,
                "Describe it to an agent and let it set things up, or configure it yourself.",
                12.5,
                theme.sub_text(),
            ))
            .with_child(
                Container::new(
                    Flex::row()
                        .with_spacing(8.0)
                        .with_child(
                            SavePosition::new(
                                button(
                                    theme,
                                    ui,
                                    id,
                                    view.mouse("empty-ai"),
                                    Some(AppIcon::Sparkle),
                                    "Create with AI",
                                    ButtonKind::Primary,
                                    Some(A::CreateWithAgent(EMPTY_AI_TAG)),
                                ),
                                EMPTY_AI_TAG,
                            )
                            .finish(),
                        )
                        .with_child(button(
                            theme,
                            ui,
                            id,
                            view.mouse("empty-manual"),
                            Some(AppIcon::Edit),
                            "Create manually",
                            ButtonKind::Secondary,
                            Some(A::NewAutomation),
                        ))
                        .finish(),
                )
                .with_padding_top(6.0)
                .finish(),
            )
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_padding_top(32.0)
    .with_padding_bottom(32.0)
    .finish();

    let mut grid = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(12.0)
        .with_run_spacing(12.0);
    for index in 0..TEMPLATES.len() {
        grid = grid.with_child(
            ConstrainedBox::new(template_card(view, theme, index, false))
                .with_width(286.0)
                .finish(),
        );
    }
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(18.0)
        .with_child(start)
        .with_child(label(ui, "SUGGESTED", 10.5, theme.disabled_text()))
        .with_child(grid.finish())
        .finish()
}

fn cli_hint(view: &AutomationsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(render_icon(AppIcon::Terminal, theme.sub_text(), 13.0))
            .with_child(
                Expanded::new(
                    1.0,
                    label(
                        ui,
                        "Manage automations from a terminal too: run `dragent automations help` in a Dragent terminal, or ask an agent to.",
                        11.5,
                        theme.sub_text(),
                    ),
                )
                .finish(),
            )
            .with_child(button(
                theme,
                ui,
                view.view_id,
                view.mouse("cli-hint"),
                Some(AppIcon::XClose),
                "",
                ButtonKind::Ghost,
                Some(A::DismissCliHint),
            ))
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .with_padding_left(12.0)
    .with_padding_right(4.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish()
}
