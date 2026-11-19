//! Native overlay toolbar/input panel shown when Claude Code is running in alt-screen mode.
//!
//! Rendered as a positioned stack child inside a `Stack` — it floats over the raw
//! terminal canvas without altering the PTY viewport dimensions.

use warpui::color::ColorU;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element,
    EventHandler, Expanded, Flex, Image, MainAxisAlignment, MainAxisSize, ParentElement, Text,
    Wrap,
};
use warpui::fonts::FamilyId;
use warpui::ViewHandle;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::workspace::AgentImage;

/// Chip icon size. The chip labels are `scaled(11.0)` and a text line box is
/// ~1.2x its font size, so a 12px icon left the icon-only chips (attach, mic)
/// visibly shorter than the ones with labels. Sized to the label's line box and
/// scaled the same way, so both kinds of chip share one height at any font
/// setting.
fn chip_icon_size() -> f32 {
    crate::theme::scaled(13.5)
}
use crate::terminal::render::AiChromeColors;
use crate::theme::{Theme, INPUT_BOX_MARGIN, INPUT_BOX_RADIUS, INPUT_FONT_SIZE};
use crate::workspace::InputEditor;

const RICH_INPUT_MAX_H: f32 = 160.0;

/// Chip click targets for [`render_ai_input_panel`]. Pass `ConsoleAction` in the
/// workspace console or `TerminalAction` in the standalone terminal view.
#[derive(Clone, Debug)]
pub struct AiPanelActions<A: Clone + std::fmt::Debug + Send + Sync + 'static> {
    pub open_file_picker: A,
    pub toggle_rich_input: A,
    pub toggle_voice_input: A,
    /// The tab can show this agent as a chat: whether it does now, and the
    /// actions that switch to the chat and to the terminal.
    pub view_switch: Option<(bool, A, A)>,
    /// Shown as a chat: the round Send button at the end of the input,
    /// which turns into Stop while the agent works.
    pub send: Option<SendButton<A>>,
}

/// The chat's Send / Stop button.
#[derive(Clone, Debug)]
pub struct SendButton<A> {
    /// The agent is working: the button stops it.
    pub busy: bool,
    /// There is something to send (dims the button when not).
    pub has_text: bool,
    pub send: A,
    pub stop: A,
}

/// Helper — build a small static chip element.
fn static_image_chip(
    source: AssetSource,
    label: &str,
    text_color: ColorU,
    chip_bg: ColorU,
    border: ColorU,
    mono: FamilyId,
) -> Box<dyn Element> {
    let mut inner = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            ConstrainedBox::new(
                Image::new(source, CacheOption::BySize)
                    .contain()
                    .finish(),
            )
            .with_width(chip_icon_size())
            .with_height(chip_icon_size())
            .finish(),
        );

    if !label.is_empty() {
        inner = inner.with_spacing(4.0).with_child(Box::new(
            Text::new(label.to_string(), mono, crate::theme::text_size(11.0)).with_color(text_color),
        ));
    }

    Container::new(inner.finish())
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(chip_bg)
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
}
fn static_chip(
    icon: AppIcon,
    label: &str,
    icon_color: ColorU,
    text_color: ColorU,
    chip_bg: ColorU,
    border: ColorU,
    mono: FamilyId,
) -> Box<dyn Element> {
    let mut inner = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(render_icon(icon, icon_color, chip_icon_size()));

    if !label.is_empty() {
        inner = inner.with_spacing(4.0).with_child(Box::new(
            Text::new(label.to_string(), mono, crate::theme::text_size(11.0)).with_color(text_color),
        ));
    }

    Container::new(inner.finish())
        // +1px over the old 4/2 so the glyph isn't crowded against the border.
        .with_padding_left(5.0)
        .with_padding_right(5.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(chip_bg)
        // Outlined to match the terminal input's chips (`input_bar::make_chip`):
        // a bare fill reads as decoration, a bordered one reads as a control.
        .with_border(Border::all(1.0).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish()
}

/// Round Send button (an arrow), a square Stop while the agent works.
fn send_button<A: Clone + std::fmt::Debug + Send + Sync + 'static>(button: SendButton<A>, theme: &Theme) -> Box<dyn Element> {
    let accent = theme.accent;
    let glyph = theme.background;
    let (inner, action): (Box<dyn Element>, A) = if button.busy {
        let square = Container::new(warpui::elements::Empty::new().finish())
            .with_background_color(glyph)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(2.0)))
            .finish();
        (ConstrainedBox::new(square).with_width(11.0).with_height(11.0).finish(), button.stop)
    } else {
        (render_icon(AppIcon::ArrowUp, glyph, 15.0), button.send)
    };
    let bg = if button.busy || button.has_text { accent } else { ColorU { a: 110, ..accent } };
    let body = ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_main_axis_alignment(MainAxisAlignment::Center)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(inner)
                .finish(),
        )
        .with_background_color(bg)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
        .finish(),
    )
    .with_width(28.0)
    .with_height(28.0)
    .finish();
    EventHandler::new(body)
        .on_left_mouse_down(move |event_ctx, _, _| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// "CLI | Chat": one outlined chip with the current view filled.
pub(crate) fn view_switch_chip<A: Clone + std::fmt::Debug + Send + Sync + 'static>(
    chat: bool,
    show_chat: A,
    show_terminal: A,
    chrome: AiChromeColors,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let segment = |label: &'static str, selected: bool, action: A| -> Box<dyn Element> {
        let theme = *theme;
        let cell = Container::new(Box::new(
            Text::new(label, mono, crate::theme::text_size(11.0))
                .with_color(if selected { theme.accent } else { theme.sub_text() }),
        ))
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(if selected { chrome.chip_active_bg } else { chrome.chip_bg })
        .finish();
        EventHandler::new(cell)
            .on_left_mouse_down(move |event_ctx, _, _| {
                event_ctx.dispatch_typed_action(action.clone());
                DispatchEventResult::StopPropagation
            })
            .finish()
    };
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(segment("CLI", !chat, show_terminal))
            .with_child(segment("Chat", chat, show_chat))
            .finish(),
    )
    .with_border(Border::all(1.0).with_border_color(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
    .finish()
}

/// Helper — build a small clickable chip element.
fn interactive_chip<A: Clone + std::fmt::Debug + Send + Sync + 'static>(
    icon: AppIcon,
    label: &str,
    action: A,
    active: bool,
    chrome: AiChromeColors,
    theme: &Theme,
    mono: FamilyId,
) -> Box<dyn Element> {
    let bg = if active {
        chrome.chip_active_bg
    } else {
        chrome.chip_bg
    };
    let icon_color = if active {
        theme.accent
    } else {
        theme.sub_text()
    };
    let text_color = if active {
        theme.accent
    } else {
        theme.sub_text()
    };

    let mut inner = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(render_icon(icon, icon_color, chip_icon_size()));

    if !label.is_empty() {
        inner = inner.with_spacing(4.0).with_child(Box::new(
            Text::new(label.to_string(), mono, crate::theme::text_size(11.0)).with_color(text_color),
        ));
    }

    let chip = Container::new(inner.finish())
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(bg)
        // Outlined like the terminal input's chips (`input_bar::make_chip`).
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
        .finish();

    EventHandler::new(chip)
        .on_left_mouse_down(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

/// The mic chip — same affordance as the main input bar's mic button: while
/// permissions/token/socket are in flight the mic glyph is replaced by a
/// spinner (no text — the motion is the message), turning red once capture is
/// actually live.
fn voice_chip<A: Clone + std::fmt::Debug + Send + Sync + 'static>(
    action: A,
    state: crate::voice::VoiceUiState,
    spin: usize,
    chrome: AiChromeColors,
    theme: &Theme,
    _mono: FamilyId,
) -> Box<dyn Element> {
    use crate::voice::VoiceUiState;
    let icon_color = match state {
        VoiceUiState::Idle => theme.sub_text(),
        VoiceUiState::Connecting => theme.accent,
        VoiceUiState::Recording => theme.ui_red(),
    };

    let glyph = if state == VoiceUiState::Connecting {
        crate::icons::render_spinner(spin, icon_color, chip_icon_size())
    } else {
        render_icon(AppIcon::Microphone, icon_color, chip_icon_size())
    };
    let inner = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(glyph);

    let mut chip = Container::new(inner.finish())
        .with_padding_left(7.0)
        .with_padding_right(7.0)
        .with_padding_top(3.0)
        .with_padding_bottom(3.0)
        .with_background_color(chrome.chip_bg)
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
    if state != VoiceUiState::Idle {
        chip = chip.with_border(Border::all(1.0).with_border_color(icon_color));
    }

    EventHandler::new(chip.finish())
        .on_left_mouse_down(move |event_ctx, _app, _pos| {
            event_ctx.dispatch_typed_action(action.clone());
            DispatchEventResult::StopPropagation
        })
        .finish()
}

// Single source of truth for AI tool identity lives with the input-mode state
// machine; re-exported here so existing `ai_toolbar::AiTool` paths keep working.
pub use crate::terminal::model::input_mode::AiTool;

/// Brand icon for a tool's chip; tools without a bundled logo get the
/// terminal glyph (label carries the identity).
pub fn brand_icon(tool: AiTool) -> AppIcon {
    match tool {
        AiTool::Claude => AppIcon::ClaudeLogo,
        AiTool::Codex => AppIcon::ChatgptLogo,
        AiTool::Ollama => AppIcon::OllamaLogo,
        _ => AppIcon::Terminal,
    }
}

/// Unified input panel shown when an AI tool is active.
///
/// If `rich_input_active` is true, the panel renders in the expanded state with the
/// multi-line text editor and shortcuts hint. Otherwise, it collapses to render only
/// the row of status and action chips.
///
/// Pass `panel_width: None` when the panel sits in a stretch column (block mode) so
/// it fills the available width. Pass `Some(w)` for alt-screen overlays that need an
/// explicit pixel width.
pub fn render_ai_input_panel<A: Clone + std::fmt::Debug + Send + Sync + 'static>(
    rich_editor: &ViewHandle<InputEditor>,
    theme: &Theme,
    chrome: AiChromeColors,
    mono: FamilyId,
    git_status: Option<&crate::console::git_status::GitStatus>,
    cwd: Option<&std::path::Path>,
    rich_input_active: bool,
    // True while a `/`-triggered live-proxy session is forwarding every
    // keystroke straight to the wrapped CLI's own prompt (see
    // `TerminalAction::ForwardRichInputProxyKey`) — swaps the hint line to
    // say so instead of the normal compose-mode shortcuts.
    rich_input_proxy_active: bool,
    voice_state: crate::voice::VoiceUiState,
    // Spinner frame, only read while `voice_state` is `Connecting`.
    voice_spin: usize,
    input_notice: Option<&str>,
    panel_width: Option<f32>,
    ai_tool: AiTool,
    // Registry artwork for this session (resolved from its `agent_id` or
    // running command). The brand glyph below is only a fallback — the UI
    // never hard-codes per-agent images here.
    brand_image: Option<AgentImage>,
    actions: &AiPanelActions<A>,
    // The model selector, when the tab's chat talks to its agent directly
    // and can switch its model.
    model_selector: Option<Box<dyn Element>>,
) -> Box<dyn Element> {
    let mut chips_row = Wrap::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::Start)
        .with_spacing(6.0)
        .with_run_spacing(6.0);

    // The agent shows as its icon alone; its name is in the tab already, and
    // the room goes to the CLI / Chat switch beside it.
    let (brand_icon, brand_label) = (brand_icon(ai_tool), "");

    // 1. AI Tool Brand chip (static): registry artwork when the session
    // resolved one, otherwise the shared brand glyph.
    let brand_chip: Box<dyn Element> = match brand_image {
        Some(AgentImage::Bundled(path)) => static_image_chip(
            AssetSource::Bundled { path },
            brand_label,
            theme.accent,
            chrome.chip_bg,
            theme.outline(),
            mono,
        ),
        Some(AgentImage::File(path)) => static_image_chip(
            AssetSource::LocalFile { path },
            brand_label,
            theme.accent,
            chrome.chip_bg,
            theme.outline(),
            mono,
        ),
        None => static_chip(
            brand_icon,
            brand_label,
            theme.accent,
            theme.accent,
            chrome.chip_bg,
            theme.outline(),
            mono,
        ),
    };
    chips_row = chips_row.with_child(brand_chip);
    if let Some((chat, show_chat, show_terminal)) = actions.view_switch.clone() {
        chips_row = chips_row.with_child(view_switch_chip(chat, show_chat, show_terminal, chrome, theme, mono));
    }
    if let Some(selector) = model_selector {
        chips_row = chips_row.with_child(selector);
    }


    // 2. Icon-only chips are kept TOGETHER — attach and mic side by side rather
    //    than split apart by the labelled chips, so the bare glyphs read as one
    //    group of controls instead of scattered dots along the row.
    chips_row = chips_row.with_child(interactive_chip(
        AppIcon::Plus,
        "",
        actions.open_file_picker.clone(),
        false,
        chrome,
        theme,
        mono,
    ));

    // Voice chip (clickable, triggers ToggleVoiceInput). Same treatment as the
    // main input bar's mic: hidden on unsupported platforms, red while recording.
    if crate::voice::supported() {
        chips_row = chips_row.with_child(voice_chip(
            actions.toggle_voice_input.clone(),
            voice_state,
            voice_spin,
            chrome,
            theme,
            mono,
        ));
    }

    // 3. Rich Input chip (clickable, active if rich_input_active)
    chips_row = chips_row.with_child(interactive_chip(
        AppIcon::MessageSquare,
        "Rich Input  ^G",
        actions.toggle_rich_input.clone(),
        rich_input_active,
        chrome,
        theme,
        mono,
    ));

    let _ = cwd; // path chip removed by user request
    let _ = git_status; // branch chip removed by user request

    // Chips header content: the chip row plus the one-shot notice line
    // (voice/attachment errors) — the same notice the main input bar shows.
    let mut header_col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(chips_row.finish());
    if let Some(notice) = input_notice {
        header_col = header_col.with_child(
            Container::new(crate::console::input_bar::render_token_hint_row(
                notice, theme, mono,
            ))
            .with_padding_top(6.0)
            .finish(),
        );
    }
    let header_content = header_col.finish();

    let input_container: Box<dyn Element> = if rich_input_active {
        // Expanded State: chips row + editor row + hint
        let prompt_glyph = Container::new(render_icon(
            AppIcon::ChevronRight,
            theme.accent,
            INPUT_FONT_SIZE,
        ))
        .with_padding_right(8.0)
        .finish();

        let editor_child = ChildView::new(rich_editor).finish();

        let mut editor_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_child(prompt_glyph)
            .with_child(Box::new(Expanded::new(1.0, editor_child)));
        if let Some(button) = actions.send.clone() {
            editor_row = editor_row.with_child(
                Container::new(send_button(button, theme)).with_padding_left(8.0).finish(),
            );
        }
        let editor_row = editor_row.finish();

        let editor_constrained = ConstrainedBox::new(editor_row)
            .with_max_height(RICH_INPUT_MAX_H)
            .finish();

        // Only a MODE hint is shown. The permanent keybinding list that used to
        // live here (newline / send / !cmd / history / suggestion / Esc) was a
        // wall of text under every message — reference material, not something
        // you need on screen while typing. It lives in the user guide instead.
        // Proxy mode still announces itself, because that IS transient state:
        // keystrokes are going somewhere non-obvious and there has to be a way
        // back out.
        let hint_container = rich_input_proxy_active.then(|| {
            let hint = Text::new(
                format!(
                    "Typing directly in {}'s own prompt — Esc to cancel",
                    ai_tool.label()
                ),
                mono,
                crate::theme::text_size(10.0),
            )
            .with_color(theme.disabled_text());
            Container::new(Box::new(hint))
                .with_padding_top(4.0)
                .finish()
        });

        if panel_width.is_some() {
            let chips_header = Container::new(header_content)
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_border(Border::bottom(1.0).with_border_color(chrome.border))
                .finish();

            let editor_container = Container::new(editor_constrained)
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(12.0)
                .with_padding_bottom(12.0)
                .finish();

            let mut col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(chips_header)
                .with_child(editor_container);
            if let Some(hint_container) = hint_container {
                col = col.with_child(
                    Container::new(hint_container)
                        .with_background_color(theme.pane_surface())
                        .with_padding_left(12.0)
                        .with_padding_right(12.0)
                        .with_padding_bottom(12.0)
                        .finish(),
                );
            }
            let col = col.finish();

            Container::new(col)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(INPUT_BOX_RADIUS)))
                .with_border(Border::all(1.0).with_border_color(chrome.border))
                .finish()
        } else {
            let chips_header = Container::new(header_content)
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .with_border(Border::top(1.0).with_border_color(chrome.border))
                .finish();

            let editor_container = Container::new(editor_constrained)
                .with_background_color(theme.pane_surface())
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(12.0)
                .with_padding_bottom(12.0)
                .finish();

            let mut col = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(chips_header)
                .with_child(editor_container);
            if let Some(hint_container) = hint_container {
                col = col.with_child(
                    Container::new(hint_container)
                        .with_background_color(theme.pane_surface())
                        .with_padding_left(12.0)
                        .with_padding_right(12.0)
                        .with_padding_bottom(12.0)
                        .finish(),
                );
            }
            let col = col.finish();

            // Opaque fill on the OUTERMOST wrapper too, not just the rows inside.
            // Without it this container was see-through, so anything the rows did
            // not themselves cover — inter-row gaps, and the whole band while the
            // layout is mid-change — let the terminal output show through. On
            // mobile that is constant: opening the keyboard shrinks the visual
            // viewport, the content behind reflows, and it slid visibly under the
            // AI input text.
            Container::new(col)
                .with_background_color(theme.pane_surface())
                .finish()
        }
    } else {
        // Collapsed State: chips row only
        //
        // Fills are `pane_surface()`, exactly like the command input bar: under a
        // background-image theme it is TRANSPARENT so the theme image — painted
        // once at the workspace level behind every pane — shows through here as
        // it does everywhere else. Painting an opaque colour (or a second copy of
        // the image) made this bar read as a slab against the wallpaper.
        let mut panel = Container::new(header_content)
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_background_color(theme.pane_surface());

        if panel_width.is_some() {
            panel = panel
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(INPUT_BOX_RADIUS)))
                .with_border(Border::all(1.0).with_border_color(chrome.border));
        } else {
            panel = panel.with_border(Border::top(1.0).with_border_color(chrome.border));
        }
        panel.finish()
    };

    // Responsive panel container wrapping the input container.
    let sized_panel = match panel_width {
        Some(width) => ConstrainedBox::new(input_container)
            .with_width(width)
            .finish(),
        None => input_container,
    };

    let (margin_l, margin_r) = if panel_width.is_some() {
        (INPUT_BOX_MARGIN, INPUT_BOX_MARGIN)
    } else {
        (0.0, 0.0)
    };

    // NO bottom margin, in either variant. A margin is not painted, so it left a
    // bare strip between the panel and the window edge showing whatever was
    // behind — the same fault the command input bar fixed by dropping its own.
    // The side insets stay on the floating variant (they frame the rounded card
    // horizontally); only the bottom gap is wrong, because there is nothing below
    // the input to inset from.
    let margin_b = 0.0;
    Container::new(sized_panel)
        .with_margin_left(margin_l)
        .with_margin_right(margin_r)
        .with_margin_bottom(margin_b)
        .finish()
}
