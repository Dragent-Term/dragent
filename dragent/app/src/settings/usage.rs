//! Settings → Usage: what the coding agents on this computer used, read from
//! their own records (`dragent_host::token_usage`), plus each Claude Code and
//! Codex account's plan limits and which account new sessions use
//! (`dragent_host::agent_usage`).

use dragent_host::agent_usage::AccountUsage;
use dragent_host::token_usage::{self, dollars, short_count, Row, Summary, Totals};
use chrono::Datelike as _;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::ViewContext;
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::widgets::{page_header, segmented_control, setting_group, SegmentOption};
use super::{SettingsAction, SettingsView};
use crate::theme::{text_size, Theme};
use crate::workspace::PromptAgent;

#[derive(Clone, Debug, PartialEq)]
pub enum UsageAction {
    /// Show the last N days (1 = today).
    Range(i32),
    /// Chart and rank by estimated cost instead of tokens.
    ShowCost(bool),
    /// Count work done in temporary folders (tests, scratch, background jobs).
    IncludeTemporary(bool),
    Refresh,
    /// Make an account the one new sessions of its agent use.
    UseAccount { provider: String, account: String },
}

pub(crate) struct UsageState {
    pub days: i32,
    pub show_cost: bool,
    pub include_temporary: bool,
    pub rows: Vec<Row>,
    pub accounts: Vec<AccountUsage>,
    pub loading: bool,
    pub loaded: bool,
}

impl Default for UsageState {
    fn default() -> Self {
        Self { days: 7, show_cost: false, include_temporary: true, rows: Vec::new(), accounts: Vec::new(), loading: false, loaded: false }
    }
}

impl SettingsView {
    pub(crate) fn handle_usage(&mut self, action: &UsageAction, ctx: &mut ViewContext<Self>) {
        match action {
            UsageAction::Range(days) => self.usage.days = *days,
            UsageAction::ShowCost(cost) => self.usage.show_cost = *cost,
            UsageAction::IncludeTemporary(include) => self.usage.include_temporary = *include,
            UsageAction::Refresh => self.refresh_usage(ctx),
            UsageAction::UseAccount { provider, account } => {
                if let Err(error) = dragent_host::agent_usage::set_default(provider, account) {
                    log::warn!("[settings] switching {provider} account: {error}");
                }
                self.refresh_usage(ctx);
            }
        }
        ctx.notify();
    }

    /// Read every agent's records and every account's limits, off the UI thread.
    pub(crate) fn refresh_usage(&mut self, ctx: &mut ViewContext<Self>) {
        if self.usage.loading {
            return;
        }
        self.usage.loading = true;
        ctx.spawn(
            async { (token_usage::collect(), dragent_host::agent_usage::accounts()) },
            |view, (rows, accounts), ctx| {
                view.usage.rows = rows;
                view.usage.accounts = accounts;
                view.usage.loading = false;
                view.usage.loaded = true;
                ctx.notify();
            },
        );
        ctx.notify();
    }
}

// ── Agents ───────────────────────────────────────────────────────────────────

fn agent_for(key: &str) -> Option<PromptAgent> {
    PromptAgent::ALL.into_iter().find(|agent| agent.cli() == Some(key))
}

fn agent_name(key: &str) -> String {
    match key {
        "mimo" => "MiMo Code".into(),
        "copilot" => "GitHub Copilot".into(),
        "qwen" => "Qwen Code".into(),
        _ => agent_for(key).map_or_else(|| key.to_string(), |agent| agent.label().to_string()),
    }
}

fn agent_icon(key: &str, theme: &Theme, size: f32) -> Box<dyn Element> {
    match agent_for(key) {
        Some(agent) => crate::workspace::prompt_agent_image(agent, theme, size, 1.0),
        None => crate::icons::render_icon(crate::icons::AppIcon::Agent, theme.sub_text(), size),
    }
}

/// A steady colour per agent, in the order agents first appear.
fn agent_color(theme: &Theme, index: usize) -> ColorU {
    let palette = [
        theme.accent,
        ColorU::new(167, 139, 250, 255),
        theme.ui_green(),
        ColorU::new(251, 146, 60, 255),
        theme.ui_yellow(),
        theme.ansi_red(),
        theme.ansi_blue(),
    ];
    palette[index % palette.len()]
}

fn measure(totals: &Totals, cost: bool) -> f64 {
    if cost { totals.cost } else { totals.tokens() as f64 }
}

fn day_label(day: i32) -> String {
    chrono::NaiveDate::from_num_days_from_ce_opt(day)
        .map(|date| date.format("%b %-d").to_string())
        .unwrap_or_default()
}

// ── Pieces ───────────────────────────────────────────────────────────────────

fn label(text: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(text.into(), font, text_size(size)).with_color(color).finish()
}

fn clipped(text: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(text.into(), font, text_size(size))
        .with_color(color)
        .soft_wrap(false)
        .with_clip(ClipConfig::ellipsis())
        .finish()
}

/// One headline number with what it is and a line of detail.
fn tile(theme: &Theme, font: FamilyId, title: &str, value: String, detail: String) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(4.0)
            .with_child(label(title, font, 11.5, theme.sub_text()))
            .with_child(label(value, font, 22.0, theme.main_text()))
            .with_child(clipped(detail, font, 11.0, theme.disabled_text()))
            .finish(),
    )
    .with_uniform_padding(14.0)
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .finish()
}

/// A thin proportional bar.
fn bar(theme: &Theme, fraction: f32, color: ColorU, height: f32) -> Box<dyn Element> {
    let fraction = fraction.clamp(0.0, 1.0);
    ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_child(
                    Expanded::new(
                        fraction.max(0.0001),
                        Container::new(Empty::new().finish())
                            .with_background_color(color)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(height / 2.0)))
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(Expanded::new((1.0 - fraction).max(0.0001), Empty::new().finish()).finish())
                .finish(),
        )
        .with_background_color(theme.surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(height / 2.0)))
        .finish(),
    )
    .with_height(height)
    .finish()
}

/// "Est. $12.34", or why there is no estimate.
fn cost_text(totals: &Totals) -> String {
    if totals.cost > 0.0 {
        dollars(totals.cost)
    } else if totals.unpriced_tokens > 0 {
        "No price".into()
    } else {
        "$0.00".into()
    }
}

/// Daily columns, stacked by agent, with a legend.
fn chart(theme: &Theme, font: FamilyId, summary: &Summary, agents: &[String], cost: bool) -> Box<dyn Element> {
    const HEIGHT: f32 = 140.0;
    let peak = summary
        .days
        .iter()
        .map(|(_, per_agent)| per_agent.values().map(|totals| measure(totals, cost)).sum::<f64>())
        .fold(0.0, f64::max);
    let count = summary.days.len();
    let mut columns = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(if count > 40 { 1.0 } else { 3.0 });
    for (day, per_agent) in &summary.days {
        let mut stack = Flex::column().with_main_axis_size(MainAxisSize::Min);
        // Draw the largest agent at the bottom.
        for (index, agent) in agents.iter().enumerate().rev() {
            let Some(totals) = per_agent.get(agent) else { continue };
            let value = measure(totals, cost);
            if value <= 0.0 || peak <= 0.0 {
                continue;
            }
            let height = (value / peak * HEIGHT as f64).max(1.0) as f32;
            stack = stack.with_child(
                ConstrainedBox::new(Container::new(Empty::new().finish()).with_background_color(agent_color(theme, index)).finish())
                    .with_height(height)
                    .finish(),
            );
        }
        let _ = day;
        columns = columns.with_child(
            Expanded::new(
                1.0,
                ConstrainedBox::new(
                    Container::new(
                        Flex::column()
                            .with_main_axis_alignment(MainAxisAlignment::End)
                            .with_main_axis_size(MainAxisSize::Max)
                            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                            .with_child(stack.finish())
                            .finish(),
                    )
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                    .finish(),
                )
                .with_height(HEIGHT)
                .finish(),
            )
            .finish(),
        );
    }
    let axis = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_child(label(summary.days.first().map(|(day, _)| day_label(*day)).unwrap_or_default(), font, 10.5, theme.disabled_text()))
        .with_child(label(if count == 1 { String::new() } else { "Today".to_string() }, font, 10.5, theme.disabled_text()))
        .finish();
    let mut legend = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(14.0);
    for (index, agent) in agents.iter().enumerate() {
        legend = legend.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(
                    ConstrainedBox::new(
                        Container::new(Empty::new().finish())
                            .with_background_color(agent_color(theme, index))
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
                            .finish(),
                    )
                    .with_width(8.0)
                    .with_height(8.0)
                    .finish(),
                )
                .with_child(agent_icon(agent, theme, 12.0))
                .with_child(label(agent_name(agent), font, 11.5, theme.sub_text()))
                .finish(),
        );
    }
    let peak_text = if cost { dollars(peak) } else { short_count(peak as u64) };
    super::widgets::settings_card(
        theme,
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.0)
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_child(label(if cost { "Estimated cost per day" } else { "Tokens per day" }, font, 12.0, theme.main_text()))
                    .with_child(label(format!("Busiest day: {peak_text}"), font, 11.0, theme.disabled_text()))
                    .finish(),
            )
            .with_child(columns.finish())
            .with_child(axis)
            .with_child(legend.finish())
            .finish(),
    )
}

/// One ranked row: icon, name, detail, share bar, value, cost.
#[allow(clippy::too_many_arguments)]
fn ranked_row(
    theme: &Theme,
    font: FamilyId,
    icon: Box<dyn Element>,
    name: String,
    detail: String,
    totals: &Totals,
    fraction: f32,
    color: ColorU,
) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(ConstrainedBox::new(Align::new(icon).finish()).with_width(18.0).finish())
        .with_child(
            Expanded::new(
                3.0,
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(2.0)
                    .with_child(clipped(name, font, 12.5, theme.main_text()))
                    .with_child(clipped(detail, font, 11.0, theme.disabled_text()))
                    .finish(),
            )
            .finish(),
        )
        .with_child(Expanded::new(2.0, bar(theme, fraction, color, 6.0)).finish())
        .with_child(
            ConstrainedBox::new(Align::new(label(short_count(totals.tokens()), font, 12.5, theme.main_text())).right().finish())
                .with_width(70.0)
                .finish(),
        )
        .with_child(
            ConstrainedBox::new(Align::new(label(cost_text(totals), font, 12.5, theme.sub_text())).right().finish())
                .with_width(80.0)
                .finish(),
        )
        .finish()
}

fn breakdown(totals: &Totals) -> String {
    format!(
        "{} in · {} out · {} cached · {} replies",
        short_count(totals.input),
        short_count(totals.output),
        short_count(totals.cache_read + totals.cache_write),
        short_count(totals.replies),
    )
}

/// Plan-limit card for one account.
fn limit_card(view: &SettingsView, theme: &Theme, account: &AccountUsage) -> Box<dyn Element> {
    let font = view.ui_font;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let mut title = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(agent_icon(&account.provider, theme, 14.0))
        .with_child(
            Shrinkable::new(1.0, clipped(format!("{} · {}", agent_name(&account.provider), account.label), font, 12.5, theme.main_text()))
                .finish(),
        );
    if let Some(plan) = account.usage.as_ref().and_then(|usage| usage.plan.clone()) {
        title = title.with_child(label(plan, font, 11.0, theme.disabled_text()));
    }
    title = title.with_child(Expanded::new(1.0, Empty::new().finish()).finish());
    if account.default {
        title = title.with_child(super::widgets::status_badge(theme, font, "Used for new sessions", true));
    } else {
        let action = SettingsAction::Usage(UsageAction::UseAccount {
            provider: account.provider.clone(),
            account: account.account.clone(),
        });
        let theme_owned = *theme;
        title = title.with_child(
            Hoverable::new(view.hover(&format!("usage-use-{}-{}", account.provider, account.account)), move |state| {
                Container::new(label("Use for new sessions", font, 11.5, if state.is_hovered() { theme_owned.main_text() } else { theme_owned.accent }))
                    .with_horizontal_padding(8.0)
                    .with_vertical_padding(3.0)
                    .with_border(Border::all(1.0).with_border_color(theme_owned.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click({
                let id = view.view_id;
                move |ctx, _, _| ctx.dispatch_typed_action_for(id, action.clone())
            })
            .finish(),
        );
    }
    let mut card = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(title.finish());
    let windows: Vec<_> = account
        .usage
        .iter()
        .flat_map(|usage| usage.windows.iter())
        .filter(|window| window.resets_at.is_none_or(|at| at > now))
        .collect();
    if windows.is_empty() {
        card = card.with_child(label(
            match account.provider.as_str() {
                "claude" => "Limits show after this account's next reply in a Claude Code session started in Dragent. API-key sign-ins have no plan limits.",
                _ => "Limits show after this account's next Codex reply.",
            },
            font,
            11.0,
            theme.disabled_text(),
        ));
    }
    for window in windows {
        let color = if window.used_percent >= 95.0 {
            theme.ui_red()
        } else if window.used_percent >= 80.0 {
            theme.ui_yellow()
        } else {
            theme.accent
        };
        let resets = window
            .resets_at
            .map(|at| format!("resets {}", resets_in(at, now)))
            .unwrap_or_default();
        card = card.with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(10.0)
                .with_child(ConstrainedBox::new(label(window.label.clone(), font, 11.5, theme.sub_text())).with_width(90.0).finish())
                .with_child(Expanded::new(1.0, bar(theme, (window.used_percent / 100.0) as f32, color, 6.0)).finish())
                .with_child(ConstrainedBox::new(Align::new(label(format!("{:.0}% used", window.used_percent), font, 11.5, theme.main_text())).right().finish()).with_width(70.0).finish())
                .with_child(ConstrainedBox::new(Align::new(label(resets, font, 11.0, theme.disabled_text())).right().finish()).with_width(110.0).finish())
                .finish(),
        );
    }
    super::widgets::settings_card(theme, card.finish())
}

/// Whether a folder is a temporary one (test runs, scratch work, background
/// jobs) rather than a project.
fn is_temporary(path: &str) -> bool {
    let temp = std::env::temp_dir();
    let temp = temp.to_string_lossy();
    ["/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/"]
        .iter()
        .any(|prefix| path.starts_with(prefix) || path == prefix.trim_end_matches('/'))
        || (!temp.is_empty() && path.starts_with(temp.as_ref()))
}

/// Projects by the folder each reply ran in: `(name, full path, totals)`,
/// largest first, with every temporary folder summed into one row.
fn group_projects(projects: &[(String, Totals)]) -> Vec<(String, String, Totals)> {
    let mut rows = Vec::new();
    let mut temporary: Option<Totals> = None;
    let mut temporary_count = 0;
    for (path, totals) in projects {
        if is_temporary(path) {
            temporary_count += 1;
            let sum = temporary.get_or_insert_with(Totals::default);
            sum.input += totals.input;
            sum.output += totals.output;
            sum.cache_read += totals.cache_read;
            sum.cache_write += totals.cache_write;
            sum.replies += totals.replies;
            sum.cost += totals.cost;
            sum.unpriced_tokens += totals.unpriced_tokens;
            continue;
        }
        let name = std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        rows.push((name, path.clone(), totals.clone()));
    }
    if let Some(totals) = temporary {
        rows.push((
            "Temporary folders".into(),
            format!("{temporary_count} scratch, test and background-job folder{}", if temporary_count == 1 { "" } else { "s" }),
            totals,
        ));
        rows.sort_by(|a, b| b.2.tokens().cmp(&a.2.tokens()));
    }
    rows
}

/// "in 2h 10m", "in 3d 4h", "in 45m", "now".
pub fn resets_in(resets_at: i64, now: i64) -> String {
    let seconds = resets_at - now;
    if seconds <= 60 {
        return "now".into();
    }
    let (days, hours, minutes) = (seconds / 86_400, seconds % 86_400 / 3_600, seconds % 3_600 / 60);
    if days > 0 {
        format!("in {days}d {hours}h")
    } else if hours > 0 {
        format!("in {hours}h {minutes}m")
    } else {
        format!("in {minutes}m")
    }
}

// ── Page ─────────────────────────────────────────────────────────────────────

pub fn render(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let state = &view.usage;
    let font = view.ui_font;
    let id = view.view_id;
    let mut page = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(14.0)
        .with_child(page_header(
            theme,
            font,
            "Usage",
            "Tokens your coding agents used on this computer, what the same work would cost at API prices, and how close each account is to its plan limits. Read from each agent's own records; nothing leaves this computer.",
        ));

    let ranges = [(1, "Today"), (7, "7 days"), (30, "30 days"), (90, "90 days")];
    let range_control = segmented_control(
        theme,
        font,
        id,
        ranges
            .iter()
            .map(|(days, name)| SegmentOption {
                label: name.to_string(),
                selected: state.days == *days,
                hover: view.hover(&format!("usage-range-{days}")),
                action: SettingsAction::Usage(UsageAction::Range(*days)),
            })
            .collect(),
    );
    let metric_control = segmented_control(
        theme,
        font,
        id,
        [("Tokens", false), ("Cost", true)]
            .iter()
            .map(|(name, cost)| SegmentOption {
                label: name.to_string(),
                selected: state.show_cost == *cost,
                hover: view.hover(&format!("usage-metric-{name}")),
                action: SettingsAction::Usage(UsageAction::ShowCost(*cost)),
            })
            .collect(),
    );
    let temporary_control = segmented_control(
        theme,
        font,
        id,
        [("Include temporary folders", true), ("Hide them", false)]
            .iter()
            .map(|(name, include)| SegmentOption {
                label: name.to_string(),
                selected: state.include_temporary == *include,
                hover: view.hover(&format!("usage-temporary-{include}")),
                action: SettingsAction::Usage(UsageAction::IncludeTemporary(*include)),
            })
            .collect(),
    );
    let refresh = super::widgets::link_button(
        theme,
        font,
        id,
        view.hover("usage-refresh"),
        if state.loading { "Reading…" } else { "Refresh" },
        SettingsAction::Usage(UsageAction::Refresh),
    );
    page = page.with_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0)
            .with_child(range_control)
            .with_child(metric_control)
            .with_child(temporary_control)
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .with_child(refresh)
            .finish(),
    );

    if !state.loaded {
        return page
            .with_child(label(
                "Reading your agents' records… The first time can take a little while; after that it's quick.",
                font,
                12.0,
                theme.sub_text(),
            ))
            .finish();
    }

    // Hiding temporary folders leaves out tests, scratch work and
    // background jobs everywhere on the page, not only under By project.
    let summary = if state.include_temporary {
        token_usage::summarise(&state.rows, state.days)
    } else {
        let kept: Vec<Row> = state
            .rows
            .iter()
            .filter(|row| !row.project.as_deref().is_some_and(is_temporary))
            .cloned()
            .collect();
        token_usage::summarise(&kept, state.days)
    };
    let total = &summary.total;
    let cache_share = if total.tokens() > 0 {
        total.cache_read as f64 / total.tokens() as f64 * 100.0
    } else {
        0.0
    };
    let unpriced = if total.unpriced_tokens > 0 {
        format!("+{} tokens unpriced", short_count(total.unpriced_tokens))
    } else {
        "At API list prices. Subscriptions aren't billed per token.".to_string()
    };
    page = page.with_child(
        Flex::row()
            .with_spacing(10.0)
            .with_child(Expanded::new(1.0, tile(theme, font, "Tokens", short_count(total.tokens()), format!("{} input · {} output", short_count(total.input), short_count(total.output)))).finish())
            .with_child(Expanded::new(1.0, tile(theme, font, "Estimated API cost", dollars(total.cost), unpriced)).finish())
            .with_child(Expanded::new(1.0, tile(theme, font, "Read from cache", format!("{cache_share:.0}%"), if total.cache_savings > 0.0 { format!("Caching saved about {}", dollars(total.cache_savings)) } else { "of all tokens".into() })).finish())
            .with_child(Expanded::new(1.0, tile(theme, font, "Replies", short_count(total.replies), format!("from {} agent{}", summary.agents.len(), if summary.agents.len() == 1 { "" } else { "s" }))).finish())
            .finish(),
    );

    if summary.agents.is_empty() {
        page = page.with_child(super::widgets::empty_state(
            theme,
            font,
            "No agent activity in this period",
            "Claude Code, Codex, OpenCode, MiMo Code, GitHub Copilot, Gemini CLI and Qwen Code are counted once they reply.",
        ));
    } else {
        let agents: Vec<String> = summary.agents.iter().map(|(agent, _)| agent.clone()).collect();
        page = page.with_child(chart(theme, font, &summary, &agents, state.show_cost));

        let cost = state.show_cost;
        let largest = |values: &mut dyn Iterator<Item = f64>| values.fold(0.0, f64::max).max(f64::MIN_POSITIVE);
        let top_agent = largest(&mut summary.agents.iter().map(|(_, totals)| measure(totals, cost)));
        let rows: Vec<_> = summary
            .agents
            .iter()
            .enumerate()
            .map(|(index, (agent, totals))| {
                ranked_row(theme, font, agent_icon(agent, theme, 16.0), agent_name(agent), breakdown(totals), totals, (measure(totals, cost) / top_agent) as f32, agent_color(theme, index))
            })
            .collect();
        page = page.with_child(setting_group(theme, font, "By agent", rows));

        let top_model = largest(&mut summary.models.iter().map(|(_, _, totals)| measure(totals, cost)));
        let rows: Vec<_> = summary
            .models
            .iter()
            .take(10)
            .map(|(agent, model, totals)| {
                let index = agents.iter().position(|a| a == agent).unwrap_or(0);
                ranked_row(theme, font, agent_icon(agent, theme, 14.0), model.clone(), format!("{} · {}", agent_name(agent), breakdown(totals)), totals, (measure(totals, cost) / top_model) as f32, agent_color(theme, index))
            })
            .collect();
        page = page.with_child(setting_group(theme, font, "By model", rows));

        let projects = group_projects(&summary.projects);
        if !projects.is_empty() {
            let top_project = largest(&mut projects.iter().map(|(_, _, totals)| measure(totals, cost)));
            let rows: Vec<_> = projects
                .iter()
                .take(8)
                .map(|(name, detail, totals)| {
                    ranked_row(theme, font, crate::icons::render_icon(crate::icons::AppIcon::Folder, theme.sub_text(), 14.0), name.clone(), detail.clone(), totals, (measure(totals, cost) / top_project) as f32, theme.accent)
                })
                .collect();
            page = page.with_child(setting_group(theme, font, "By project", rows));
        }
    }

    let limits: Vec<_> = state.accounts.iter().map(|account| limit_card(view, theme, account)).collect();
    if !limits.is_empty() {
        page = page
            .with_child(label("Plan limits", font, 13.0, theme.main_text()))
            .with_child(label(
                "How much of each Claude Code and Codex subscription is used, and which account new sessions start with.",
                font,
                11.5,
                theme.sub_text(),
            ));
        for card in limits {
            page = page.with_child(card);
        }
    }
    page.finish()
}

/// Sample figures for the UI preview: invented, never the person's.
pub(crate) fn preview_state() -> UsageState {
    use dragent_host::agent_usage::{Usage, Window};
    let today = token_usage::today();
    let mut rows = Vec::new();
    for back in 0..7 {
        let day = today - back;
        let scale = 1.0 + (back as f64 * 0.7).sin().abs();
        let row = |agent: &str, model: &str, project: &str, output: f64, cache: f64| Row {
            agent: agent.into(),
            model: model.into(),
            day,
            project: Some(project.into()),
            input: (40_000.0 * scale) as u64,
            output: (output * scale) as u64,
            cache_read: (cache * scale) as u64,
            cache_write_5m: (cache * scale / 30.0) as u64,
            replies: (120.0 * scale) as u64,
            ..Default::default()
        };
        rows.push(row("claude", "claude-opus-5-5", "/demo/code/acme-app", 900_000.0, 40_000_000.0));
        rows.push(row("claude", "claude-sonnet-5-5", "/demo/code/design-system", 300_000.0, 9_000_000.0));
        rows.push(row("codex", "gpt-codex", "/demo/code/acme-app", 400_000.0, 15_000_000.0));
        rows.push(row("opencode", "claude-haiku-5-5", "/demo/code/marketing-site", 200_000.0, 6_000_000.0));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let account = |provider: &str, label: &str, default: bool, plan: &str, windows: Vec<Window>| AccountUsage {
        provider: provider.into(),
        account: label.to_lowercase(),
        label: label.into(),
        default,
        usage: Some(Usage { provider: provider.into(), account: label.to_lowercase(), windows, plan: Some(plan.into()), updated_at: now }),
    };
    let window = |label: &str, used: f64, hours: i64| Window { label: label.into(), used_percent: used, resets_at: Some(now + hours * 3600) };
    UsageState {
        days: 7,
        show_cost: false,
        include_temporary: true,
        rows,
        accounts: vec![
            account("claude", "System login", true, "max", vec![window("5-hour", 42.0, 3), window("Weekly", 83.0, 76)]),
            account("codex", "Personal", true, "plus", vec![window("5-hour", 12.0, 2), window("Weekly", 64.0, 100)]),
            account("codex", "Work", false, "pro", vec![window("Weekly", 21.0, 30)]),
        ],
        loading: false,
        loaded: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_times_read_naturally() {
        assert_eq!(resets_in(1_000, 1_000), "now");
        assert_eq!(resets_in(1_000 + 45 * 60, 1_000), "in 45m");
        assert_eq!(resets_in(1_000 + 2 * 3600 + 10 * 60, 1_000), "in 2h 10m");
        assert_eq!(resets_in(1_000 + 3 * 86_400 + 4 * 3600, 1_000), "in 3d 4h");
    }

    #[test]
    fn temporary_folders_are_one_row() {
        let totals = |output: u64| Totals { output, ..Default::default() };
        let projects = vec![
            ("/Users/me/dev/app".to_string(), totals(10)),
            ("/private/var/folders/b6/x/T/dragent-driver-live".to_string(), totals(7)),
            ("/private/tmp/acp-probe".to_string(), totals(5)),
        ];
        let rows = group_projects(&projects);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "Temporary folders");
        assert_eq!(rows[0].2.output, 12);
        assert_eq!(rows[1].0, "app");
    }

    #[test]
    fn costs_say_when_there_is_no_price() {
        let priced = Totals { cost: 12.5, ..Default::default() };
        let unpriced = Totals { unpriced_tokens: 10, ..Default::default() };
        assert_eq!(cost_text(&priced), "$12.50");
        assert_eq!(cost_text(&unpriced), "No price");
        assert_eq!(agent_name("mimo"), "MiMo Code");
        assert_eq!(day_label(chrono::NaiveDate::from_ymd_opt(2026, 11, 15).unwrap().num_days_from_ce()), "Nov 15");
    }
}
