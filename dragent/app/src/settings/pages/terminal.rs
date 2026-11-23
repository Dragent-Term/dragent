//! Terminal and Input — shell and input behavior.
//! Typography and cursor live on the Appearance page (editor standard); every
//! control here is wired to `EditorSettings.terminal` and applies live via
//! `SettingsEvent::EditorSettingsChanged` → `sync_editor_settings`.

use warpui::elements::{Element, Flex, MainAxisSize, ParentElement};

use crate::console::editor_settings::MacOptionAsAlt;
use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, page_header, secondary_button, select_control, setting_row,
    toggle_row, SelectItem,
};

pub fn render_terminal_input(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let prefs = &view.editor_settings.terminal;

    // ── Shell ─────────────────────────────────────────────────────────────────
    let current_shell_label = prefs
        .default_shell
        .as_deref()
        .map(|path| {
            dragent_pty::available_shells()
                .iter()
                .find(|s| s.path == path)
                .map(|s| s.label.clone())
                .unwrap_or_else(|| path.to_string())
        })
        .unwrap_or_else(|| "System default".to_string());

    let mut shell_items = vec![SelectItem {
        label: "System default".to_string(),
        selected: prefs.default_shell.is_none(),
        enabled: true,
        hover: view.hover("shell_default"),
        action: SettingsAction::SelectDefaultShell(None),
    }];
    for shell in dragent_pty::available_shells() {
        shell_items.push(SelectItem {
            label: shell.label.clone(),
            selected: prefs.default_shell.as_deref() == Some(shell.path.as_str()),
            enabled: true,
            hover: view.hover(&format!("shell_{}", shell.path)),
            action: SettingsAction::SelectDefaultShell(Some(shell.path.clone())),
        });
    }
    let shell_select = select_control(
        theme,
        ui_font,
        id,
        view.hover("shell_btn"),
        &current_shell_label,
        190.0,
        view.picker_open("shell"),
        SettingsAction::TogglePicker("shell".to_string()),
        shell_items,
    );

    let shell_card = card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "Default shell",
            Some("The shell launched in new terminal sessions."),
            shell_select,
        )],
    );

    // ── Input ─────────────────────────────────────────────────────────────────
    // Typography, cursor, and keybinding mode moved to Appearance and Editor
    // (editor standard); agent-input behavior stays here.
    let input_card = card_rows(
        theme,
        vec![toggle_row(
            theme,
            ui_font,
            id,
            view.hover("autodetect_terminal"),
            "Input autodetection",
            Some("Detect shell vs agent input automatically."),
            view.editor_settings.input_autodetection_enabled,
            SettingsAction::ToggleInputAutodetect,
        )],
    );

    // ── macOS option key ────────────────────────────────────────────────────
    let mac_modes = [
        MacOptionAsAlt::Auto,
        MacOptionAsAlt::True,
        MacOptionAsAlt::False,
        MacOptionAsAlt::Left,
        MacOptionAsAlt::Right,
    ];
    let current_mac = prefs.mac_option_as_alt;
    let current_mac_label = current_mac.label().to_string();
    let mac_select = select_control(
        theme,
        ui_font,
        id,
        view.hover("mac_option_btn"),
        &current_mac_label,
        190.0,
        view.picker_open("mac_option"),
        SettingsAction::TogglePicker("mac_option".to_string()),
        mac_modes
            .into_iter()
            .map(|mode| SelectItem {
                label: mode.label().to_string(),
                selected: current_mac == mode,
                enabled: true,
                hover: view.hover(&format!("mac_option_{:?}", mode)),
                action: SettingsAction::SelectMacOptionAsAlt(mode),
            })
            .collect(),
    );
    let mac_card = card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "macOS Option key",
            Some("How Option acts in the terminal. Auto is Alt on macOS only."),
            mac_select,
        )],
    );

    // ── Clipboard ───────────────────────────────────────────────────────────
    let clipboard_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("clipboard_on_select"),
                "Copy on select",
                Some("Copy terminal selection to the clipboard automatically."),
                prefs.clipboard_on_select,
                SettingsAction::ToggleClipboardOnSelect,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("copy_trims_gutter"),
                "Copy trims gutter",
                Some("Strip gutter whitespace when copying terminal output."),
                prefs.copy_trims_gutter,
                SettingsAction::ToggleCopyTrimsGutter,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("middle_click_paste"),
                "Middle-click paste",
                Some("Middle-click pastes the clipboard. Default is Linux-only."),
                prefs.middle_click_paste_enabled(),
                SettingsAction::ToggleMiddleClickPaste,
            ),
        ],
    );

    // ── Selection ───────────────────────────────────────────────────────────
    let sep_label = prefs
        .word_separator
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("Default");
    let sep_items = vec![
        SelectItem {
            label: "Default".to_string(),
            selected: prefs.word_separator.is_none(),
            enabled: true,
            hover: view.hover("word_sep_default"),
            action: SettingsAction::SetWordSeparator(None),
        },
        SelectItem {
            label: ": / , ;".to_string(),
            selected: prefs.word_separator.as_deref() == Some(":/,;"),
            enabled: true,
            hover: view.hover("word_sep_code"),
            action: SettingsAction::SetWordSeparator(Some(":/,;".to_string())),
        },
        SelectItem {
            label: "Whitespace only".to_string(),
            selected: prefs.word_separator.as_deref() == Some(" \t"),
            enabled: true,
            hover: view.hover("word_sep_ws"),
            action: SettingsAction::SetWordSeparator(Some(" \t".to_string())),
        },
    ];
    let sep_select = select_control(
        theme,
        ui_font,
        id,
        view.hover("word_sep_btn"),
        sep_label,
        190.0,
        view.picker_open("word_sep"),
        SettingsAction::TogglePicker("word_sep".to_string()),
        sep_items,
    );
    let selection_card = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui_font,
                "Word separator",
                Some("Extra characters that break words on double-click select."),
                sep_select,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("jis_yen"),
                "JIS ¥ as backslash",
                Some("Map the Japanese JIS Yen key to backslash in the terminal."),
                prefs.jis_yen_to_backslash,
                SettingsAction::ToggleJisYen,
            ),
        ],
    );

    let mut page = Flex::column()
        .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui_font,
            "Terminal and Input",
            "Configure the shell and input behavior. Typography and cursor live under Appearance.",
        ))
        .with_child(group_label(theme, ui_font, "Shell"))
        .with_child(shell_card)
        .with_child(group_label(theme, ui_font, "Input"))
        .with_child(input_card)
        .with_child(group_label(theme, ui_font, "macOS option key"))
        .with_child(mac_card)
        .with_child(group_label(theme, ui_font, "Clipboard"))
        .with_child(clipboard_card)
        .with_child(group_label(theme, ui_font, "Selection"))
        .with_child(selection_card);
    #[cfg(not(target_family = "wasm"))]
    {
        page = page
            .with_child(group_label(theme, ui_font, "Command line"))
            .with_child(command_line_card(view, theme));
    }
    page.finish()
}

/// The `dragent` command: always on PATH in Dragent's terminals; other
/// terminals need a line in the shell profile, added on request.
#[cfg(not(target_family = "wasm"))]
fn command_line_card(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use warpui::elements::Text;

    let ui_font = view.ui_font;
    let bin = crate::cli::user_bin_dir();
    let setup = crate::cli::shell_setup::Setup::current();
    let installed = setup.as_ref().is_some_and(|setup| setup.installed(&bin));
    // A Dev build never edits shell profiles or the user PATH, and never
    // writes into shared skill folders, so both rows below explain that
    // instead of offering the setup.
    let dev = dragent_paths::is_dev();
    let description = if dev {
        "Available in Dragent's terminals. Dev builds don't change other apps' settings."
            .to_string()
    } else {
        match (&view.cli_setup_error, &setup) {
            (Some(error), _) => error.clone(),
            (None, Some(setup)) if installed => format!(
                "Available in Dragent's terminals, and in other terminals through {}. New terminal windows pick it up.",
                setup.place()
            ),
            (None, Some(setup)) => format!(
                "Available in Dragent's terminals. Add it to {} to use it in other terminals too.",
                setup.place()
            ),
            (None, None) => format!(
                "Available in Dragent's terminals. To use it elsewhere, add {} to your PATH.",
                bin.display()
            ),
        }
    };
    let label = if cfg!(windows) { "Add to PATH" } else { "Add to shell" };
    let control: Box<dyn Element> = match &setup {
        Some(_) if !installed && !dev => secondary_button(
            theme,
            ui_font,
            view.view_id,
            view.hover("cli_install"),
            label,
            SettingsAction::InstallCliInShell,
        ),
        Some(_) if installed => {
            Text::new("Added".to_string(), ui_font, crate::theme::text_size(12.5))
                .with_color(theme.sub_text())
                .finish()
        }
        _ => warpui::elements::Empty::new().finish(),
    };
    let skill_description = view.agent_skill_note.clone().unwrap_or_else(|| {
        if dev {
            "Dev builds don't change other apps' settings.".to_string()
        } else {
            "Installs Dragent’s focused skills for browser use, panes, teams, automations and connected services in shared agent folders. User-edited files are preserved. Agents that are already running need a restart to see changes. Claude Code sessions launched by Dragent and Dragent’s own agent include the pack. Desktop guidance has its own control in Computer use.".into()
        }
    });
    card_rows(
        theme,
        vec![
            setting_row(theme, ui_font, "dragent command", Some(&description), control),
            toggle_row(
                theme,
                ui_font,
                view.view_id,
                view.hover("agent_skill"),
                "Teach other agents",
                Some(&skill_description),
                crate::cli::agent_skill::enabled(),
                SettingsAction::ToggleAgentSkill,
            ),
        ],
    )
}
