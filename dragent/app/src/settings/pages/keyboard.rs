use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize,
    ParentElement, Text,
};
use warpui_core::elements::Hoverable;
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::machine_ui::{self as mui, Size, Tone, Variant};
use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::shortcut_settings::{
    CtrlTabOrder, TerminalShortcutPolicy, DEFAULT_BINDINGS,
};
use super::super::widgets::{
    card_rows, danger_button, group_label, page_header, segmented_control, setting_row,
    SegmentOption,
};

/// Fixed terminal keys that are not rebindable (owned by the PTY or the
/// input editor). Shown for completeness under the editable table.
const FIXED_ROWS: &[(&str, &[(&str, &str)])] = &[
    (
        "Terminal editing (fixed)",
        &[
            ("Interrupt running command (SIGINT)", "Ctrl+C"),
            ("Clear screen", "type \u{201c}clear\u{201d}"),
            ("Previous command (history)", "\u{2191}"),
            ("Next command (history)", "\u{2193}"),
            ("Accept autosuggestion", "\u{2192}"),
            ("Autocomplete path / command", "Tab"),
            ("Autocomplete previous", "\u{21e7}Tab"),
            ("Send message", "Enter"),
            ("Insert newline", "\u{21e7}Enter"),
            ("Accept search result", "Enter"),
            ("Close search / palette", "Esc"),
        ],
    ),
];

/// A stored chord (`cmdorctrl-shift-N`) the way the OS shows it (`⇧⌘N`).
fn displayed_chord(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(|step| {
            warpui::keymap::Keystroke::parse(step)
                .map(|keystroke| keystroke.displayed())
                .unwrap_or_else(|_| step.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn matches(query: &str, cmd: &str, bind: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    cmd.to_lowercase().contains(&q) || bind.to_lowercase().contains(&q)
}

/// One key, drawn as a quiet key cap.
fn key_cap(theme: &Theme, ui: warpui::fonts::FamilyId, key: &str) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            warpui::elements::Align::new(mui::medium(ui, key, 11.5, theme.main_text())).finish(),
        )
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(2.0)
        .with_padding_bottom(2.0)
        .finish(),
    )
    .with_min_width(24.0)
    .with_height(24.0)
    .finish()
}

/// The keys of one way to press a shortcut: `⇧⌘N` → ⇧ ⌘ N, `Ctrl+C` → Ctrl C.
fn key_tokens(chord: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut current = String::new();
    for ch in chord.trim().chars() {
        if matches!(ch, '⌘' | '⇧' | '⌥' | '⌃') {
            if !current.is_empty() {
                keys.push(std::mem::take(&mut current));
            }
            keys.push(ch.to_string());
        } else if ch == '+' && !current.is_empty() {
            keys.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        keys.push(current);
    }
    keys
}

/// A shortcut as key caps. Several ways to press it are joined by "or";
/// something that isn't a key press (`type "clear"`) stays text.
fn key_caps(theme: &Theme, ui: warpui::fonts::FamilyId, binding: &str) -> Box<dyn Element> {
    if binding.trim().is_empty() {
        return mui::text(ui, "Not set", 12.0, theme.disabled_text());
    }
    if binding.contains('\u{201c}') {
        return mui::text(ui, binding, 12.0, theme.sub_text());
    }
    let mut row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(4.0);
    for (i, alternative) in binding.split(" / ").enumerate() {
        if i > 0 {
            row = row.with_child(
                Container::new(mui::text(ui, "or", 11.5, theme.disabled_text()))
                    .with_padding_left(4.0)
                    .with_padding_right(4.0)
                    .finish(),
            );
        }
        for key in key_tokens(alternative) {
            row = row.with_child(key_cap(theme, ui, &key));
        }
    }
    row.finish()
}

/// A shortcut that can be changed: click the row to change it.
fn editable_row(view: &SettingsView, theme: &Theme, action_id: &str, label: &str) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    if view.shortcut_rebind_target.as_deref() == Some(action_id) {
        return rebind_row(view, theme, action_id, label);
    }
    let shortcuts = &view.app_settings.shortcuts;
    let keys = displayed_chord(&shortcuts.effective_chord(action_id).unwrap_or_default());
    let custom = shortcuts.custom_bindings.contains_key(action_id);
    let theme_c = *theme;
    let label = label.to_string();
    let start = SettingsAction::StartRebind(action_id.to_string());
    let row = Hoverable::new(view.hover(&format!("shortcut_row_{action_id}")), move |state| {
        let hovered = state.is_hovered();
        let mut title = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(mui::text(ui, label.clone(), 13.5, theme_c.main_text()));
        if custom {
            title = title.with_child(mui::pill(&theme_c, ui, "Custom", Tone::Info));
        }
        let mut keys_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(10.0);
        if hovered {
            keys_row = keys_row.with_child(mui::text(ui, "Change", 12.0, theme_c.accent));
        }
        keys_row = keys_row.with_child(key_caps(&theme_c, ui, &keys));
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(12.0)
                .with_child(Expanded::new(1.0, title.finish()).finish())
                .with_child(keys_row.finish())
                .finish(),
        )
        .with_background_color(if hovered {
            theme_c.surface_1()
        } else {
            warpui::color::ColorU::transparent_black()
        })
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(11.0)
        .with_padding_bottom(11.0)
        .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(id, start.clone()))
    .finish();
    if !custom {
        return row;
    }
    // Reset sits outside the row so clicking it doesn't also start a change.
    Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(Expanded::new(1.0, row).finish())
        .with_child(
            Container::new(mui::button(
                theme,
                ui,
                id,
                view.hover(&format!("shortcut_reset_{action_id}")),
                Some(crate::icons::AppIcon::RotateCcw),
                "",
                Variant::Ghost,
                Size::Xs,
                true,
                SettingsAction::ResetBinding(action_id.to_string()),
            ))
            .with_padding_right(10.0)
            .finish(),
        )
        .finish()
}

/// The row being changed: the new keys, typed, with Save and Cancel.
fn rebind_row(view: &SettingsView, theme: &Theme, action_id: &str, label: &str) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let field = crate::shared_input::shared_input(
        theme,
        &view.shortcut_binding_editor,
        crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fixed(240.0),
            height: Some(mui::CONTROL_HEIGHT_SM),
            radius: mui::RADIUS_CONTROL,
            pad_left: 10.0,
            pad_right: 10.0,
            ..Default::default()
        },
    );
    let controls = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(field)
        .with_child(mui::button(
            theme,
            ui,
            id,
            view.hover(&format!("shortcut_save_{action_id}")),
            None,
            "Save",
            Variant::Primary,
            Size::Xs,
            true,
            SettingsAction::SetBinding {
                action: action_id.to_string(),
                chord: String::new(),
            },
        ))
        .with_child(mui::button(
            theme,
            ui,
            id,
            view.hover(&format!("shortcut_cancel_{action_id}")),
            None,
            "Cancel",
            Variant::Ghost,
            Size::Xs,
            true,
            SettingsAction::CancelRebind,
        ))
        .finish();
    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(10.0)
        .with_child(mui::text(ui, label, 13.5, theme.main_text()))
        .with_child(controls)
        .with_child(mui::text(
            ui,
            "Join the keys with \u{201c}-\u{201d}: cmd-shift-k, ctrl-g. cmdorctrl is \u{2318} on macOS and Ctrl elsewhere.",
            11.5,
            theme.sub_text(),
        ));
    if let Some(note) = &view.shortcut_status_message {
        column = column.with_child(mui::text(ui, note.clone(), 11.5, theme.ui_red()));
    }
    Container::new(column.finish())
        .with_background_color(theme.surface_1())
        .with_padding_left(16.0)
        .with_padding_right(16.0)
        .with_padding_top(14.0)
        .with_padding_bottom(14.0)
        .finish()
}

/// A shortcut owned by the terminal or the editor: shown, not changeable.
fn fixed_row(theme: &Theme, ui: warpui::fonts::FamilyId, command: &str, binding: &str) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(12.0)
            .with_child(Expanded::new(1.0, mui::text(ui, command, 13.5, theme.sub_text())).finish())
            .with_child(key_caps(theme, ui, binding))
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_right(16.0)
    .with_padding_top(11.0)
    .with_padding_bottom(11.0)
    .finish()
}

/// A titled group of shortcut rows in one card.
fn shortcut_group(theme: &Theme, ui: warpui::fonts::FamilyId, title: &str, rows: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(8.0)
        .with_child(
            Container::new(mui::medium(ui, title, 12.5, theme.sub_text()))
                .with_padding_left(2.0)
                .finish(),
        )
        .with_child(mui::list_card(theme, rows))
        .finish()
}

pub fn render_keyboard_shortcuts(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let query = view.shortcut_search_query.trim();

    // ── Behavior ──────────────────────────────────────────────────────────
    let policy = view.app_settings.shortcuts.terminal_shortcut_policy;
    let policy_control = segmented_control(
        theme,
        ui_font,
        id,
        [TerminalShortcutPolicy::AppFirst, TerminalShortcutPolicy::TerminalFirst]
            .into_iter()
            .map(|p| SegmentOption {
                label: p.label().to_string(),
                selected: policy == p,
                hover: view.hover(&format!("shortcut_policy_{:?}", p)),
                action: SettingsAction::SelectShortcutPolicy(p),
            })
            .collect(),
    );
    let order = view.app_settings.shortcuts.ctrl_tab_order;
    let order_control = segmented_control(
        theme,
        ui_font,
        id,
        [CtrlTabOrder::Creation, CtrlTabOrder::Mru]
            .into_iter()
            .map(|o| SegmentOption {
                label: o.label().to_string(),
                selected: order == o,
                hover: view.hover(&format!("shortcut_order_{:?}", o)),
                action: SettingsAction::SelectCtrlTabOrder(o),
            })
            .collect(),
    );
    let policy_card = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui_font,
                "Shortcut policy",
                Some("App first: Dragent's shortcuts win. Terminal first: a running command gets the keys."),
                policy_control,
            ),
            setting_row(
                theme,
                ui_font,
                "Next and previous tab",
                Some("Creation order walks the tab strip left to right; recently used walks by recency."),
                order_control,
            ),
        ],
    );

    // ── Shortcuts ─────────────────────────────────────────────────────────
    let search = crate::shared_input::shared_input(
        theme,
        &view.shortcut_search_editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: crate::icons::AppIcon::Search,
                size: 13.0,
            }),
            width: crate::shared_input::SharedInputWidth::Fixed(260.0),
            height: Some(mui::CONTROL_HEIGHT_SM),
            radius: mui::RADIUS_CONTROL,
            ..Default::default()
        },
    );
    let shortcuts_header = Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1.0, mui::eyebrow(ui_font, theme, "Shortcuts")).finish())
            .with_child(search)
            .finish(),
    )
    .with_padding_top(20.0)
    .finish();

    // Editable shortcuts by their section, in declared order.
    let mut sections: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for (action_id, _chord, label, section) in DEFAULT_BINDINGS {
        let effective = view
            .app_settings
            .shortcuts
            .effective_chord(action_id)
            .unwrap_or_default();
        if !matches(query, label, &displayed_chord(&effective)) && !matches(query, label, &effective) {
            continue;
        }
        match sections.last_mut() {
            Some((title, list)) if *title == *section => list.push((action_id, label)),
            _ => sections.push((section, vec![(action_id, label)])),
        }
    }
    let mut groups: Vec<Box<dyn Element>> = sections
        .into_iter()
        .map(|(title, items)| {
            let rows = items
                .into_iter()
                .map(|(action_id, label)| editable_row(view, theme, action_id, label))
                .collect();
            shortcut_group(theme, ui_font, title, rows)
        })
        .collect();
    for (title, shortcuts) in FIXED_ROWS {
        let rows: Vec<Box<dyn Element>> = shortcuts
            .iter()
            .filter(|(cmd, bind)| matches(query, cmd, bind))
            .map(|(cmd, bind)| fixed_row(theme, ui_font, cmd, bind))
            .collect();
        if !rows.is_empty() {
            groups.push(shortcut_group(theme, ui_font, title, rows));
        }
    }
    if groups.is_empty() {
        groups.push(mui::empty_state(
            theme,
            ui_font,
            crate::icons::AppIcon::Keyboard,
            "No shortcuts match",
            &format!("Nothing matches \u{201c}{query}\u{201d}. Try a command name or a key like cmd."),
            None,
        ));
    }

    let footer = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(12.0)
        .with_child(danger_button(
            theme,
            ui_font,
            id,
            view.hover("shortcut_reset_all"),
            "Reset all shortcuts",
            SettingsAction::ResetAllBindings,
        ))
        .with_child(mui::text(
            ui_font,
            "Changes are also saved to ~/.dragent/keybindings.json.",
            11.5,
            theme.disabled_text(),
        ))
        .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui_font,
            "Keyboard shortcuts",
            "Click a shortcut to change it. Terminal and editor keys are listed for reference.",
        ))
        .with_child(group_label(theme, ui_font, "Behavior"))
        .with_child(policy_card)
        .with_child(shortcuts_header)
        .with_child(
            Container::new(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_spacing(20.0)
                    .with_children(groups)
                    .finish(),
            )
            .with_padding_top(14.0)
            .with_padding_bottom(20.0)
            .finish(),
        )
        .with_child(footer)
        .finish()
}
