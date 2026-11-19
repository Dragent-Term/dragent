use std::collections::VecDeque;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::types::*;
use super::weight::CustomWeight;

pub fn parse_markdown(source: &str) -> FormattedText {
    FormattedText::new(parse_markdown_lines(source))
}

pub fn parse_markdown_with_gfm_tables(source: &str) -> FormattedText {
    parse_markdown(source)
}

pub fn parse_inline_markdown(source: &str) -> FormattedTextInline {
    vec![FormattedTextFragment::plain_text(source)]
}

pub fn parse_html(source: &str) -> Vec<FormattedTextLine> {
    vec![FormattedTextLine::Line(vec![
        FormattedTextFragment::plain_text(source),
    ])]
}

pub fn parse_image_prefix(_source: &str) -> Option<FormattedImage> {
    None
}

pub fn parse_image_run_line(_source: &str) -> Option<FormattedTextLine> {
    None
}

fn parse_markdown_lines(source: &str) -> VecDeque<FormattedTextLine> {
    let mut lines = VecDeque::new();
    let parser = Parser::new_ext(source, Options::all());
    let mut current_inline = Vec::new();
    let mut current_text = String::new();
    let mut current_styles = FormattedTextStyles::default();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    let flush_inline = |inline: &mut Vec<FormattedTextFragment>,
                        text: &mut String,
                        styles: &mut FormattedTextStyles,
                        lines: &mut VecDeque<FormattedTextLine>| {
        if !text.is_empty() {
            inline.push(FormattedTextFragment {
                text: std::mem::take(text),
                styles: std::mem::take(styles),
            });
        }
        if !inline.is_empty() {
            lines.push_back(FormattedTextLine::Line(std::mem::take(inline)));
        }
    };

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                let heading_size = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                lines.push_back(FormattedTextLine::Heading(FormattedTextHeader {
                    heading_size,
                    text: Vec::new(),
                }));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(FormattedTextLine::Heading(header)) = lines.back_mut() {
                    header.text = std::mem::take(&mut current_inline);
                }
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                in_code_block = true;
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                code_buffer.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                lines.push_back(FormattedTextLine::CodeBlock(CodeBlockText {
                    lang: std::mem::take(&mut code_lang),
                    code: std::mem::take(&mut code_buffer),
                }));
                in_code_block = false;
            }
            Event::Text(text) if in_code_block => code_buffer.push_str(&text),
            Event::Code(text) if !in_code_block => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_inline.push(FormattedTextFragment::inline_code(text.to_string()));
            }
            Event::Text(text) => current_text.push_str(&text),
            Event::SoftBreak | Event::HardBreak => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                lines.push_back(FormattedTextLine::LineBreak);
            }
            Event::Start(Tag::Strong) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.weight = Some(CustomWeight::Bold);
            }
            Event::End(TagEnd::Strong) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.weight = None;
            }
            Event::Start(Tag::Emphasis) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.italic = true;
            }
            Event::End(TagEnd::Emphasis) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.italic = false;
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.hyperlink = Some(Hyperlink::Url(dest_url.to_string()));
            }
            Event::End(TagEnd::Link) => {
                flush_inline(
                    &mut current_inline,
                    &mut current_text,
                    &mut current_styles,
                    &mut lines,
                );
                current_styles.hyperlink = None;
            }
            Event::Rule => lines.push_back(FormattedTextLine::HorizontalRule),
            _ => {}
        }
    }

    flush_inline(
        &mut current_inline,
        &mut current_text,
        &mut current_styles,
        &mut lines,
    );

    if lines.is_empty() {
        lines.push_back(FormattedTextLine::Line(vec![
            FormattedTextFragment::plain_text(source),
        ]));
    }

    lines
}
