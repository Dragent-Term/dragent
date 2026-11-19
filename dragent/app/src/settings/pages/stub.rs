//! Placeholder pages for settings areas whose functionality is still being
//! built. Each renders the standard page header plus an intentional
//! empty-state so the information architecture reads as
//! the production target instead of a dead end.

use warpui::elements::{CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement};

use crate::theme::Theme;

use super::super::settings_view::SettingsView;
use super::super::widgets::{empty_state, page_header, status_badge};

pub fn render_coming_soon(
    view: &SettingsView,
    theme: &Theme,
    title: &str,
    caption: &str,
) -> Box<dyn Element> {
    render_planned(view, theme, title, caption, title)
}

/// Planned page with its own empty-state headline (usually the thing the
/// page will list, e.g. "No teams yet").
pub fn render_planned(
    view: &SettingsView,
    theme: &Theme,
    title: &str,
    caption: &str,
    empty_title: &str,
) -> Box<dyn Element> {
    let ui_font = view.ui_font;

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(theme, ui_font, title, caption))
        .with_child(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(status_badge(theme, ui_font, "Planned", false))
                .finish(),
        )
        .with_child(empty_state(
            theme,
            ui_font,
            empty_title,
            "This area is still being built. Check back in a future update.",
        ))
        .finish()
}
