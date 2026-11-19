//! Markdown renderer using pulldown-cmark (MIT licensed).
//!
//! Converts LLM markdown output into a warpui element tree with proper
//! styling for headings, bold/italic inline spans, code blocks, lists,
//! and blockquotes.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use warpui::color::ColorU;
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Empty, Flex, MainAxisSize, ParentElement,
};
use warpui::fonts::{FamilyId, Weight};
use warpui_core::scene::Border;

use crate::terminal::render::markdown_agent::heading_style_for;
use crate::terminal::render::markdown_inline::collect_code_block;
use crate::terminal::render::markdown_inline::collect_inline;
use crate::terminal::render::markdown_inline::is_end_blockquote;
use crate::terminal::render::markdown_inline::is_end_paragraph;
use crate::terminal::render::markdown_inline::render_list;
use crate::terminal::render::markdown_tables::build_text_with_char_styles;
use crate::terminal::render::markdown_tables::markdown_code_language;
use crate::terminal::render::markdown_tables::markdown_code_tag;
use crate::terminal::render::markdown_tables::render_code_block_styled;
use crate::terminal::render::markdown_tables::render_table;
use crate::theme::Theme;

// ── Public API ────────────────────────────────────────────────────────────────

/// Render markdown text into a warpui column of styled elements.

#[derive(Clone, Copy)]
pub(crate) struct MarkdownStyle {
    pub(crate) body_font: FamilyId,
    pub(crate) code_font: FamilyId,
    pub(crate) body_size: f32,
    pub(crate) block_spacing: f32,
    pub(crate) line_height_ratio: f32,
    pub(crate) document: bool,
    /// A hairline under each paragraph: a file's look, not a message's.
    pub(crate) rules: bool,
}

impl MarkdownStyle {
    pub(crate) fn compact(mono: FamilyId) -> Self {
        Self {
            body_font: mono,
            code_font: mono,
            body_size: crate::theme::scaled_mono(14.0),
            block_spacing: 6.0,
            line_height_ratio: 1.25,
            document: false,
            rules: false,
        }
    }

    pub(crate) fn document(
        body_font: FamilyId,
        code_font: FamilyId,
        body_size: f32,
        line_height_ratio: f32,
    ) -> Self {
        Self {
            body_font,
            code_font,
            body_size,
            block_spacing: 0.0,
            // Markdown is prose, so it needs more breathing room than the
            // terminal/editor even when it inherits their configured font.
            line_height_ratio: line_height_ratio.max(1.45),
            document: true,
            rules: true,
        }
    }

    /// The document's type and spacing without its rules, for text an
    /// agent wrote shown inside a card or a list.
    pub(crate) fn prose(body_font: FamilyId, code_font: FamilyId, body_size: f32, line_height_ratio: f32) -> Self {
        Self { rules: false, ..Self::document(body_font, code_font, body_size, line_height_ratio) }
    }
}

pub(crate) fn render_markdown_with_style(
    text: &str,
    theme: Theme,
    style: MarkdownStyle,
) -> Box<dyn Element> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let events: Vec<Event<'_>> = Parser::new_ext(text, opts).collect();

    let mut blocks: Vec<Box<dyn Element>> = Vec::new();
    let mut i = 0;

    while i < events.len() {
        let (el, consumed) = render_block_with_style(&events, i, theme, style);
        i += consumed.max(1);
        if let Some(el) = el {
            blocks.push(apply_document_block_rhythm(
                el,
                events.get(i - consumed),
                style,
            ));
        }
    }

    if blocks.is_empty() {
        return Box::new(Empty::new());
    }

    let mut col = Flex::column()
        .with_cross_axis_alignment(if style.document {
            CrossAxisAlignment::Stretch
        } else {
            CrossAxisAlignment::Start
        })
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(style.block_spacing);
    for block in blocks {
        col = col.with_child(block);
    }
    col.finish()
}

// ── Block-level dispatch ──────────────────────────────────────────────────────

/// Returns (element, events consumed including opening Start and closing End tags).
pub(crate) fn render_block(
    events: &[Event<'_>],
    i: usize,
    theme: Theme,
    mono: FamilyId,
) -> (Option<Box<dyn Element>>, usize) {
    render_block_with_style(events, i, theme, MarkdownStyle::compact(mono))
}

pub(crate) fn render_block_with_style(
    events: &[Event<'_>],
    i: usize,
    theme: Theme,
    style: MarkdownStyle,
) -> (Option<Box<dyn Element>>, usize) {
    let ev = match events.get(i) {
        Some(e) => e,
        None => return (None, 1),
    };

    match ev {
        Event::Start(Tag::Paragraph) => {
            let (text, char_styles, consumed) = collect_inline(events, i + 1, is_end_paragraph);
            let text = build_text_with_char_styles(
                text,
                char_styles,
                style.body_size,
                Weight::Normal,
                theme.main_text(),
                theme,
                style.body_font,
                style.line_height_ratio,
            );
            let el = if style.rules {
                Container::new(text)
                    .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                    .finish()
            } else {
                text
            };
            (Some(el), 1 + consumed)
        }

        Event::Start(Tag::Heading { level, .. }) => {
            let level = *level;
            let end_check: fn(&Event<'_>) -> bool = match level {
                HeadingLevel::H1 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H1))),
                HeadingLevel::H2 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H2))),
                HeadingLevel::H3 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H3))),
                HeadingLevel::H4 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H4))),
                HeadingLevel::H5 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H5))),
                HeadingLevel::H6 => |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H6))),
            };
            let (text, char_styles, consumed) = collect_inline(events, i + 1, end_check);
            let (size, weight) = heading_style_for(level, style.document, style.body_size);
            let el = build_text_with_char_styles(
                text,
                char_styles,
                size,
                weight,
                theme.main_text(),
                theme,
                style.body_font,
                style.line_height_ratio,
            );
            (Some(el), 1 + consumed)
        }

        Event::Start(Tag::CodeBlock(kind)) => {
            let (code, consumed) = collect_code_block(events, i + 1);
            let el = render_code_block_styled(
                code.trim_end(),
                markdown_code_tag(kind),
                markdown_code_language(kind),
                theme,
                style,
            );
            (Some(el), 1 + consumed)
        }

        Event::Start(Tag::List(ordered)) => {
            let ordered = ordered.is_some();
            let (el, consumed) = render_list(events, i + 1, ordered, theme, style);
            (Some(el), 1 + consumed)
        }

        Event::Start(Tag::BlockQuote(_)) => {
            let (text, char_styles, consumed) = collect_inline(events, i + 1, is_end_blockquote);
            let inner = build_text_with_char_styles(
                text,
                char_styles,
                style.body_size,
                Weight::Normal,
                if style.document {
                    theme.sub_text()
                } else {
                    theme.disabled_text()
                },
                theme,
                style.body_font,
                style.line_height_ratio,
            );
            let el = Container::new(inner)
                .with_padding_left(if style.document { 16.0 } else { 12.0 })
                .with_padding_right(if style.document { 12.0 } else { 0.0 })
                .with_padding_top(if style.document { 8.0 } else { 4.0 })
                .with_padding_bottom(if style.document { 8.0 } else { 4.0 })
                .with_background_color(ColorU::new(0, 0, 0, 0))
                .with_border(
                    Border::left(if style.document { 3.0 } else { 2.0 })
                        .with_border_color(theme.accent),
                )
                .finish();
            (Some(el), 1 + consumed)
        }

        Event::Start(Tag::Table(alignments)) => {
            let alignments = alignments.clone();
            let (el, consumed) = render_table(events, i + 1, &alignments, theme, style);
            (Some(el), 1 + consumed)
        }

        Event::Rule => {
            let el = Container::new(Empty::new().finish())
                .with_padding_top(1.0)
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .finish();
            (Some(el), 1)
        }

        _ => (None, 1),
    }
}

/// Standard document rhythm: prose elements own their semantic margins
/// instead of receiving the same generic gap. Agent Markdown remains compact.
pub(crate) fn apply_document_block_rhythm(
    element: Box<dyn Element>,
    event: Option<&Event<'_>>,
    style: MarkdownStyle,
) -> Box<dyn Element> {
    if !style.document {
        return element;
    }
    let (top, bottom) = match event {
        Some(Event::Start(Tag::Heading { .. })) => (14.0, 8.0),
        Some(Event::Start(Tag::Paragraph)) => (0.0, 10.0),
        Some(Event::Start(Tag::CodeBlock(_))) => (6.0, 14.0),
        Some(Event::Start(Tag::List(_))) => (2.0, 10.0),
        Some(Event::Start(Tag::BlockQuote(_))) => (4.0, 12.0),
        Some(Event::Start(Tag::Table(_))) => (6.0, 14.0),
        Some(Event::Rule) => (14.0, 14.0),
        _ => (0.0, 0.0),
    };
    Container::new(element)
        .with_padding_top(top)
        .with_padding_bottom(bottom)
        .finish()
}
