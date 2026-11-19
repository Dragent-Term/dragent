//! The header's Usage button, which opens Settings → Usage. It warns when an
//! account new sessions use is close to a plan limit (readings from
//! `dragent_host::agent_usage`).

use dragent_host::agent_usage::AccountUsage;
use warpui::color::ColorU;
use warpui::elements::{
    ChildAnchor, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisSize, OffsetPositioning, ParentElement, PositionedElementAnchor,
    PositionedElementOffsetBounds, SavePosition, Text,
};
use std::cell::RefCell;
use std::collections::HashMap;

use warpui::elements::{ClippedScrollStateHandle, ClippedScrollable, ScrollbarWidth, Shrinkable};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::EntityId;
use warpui_core::elements::{DispatchEventResult, EventHandler, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::action::PromptAgent;
use super::WorkspaceAction;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, Theme};

/// SavePosition tag the usage panel anchors to.
pub const USAGE_MENU_TAG: &str = "header_agent_usage";

/// A window at or above this share of its limit is shown as a warning.
const WARN_PERCENT: f64 = 80.0;
/// …and at or above this one as nearly out.
const CRITICAL_PERCENT: f64 = 95.0;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn level_color(theme: &Theme, percent: f64) -> ColorU {
    if percent >= CRITICAL_PERCENT {
        theme.ui_red()
    } else if percent >= WARN_PERCENT {
        theme.ui_yellow()
    } else {
        theme.accent
    }
}

/// The most used window among the accounts new sessions use: what the chip
/// shows.
pub fn headline(accounts: &[AccountUsage]) -> Option<f64> {
    let now = now();
    accounts
        .iter()
        .filter(|account| account.default)
        .filter_map(|account| account.usage.as_ref()?.highest(now))
        .map(|window| window.used_percent)
        .max_by(f64::total_cmp)
}

/// Header button: "Usage", opening the usage popover. While a default
/// account is near a plan limit it also shows how near, in warning colours.
pub fn render_usage_button(
    ui_font: FamilyId,
    theme: &Theme,
    view_id: EntityId,
    mouse_state: MouseStateHandle,
    headline: Option<f64>,
    open: bool,
) -> Box<dyn Element> {
    let theme = *theme;
    let button = Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered() || open;
        let rest = if hovered { theme.main_text() } else { theme.sub_text() };
        let warn = headline.filter(|percent| *percent >= WARN_PERCENT);
        let icon_color = warn.map_or(rest, |percent| level_color(&theme, percent));
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(5.0)
            .with_child(render_icon(AppIcon::Asset("icons/activity.svg"), icon_color, 13.0))
            .with_child(Text::new("Usage".to_string(), ui_font, text_size(11.5)).with_color(rest).finish());
        if let Some(percent) = warn {
            row = row.with_child(
                Text::new(format!("{percent:.0}%"), ui_font, text_size(11.0))
                    .with_color(level_color(&theme, percent))
                    .finish(),
            );
        }
        Container::new(row.finish())
            .with_horizontal_padding(7.0)
            .with_vertical_padding(4.0)
            .with_background_color(if hovered { theme.surface_1() } else { ColorU::transparent_black() })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(view_id, WorkspaceAction::ToggleUsageMenu);
    })
    .finish();
    SavePosition::new(button, USAGE_MENU_TAG).finish()
}



// ── Usage popover ────────────────────────────────────────────────────────────
//
// One tab per installed agent CLI (its own icon), and for the chosen agent:
// the accounts Dragent can start it with, which one new sessions use, and
// how much of each plan limit is used. Two ways out: the detailed usage
// screen and the agent accounts screen.

type States = RefCell<HashMap<String, MouseStateHandle>>;

const POPOVER_WIDTH: f32 = 384.0;

fn state(states: &States, key: &str) -> MouseStateHandle {
    states.borrow_mut().entry(key.to_string()).or_default().clone()
}

/// The provider key `agent_usage` uses for an agent ("claude", "codex").
fn provider_of(agent: PromptAgent) -> &'static str {
    agent.command_key()
}

/// Accounts with limit readings for `agent`, default first.
fn accounts_for<'a>(accounts: &'a [AccountUsage], agent: PromptAgent) -> Vec<&'a AccountUsage> {
    let mut rows: Vec<&AccountUsage> =
        accounts.iter().filter(|account| account.provider == provider_of(agent)).collect();
    rows.sort_by_key(|account| !account.default);
    rows
}

/// The installed agents the popover lists, in the app's usual order. Agents
/// that report limits are listed even before the install check has run.
pub fn popover_agents(installed: &std::collections::HashSet<PromptAgent>, accounts: &[AccountUsage]) -> Vec<PromptAgent> {
    PromptAgent::ALL
        .into_iter()
        .filter(|agent| *agent != PromptAgent::Dragent)
        .filter(|agent| {
            installed.contains(agent) || accounts.iter().any(|account| account.provider == provider_of(*agent))
        })
        .collect()
}

/// The tab to show when the popover opens: an agent near a limit, else the
/// first agent with a reading, else the first one.
pub fn initial_agent(agents: &[PromptAgent], accounts: &[AccountUsage]) -> Option<PromptAgent> {
    let now = now();
    let peak = |agent: PromptAgent| {
        accounts_for(accounts, agent)
            .iter()
            .filter_map(|account| account.usage.as_ref()?.highest(now).map(|window| window.used_percent))
            .max_by(f64::total_cmp)
    };
    agents
        .iter()
        .copied()
        .find(|agent| peak(*agent).is_some_and(|percent| percent >= WARN_PERCENT))
        .or_else(|| agents.iter().copied().find(|agent| peak(*agent).is_some()))
        .or_else(|| agents.first().copied())
}

fn text(value: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), font, text_size(size)).with_color(color).finish()
}

fn strong(value: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(value.into(), font, text_size(size))
        .with_color(color)
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish()
}

fn divider(theme: &Theme) -> Box<dyn Element> {
    ConstrainedBox::new(Container::new(Empty::new().finish()).with_background_color(theme.outline()).finish())
        .with_height(1.0)
        .finish()
}

/// A slim rounded meter.
fn meter(theme: &Theme, fraction: f32, color: ColorU) -> Box<dyn Element> {
    const HEIGHT: f32 = 5.0;
    let fraction = fraction.clamp(0.0, 1.0);
    ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_child(
                    Expanded::new(
                        fraction.max(0.0001),
                        Container::new(Empty::new().finish())
                            .with_background_color(color)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(HEIGHT / 2.0)))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(Expanded::new((1.0 - fraction).max(0.0001), Empty::new().finish()).finish())
                .finish(),
        )
        .with_background_color(theme.surface_3())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(HEIGHT / 2.0)))
        .finish(),
    )
    .with_height(HEIGHT)
    .finish()
}

/// A small rounded label ("Default", "Max").
fn pill(value: impl Into<String>, font: FamilyId, fg: ColorU, bg: ColorU, border: ColorU) -> Box<dyn Element> {
    Container::new(text(value, font, 10.0, fg))
        .with_horizontal_padding(7.0)
        .with_vertical_padding(2.0)
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .finish()
}

fn tinted(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

/// The agent icon row along the top, like a tab strip.
fn agent_tabs(
    agents: &[PromptAgent],
    selected: Option<PromptAgent>,
    theme: &Theme,
    view_id: EntityId,
    states: &States,
    scroll: &ClippedScrollStateHandle,
) -> Box<dyn Element> {
    let mut row = Flex::row().with_main_axis_size(MainAxisSize::Min).with_spacing(2.0);
    for agent in agents.iter().copied() {
        let is_selected = selected == Some(agent);
        let theme = *theme;
        let tab = Hoverable::new(state(states, &format!("usage_tab_{}", agent.command_key())), move |hover| {
            let hovered = hover.is_hovered();
            let icon = super::project_prompt::agent_image(
                agent,
                &theme,
                19.0,
                if is_selected || hovered { 1.0 } else { 0.62 },
            );
            let underline = ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(if is_selected { theme.main_text() } else { ColorU::transparent_black() })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.0)))
                    .finish(),
            )
            .with_width(22.0)
            .with_height(2.0)
            .finish();
            ConstrainedBox::new(
                Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(7.0)
                        .with_child(icon)
                        .with_child(underline)
                        .finish(),
                )
                .with_padding_top(8.0)
                .with_background_color(if hovered && !is_selected {
                    theme.surface_2()
                } else {
                    ColorU::transparent_black()
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish(),
            )
            .with_width(38.0)
            .with_height(38.0)
            .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, WorkspaceAction::SelectUsageAgent(agent)))
        .finish();
        row = row.with_child(tab);
    }
    // One row that scrolls sideways (wheel or trackpad) when more agents are
    // installed than fit across the popover.
    let clear = ColorU::transparent_black();
    let scroller = ClippedScrollable::horizontal_axis_locked(
        scroll.clone(),
        row.finish(),
        ScrollbarWidth::Custom(3.0),
        theme.surface_3().into(),
        theme.sub_text().into(),
        clear.into(),
    )
    .with_overlayed_scrollbar()
    .finish();
    Container::new(scroller)
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(4.0)
        .finish()
}

/// One account: name, "Default", and each plan window as a meter.
fn account_block(account: &AccountUsage, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    let now = now();
    let mut head = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(render_icon(AppIcon::User, theme.sub_text(), 13.0))
        .with_child(Shrinkable::new(1.0, strong(account.label.clone(), font, 12.5, theme.main_text())).finish());
    if let Some(plan) = account.usage.as_ref().and_then(|usage| usage.plan.clone()) {
        head = head.with_child(pill(plan, font, theme.sub_text(), ColorU::transparent_black(), theme.outline()));
    }
    head = head.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if account.default {
        head = head.with_child(pill(
            "Default",
            font,
            theme.accent,
            tinted(theme.accent, 28),
            tinted(theme.accent, 70),
        ));
    }
    let mut block = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(10.0)
        .with_child(head.finish());

    let windows: Vec<_> = account
        .usage
        .iter()
        .flat_map(|usage| usage.windows.iter())
        .filter(|window| window.resets_at.is_none_or(|at| at > now))
        .collect();
    if windows.is_empty() {
        block = block.with_child(text(
            match account.provider.as_str() {
                "claude" => "Limits appear after this account's next reply in a Claude Code session.",
                _ => "Limits appear after this account's next Codex reply.",
            },
            font,
            11.0,
            theme.disabled_text(),
        ));
    }
    for window in windows {
        let color = level_color(theme, window.used_percent);
        let resets = window
            .resets_at
            .map(|at| format!("Resets {}", crate::settings::usage::resets_in(at, now)))
            .unwrap_or_default();
        let percent_color = if window.used_percent >= WARN_PERCENT { color } else { theme.main_text() };
        block = block.with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(6.0)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(Shrinkable::new(1.0, text(window.label.clone(), font, 11.5, theme.sub_text())).finish())
                        .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                        .with_child(strong(format!("{:.0}%", window.used_percent), font, 11.5, percent_color))
                        .finish(),
                )
                .with_child(meter(theme, (window.used_percent / 100.0) as f32, color))
                .with_child(text(resets, font, 10.5, theme.disabled_text()))
                .finish(),
        );
    }
    Container::new(block.finish())
        .with_uniform_padding(12.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
        .finish()
}

/// What the popover says for an agent Dragent has no limit readings for.
fn no_limits_block(agent: PromptAgent, theme: &Theme, font: FamilyId) -> Box<dyn Element> {
    let name = agent.label();
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(10.0)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(8.0)
                    .with_child(render_icon(AppIcon::User, theme.sub_text(), 13.0))
                    .with_child(Shrinkable::new(1.0, strong("System login", font, 12.5, theme.main_text())).finish())
                    .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
                    .with_child(pill("Default", font, theme.accent, tinted(theme.accent, 28), tinted(theme.accent, 70)))
                    .finish(),
            )
            .with_child(text(
                format!("Signed in through {name}'s own CLI on this computer."),
                font,
                11.0,
                theme.sub_text(),
            ))
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(AppIcon::Info, theme.disabled_text(), 11.0))
                    .with_child(
                        Shrinkable::new(
                            1.0,
                            text(
                                format!("{name} doesn't report plan limits. Its token use is in Detailed usage."),
                                font,
                                10.5,
                                theme.disabled_text(),
                            ),
                        )
                        .finish(),
                    )
                    .finish(),
            )
            .finish(),
    )
    .with_uniform_padding(12.0)
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(9.0)))
    .finish()
}

#[allow(clippy::too_many_arguments)]
fn footer_button(
    label: &'static str,
    icon: AppIcon,
    primary: bool,
    action: WorkspaceAction,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
    mouse: MouseStateHandle,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(mouse, move |hover| {
        let hovered = hover.is_hovered();
        let (fg, bg, border) = if primary {
            (
                theme.accent,
                tinted(theme.accent, if hovered { 44 } else { 26 }),
                tinted(theme.accent, if hovered { 110 } else { 70 }),
            )
        } else {
            (
                if hovered { theme.main_text() } else { theme.sub_text() },
                if hovered { theme.surface_3() } else { theme.surface_2() },
                theme.outline(),
            )
        };
        Container::new(
            warpui::elements::Align::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(icon, fg, 12.0))
                    .with_child(strong(label, font, 11.5, fg))
                    .finish(),
            )
            .finish(),
        )
        .with_vertical_padding(8.0)
        .with_horizontal_padding(10.0)
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

/// The usage popover under the header's Usage button. Returns (menu,
/// click-catcher, positioning), like the account menu.
#[allow(clippy::too_many_arguments)]
pub fn usage_menu_overlay(
    agents: &[PromptAgent],
    selected: Option<PromptAgent>,
    accounts: &[AccountUsage],
    loading: bool,
    theme: &Theme,
    font: FamilyId,
    view_id: EntityId,
    states: &States,
    tabs_scroll: &ClippedScrollStateHandle,
) -> (Box<dyn Element>, Box<dyn Element>, OffsetPositioning) {
    let selected = selected.filter(|agent| agents.contains(agent)).or_else(|| initial_agent(agents, accounts));
    let mut body = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    if agents.is_empty() {
        body = body.with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(6.0)
                    .with_child(strong("No coding agents found", font, 13.0, theme.main_text()))
                    .with_child(text(
                        "Install Claude Code, Codex or another agent CLI and its accounts and limits show here.",
                        font,
                        11.5,
                        theme.sub_text(),
                    ))
                    .finish(),
            )
            .with_uniform_padding(16.0)
            .finish(),
        );
    } else {
        body = body.with_child(agent_tabs(agents, selected, theme, view_id, states, tabs_scroll)).with_child(divider(theme));
        if let Some(agent) = selected {
            let rows = accounts_for(accounts, agent);
            let count = rows.len().max(1);
            let mut title = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(super::project_prompt::agent_image(agent, theme, 22.0, 1.0))
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Flex::column()
                            .with_spacing(2.0)
                            .with_child(strong(agent.label(), font, 14.0, theme.main_text()))
                            .with_child(text(
                                format!("{count} account{} connected", if count == 1 { "" } else { "s" }),
                                font,
                                11.0,
                                theme.sub_text(),
                            ))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(Expanded::new(1.0, Empty::new().finish()).finish());
            if loading {
                title = title.with_child(text("Updating…", font, 10.5, theme.disabled_text()));
            }
            let mut section = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(10.0)
                .with_child(title.finish());
            if rows.is_empty() {
                section = section.with_child(no_limits_block(agent, theme, font));
            } else {
                for account in rows {
                    section = section.with_child(account_block(account, theme, font));
                }
            }
            body = body.with_child(
                ConstrainedBox::new(
                    Container::new(section.finish())
                        .with_padding_left(14.0)
                        .with_padding_right(14.0)
                        .with_padding_top(14.0)
                        .with_padding_bottom(14.0)
                        .finish(),
                )
                .with_max_height(460.0)
                .finish(),
            );
        }
    }

    body = body.with_child(divider(theme)).with_child(
        Container::new(
            Flex::row()
                .with_spacing(8.0)
                .with_child(
                    Expanded::new(
                        1.0,
                        footer_button(
                            "Detailed usage",
                            AppIcon::Activity,
                            true,
                            WorkspaceAction::OpenSettingsSection(crate::settings::SettingsSection::Usage),
                            theme,
                            font,
                            view_id,
                            state(states, "usage_footer_details"),
                        ),
                    )
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.0,
                        footer_button(
                            "Manage agent accounts",
                            AppIcon::Key,
                            false,
                            WorkspaceAction::OpenSettingsSection(crate::settings::SettingsSection::AgentAccounts),
                            theme,
                            font,
                            view_id,
                            state(states, "usage_footer_accounts"),
                        ),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(10.0)
        .finish(),
    );

    let menu = ConstrainedBox::new(
        Container::new(body.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
            .finish(),
    )
    .with_width(POPOVER_WIDTH)
    .finish();

    // Full-window transparent catcher: a click anywhere else dismisses.
    let catcher = EventHandler::new(
        Container::new(Empty::new().finish()).with_background_color(ColorU::new(0, 0, 0, 0)).finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(view_id, WorkspaceAction::CloseUsageMenu);
        DispatchEventResult::StopPropagation
    })
    .finish();

    let offset = OffsetPositioning::offset_from_save_position_element(
        USAGE_MENU_TAG,
        vec2f(0.0, 6.0),
        PositionedElementOffsetBounds::WindowByPosition,
        PositionedElementAnchor::BottomRight,
        ChildAnchor::TopRight,
    );
    (menu, catcher, offset)
}


/// The usage popover with sample accounts, for `ui_preview`.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn usage_popover_preview(font: FamilyId, theme: &Theme, view_id: EntityId, selected: PromptAgent) -> Box<dyn Element> {
    use dragent_host::agent_usage::{Usage, Window};
    let now = now();
    let account = |provider: &str, account: &str, label: &str, default: bool, plan: &str, windows: &[(&str, f64, i64)]| AccountUsage {
        provider: provider.into(),
        account: account.into(),
        label: label.into(),
        default,
        usage: Some(Usage {
            provider: provider.into(),
            account: account.into(),
            windows: windows
                .iter()
                .map(|(label, used, resets)| Window { label: (*label).into(), used_percent: *used, resets_at: Some(now + resets) })
                .collect(),
            plan: Some(plan.into()),
            updated_at: now,
        }),
    };
    let accounts = vec![
        account("claude", "system", "System login", true, "Max", &[("Current session", 34.0, 8_040), ("Weekly · all models", 82.0, 266_400)]),
        account("claude", "work", "Work", false, "Pro", &[("Current session", 96.0, 1_500), ("Weekly · all models", 41.0, 400_000)]),
        account("codex", "system", "System login", true, "Plus", &[("5 hours", 12.0, 12_000), ("Weekly", 58.0, 300_000)]),
    ];
    // More agents than fit across, so the row overflows and scrolls.
    let agents: Vec<PromptAgent> = PromptAgent::ALL.into_iter().filter(|a| *a != PromptAgent::Dragent).take(14).collect();
    let states = States::default();
    let (menu, _, _) = usage_menu_overlay(
        &agents,
        Some(selected),
        &accounts,
        false,
        theme,
        font,
        view_id,
        &states,
        &ClippedScrollStateHandle::new(),
    );
    Container::new(
        Flex::row()
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Center)
            .with_child(Flex::column().with_child(menu).finish())
            .finish(),
    )
    .with_padding_top(40.0)
    .with_background_color(theme.solid_background())
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_host::agent_usage::{Usage, Window};

    #[test]
    fn the_chip_shows_the_default_accounts_highest_use() {
        let account = |default: bool, used: f64| AccountUsage {
            provider: "codex".into(),
            account: "system".into(),
            label: "System login".into(),
            default,
            usage: Some(Usage {
                provider: "codex".into(),
                account: "system".into(),
                windows: vec![Window { label: "Weekly".into(), used_percent: used, resets_at: None }],
                plan: None,
                updated_at: 0,
            }),
        };
        assert_eq!(headline(&[account(true, 30.0), account(false, 99.0), account(true, 64.0)]), Some(64.0));
        assert_eq!(headline(&[account(false, 50.0)]), None);
    }

    fn reading(provider: &str, used: f64) -> AccountUsage {
        AccountUsage {
            provider: provider.into(),
            account: "system".into(),
            label: "System login".into(),
            default: true,
            usage: Some(Usage {
                provider: provider.into(),
                account: "system".into(),
                windows: vec![Window { label: "5 hours".into(), used_percent: used, resets_at: None }],
                plan: None,
                updated_at: 0,
            }),
        }
    }

    #[test]
    fn the_popover_lists_every_installed_agent_in_app_order() {
        let installed: std::collections::HashSet<PromptAgent> =
            [PromptAgent::OpenCode, PromptAgent::Claude, PromptAgent::Dragent, PromptAgent::Gemini].into_iter().collect();
        // Codex isn't marked installed yet but has a reading, so it shows too.
        let agents = popover_agents(&installed, &[reading("codex", 10.0)]);
        assert_eq!(agents, vec![PromptAgent::Claude, PromptAgent::Codex, PromptAgent::Gemini, PromptAgent::OpenCode]);
    }

    #[test]
    fn the_popover_opens_on_the_agent_nearest_a_limit() {
        let agents = [PromptAgent::Claude, PromptAgent::Codex, PromptAgent::OpenCode];
        assert_eq!(initial_agent(&agents, &[reading("claude", 20.0), reading("codex", 91.0)]), Some(PromptAgent::Codex));
        assert_eq!(initial_agent(&agents, &[reading("codex", 20.0)]), Some(PromptAgent::Codex));
        assert_eq!(initial_agent(&agents, &[]), Some(PromptAgent::Claude));
        assert_eq!(initial_agent(&[], &[]), None);
    }
}
