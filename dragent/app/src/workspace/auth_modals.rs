//! Auth-related modals for the workspace: the "Welcome to Dragent" sign-in
//! prompt shown when an action that needs an account (e.g. sharing a session) is
//! taken while signed out. Styled to match the app's own modals.

use std::cell::RefCell;
use std::collections::HashMap;

use warpui::color::ColorU;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element,
    EventHandler, Flex, Image, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::theme::{scaled, Theme};

use super::action::WorkspaceAction;

type States = RefCell<HashMap<String, MouseStateHandle>>;

const PANEL_W: f32 = 430.0;
const WHITE: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

const TERMS_URL: &str = "https://dragent.dev/terms";
const PRIVACY_URL: &str = "https://dragent.dev/privacy";

fn tint(c: ColorU, a: u8) -> ColorU {
    ColorU {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
}

/// The app's brand mark in a rounded square.
pub fn brand_icon(size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Image::new(
            AssetSource::Bundled {
                path: "icons/dragent-icon.png",
            },
            CacheOption::Original,
        )
        .contain()
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
        .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

/// "Welcome to Dragent" — shown when a sign-in-required action is taken while
/// signed out. Sign up / Sign in start the browser OAuth flow; Skip dismisses.
pub fn render_login_prompt(
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(18.0)
            .with_child(brand_icon(56.0))
            .with_child(
                Text::new("Welcome to Dragent!".to_string(), ui, crate::theme::text_size(22.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(signup_button(theme, ui, id, states))
            .with_child(link_line(
                "Already have an account? ",
                "Sign in",
                "signin",
                WorkspaceAction::SignInFromHeader,
                theme,
                ui,
                id,
                states,
            ))
            .with_child(link_line(
                "Don't want to sign in right now? ",
                "Skip for now",
                "skip",
                WorkspaceAction::CloseLoginPrompt,
                theme,
                ui,
                id,
                states,
            ))
            .with_child(footer(theme, ui, id, states))
            .finish(),
    )
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(14.0)))
    .with_padding_left(26.0)
    .with_padding_right(26.0)
    .with_padding_top(24.0)
    .with_padding_bottom(24.0)
    .finish();

    modal_scrim(panel, WorkspaceAction::CloseLoginPrompt, id)
}

fn footer(theme: &Theme, ui: FamilyId, id: EntityId, states: &States) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(8.0)
        .with_child(link_line(
            "By continuing, you agree to our ",
            "Terms of Service",
            "terms",
            WorkspaceAction::OpenExternalUrl {
                url: TERMS_URL.to_string(),
            },
            theme,
            ui,
            id,
            states,
        ))
        .with_child(link_line(
            "You can adjust your ",
            "Privacy Settings",
            "privacy",
            WorkspaceAction::OpenExternalUrl {
                url: PRIVACY_URL.to_string(),
            },
            theme,
            ui,
            id,
            states,
        ))
        .finish()
}

/// A big full-width filled "Sign up" button.
fn signup_button(theme: &Theme, ui: FamilyId, id: EntityId, states: &States) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry("auth_signup".to_string())
        .or_default()
        .clone();
    let base = theme.accent;
    let hovered = tint(theme.accent, 220);
    Box::new(
        Hoverable::new(hover, move |st| {
            let bg = if st.is_hovered() { hovered } else { base };
            Container::new(
                Align::new(
                    Text::new("Sign up".to_string(), ui, crate::theme::text_size(14.0))
                        .with_color(WHITE)
                        .finish(),
                )
                .finish(),
            )
            .with_padding_top(12.0)
            .with_padding_bottom(12.0)
            .with_background_color(bg)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
            .finish()
        })
        .on_click(move |ectx, _, _| {
            ectx.dispatch_typed_action_for(id, WorkspaceAction::SignInFromHeader)
        }),
    )
}

/// A "{prefix}{link}" row where the link portion is a clickable accent word.
#[allow(clippy::too_many_arguments)]
fn link_line(
    prefix: &str,
    link: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if !prefix.is_empty() {
        row = row.with_child(
            Text::new(prefix.to_string(), ui, crate::theme::text_size(13.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    }
    row.with_child(link_text(link, key, action, theme, ui, id, states))
        .finish()
}

fn link_text(
    label: &str,
    key: &str,
    action: WorkspaceAction,
    theme: &Theme,
    ui: FamilyId,
    id: EntityId,
    states: &States,
) -> Box<dyn Element> {
    let hover = states
        .borrow_mut()
        .entry(format!("authlink_{key}"))
        .or_default()
        .clone();
    let label = label.to_string();
    let base = theme.accent;
    let hovered = tint(theme.accent, 200);
    Box::new(
        Hoverable::new(hover, move |st| {
            let color = if st.is_hovered() { hovered } else { base };
            Text::new(label.clone(), ui, crate::theme::text_size(13.0))
                .with_color(color)
                .finish()
        })
        .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(id, action.clone())),
    )
}

/// Wrap a panel in a dismissable centered scrim. Clicking the panel is swallowed;
/// clicking the scrim dispatches `dismiss`.
pub fn modal_scrim(
    panel: Box<dyn Element>,
    dismiss: WorkspaceAction,
    id: EntityId,
) -> Box<dyn Element> {
    let panel = ConstrainedBox::new(panel).with_max_width(PANEL_W).finish();
    modal_scrim_wide(panel, dismiss, id)
}

/// [`modal_scrim`] for a panel that sets its own width.
pub fn modal_scrim_wide(
    panel: Box<dyn Element>,
    dismiss: WorkspaceAction,
    id: EntityId,
) -> Box<dyn Element> {
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_e, _a, _p| DispatchEventResult::StopPropagation)
        .finish();
    EventHandler::new(
        Container::new(Align::new(panel).finish())
            .with_background_color(ColorU {
                r: 0,
                g: 0,
                b: 0,
                a: 150,
            })
            .finish(),
    )
    .on_left_mouse_down(move |ectx, _, _| {
        ectx.dispatch_typed_action_for(id, dismiss.clone());
        DispatchEventResult::StopPropagation
    })
    .finish()
}
