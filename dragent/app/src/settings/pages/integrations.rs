//! Settings → Integrations, in three tabs: Services (what's connected, then
//! every other service by what it's for, as cards; a card opens its dialog —
//! the connection, or a form to connect it), Task defaults, and Agent access
//! (which coding agents get the connected services).
//!
//! Drawn from a [`Model`] rather than the whole settings view, so the UI
//! preview can draw it with sample data.

use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize,
    ParentElement, Shrinkable,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::integrations::{marks, Auth, Category, Provider, Source, Status};
use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::shared_input::{shared_input, SharedInputOpts};
use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::integrations::{IntegrationAction as I, IntegrationsState, IntegrationsTab};
use super::super::settings_view::SettingsView;
use super::super::widgets::{card_rows, page_header, toggle_row};

/// The narrowest a service card gets before the grid drops a column.
const CARD_MIN_WIDTH: f32 = 320.0;
const CARD_HEIGHT: f32 = 72.0;
const GRID_GAP: f32 = 12.0;
const DIALOG_WIDTH: f32 = 520.0;

/// What the page draws from.
pub(crate) struct Model<'a> {
    pub ui: FamilyId,
    pub theme: &'a Theme,
    pub view_id: EntityId,
    pub state: &'a IntegrationsState,
    pub hover: &'a dyn Fn(&str) -> MouseStateHandle,
    /// Service cards per row.
    pub columns: usize,
}

/// Service cards per row in a content area `width` wide.
pub(crate) fn grid_columns(width: f32) -> usize {
    (((width + GRID_GAP) / (CARD_MIN_WIDTH + GRID_GAP)).floor() as usize).clamp(1, 3)
}

fn act(action: I) -> SettingsAction {
    SettingsAction::Integration(action)
}

pub fn render_integrations(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let hover = |key: &str| view.hover(key);
    page(
        &Model {
            ui: view.ui_font,
            theme,
            view_id: view.view_id,
            state: &view.integrations,
            hover: &hover,
            columns: grid_columns(super::super::settings_view::content_width(
                view.content_pane_width,
            )),
        },
        || super::super::settings_tasks_hosts::render_tasks_card(view, theme),
    )
}

/// The open dialog, over the page.
pub fn render_integration_dialog(view: &SettingsView, theme: &Theme) -> Option<Box<dyn Element>> {
    let hover = |key: &str| view.hover(key);
    dialog(&Model {
        ui: view.ui_font,
        theme,
        view_id: view.view_id,
        state: &view.integrations,
        hover: &hover,
        columns: 1,
    })
}

fn connected(state: &IntegrationsState, provider: Provider) -> bool {
    matches!(
        state.statuses.get(&provider),
        Some(Status::Connected { .. })
    )
}

/// The page: its tabs, and the open one. `task_defaults` draws the Task
/// defaults tab, which needs the whole settings view.
pub(crate) fn page(model: &Model, task_defaults: impl FnOnce() -> Box<dyn Element>) -> Box<dyn Element> {
    let (ui, theme) = (model.ui, model.theme);
    let linked: Vec<Provider> = Provider::ALL
        .into_iter()
        .filter(|p| p.offered() && connected(model.state, *p))
        .collect();
    let open = model.state.tab;
    let tabs = [
        (IntegrationsTab::Services, AppIcon::Plug, "Services", Some(linked.len())),
        (IntegrationsTab::Tasks, AppIcon::TaskList, "Task defaults", None),
        (IntegrationsTab::Agents, AppIcon::Agent, "Agent access", None),
    ]
    .into_iter()
    .map(|(tab, icon, label, count)| {
        mui::tab(
            theme,
            ui,
            model.view_id,
            (model.hover)(&format!("integrations_tab_{label}")),
            icon,
            label,
            count,
            open == tab,
            act(I::Tab(tab)),
        )
    })
    .collect();
    let content = match open {
        IntegrationsTab::Services => services(model, &linked),
        IntegrationsTab::Tasks => task_defaults(),
        IntegrationsTab::Agents => agent_access(model),
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui,
            "Integrations",
            "Connect the tools your work lives in, then start a workspace from any of their \
             issues, tasks or messages with + on the new-workspace screen.",
        ))
        .with_child(mui::tab_strip(theme, tabs))
        .with_child(Container::new(content).with_padding_top(24.0).finish())
        .finish()
}

/// An eyebrow over what it names.
fn group(model: &Model, title: &str, body: Box<dyn Element>) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(mui::eyebrow(model.ui, model.theme, title))
        .with_child(body)
        .finish()
}

/// What's connected, then every other service by what it's for.
fn services(model: &Model, linked: &[Provider]) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(28.0);
    if !linked.is_empty() {
        let rows = linked
            .iter()
            .map(|provider| connected_row(model, *provider))
            .collect();
        column = column.with_child(group(model, "Connected", mui::list_card(model.theme, rows)));
    }
    let categories = [
        Category::Code,
        Category::Issues,
        Category::Projects,
        Category::Docs,
        Category::Support,
        Category::Monitoring,
        Category::Messages,
    ];
    for category in categories {
        let providers: Vec<Provider> = Provider::ALL
            .into_iter()
            .filter(|p| p.offered() && p.category() == category && !connected(model.state, *p))
            .collect();
        if !providers.is_empty() {
            column = column.with_child(group(model, category.label(), grid(model, &providers)));
        }
    }
    column.finish()
}

/// A connected service: who it's connected as and how, and a way in.
fn connected_row(model: &Model, provider: Provider) -> Box<dyn Element> {
    let (ui, theme) = (model.ui, model.theme);
    let Some(Status::Connected { account, source }) = model.state.statuses.get(&provider) else {
        return warpui::elements::Empty::new().finish();
    };
    let detail = match account {
        Some(account) => format!("{account} · {}", source.label()),
        None => source.label(),
    };
    let pill = match source {
        Source::Team { .. } => mui::pill(theme, ui, "Team", Tone::Info),
        _ => mui::pill(theme, ui, "Connected", Tone::Success),
    };
    let manage = mui::button(
        theme,
        ui,
        model.view_id,
        (model.hover)(&format!("integration_manage_{}", provider.key())),
        None,
        "Manage",
        Variant::Secondary,
        Size::Xs,
        true,
        act(I::Open(provider)),
    );
    mui::list_row(
        ui,
        theme,
        Some(marks::tile(theme, ui, provider, 32.0)),
        mui::medium(ui, provider.label(), 13.5, theme.main_text()),
        Some(detail),
        vec![pill, manage],
    )
}

/// Cards in rows of `model.columns`, every card as wide as the others.
fn grid(model: &Model, providers: &[Provider]) -> Box<dyn Element> {
    let columns = model.columns.max(1);
    let mut rows = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(GRID_GAP);
    for chunk in providers.chunks(columns) {
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(GRID_GAP);
        for provider in chunk {
            row = row.with_child(Expanded::new(1.0, card(model, *provider)).finish());
        }
        for _ in chunk.len()..columns {
            row = row.with_child(
                Expanded::new(1.0, warpui::elements::Empty::new().finish()).finish(),
            );
        }
        rows = rows.with_child(row.finish());
    }
    rows.finish()
}

/// A service that isn't connected: its logo, name and what it does, and
/// what clicking it does.
fn card(model: &Model, provider: Provider) -> Box<dyn Element> {
    let (ui, theme) = (model.ui, *model.theme);
    let needs_cli = provider == Provider::Github
        && matches!(model.state.statuses.get(&provider), Some(Status::Unavailable(_)));
    let view_id = model.view_id;
    Hoverable::new(
        (model.hover)(&format!("integration_card_{}", provider.key())),
        move |state| {
            let hovered = state.is_hovered();
            let trailing = if needs_cli {
                mui::pill(&theme, ui, "Needs GitHub CLI", Tone::Warning)
            } else {
                connect_chip(&theme, ui, hovered)
            };
            // The action sits on the name's line so the description gets the
            // card's whole width.
            let title = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(
                    Expanded::new(1.0, mui::medium(ui, provider.label(), 13.5, theme.main_text()))
                        .finish(),
                )
                .with_child(trailing)
                .finish();
            let labels = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(2.0)
                .with_child(title)
                .with_child(mui::line(ui, provider.description(), 12.0, theme.sub_text()))
                .finish();
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(marks::tile(&theme, ui, provider, 36.0))
                .with_child(Expanded::new(1.0, labels).finish())
                .finish();
            ConstrainedBox::new(
                Container::new(warpui::elements::Align::new(row).left().finish())
                    .with_background_color(if hovered {
                        theme.surface_1()
                    } else {
                        warpui::color::ColorU::transparent_black()
                    })
                    .with_border(Border::all(1.0).with_border_color(if hovered {
                        theme.border()
                    } else {
                        theme.outline()
                    }))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(mui::RADIUS_CARD)))
                    .with_padding_left(14.0)
                    .with_padding_right(14.0)
                    .finish(),
            )
            .with_height(CARD_HEIGHT)
            .finish()
        },
    )
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, act(I::Open(provider))))
    .finish()
}

/// The card's "Connect": drawn like a small button, but the whole card is
/// what takes the click.
fn connect_chip(theme: &Theme, ui: FamilyId, hovered: bool) -> Box<dyn Element> {
    let color = if hovered { theme.accent } else { theme.sub_text() };
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.0)
            .with_child(render_icon(AppIcon::Plus, color, 12.0))
            .with_child(mui::medium(ui, "Connect", 12.0, color))
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(if hovered {
        theme.accent
    } else {
        theme.outline()
    }))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
    .with_padding_left(9.0)
    .with_padding_right(10.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish()
}

/// Which coding agents get the connected services, each with its icon.
fn agent_access(model: &Model) -> Box<dyn Element> {
    use crate::agent::mcp_agent_sync::{prompt_agent, SyncAgent};
    use crate::workspace::PromptAgent;
    let (ui, theme) = (model.ui, model.theme);
    let on = model.state.agent_tools;
    let switch = card_rows(
        theme,
        vec![toggle_row(
            theme,
            ui,
            model.view_id,
            (model.hover)("integrations-agent-tools"),
            "Let coding agents use these services",
            Some(
                "Agents on this computer get Dragent's tools to search, read and comment on what's \
                 connected, through the dragent MCP server added to their settings.",
            ),
            on,
            act(I::ToggleAgentTools),
        )],
    );

    let mut rows = vec![mui::list_row(
        ui,
        theme,
        Some(agent_tile(theme, PromptAgent::Dragent, true)),
        mui::medium(ui, "Dragent", 13.5, theme.main_text()),
        Some("Always has them, and asks before it posts anything.".into()),
        vec![mui::pill(theme, ui, "Built in", Tone::Info)],
    )];
    for agent in SyncAgent::ALL {
        let present = model.state.agents_present.contains(&agent);
        let (detail, pill) = match (present, on) {
            (true, true) => (
                "The dragent MCP server is in its settings.",
                mui::pill(theme, ui, "On", Tone::Success),
            ),
            (true, false) => (
                "Turn on access above to give it the services.",
                mui::pill(theme, ui, "Off", Tone::Neutral),
            ),
            (false, _) => (
                "Not found on this computer.",
                mui::pill(theme, ui, "Not installed", Tone::Neutral),
            ),
        };
        rows.push(mui::list_row(
            ui,
            theme,
            Some(agent_tile(theme, prompt_agent(agent), present)),
            mui::medium(
                ui,
                agent.label(),
                13.5,
                if present { theme.main_text() } else { theme.sub_text() },
            ),
            Some(detail.into()),
            vec![pill],
        ));
    }

    let kept = if model.state.server {
        "Your connections stay on this computer, in its keychain, and requests go straight to the service. \
         Connections shared with your team are kept encrypted on Dragent's server, so teammates can use them."
    } else {
        "Connections are yours and stay on this computer, in its keychain."
    };
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(28.0)
        .with_child(switch)
        .with_child(group(model, "Agents", mui::list_card(theme, rows)))
        .with_child(mui::notice(
            theme,
            ui,
            Tone::Neutral,
            "Automation runs only read from these services unless the automation allows changes.",
            Some(kept.to_string()),
            None,
        ))
        .finish()
}

/// An agent's icon on a quiet tile, dimmed when it isn't installed.
fn agent_tile(theme: &Theme, agent: crate::workspace::PromptAgent, present: bool) -> Box<dyn Element> {
    Container::new(mui::sized(
        crate::workspace::prompt_agent_image(agent, theme, 18.0, if present { 1.0 } else { 0.45 }),
        32.0,
        32.0,
    ))
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .finish()
}

fn input(model: &Model, provider: Provider, field: &str) -> Option<Box<dyn Element>> {
    let editor = model.state.fields.get(&(provider, field))?;
    Some(shared_input(
        model.theme,
        editor,
        SharedInputOpts {
            height: Some(mui::CONTROL_HEIGHT),
            radius: mui::RADIUS_CONTROL,
            pad_left: 11.0,
            pad_right: 11.0,
            ..Default::default()
        },
    ))
}

fn button(
    model: &Model,
    key: &str,
    icon: Option<crate::icons::AppIcon>,
    label: &str,
    variant: Variant,
    enabled: bool,
    action: I,
) -> Box<dyn Element> {
    mui::button(
        model.theme,
        model.ui,
        model.view_id,
        (model.hover)(key),
        icon,
        label,
        variant,
        Size::Md,
        enabled,
        act(action),
    )
}

/// The open provider's dialog.
pub(crate) fn dialog(model: &Model) -> Option<Box<dyn Element>> {
    let provider = model.state.open?;
    let (ui, theme) = (model.ui, model.theme);
    let state = model.state;
    let status = state.statuses.get(&provider).cloned();
    let busy = state.busy == Some(provider);
    let key = provider.key();

    let title = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(marks::tile(theme, ui, provider, 40.0))
        .with_child(
            Shrinkable::new(
                1.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(3.0)
                    .with_child(mui::semibold(ui, provider.label(), 16.0, theme.main_text()))
                    .with_child(mui::text(
                        ui,
                        format!(
                            "{} · Link its {}s to a workspace",
                            provider.category().label(),
                            provider.item()
                        ),
                        12.5,
                        theme.sub_text(),
                    ))
                    .finish(),
            )
            .finish(),
        )
        .finish();

    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(16.0)
        .with_child(mui::text(
            ui,
            provider.description(),
            13.0,
            theme.main_text(),
        ));
    let (help, _) = provider.help();
    let mut left: Option<Box<dyn Element>> = None;
    let mut actions: Vec<Box<dyn Element>> = Vec::new();
    let close = |label: &str, variant| {
        button(
            model,
            &format!("integration_close_{key}"),
            None,
            label,
            variant,
            true,
            I::Close,
        )
    };

    match (provider.auth(), &status) {
        (Auth::GithubCli, None) => {
            body = body.with_child(mui::notice(
                theme,
                ui,
                Tone::Neutral,
                "Checking the GitHub CLI…",
                None,
                None,
            ));
            actions.push(close("Done", Variant::Primary));
        }
        (Auth::GithubCli, Some(Status::Connected { account, .. })) => {
            body = body.with_child(mui::notice(
                theme,
                ui,
                Tone::Success,
                &match account {
                    Some(login) => format!("Signed in as {login}"),
                    None => "Signed in".to_string(),
                },
                Some("Through the GitHub CLI. Issues and pull requests come from each project's own repository.".into()),
                None,
            ));
            actions.push(button(
                model,
                &format!("integration_refresh_{key}"),
                Some(crate::icons::AppIcon::RefreshCw),
                "Check again",
                Variant::Secondary,
                true,
                I::Refresh,
            ));
            actions.push(close("Done", Variant::Primary));
        }
        (Auth::GithubCli, Some(status)) => {
            let reason = match status {
                Status::Unavailable(reason) => reason.clone(),
                _ => "The GitHub CLI isn't set up.".into(),
            };
            body = body.with_child(mui::notice(
                theme,
                ui,
                Tone::Warning,
                &reason,
                Some(help.to_string()),
                None,
            ));
            left = Some(button(
                model,
                &format!("integration_help_{key}"),
                None,
                "Get the GitHub CLI",
                Variant::Ghost,
                true,
                I::OpenHelp(provider),
            ));
            actions.push(button(
                model,
                &format!("integration_refresh_{key}"),
                Some(crate::icons::AppIcon::RefreshCw),
                "Check again",
                Variant::Secondary,
                true,
                I::Refresh,
            ));
            actions.push(close("Done", Variant::Primary));
        }
        (_, Some(Status::Connected { account, source })) => {
            let team = matches!(source, Source::Team { .. });
            let disconnect = source.removable().then(|| {
                mui::button(
                    theme,
                    ui,
                    model.view_id,
                    (model.hover)(&format!("integration_disconnect_{key}")),
                    None,
                    "Disconnect",
                    Variant::Destructive,
                    Size::Xs,
                    true,
                    act(I::Disconnect(provider)),
                )
            });
            body = body.with_child(mui::list_card(
                theme,
                vec![mui::list_row(
                    ui,
                    theme,
                    Some(if team {
                        mui::pill(theme, ui, "Team", Tone::Info)
                    } else {
                        mui::pill(theme, ui, "Connected", Tone::Success)
                    }),
                    mui::medium(
                        ui,
                        account
                            .clone()
                            .unwrap_or_else(|| provider.label().to_string()),
                        13.0,
                        theme.main_text(),
                    ),
                    Some(source.label().to_string()),
                    disconnect.into_iter().collect(),
                )],
            ));
            body = body.with_child(mui::text(
                ui,
                format!("Link {} with + on the new-workspace screen; the agent gets its text and its branch is named after it.", provider.an_item()),
                12.0,
                theme.sub_text(),
            ));
            actions.push(close("Done", Variant::Primary));
        }
        (Auth::OAuth(_), _) => {
            if let Some(reason) = provider.unavailable() {
                body = body.with_child(mui::notice(
                    theme,
                    ui,
                    Tone::Warning,
                    reason,
                    Some(format!("{} needs Dragent's own app registered with {}; it comes in a later release.", provider.label(), if provider == Provider::Gmail { "Google" } else { "Microsoft" })),
                    None,
                ));
                actions.push(close("Close", Variant::Primary));
            } else {
                body = body.with_child(mui::notice(
                    theme,
                    ui,
                    Tone::Info,
                    "You'll sign in in your browser",
                    Some(help.to_string()),
                    None,
                ));
                actions.push(close("Cancel", Variant::Secondary));
                let label = if busy {
                    "Waiting for the browser…".to_string()
                } else {
                    format!(
                        "Sign in with {}",
                        if provider == Provider::Gmail {
                            "Google"
                        } else {
                            "Microsoft"
                        }
                    )
                };
                actions.push(button(
                    model,
                    &format!("integration_connect_{key}"),
                    None,
                    &label,
                    Variant::Primary,
                    !busy,
                    I::Connect(provider),
                ));
            }
        }
        (Auth::Fields(fields), _) => {
            let mut form = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(14.0);
            if state.server && state.admin && provider.shareable() {
                let choice = mui::segmented(
                    theme,
                    ui,
                    model.view_id,
                    vec![
                        mui::Segment {
                            hover: (model.hover)(&format!("integration_mine_{key}")),
                            icon: None,
                            label: "Just me".into(),
                            selected: !state.shared,
                            action: act(I::SetShared(false)),
                        },
                        mui::Segment {
                            hover: (model.hover)(&format!("integration_team_{key}")),
                            icon: None,
                            label: "Everyone in the team".into(),
                            selected: state.shared,
                            action: act(I::SetShared(true)),
                        },
                    ],
                    true,
                );
                let hint = if state.shared {
                    "Everyone in the organization, and their agents, use this key. Use a key that belongs to the workspace, not to a person."
                } else {
                    "Only you, and your agents, use this key."
                };
                form = form.with_child(mui::field(ui, theme, "Who uses it", choice, Some(hint)));
            }
            for field in fields {
                let Some(control) = input(model, provider, field.id) else {
                    continue;
                };
                let hint = field.default.map(|default| {
                    format!(
                        "Leave empty for {}.",
                        default.trim_start_matches("https://")
                    )
                });
                form =
                    form.with_child(mui::field(ui, theme, field.label, control, hint.as_deref()));
            }
            body = body.with_child(form.finish());
            body = body.with_child(mui::notice(theme, ui, Tone::Neutral, help, None, None));
            left = Some(button(
                model,
                &format!("integration_help_{key}"),
                None,
                "Where to find it",
                Variant::Ghost,
                true,
                I::OpenHelp(provider),
            ));
            actions.push(close("Cancel", Variant::Secondary));
            actions.push(button(
                model,
                &format!("integration_connect_{key}"),
                None,
                if busy { "Checking…" } else { "Connect" },
                Variant::Primary,
                !busy,
                I::Connect(provider),
            ));
        }
    }
    if let Some((_, error)) = state
        .error
        .as_ref()
        .filter(|(errored, _)| *errored == provider)
    {
        body = body.with_child(mui::notice(
            theme,
            ui,
            Tone::Error,
            "That didn't work",
            Some(error.clone()),
            None,
        ));
    }

    Some(mui::dialog(
        theme,
        ui,
        model.view_id,
        (model.hover)(&format!("integration_x_{key}")),
        &state.scroll,
        title,
        body.finish(),
        Some(mui::dialog_footer(left, actions)),
        DIALOG_WIDTH,
        Some(act(I::Close)),
    ))
}
