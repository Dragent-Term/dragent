//! Appearance — theme gallery, text typography, and cursor style.
//!
//! Follows the standard platform conventions (theme, fonts,
//! cursor blink/shape) for everything that changes how the app looks
//! lives here. Terminal *behavior* (shell, keybindings, autodetection) stays
//! on the Terminal page. All controls dispatch the shared [`SettingsAction`]
//! set, so state stays in one place no matter which page renders them.

use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, MainAxisSize,
    ParentElement, Scrollable, ScrollableElement, ScrollbarWidth, Text, UniformList,
};
use warpui_core::scene::{CornerRadius, Radius};

use crate::console::editor_settings::{
    CursorStyle, CURSOR_BLINK_RATE_MAX_MS, CURSOR_BLINK_RATE_MIN_MS, FONT_SIZE_MAX, FONT_SIZE_MIN,
    LINE_HEIGHT_PCT_MAX, LINE_HEIGHT_PCT_MIN,
};
use crate::theme::Theme;
use warpui::fonts::Weight;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, link_button, page_header, segmented_control,
    select_control_with_scroll, setting_group, setting_row, slider_control, theme_row, toggle_row,
    SegmentOption, SelectItem,
};
use warpui_core::elements::Hoverable;

/// Bundled Inter (the default) first, then the macOS system font, then every
/// installed family (discovered when the Appearance page opens).
fn interface_font_options(view: &SettingsView) -> Vec<(Option<String>, String)> {
    let mut opts: Vec<(Option<String>, String)> = vec![(
        None,
        format!("{} (default)", crate::fonts::DEFAULT_UI_FONT_LABEL),
    )];
    #[cfg(target_os = "macos")]
    opts.push((
        Some(crate::fonts::SYSTEM_UI_FONT_LABEL.to_string()),
        format!("{} (system)", crate::fonts::SYSTEM_UI_FONT_LABEL),
    ));
    for family in &view.available_ui_fonts {
        let listed = opts.iter().any(|(value, _)| {
            value
                .as_deref()
                .unwrap_or(crate::fonts::DEFAULT_UI_FONT_LABEL)
                .eq_ignore_ascii_case(family)
        });
        if !listed {
            opts.push((Some(family.clone()), family.clone()));
        }
    }
    opts
}

/// Bundled families are always available; installed monospace families are
/// discovered when the Appearance page opens.
fn font_options(view: &SettingsView) -> Vec<(Option<String>, String)> {
    let mut opts: Vec<(Option<String>, String)> = vec![
        (
            None,
            format!("{} (default)", crate::fonts::DEFAULT_TERMINAL_FONT_LABEL),
        ),
        (Some("Fira Code".to_string()), "Fira Code".to_string()),
        (
            Some("JetBrains Mono".to_string()),
            "JetBrains Mono".to_string(),
        ),
    ];
    for family in &view.available_mono_fonts {
        if family != crate::fonts::DEFAULT_TERMINAL_FONT_LABEL {
            opts.push((Some(family.clone()), family.clone()));
        }
    }
    opts
}

pub fn render_appearance(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let prefs = &view.editor_settings.terminal;

    // ── Text ────────────────────────────────────────────────────────────────
    let current_ui_font_label = prefs
        .ui_font_family
        .as_deref()
        .unwrap_or(crate::fonts::DEFAULT_UI_FONT_LABEL)
        .to_string();
    let ui_font_select = select_control_with_scroll(
        theme,
        ui_font,
        id,
        view.hover("ui_font_btn"),
        &current_ui_font_label,
        200.0,
        view.picker_open("ui_font"),
        SettingsAction::TogglePicker("ui_font".to_string()),
        interface_font_options(view)
            .into_iter()
            .map(|(family, label)| SelectItem {
                // Choosing Inter by name is the same as the default.
                selected: prefs.ui_font_family == family
                    || (family.is_none()
                        && prefs.ui_font_family.as_deref()
                            == Some(crate::fonts::DEFAULT_UI_FONT_LABEL)),
                enabled: true,
                hover: view.hover(&format!("ui_font_{label}")),
                action: SettingsAction::SelectInterfaceFont(family),
                label,
            })
            .collect(),
        Some(view.ui_font_picker_scroll.clone()),
    );

    let weight_segmented = segmented_control(
        theme,
        ui_font,
        id,
        [
            (Weight::Light, "Light"),
            (Weight::Normal, "Regular"),
            (Weight::Medium, "Medium"),
            (Weight::Semibold, "Semibold"),
            (Weight::Bold, "Bold"),
        ]
        .into_iter()
        .map(|(weight, label)| SegmentOption {
            label: label.to_string(),
            selected: view.editor_settings.editor.font_weight == weight,
            hover: view.hover(&format!("font_weight_{weight:?}")),
            action: SettingsAction::SelectEditorFontWeight(weight),
        })
        .collect(),
    );

    let current_font_label = prefs
        .font_family
        .as_deref()
        .unwrap_or(crate::fonts::DEFAULT_TERMINAL_FONT_LABEL)
        .to_string();
    let font_select = select_control_with_scroll(
        theme,
        ui_font,
        id,
        view.hover("font_btn"),
        &current_font_label,
        200.0,
        view.picker_open("font"),
        SettingsAction::TogglePicker("font".to_string()),
        font_options(view)
            .into_iter()
            .map(|(family, label)| SelectItem {
                selected: prefs.font_family == family
                    || (family.is_none()
                        && prefs.font_family.as_deref()
                            == Some(crate::fonts::DEFAULT_TERMINAL_FONT_LABEL)),
                enabled: true,
                hover: view.hover(&format!("font_{label}")),
                action: SettingsAction::SelectTerminalFont(family),
                label,
            })
            .collect(),
        Some(view.font_picker_scroll.clone()),
    );

    let font_size_slider = slider_control(
        theme,
        ui_font,
        id,
        view.slider("font_size"),
        prefs.clamped_font_size(),
        FONT_SIZE_MIN..FONT_SIZE_MAX,
        1.0,
        format!("{}px", prefs.clamped_font_size() as u32),
        170.0,
        SettingsAction::SetTerminalFontSize,
    );

    let line_height_slider = slider_control(
        theme,
        ui_font,
        id,
        view.slider("line_height"),
        prefs
            .line_height_pct
            .clamp(LINE_HEIGHT_PCT_MIN, LINE_HEIGHT_PCT_MAX) as f32,
        (LINE_HEIGHT_PCT_MIN as f32)..(LINE_HEIGHT_PCT_MAX as f32),
        5.0,
        format!("{}%", prefs.line_height_pct),
        170.0,
        |v| SettingsAction::SetTerminalLineHeight(v.round() as u32),
    );

    let text_card = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui_font,
                "Interface font",
                Some("Menus, sidebars, settings and every other piece of text that isn't code."),
                ui_font_select,
            ),
            setting_row(
                theme,
                ui_font,
                "Code font",
                Some("Terminals, editors, diffs and code everywhere. Missing fonts fall back to the platform default."),
                font_select,
            ),
            setting_row(
                theme,
                ui_font,
                "Font size",
                Some("Sizes all text in the app; 14px matches the default."),
                font_size_slider,
            ),
            setting_row(
                theme,
                ui_font,
                "Font weight",
                Some("Applies to all text; headings stay heavier than body text."),
                weight_segmented,
            ),
            setting_row(theme, ui_font, "Line height", None, line_height_slider),
        ],
    );

    // ── Cursor ──────────────────────────────────────────────────────────────
    let cursor_segmented = segmented_control(
        theme,
        ui_font,
        id,
        [CursorStyle::Block, CursorStyle::Bar, CursorStyle::Underline]
            .into_iter()
            .map(|style| SegmentOption {
                label: style.label().to_string(),
                selected: prefs.cursor_style == style,
                hover: view.hover(&format!("cursor_{style:?}")),
                action: SettingsAction::SelectCursorStyle(style),
            })
            .collect(),
    );

    let cursor_card = card_rows(
        theme,
        vec![
            setting_row(theme, ui_font, "Cursor style", None, cursor_segmented),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("cursor_blink"),
                "Cursor blink",
                Some("Blink the cursor when the terminal is focused."),
                prefs.cursor_blink,
                SettingsAction::ToggleCursorBlink,
            ),
            setting_row(
                theme,
                ui_font,
                "Blink rate",
                Some("Caret and grid blink interval."),
                slider_control(
                    theme,
                    ui_font,
                    id,
                    view.slider("cursor_blink_rate"),
                    prefs
                        .cursor_blink_rate_ms
                        .clamp(CURSOR_BLINK_RATE_MIN_MS, CURSOR_BLINK_RATE_MAX_MS)
                        as f32,
                    (CURSOR_BLINK_RATE_MIN_MS as f32)..(CURSOR_BLINK_RATE_MAX_MS as f32),
                    50.0,
                    format!(
                        "{}ms",
                        prefs
                            .cursor_blink_rate_ms
                            .clamp(CURSOR_BLINK_RATE_MIN_MS, CURSOR_BLINK_RATE_MAX_MS)
                    ),
                    170.0,
                    |v| SettingsAction::SetCursorBlinkRate(v.round() as u32),
                ),
            ),
        ],
    );

    // ── Themes (Warp flow) ────────────────────────────────────────────────
    // Hero on the page (custom-theme link, OS sync, current-theme card);
    // the full gallery lives in the side chooser panel.
    let catalog = Theme::all_infos();
    let current = catalog
        .iter()
        .find(|t| t.id == view.theme_id)
        .cloned()
        .unwrap_or_else(|| catalog[0].clone());
    let mut theme_body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(12.0)
        .with_child(link_button(
            theme,
            ui_font,
            id,
            view.hover("theme_import_link"),
            "Create your own custom theme",
            SettingsAction::ImportTheme,
        ));
    if let Some(status) = &view.theme_status_message {
        theme_body = theme_body.with_child(
            Text::new(status.clone(), ui_font, crate::theme::text_size(12.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    }
    theme_body = theme_body
        .with_child(toggle_row(
            theme,
            ui_font,
            id,
            view.hover("theme_sync_os"),
            "Sync with OS",
            None,
            view.theme_sync_os,
            SettingsAction::ToggleThemeSyncOs,
        ))
        .with_child(
            Text::new(
                "Automatically switch between light and dark themes when your system does."
                    .to_string(),
                ui_font,
                crate::theme::text_size(12.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    {
        let current_owned = current.clone();
        let current_name = current.name.clone();
        let box_bg = theme.surface_1();
        let box_border = theme.outline();
        let box_text = theme.main_text();
        let chrome_owned = *theme;
        theme_body = theme_body.with_child(Box::new(
            Hoverable::new(view.hover("theme_hero"), move |_| {
                let hero_preview = super::super::widgets::theme_preview_card(
                    &chrome_owned,
                    &current_owned.theme,
                    ui_font,
                    false,
                    false,
                    current_owned
                        .bg_image
                        .as_ref()
                        .map(|background| background.thumb_asset_source()),
                );
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(16.0)
                        .with_child(
                            Text::new(
                                "Current theme".to_string(),
                                ui_font,
                                crate::theme::text_size(14.0),
                            )
                            .with_color(box_text)
                            .finish(),
                        )
                        .with_child(ConstrainedBox::new(hero_preview).with_width(220.0).finish())
                        .with_child(
                            Text::new(current_name.clone(), ui_font, crate::theme::text_size(14.0))
                                .with_color(box_text)
                                .finish(),
                        )
                        .finish(),
                )
                .with_background_color(box_bg)
                .with_border(Border::all(1.0).with_border_color(box_border))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
                .with_uniform_padding(14.0)
                .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::OpenThemeChooser);
            }),
        ));
    }

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(
            theme,
            ui_font,
            "Appearance",
            "Themes, text, and cursor — everything that changes how the app looks.",
        ))
        .with_child(group_label(theme, ui_font, "Text"))
        .with_child(text_card)
        .with_child(group_label(theme, ui_font, "Cursor"))
        .with_child(cursor_card)
        .with_child(setting_group(
            theme,
            ui_font,
            "Themes",
            vec![theme_body.finish()],
        ))
        .finish()
}

/// Filtered theme gallery (search + virtualized preview list), shared by the
/// side chooser panel. Rows render lazily in-viewport (Warp speed pattern).
pub(crate) fn theme_gallery_list(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let query = view.theme_search_query.trim().to_lowercase();

    let catalog = Theme::all_infos();
    let matches: Vec<crate::theme::ThemeInfo> = catalog
        .iter()
        .filter(|t| {
            query.is_empty()
                || t.name.to_lowercase().contains(&query)
                || t.id.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();

    let hover_states: Vec<_> = matches
        .iter()
        .map(|info| view.hover(&format!("theme_{}", info.id)))
        .collect();
    let row_count = matches.len();
    let chrome_theme = *theme;
    let selected_theme_id = view.theme_id.clone();
    let theme_list = if row_count == 0 {
        None
    } else {
        let rows = UniformList::new(
            view.theme_list_state.clone(),
            row_count,
            move |range, _app| {
                range
                    .map(|index| {
                        let info = &matches[index];
                        let thumb = info
                            .bg_image
                            .as_ref()
                            .map(|background| background.thumb_asset_source());
                        Container::new(theme_row(
                            &chrome_theme,
                            &info.theme,
                            ui_font,
                            id,
                            hover_states[index].clone(),
                            &info.name,
                            info.id,
                            selected_theme_id == info.id,
                            thumb,
                        ))
                        .with_padding_bottom(8.0)
                        .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            },
        );
        let scrollable = Scrollable::vertical(
            view.theme_list_scroll.clone(),
            rows.finish_scrollable(),
            ScrollbarWidth::None,
            theme.disabled_text().into(),
            theme.sub_text().into(),
            theme.background.into(),
        )
        .finish();
        Some(scrollable)
    };

    let footer: Option<Box<dyn Element>> = if row_count == 0 {
        Some(
            Text::new(
                format!("No themes match “{}”.", view.theme_search_query.trim()),
                ui_font,
                crate::theme::text_size(12.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        )
    } else {
        None
    };

    let search_box = crate::shared_input::shared_input(
        theme,
        &view.theme_search_editor,
        crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 10.0,
            background: Some(theme.surface_1()),
            border: Some(theme.outline()),
            pad_left: 10.0,
            pad_right: 10.0,
            pad_top: 10.0,
            pad_bottom: 10.0,
            ..Default::default()
        },
    );

    // Capped against the captured pane height (unbounded-height ancestor
    // above): sizes to content when short, scrolls when tall. Room reserved
    // for the panel header, subtitle, and search box.
    let list_max_h = (view.pane_height() - 220.0).max(200.0);
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(12.0)
        .with_child(search_box);
    if let Some(theme_list) = theme_list {
        body = body.with_child(
            ConstrainedBox::new(theme_list)
                .with_max_height(list_max_h)
                .finish(),
        );
    }
    if let Some(footer) = footer {
        body = body.with_child(footer);
    }
    body.finish()
}

/// Theme chooser side panel (Warp flow): title, subtitle, close control, and
/// the shared filterable gallery. Selecting a theme applies it live; the
/// panel stays open for browsing until dismissed.
pub fn render_theme_chooser(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;

    let header = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(warpui::elements::MainAxisAlignment::SpaceBetween)
        .with_child(
            Text::new("Themes".to_string(), ui_font, crate::theme::text_size(17.0))
                .with_color(theme.main_text())
                .finish(),
        )
        .with_child(Box::new(
            Hoverable::new(view.hover("theme_chooser_close"), move |state| {
                Container::new(
                    Text::new("✕".to_string(), ui_font, crate::theme::text_size(13.0))
                        .with_color(if state.is_hovered() {
                            theme.main_text()
                        } else {
                            theme.disabled_text()
                        })
                        .finish(),
                )
                .with_uniform_padding(6.0)
                .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::CloseThemeChooser);
            }),
        ))
        .finish();

    let panel = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(header)
            .with_child(
                Text::new(
                    "Change your current theme.".to_string(),
                    ui_font,
                    crate::theme::text_size(12.5),
                )
                .with_color(theme.sub_text())
                .finish(),
            )
            .with_child(theme_gallery_list(view, theme))
            .finish(),
    )
    .with_background_color(theme.sidebar_background())
    .with_border(Border::left(1.0).with_border_color(theme.border()))
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(16.0)
    .with_padding_bottom(16.0)
    .finish();

    ConstrainedBox::new(panel).with_width(320.0).finish()
}
