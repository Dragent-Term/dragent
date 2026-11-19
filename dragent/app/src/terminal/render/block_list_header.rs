//! Scrollable block list for normal (non-alt) terminal mode.

use async_channel::Sender;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::terminal::render::block_grid_painter::paint_grid_line;
use crate::terminal::render::block_list_element::BLOCK_ACCENT_WIDTH;
use crate::terminal::render::block_list_element::PROMPT_TAIL_MAX_LINES;
use crate::terminal::view::actions::{block_copy_key, EntryHint, LinkHint, TerminalAction};
use crate::terminal::view::copy_confirmation::copy_glyph;
use crate::workspace::InputEditor;
use dragent_terminal::StyledLine;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded,
    Fill, Flex, Hoverable, MainAxisAlignment, MainAxisSize, ParentElement, Point, SavePosition,
    ScrollData, ScrollableElement, SelectableElement, Selection, SelectionFragment, SmartSelectFn,
    Text,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::{vec2f, Vector2F};
use warpui::platform::Cursor;
use warpui::string_offset::{ByteOffset, CharOffset};
use warpui::text::{IsRect, SelectionDirection, SelectionType};
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui::units::Pixels;
use warpui::ViewHandle;
use warpui::{
    AfterLayoutContext, AppContext, ClipBounds, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};
use warpui_core::scene::ZIndex;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text::word_boundaries::WordBoundariesPolicy;
use warpui_core::text::TextBuffer;
use warpui_core::text_layout::ClipConfig;

use crate::console::block_filter::{count_block_matches, BlockFilterState};
use crate::console::dir_listing::{entry_click_hint_label, DirSort, DirView};
use crate::console::escape_parse::{styled_to_colored, ColoredLine, TextSpan};
use crate::icons::{render_icon, AppIcon};
use crate::terminal::model::block::{live_primary_line_count, Block, BlockStatus};
use crate::terminal::model::block_height_tree::{
    BlockHeightIndex, BlockListEntry, BLOCK_HEADER_HEIGHT_PX, COMPLETED_BLOCK_MAX_LINES,
};
use crate::terminal::model::is_signal_exit;
use crate::terminal::render::block_card_element::header_command_line;
use crate::terminal::render::dir_listing_paint::{
    dir_entry_char_width, dir_entry_name_origin_in_body, dir_listing_body_height,
    dir_listing_selection_lines, layout_dir_listing, paint_dir_listing, sort_dir_entries,
    DirBlockState, DirListingHit, DirListingLayout, DirListingPaintParams, DIR_BODY_PAD_LEFT,
};
use crate::terminal::render::grid_metrics::{
    line_terminal_column_count, slice_line_terminal_columns, terminal_column_at_char_index,
    terminal_column_center_x, x_to_terminal_column, CellMetrics,
};
use crate::terminal::render::grid_renderer::{build_alt_screen_column, GridLineParams};
use crate::terminal::render::BlockRenderContext;
use crate::terminal::render::CellGlyphCache;
use crate::terminal::viewport::{ScrollPosition, VisibleEntry, VisibleEntryKind};
use crate::theme::{mono_font_size, Theme, TERMINAL_PADDING_LEFT};

/// Live prompt grid shown below completed blocks when no command is running.
#[derive(Clone)]
pub struct LivePromptSection {
    pub lines: Vec<StyledLine>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
}

/// Keep only the last few session lines for the typing prompt.
pub fn extract_prompt_tail(
    lines: Vec<StyledLine>,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
) -> LivePromptSection {
    let cursor_row = cursor_row.min(lines.len().saturating_sub(1));
    let end = cursor_row.saturating_add(1).min(lines.len());
    let start = end.saturating_sub(PROMPT_TAIL_MAX_LINES);
    let tail: Vec<_> = lines.into_iter().skip(start).take(end - start).collect();
    LivePromptSection {
        lines: tail,
        cursor_row: cursor_row.saturating_sub(start),
        cursor_col,
        cursor_visible,
    }
}

/// Fallback single-grid UI before shell integration creates blocks.
pub fn build_bootstrap_grid_ui(
    lines: Vec<StyledLine>,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    theme: Theme,
    mono_family: FamilyId,
) -> Box<dyn Element> {
    build_alt_screen_column(
        &lines,
        cursor_row,
        cursor_col,
        cursor_visible,
        None,
        GridLineParams {
            theme,
            mono_family,
            padding_left: TERMINAL_PADDING_LEFT,
            font_size: mono_font_size(),
        },
    )
}

fn stripe_color(status: BlockStatus, theme: Theme) -> ColorU {
    match status {
        BlockStatus::Running => theme.accent,
        BlockStatus::Completed { exit_code: Some(0) }
        | BlockStatus::Completed { exit_code: None } => theme.ui_green(),
        BlockStatus::Completed {
            exit_code: Some(code),
        } if is_signal_exit(code) => theme.disabled_text(),
        BlockStatus::Completed { exit_code: Some(_) } => theme.ui_red(),
        BlockStatus::Pending => theme.accent,
    }
}

pub fn is_error_status(status: BlockStatus) -> bool {
    matches!(
        status,
        BlockStatus::Completed { exit_code: Some(code) } if !is_signal_exit(code) && code != 0
    )
}

fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f32 / 1000.0)
    } else if ms < 3_600_000 {
        format!("{:.1}m", ms as f32 / 60_000.0)
    } else {
        format!("{:.1}h", ms as f32 / 3_600_000.0)
    }
}

fn render_copy_icon_btn(
    block_id: usize,
    command: String,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mouse_state = ctx.mouse_state(&format!("copy_btn:{block_id}"));
    let key = block_copy_key(block_id);
    let copied = ctx.is_copied(&key);

    Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let (bg_color, border_color, icon_color) = if hovered {
            (theme.surface_3(), theme.outline(), theme.main_text())
        } else {
            (
                ColorU::new(0, 0, 0, 0),
                theme.outline(),
                theme.disabled_text(),
            )
        };
        let (icon, icon_color) = copy_glyph(copied, icon_color, theme);

        Container::new(render_icon(icon, icon_color, 14.0))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(bg_color)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::CopyWithConfirmation {
            key: key.clone(),
            text: command.clone(),
        });
    })
    .finish()
}

/// The ⋯ button on a block header that toggles the per-block options dropdown.
fn render_block_menu_btn(block_id: usize, ctx: &BlockRenderContext<'_>) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mouse_state = ctx.mouse_state(&format!("block_menu_btn:{block_id}"));

    Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let (bg_color, border_color, icon_color) = if hovered {
            (theme.surface_3(), theme.outline(), theme.main_text())
        } else {
            (
                ColorU::new(0, 0, 0, 0),
                theme.outline(),
                theme.disabled_text(),
            )
        };
        Container::new(render_icon(AppIcon::DotsVertical, icon_color, 14.0))
            .with_padding_left(7.0)
            .with_padding_right(7.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(bg_color)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::ToggleBlockMenu { block_id });
    })
    .finish()
}

/// Width of the block options dropdown menu.
pub const BLOCK_MENU_WIDTH: f32 = 220.0;

/// The inline output filter bar shown under a block header while its filter is
/// active. Contains a search glyph, the live query text, a match count, a regex
/// toggle (`.*`) and a close (×) button. Text entry is routed via keystroke
/// actions by the terminal view (the bar itself is display-only).
pub fn build_block_filter_bar(
    block_id: usize,
    editor: &ViewHandle<InputEditor>,
    query_empty: bool,
    use_regex: bool,
    regex_error: bool,
    match_count: usize,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mono = ctx.mono;

    // Regex toggle (.*)
    let regex_state = ctx.mouse_state(&format!("bfilter_regex:{block_id}"));
    let regex_btn = Hoverable::new(regex_state, move |st| {
        let hovered = st.is_hovered();
        let (bg, fg) = if use_regex {
            (theme.accent, theme.background)
        } else if hovered {
            (theme.surface_3(), theme.main_text())
        } else {
            (ColorU::new(0, 0, 0, 0), theme.sub_text())
        };
        Container::new(Box::new(
            Text::new(".*".to_string(), mono, crate::theme::text_size(11.0)).with_color(fg),
        ))
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .with_background_color(bg)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ec, _app, _pos| {
        ec.dispatch_typed_action(TerminalAction::ToggleBlockFilterRegex { block_id });
    })
    .finish();

    // Close (×)
    let close_state = ctx.mouse_state(&format!("bfilter_close:{block_id}"));
    let close_btn = Hoverable::new(close_state, move |st| {
        let hovered = st.is_hovered();
        let fg = if hovered {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        Container::new(render_icon(AppIcon::XClose, fg, 11.0))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(if hovered {
                theme.surface_3()
            } else {
                ColorU::new(0, 0, 0, 0)
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ec, _app, _pos| {
        ec.dispatch_typed_action(TerminalAction::CloseBlockFilter { block_id });
    })
    .finish();

    // Match count / regex error indicator.
    let (count_text, count_color) = if regex_error {
        ("Invalid regex".to_string(), theme.ansi_red())
    } else if query_empty {
        (String::new(), theme.disabled_text())
    } else if match_count == 0 {
        ("No matches".to_string(), theme.disabled_text())
    } else {
        let current = ctx
            .block_filter_match_index
            .min(match_count.saturating_sub(1));
        (format!("{} / {match_count}", current + 1), theme.sub_text())
    };

    let match_nav_button = |key: String, icon: AppIcon, action: TerminalAction| {
        let state = ctx.mouse_state(&key);
        let enabled = !query_empty && !regex_error && match_count > 0;
        Hoverable::new(state, move |st| {
            let hovered = st.is_hovered();
            let fg = if !enabled {
                theme.disabled_text()
            } else if hovered {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            Container::new(render_icon(icon, fg, 13.0))
                .with_padding_left(4.0)
                .with_padding_right(4.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if hovered && enabled {
                    theme.surface_3()
                } else {
                    ColorU::new(0, 0, 0, 0)
                })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
        })
        .with_cursor(if enabled {
            Cursor::PointingHand
        } else {
            Cursor::Arrow
        })
        .on_click(move |ec, _app, _pos| {
            if enabled {
                ec.dispatch_typed_action(action.clone());
            }
        })
        .finish()
    };

    let previous_match = match_nav_button(
        format!("bfilter_prev:{block_id}"),
        AppIcon::ArrowUp,
        TerminalAction::BlockFilterMatch {
            block_id,
            delta: -1,
        },
    );
    let next_match = match_nav_button(
        format!("bfilter_next:{block_id}"),
        AppIcon::ArrowDown,
        TerminalAction::BlockFilterMatch { block_id, delta: 1 },
    );

    // Keep the query visually distinct from the toolbar actions, matching the
    // compact find bars used by terminal and editor applications.
    let search_field = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(AppIcon::Search, theme.sub_text(), 13.0))
            .with_child(Box::new(Expanded::new(
                1.0,
                ChildView::new(editor).finish(),
            )))
            .finish(),
    )
    .with_padding_left(8.0)
    .with_padding_right(8.0)
    .with_padding_top(3.0)
    .with_padding_bottom(3.0)
    .with_background_color(theme.surface_1())
    .with_border(Border::all(1.0).with_border_color(theme.accent))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish();

    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(7.0)
        .with_child(Box::new(Expanded::new(1.0, search_field)))
        .with_child(Box::new(
            Text::new(count_text, mono, crate::theme::text_size(10.5)).with_color(count_color),
        ))
        .with_child(previous_match)
        .with_child(next_match)
        .with_child(regex_btn)
        .with_child(close_btn)
        .finish();

    let body = Container::new(row)
        .with_padding_left(8.0)
        .with_padding_right(7.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .finish();

    Align::new(ConstrainedBox::new(body).with_max_width(520.0).finish())
        .right()
        .finish()
}

fn block_menu_divider(theme: Theme) -> Box<dyn Element> {
    Container::new(
        Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_height(1.0)
                .finish(),
        )
        .with_background_color(theme.outline())
        .finish(),
    )
    .with_padding_top(4.0)
    .with_padding_bottom(4.0)
    .finish()
}

fn block_menu_item(
    key: String,
    icon: AppIcon,
    label: &str,
    action: TerminalAction,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mono = ctx.mono;
    let mouse_state = ctx.mouse_state(&key);
    let label = label.to_string();
    let is_danger = label == "Terminate process";

    Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered();
        let bg = if hovered {
            if is_danger {
                ColorU::new(
                    theme.ansi_red().r,
                    theme.ansi_red().g,
                    theme.ansi_red().b,
                    28,
                )
            } else {
                theme.surface_3()
            }
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        let fg = if is_danger {
            theme.ansi_red()
        } else if hovered {
            theme.main_text()
        } else {
            theme.sub_text()
        };
        let row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(9.0)
            .with_child(render_icon(icon, fg, 12.0))
            .with_child(Box::new(
                Text::new(label.clone(), mono, crate::theme::text_size(12.0)).with_color(fg),
            ))
            .finish();
        Container::new(row)
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(6.0)
            .with_padding_bottom(6.0)
            .with_background_color(bg)
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(action.clone());
    })
    .finish()
}

/// The dropdown menu shown when a block's ⋯ button is clicked. Rendered as an
/// overlay by the terminal view, anchored to the block header.
pub fn build_block_options_menu(
    block_id: usize,
    command: String,
    filter_active: bool,
    running: bool,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let filter_label = if filter_active {
        "Hide filter"
    } else {
        "Filter output"
    };

    let col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(block_menu_item(
            format!("bmenu_copycmd:{block_id}"),
            AppIcon::Copy,
            "Copy command",
            TerminalAction::CopyWithConfirmation {
                key: block_copy_key(block_id),
                text: command,
            },
            ctx,
        ))
        .with_child(block_menu_item(
            format!("bmenu_copyout:{block_id}"),
            AppIcon::Copy,
            "Copy output",
            TerminalAction::CopyBlockOutput { block_id },
            ctx,
        ))
        .with_child(block_menu_item(
            format!("bmenu_copyboth:{block_id}"),
            AppIcon::Copy,
            "Copy command & output",
            TerminalAction::CopyBlockCommandAndOutput { block_id },
            ctx,
        ))
        .with_child(block_menu_divider(theme))
        .with_child(block_menu_item(
            format!("bmenu_top:{block_id}"),
            AppIcon::ArrowUp,
            "Scroll to top",
            TerminalAction::ScrollBlockToTop { block_id },
            ctx,
        ))
        .with_child(block_menu_item(
            format!("bmenu_bot:{block_id}"),
            AppIcon::ArrowDown,
            "Scroll to bottom",
            TerminalAction::ScrollBlockToBottom { block_id },
            ctx,
        ))
        .with_child(block_menu_divider(theme))
        .with_child(block_menu_item(
            format!("bmenu_filter:{block_id}"),
            AppIcon::Search,
            filter_label,
            TerminalAction::ToggleBlockFilter { block_id },
            ctx,
        ))
        .with_child(if running {
            block_menu_item(
                format!("bmenu_terminate:{block_id}"),
                AppIcon::XClose,
                "Terminate process",
                TerminalAction::TerminateBlock { block_id },
                ctx,
            )
        } else {
            Empty::new().finish()
        })
        .finish();

    let body = Container::new(col)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish();

    ConstrainedBox::new(body)
        .with_width(BLOCK_MENU_WIDTH)
        .finish()
}

/// Small action menu for a clicked link/path: "Open in Editor" + "Reveal in
/// Finder" for a file path, or "Open in Browser" + "Open in External
/// Browser" + "Copy Link" for a URL —
/// same idea as Warp's click-to-choose affordance, instead of a
/// single implicit action baked into the click itself.
pub fn build_link_options_menu(hint: &LinkHint, ctx: &BlockRenderContext<'_>) -> Box<dyn Element> {
    let theme = ctx.theme;
    let is_url = hint.url.starts_with("http://") || hint.url.starts_with("https://");
    // A trailing slash is how terminal output marks a folder (`ls -F`, git
    // status for untracked dirs); a folder has no editor destination.
    let is_folder = !is_url && hint.url.ends_with('/');
    let block_id = hint.block_id;
    let url = hint.url.clone();
    let item = |key: &str, icon: AppIcon, label: &str, action: TerminalAction| {
        block_menu_item(
            format!("lmenu_{key}:{block_id}:{url}"),
            icon,
            label,
            action,
            ctx,
        )
    };
    let open = TerminalAction::OpenLink {
        block_id,
        url: url.clone(),
    };
    let external = TerminalAction::OpenLinkExternal {
        block_id,
        url: url.clone(),
    };
    let reveal = TerminalAction::RevealLinkInFinder {
        block_id,
        url: url.clone(),
    };
    let copy = TerminalAction::CopyLinkTarget {
        block_id,
        url: url.clone(),
    };

    let items: Vec<Box<dyn Element>> = if is_url {
        vec![
            item("open", AppIcon::Share, "Open in Browser", open),
            item(
                "external",
                AppIcon::Globe,
                "Open in External Browser",
                external,
            ),
            item("copy", AppIcon::Copy, "Copy Link", copy),
        ]
    } else if is_folder {
        vec![
            item("reveal", AppIcon::FolderOpen, "Reveal in Finder", reveal),
            item("copy", AppIcon::Copy, "Copy Path", copy),
        ]
    } else {
        vec![
            item("open", AppIcon::Code, "Open in Editor", open),
            item("reveal", AppIcon::FolderOpen, "Reveal in Finder", reveal),
            item("copy", AppIcon::Copy, "Copy Path", copy),
        ]
    };
    let mut col = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for element in items {
        col = col.with_child(element);
    }
    let col = col.finish();

    let body = Container::new(col)
        .with_padding_top(4.0)
        .with_padding_bottom(4.0)
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
        .finish();

    ConstrainedBox::new(body)
        .with_width(BLOCK_MENU_WIDTH)
        .finish()
}

pub fn render_fix_button(
    block_id: usize,
    heal_state: &crate::agent::self_heal::BlockHealState,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    use crate::agent::self_heal::BlockHealState;
    let theme = ctx.theme;
    let mono = ctx.mono;
    let analyzing = matches!(heal_state, BlockHealState::Analyzing);
    let label = if analyzing { "Fix…" } else { "Fix" };
    let label_color = if analyzing {
        theme.disabled_text()
    } else {
        theme.accent
    };
    let mouse_state = ctx.mouse_state(&format!("fix_btn:{block_id}"));

    let btn = Hoverable::new(mouse_state, move |state| {
        let hovered = state.is_hovered() && !analyzing;
        let bg_color = if hovered {
            ColorU::new(label_color.r, label_color.g, label_color.b, 18)
        } else {
            ColorU::new(0, 0, 0, 0)
        };
        let border_color = if hovered {
            label_color
        } else {
            theme.outline()
        };

        Container::new(
            Text::new(label.to_string(), mono, crate::theme::text_size(12.5))
                .with_color(label_color)
                .with_style(warpui::fonts::Properties {
                    weight: warpui::fonts::Weight::Medium,
                    ..Default::default()
                })
                .finish(),
        )
        .with_padding_left(8.0)
        .with_padding_right(8.0)
        .with_padding_top(5.0)
        .with_padding_bottom(5.0)
        .with_background_color(bg_color)
        .with_border(Border::all(1.0).with_border_color(border_color))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .finish()
    });

    let btn: Box<dyn Element> = if analyzing {
        btn.finish()
    } else {
        btn.with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx.dispatch_typed_action(TerminalAction::AnalyzeBlock { block_id });
            })
            .finish()
    };
    btn
}

/// A block's working folder for its header, with the home directory shown as
/// `~` the way a shell prompt does.
pub(crate) fn display_cwd(cwd: &std::path::Path) -> String {
    // A remote project's folder is shown as the machine's own path, with the
    // machine's home as `~`, never as its local stand-in.
    if let Some(remote) = crate::remote_fs::resolve(cwd) {
        let home = crate::remote_fs::home(&remote.machine_id);
        return display_cwd_with_home(
            std::path::Path::new(&remote.path),
            home.as_deref().map(std::path::Path::new),
        );
    }
    display_cwd_with_home(
        cwd,
        std::env::var_os("HOME")
            .as_deref()
            .map(std::path::Path::new),
    )
}

fn display_cwd_with_home(cwd: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if cwd.as_os_str().is_empty() {
        return String::new();
    }
    if let Some(home) = home.filter(|h| !h.as_os_str().is_empty()) {
        if cwd == home {
            return "~".to_string();
        }
        if let Ok(rest) = cwd.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    cwd.display().to_string()
}

#[cfg(test)]
mod display_cwd_tests {
    use super::display_cwd_with_home;
    use std::path::Path;

    #[test]
    fn abbreviates_home_like_a_prompt() {
        let home = Some(Path::new("/Users/me"));
        assert_eq!(
            display_cwd_with_home(Path::new("/Users/me/dev/Terminal agent"), home),
            "~/dev/Terminal agent"
        );
        assert_eq!(display_cwd_with_home(Path::new("/Users/me"), home), "~");
        assert_eq!(display_cwd_with_home(Path::new("/tmp/x"), home), "/tmp/x");
        assert_eq!(
            display_cwd_with_home(Path::new("/Users/meow"), home),
            "/Users/meow"
        );
        assert_eq!(display_cwd_with_home(Path::new(""), home), "");
    }
}

pub fn build_block_header_widget(
    block: &Block,
    heal_state: &crate::agent::self_heal::BlockHealState,
    filter: Option<&BlockFilterState>,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mono = ctx.mono;
    let block_id = block.id;
    let block_command = block.command.clone();
    let expanded = block.expanded;
    let running = block.is_running();
    let stripe = stripe_color(block.status, theme);

    let status_dot = Container::new(
        ConstrainedBox::new(Empty::new().finish())
            .with_width(6.0)
            .with_height(6.0)
            .finish(),
    )
    .with_background_color(stripe)
    .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
        warpui_core::scene::Radius::Pixels(3.0),
    ))
    .finish();

    // Fixed-height header: the command is forced onto one line (no soft wrap)
    // and ellipsized at the available width — a long or multi-line command
    // must never grow the header. The copy button still copies the full text.
    let command_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        // SVG icon, not the `❯` character (U+276F): it is absent from Fira Code,
        // and the web build embeds ONLY Fira Code with no fallback provider — so
        // the glyph rendered as tofu (a box) there while looking fine on desktop,
        // which has system fonts to fall back on. The icon renders identically on
        // both and carries no font dependency.
        // No prompt marker. It used to be the `❯` character, which is absent from
        // Fira Code — the only font the web build embeds — so it rendered as a
        // tofu box there. Substituting the chevron SVG icon fixed the box but did
        // not match the original glyph's weight, and still rendered as a
        // placeholder on the web. The status dot and the command text already
        // identify the row, so the marker is simply dropped on both platforms.
        .with_child(Box::new(Expanded::new(
            1.0,
            Box::new(
                Text::new(
                    header_command_line(&block_command),
                    mono,
                    crate::theme::text_size(14.0),
                )
                .with_color(theme.main_text())
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .with_style(Properties {
                    weight: Weight::Medium,
                    ..Default::default()
                }),
            ),
        )))
        .finish();

    let mut right_side = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(7.0);

    right_side = right_side.with_child(render_copy_icon_btn(block_id, block_command.clone(), ctx));

    // While running, count up live from `started_at`; once completed, show
    // the final `duration_ms`. Either way this ticks/settles the same as a
    // non-running block once it finishes.
    let live_duration_ms = if running {
        block.started_at.map(|t| t.elapsed().as_millis() as u64)
    } else {
        block.duration_ms
    };
    if let Some(ms) = live_duration_ms {
        right_side = right_side.with_child(Box::new(
            Text::new(format_duration(ms), mono, crate::theme::text_size(11.5))
                .with_color(theme.disabled_text()),
        ));
    }

    if running {
        let spinner = Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(15.0)
                .with_height(15.0)
                .finish(),
        )
        .with_border(
            warpui_core::scene::Border::all(1.5)
                .with_border_color(ColorU::new(stripe.r, stripe.g, stripe.b, 180)),
        )
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(6.0),
        ))
        .finish();
        right_side = right_side.with_child(spinner);
    } else {
        let badge_text = match block.status {
            BlockStatus::Completed { exit_code: Some(0) }
            | BlockStatus::Completed { exit_code: None } => "ok",
            BlockStatus::Completed {
                exit_code: Some(code),
            } if is_signal_exit(code) => "–",
            BlockStatus::Completed { exit_code: Some(_) } => "err",
            BlockStatus::Running => "···",
            BlockStatus::Pending => "···",
        };
        let badge_bg = ColorU::new(stripe.r, stripe.g, stripe.b, 31);
        let badge_border = ColorU::new(stripe.r, stripe.g, stripe.b, 56);
        let badge_el = Container::new(
            Text::new(
                badge_text.to_string(),
                mono,
                crate::theme::text_size(11.0),
            )
            .with_color(stripe)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Default::default()
            })
            .finish(),
        )
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(badge_bg)
        .with_border(warpui_core::scene::Border::all(1.0).with_border_color(badge_border))
        .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
            warpui_core::scene::Radius::Pixels(3.0),
        ))
        .finish();
        right_side = right_side.with_child(badge_el);

        // Fix button — only on completed failed blocks (not signal exits).
        if is_error_status(block.status) {
            let fix_btn = render_fix_button(block_id, heal_state, ctx);
            right_side = right_side.with_child(fix_btn);
        }
    }

    // Block options (⋯) — always available, rightmost in the header.
    if !running {
        right_side = right_side.with_child(render_block_menu_btn(block_id, ctx));
    }

    let header_left = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(9.0)
        .with_child(status_dot)
        .with_child(Box::new(Expanded::new(1.0, command_row)))
        .finish();

    let header_row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Box::new(Expanded::new(1.0, header_left)))
        .with_child(right_side.finish())
        .finish();

    // The folder the command ran in, above the command — like a shell prompt
    // line — so a block's paths are readable without scrolling back to a `cd`.
    let mut header_stack = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::Center)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    let cwd_label = display_cwd(&block.cwd);
    if !cwd_label.is_empty() {
        header_stack = header_stack.with_child(
            Container::new(
                Text::new(cwd_label, mono, crate::theme::text_size(11.5))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .with_padding_left(15.0)
            .with_padding_bottom(4.0)
            .finish(),
        );
    }
    header_stack = header_stack.with_child(header_row);

    // Pinned to BLOCK_HEADER_HEIGHT: the block height index estimates every
    // header at that constant, so the extra folder line must fit inside it
    // rather than grow the header and desync scrolling/hit-testing.
    let header_body = ConstrainedBox::new(
        Container::new(header_stack.finish())
            .with_padding_left(10.0)
            .with_padding_right(13.0)
            .with_border(
                warpui_core::scene::Border::left(BLOCK_ACCENT_WIDTH).with_border_color(stripe),
            )
            .finish(),
    )
    .with_height(crate::terminal::render::BLOCK_HEADER_HEIGHT)
    .finish();

    // Inline output filter bar, shown under the header while this block's filter
    // is active. Rendered inline (not as a floating overlay) so it stays attached
    // to the block and its embedded editor gains focus on click.
    let header_content: Box<dyn Element> = match (filter, ctx.block_filter_editor) {
        (Some(f), Some(editor)) => {
            let match_count = count_block_matches(block, f, &ctx.theme);
            let bar = build_block_filter_bar(
                block_id,
                editor,
                f.is_empty(),
                f.use_regex,
                f.regex_error,
                match_count,
                ctx,
            );
            let bar_box = Container::new(bar)
                .with_padding_left(10.0)
                .with_padding_right(13.0)
                .with_padding_bottom(8.0)
                .with_border(
                    warpui_core::scene::Border::left(BLOCK_ACCENT_WIDTH).with_border_color(stripe),
                )
                .finish();
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(header_body)
                .with_child(bar_box)
                .finish()
        }
        _ => header_body,
    };

    Container::new(header_content)
        .with_border(warpui_core::scene::Border::top(1.0).with_border_color(theme.outline()))
        .finish()
}
