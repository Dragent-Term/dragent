use warpui::elements::{Element, Flex, MainAxisSize, ParentElement};

use warpui::AppContext;

use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{link_button, page_header};

pub fn render_privacy(view: &SettingsView, theme: &Theme, _app: &AppContext) -> Box<dyn Element> {
    let ui_font = view.ui_font;

    Flex::column()
        .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(
            theme,
            ui_font,
            "Privacy",
            "Review how Dragent handles your data.",
        ))
        .with_child(link_button(
            theme,
            ui_font,
            view.view_id,
            view.hover("privacy_link"),
            "Privacy policy",
            SettingsAction::OpenPrivacyPolicy,
        ))
        .finish()
}
