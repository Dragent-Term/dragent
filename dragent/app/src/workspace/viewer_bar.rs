//! The thin status bar shown at the top of a joined (viewer) session's mirror
//! tab: connection status, who else is in the session, and a Request-edit /
//! You-can-edit control. Purely presentational — it dispatches
//! [`WorkspaceAction`]s and reads a cached [`ViewerBarSnapshot`].

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Expanded, Flex, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::{scaled, Theme};

use super::action::WorkspaceAction;
use super::share_sheet::{avatar, ShareParticipant};

type States = RefCell<HashMap<String, MouseStateHandle>>;

const TRANSPARENT: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 0,
};
const WHITE: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// A UI snapshot of a joined session, from the viewer's side.
#[derive(Clone, Debug, Default)]
pub struct ViewerBarSnapshot {
    /// The realtime socket is connected.
    pub online: bool,
    /// This viewer currently holds Edit (control).
    pub can_edit: bool,
    /// Everyone in the session the gateway reports (host + viewers, incl. self).
    pub participants: Vec<ShareParticipant>,
}

fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

/// Render the viewer status bar for the mirror tab.
pub fn render_viewer_bar(
    snap: &ViewerBarSnapshot,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let (dot, status) = if snap.online {
        (theme.ui_green(), "Live")
    } else {
        (theme.ui_yellow(), "Connecting…")
    };

    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(status_dot(dot))
        .with_child(
            Text::new("Viewing shared session".to_string(), ui, crate::theme::text_size(12.5))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(
            Text::new(status.to_string(), ui, crate::theme::text_size(11.5))
                .with_color(theme.sub_text())
                .finish(),
        )
        .with_child(Expanded::new(1.0, Empty()).finish());

    // Presence: overlapping row of avatars for everyone in the session.
    if !snap.participants.is_empty() {
        let mut people = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            // Keep each photo's circular silhouette visible. Overlapping the
            // square avatar bounds makes adjacent circles read as one eye-like
            // shape, especially when the photos have light backgrounds.
            .with_spacing(2.0);
        for p in snap.participants.iter().take(6) {
            people = people.with_child(avatar(&p.name, &p.email, &p.avatar_url, theme, ui));
        }
        row = row.with_child(people.finish());
    }

    row = row.with_child(edit_control(snap.can_edit, theme, ui, id, states));

    Container::new(row.finish())
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(7.0)
        .with_padding_bottom(7.0)
        .with_background_color(theme.surface_1())
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish()
}

fn status_dot(color: ColorU) -> Box<dyn Element> {
    warpui::elements::ConstrainedBox::new(
        Container::new(Empty())
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish(),
    )
    .with_width(8.0)
    .with_height(8.0)
    .finish()
}

/// Right-hand control: a green "You can edit" pill when this viewer holds
/// control, otherwise a clickable "Request edit" button.
fn edit_control(
    can_edit: bool,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    if can_edit {
        return Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(6.0)
                .with_child(render_icon(AppIcon::Edit, theme.ui_green(), 13.0))
                .with_child(
                    Text::new("You can edit".to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.ui_green())
                        .finish(),
                )
                .finish(),
        )
        .with_padding_left(10.0)
        .with_padding_right(10.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(tint(theme.ui_green(), 28))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish();
    }

    let hover = states
        .borrow_mut()
        .entry("viewer_request_edit".to_string())
        .or_default()
        .clone();
    let accent = theme.accent;
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() {
                accent
            } else {
                tint(accent, 210)
            };
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(6.0)
                    .with_child(render_icon(AppIcon::Edit, WHITE, 13.0))
                    .with_child(
                        Text::new("Request edit".to_string(), ui, crate::theme::text_size(12.0))
                            .with_color(WHITE)
                            .finish(),
                    )
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, WorkspaceAction::RequestSessionEdit)
        }),
    )
}

/// A zero-size placeholder element.
#[allow(non_snake_case)]
fn Empty() -> Box<dyn Element> {
    warpui::elements::Empty::new().finish()
}
