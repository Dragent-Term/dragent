//! Semantic editor colors derived from the active application theme.
//!
//! Keeping this mapping out of the lexer means every built-in and imported
//! theme gets a coherent code palette without teaching the editor domain about
//! WarpUI colors.

use dragent_editor::TokenKind;
use warpui::color::ColorU;

use crate::theme::Theme;

#[derive(Clone, Copy, Debug)]
pub struct EditorPalette {
    plain: ColorU,
    comment: ColorU,
    keyword: ColorU,
    function: ColorU,
    type_name: ColorU,
    string: ColorU,
    number: ColorU,
    constant: ColorU,
    operator: ColorU,
    property: ColorU,
    macro_name: ColorU,
    delimiter: ColorU,
}

impl EditorPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        let readable = |color| ensure_contrast(color, theme.background, MIN_CODE_CONTRAST);
        Self {
            plain: readable(theme.main_text()),
            comment: readable(with_alpha(theme.ansi_normal[2], 0.78)),
            keyword: readable(theme.ansi_normal[4]),
            function: readable(theme.ansi_bright[3]),
            type_name: readable(theme.ansi_normal[6]),
            string: readable(theme.ansi_bright[2]),
            number: readable(theme.ansi_bright[2]),
            constant: readable(theme.ansi_bright[3]),
            operator: readable(theme.main_text()),
            property: readable(theme.ansi_bright[4]),
            macro_name: readable(theme.ansi_bright[5]),
            delimiter: readable(theme.ansi_bright[3]),
        }
    }

    pub fn plain(self) -> ColorU {
        self.plain
    }

    pub fn token(self, kind: TokenKind) -> ColorU {
        match kind {
            TokenKind::Comment => self.comment,
            TokenKind::Keyword => self.keyword,
            TokenKind::Function => self.function,
            TokenKind::Type => self.type_name,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Constant => self.constant,
            TokenKind::Operator => self.operator,
            TokenKind::Property => self.property,
            TokenKind::Macro => self.macro_name,
            TokenKind::Delimiter => self.delimiter,
        }
    }
}

/// WCAG AA contrast for normal-sized text. Terminal ANSI palettes are designed
/// for arbitrary terminal output and some imported themes contain colors that
/// are effectively invisible on their own background. Source code cannot use
/// those values unchecked.
const MIN_CODE_CONTRAST: f32 = 4.5;

fn ensure_contrast(foreground: ColorU, background: ColorU, minimum: f32) -> ColorU {
    // A background-image theme may make the editor surface transparent, but
    // its RGB channels still describe the intended base/tint. Treat those
    // channels as opaque here rather than incorrectly assuming black beneath
    // every image theme.
    let background = ColorU::new(background.r, background.g, background.b, 255);
    let foreground = composite_over(foreground, background);
    if contrast_ratio(foreground, background) >= minimum {
        return foreground;
    }

    let black = ColorU::new(0, 0, 0, 255);
    let white = ColorU::new(255, 255, 255, 255);
    let anchor = if contrast_ratio(white, background) >= contrast_ratio(black, background) {
        white
    } else {
        black
    };

    // Find the smallest adjustment that reaches the target, retaining as much
    // of the theme's original token hue as possible.
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..12 {
        let amount = (low + high) * 0.5;
        let candidate = mix(foreground, anchor, amount);
        if contrast_ratio(candidate, background) >= minimum {
            high = amount;
        } else {
            low = amount;
        }
    }
    mix(foreground, anchor, high)
}

fn composite_over(foreground: ColorU, background: ColorU) -> ColorU {
    let alpha = foreground.a as f32 / 255.0;
    ColorU::new(
        blend_channel(background.r, foreground.r, alpha),
        blend_channel(background.g, foreground.g, alpha),
        blend_channel(background.b, foreground.b, alpha),
        255,
    )
}

fn mix(from: ColorU, to: ColorU, amount: f32) -> ColorU {
    ColorU::new(
        blend_channel(from.r, to.r, amount),
        blend_channel(from.g, to.g, amount),
        blend_channel(from.b, to.b, amount),
        255,
    )
}

fn blend_channel(from: u8, to: u8, amount: f32) -> u8 {
    (from as f32 + (to as f32 - from as f32) * amount.clamp(0.0, 1.0)).round() as u8
}

fn contrast_ratio(a: ColorU, b: ColorU) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn relative_luminance(color: ColorU) -> f32 {
    fn linear(channel: u8) -> f32 {
        let value = channel as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::{contrast_ratio, ensure_contrast, EditorPalette, MIN_CODE_CONTRAST};
    use crate::theme::Theme;
    use dragent_editor::TokenKind;
    use warpui::color::ColorU;

    #[test]
    fn dark_theme_tokens_are_lifted_to_readable_contrast() {
        let background = ColorU::new(0, 0, 0, 255);
        let adjusted = ensure_contrast(ColorU::new(30, 34, 38, 255), background, 4.5);

        assert!(contrast_ratio(adjusted, background) >= 4.5);
        assert!(adjusted.r > 30 && adjusted.g > 34 && adjusted.b > 38);
    }

    #[test]
    fn light_theme_tokens_are_darkened_to_readable_contrast() {
        let background = ColorU::new(250, 250, 250, 255);
        let adjusted = ensure_contrast(ColorU::new(235, 220, 80, 255), background, 4.5);

        assert!(contrast_ratio(adjusted, background) >= 4.5);
        assert!(adjusted.r < 235 && adjusted.g < 220);
    }

    #[test]
    fn every_semantic_token_meets_the_editor_contrast_floor() {
        for info in Theme::all_infos() {
            let theme = Theme::from_id(info.id);
            let palette = EditorPalette::from_theme(&theme);
            let background = ColorU::new(
                theme.background.r,
                theme.background.g,
                theme.background.b,
                255,
            );
            assert!(contrast_ratio(palette.plain(), background) >= MIN_CODE_CONTRAST);
            for kind in [
                TokenKind::Comment,
                TokenKind::Keyword,
                TokenKind::Function,
                TokenKind::Type,
                TokenKind::String,
                TokenKind::Number,
                TokenKind::Constant,
                TokenKind::Operator,
                TokenKind::Property,
                TokenKind::Macro,
                TokenKind::Delimiter,
            ] {
                assert!(
                    contrast_ratio(palette.token(kind), background) >= MIN_CODE_CONTRAST,
                    "{kind:?} is not readable in {}",
                    theme.id
                );
            }
        }
    }
}
