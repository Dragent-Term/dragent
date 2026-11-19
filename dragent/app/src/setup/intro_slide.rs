use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Empty, Flex, Image, MainAxisSize, ParentElement, Stack,
    Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::text_layout::TextAlignment;
use warpui::EntityId;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::elements::MouseStateHandle;
use warpui_core::image_cache::CacheOption;

use crate::theme::Theme;

use super::chrome::{intro_centered, render_logo, render_primary_button, INTRO_TITLE_SIZE};
use super::onboarding_flow::OnboardingAction;

pub fn render_intro_slide(
    theme: &Theme,
    ui_font: FamilyId,
    flow_id: EntityId,
    next_hover: MouseStateHandle,
) -> Box<dyn Element> {
    let content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(render_logo(64.0))
        .with_child(Box::new(
            Text::new("Welcome to Dragent.".to_string(), ui_font, crate::theme::text_size(INTRO_TITLE_SIZE))
                .with_color(theme.foreground)
                .with_style(Properties::default().weight(Weight::Bold)),
        ))
        .with_child(Box::new(
            Text::new(
                "Your projects, terminals, and agents have a place to work together.".to_string(),
                ui_font,
                crate::theme::text_size(14.0),
            )
            .with_color(theme.sub_text())
            .with_alignment(TextAlignment::Center),
        ))
        .with_child(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(render_primary_button(
                    theme,
                    ui_font,
                    flow_id,
                    next_hover,
                    "Set up workspace →",
                    OnboardingAction::Next,
                    false,
                ))
                .finish(),
        )
        .finish();

    let backdrop = Container::new(Empty::new().finish())
        .with_background_color(theme.solid_bg())
        .finish();
    let stellar = Image::new(
        AssetSource::Bundled {
            path: "dragent/stellar_bg.jpg",
        },
        CacheOption::Original,
    )
    .cover()
    .finish();
    let image_scrim = Container::new(Empty::new().finish())
        .with_background_color(ColorU::new(
            theme.background.r,
            theme.background.g,
            theme.background.b,
            185,
        ))
        .finish();
    let foreground = Container::new(intro_centered(content))
        .with_padding_left(24.0)
        .with_padding_right(24.0)
        .with_padding_bottom(120.0)
        .finish();

    Stack::new()
        .with_child(backdrop)
        .with_child(stellar)
        .with_child(image_scrim)
        .with_child(foreground)
        .finish()
}
