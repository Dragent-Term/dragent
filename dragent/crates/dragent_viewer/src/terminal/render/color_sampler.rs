//! Infer terminal background colors for AI overlay chrome.
//!
//! Samples the active emulator grid (alt or primary) and picks the dominant
//! background near the cursor and bottom rows so the rich-input panel blends
//! with Codex / Claude Code TUIs instead of using fixed theme surfaces.

use std::collections::HashMap;

use dragent_terminal::{Color as TermColor, Emulator, StyledLine};
use warpui::color::ColorU;

use crate::theme::Theme;

/// Derived UI colors for the AI input panel, matched to the live TUI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiChromeColors {
    pub panel_bg: ColorU,
    pub chip_bg: ColorU,
    pub chip_active_bg: ColorU,
    pub border: ColorU,
}

/// Sample the active screen and build panel/chip colors.
pub fn sample_ai_chrome(emu: &Emulator, theme: &Theme) -> AiChromeColors {
    let lines = active_screen_lines(emu);
    let (cursor_row, _) = emu.cursor_position();
    // Follow the app only when it painted the whole screen. Sampling anything
    // less lets a highlighted row, or a large selection, recolour the terminal
    // background and the input panel with it.
    let base = if app_paints_whole_screen(emu) {
        dominant_background(&lines, cursor_row, theme)
    } else {
        theme.background
    };
    let mut chrome = derive_chrome(base, theme);
    // For a background-image theme the AI input panel and alt-screen canvas must
    // stack NO tint of their own: the image and its single tint are painted once
    // at the workspace level behind every pane, so `pane_surface()` here is fully
    // transparent and reveals that one uniform tint — the same overlay the normal
    // terminal output shows, so alt-screen and normal never diverge. (Chips/borders
    // derive via `blend`, which forces full alpha, so they stay solid.)
    if theme.background.a < 255 {
        chrome.panel_bg = theme.pane_surface();
    }
    chrome
}

/// Whether the app painted its own background over (nearly) the whole grid.
fn app_paints_whole_screen(emu: &Emulator) -> bool {
    let (rows, cols) = emu.dimensions();
    let painted: usize = active_screen_lines(emu)
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| !matches!(span.style.bg, TermColor::Default))
        .map(|span| span.text.chars().count())
        .sum();
    painted > 0 && painted * 20 >= rows * cols * 19
}

fn active_screen_lines(emu: &Emulator) -> Vec<StyledLine> {
    if emu.alt_screen_active() {
        emu.alt_screen_all_lines()
    } else {
        emu.grid().screen_styled_lines()
    }
}

fn dominant_background(lines: &[StyledLine], cursor_row: usize, theme: &Theme) -> ColorU {
    if lines.is_empty() {
        return theme.background;
    }

    let mut histogram: HashMap<u32, u32> = HashMap::new();
    let n = lines.len();
    let bottom_band_start = n.saturating_sub((n / 4).max(3));

    for (row_idx, line) in lines.iter().enumerate() {
        let row_weight = if row_idx == cursor_row {
            4
        } else if row_idx >= bottom_band_start {
            3
        } else if line.is_blank() {
            0
        } else {
            1
        };
        if row_weight == 0 {
            continue;
        }
        accumulate_line_bg(line, row_weight, theme, &mut histogram);
    }

    if histogram.is_empty() {
        for (row_idx, line) in lines.iter().enumerate() {
            if row_idx < bottom_band_start && row_idx != cursor_row {
                continue;
            }
            accumulate_line_bg(line, 1, theme, &mut histogram);
        }
    }

    histogram
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(packed, _)| unpack_rgb(packed))
        .unwrap_or(theme.background)
}

fn accumulate_line_bg(
    line: &StyledLine,
    row_weight: u32,
    theme: &Theme,
    histogram: &mut HashMap<u32, u32>,
) {
    for span in &line.spans {
        let chars = span.text.chars().filter(|c| !c.is_control()).count().max(1) as u32;
        let bg = resolve_term_bg(span.style.bg, theme);
        *histogram.entry(pack_rgb(bg)).or_default() += chars * row_weight;
    }
}

fn derive_chrome(base: ColorU, _theme: &Theme) -> AiChromeColors {
    let fg_hint = if luminance(base) > 140 {
        ColorU::new(20, 20, 20, 255)
    } else {
        ColorU::new(240, 240, 240, 255)
    };
    AiChromeColors {
        panel_bg: base,
        chip_bg: blend(base, fg_hint, 0.10),
        chip_active_bg: blend(base, fg_hint, 0.18),
        border: with_alpha(fg_hint, 0.14),
    }
}

fn resolve_term_bg(color: TermColor, theme: &Theme) -> ColorU {
    match color {
        TermColor::Default => theme.background,
        TermColor::Rgb(r, g, b) => ColorU::new(r, g, b, 255),
        TermColor::Indexed(idx) => indexed_color(idx, theme),
        TermColor::Named(n) => indexed_color(n.index(), theme),
    }
}

fn indexed_color(idx: u8, theme: &Theme) -> ColorU {
    match idx {
        0 => ColorU::new(30, 30, 30, 255),
        1 | 9 => theme.ansi_red(),
        2 | 10 => theme.ansi_green(),
        3 | 11 => theme.ui_yellow(),
        4 | 12 => theme.ansi_blue(),
        5 | 13 => ColorU::new(255, 156, 242, 255),
        6 | 14 => ColorU::new(136, 225, 225, 255),
        7 => ColorU::new(204, 204, 204, 255),
        8 => ColorU::new(128, 128, 128, 255),
        15 => ColorU::new(255, 255, 255, 255),
        16..=231 => {
            let v = idx - 16;
            let scale = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
            ColorU::new(scale(v / 36), scale((v % 36) / 6), scale(v % 6), 255)
        }
        232..=255 => {
            let level = 8 + (idx - 232) * 10;
            ColorU::new(level, level, level, 255)
        }
    }
}

fn pack_rgb(c: ColorU) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

fn unpack_rgb(packed: u32) -> ColorU {
    ColorU::new(
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
        255,
    )
}

fn luminance(c: ColorU) -> u16 {
    // Widen to u32 for the multiply: a single term alone (255 * 587 = 149685)
    // already exceeds u16::MAX, so any sufficiently bright color overflowed
    // here — panicking in debug builds, silently wrapping to a wrong value in
    // release. The final result is always <= 255, so u16 still fits as the
    // return type; only the intermediate sum needed the wider type.
    ((u32::from(c.r) * 299 + u32::from(c.g) * 587 + u32::from(c.b) * 114) / 1000) as u16
}

fn blend(bg: ColorU, fg: ColorU, ratio: f32) -> ColorU {
    let t = ratio.clamp(0.0, 1.0);
    ColorU::new(
        lerp(bg.r, fg.r, t),
        lerp(bg.g, fg.g, t),
        lerp(bg.b, fg.b, t),
        255,
    )
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (255.0 * alpha.clamp(0.0, 1.0)) as u8,
    )
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_terminal::{Flags, Style, StyledSpan};

    fn line_with_bg(text: &str, bg: TermColor) -> StyledLine {
        StyledLine {
            spans: vec![StyledSpan {
                text: text.to_string(),
                style: Style {
                    fg: TermColor::Default,
                    bg,
                    flags: Flags::empty(),
                    underline_color: None,
                },
            }],
            wrapped: false,
        }
    }

    #[test]
    fn picks_weighted_bottom_row_background() {
        let theme = Theme::dark();
        let lines = vec![
            line_with_bg("header", TermColor::Rgb(10, 10, 10)),
            line_with_bg("body", TermColor::Rgb(20, 20, 20)),
            line_with_bg("input area    ", TermColor::Rgb(40, 60, 90)),
        ];
        let picked = dominant_background(&lines, 2, &theme);
        assert_eq!(picked, ColorU::new(40, 60, 90, 255));
    }

    #[test]
    fn only_a_fully_painted_screen_counts_as_painted() {
        let mut emu = Emulator::new(24, 80);
        // One highlighted row, as in an agent's input box.
        emu.feed(b"\x1b[?1049h\x1b[48;2;40;60;90m");
        emu.feed("x".repeat(80).as_bytes());
        assert!(!app_paints_whole_screen(&emu));

        emu.feed("x".repeat(80 * 23).as_bytes());
        assert!(app_paints_whole_screen(&emu));
    }

    #[test]
    fn default_bg_resolves_to_theme_background() {
        let theme = Theme::dark();
        let lines = vec![line_with_bg("   ", TermColor::Default)];
        let picked = dominant_background(&lines, 0, &theme);
        assert_eq!(picked, theme.background);
    }

    #[test]
    fn derive_chrome_lightens_chips_on_dark_base() {
        let theme = Theme::dark();
        let base = ColorU::new(30, 40, 55, 255);
        let chrome = derive_chrome(base, &theme);
        assert_eq!(chrome.panel_bg, base);
        assert!(chrome.chip_bg.r > base.r);
    }

    #[test]
    fn luminance_of_bright_colors_does_not_overflow() {
        // THE bug: a single term alone (255 * 587 = 149685) exceeds u16::MAX,
        // so any bright color — exactly what the light theme's near-white
        // background is — panicked here in debug builds ("attempt to multiply
        // with overflow") when selecting Settings → Appearance → Light.
        assert_eq!(luminance(ColorU::new(255, 255, 255, 255)), 255);
        assert_eq!(luminance(ColorU::new(0, 255, 0, 255)), 149);
        assert_eq!(luminance(ColorU::new(0, 0, 0, 255)), 0);
    }
}
