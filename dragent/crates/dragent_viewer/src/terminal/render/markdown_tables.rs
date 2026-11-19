//! Markdown renderer using pulldown-cmark (MIT licensed).
//!
//! Converts LLM markdown output into a warpui element tree with proper
//! styling for headings, bold/italic inline spans, code blocks, lists,
//! and blockquotes.

use dragent_editor::{Language, LexicalHighlighter, TokenKind};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Tag, TagEnd};
use std::path::Path;
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, MainAxisSize, ParentElement,
    Text,
};
use warpui::fonts::{FamilyId, Properties, Style, Weight};
use warpui_core::elements::{Highlight, HighlightedRange};
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::TextStyle;

use crate::terminal::render::markdown_blocks::MarkdownStyle;
use crate::terminal::render::markdown_inline::collect_inline;
use crate::terminal::render::markdown_inline::CharStyle;
use crate::theme::Theme;

// ── Public API ────────────────────────────────────────────────────────────────

/// A table's rows from its events (after `Start(Table)`): whether each is
/// the header, and its cells' text and styles. Returns how many events the
/// table took.
pub(crate) fn table_rows(events: &[Event<'_>], start: usize) -> (Vec<TableRow>, usize) {
    let mut rows: Vec<(bool, Vec<(String, Vec<CharStyle>)>)> = Vec::new(); // (is_header, cells)
    let mut consumed = 0;
    let mut j = start;
    let mut in_header = false;
    let mut current_row: Option<(bool, Vec<(String, Vec<CharStyle>)>)> = None;

    while j < events.len() {
        let ev = &events[j];
        j += 1;
        consumed += 1;

        match ev {
            Event::End(TagEnd::Table) => break,

            // The head's cells come straight inside it, with no row around
            // them: it is the header row itself.
            Event::Start(Tag::TableHead) => {
                in_header = true;
                current_row = Some((true, Vec::new()));
            }
            Event::End(TagEnd::TableHead) => {
                in_header = false;
                if let Some(row) = current_row.take().filter(|(_, cells)| !cells.is_empty()) {
                    rows.push(row);
                }
            }

            Event::Start(Tag::TableRow) => {
                // (A head that wraps its cells in a row still makes one row.)
                if !(in_header && current_row.is_some()) {
                    current_row = Some((in_header, Vec::new()));
                }
            }
            Event::End(TagEnd::TableRow) => {
                if !in_header {
                    if let Some(row) = current_row.take() {
                        rows.push(row);
                    }
                }
            }

            Event::Start(Tag::TableCell) => {
                let (cell_text, cell_char_styles, cell_consumed) =
                    collect_inline(events, j, |e| matches!(e, Event::End(TagEnd::TableCell)));
                j += cell_consumed;
                consumed += cell_consumed;
                if let Some((_, ref mut cells)) = current_row {
                    cells.push((cell_text, cell_char_styles));
                }
            }

            _ => {}
        }
    }
    (rows, consumed)
}

/// One table row: whether it is the header, and its cells' text and styles.
pub(crate) type TableRow = (bool, Vec<(String, Vec<CharStyle>)>);

/// Render markdown text into a warpui column of styled elements.

pub(crate) fn render_table(
    events: &[Event<'_>],
    start: usize,
    _alignments: &[Alignment],
    theme: Theme,
    style: MarkdownStyle,
) -> (Box<dyn Element>, usize) {
    let (mut rows, consumed) = table_rows(events, start);

    // First, find the maximum number of columns across all rows
    let mut max_cols = 0;
    for (_, cells) in &rows {
        max_cols = max_cols.max(cells.len());
    }

    for (_, cells) in rows.iter_mut() {
        while cells.len() < max_cols {
            cells.push((String::new(), Vec::new()));
        }
    }

    // Now, calculate the maximum character length for each column
    let mut col_max_chars = vec![0; max_cols];
    for (_, cells) in &rows {
        for (col_idx, (cell_text, _)) in cells.iter().enumerate() {
            col_max_chars[col_idx] = col_max_chars[col_idx].max(cell_text.chars().count());
        }
    }

    // These values are flex weights, not hard maximums. Keep compact columns
    // wide enough for labels while capping prose-heavy columns so one long
    // cell cannot squeeze every sibling into character-by-character wrapping.
    let col_widths: Vec<f32> = col_max_chars
        .iter()
        .map(|&chars| table_column_weight(chars, style.document))
        .collect();
    // Short label columns keep their natural width so their words never
    // break; only the prose columns share what's left. Weights alone gave a
    // label column a sliver beside two long ones ("Ho/w/it/co/nn…").
    let fixed = label_widths(&col_max_chars, style.document);

    // Build the column of rows
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0);

    let row_count = rows.len();
    for (row_idx, (is_header, cells)) in rows.into_iter().enumerate() {
        let weight = if is_header {
            Weight::Bold
        } else {
            Weight::Normal
        };
        let color = if is_header {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let is_last = row_idx == row_count - 1;

        let mut row_flex = Flex::row()
            // Stretch every cell to the tallest cell in this row. Otherwise a
            // cell's right border ends at its own text height, leaving broken
            // vertical dividers whenever a neighboring cell wraps.
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(0.0);

        let cell_count = cells.len();
        for (col_idx, (cell_text, cell_char_styles)) in cells.into_iter().enumerate() {
            let text_el = build_text_with_char_styles(
                cell_text,
                cell_char_styles,
                style.body_size,
                weight,
                color,
                theme,
                style.body_font,
                style.line_height_ratio,
            );
            let _ = cell_count;
            let cell_builder = Container::new(text_el)
                .with_padding_left(if style.document { 12.0 } else { 8.0 })
                .with_padding_right(if style.document { 12.0 } else { 8.0 })
                .with_padding_top(if style.document { 8.0 } else { 5.0 })
                .with_padding_bottom(if style.document { 8.0 } else { 5.0 });
            let cell_el = cell_builder.finish();
            let cell_width = col_widths.get(col_idx).copied().unwrap_or(96.0);
            // Expanded supplies a finite allocated width. Do not cap the child
            // to the weight again: that was why a visually wide cell still
            // wrapped "Done" and "Viewport" into multiple lines.
            row_flex = match fixed.get(col_idx).copied().flatten() {
                Some(width) => row_flex
                    .with_child(ConstrainedBox::new(cell_el).with_width(width).finish()),
                None => row_flex.with_child(Box::new(Expanded::new(cell_width, cell_el))),
            };
        }

        let mut row_builder = Container::new(row_flex.finish());
        if is_header {
            row_builder = row_builder.with_background_color(theme.surface_1());
        }
        if !is_last {
            row_builder =
                row_builder.with_border(Border::bottom(1.0).with_border_color(theme.outline()));
        }
        let row_el = row_builder.finish();
        col = col.with_child(row_el);
    }

    let table_el = Container::new(col.finish())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(if style.document {
            7.0
        } else {
            4.0
        })))
        .finish();

    (table_el, consumed)
}

/// The widest a column may be and still count as a label column.
const LABEL_COLUMN_MAX: f32 = 240.0;

/// Each column's fixed width when it's a short label column (its longest
/// cell fits on one line within [`LABEL_COLUMN_MAX`]), else None: it
/// shares the remaining width. At least one column always flexes, so the
/// table still fills its space.
pub(crate) fn label_widths(col_max_chars: &[usize], document: bool) -> Vec<Option<f32>> {
    // A little over the body's average glyph, so bold headers fit too.
    let char_width = crate::theme::scaled(if document { 8.2 } else { 9.0 });
    let padding = if document { 24.0 } else { 16.0 };
    let mut widths: Vec<Option<f32>> = col_max_chars
        .iter()
        .map(|&chars| {
            let width = chars.max(3) as f32 * char_width + padding;
            (width <= crate::theme::scaled(LABEL_COLUMN_MAX)).then_some(width)
        })
        .collect();
    if widths.iter().all(Option::is_some) {
        // All short: the last column takes up the slack.
        if let Some(last) = widths.last_mut() {
            *last = None;
        }
    }
    widths
}

pub(crate) fn table_column_weight(max_chars: usize, document: bool) -> f32 {
    let estimated_char_width = if document { 7.4 } else { 8.5 };
    let horizontal_padding = if document { 24.0 } else { 16.0 };
    let minimum = if document { 112.0 } else { 72.0 };
    // High enough that a prose column outweighs label columns by far: the
    // labels keep their natural width and the prose gets the rest.
    let maximum = if document { 900.0 } else { 900.0 };
    ((max_chars as f32) * crate::theme::scaled(estimated_char_width) + horizontal_padding)
        .clamp(minimum, maximum)
}

// ── Element builders ──────────────────────────────────────────────────────────

pub(crate) fn render_code_block_styled(
    code: &str,
    fence_tag: Option<&str>,
    language: Language,
    theme: Theme,
    style: MarkdownStyle,
) -> Box<dyn Element> {
    // The fence's grammar colors the block when it is compiled in (any
    // language arborium knows, not just `Language`'s handful); the lexical
    // scanner is only the fallback for unknown fences.
    let syntax = fence_tag
        .and_then(dragent_editor::grammar_for_fence)
        .and_then(|grammar| dragent_editor::highlight_document_cached(grammar, code));
    let code_size = crate::theme::scaled_mono(if style.document { 13.0 } else { 12.5 });
    let properties = Properties::default();
    let mut highlighted = Text::new("", style.code_font, code_size)
        .with_color(theme.main_text())
        .with_line_height_ratio(if style.document {
            style.line_height_ratio.max(1.45)
        } else {
            1.25
        });
    let mut comment_state = dragent_editor::HighlightState::default();
    let mut lines = code.split('\n').enumerate().peekable();
    while let Some((line_index, line)) = lines.next() {
        let mut cursor = 0usize;
        let spans = match syntax.as_ref().and_then(|doc| doc.lines.get(line_index)) {
            Some(spans) => spans.clone(),
            None => {
                LexicalHighlighter::highlight_line_with_state(language, line, &mut comment_state)
            }
        };
        for span in spans {
            if span.range.start < cursor || span.range.end > line.len() {
                continue;
            }
            if span.range.start > cursor {
                highlighted.add_text_with_highlights(
                    &line[cursor..span.range.start],
                    theme.main_text(),
                    properties,
                );
            }
            highlighted.add_text_with_highlights(
                &line[span.range.clone()],
                markdown_token_color(span.kind, &theme),
                properties,
            );
            cursor = span.range.end;
        }
        if cursor < line.len() {
            highlighted.add_text_with_highlights(&line[cursor..], theme.main_text(), properties);
        }
        if lines.peek().is_some() {
            highlighted.add_text_with_highlights("\n", theme.main_text(), properties);
        }
    }
    Container::new(highlighted.finish())
        .with_padding_left(if style.document { 16.0 } else { 12.0 })
        .with_padding_right(if style.document { 16.0 } else { 12.0 })
        .with_padding_top(if style.document { 13.0 } else { 8.0 })
        .with_padding_bottom(if style.document { 13.0 } else { 8.0 })
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(if style.document {
            8.0
        } else {
            4.0
        })))
        .finish()
}

/// A fenced block's info-string tag (` ```tsx title=x ` → `tsx`).
pub(crate) fn markdown_code_tag<'a>(kind: &'a CodeBlockKind<'_>) -> Option<&'a str> {
    let CodeBlockKind::Fenced(info) = kind else {
        return None;
    };
    info.split_whitespace().next().filter(|tag| !tag.is_empty())
}

pub(crate) fn markdown_code_language(kind: &CodeBlockKind<'_>) -> Language {
    let CodeBlockKind::Fenced(info) = kind else {
        return Language::PlainText;
    };
    let language = info.split_whitespace().next().unwrap_or_default();
    match language.to_ascii_lowercase().as_str() {
        "rs" | "rust" => Language::Rust,
        "ts" | "tsx" | "typescript" => Language::TypeScript,
        "js" | "jsx" | "javascript" => Language::JavaScript,
        "py" | "python" => Language::Python,
        "go" | "golang" => Language::Go,
        "json" | "jsonc" | "json5" => Language::Json,
        "html" | "htm" => Language::Html,
        "css" | "scss" | "sass" | "less" => Language::Css,
        "sh" | "bash" | "zsh" | "shell" => Language::Shell,
        "yaml" | "yml" => Language::Yaml,
        "toml" => Language::Toml,
        "md" | "markdown" => Language::Markdown,
        other => Language::from_path(Path::new(&format!("preview.{other}"))),
    }
}

fn markdown_token_color(kind: TokenKind, theme: &Theme) -> ColorU {
    match kind {
        TokenKind::Comment => theme.ansi_normal[2],
        TokenKind::Keyword => theme.ansi_normal[4],
        TokenKind::Function => theme.ansi_bright[3],
        TokenKind::Type => theme.ansi_normal[6],
        TokenKind::String | TokenKind::Number => theme.ansi_bright[2],
        TokenKind::Constant | TokenKind::Delimiter => theme.ansi_bright[3],
        TokenKind::Property => theme.ansi_bright[4],
        TokenKind::Macro => theme.ansi_bright[5],
        TokenKind::Operator => theme.main_text(),
    }
}

/// Build a `Text` element from pre-computed per-character style flags.
///
/// Fast path: when `char_styles` is empty (no inline formatting detected)
/// the element is created directly without any highlight allocation.
///
/// Styled path: consecutive characters with the same `CharStyle` are grouped
/// into sorted, non-overlapping `HighlightedRange`s for `Text::with_highlights`.
pub(crate) fn build_text_with_char_styles(
    text: String,
    char_styles: Vec<CharStyle>,
    font_size: f32,
    global_weight: Weight,
    color: ColorU,
    theme: Theme,
    mono: FamilyId,
    line_height_ratio: f32,
) -> Box<dyn Element> {
    if text.is_empty() {
        return Box::new(Empty::new());
    }

    let base = Text::new(text, mono, font_size)
        .with_color(color)
        .with_line_height_ratio(line_height_ratio)
        .with_style(Properties {
            weight: global_weight,
            style: Style::Normal,
        })
        .with_selectable(true);

    if char_styles.is_empty() {
        return base.finish();
    }

    let highlights = char_styles_to_highlights(&char_styles, color, theme);
    base.with_highlights(highlights).finish()
}

fn char_styles_to_highlights(
    char_styles: &[CharStyle],
    default_color: ColorU,
    theme: Theme,
) -> Vec<HighlightedRange> {
    let mut out: Vec<HighlightedRange> = Vec::new();

    let mut i = 0;
    while i < char_styles.len() {
        let style = char_styles[i];
        if style.is_default() {
            i += 1;
            continue;
        }
        let run_start = i;
        while i < char_styles.len() && char_styles[i] == style {
            i += 1;
        }
        let run_end = i;
        let indices: Vec<usize> = (run_start..run_end).collect();
        let highlight = make_highlight(style, default_color, theme);
        out.push(HighlightedRange {
            highlight,
            highlight_indices: indices,
        });
    }

    out
}

fn make_highlight(style: CharStyle, default_color: ColorU, theme: Theme) -> Highlight {
    if style.link {
        return Highlight::new()
            .with_foreground_color(theme.accent)
            .with_text_style(TextStyle::new().with_foreground_color(theme.accent));
    }
    if style.code {
        let code_background = ColorU::new(default_color.r, default_color.g, default_color.b, 20);
        // A file in a code span takes the terminal's file colours.
        let color = if style.file {
            crate::terminal::render::term_colors::file_color(&theme)
        } else if style.position {
            crate::terminal::render::term_colors::position_color(&theme)
        } else {
            default_color
        };
        return Highlight::new()
            .with_foreground_color(color)
            .with_text_style(
                TextStyle::new()
                    .with_foreground_color(color)
                    .with_background_color(code_background),
            );
    }
    let weight = if style.bold {
        Weight::Bold
    } else {
        Weight::Normal
    };
    let font_style = if style.italic {
        Style::Italic
    } else {
        Style::Normal
    };
    let mut h = Highlight::new().with_properties(Properties {
        weight,
        style: font_style,
    });
    if style.strike {
        h = h.with_text_style(TextStyle::new().with_show_strikethrough(true));
    }
    h
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use pulldown_cmark::{Options, Parser};

    fn rows_of(markdown: &str) -> Vec<TableRow> {
        let events: Vec<Event<'_>> = Parser::new_ext(markdown, Options::ENABLE_TABLES).collect();
        let start = events.iter().position(|e| matches!(e, Event::Start(Tag::Table(_)))).unwrap() + 1;
        table_rows(&events, start).0
    }

    #[test]
    fn short_label_columns_keep_their_width_beside_prose() {
        // "Browser sign-in" beside two long prose columns.
        let widths = label_widths(&[15, 110, 120], true);
        assert!(widths[0].is_some_and(|w| w >= 15.0 * 7.4 + 24.0), "{widths:?}");
        assert_eq!(widths[1], None);
        assert_eq!(widths[2], None);
        // All short: the last one still fills the row.
        let widths = label_widths(&[6, 5], false);
        assert!(widths[0].is_some());
        assert_eq!(widths[1], None);
    }

    #[test]
    fn the_header_row_is_kept() {
        let rows = rows_of("| Agent | Same? |\n|---|---|\n| OpenCode | Yes |\n| Claude | No |\n");
        assert_eq!(rows.len(), 3, "header + 2 rows");
        assert!(rows[0].0, "first row is the header");
        assert_eq!(rows[0].1[0].0, "Agent");
        assert_eq!(rows[1].1[0].0, "OpenCode");
        assert!(!rows[1].0);
    }
}
