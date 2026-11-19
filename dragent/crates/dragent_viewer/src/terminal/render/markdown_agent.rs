//! Markdown renderer using pulldown-cmark (MIT licensed).
//!
//! Converts LLM markdown output into a warpui element tree with proper
//! styling for headings, bold/italic inline spans, code blocks, lists,
//! and blockquotes.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Fill, Flex, Hoverable,
    MainAxisSize, ParentElement, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui::{Action, EntityId};
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::EventContext;

use crate::terminal::render::block_output::{
    find_terminal_links, BlockRenderContext, TerminalLinkKind,
};
use crate::terminal::render::markdown_blocks::apply_document_block_rhythm;
use crate::terminal::render::markdown_blocks::render_block;
use crate::terminal::render::markdown_blocks::render_block_with_style;
use crate::terminal::render::markdown_blocks::MarkdownStyle;
use crate::terminal::render::markdown_inline::collect_code_block;
use crate::terminal::render::markdown_inline::is_end_item;
use crate::terminal::render::markdown_inline::is_end_paragraph;
use crate::terminal::render::markdown_inline::item_end;
use crate::terminal::render::markdown_inline::CharStyle;
use crate::terminal::render::markdown_tables::build_text_with_char_styles;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Public API ────────────────────────────────────────────────────────────────

/// Render markdown text into a warpui column of styled elements.

/// Like `render_markdown` but makes markdown links interactive: single-click
/// selects them and shows a "Cmd+Click to open" tooltip; Cmd+Click opens.
pub fn render_markdown_agent<E: warpui::Entity>(
    text: &str,
    block_id: usize,
    ctx: &BlockRenderContext<'_, E>,
    theme: Theme,
    mono: FamilyId,
    diagrams_live: bool,
) -> Box<dyn Element> {
    render_agent_with_style(text, block_id, ctx, theme, MarkdownStyle::compact(mono), diagrams_live)
}

/// Conversation prose uses the interface font; fenced code retains its code font.
pub fn render_markdown_agent_document<E: warpui::Entity>(
    text: &str, block_id: usize, ctx: &BlockRenderContext<'_, E>, theme: Theme,
    body: FamilyId, code: FamilyId, diagrams_live: bool,
) -> Box<dyn Element> {
    render_agent_with_style(text, block_id, ctx, theme, MarkdownStyle::document(body, code, crate::theme::text_size(14.0), 1.5), diagrams_live)
}

fn render_agent_with_style<E: warpui::Entity>(
    text: &str, block_id: usize, ctx: &BlockRenderContext<'_, E>, theme: Theme,
    style: MarkdownStyle, diagrams_live: bool,
) -> Box<dyn Element> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let events: Vec<Event<'_>> = Parser::new_ext(text, opts).collect();

    let mut blocks: Vec<Box<dyn Element>> = Vec::new();
    let mut i = 0;

    while i < events.len() {
        let (el, consumed) =
            render_block_agent(&events, i, block_id, ctx, theme, style, diagrams_live);
        i += consumed.max(1);
        if let Some(el) = el {
            blocks.push(el);
        }
    }

    if blocks.is_empty() {
        return Box::new(Empty::new());
    }

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(if style.document { 12.0 } else { 6.0 });
    for block in blocks {
        col = col.with_child(block);
    }
    col.finish()
}

fn render_block_agent<E: warpui::Entity>(
    events: &[Event<'_>],
    i: usize,
    block_id: usize,
    ctx: &BlockRenderContext<'_, E>,
    theme: Theme,
    style: MarkdownStyle,
    diagrams_live: bool,
) -> (Option<Box<dyn Element>>, usize) {
    let mono = style.body_font;
    let ev = match events.get(i) {
        Some(e) => e,
        None => return (None, 1),
    };

    match ev {
        Event::Start(Tag::Paragraph) => {
            let (text, char_styles, link_ranges, consumed) =
                collect_inline_links(events, i + 1, is_end_paragraph);
            let el = if link_ranges.is_empty() {
                build_text_with_char_styles(
                    text,
                    char_styles,
                    style.body_size,
                    Weight::Normal,
                    theme.main_text(),
                    theme,
                    mono,
                    style.line_height_ratio,
                )
            } else {
                build_paragraph_with_links(
                    text,
                    char_styles,
                    link_ranges,
                    block_id,
                    ctx,
                    theme,
                    mono,
                )
            };
            (Some(el), 1 + consumed)
        }
        // A ```mermaid fenced block becomes a pan/zoom diagram canvas (tab-accent
        // bordered) instead of a plain code block.
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang)))
            if lang.trim().eq_ignore_ascii_case("mermaid") =>
        {
            let (code, consumed) = collect_code_block(events, i + 1);
            let el = ctx.render_mermaid_canvas(code.trim_end(), block_id, i, diagrams_live);
            (Some(el), 1 + consumed)
        }
        // For everything else fall back to the non-interactive render_block path
        _ => if style.document { render_block_with_style(events, i, theme, style) } else { render_block(events, i, theme, mono) },
    }
}

/// Like `collect_inline` but also returns link ranges: `(char_start, char_end, url)`.
pub(crate) fn collect_inline_links(
    events: &[Event<'_>],
    start: usize,
    is_end: fn(&Event<'_>) -> bool,
) -> (String, Vec<CharStyle>, Vec<(usize, usize, String)>, usize) {
    let mut text = String::new();
    let mut char_styles: Vec<CharStyle> = Vec::new();
    let mut has_styling = false;
    let mut link_ranges: Vec<(usize, usize, String)> = Vec::new();
    let mut link_stack: Vec<(usize, String)> = Vec::new();

    let mut bold_depth: u32 = 0;
    let mut italic_depth: u32 = 0;
    let mut strike_depth: u32 = 0;
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
                        link: !link_stack.is_empty(),
                        ..Default::default()
                    };
                    for _ in clean.chars() {
                        char_styles.push(style);
                    }
                }
                // Bare URLs/file paths in plain prose (not inside a Markdown
                // `[text](url)` link already) get the same click-to-open
                // treatment as an explicit link, e.g. "I edited src/main.rs".
                if link_stack.is_empty() {
                    let base_char = text.chars().count();
                    for found in find_terminal_links(&clean) {
                        let start_char = base_char + clean[..found.start].chars().count();
                        let end_char = base_char + clean[..found.end].chars().count();
                        let url = match found.kind {
                            TerminalLinkKind::Url(url) => url,
                            TerminalLinkKind::FilePath { raw, .. } => raw,
                        };
                        link_ranges.push((start_char, end_char, url));
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
                char_styles.extend(crate::terminal::render::markdown_inline::code_span_styles(&clean));
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
            Event::Start(Tag::Link { dest_url, .. }) => {
                if !has_styling {
                    has_styling = true;
                    char_styles = vec![CharStyle::default(); text.chars().count()];
                }
                link_stack.push((text.chars().count(), dest_url.to_string()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((link_start, url)) = link_stack.pop() {
                    let link_end = text.chars().count();
                    if link_end > link_start && !url.is_empty() {
                        link_ranges.push((link_start, link_end, url));
                    }
                }
            }
            Event::Start(Tag::Paragraph) | Event::End(TagEnd::Paragraph) => {}
            _ => {}
        }
    }

    (text, char_styles, link_ranges, consumed)
}

fn build_paragraph_with_links<E: warpui::Entity>(
    text: String,
    char_styles: Vec<CharStyle>,
    link_ranges: Vec<(usize, usize, String)>,
    block_id: usize,
    ctx: &BlockRenderContext<'_, E>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let chars: Vec<char> = text.chars().collect();
    let mut children: Vec<Box<dyn Element>> = Vec::new();
    let mut last = 0usize;

    for (link_start, link_end, url) in &link_ranges {
        let link_start = *link_start;
        let link_end = (*link_end).min(chars.len());
        if link_start > last {
            let seg: String = chars[last..link_start].iter().collect();
            let seg_styles = if char_styles.is_empty() {
                vec![]
            } else {
                char_styles[last..link_start].to_vec()
            };
            children.push(build_text_with_char_styles(
                seg,
                seg_styles,
                crate::theme::scaled_mono(14.0),
                Weight::Normal,
                theme.main_text(),
                theme,
                mono,
                1.25,
            ));
        }

        let display: String = chars[link_start..link_end].iter().collect();
        let selected = ctx
            .link_hint
            .is_some_and(|h| h.block_id == block_id && h.url == *url);
        children.push(build_agent_link(
            display,
            url.clone(),
            block_id,
            selected,
            ctx,
            theme,
            mono,
        ));
        last = link_end;
    }

    if last < chars.len() {
        let seg: String = chars[last..].iter().collect();
        let seg_styles = if char_styles.is_empty() {
            vec![]
        } else {
            char_styles[last..].to_vec()
        };
        children.push(build_text_with_char_styles(
            seg,
            seg_styles,
            crate::theme::scaled_mono(14.0),
            Weight::Normal,
            theme.main_text(),
            theme,
            mono,
            1.25,
        ));
    }

    if children.len() == 1 {
        return children.remove(0);
    }

    let mut wrap = warpui::elements::Wrap::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max);
    for child in children {
        wrap = wrap.with_child(child);
    }
    wrap.finish()
}

pub(crate) fn build_document_paragraph_with_links<A, F>(
    text: String,
    char_styles: Vec<CharStyle>,
    link_ranges: Vec<(usize, usize, String)>,
    view_id: EntityId,
    action_for_url: F,
    theme: Theme,
    style: MarkdownStyle,
    font_size: f32,
    weight: Weight,
    text_color: ColorU,
) -> Box<dyn Element>
where
    A: Action + Clone + 'static,
    F: Fn(String) -> A + Clone + 'static,
{
    let chars = text.chars().collect::<Vec<_>>();
    let mut children = Vec::new();
    let mut last = 0;
    for (start, end, url) in link_ranges {
        let start = start.min(chars.len());
        let end = end.min(chars.len()).max(start);
        if start > last {
            let segment = chars[last..start].iter().collect::<String>();
            let styles = if char_styles.is_empty() {
                Vec::new()
            } else {
                char_styles[last..start].to_vec()
            };
            children.push(build_text_with_char_styles(
                segment,
                styles,
                font_size,
                weight,
                text_color,
                theme,
                style.body_font,
                style.line_height_ratio,
            ));
        }
        let label = chars[start..end].iter().collect::<String>();
        let link_url = url.clone();
        let factory = action_for_url.clone();
        let link = Hoverable::new(
            warpui_core::elements::MouseStateHandle::default(),
            move |mouse| {
                let color = if mouse.is_hovered() {
                    theme.link()
                } else {
                    theme.accent
                };
                Container::new(
                    Text::new(label.clone(), style.body_font, font_size)
                        .with_color(color)
                        .with_line_height_ratio(style.line_height_ratio)
                        .with_style(Properties {
                            weight: if weight == Weight::Bold {
                                Weight::Bold
                            } else {
                                Weight::Semibold
                            },
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_padding_left(1.0)
                .with_padding_right(1.0)
                .with_border(Border::bottom(1.0).with_border_color(color))
                .finish()
            },
        )
        .with_cursor(Cursor::PointingHand)
        // Dispatch on press. The link's Hoverable child is rebuilt as hover
        // state changes; waiting for mouse-up can leave that new instance
        // without the original press state, silently dropping the click.
        // Interactive Markdown links use the same press-based routing.
        .on_mouse_down(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, factory(link_url.clone()));
        });
        children.push(Box::new(link) as Box<dyn Element>);
        last = end;
    }
    if last < chars.len() {
        let segment = chars[last..].iter().collect::<String>();
        let styles = if char_styles.is_empty() {
            Vec::new()
        } else {
            char_styles[last..].to_vec()
        };
        children.push(build_text_with_char_styles(
            segment,
            styles,
            font_size,
            weight,
            text_color,
            theme,
            style.body_font,
            style.line_height_ratio,
        ));
    }
    let mut wrap = warpui::elements::Wrap::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max);
    for child in children {
        wrap = wrap.with_child(child);
    }
    wrap.finish()
}

pub(crate) fn render_document_list_with_links<A, F>(
    events: &[Event<'_>],
    start: usize,
    ordered: bool,
    view_id: EntityId,
    action_for_url: F,
    theme: Theme,
    style: MarkdownStyle,
) -> (Box<dyn Element>, usize)
where
    A: Action + Clone + 'static,
    F: Fn(String) -> A + Clone + 'static,
{
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(7.0);
    let mut item_index = 1u32;
    let mut cursor = start;
    while cursor < events.len() {
        match &events[cursor] {
            Event::End(TagEnd::List(_)) => {
                cursor += 1;
                break;
            }
            Event::Start(Tag::Item) => {
                let content_start = cursor + 1;
                let end = item_end(events, content_start);
                let content = render_document_item_with_links(
                    events,
                    content_start,
                    end,
                    view_id,
                    action_for_url.clone(),
                    theme,
                    style,
                );
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
                column = column.with_child(
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
    (column.finish(), cursor.saturating_sub(start))
}

fn render_document_item_with_links<A, F>(
    events: &[Event<'_>],
    start: usize,
    end: usize,
    view_id: EntityId,
    action_for_url: F,
    theme: Theme,
    style: MarkdownStyle,
) -> Box<dyn Element>
where
    A: Action + Clone + 'static,
    F: Fn(String) -> A + Clone + 'static,
{
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
        let (text, styles, links, _) = collect_inline_links(events, start, is_end_item);
        return if links.is_empty() {
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
                action_for_url,
                theme,
                style,
                style.body_size,
                Weight::Normal,
                theme.main_text(),
            )
        };
    }

    let mut content = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(0.0);
    let mut cursor = start;
    while cursor < end {
        let event = events.get(cursor);
        let (element, consumed) = match event {
            Some(Event::Start(Tag::Paragraph)) => {
                let (text, styles, links, consumed) =
                    collect_inline_links(events, cursor + 1, is_end_paragraph);
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
            Some(Event::Start(Tag::List(ordered))) => {
                let (element, consumed) = render_document_list_with_links(
                    events,
                    cursor + 1,
                    ordered.is_some(),
                    view_id,
                    action_for_url.clone(),
                    theme,
                    style,
                );
                (Some(element), 1 + consumed)
            }
            _ => render_block_with_style(events, cursor, theme, style),
        };
        cursor += consumed.max(1);
        if let Some(element) = element {
            content = content.with_child(apply_document_block_rhythm(element, event, style));
        }
    }
    content.finish()
}

fn build_agent_link<E: warpui::Entity>(
    display: String,
    url: String,
    block_id: usize,
    selected: bool,
    ctx: &BlockRenderContext<'_, E>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use crate::console::dir_listing::link_click_hint_label;
    use warpui::elements::{
        ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack,
    };
    use warpui::geometry::vector::vec2f;
    use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
    use warpui::ui_components::tool_tip::Tooltip;

    let mouse_state = ctx.mouse_state(&format!("agent-link:{}:{}", block_id, url));
    let url_open = url.clone();
    let url_select = url.clone();

    let link_el = Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let color = if hovered || selected {
            theme.link()
        } else {
            theme.accent
        };
        let mut text_el =
            Text::new(display.clone(), mono, crate::theme::text_size(14.0)).with_color(color);
        if hovered || selected {
            text_el = text_el.with_style(Properties {
                weight: Weight::Semibold,
                ..Default::default()
            });
        }
        let mut container = Container::new(text_el.finish())
            .with_padding_left(1.0)
            .with_padding_right(1.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0);
        if selected {
            container = container
                .with_background_color(theme.link_surface())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
        } else if hovered {
            container = container
                .with_background_color(theme.fg_overlay_1())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
        } else {
            container = container.with_border(Border::bottom(1.0).with_border_color(theme.accent));
        }
        container.finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_mouse_down_with_modifiers(move |event_ctx: &mut EventContext, _app, pos, mods| {
        if mods.cmd || mods.ctrl {
            event_ctx.dispatch_typed_action(TerminalAction::OpenLink {
                block_id,
                url: url_open.clone(),
            });
        } else {
            event_ctx.dispatch_typed_action(TerminalAction::SelectLink {
                block_id,
                url: url_select.clone(),
                occurrence: None,
                absolute_position: pos,
            });
        }
    })
    .on_click_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, _mods| {
        event_ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
    })
    .finish();

    if selected {
        let tooltip_styles = UiComponentStyles {
            font_family_id: Some(mono),
            font_size: Some(crate::theme::text_size(11.0)),
            font_color: Some(theme.main_text()),
            background: Some(Fill::Solid(theme.surface_2())),
            border_color: Some(Fill::Solid(theme.outline())),
            border_width: Some(1.0),
            border_radius: Some(CornerRadius::with_all(Radius::Pixels(6.0))),
            padding: Some(Coords {
                top: 5.0,
                bottom: 5.0,
                left: 8.0,
                right: 8.0,
            }),
            ..Default::default()
        };
        let tooltip = Tooltip::new(link_click_hint_label(), tooltip_styles)
            .build()
            .finish();
        Stack::new()
            .with_child(link_el)
            .with_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::BottomMiddle,
                ),
            )
            .finish()
    } else {
        link_el
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn heading_style_for(
    level: HeadingLevel,
    document: bool,
    body_size: f32,
) -> (f32, Weight) {
    if document {
        return match level {
            HeadingLevel::H1 => (body_size * 1.58, Weight::Bold),
            HeadingLevel::H2 => (body_size * 1.41, Weight::Bold),
            HeadingLevel::H3 => (body_size * 1.20, Weight::Semibold),
            HeadingLevel::H4 => (body_size * 1.10, Weight::Semibold),
            HeadingLevel::H5 => (body_size * 1.03, Weight::Semibold),
            HeadingLevel::H6 => (body_size * 0.93, Weight::Semibold),
        };
    }
    match level {
        HeadingLevel::H1 => (crate::theme::scaled_mono(18.0), Weight::Bold),
        HeadingLevel::H2 => (crate::theme::scaled_mono(16.0), Weight::Bold),
        HeadingLevel::H3 => (crate::theme::scaled_mono(15.0), Weight::Semibold),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
            (crate::theme::scaled_mono(14.0), Weight::Semibold)
        }
    }
}

/// Strip emoji characters so they don't reach the text layout engine.
pub fn strip_emoji_str(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            let n = c as u32;
            !matches!(n,
                0x1F300..=0x1F9FF  // misc symbols, emoticons, transport
                | 0x1FA00..=0x1FAFF // chess, symbols extended-a
                | 0x2600..=0x27BF   // misc symbols, dingbats
                | 0xFE00..=0xFE0F   // variation selectors
                | 0x200D            // zero-width joiner
                | 0x20E3            // combining enclosing keycap
                | 0x1F1E0..=0x1F1FF // regional indicator symbols (flags)
                | 0x23F0..=0x23FF   // clock symbols
                | 0x25AA..=0x25FE   // geometric shapes subset
                | 0x2B00..=0x2BFF   // misc symbols and arrows
            )
        })
        .collect()
}

#[cfg(test)]
mod document_typography_tests {
    use super::super::markdown_tables::{markdown_code_language, table_column_weight};
    use super::*;
    use dragent_editor::Language;

    #[test]
    fn preview_headings_scale_from_the_configured_body_size() {
        let body = 17.0;
        let (h1, _) = heading_style_for(HeadingLevel::H1, true, body);
        let (h2, _) = heading_style_for(HeadingLevel::H2, true, body);
        let (h3, _) = heading_style_for(HeadingLevel::H3, true, body);
        assert!(h1 > h2 && h2 > h3 && h3 > body);
    }

    #[test]
    fn preview_line_height_never_inherits_terminal_density() {
        let style = MarkdownStyle::document(FamilyId(1), FamilyId(1), 13.0, 1.0);
        assert_eq!(style.body_size, 13.0);
        assert_eq!(style.line_height_ratio, 1.45);
    }

    #[test]
    fn preview_table_columns_have_readable_bounded_weights() {
        assert_eq!(table_column_weight(0, true), 112.0);
        assert_eq!(table_column_weight(10_000, true), 900.0);
        // A prose column outweighs a label column several times over.
        assert!(table_column_weight(300, true) > 3.0 * table_column_weight(12, true));
        let label = table_column_weight("Viewport".chars().count(), true);
        assert!(label >= 112.0);
    }

    #[test]
    fn structured_list_items_preserve_fenced_code_blocks() {
        let events = Parser::new_ext(
            "1. Create flow:\n\n   ```json\n   {\"active\": true}\n   ```\n\n   Save it.\n",
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
        )
        .collect::<Vec<_>>();
        let item_start = events
            .iter()
            .position(|event| matches!(event, Event::Start(Tag::Item)))
            .expect("list item");
        let end = item_end(&events, item_start + 1);
        let code_kind = events[item_start + 1..end]
            .iter()
            .find_map(|event| match event {
                Event::Start(Tag::CodeBlock(kind)) => Some(kind),
                _ => None,
            })
            .expect("fenced code remains a child block");

        assert_eq!(markdown_code_language(code_kind), Language::Json);
    }
}
