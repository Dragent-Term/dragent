//! How a provider looks wherever it's shown: its logo on a rounded tile.

use warpui::color::ColorU;
use warpui::elements::{Align, ConstrainedBox, Container, Element, Text};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::{Logo, Provider};
use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;

fn rgb(hex: u32) -> ColorU {
    ColorU::new((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255)
}

fn luminance(color: ColorU) -> f32 {
    (0.2126 * color.r as f32 + 0.7152 * color.g as f32 + 0.0722 * color.b as f32) / 255.0
}

/// The colour to draw `provider`'s mark in: its brand colour, unless that
/// would vanish into the theme (GitHub's near-black on a dark tile).
pub fn color(theme: &Theme, provider: Provider) -> ColorU {
    let brand = rgb(provider.color());
    let light = luminance(brand);
    if (theme.is_dark && light < 0.22) || (!theme.is_dark && light > 0.85) {
        theme.main_text()
    } else {
        brand
    }
}

/// The bare mark at `size`, for menus and pills.
pub fn mark(
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    provider: Provider,
    size: f32,
) -> Box<dyn Element> {
    match provider.logo() {
        Logo::Svg(path) => render_icon(AppIcon::Asset(path), color(theme, provider), size),
        Logo::Letters(letters) => letters_tile(theme, ui, provider, letters, size),
    }
}

/// The mark centred on a rounded tile of `size` (cards, dialog headers).
pub fn tile(
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    provider: Provider,
    size: f32,
) -> Box<dyn Element> {
    match provider.logo() {
        Logo::Letters(letters) => letters_tile(theme, ui, provider, letters, size),
        Logo::Svg(path) => Container::new(
            ConstrainedBox::new(
                Align::new(render_icon(
                    AppIcon::Asset(path),
                    color(theme, provider),
                    (size * 0.5).round(),
                ))
                .finish(),
            )
            .with_width(size)
            .with_height(size)
            .finish(),
        )
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
            (size * 0.26).round(),
        )))
        .finish(),
    }
}

/// Brands without a bundled logo: their initials in their colour, on the
/// same quiet tile as the others.
fn letters_tile(
    theme: &Theme,
    ui: warpui::fonts::FamilyId,
    provider: Provider,
    letters: &str,
    size: f32,
) -> Box<dyn Element> {
    let font = (size * if letters.len() > 1 { 0.36 } else { 0.5 }).round();
    Container::new(
        ConstrainedBox::new(
            Align::new(
                Text::new(letters.to_string(), ui, crate::theme::text_size(font))
                    .with_color(color(theme, provider))
                    .finish(),
            )
            .finish(),
        )
        .with_width(size)
        .with_height(size)
        .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
        (size * 0.26).round(),
    )))
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_brands_stay_visible_on_dark_themes() {
        let dark = Theme::dark();
        assert_eq!(color(&dark, Provider::Github), dark.main_text());
        assert_eq!(color(&dark, Provider::Gitlab), rgb(0xFC6D26));
        let light = Theme::light();
        assert_eq!(color(&light, Provider::Github), rgb(0x181717));
    }
}
