use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, EventHandler, Expanded, Flex, ParentElement, ScrollbarWidth,
    Text,
};
use warpui::fonts::FamilyId;
use warpui::{Action, EntityId};

/// The most of a message the popup shows.
const MAX_SHOWN_CHARS: usize = 4_000;

/// `error`, or its start and how much more there is when it's very long.
fn shown_text(error: &str) -> String {
    match error.char_indices().nth(MAX_SHOWN_CHARS) {
        None => error.to_string(),
        Some((cut, _)) => {
            let more = error[cut..].lines().count();
            format!("{}…\n\n({more} more lines: Copy has the whole text)", &error[..cut])
        }
    }
}

/// Shared copyable error popup, used at window level by every workspace surface.
pub(crate) fn render<A: Action + Clone>(
    title: &str,
    error: &str,
    theme: &crate::theme::Theme,
    mono: FamilyId,
    scroll: ClippedScrollStateHandle,
    copied: bool,
    pane_view_id: EntityId,
    dismiss: A,
    copy: A,
) -> Box<dyn Element> {
    let red = theme.ui_red();

    let close_btn = EventHandler::new(
        Container::new(crate::icons::render_icon(
            crate::icons::AppIcon::XClose,
            theme.disabled_text(),
            13.0,
        ))
        .with_padding_left(8.0)
        .with_padding_right(4.0)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .finish(),
    )
    .on_left_mouse_down(move |event_ctx, _, _| {
        event_ctx.dispatch_typed_action_for(pane_view_id, dismiss.clone());
        DispatchEventResult::StopPropagation
    })
    .finish();

    let copy_btn: Box<dyn Element> = if copied {
        Container::new(
            Text::new("Copied!".to_string(), mono, crate::theme::text_size(11.0))
                .with_color(theme.ui_green())
                .finish(),
        )
        .with_padding_left(4.0)
        .with_padding_right(4.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .finish()
    } else {
        EventHandler::new(
            Container::new(crate::icons::render_icon(
                crate::icons::AppIcon::Copy,
                theme.disabled_text(),
                11.0,
            ))
            .with_padding_left(4.0)
            .with_padding_right(4.0)
            .with_padding_top(2.0)
            .with_padding_bottom(2.0)
            .finish(),
        )
        .on_left_mouse_down(move |event_ctx, _, _| {
            event_ctx.dispatch_typed_action_for(pane_view_id, copy.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
    };

    let title_row = Flex::row()
        .with_child(Box::new(Expanded::new(
            1.0,
            Text::new(title.to_string(), mono, crate::theme::text_size(11.5))
                .with_color(red)
                .finish(),
        )))
        .with_child(copy_btn)
        .with_child(close_btn)
        .finish();

    // The error text in a scrollable body so long output (lint-staged, etc.)
    // is readable; a very long one shows its start (the popup lays its text
    // out on every frame), and Copy still takes all of it.
    let body_text = Container::new(
        Text::new(shown_text(error), mono, crate::theme::text_size(11.0))
            .with_color(theme.sub_text())
            .finish(),
    )
    .with_padding_top(6.0)
    .finish();

    let body_scroll = ClippedScrollable::vertical(
        scroll,
        body_text,
        ScrollbarWidth::Auto,
        theme.outline().into(),
        theme.sub_text().into(),
        theme.surface_2().into(),
    )
    .with_overlayed_scrollbar()
    .finish();

    let body_constrained = ConstrainedBox::new(body_scroll)
        .with_max_height(200.0)
        .finish();

    let inner = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(title_row)
        .with_child(body_constrained)
        .finish();

    ConstrainedBox::new(
        Container::new(inner)
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_background_color(theme.surface_2())
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(6.0),
            ))
            .finish(),
    )
    .with_max_width(360.0)
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_very_long_message_shows_its_start() {
        assert_eq!(shown_text("short"), "short");
        let long: String = (0..2_000).map(|n| format!("line {n}\n")).collect();
        let shown = shown_text(&long);
        assert!(shown.starts_with("line 0\n"));
        assert!(shown.ends_with("more lines: Copy has the whole text)"));
        assert!(shown.chars().count() < MAX_SHOWN_CHARS + 80);
    }
}
