use warpui::elements::{
    ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment,
    DispatchEventResult, Element, Empty, EventHandler, Expanded, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, ScrollbarWidth, Text,
};
use warpui::fonts::FamilyId;
use warpui::Action;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::console::fs_suggest::FsEntry;
use crate::console::history_search::HistoryMatch;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{mono_font_size, Theme, COMPLETION_MODAL_MAX_HEIGHT, INPUT_BOX_MARGIN};

pub fn render_path_picker_overlay<A: Action + Clone + 'static>(
    entries: &[FsEntry],
    selected: usize,
    theme: Theme,
    mono: FamilyId,
    scroll_state: ClippedScrollStateHandle,
    click_action: impl Fn(usize) -> A + Copy + 'static,
) -> Box<dyn Element> {
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (idx, entry) in entries.iter().enumerate() {
        let is_selected = idx == selected;
        let is_dir = entry.is_dir;
        let icon = if is_dir {
            AppIcon::Folder
        } else {
            AppIcon::Terminal
        };
        let kind = if is_dir { "Directory" } else { "File" };
        let display_text = entry
            .description
            .clone()
            .unwrap_or_else(|| kind.to_string());
        let entry_name = entry.name.clone();

        let mouse_state = MouseStateHandle::default();
        let row_el = Hoverable::new(mouse_state, move |state| {
            let hovered = state.is_hovered();
            let mut cell = Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(10.0)
                            .with_child(render_icon(icon, theme.main_text(), 14.0))
                            .with_child(Box::new(
                                Text::new(entry_name.clone(), mono, mono_font_size())
                                    .with_color(theme.main_text()),
                            ))
                            .finish(),
                    )
                    .with_child(Box::new(
                        Text::new(display_text.clone(), mono, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text()),
                    ))
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0);

            if is_selected {
                cell = cell.with_background_color(theme.surface_2());
            } else if hovered {
                cell = cell.with_background_color(theme.fg_overlay_1());
            }

            cell.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_mouse_down_with_modifiers(move |event_ctx, _app, _pos, _mods| {
            event_ctx.dispatch_typed_action(click_action(idx));
        })
        .finish();

        list = list.with_child(row_el);
    }

    let scrollable = ClippedScrollable::vertical(
        scroll_state,
        list.finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.main_text().into(),
        theme.surface_2().into(),
    )
    .finish();

    let modal = Container::new(
        ConstrainedBox::new(scrollable)
            .with_max_height(COMPLETION_MODAL_MAX_HEIGHT)
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_margin_left(INPUT_BOX_MARGIN)
    .with_margin_right(INPUT_BOX_MARGIN)
    .with_margin_bottom(4.0)
    .finish();

    modal
}

pub fn render_history_search_overlay<A: Action + Clone + 'static>(
    results: &[HistoryMatch],
    selected: usize,
    query: &str,
    theme: Theme,
    mono: FamilyId,
    click_action: impl Fn(usize) -> A + Copy + 'static,
) -> Box<dyn Element> {
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    let title_label = if query.is_empty() {
        "Recent commands".to_string()
    } else {
        format!("Search: {query}")
    };
    let title_row = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Box::new(
                Text::new("\u{1F50D} ".to_string(), mono, mono_font_size())
                    .with_color(theme.sub_text()),
            ))
            .with_child(Box::new(
                Text::new(title_label, mono, mono_font_size()).with_color(theme.main_text()),
            ))
            .finish(),
    )
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(8.0)
    .with_padding_bottom(8.0)
    .finish();
    list = list.with_child(title_row);

    list = list.with_child(
        Container::new(Empty::new().finish())
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish(),
    );

    if results.is_empty() {
        let empty_row = Container::new(Box::new(
            Text::new("No matches".to_string(), mono, mono_font_size())
                .with_color(theme.disabled_text()),
        ))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish();
        list = list.with_child(empty_row);
    } else {
        for (idx, result) in results.iter().enumerate() {
            let is_selected = idx == selected;
            let command = result.command.clone();
            let match_ranges = result.match_ranges.clone();
            let count = result.count;

            let mut text_el = Text::new("", mono, mono_font_size());
            let props = warpui_core::fonts::Properties::default();
            let mut byte_pos = 0usize;
            while byte_pos < command.len() {
                let ch = command[byte_pos..].chars().next().unwrap_or(' ');
                let ch_end = byte_pos + ch.len_utf8();
                let in_match = match_ranges
                    .iter()
                    .any(|r| r.start <= byte_pos && byte_pos < r.end);
                let color = if in_match {
                    theme.accent
                } else {
                    theme.main_text()
                };
                text_el.add_text_with_highlights(&command[byte_pos..ch_end], color, props);
                byte_pos = ch_end;
            }
            // Keep each result on a SINGLE line — long commands (e.g. a full
            // `aws … --service-arn arn:…`) otherwise soft-wrap (Text::new defaults
            // to wrapping) and the taller rows overlap each other + the input bar.
            // Clip with an ellipsis instead, fzf/Ctrl-R style.
            let text_el = text_el.soft_wrap(false).with_clip(ClipConfig::ellipsis());

            let count_label = format!("\u{00D7}{count}");
            let selector = if is_selected { "\u{25B6} " } else { "  " };

            let row_inner = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Box::new(
                    Text::new(selector.to_string(), mono, mono_font_size())
                        .with_color(theme.accent),
                ))
                .with_child(Box::new(Expanded::new(1.0, Box::new(text_el))))
                .with_child(Box::new(
                    Text::new(count_label, mono, crate::theme::text_size(11.0))
                        .with_color(theme.disabled_text()),
                ))
                .finish();

            let mut row_container = Container::new(row_inner)
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(6.0)
                .with_padding_bottom(6.0);

            if is_selected {
                row_container = row_container.with_background_color(theme.surface_3());
            }

            let row_el = EventHandler::new(row_container.finish())
                .on_left_mouse_down(move |event_ctx, _app, _pos| {
                    event_ctx.dispatch_typed_action(click_action(idx));
                    DispatchEventResult::StopPropagation
                })
                .finish();

            list = list.with_child(row_el);
        }
    }

    let footer = Container::new(Box::new(
        Text::new(
            "\u{2191}\u{2193} navigate \u{00B7} Enter accept \u{00B7} Esc cancel".to_string(),
            mono,
            crate::theme::text_size(11.0),
        )
        .with_color(theme.disabled_text()),
    ))
    .with_padding_left(12.0)
    .with_padding_right(12.0)
    .with_padding_top(6.0)
    .with_padding_bottom(6.0)
    .with_border(Border::top(1.0).with_border_color(theme.outline()))
    .finish();
    list = list.with_child(footer);

    let modal = Container::new(
        ConstrainedBox::new(list.finish())
            .with_max_height(COMPLETION_MODAL_MAX_HEIGHT)
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_margin_left(INPUT_BOX_MARGIN)
    .with_margin_right(INPUT_BOX_MARGIN)
    .with_margin_bottom(4.0)
    .finish();

    modal
}

pub fn render_workflow_search_overlay<A: Action + Clone + 'static>(
    results: &[crate::console::workflows::WorkflowMatch],
    selected: usize,
    query: &str,
    theme: Theme,
    mono: FamilyId,
    scroll_state: ClippedScrollStateHandle,
    click_action: impl Fn(usize) -> A + Copy + 'static,
) -> Box<dyn Element> {
    let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    let title = if query.is_empty() {
        "Workflows".to_string()
    } else {
        format!("Workflows: {query}")
    };
    list = list.with_child(
        Container::new(Box::new(
            Text::new(title, mono, mono_font_size()).with_color(theme.main_text()),
        ))
        .with_padding_left(12.0)
        .with_padding_right(12.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish(),
    );
    list = list.with_child(
        Container::new(Empty::new().finish())
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish(),
    );

    if results.is_empty() {
        list = list.with_child(
            Container::new(Box::new(
                Text::new("No workflows found".to_string(), mono, mono_font_size())
                    .with_color(theme.disabled_text()),
            ))
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(14.0)
            .with_padding_bottom(14.0)
            .finish(),
        );
    } else {
        for (idx, result) in results.iter().enumerate() {
            let is_selected = idx == selected;
            let name = result.workflow.name.clone();
            let desc = result.workflow.description.clone();
            let mouse_state = MouseStateHandle::default();
            let row = Hoverable::new(mouse_state, move |state| {
                let bg = if is_selected || state.is_hovered() {
                    theme.surface_3()
                } else {
                    theme.surface_2()
                };
                let mut col = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(3.0);
                col = col.with_child(Box::new(
                    Text::new(
                        format!("{}{}", if is_selected { "› " } else { "  " }, name),
                        mono,
                        mono_font_size(),
                    )
                    .with_color(if is_selected {
                        theme.accent
                    } else {
                        theme.main_text()
                    })
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis()),
                ));
                if !desc.is_empty() {
                    col = col.with_child(Box::new(
                        Text::new(desc, mono, crate::theme::text_size(11.0))
                            .with_color(theme.sub_text())
                            .soft_wrap(false)
                            .with_clip(ClipConfig::ellipsis()),
                    ));
                }
                Container::new(col.finish())
                    .with_background_color(bg)
                    .with_border(if is_selected {
                        Border::left(2.0).with_border_color(theme.accent)
                    } else {
                        Border::left(2.0).with_border_color(theme.surface_2())
                    })
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(8.0)
                    .with_padding_bottom(8.0)
                    .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(click_action(idx));
            })
            .finish();
            list = list.with_child(row);
        }
    }

    let results_view = ClippedScrollable::vertical(
        scroll_state,
        list.finish(),
        ScrollbarWidth::Auto,
        theme.disabled_text().into(),
        theme.main_text().into(),
        theme.surface_2().into(),
    )
    .finish();

    Container::new(
        ConstrainedBox::new(results_view)
            .with_max_height(COMPLETION_MODAL_MAX_HEIGHT)
            .finish(),
    )
    .with_background_color(theme.surface_2())
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
    .with_margin_left(INPUT_BOX_MARGIN)
    .with_margin_right(INPUT_BOX_MARGIN)
    .with_margin_bottom(4.0)
    .finish()
}
