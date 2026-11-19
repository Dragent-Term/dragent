//! Renderer for agent conversation blocks (messages, approval cards, results).

use uuid::Uuid;
use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Empty, Expanded, Flex,
    Hoverable, MainAxisSize, ParentElement, SavePosition, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::markdown::{render_markdown, render_markdown_agent, strip_emoji_str};

use crate::agent::controller::block::{
    AgentBlock, AgentBlockKind, ApprovalStatus, ErrorKind, PlanStatus, QuestionItem,
    QuestionStatus, TodoListItem, ToolStepStatus,
};
use crate::terminal::render::agent_block_element::agent_block_card;
use crate::terminal::render::agent_block_element::agent_block_header;
use crate::terminal::render::agent_block_element::user_prompt_header;
use crate::terminal::render::BlockRenderContext;
use crate::terminal::view::actions::TerminalAction;
use crate::theme::Theme;

// ── Layout constants ──────────────────────────────────────────────────────────

pub(crate) fn build_user_query(
    block_id: usize,
    text: &str,
    author: Option<&crate::agent::controller::AgentAuthor>,
    attached_blocks: &[crate::agent::api::session::AttachedBlock],
    attachments: &[crate::agent::attachments::UserBlockAttachment],
    has_live_checkpoint: bool,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.disabled_text();

    // Copy button — every user message can be copied (parity with command blocks).
    let copy_key = format!("copy-msg-{block_id}");
    let copy_state = ctx.mouse_state(&copy_key);
    let copied = ctx.is_copied(&copy_key);
    let copy_text = text.to_string();
    let copy_btn: Box<dyn Element> = Box::new(
        Hoverable::new(copy_state, move |state| {
            let color = if state.is_hovered() {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            let (icon, color) =
                crate::terminal::view::copy_confirmation::copy_glyph(copied, color, theme);
            crate::icons::render_icon(icon, color, crate::icons::ICON_SIZE_SM)
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::CopyWithConfirmation {
                key: copy_key.clone(),
                text: copy_text.clone(),
            });
        }),
    );

    let mut actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0);
    // Restore only appears when the live store can actually back it up. A
    // persisted marker alone (e.g. after an app restart) is not sufficient.
    if has_live_checkpoint {
        let restore_state = ctx.mouse_state(&format!("restore-msg-{block_id}"));
        let restore_btn: Box<dyn Element> = Box::new(
            Hoverable::new(restore_state, move |state| {
                let color = if state.is_hovered() {
                    theme.accent
                } else {
                    theme.sub_text()
                };
                Container::new(crate::icons::render_icon(
                    crate::icons::AppIcon::Undo2,
                    color,
                    crate::icons::ICON_SIZE_SM,
                ))
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0)
                .with_background_color(if state.is_hovered() {
                    theme.surface_2()
                } else {
                    warpui::color::ColorU::transparent_black()
                })
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |event_ctx, _app, _pos| {
                event_ctx
                    .dispatch_typed_action(TerminalAction::ToggleCheckpointPreview { block_id });
            }),
        );
        let restore_btn =
            SavePosition::new(restore_btn, &format!("checkpoint_btn_{block_id}")).finish();
        actions = actions.with_child(restore_btn);
    }
    actions = actions.with_child(copy_btn);
    let label = author
        .map(|author| author.display_name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or("You");
    let avatar_url = author
        .map(|author| author.avatar_url.as_str())
        .unwrap_or("");
    let header = user_prompt_header(
        label,
        avatar_url,
        text,
        accent,
        actions.finish(),
        theme,
        mono,
    );

    let mut body_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(6.0);

    // Files sent with this message — image thumbnails and named chips — so the
    // prompt block records what the user attached and lets them view it again.
    if !attachments.is_empty() {
        use warpui_core::assets::asset_cache::AssetSource;
        use warpui_core::image_cache::CacheOption;
        let mut att_wrap = warpui::elements::Wrap::row()
            .with_main_axis_size(warpui::elements::MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Start)
            .with_spacing(6.0)
            .with_run_spacing(6.0);
        for att in attachments {
            let show_thumb = att.is_image && att.path.as_ref().map(|p| p.exists()).unwrap_or(false);
            if show_thumb {
                let path = att.path.as_ref().unwrap().to_string_lossy().into_owned();
                let thumb = warpui::elements::Image::new(
                    AssetSource::LocalFile { path },
                    CacheOption::Original,
                )
                .contain()
                .finish();
                let framed = Container::new(
                    warpui::elements::ConstrainedBox::new(thumb)
                        .with_width(46.0)
                        .with_height(46.0)
                        .finish(),
                )
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .finish();
                att_wrap = att_wrap.with_child(framed);
            } else {
                // Text file, or an image whose source file is gone → named chip.
                let chip = Container::new(
                    Text::new(
                        format!("\u{2261} {}", att.name),
                        mono,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .with_background_color(theme.surface_2())
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish();
                att_wrap = att_wrap.with_child(chip);
            }
        }
        body_col = body_col.with_child(att_wrap.finish());
    }

    if !attached_blocks.is_empty() {
        let mut attached_wrap = warpui::elements::Wrap::row()
            .with_main_axis_size(warpui::elements::MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Start)
            .with_spacing(6.0)
            .with_run_spacing(6.0);

        for block in attached_blocks {
            let block_id = block.block_id;
            let chip_content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.0)
                .with_child(crate::icons::render_icon(
                    crate::icons::AppIcon::Terminal,
                    theme.sub_text(),
                    crate::icons::ICON_SIZE_SM,
                ))
                .with_child(Box::new(
                    Text::new(format!("Block #{}", block_id), mono, crate::theme::text_size(13.0))
                        .with_color(theme.sub_text()),
                ))
                .finish();

            let chip = Container::new(chip_content)
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(2.0)
                .with_padding_bottom(2.0)
                .with_background_color(theme.surface_2())
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish();

            attached_wrap = attached_wrap.with_child(chip);
        }

        body_col = body_col.with_child(
            Container::new(attached_wrap.finish())
                .with_padding_top(4.0)
                .finish(),
        );
    }

    let body = (!attachments.is_empty() || !attached_blocks.is_empty()).then(|| {
        Container::new(body_col.finish())
            // Align supporting content with the prompt text, after the avatar.
            .with_padding_left(32.0)
            .with_padding_top(6.0)
            .finish()
    });

    agent_block_card(header, body, accent)
}

/// A prompt waiting its turn in the queue: a muted "Queued" pill with the
/// message preview and an × to remove it before it runs.
pub(crate) fn build_queued_message(
    queue_id: Uuid,
    text: &str,
    block_id: usize,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let accent = theme.sub_text();

    // "Queued" chip.
    let chip = Container::new(
        Text::new("QUEUED".to_string(), mono, crate::theme::text_size(9.5))
            .with_color(theme.accent)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Default::default()
            })
            .finish(),
    )
    .with_padding_left(6.0)
    .with_padding_right(6.0)
    .with_padding_top(2.0)
    .with_padding_bottom(2.0)
    .with_background_color(ColorU::new(
        theme.accent.r,
        theme.accent.g,
        theme.accent.b,
        28,
    ))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)))
    .finish();

    // × remove button.
    let remove_state = ctx.mouse_state(&format!("queued-x-{block_id}"));
    let remove_btn: Box<dyn Element> = Box::new(
        Hoverable::new(remove_state, move |state| {
            let color = if state.is_hovered() {
                theme.ui_red()
            } else {
                theme.sub_text()
            };
            crate::icons::render_icon(
                crate::icons::AppIcon::XClose,
                color,
                crate::icons::ICON_SIZE_SM,
            )
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(TerminalAction::CancelQueuedMessage { queue_id });
        }),
    );

    // Single-line preview of the queued prompt.
    let preview: String = {
        let one_line = text.replace('\n', " ");
        if one_line.chars().count() > 140 {
            let mut s: String = one_line.chars().take(140).collect();
            s.push('…');
            s
        } else {
            one_line
        }
    };

    let header_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(chip)
        .with_child(Box::new(Expanded::new(
            1.0,
            Text::new(preview, mono, crate::theme::text_size(13.5))
                .with_color(theme.disabled_text())
                .finish(),
        )))
        .with_child(remove_btn)
        .finish();

    let body = Container::new(header_row)
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(8.0)
        .with_padding_bottom(8.0)
        .finish();

    // Dashed-feel muted card (reuses the standard header/card frame in a quiet accent).
    let header = agent_block_header("Queued", accent, theme, mono);
    agent_block_card(header, Some(body), accent)
}

/// A system continuation (finished background command output fed to the
/// agent). Distinct from a user prompt: a muted "System" header, no restore
/// control, condensed body.
pub(crate) fn build_system_note(text: &str, theme: Theme, mono: FamilyId) -> Box<dyn Element> {
    let accent = theme.disabled_text();
    let header = agent_block_header("System", accent, theme, mono);

    // Condense: show the first line (usually the "[Background task … finished]"
    // banner) plus a short tail so the user can see the outcome at a glance.
    let condensed: String = {
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() <= 8 {
            text.to_string()
        } else {
            let head = lines[..4].join("\n");
            let tail = lines[lines.len() - 3..].join("\n");
            format!("{head}\n… ({} more lines) …\n{tail}", lines.len() - 7)
        }
    };

    let body = Container::new(
        Text::new(condensed, mono, crate::theme::text_size(12.5))
            .with_color(theme.disabled_text())
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(13.0)
    .with_padding_top(6.0)
    .with_padding_bottom(10.0)
    .finish();

    agent_block_card(header, Some(body), accent)
}

pub(crate) fn build_agent_message(
    text: &str,
    is_streaming: bool,
    block: &AgentBlock,
    ctx: &BlockRenderContext<'_>,
    theme: Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    use std::time::Duration;
    use warpui::elements::shimmering_text::{ShimmerConfig, ShimmeringTextElement};

    let accent = theme.accent;
    let header = agent_block_header("Dragent", accent, theme, mono);

    let mut body_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(6.0)
        .with_child(render_markdown_agent(
            text,
            block.id,
            ctx,
            theme,
            mono,
            !is_streaming,
        ));

    if is_streaming {
        let shimmer_handle = ctx.shimmer_state(&format!("msg-stream-{}", block.id));
        let cursor = ShimmeringTextElement::new(
            "▊".to_string(),
            mono,
            crate::theme::text_size(14.0),
            theme.sub_text(),
            theme.main_text(),
            ShimmerConfig {
                period: Duration::from_millis(800),
                shimmer_radius: 2,
                padding: 0,
            },
            shimmer_handle,
        )
        .finish();
        body_col = body_col.with_child(cursor);
    }

    let body = Container::new(body_col.finish())
        .with_padding_left(16.0)
        .with_padding_right(13.0)
        .with_padding_top(6.0)
        .with_padding_bottom(10.0)
        .finish();

    agent_block_card(header, Some(body), accent)
}
