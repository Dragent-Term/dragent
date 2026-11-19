//! Cell grid rendering for alt-screen and block output.

use dragent_terminal::StyledLine;
use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};

use warpui_core::scene::Border;

use crate::console::editor_settings::{CursorStyle, EditorSettings};
use crate::console::escape_parse::{styled_to_colored_exact, ColoredLine};
use crate::terminal::model::selection::CellSelection;
use crate::theme::Theme;

/// Top padding inside the alt-screen grid (must match mouse hit testing).
pub const ALT_GRID_PADDING_TOP: f32 = 8.0;


/// Parameters for building an alt-screen column.
pub struct GridLineParams {
    pub theme: Theme,
    pub mono_family: FamilyId,
    pub padding_left: f32,
    /// Monospace font size to render each cell at. Normally [`mono_font_size`],
    /// but a shared-session mirror passes a scaled size so the host's fixed grid
    /// (rows×cols) fits the viewer's viewport — screen-share style — rather than
    /// clipping when the two windows differ in size.
    pub font_size: f32,
}

/// Build a vertical column of monospace rows for the alt-screen grid from the
/// terminal's own `StyledLine`s (ANSI colors are resolved against the theme).
pub fn build_alt_screen_column(
    lines: &[StyledLine],
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    selection: Option<&CellSelection>,
    params: GridLineParams,
) -> Box<dyn Element> {
    let colored: Vec<ColoredLine> = lines
        .iter()
        .map(|line| styled_to_colored_exact(line, &params.theme))
        .collect();
    build_alt_screen_column_colored(
        &colored,
        cursor_row,
        cursor_col,
        cursor_visible,
        selection,
        params,
    )
}

/// Build the alt-screen column from already-colored lines. A shared-session
/// mirror uses this with the host's broadcast snapshot (colors are pre-resolved
/// on the host), so it paints the exact cells the host drew — no re-parsing.
pub fn build_alt_screen_column_colored(
    lines: &[ColoredLine],
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    selection: Option<&CellSelection>,
    params: GridLineParams,
) -> Box<dyn Element> {
    let GridLineParams {
        theme,
        mono_family,
        padding_left,
        font_size,
    } = params;

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min);

    for (row_idx, line) in lines.iter().enumerate() {
        let cursor = if cursor_visible && row_idx == cursor_row {
            Some(cursor_col)
        } else {
            None
        };
        col = col.with_child(build_row_element(
            line,
            row_idx,
            cursor,
            selection,
            theme,
            mono_family,
            font_size,
        ));
    }

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_child(
                Container::new(col.finish())
                    .with_padding_left(padding_left)
                    .with_padding_right(padding_left)
                    .with_padding_top(ALT_GRID_PADDING_TOP)
                    .with_padding_bottom(4.0)
                    .with_background_color(theme.background)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(theme.background)
    .finish()
}

fn build_row_element(
    colored: &ColoredLine,
    row_idx: usize,
    cursor_col: Option<usize>,
    selection: Option<&CellSelection>,
    theme: Theme,
    mono: FamilyId,
    font_size: f32,
) -> Box<dyn Element> {
    let default_fg = theme.main_text();
    let term_bg = theme.background;
    let selection_bg = ColorU::new(25, 90, 120, 180);
    let cursor_col = cursor_col.unwrap_or(usize::MAX);

    struct Seg {
        text: String,
        fg: ColorU,
        bg: Option<ColorU>,
        /// Some = this single-cell segment carries the cursor, drawn in the
        /// user's configured style. Never merged with neighbors.
        cursor: Option<CursorStyle>,
    }

    let cursor_style = EditorSettings::global().terminal.cursor_style;
    let mut segs: Vec<Seg> = Vec::new();
    let mut col = 0usize;
    let mut cursor_placed = false;

    let push = |segs: &mut Vec<Seg>, text: &str, fg: ColorU, bg: Option<ColorU>| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = segs.last_mut() {
            if last.fg == fg && last.bg == bg && last.cursor.is_none() {
                last.text.push_str(text);
                return;
            }
        }
        segs.push(Seg {
            text: text.to_string(),
            fg,
            bg,
            cursor: None,
        });
    };
    let push_cursor = |segs: &mut Vec<Seg>, text: &str, fg: ColorU, bg: Option<ColorU>| {
        segs.push(Seg {
            text: text.to_string(),
            fg,
            bg,
            cursor: Some(cursor_style),
        });
    };

    for span in &colored.spans {
        let span_fg = span.foreground(default_fg);
        for ch in span.text.chars() {
            let w = char_display_width(ch);
            if w == 0 {
                continue;
            }

            if !cursor_placed && col == cursor_col {
                match cursor_style {
                    CursorStyle::Block => {
                        // Full inverted cell.
                        push_cursor(&mut segs, &ch.to_string(), term_bg, Some(theme.accent));
                    }
                    CursorStyle::Bar | CursorStyle::Underline => {
                        // Keep the glyph's own colors; the accent bar/underline
                        // is drawn as a border on the segment container.
                        push_cursor(&mut segs, &ch.to_string(), span_fg, span.bg);
                    }
                }
                cursor_placed = true;
            } else if selection
                .map(|sel| sel.contains_cell(row_idx, col))
                .unwrap_or(false)
            {
                push(&mut segs, &ch.to_string(), span_fg, Some(selection_bg));
            } else {
                push(&mut segs, &ch.to_string(), span_fg, span.bg);
            }
            col += w;
        }
    }

    // Cursor past end-of-line (common in vim normal mode).
    if !cursor_placed && cursor_col != usize::MAX {
        while col < cursor_col {
            if selection
                .map(|sel| sel.contains_cell(row_idx, col))
                .unwrap_or(false)
            {
                push(&mut segs, " ", default_fg, Some(selection_bg));
            } else {
                push(&mut segs, " ", default_fg, None);
            }
            col += 1;
        }
        if col == cursor_col {
            match cursor_style {
                CursorStyle::Block => push_cursor(&mut segs, " ", term_bg, Some(theme.accent)),
                CursorStyle::Bar | CursorStyle::Underline => {
                    push_cursor(&mut segs, " ", default_fg, None)
                }
            }
        }
    }

    if segs.is_empty() {
        segs.push(Seg {
            text: " ".to_string(),
            fg: default_fg,
            bg: None,
            cursor: None,
        });
    }

    let mut row = Flex::row();
    for seg in segs {
        if seg.text.is_empty() {
            continue;
        }
        let text_el = Box::new(
            Text::new(seg.text.clone(), mono, font_size)
                .with_color(seg.fg)
                .with_style(Properties::default().weight(Weight::Normal)),
        );
        let el: Box<dyn Element> = match seg.cursor {
            Some(CursorStyle::Bar) => {
                let c = Container::new(text_el as Box<dyn Element>)
                    .with_border(Border::left(2.0).with_border_color(theme.accent));
                let c = if let Some(bg) = seg.bg {
                    c.with_background_color(bg)
                } else {
                    c
                };
                c.finish()
            }
            Some(CursorStyle::Underline) => {
                let c = Container::new(text_el as Box<dyn Element>)
                    .with_border(Border::bottom(2.0).with_border_color(theme.accent));
                let c = if let Some(bg) = seg.bg {
                    c.with_background_color(bg)
                } else {
                    c
                };
                c.finish()
            }
            _ => {
                if let Some(bg) = seg.bg {
                    Container::new(text_el as Box<dyn Element>)
                        .with_background_color(bg)
                        .finish()
                } else {
                    text_el
                }
            }
        };
        row = row.with_child(el);
    }

    row.finish()
}

fn char_display_width(c: char) -> usize {
    match c as u32 {
        0x00..=0x1f | 0x7f..=0x9f => 0,
        0x0300..=0x036f | 0x1dc0..=0x1dff | 0x20d0..=0x20ff | 0xfe00..=0xfe0f => 0,
        0x1100..=0x115f
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x20000..=0x2fffd
        | 0x30000..=0x3fffd => 2,
        _ => 1,
    }
}
