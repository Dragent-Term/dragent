//! The "Log out?" confirmation, shared by Settings → Account and the
//! workspace's sign-out controls (header account menu, sidebar organisation
//! area) so every sign-out asks the same way, honouring "Don't ask again".

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment,
    ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::{Action, AppContext, EntityId, SingletonEntity};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::theme::Theme;

use super::widgets::secondary_button;

/// What the dialog's controls dispatch to its owning view.
pub(crate) struct LogoutConfirmActions<A> {
    pub toggle_dont_ask: A,
    pub cancel: A,
    pub confirm: A,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn logout_confirm_modal<A: Action + Clone>(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    app: &AppContext,
    dont_ask: bool,
    hover: impl Fn(&str) -> MouseStateHandle,
    actions: LogoutConfirmActions<A>,
) -> Box<dyn Element> {
    let theme = *theme;
    let LogoutConfirmActions {
        toggle_dont_ask,
        cancel,
        confirm,
    } = actions;

    let has_session = crate::realtime::manager::SessionManager::as_ref(app).is_active();
    let subtitle = if has_session {
        "You have 1 shared session.".to_string()
    } else {
        "You'll be signed out on this device.".to_string()
    };

    let icon = ConstrainedBox::new(
        warpui::elements::Image::new(
            warpui_core::assets::asset_cache::AssetSource::Bundled {
                path: "icons/dragent-icon.png",
            },
            warpui_core::image_cache::CacheOption::Original,
        )
        .contain()
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .finish(),
    )
    .with_width(52.0)
    .with_height(52.0)
    .finish();

    // "Don't ask again" checkbox row.
    let checked = dont_ask;
    let box_bg = if checked {
        theme.accent
    } else {
        ColorU::new(0, 0, 0, 0)
    };
    let check_row = Box::new(
        Hoverable::new(hover("logout_dontask"), move |_st| {
            let mut inner = Container::new(if checked {
                crate::icons::render_icon(crate::icons::AppIcon::Check, theme.foreground, 13.0)
            } else {
                warpui::elements::Empty::new().finish()
            })
            .with_background_color(box_bg)
            .with_border(Border::all(1.0).with_border_color(theme.border()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .with_padding_left(3.0)
            .with_padding_right(3.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0);
            if !checked {
                inner = inner
                    .with_padding_left(9.0)
                    .with_padding_right(9.0)
                    .with_padding_top(9.0)
                    .with_padding_bottom(9.0);
            }
            Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(9.0)
                    .with_child(inner.finish())
                    .with_child(
                        Text::new("Don't ask again".to_string(), ui, crate::theme::text_size(13.0))
                            .with_color(theme.main_text())
                            .finish(),
                    )
                    .finish(),
            )
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, toggle_dont_ask.clone())),
    );

    // "Yes, log out" — a filled purple confirm button.
    let purple = ColorU::new(168, 85, 200, 255);
    let confirm = Box::new(
        Hoverable::new(hover("logout_confirm"), move |st| {
            let bg = if st.is_hovered() {
                ColorU::new(
                    purple.r.saturating_add(15),
                    purple.g.saturating_add(15),
                    purple.b.saturating_add(15),
                    255,
                )
            } else {
                purple
            };
            Container::new(
                Text::new("Yes, log out".to_string(), ui, crate::theme::text_size(13.0))
                    .with_color(ColorU::new(255, 255, 255, 255))
                    .finish(),
            )
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(9.0)
            .with_padding_bottom(9.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(999.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, confirm.clone())),
    );

    let dialog = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(icon)
            .with_child(
                Text::new("Log out?".to_string(), ui, crate::theme::text_size(17.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(
                Text::new(subtitle, ui, crate::theme::text_size(13.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_child(check_row)
            .with_child(
                Flex::row()
                    .with_spacing(8.0)
                    .with_main_axis_alignment(MainAxisAlignment::End)
                    .with_child(secondary_button(
                        &theme,
                        ui,
                        id,
                        hover("logout_cancel"),
                        "Cancel",
                        cancel,
                    ))
                    .with_child(confirm)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.border()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
    .with_uniform_padding(24.0)
    .finish();

    let centered = ConstrainedBox::new(dialog).with_max_width(380.0).finish();
    Container::new(Align::new(centered).finish())
        .with_background_color(ColorU::new(0, 0, 0, 160))
        .finish()
}
