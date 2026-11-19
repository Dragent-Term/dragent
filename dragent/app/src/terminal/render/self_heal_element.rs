//! Renderer for the self-heal fix card shown below failed shell blocks.

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Flex, Hoverable, MainAxisSize,
    ParentElement, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::agent::api::types::BlockAnalysis;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

const ACCENT_WIDTH: f32 = 3.0;

/// Build the fix card element for a block in `Ready` state.
/// Placed below the block body (after text output).
pub fn build_fix_card(
    block_id: usize,
    analysis: &BlockAnalysis,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.ui_red();

    let header = fix_card_header(accent, theme, mono);

    let explanation_el = Text::new(
        analysis.explanation.clone(),
        mono,
        crate::theme::text_size(12.0),
    )
    .with_color(theme.main_text())
    .finish();

    let mut body_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(explanation_el);

    let cmd_str = analysis
        .suggested_command
        .as_deref()
        .filter(|s| !s.trim().is_empty());
    if let Some(cmd) = cmd_str {
        let cmd_el = Container::new(
            Text::new(format!("$ {cmd}"), mono, crate::theme::text_size(12.0))
                .with_color(theme.main_text())
                .with_style(Properties {
                    weight: Weight::Medium,
                    ..Default::default()
                })
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(theme.surface_3())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish();
        body_col = body_col.with_child(cmd_el);

        // The diagnosis is "rerun this corrected command" (the analysis produced
        // a shell command). So the PRIMARY action reruns it — the button says
        // what it does. "Fix with agent" stays as a secondary escape hatch for
        // when rerunning isn't enough and code actually needs editing.
        let rerun_btn = fix_button("Rerun command", true, block_id, ctx, theme, mono);
        let agent_fix_btn = fix_button("Fix with agent", false, block_id, ctx, theme, mono);
        let agent_btn = fix_button("Open in Agent", false, block_id, ctx, theme, mono);
        let dismiss_btn = fix_button("Dismiss", false, block_id, ctx, theme, mono);
        let buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(rerun_btn)
            .with_child(agent_fix_btn)
            .with_child(agent_btn)
            .with_child(dismiss_btn)
            .finish();
        body_col = body_col.with_child(buttons);
    } else {
        // No command to rerun → hand the analysis to the normal code-editing
        // agent workflow.
        let agent_btn = fix_button("Fix with agent", true, block_id, ctx, theme, mono);
        let dismiss_btn = fix_button("Dismiss", false, block_id, ctx, theme, mono);
        let buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(agent_btn)
            .with_child(dismiss_btn)
            .finish();
        body_col = body_col.with_child(buttons);
    }

    let body = Container::new(body_col.finish())
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(10.0)
        .finish();

    let body_with_accent = Container::new(body)
        .with_border(
            Border::left(ACCENT_WIDTH)
                .with_border_color(ColorU::new(accent.r, accent.g, accent.b, 50)),
        )
        .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header)
        .with_child(body_with_accent)
        .finish()
}

fn fix_card_header(accent: ColorU, theme: Theme, mono: FamilyId) -> Box<dyn Element> {
    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(accent)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();

    let label_el = Text::new(
        "Fix · Dragent".to_string(),
        mono,
        crate::theme::text_size(12.0),
    )
    .with_color(accent)
    .with_style(Properties {
        weight: Weight::Semibold,
        ..Default::default()
    })
    .finish();

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(label_el)
        .finish();

    let header_body = Container::new(row)
        .with_padding_left(10.0)
        .with_padding_right(13.0)
        .with_padding_top(10.0)
        .with_padding_bottom(10.0)
        .with_border(Border::left(ACCENT_WIDTH).with_border_color(accent))
        .finish();

    Container::new(header_body)
        .with_border(Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}

fn fix_button(
    label: &'static str,
    is_primary: bool,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let base_bg = if is_primary {
        theme.accent
    } else {
        theme.surface_3()
    };
    let text_color = if is_primary {
        ColorU::white()
    } else {
        theme.main_text()
    };

    let btn_inner = Container::new(
        Text::new(label.to_string(), mono, crate::theme::text_size(11.5))
            .with_color(text_color)
            .with_style(Properties {
                weight: Weight::Medium,
                ..Default::default()
            })
            .finish(),
    )
    .with_padding_left(10.0)
    .with_padding_right(10.0)
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .with_background_color(base_bg)
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish();

    // The button LABEL reflects the diagnosis (rerun a command vs edit code),
    // so it maps to the matching action here. "Rerun command" runs the
    // suggested shell command; "Fix with agent" launches the normal
    // code-editing agent workflow.
    let action: TerminalAction =
        if label == "Rerun command" || label == "Run fix" || label == "Run Fix" {
            TerminalAction::RunBlockFix { block_id }
        } else if label == "Fix with agent" {
            TerminalAction::AutoFixBlock { block_id }
        } else if label == "Open in Agent" {
            TerminalAction::OpenInAgent { block_id }
        } else {
            TerminalAction::DismissBlockFix { block_id }
        };

    let mouse_state = ctx.mouse_state(&format!("fix-{block_id}-{label}"));
    Box::new(
        Hoverable::new(mouse_state, move |_| btn_inner)
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(action.clone());
            }),
    )
}

/// Build an error card element for a block in `Error` state.
pub fn build_error_card(
    block_id: usize,
    message: &str,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.ui_red();

    let header = fix_card_header(accent, theme, mono);

    let error_el = Text::new(message.to_string(), mono, crate::theme::text_size(12.0))
        .with_color(theme.main_text())
        .finish();

    let dismiss_btn = fix_button("Dismiss", false, block_id, ctx, theme, mono);

    let body_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(error_el)
        .with_child(dismiss_btn)
        .finish();

    let body = Container::new(body_col)
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(10.0)
        .finish();

    let body_with_accent = Container::new(body)
        .with_border(
            Border::left(ACCENT_WIDTH)
                .with_border_color(ColorU::new(accent.r, accent.g, accent.b, 50)),
        )
        .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(header)
        .with_child(body_with_accent)
        .finish()
}

/// Compact chip shown when a background (proactive) scan found a fix but
/// "Auto-fix on failure" is off: one line, no explanation, a View button that
/// reveals the full card and a Dismiss affordance.
pub fn build_fix_available_card(
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.ui_yellow();

    let dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(accent)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();

    let label = Text::new(
        "Fix available".to_string(),
        mono,
        crate::theme::text_size(12.0),
    )
    .with_color(theme.main_text())
    .finish();

    let view_btn: Box<dyn Element> = {
        let btn_inner = Container::new(
            Text::new(
                "View fix".to_string(),
                mono,
                crate::theme::text_size(11.5),
            )
            .with_color(theme.accent_text())
            .with_style(Properties {
                weight: Weight::Medium,
                ..Default::default()
            })
            .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(theme.accent)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish();
        let mouse_state = ctx.mouse_state(&format!("fix-available-view-{block_id}"));
        Box::new(
            Hoverable::new(mouse_state, move |_| btn_inner)
                .with_cursor(Cursor::PointingHand)
                .on_click(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(TerminalAction::RevealBlockFix { block_id });
                }),
        )
    };

    let dismiss_btn = fix_button("Dismiss", false, block_id, ctx, theme, mono);

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(dot)
        .with_child(label)
        .with_child(view_btn)
        .with_child(dismiss_btn)
        .finish();

    Container::new(
        Container::new(row)
            .with_padding_left(16.0)
            .with_padding_right(13.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish(),
    )
    .with_border(
        Border::left(ACCENT_WIDTH).with_border_color(ColorU::new(accent.r, accent.g, accent.b, 50)),
    )
    .finish()
}
