//! Draws laid-out chat lines on the desktop: your message as a soft bubble on
//! the right, replies in full, steps hanging from a thin guide line on the
//! left (a vertical stroke with a rounded elbow into each step), a tally over
//! a group of steps that expands, the turn timer, and the changed-files card.
//! Work still in progress shimmers.
//!
//! Expand state lives with the caller (`open` keys); clicks dispatch
//! [`TerminalAction::ToggleChatLine`] with the line's key.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use warpui::color::ColorU;
use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, Hoverable, MainAxisAlignment,
    MainAxisSize, ParentElement, Shrinkable, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::layout::{clock_label, dominant_kind, run_summary, running_time, ChangedFile, Clock, Line, Step, StepKind, YouNote};
use crate::icons::{render_icon, AppIcon};
use crate::terminal::render::{markdown::render_markdown_agent_document, BlockRenderContext};
use crate::terminal::view::actions::TerminalAction;
use crate::theme::{text_size, Theme};

/// The newest steps a running group keeps listed while folded.
const ACTIVE_TAIL: usize = 3;
/// Widest your message bubble gets.
const BUBBLE_MAX_WIDTH: f32 = 560.0;

/// Everything the chat lines need from the screen around them.
pub struct ChatView<'a> {
    pub ctx: &'a BlockRenderContext<'a>,
    pub theme: Theme,
    pub ui: FamilyId,
    pub mono: FamilyId,
    /// Unix ms, for running timers.
    pub now_ms: i64,
    /// Keys of the groups and steps the person opened.
    pub open: &'a HashSet<String>,
    /// What goes under one of your messages (by its id): restore, attachments.
    pub you_extras: Option<&'a dyn Fn(&str) -> Option<Box<dyn Element>>>,
}

/// `amount` of the text colour over the background, opaque.
fn ink(theme: &Theme, amount: f32) -> ColorU {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    let (bg, fg) = (theme.background, theme.foreground);
    ColorU::new(mix(bg.r, fg.r), mix(bg.g, fg.g), mix(bg.b, fg.b), 255)
}

fn text(content: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(content.into(), font, text_size(size)).with_color(color).finish()
}

fn line_text(content: impl Into<String>, font: FamilyId, size: f32, color: ColorU) -> Box<dyn Element> {
    Text::new(content.into(), font, text_size(size)).with_color(color).soft_wrap(false).finish()
}

fn shimmer(view: &ChatView<'_>, key: &str, content: String, size: f32) -> Box<dyn Element> {
    ShimmeringTextElement::new(
        content,
        view.ui,
        text_size(size),
        ink(&view.theme, 0.5),
        view.theme.main_text(),
        ShimmerConfig { period: Duration::from_millis(1600), shimmer_radius: 5, padding: 6 },
        view.ctx.shimmer_state(key),
    )
    .finish()
}

fn numeric_id(key: &str) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish() as usize
}

fn toggle(key: String) -> TerminalAction {
    TerminalAction::ToggleChatLine { key }
}

/// Draw `lines`; `card` draws what only the caller knows (approvals, questions…).
pub fn render<C>(lines: &[Line<C>], view: &ChatView<'_>, card: &dyn Fn(&C) -> Box<dyn Element>) -> Box<dyn Element> {
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for line in lines {
        let (body, top, bottom) = match line {
            Line::You { id, text, note, turn_start, .. } => (you(id, text, note.as_ref(), view), if *turn_start { 16.0 } else { 0.0 }, 4.0),
            Line::Clock { key, clock } => (timer(key, clock, view), 0.0, 4.0),
            Line::Reply { key, text, streaming, last_reply, at, after_work, .. } => {
                (reply(key, text, *streaming, *last_reply, *at, view), if *after_work { 4.0 } else { 12.0 }, 4.0)
            }
            Line::Run { key, steps, active } => (run(key, steps, *active, view), 0.0, 4.0),
            Line::Step { step, .. } => (
                if step.kind == StepKind::Think { step_line(step, view) } else { guide(true, step_line(step, view), view) },
                0.0,
                4.0,
            ),
            Line::Changes { files, .. } => (changes(files, view), 4.0, 8.0),
            Line::Card { card: c, .. } => (card(c), 6.0, 6.0),
        };
        column = column.with_child(Container::new(body).with_padding_top(top).with_padding_bottom(bottom).finish());
    }
    column.finish()
}

/// Your message: a soft bubble on the right.
fn you(id: &str, content: &str, note: Option<&YouNote>, view: &ChatView<'_>) -> Box<dyn Element> {
    let theme = &view.theme;
    let settling = matches!(note, Some(YouNote::Sending | YouNote::Queued));
    let bubble = Container::new(text(content, view.ui, 14.0, if settling { theme.sub_text() } else { theme.main_text() }))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .with_background_color(ink(theme, 0.1))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .finish();
    let bubble = ConstrainedBox::new(bubble).with_max_width(BUBBLE_MAX_WIDTH).finish();
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::End)
        .with_spacing(4.0)
        .with_child(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_child(Shrinkable::new(1.0, bubble).finish())
                .finish(),
        );
    if let Some(note) = note {
        let (words, color) = match note {
            YouNote::Sending => ("Sending…".to_string(), theme.sub_text()),
            YouNote::Queued => ("Queued".to_string(), theme.sub_text()),
            YouNote::Failed(reason) => (format!("Not delivered · {reason}"), theme.ui_red()),
        };
        column = column.with_child(text(words, view.ui, 11.5, color));
    }
    if let Some(extras) = view.you_extras.and_then(|f| f(id)) {
        column = column.with_child(extras);
    }
    column.finish()
}

/// The turn timer: sweeping while the turn runs, then how long it took.
fn timer(key: &str, clock: &Clock, view: &ChatView<'_>) -> Box<dyn Element> {
    let Some(label) = clock_label(clock, view.now_ms) else {
        return Empty::new().finish();
    };
    let body = if clock.running && !clock.waiting {
        shimmer(view, key, label, 13.0)
    } else {
        line_text(label, view.ui, 13.0, ink(&view.theme, 0.5))
    };
    Container::new(body).with_padding_top(4.0).with_padding_bottom(4.0).finish()
}

/// The agent's reply in full; Copy and its time under the turn's last one.
fn reply(key: &str, content: &str, streaming: bool, last_reply: bool, at: Option<i64>, view: &ChatView<'_>) -> Box<dyn Element> {
    let theme = view.theme;
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(6.0).with_child(
        render_markdown_agent_document(content, numeric_id(key), view.ctx, theme, view.ui, view.mono, !streaming),
    );
    if last_reply && !streaming {
        let copy_key = format!("chat-copy:{key}");
        let copied = view.ctx.is_copied(&copy_key);
        let copy_text = content.to_string();
        let action_key = copy_key.clone();
        let copy = Hoverable::new(view.ctx.mouse_state(&copy_key), move |state| {
            let color = if copied { theme.main_text() } else if state.is_hovered() { theme.sub_text() } else { ink(&theme, 0.4) };
            Container::new(render_icon(if copied { AppIcon::Check } else { AppIcon::Copy }, color, 13.0))
                .with_padding_left(3.0)
                .with_padding_right(3.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event, _, _| {
            event.dispatch_typed_action(TerminalAction::CopyWithConfirmation { key: action_key.clone(), text: copy_text.clone() })
        });
        let mut footer = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(8.0).with_child(Box::new(copy));
        if let Some(at) = at {
            footer = footer.with_child(text(clock_time(at), view.ui, 11.0, ink(&theme, 0.4)));
        }
        column = column.with_child(footer.finish());
    }
    column.finish()
}

/// "3:07 PM", local time.
fn clock_time(at_ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(at_ms).single() {
        Some(time) => time.format("%-I:%M %p").to_string(),
        None => String::new(),
    }
}

fn kind_icon(kind: StepKind) -> AppIcon {
    match kind {
        StepKind::Read => AppIcon::File,
        StepKind::Edit => AppIcon::Edit,
        StepKind::Delete => AppIcon::Trash,
        StepKind::Search => AppIcon::Search,
        StepKind::Execute => AppIcon::Terminal,
        StepKind::Fetch => AppIcon::Globe,
        StepKind::Think => AppIcon::Sparkle,
        StepKind::Agent => AppIcon::Agent,
        StepKind::Plan => AppIcon::TaskList,
        StepKind::Other => AppIcon::Tool,
    }
}

/// A step hanging from the guide: a vertical stroke down the left with an
/// elbow into the step's text; the final step's stroke stops there with a
/// rounded corner. The stroke is the left border of the step's own box, so it
/// spans the step's height without a flexible child (the chat sits in a
/// scroll view, whose height is unbounded).
fn guide(last: bool, body: Box<dyn Element>, view: &ChatView<'_>) -> Box<dyn Element> {
    let color = ink(&view.theme, 0.16);
    let track = if last {
        let stroke = ConstrainedBox::new(
            Container::new(Empty::new().finish()).with_border(Border::left(1.0).with_border_color(color)).finish(),
        )
        .with_height(6.0)
        .with_width(1.0)
        .finish();
        let elbow = ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_border(Border::new(1.0).with_sides(false, true, true, false).with_border_color(color))
                .with_corner_radius(CornerRadius::with_bottom_left(Radius::Pixels(8.0)))
                .finish(),
        )
        .with_width(8.0)
        .with_height(8.0)
        .finish();
        Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start).with_child(stroke).with_child(elbow).finish()
    } else {
        // The box's left border is the stroke; the elbow is a short rule
        // branching off it at the same height as the last step's corner.
        let spacer = ConstrainedBox::new(Empty::new().finish()).with_height(13.0).with_width(7.0).finish();
        let rule = ConstrainedBox::new(
            Container::new(Empty::new().finish()).with_border(Border::bottom(1.0).with_border_color(color)).finish(),
        )
        .with_height(1.0)
        .with_width(7.0)
        .finish();
        Flex::column().with_child(spacer).with_child(rule).finish()
    };
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(track)
        .with_child(Expanded::new(1.0, body).finish())
        .finish();
    let boxed =
        if last { row } else { Container::new(row).with_border(Border::left(1.0).with_border_color(color)).finish() };
    Container::new(boxed).with_padding_left(6.0).finish()
}

fn tally(added: Option<u32>, removed: Option<u32>, view: &ChatView<'_>) -> Option<Box<dyn Element>> {
    if added.is_none() && removed.is_none() {
        return None;
    }
    Some(
        Flex::row()
            .with_spacing(4.0)
            .with_child(line_text(format!("+{}", added.unwrap_or(0)), view.mono, 12.0, view.theme.ui_green()))
            .with_child(line_text(format!("−{}", removed.unwrap_or(0)), view.mono, 12.0, view.theme.ui_red()))
            .finish(),
    )
}

/// One step: the action word, a file chip or the command in a monospace
/// face, line counts for edits. Unfinished steps shimmer and count seconds;
/// finished ones recede into a quieter colour. Click to see the full command
/// or path.
fn step_line(step: &Step, view: &ChatView<'_>) -> Box<dyn Element> {
    let theme = view.theme;
    let detail_key = format!("chat-step:{}", step.id);
    let open = view.open.contains(&detail_key);
    let detail = step.detail.clone().or_else(|| (step.label.chars().count() > 72).then(|| step.label.clone()));
    let quiet = ink(&theme, 0.5);
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center).with_spacing(6.0);
    if step.running {
        row = row.with_child(render_icon(AppIcon::Clock, quiet, 12.0));
        let words = [step.verb.clone(), if step.file.is_some() { None } else { Some(step.label.clone()) }]
            .into_iter()
            .flatten()
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        row = row.with_child(Shrinkable::new(1.0, shimmer(view, &detail_key, words, 13.0)).finish());
    } else if let Some(verb) = &step.verb {
        row = row.with_child(line_text(verb.clone(), view.ui, 13.0, if step.failed { theme.ui_red() } else { quiet }));
    }
    if let Some(file) = &step.file {
        row = row.with_child(
            Shrinkable::new(
                1.0,
                Container::new(line_text(file.clone(), view.mono, 12.0, ink(&theme, 0.72)))
                    .with_padding_left(4.0)
                    .with_padding_right(4.0)
                    .with_padding_top(1.0)
                    .with_padding_bottom(1.0)
                    .with_background_color(ink(&theme, 0.07))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                    .finish(),
            )
            .finish(),
        );
    } else if !step.running && !step.label.is_empty() {
        let font = if step.mono { view.mono } else { view.ui };
        let label = super::layout::first_line(&step.label, 200);
        row = row.with_child(Shrinkable::new(1.0, line_text(label, font, if step.mono { 12.5 } else { 13.0 }, quiet)).finish());
    }
    if !step.running {
        if let Some(t) = tally(step.added, step.removed, view) {
            row = row.with_child(t);
        }
    }
    if let (true, Some(start)) = (step.running, step.started_at) {
        row = row.with_child(line_text(running_time(start, view.now_ms), view.mono, 11.0, ink(&theme, 0.4)));
    }
    if step.failed {
        row = row.with_child(line_text("failed", view.mono, 11.0, theme.ui_red()));
    }
    let row = Container::new(row.finish()).with_padding_top(4.0).with_padding_bottom(4.0).finish();
    let Some(detail) = detail else {
        return row;
    };
    let action_key = detail_key.clone();
    let head = Hoverable::new(view.ctx.mouse_state(&detail_key), move |_| row)
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event, _, _| event.dispatch_typed_action(toggle(action_key.clone())));
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_child(Box::new(head));
    if open {
        column = column.with_child(
            Container::new(text(detail, view.mono, 12.0, quiet))
                .with_margin_left(7.0)
                .with_margin_bottom(8.0)
                .with_padding_left(12.0)
                .with_border(Border::left(1.0).with_border_color(ink(&theme, 0.16)))
                .finish(),
        );
    }
    column.finish()
}

/// Back-to-back steps under one tally ("6 commands · 3 edits"). Folded until
/// clicked; while the agent works on them the newest few stay listed.
fn run(key: &str, steps: &[Step], active: bool, view: &ChatView<'_>) -> Box<dyn Element> {
    let theme = view.theme;
    let open_key = format!("chat-run:{key}");
    let open = view.open.contains(&open_key);
    let done: Vec<Step> = steps.iter().filter(|s| !s.running).cloned().collect();
    let live = done.is_empty();
    let summary = run_summary(if live { steps } else { &done });
    let quiet = ink(&theme, 0.5);
    let icon = render_icon(kind_icon(dominant_kind(steps)), ink(&theme, 0.45), 13.0);
    let label = if live { shimmer(view, &open_key, summary, 13.0) } else { line_text(summary, view.ui, 13.0, quiet) };
    let chevron = render_icon(if open { AppIcon::ChevronDown } else { AppIcon::ChevronRight }, ink(&theme, 0.45), 12.0);
    let header_row = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(icon)
            .with_child(Shrinkable::new(1.0, label).finish())
            .with_child(chevron)
            .finish(),
    )
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish();
    let action_key = open_key.clone();
    let header = Hoverable::new(view.ctx.mouse_state(&open_key), move |_| header_row)
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event, _, _| event.dispatch_typed_action(toggle(action_key.clone())));
    let shown: &[Step] = if open {
        steps
    } else if active {
        &steps[steps.len().saturating_sub(ACTIVE_TAIL)..]
    } else {
        &[]
    };
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_child(Box::new(header));
    for (index, step) in shown.iter().enumerate() {
        column = column.with_child(guide(index + 1 == shown.len(), step_line(step, view), view));
    }
    column.finish()
}

/// The files a finished turn touched, with what each cost.
fn changes(files: &[ChangedFile], view: &ChatView<'_>) -> Box<dyn Element> {
    let theme = view.theme;
    let border = theme.outline();
    let added: u32 = files.iter().map(|f| f.added).sum();
    let removed: u32 = files.iter().map(|f| f.removed).sum();
    let mut header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(render_icon(AppIcon::GitCompare, ink(&theme, 0.45), 13.0))
        .with_child(
            Expanded::new(
                1.0,
                line_text(format!("{} file{} changed", files.len(), if files.len() == 1 { "" } else { "s" }), view.ui, 13.0, theme.main_text()),
            )
            .finish(),
        );
    if let Some(t) = tally(Some(added), Some(removed), view) {
        header = header.with_child(t);
    }
    let mut card = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_child(
        Container::new(header.finish())
            .with_padding_left(12.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(ink(&theme, 0.03))
            .with_border(Border::bottom(1.0).with_border_color(border))
            .finish(),
    );
    for (index, file) in files.iter().enumerate() {
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, line_text(file.name.clone(), view.mono, 12.5, ink(&theme, 0.72))).finish());
        if let Some(t) = tally(Some(file.added), Some(file.removed), view) {
            row = row.with_child(t);
        }
        let mut cell = Container::new(row.finish()).with_padding_left(12.0).with_padding_right(10.0).with_padding_top(6.0).with_padding_bottom(6.0);
        if index > 0 {
            cell = cell.with_border(Border::top(1.0).with_border_color(border));
        }
        card = card.with_child(cell.finish());
    }
    Container::new(card.finish())
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish()
}

/// Weighted label used by the chat's headings (kept for callers drawing
/// empty states in the same style).
pub fn heading(content: &str, view: &ChatView<'_>) -> Box<dyn Element> {
    Text::new(content.to_string(), view.ui, text_size(15.0))
        .with_color(view.theme.main_text())
        .with_style(Properties { weight: Weight::Semibold, ..Default::default() })
        .finish()
}
