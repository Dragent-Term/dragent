//! System settings: privacy-adjacent platform areas.
//!
//! Each page renders the standard header plus an intentional planned-state
//! until its backend lands.

use warpui::elements::Element;

use crate::theme::Theme;

use super::super::settings_view::SettingsView;
use super::stub::render_planned;

pub fn render_remote_access(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    render_planned(
        view,
        theme,
        "Remote Access",
        "Expose this workspace over a secure relay and manage remote sessions.",
        "Remote access is not set up yet",
    )
}
