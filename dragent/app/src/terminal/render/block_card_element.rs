//! Sticky block header overlay for the virtualized block list.

use warpui::color::ColorU;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex, Hoverable,
    MainAxisSize, ParentElement, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use crate::icons::{render_icon, AppIcon};
use crate::terminal::model::block::BlockStatus;
use crate::terminal::model::is_signal_exit;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::{block_copy_key, TerminalAction};
use crate::terminal::view::copy_confirmation::copy_glyph;
use crate::theme::Theme;

const BLOCK_ACCENT_WIDTH: f32 = 3.0;

/// Visual height of a block header: padding 22+22 around the tallest right-side
/// control (the ⋯/copy icon buttons ≈ 26px) + top-border 1 ≈ 71. Must stay in
/// sync with the header container padding below and with `BLOCK_HEADER_HEIGHT_PX`
/// in `block_height_tree.rs`.
pub const BLOCK_HEADER_HEIGHT: f32 = 70.0;

/// Lightweight snapshot of the fields needed to render a sticky block header overlay.
#[derive(Clone)]
pub struct StickyBlockHeader {
    pub id: usize,
    pub command: String,
    pub status: BlockStatus,
    pub duration_ms: Option<u64>,
    pub expanded: bool,
    pub running: bool,
    pub heal_state: crate::agent::self_heal::BlockHealState,
    // `web_time::Instant` is `web_time::Instant` on native and a browser clock
    // on wasm; matches `Block`'s timing fields so this compiles on both.
    pub started_at: Option<web_time::Instant>,
}

/// Collapse a command to ONE header line: all whitespace (including newlines
/// from heredocs / `\` continuations) becomes single spaces so a long command
/// can never grow the header's height, and a hard cap keeps pathological
/// commands cheap to lay out. Horizontal overflow is handled by the Text
/// element's ellipsis clip, not here.
pub(crate) fn header_command_line(command: &str) -> String {
    let mut one_line = command.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 300;
    if one_line.chars().count() > MAX_CHARS {
        one_line = one_line.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    one_line
}

fn with_alpha(color: ColorU, alpha: f32) -> ColorU {
    ColorU::new(
        color.r,
        color.g,
        color.b,
        (color.a as f32 * alpha.clamp(0.0, 1.0)) as u8,
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

        Container::new(render_icon(AppIcon::Copy, icon_color, 11.0))
            .with_padding_left(5.0)
            .with_padding_right(5.0)
            .with_padding_top(3.0)
            .with_padding_bottom(3.0)
            .with_background_color(bg_color)
            .with_border(Border::all(1.0).with_border_color(border_color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |event_ctx, _app, _pos| {
        event_ctx.dispatch_typed_action(TerminalAction::CopyText {
            text: command.clone(),
        });
    })
    .finish()
}

/// Build the sticky header overlay for a block that has scrolled above the viewport top.
pub fn build_sticky_block_header(
    info: &StickyBlockHeader,
    ctx: &BlockRenderContext<'_>,
) -> Box<dyn Element> {
    let theme = ctx.theme;
    let mono = ctx.mono;
    let block_id = info.id;
    let block_command = info.command.clone();
    let stripe = stripe_color(info.status, theme);
    let running = info.running;
    let expanded = info.expanded;
    let duration_ms = info.duration_ms;
    let status = info.status;
    let heal_state = info.heal_state.clone();
    let started_at = info.started_at;

    let header_mouse_state = ctx.mouse_state(&format!("sticky_header:{block_id}"));

    let header_el = Hoverable::new(header_mouse_state, move |state| {
        let hovered = state.is_hovered();

        let status_dot = Container::new(
            ConstrainedBox::new(Empty::new().finish())
                .with_width(6.0)
                .with_height(6.0)
                .finish(),
        )
        .with_background_color(stripe)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
        .finish();

        // Fixed-height header: the command is forced onto one line (no soft
        // wrap) and ellipsized at the available width — it must never grow
        // the sticky header as the command grows.
        let command_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            // No prompt marker — see the note in `block_list_element`.
            .with_child(Box::new(Expanded::new(
                1.0,
                Box::new(
                    Text::new(
                        header_command_line(&block_command),
                        mono,
                        crate::theme::text_size(12.5),
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

        // Copy button — same as card header, appears on hover. It stays while
        // confirming a copy, which may have come from the options menu with
        // the pointer elsewhere.
        let copy_key = block_copy_key(block_id);
        let copied = ctx.is_copied(&copy_key);
        if hovered || copied {
            let copy_mouse = ctx.mouse_state(&format!("sticky_copy_btn:{block_id}"));
            let cmd_for_copy = block_command.clone();
            let copy_btn = Hoverable::new(copy_mouse, move |btn_state| {
                let btn_hovered = btn_state.is_hovered();
                let (bg_color, border_color, icon_color) = if btn_hovered {
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
                    key: copy_key.clone(),
                    text: cmd_for_copy.clone(),
                });
            })
            .finish();
            right_side = right_side.with_child(copy_btn);
        }

        // While running, count up live from `started_at`; once completed, show
        // the final `duration_ms` — mirrors the in-flow card header.
        let live_duration_ms = if running {
            started_at.map(|t| t.elapsed().as_millis() as u64)
        } else {
            duration_ms
        };
        if let Some(ms) = live_duration_ms {
            right_side = right_side.with_child(Box::new(
                Text::new(format_duration(ms), mono, crate::theme::text_size(11.5))
                    .with_color(theme.disabled_text()),
            ));
        }

        if running {
            right_side = right_side.with_child(
                Container::new(
                    ConstrainedBox::new(Empty::new().finish())
                        .with_width(15.0)
                        .with_height(15.0)
                        .finish(),
                )
                .with_border(
                    Border::all(1.5)
                        .with_border_color(ColorU::new(stripe.r, stripe.g, stripe.b, 180)),
                )
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish(),
            );
        } else {
            let badge_text = match status {
                BlockStatus::Completed { exit_code: Some(0) }
                | BlockStatus::Completed { exit_code: None } => "ok",
                BlockStatus::Completed {
                    exit_code: Some(code),
                } if is_signal_exit(code) => "–",
                BlockStatus::Completed { exit_code: Some(_) } => "err",
                BlockStatus::Running | BlockStatus::Pending => "···",
            };
            let badge_bg = ColorU::new(stripe.r, stripe.g, stripe.b, 31);
            let badge_border = ColorU::new(stripe.r, stripe.g, stripe.b, 56);
            right_side = right_side.with_child(
                Container::new(
                    Text::new(badge_text.to_string(), mono, crate::theme::text_size(11.0))
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
                .with_border(Border::all(1.0).with_border_color(badge_border))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
                .finish(),
            );

            // Fix button — mirrors the card header's, so it stays reachable
            // for a failed block once its header has scrolled up and pinned.
            if crate::terminal::render::block_list_element::is_error_status(status) {
                let fix_btn = crate::terminal::render::block_list_element::render_fix_button(
                    block_id,
                    &heal_state,
                    ctx,
                );
                right_side = right_side.with_child(fix_btn);
            }
        }

        // Block options (⋯) — mirrors the card header so the menu is reachable
        // while the header is pinned. Distinct mouse-state key from the card
        // header's button so their hover states don't collide when both exist.
        if !running {
            let menu_mouse = ctx.mouse_state(&format!("sticky_block_menu_btn:{block_id}"));
            let menu_btn = Hoverable::new(menu_mouse, move |btn_state| {
                let btn_hovered = btn_state.is_hovered();
                let (bg_color, border_color, icon_color) = if btn_hovered {
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
            .finish();
            right_side = right_side.with_child(menu_btn);
        }

        let header_left = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
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

        let header_body = Container::new(header_row)
            .with_padding_left(10.0)
            .with_padding_right(13.0)
            .with_padding_top(22.0)
            .with_padding_bottom(22.0)
            .with_border(Border::left(BLOCK_ACCENT_WIDTH).with_border_color(stripe))
            .finish();

        let bordered = Container::new(
            Container::new(header_body)
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .finish(),
        )
        .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
        .finish();

        // The header floats over scrolled output, so it must never be
        // see-through — but under a background-image theme a flat fill read as a
        // solid slab against the wallpaper. Paint the REAL backdrop instead, the
        // same layering the workspace uses and the history popover now uses:
        // opaque base (hides the output beneath) → the theme image at its
        // designed opacity → the translucent `theme.background` fill that every
        // other pane surface sits behind → the content.
        //
        // The image is CLIPPED and given an explicit height: `cover()` scales it
        // larger than its box by definition, so unclipped it paints outside the
        // header, and under a loose constraint it would expand to fill whatever
        // it was given. `BLOCK_HEADER_HEIGHT_PX` is the height the viewport
        // already reserves for this header, so the two agree by construction.
        match crate::theme::Theme::bg_image_for(theme.id) {
            Some(bg) => {
                let image = warpui_core::elements::Clipped::new(
                    ConstrainedBox::new(
                        warpui::elements::Image::new(
                            bg.asset_source(),
                            warpui::elements::CacheOption::Original,
                        )
                        .cover()
                        .with_opacity((bg.opacity as f32 / 100.0).clamp(0.0, 1.0))
                        .finish(),
                    )
                    .with_height(crate::terminal::model::BLOCK_HEADER_HEIGHT_PX)
                    .finish(),
                )
                .finish();
                let tinted = Container::new(bordered)
                    .with_background_color(theme.background)
                    .finish();
                Container::new(Box::new(Stack::new().with_child(image).with_child(tinted)))
                    .with_background_color(ColorU::new(
                        theme.background.r,
                        theme.background.g,
                        theme.background.b,
                        255,
                    ))
                    .finish()
            }
            // A solid theme has no image to repaint. Use its actual pane
            // background so the pinned header is continuous with the terminal
            // instead of appearing as a raised `surface_2` strip.
            None => Container::new(bordered)
                .with_background_color(theme.background)
                .finish(),
        }
    })
    .with_defer_events_to_children()
    .finish();

    header_el
}

#[cfg(test)]
mod header_line_tests {
    use super::header_command_line;

    #[test]
    fn collapses_newlines_and_runs_of_whitespace() {
        assert_eq!(
            header_command_line("docker run \\\n  -it \\\n  --rm   ubuntu"),
            "docker run \\ -it \\ --rm ubuntu"
        );
        assert_eq!(header_command_line("ls -la"), "ls -la");
    }

    #[test]
    fn caps_pathological_length_with_ellipsis() {
        let long = "x".repeat(10_000);
        let line = header_command_line(&long);
        assert_eq!(line.chars().count(), 301);
        assert!(line.ends_with('…'));
    }
}
