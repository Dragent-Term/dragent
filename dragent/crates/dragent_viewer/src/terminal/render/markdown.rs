//! Markdown renderer using pulldown-cmark (MIT licensed).
//!
//! Converts LLM markdown output into a warpui element tree with proper
//! styling for headings, bold/italic inline spans, code blocks, lists,
//! and blockquotes.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use warpui::elements::{Container, CrossAxisAlignment, Element, Flex, MainAxisSize, ParentElement};
use warpui::fonts::{FamilyId, Weight};
use warpui::{Action, EntityId};
use warpui_core::scene::Border;

use crate::theme::Theme;

// ── Public API ────────────────────────────────────────────────────────────────

/// Render markdown text into a warpui column of styled elements.

/// Render markdown text into a warpui column of styled elements.
pub fn render_markdown(text: &str, theme: Theme, mono: FamilyId) -> Box<dyn Element> {
    render_markdown_with_style(text, theme, MarkdownStyle::compact(mono))
}

/// Render a standalone Markdown document. Unlike compact agent Markdown, the
/// document surface uses proportional UI typography for prose, a dedicated
/// monospace face for code, and a more generous vertical rhythm.
pub fn render_markdown_document(
    text: &str,
    theme: Theme,
    body_font: FamilyId,
    code_font: FamilyId,
    body_size: f32,
    line_height_ratio: f32,
) -> Box<dyn Element> {
    render_markdown_with_style(
        text,
        theme,
        MarkdownStyle::document(body_font, code_font, body_size, line_height_ratio),
    )
}

/// Render Markdown an agent wrote inside a card or a detail: the
/// document's type and spacing, without the rule under each paragraph.
pub fn render_markdown_prose(
    text: &str,
    theme: Theme,
    body_font: FamilyId,
    code_font: FamilyId,
    body_size: f32,
    line_height_ratio: f32,
) -> Box<dyn Element> {
    render_markdown_with_style(
        text,
        theme,
        MarkdownStyle::prose(body_font, code_font, body_size, line_height_ratio),
    )
}

/// Render a standalone document whose Markdown links dispatch an application
/// action. This keeps the renderer reusable without depending on the desktop
/// workspace crate or its navigation types.
pub fn render_markdown_document_with_links<A, F>(
    text: &str,
    theme: Theme,
    body_font: FamilyId,
    code_font: FamilyId,
    body_size: f32,
    line_height_ratio: f32,
    view_id: EntityId,
    action_for_url: F,
) -> Box<dyn Element>
where
    A: Action + Clone + 'static,
    F: Fn(String) -> A + Clone + 'static,
{
    let style = MarkdownStyle::document(body_font, code_font, body_size, line_height_ratio);
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let events: Vec<Event<'_>> = Parser::new_ext(text, opts).collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let (element, consumed) = match events.get(i) {
            Some(Event::Start(Tag::Paragraph)) => {
                let (text, styles, links, consumed) =
                    collect_inline_links(&events, i + 1, is_end_paragraph);
                let element = if links.is_empty() {
                    build_text_with_char_styles(
                        text,
                        styles,
                        style.body_size,
                        Weight::Normal,
                        theme.main_text(),
                        theme,
                        style.body_font,
                        style.line_height_ratio,
                    )
                } else {
                    build_document_paragraph_with_links(
                        text,
                        styles,
                        links,
                        view_id,
                        action_for_url.clone(),
                        theme,
                        style,
                        style.body_size,
                        Weight::Normal,
                        theme.main_text(),
                    )
                };
                (Some(element), 1 + consumed)
            }
            Some(Event::Start(Tag::Heading { level, .. })) => {
                let level = *level;
                let end: fn(&Event<'_>) -> bool = match level {
                    HeadingLevel::H1 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H1)))
                    }
                    HeadingLevel::H2 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H2)))
                    }
                    HeadingLevel::H3 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H3)))
                    }
                    HeadingLevel::H4 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H4)))
                    }
                    HeadingLevel::H5 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H5)))
                    }
                    HeadingLevel::H6 => {
                        |e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H6)))
                    }
                };
                let (text, styles, links, consumed) = collect_inline_links(&events, i + 1, end);
                if links.is_empty() {
                    render_block_with_style(&events, i, theme, style)
                } else {
                    let (size, weight) = heading_style_for(level, true, style.body_size);
                    let heading = build_document_paragraph_with_links(
                        text,
                        styles,
                        links,
                        view_id,
                        action_for_url.clone(),
                        theme,
                        style,
                        size,
                        weight,
                        theme.main_text(),
                    );
                    (
                        Some(
                            Container::new(heading)
                                .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
                                .finish(),
                        ),
                        1 + consumed,
                    )
                }
            }
            Some(Event::Start(Tag::List(ordered))) => {
                let (element, consumed) = render_document_list_with_links(
                    &events,
                    i + 1,
                    ordered.is_some(),
                    view_id,
                    action_for_url.clone(),
                    theme,
                    style,
                );
                (Some(element), 1 + consumed)
            }
            _ => render_block_with_style(&events, i, theme, style),
        };
        i += consumed.max(1);
        if let Some(element) = element {
            blocks.push(apply_document_block_rhythm(
                element,
                events.get(i - consumed),
                style,
            ));
        }
    }
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(style.block_spacing);
    for block in blocks {
        column = column.with_child(block);
    }
    column.finish()
}

pub use super::markdown_agent::*;
pub use super::markdown_blocks::*;
pub use super::markdown_inline::*;
pub use super::markdown_tables::*;
