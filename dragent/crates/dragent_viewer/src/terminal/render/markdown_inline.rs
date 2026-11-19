//! Markdown renderer using pulldown-cmark (MIT licensed).
//!
//! Converts LLM markdown output into a warpui element tree with proper
//! styling for headings, bold/italic inline spans, code blocks, lists,
//! and blockquotes.

use pulldown_cmark::{Event, Tag, TagEnd};
use warpui::elements::{
    ConstrainedBox, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize, ParentElement, Text,
};
use warpui::fonts::Weight;

use crate::terminal::render::markdown_agent::strip_emoji_str;
use crate::terminal::render::markdown_blocks::apply_document_block_rhythm;
use crate::terminal::render::markdown_blocks::render_block_with_style;
use crate::terminal::render::markdown_blocks::MarkdownStyle;
use crate::terminal::render::markdown_tables::build_text_with_char_styles;
use crate::theme::Theme;

// ── Public API ────────────────────────────────────────────────────────────────

/// Render markdown text into a warpui column of styled elements.

/// Style flags for a single character. Using saturating depth counters means
/// nested tags (e.g. bold inside bold) don't corrupt state on the End tag.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CharStyle {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) code: bool,
    pub(crate) strike: bool,
    pub(crate) link: bool,
    /// Inside a code span that names a file: its path, then its `:line`.
    pub(crate) file: bool,
    pub(crate) position: bool,
}

impl CharStyle {
    pub(crate) fn is_default(self) -> bool {
        !self.bold && !self.italic && !self.code && !self.strike && !self.link
    }
}

/// Per-character styles for a code span. One that names a file is painted in
/// the terminal's file colours, its `:line` suffix apart from the path.
pub(crate) fn code_span_styles(code: &str) -> impl Iterator<Item = CharStyle> + '_ {
    let split = crate::terminal::render::term_colors::file_reference(code);
    code.char_indices().map(move |(byte, _)| CharStyle {
        code: true,
        file: split.is_some_and(|split| byte < split),
        position: split.is_some_and(|split| byte >= split),
        ..Default::default()
    })
}

// ── Inline collector ──────────────────────────────────────────────────────────

pub(crate) fn is_end_paragraph(e: &Event<'_>) -> bool {
    matches!(e, Event::End(TagEnd::Paragraph))
}

pub(crate) fn is_end_blockquote(e: &Event<'_>) -> bool {
    matches!(e, Event::End(TagEnd::BlockQuote(..)))
}

pub(crate) fn is_end_item(e: &Event<'_>) -> bool {
    matches!(e, Event::End(TagEnd::Item))
}

/// Walk inline events from `start` until `is_end` returns true.
/// Returns (full_text, per_char_styles, events_consumed_including_end).
///
/// Per-char style flags guarantee non-overlapping output — each character has
/// exactly one style tuple regardless of nesting depth.
///
/// When no formatting events occur the returned `char_styles` vec is empty,
/// which lets the caller skip highlight allocation entirely (fast path).
pub(crate) fn collect_inline(
    events: &[Event<'_>],
    start: usize,
    is_end: fn(&Event<'_>) -> bool,
) -> (String, Vec<CharStyle>, usize) {
    let mut text = String::new();
    // Defer allocating char_styles until we actually see a formatting event.
    let mut char_styles: Vec<CharStyle> = Vec::new();
    let mut has_styling = false;

    let mut bold_depth: u32 = 0;
    let mut italic_depth: u32 = 0;
    let mut strike_depth: u32 = 0;
    let mut link_depth: u32 = 0;
    let mut consumed = 0;

    let mut j = start;
    while j < events.len() {
        let ev = &events[j];
        j += 1;
        consumed += 1;

        if is_end(ev) {
            break;
        }

        match ev {
            Event::Text(s) => {
                let clean = strip_emoji_str(s);
                if has_styling {
                    let style = CharStyle {
                        bold: bold_depth > 0,
                        italic: italic_depth > 0,
                        code: false,
                        strike: strike_depth > 0,
                        link: link_depth > 0,
                        ..Default::default()
                    };
                    for _ in clean.chars() {
                        char_styles.push(style);
                    }
                }
                text.push_str(&clean);
            }
            Event::Code(s) => {
                let clean = strip_emoji_str(s);
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                char_styles.extend(code_span_styles(&clean));
                text.push_str(&clean);
            }
            Event::SoftBreak => {
                if has_styling {
                    char_styles.push(CharStyle::default());
                }
                text.push(' ');
            }
            Event::HardBreak => {
                if has_styling {
                    char_styles.push(CharStyle::default());
                }
                text.push('\n');
            }
            Event::Start(Tag::Strong) => {
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                bold_depth += 1;
            }
            Event::End(TagEnd::Strong) => bold_depth = bold_depth.saturating_sub(1),
            Event::Start(Tag::Emphasis) => {
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                italic_depth += 1;
            }
            Event::End(TagEnd::Emphasis) => italic_depth = italic_depth.saturating_sub(1),
            Event::Start(Tag::Strikethrough) => {
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                strike_depth += 1;
            }
            Event::End(TagEnd::Strikethrough) => strike_depth = strike_depth.saturating_sub(1),
            Event::Start(Tag::Link { .. }) => {
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                link_depth += 1;
            }
            Event::End(TagEnd::Link) => link_depth = link_depth.saturating_sub(1),
            // Paragraph tags may appear inside list items (tight vs loose lists)
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => {}
            _ => {}
        }
    }

    (text, char_styles, consumed)
}

// ── Code block collector ──────────────────────────────────────────────────────

pub(crate) fn collect_code_block(events: &[Event<'_>], start: usize) -> (String, usize) {
    let mut code = String::new();
    let mut consumed = 0;
    let mut j = start;
    while j < events.len() {
        let ev = &events[j];
        j += 1;
        consumed += 1;
        match ev {
            Event::Text(s) => code.push_str(s),
            Event::End(TagEnd::CodeBlock) => break,
            _ => {}
        }
    }
    (code, consumed)
}

// ── List renderer ─────────────────────────────────────────────────────────────

pub(crate) fn render_list(
    events: &[Event<'_>],
    start: usize,
    ordered: bool,
    theme: Theme,
    style: MarkdownStyle,
) -> (Box<dyn Element>, usize) {
    if style.document {
        return render_document_list(events, start, ordered, theme, style);
    }
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(if style.document { 6.0 } else { 3.0 });

    let mut consumed = 0;
    let mut item_index: u32 = 1;
    let mut j = start;

    while j < events.len() {
        let ev = &events[j];
        j += 1;
        consumed += 1;

        match ev {
            Event::End(TagEnd::List(_)) => break,
            Event::Start(Tag::Item) => {
                let (item_text, item_char_styles, item_consumed) =
                    collect_inline(events, j, is_end_item);
                j += item_consumed;
                consumed += item_consumed;

                let bullet = if ordered {
                    format!("{item_index}. ")
                } else {
                    "• ".to_string()
                };
                let full_text = format!("{bullet}{item_text}");

                // Only build combined style vec when item_text has inline styling.
                let full_styles = if item_char_styles.is_empty() {
                    Vec::new()
                } else {
                    let mut v = vec![CharStyle::default(); bullet.chars().count()];
                    v.extend_from_slice(&item_char_styles);
                    v
                };

                let el = build_text_with_char_styles(
                    full_text,
                    full_styles,
                    style.body_size,
                    Weight::Normal,
                    theme.main_text(),
                    theme,
                    style.body_font,
                    style.line_height_ratio,
                );
                col = col.with_child(el);
                item_index += 1;
            }
            _ => {}
        }
    }

    (col.finish(), consumed)
}

pub(crate) fn item_end(events: &[Event<'_>], start: usize) -> usize {
    let mut depth = 1usize;
    for (offset, event) in events[start..].iter().enumerate() {
        match event {
            Event::Start(Tag::Item) => depth += 1,
            Event::End(TagEnd::Item) => {
                depth -= 1;
                if depth == 0 {
                    return start + offset;
                }
            }
            _ => {}
        }
    }
    events.len()
}

fn render_document_list(
    events: &[Event<'_>],
    start: usize,
    ordered: bool,
    theme: Theme,
    style: MarkdownStyle,
) -> (Box<dyn Element>, usize) {
    let mut list = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0);
    let mut cursor = start;
    let mut item_index = 1u32;

    while cursor < events.len() {
        match &events[cursor] {
            Event::End(TagEnd::List(_)) => {
                cursor += 1;
                break;
            }
            Event::Start(Tag::Item) => {
                let content_start = cursor + 1;
                let end = item_end(events, content_start);
                let content = render_document_item(events, content_start, end, theme, style);
                let marker = if ordered {
                    format!("{item_index}.")
                } else {
                    "•".to_string()
                };
                let marker = ConstrainedBox::new(
                    Text::new(marker, style.body_font, style.body_size)
                        .with_color(theme.sub_text())
                        .with_line_height_ratio(style.line_height_ratio)
                        .finish(),
                )
                .with_width(if ordered { 28.0 } else { 20.0 })
                .finish();
                list = list.with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_spacing(6.0)
                        .with_child(marker)
                        .with_child(Expanded::new(1.0, content).finish())
                        .finish(),
                );
                item_index += 1;
                cursor = end.saturating_add(1);
            }
            _ => cursor += 1,
        }
    }
    (list.finish(), cursor.saturating_sub(start))
}

fn render_document_item(
    events: &[Event<'_>],
    start: usize,
    end: usize,
    theme: Theme,
    style: MarkdownStyle,
) -> Box<dyn Element> {
    let has_blocks = events[start..end].iter().any(|event| {
        matches!(
            event,
            Event::Start(
                Tag::Paragraph
                    | Tag::CodeBlock(_)
                    | Tag::List(_)
                    | Tag::BlockQuote(_)
                    | Tag::Table(_)
            )
        )
    });
    if !has_blocks {
        let (text, styles, _) = collect_inline(events, start, is_end_item);
        return build_text_with_char_styles(
            text,
            styles,
            style.body_size,
            Weight::Normal,
            theme.main_text(),
            theme,
            style.body_font,
            style.line_height_ratio,
        );
    }

    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0);
    let mut cursor = start;
    while cursor < end {
        let event = events.get(cursor);
        let (element, consumed) = render_block_with_style(events, cursor, theme, style);
        cursor += consumed.max(1);
        if let Some(element) = element {
            content = content.with_child(apply_document_block_rhythm(element, event, style));
        }
    }
    content.finish()
}

// ── Table renderer ────────────────────────────────────────────────────────────
