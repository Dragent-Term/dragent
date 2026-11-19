//! Shared onboarding chrome — centered checklist language (Option G).
//!
//! Welcome + single "Get set up" checklist. No theme step.

use warpui::elements::{
    Align, Border, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Flex, Image,
    MainAxisAlignment, MainAxisSize, ParentElement, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;

use super::onboarding_flow::OnboardingAction;

pub const INTRO_CONTENT_MAX_WIDTH: f32 = 421.0;
pub const SETUP_CONTENT_MAX_WIDTH: f32 = 608.0;

pub const INTRO_TITLE_SIZE: f32 = 30.0;
pub const SETUP_TITLE_SIZE: f32 = 24.0;
pub const SUBTITLE_SIZE: f32 = 18.0;
pub const BODY_SIZE: f32 = 15.0;
pub const CAPTION_SIZE: f32 = 13.0;
pub const EYEBROW_SIZE: f32 = 12.0;

pub const LOGO_ASSET_PATH: &str = "icons/dragent-icon.png";
pub const LOGO_SIZE_WELCOME: f32 = 72.0;

/// Small uppercase section label in the accent color.
pub fn eyebrow(text: &str, theme: &Theme, ui_font: FamilyId) -> Box<dyn Element> {
    Box::new(Text::new(text.to_uppercase(), ui_font, crate::theme::text_size(EYEBROW_SIZE)).with_color(theme.accent))
}

/// Pill step dots: the active step is a long pill, completed
/// steps are short filled pills, upcoming steps are dim.
pub fn pill_dots(theme: &Theme, count: usize, active: usize) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(6.0)
        .with_children((0..count).map(|i| {
            let (w, color) = if i == active {
                (24.0, theme.accent)
            } else if i < active {
                (8.0, theme.accent)
            } else {
                (8.0, theme.surface_3())
            };
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(w)
                    .with_height(8.0)
                    .finish(),
            )
            .with_background_color(color)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
        }))
        .finish()
}

fn bar_segment(color: warpui::color::ColorU) -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_height(2.0)
            .finish(),
    )
    .with_background_color(color)
    .finish()
}

/// Full-bleed 2px progress bar; `fraction` of it is filled with accent.
pub fn progress_bar_full(theme: &Theme, fraction: f32) -> Box<dyn Element> {
    let f = fraction.clamp(0.0, 1.0);
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Shrinkable::new(f.max(0.001), bar_segment(theme.accent)).finish())
        .with_child(Shrinkable::new((1.0 - f).max(0.001), bar_segment(theme.surface_3())).finish())
        .finish()
}

/// 20px circular checklist badge — filled with a check when done.
pub fn check_badge(theme: &Theme, done: bool) -> Box<dyn Element> {
    let inner: Box<dyn Element> = if done {
        Align::new(render_icon(AppIcon::Check, theme.accent_text(), 11.0)).finish()
    } else {
        Empty::new().finish()
    };
    let boxed = ConstrainedBox::new(inner)
        .with_width(20.0)
        .with_height(20.0)
        .finish();
    if done {
        Container::new(boxed)
            .with_background_color(theme.accent)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish()
    } else {
        Container::new(boxed)
            .with_border(Border::all(1.5).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish()
    }
}

/// Card shell for one checklist task. The open task gets an accent border.
pub fn task_card(theme: &Theme, open: bool, content: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(content)
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(if open {
            theme.accent
        } else {
            theme.outline()
        }))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
        .with_padding_top(16.0)
        .with_padding_bottom(16.0)
        .with_padding_left(18.0)
        .with_padding_right(18.0)
        .finish()
}

/// Live-detection line: accent dot + muted caption.
pub fn status_ping(theme: &Theme, ui_font: FamilyId, label: &str) -> Box<dyn Element> {
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(6.0)
                    .with_height(6.0)
                    .finish(),
            )
            .with_background_color(theme.accent)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish(),
        )
        .with_child(Box::new(
            Text::new(label.to_string(), ui_font, crate::theme::text_size(CAPTION_SIZE)).with_color(theme.sub_text()),
        ))
        .finish()
}

/// Full-window centered intro (no side column).
pub fn intro_centered(content: Box<dyn Element>) -> Box<dyn Element> {
    let constrained = ConstrainedBox::new(content)
        .with_max_width(INTRO_CONTENT_MAX_WIDTH)
        .finish();
    Align::new(constrained).finish()
}

/// Centered setup column (wider than the intro).
pub fn setup_centered(content: Box<dyn Element>) -> Box<dyn Element> {
    let constrained = ConstrainedBox::new(content)
        .with_max_width(SETUP_CONTENT_MAX_WIDTH)
        .finish();
    Align::new(constrained).finish()
}

pub fn render_logo(size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Image::new(
            AssetSource::Bundled {
                path: LOGO_ASSET_PATH,
            },
            CacheOption::BySize,
        )
        .contain()
        .finish(),
    )
    .with_width(size)
    .with_height(size)
    .finish()
}

pub fn render_primary_button(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: OnboardingAction,
    full_width: bool,
) -> Box<dyn Element> {
    let accent = theme.accent;
    let fg = theme.accent_text();
    let hover_accent = theme.foreground;
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if state.is_hovered() {
                hover_accent
            } else {
                accent
            };
            let text = Box::new(Text::new(label_owned.clone(), ui_font, crate::theme::text_size(BODY_SIZE)).with_color(fg));
            let body = button_label_body(text, full_width);
            let button = Container::new(body)
                .with_background_color(bg)
                .with_padding_top(10.0)
                .with_padding_bottom(10.0)
                .with_padding_left(20.0)
                .with_padding_right(20.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish();

            stretch_button(button, full_width)
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(flow_id, action.clone());
        }),
    )
}

/// Low-emphasis back control.
pub fn render_naked_button(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: OnboardingAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let sub = theme.sub_text();
    let main = theme.foreground;
    Box::new(
        Hoverable::new(hover, move |state| {
            Container::new(Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(BODY_SIZE))
                    .with_color(if state.is_hovered() { main } else { sub }),
            ))
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(flow_id, action.clone());
        }),
    )
}

pub fn render_secondary_button(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: OnboardingAction,
    full_width: bool,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let outline = theme.outline();
    let accent = theme.accent;
    let surface = theme.surface_2();
    let main = theme.foreground;
    let sub = theme.sub_text();
    Box::new(
        Hoverable::new(hover, move |state| {
            let border_color = if state.is_hovered() { accent } else { outline };
            let bg = if state.is_hovered() {
                surface
            } else {
                warpui::color::ColorU::new(0, 0, 0, 0)
            };
            let text_color = if state.is_hovered() { main } else { sub };
            let text =
                Box::new(Text::new(label_owned.clone(), ui_font, crate::theme::text_size(BODY_SIZE)).with_color(text_color));
            let body = button_label_body(text, full_width);
            let button = Container::new(body)
                .with_background_color(bg)
                .with_border(Border::all(1.0).with_border_color(border_color))
                .with_padding_top(10.0)
                .with_padding_bottom(10.0)
                .with_padding_left(20.0)
                .with_padding_right(20.0)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .finish();

            stretch_button(button, full_width)
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(flow_id, action.clone());
        }),
    )
}

fn button_label_body(text: Box<dyn Element>, full_width: bool) -> Box<dyn Element> {
    if full_width {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_child(text)
            .finish()
    } else {
        text
    }
}

fn stretch_button(button: Box<dyn Element>, full_width: bool) -> Box<dyn Element> {
    if full_width {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1.0, button).finish())
            .finish()
    } else {
        button
    }
}

pub fn render_text_link(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    action: OnboardingAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let link = theme.accent;
    let sub = theme.sub_text();
    Box::new(
        Hoverable::new(hover, move |state| {
            Box::new(
                Text::new(label_owned.clone(), ui_font, crate::theme::text_size(CAPTION_SIZE))
                    .with_color(if state.is_hovered() { link } else { sub }),
            )
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(flow_id, action.clone());
        }),
    )
}
