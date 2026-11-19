//! `dragent settings` and `dragent theme`: the main preferences from the
//! command line. With the app open a change goes through it, applying live
//! and saving exactly as the Settings page does; with it closed the change
//! is saved directly and applies when it next opens.

use crate::console::editor_settings::{
    EditorSettings, MacOptionAsAlt, AUTO_SAVE_DELAY_MAX_MS, AUTO_SAVE_DELAY_MIN_MS, FONT_SIZE_MAX,
    FONT_SIZE_MIN, LINE_HEIGHT_PCT_MAX, LINE_HEIGHT_PCT_MIN,
};
use crate::settings::SettingsAction;
use crate::setup::UiSettings;
use crate::theme::{Theme, DEFAULT_THEME_ID};

use super::{ask_app, instance, listing, pretty, requests, text, Flags};

/// The settings the command line can change, with what they mean.
const KEYS: [(&str, &str); 23] = [
    ("theme", "Colour theme (`dragent theme list`)"),
    ("interface-font", "Font for the app's interface (\"default\" for the built-in one)"),
    ("code-font", "Font for terminals and code (\"default\" for the built-in one)"),
    ("font-size", "Terminal and code text size, 10–20"),
    ("line-height", "Line height in percent, 100–200"),
    ("default-shell", "Shell new terminals start (a path, or \"default\")"),
    ("vim-mode", "Vim keys in the input (on/off)"),
    ("input-autodetect", "Tell commands from agent prompts as you type (on/off)"),
    ("teach-other-agents", "Install Dragent’s focused skill pack for other agents (on/off)"),
    ("mac-option-as-alt", "macOS Option as Alt: auto/true/false/left/right"),
    ("jis-yen-backslash", "Map JIS Yen key to backslash (on/off)"),
    ("clipboard-on-select", "Copy terminal selection automatically (on/off)"),
    ("copy-trims-gutter", "Strip gutter whitespace on copy (on/off)"),
    ("word-separator", "Extra word-break chars (\"default\" for built-in)"),
    ("middle-click-paste", "Middle-click pastes (on/off/default)"),
    ("auto-save", "Save dirty documents automatically (on/off)"),
    ("auto-save-delay", "Autosave debounce in ms, 100–10000"),
    ("minimap", "Miniature overview rail in editors (on/off)"),
    ("editor-font", "Source editor font (\"default\" follows terminal)"),
    ("word-wrap", "Soft-wrap long lines (on/off)"),
    ("spellcheck", "Spellcheck in markdown surfaces (on/off)"),
    ("markdown-review-tools", "Review affordances on markdown (on/off)"),
    ("preview-tabs", "Single-click opens a reusable preview tab (on/off)"),
];

fn known(key: &str) -> Result<(), String> {
    if KEYS.iter().any(|(name, _)| *name == key) {
        Ok(())
    } else {
        Err(format!(
            "unknown setting “{key}”. Settings: {}",
            KEYS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        ))
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.into()
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => Err(format!("“{value}” isn't on or off")),
    }
}

/// The saved value, as the command line shows it.
fn current(key: &str) -> String {
    let editor = EditorSettings::load();
    let terminal = &editor.terminal;
    let or_default = |value: &Option<String>| value.clone().unwrap_or_else(|| "default".into());
    match key {
        "theme" => UiSettings::load().theme_id,
        "interface-font" => or_default(&terminal.ui_font_family),
        "code-font" => or_default(&terminal.font_family),
        "font-size" => format!("{}", terminal.font_size),
        "line-height" => terminal.line_height_pct.to_string(),
        "default-shell" => or_default(&terminal.default_shell),
        "vim-mode" => on_off(editor.vim_mode_enabled),
        "input-autodetect" => on_off(editor.input_autodetection_enabled),
        "teach-other-agents" => on_off(super::agent_skill::enabled()),
        "mac-option-as-alt" => terminal.mac_option_as_alt.as_str().to_string(),
        "jis-yen-backslash" => on_off(terminal.jis_yen_to_backslash),
        "clipboard-on-select" => on_off(terminal.clipboard_on_select),
        "copy-trims-gutter" => on_off(terminal.copy_trims_gutter),
        "word-separator" => or_default(&terminal.word_separator),
        "middle-click-paste" => match terminal.middle_click_paste {
            Some(v) => on_off(v),
            None => "default".into(),
        },
        "auto-save" => on_off(editor.editor.auto_save),
        "auto-save-delay" => editor.editor.auto_save_delay_ms.to_string(),
        "minimap" => on_off(editor.editor.minimap_enabled),
        "editor-font" => or_default(&editor.editor.font_family),
        "word-wrap" => on_off(editor.editor.word_wrap),
        "spellcheck" => on_off(editor.editor.spellcheck_enabled),
        "markdown-review-tools" => on_off(editor.editor.markdown_review_tools),
        "preview-tabs" => on_off(editor.editor.preview_tabs),
        _ => String::new(),
    }
}

/// `value` checked and written the way it's stored (`None`: the default).
fn normalise(key: &str, value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    let default = value.eq_ignore_ascii_case("default");
    Ok(match key {
        "theme" => {
            if !(value == "dark" || value == "light" || Theme::all_infos().iter().any(|theme| theme.id == value)) {
                return Err(format!("no theme called “{value}” (see `dragent theme list`)"));
            }
            Some(value.to_string())
        }
        "interface-font" | "code-font" => (!default).then(|| value.to_string()),
        "default-shell" => {
            if default {
                None
            } else if std::path::Path::new(value).is_file() {
                Some(value.to_string())
            } else {
                return Err(format!("{value} isn't a shell on this computer"));
            }
        }
        "font-size" => {
            let size: f32 = value.parse().map_err(|_| format!("“{value}” isn't a size"))?;
            if !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&size) {
                return Err(format!("the size is {FONT_SIZE_MIN}–{FONT_SIZE_MAX}"));
            }
            Some(size.to_string())
        }
        "line-height" => {
            let height: u32 = value
                .trim_end_matches('%')
                .parse()
                .map_err(|_| format!("“{value}” isn't a percentage"))?;
            if !(LINE_HEIGHT_PCT_MIN..=LINE_HEIGHT_PCT_MAX).contains(&height) {
                return Err(format!("the line height is {LINE_HEIGHT_PCT_MIN}–{LINE_HEIGHT_PCT_MAX}%"));
            }
            Some(height.to_string())
        }
        "vim-mode" | "input-autodetect" | "teach-other-agents" => Some(on_off(parse_bool(value)?)),
        "mac-option-as-alt" => {
            let mode = MacOptionAsAlt::parse(value)
                .ok_or_else(|| "use auto, true, false, left or right".to_string())?;
            Some(mode.as_str().to_string())
        }
        "jis-yen-backslash" | "clipboard-on-select" | "copy-trims-gutter" | "auto-save"
        | "minimap" | "word-wrap" | "spellcheck" | "markdown-review-tools" | "preview-tabs" => {
            Some(on_off(parse_bool(value)?))
        }
        "word-separator" | "editor-font" => (!default).then(|| value.to_string()),
        "middle-click-paste" => {
            if default {
                Some("default".into())
            } else {
                Some(on_off(parse_bool(value)?))
            }
        }
        "auto-save-delay" => {
            let delay: u32 = value.parse().map_err(|_| format!("“{value}” isn't milliseconds"))?;
            if !(AUTO_SAVE_DELAY_MIN_MS..=AUTO_SAVE_DELAY_MAX_MS).contains(&delay) {
                return Err(format!(
                    "the delay is {AUTO_SAVE_DELAY_MIN_MS}–{AUTO_SAVE_DELAY_MAX_MS}ms"
                ));
            }
            Some(delay.to_string())
        }
        _ => return Err(format!("unknown setting “{key}”")),
    })
}

/// App side: the Settings page action for a change (`None` resets).
/// `None` when nothing needs doing.
pub fn action_for(key: &str, value: Option<&str>, editor: &EditorSettings) -> Result<Option<SettingsAction>, String> {
    let defaults = EditorSettings::default();
    let number = |value: &str| value.parse::<f32>().map_err(|_| format!("“{value}” isn't a number"));
    Ok(Some(match key {
        "theme" => SettingsAction::SelectTheme(value.unwrap_or(DEFAULT_THEME_ID).to_string()),
        "interface-font" => SettingsAction::SelectInterfaceFont(value.map(str::to_string)),
        "code-font" => SettingsAction::SelectTerminalFont(value.map(str::to_string)),
        "default-shell" => SettingsAction::SelectDefaultShell(value.map(str::to_string)),
        "font-size" => SettingsAction::SetTerminalFontSize(match value {
            Some(value) => number(value)?,
            None => defaults.terminal.font_size,
        }),
        "line-height" => SettingsAction::SetTerminalLineHeight(match value {
            Some(value) => number(value)? as u32,
            None => defaults.terminal.line_height_pct,
        }),
        "vim-mode" => SettingsAction::SetVimMode(match value {
            Some(value) => parse_bool(value)?,
            None => defaults.vim_mode_enabled,
        }),
        "input-autodetect" => {
            let wanted = match value {
                Some(value) => parse_bool(value)?,
                None => defaults.input_autodetection_enabled,
            };
            if wanted == editor.input_autodetection_enabled {
                return Ok(None);
            }
            SettingsAction::ToggleInputAutodetect
        }
        "mac-option-as-alt" => SettingsAction::SelectMacOptionAsAlt(match value {
            Some(value) => MacOptionAsAlt::parse(value)
                .ok_or_else(|| "use auto, true, false, left or right".to_string())?,
            None => defaults.terminal.mac_option_as_alt,
        }),
        "jis-yen-backslash" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.terminal.jis_yen_to_backslash,
            };
            if wanted == editor.terminal.jis_yen_to_backslash {
                return Ok(None);
            }
            SettingsAction::ToggleJisYen
        }
        "clipboard-on-select" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.terminal.clipboard_on_select,
            };
            if wanted == editor.terminal.clipboard_on_select {
                return Ok(None);
            }
            SettingsAction::ToggleClipboardOnSelect
        }
        "copy-trims-gutter" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.terminal.copy_trims_gutter,
            };
            if wanted == editor.terminal.copy_trims_gutter {
                return Ok(None);
            }
            SettingsAction::ToggleCopyTrimsGutter
        }
        "word-separator" => SettingsAction::SetWordSeparator(value.map(str::to_string)),
        "middle-click-paste" => {
            let wanted = match value {
                Some(v) if v.eq_ignore_ascii_case("default") => None,
                Some(v) => Some(parse_bool(v)?),
                None => defaults.terminal.middle_click_paste,
            };
            if wanted == editor.terminal.middle_click_paste {
                return Ok(None);
            }
            SettingsAction::ToggleMiddleClickPaste
        }
        "auto-save" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.auto_save,
            };
            if wanted == editor.editor.auto_save {
                return Ok(None);
            }
            SettingsAction::ToggleAutoSave
        }
        "auto-save-delay" => SettingsAction::SetAutoSaveDelay(match value {
            Some(value) => value.parse().map_err(|_| "not milliseconds".to_string())?,
            None => defaults.editor.auto_save_delay_ms,
        }),
        "minimap" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.minimap_enabled,
            };
            if wanted == editor.editor.minimap_enabled {
                return Ok(None);
            }
            SettingsAction::ToggleMinimap
        }
        "editor-font" => SettingsAction::SelectEditorFontFamily(value.map(str::to_string)),
        "word-wrap" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.word_wrap,
            };
            if wanted == editor.editor.word_wrap {
                return Ok(None);
            }
            SettingsAction::ToggleWordWrap
        }
        "spellcheck" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.spellcheck_enabled,
            };
            if wanted == editor.editor.spellcheck_enabled {
                return Ok(None);
            }
            SettingsAction::ToggleSpellcheck
        }
        "markdown-review-tools" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.markdown_review_tools,
            };
            if wanted == editor.editor.markdown_review_tools {
                return Ok(None);
            }
            SettingsAction::ToggleMarkdownReview
        }
        "preview-tabs" => {
            let wanted = match value {
                Some(v) => parse_bool(v)?,
                None => defaults.editor.preview_tabs,
            };
            if wanted == editor.editor.preview_tabs {
                return Ok(None);
            }
            SettingsAction::TogglePreviewTabs
        }
        _ => return Err(format!("unknown setting “{key}”")),
    }))
}



/// With the app closed: save straight to the settings files.
fn save_directly(key: &str, value: Option<&str>) -> Result<(), String> {
    if key == "theme" {
        let mut ui = UiSettings::load();
        ui.theme_id = value.unwrap_or(DEFAULT_THEME_ID).to_string();
        ui.sync_os = false;
        ui.save();
        return Ok(());
    }
    let mut editor = EditorSettings::load();
    let defaults = EditorSettings::default();
    let owned = value.map(str::to_string);
    match key {
        "interface-font" => editor.terminal.ui_font_family = owned,
        "code-font" => editor.terminal.font_family = owned,
        "default-shell" => editor.terminal.default_shell = owned,
        "font-size" => {
            editor.terminal.font_size = match value {
                Some(value) => value.parse().map_err(|_| "not a number".to_string())?,
                None => defaults.terminal.font_size,
            }
        }
        "line-height" => {
            editor.terminal.line_height_pct = match value {
                Some(value) => value.parse().map_err(|_| "not a number".to_string())?,
                None => defaults.terminal.line_height_pct,
            }
        }
        "vim-mode" => editor.vim_mode_enabled = value.map(parse_bool).transpose()?.unwrap_or(defaults.vim_mode_enabled),
        "input-autodetect" => {
            editor.input_autodetection_enabled = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.input_autodetection_enabled)
        }
        "mac-option-as-alt" => {
            editor.terminal.mac_option_as_alt = match value {
                Some(v) => MacOptionAsAlt::parse(v)
                    .ok_or_else(|| "use auto, true, false, left or right".to_string())?,
                None => defaults.terminal.mac_option_as_alt,
            }
        }
        "jis-yen-backslash" => {
            editor.terminal.jis_yen_to_backslash = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.terminal.jis_yen_to_backslash)
        }
        "clipboard-on-select" => {
            editor.terminal.clipboard_on_select = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.terminal.clipboard_on_select)
        }
        "copy-trims-gutter" => {
            editor.terminal.copy_trims_gutter = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.terminal.copy_trims_gutter)
        }
        "word-separator" => editor.terminal.word_separator = owned.filter(|s| !s.is_empty()),
        "middle-click-paste" => {
            editor.terminal.middle_click_paste = match value {
                Some(v) if v.eq_ignore_ascii_case("default") => None,
                Some(v) => Some(parse_bool(v)?),
                None => defaults.terminal.middle_click_paste,
            }
        }
        "auto-save" => {
            editor.editor.auto_save = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.auto_save)
        }
        "auto-save-delay" => {
            editor.editor.auto_save_delay_ms = match value {
                Some(v) => v.parse().map_err(|_| "not milliseconds".to_string())?,
                None => defaults.editor.auto_save_delay_ms,
            }
            .clamp(AUTO_SAVE_DELAY_MIN_MS, AUTO_SAVE_DELAY_MAX_MS);
        }
        "minimap" => {
            editor.editor.minimap_enabled = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.minimap_enabled)
        }
        "editor-font" => editor.editor.font_family = owned.filter(|s| !s.is_empty()),
        "word-wrap" => {
            editor.editor.word_wrap = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.word_wrap)
        }
        "spellcheck" => {
            editor.editor.spellcheck_enabled = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.spellcheck_enabled)
        }
        "markdown-review-tools" => {
            editor.editor.markdown_review_tools = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.markdown_review_tools)
        }
        "preview-tabs" => {
            editor.editor.preview_tabs = value
                .map(parse_bool)
                .transpose()?
                .unwrap_or(defaults.editor.preview_tabs)
        }
        _ => return Err(format!("unknown setting “{key}”")),
    }
    editor.save();
    Ok(())
}

/// Apply a change: through the app when it's open, else to the files.
fn change(key: &str, value: Option<String>) -> Result<(), String> {
    if key == "teach-other-agents" {
        let on = value.as_deref().map(parse_bool).transpose()?.unwrap_or(true);
        super::agent_skill::set_enabled(on);
        return Ok(());
    }
    if instance::app_running() {
        ask_app(
            requests::Request::Setting {
                key: key.to_string(),
                value,
            },
            false,
        )
        .map(|_| ())
    } else {
        save_directly(key, value.as_deref())
    }
}

pub(super) fn run(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => {
            let rows = KEYS
                .iter()
                .map(|(key, about)| serde_json::json!({ "key": key, "value": current(key), "about": about }))
                .collect();
            Ok(listing(
                &flags,
                rows,
                |row| text(row, "key"),
                |row| format!("{}\t{}\t{}", text(row, "key"), text(row, "value"), text(row, "about")),
                "",
            ))
        }
        "get" => {
            let key = flags.argument(0).ok_or("usage: dragent settings get KEY")?;
            known(key)?;
            let value = current(key);
            Ok(if flags.json {
                pretty(serde_json::json!({ "key": key, "value": value }))
            } else {
                value
            })
        }
        "set" => {
            let key = flags.argument(0).ok_or("usage: dragent settings set KEY VALUE")?;
            known(key)?;
            let value = flags.positional[2..].join(" ");
            if value.is_empty() {
                return Err("usage: dragent settings set KEY VALUE".into());
            }
            let value = normalise(key, &value)?;
            change(key, value)?;
            let now = current(key);
            Ok(if flags.json {
                pretty(serde_json::json!({ "key": key, "value": now }))
            } else {
                format!("{key} is now {now}")
            })
        }
        "reset" => {
            let key = flags.argument(0).ok_or("usage: dragent settings reset KEY")?;
            known(key)?;
            change(key, None)?;
            let now = current(key);
            Ok(if flags.json {
                pretty(serde_json::json!({ "key": key, "value": now }))
            } else {
                format!("{key} is back to {now}")
            })
        }
        other => Err(format!("unknown settings command “{other}” (use list, get, set or reset)")),
    }
}

pub(super) fn theme(args: &[String]) -> Result<String, String> {
    let flags = Flags::parse(args)?;
    match flags.subcommand() {
        "list" => {
            let active = UiSettings::load().theme_id;
            let rows = Theme::all_infos()
                .iter()
                .map(|theme| {
                    serde_json::json!({
                        "id": theme.id,
                        "name": theme.name,
                        "active": theme.id == active,
                    })
                })
                .collect();
            Ok(listing(
                &flags,
                rows,
                |row| text(row, "id"),
                |row| {
                    format!(
                        "{}{}\t{}",
                        if row["active"].as_bool() == Some(true) { "* " } else { "  " },
                        text(row, "id"),
                        text(row, "name")
                    )
                },
                "",
            ))
        }
        "set" | "use" => {
            let id = flags.argument(0).ok_or("usage: dragent theme set ID (see `dragent theme list`)")?;
            change("theme", normalise("theme", id)?)?;
            Ok(if flags.json {
                pretty(serde_json::json!({ "theme": id }))
            } else {
                format!("Theme is now {id}")
            })
        }
        other => Err(format!("unknown theme command “{other}” (use list or set)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_checked_before_they_are_saved() {
        assert_eq!(normalise("font-size", "14").unwrap(), Some("14".into()));
        assert!(normalise("font-size", "40").is_err());
        assert_eq!(normalise("line-height", "150%").unwrap(), Some("150".into()));
        assert_eq!(normalise("code-font", "default").unwrap(), None);
        assert_eq!(normalise("vim-mode", "yes").unwrap(), Some("on".into()));
        assert!(normalise("theme", "no-such-theme").is_err());
        assert!(normalise("theme", DEFAULT_THEME_ID).is_ok());
        assert!(known("telemetry").is_err());
        // Terminal and editor keys.
        assert_eq!(
            normalise("mac-option-as-alt", "left").unwrap(),
            Some("left".into())
        );
        assert!(normalise("mac-option-as-alt", "sideways").is_err());
        assert_eq!(
            normalise("auto-save-delay", "1500").unwrap(),
            Some("1500".into())
        );
        assert!(normalise("auto-save-delay", "50").is_err());
        assert_eq!(
            normalise("middle-click-paste", "default").unwrap(),
            Some("default".into())
        );
        assert_eq!(normalise("word-separator", "default").unwrap(), None);
        assert_eq!(normalise("auto-save", "on").unwrap(), Some("on".into()));
        assert!(known("word-wrap").is_ok());
        assert!(known("preview-tabs").is_ok());
    }

    #[test]
    fn changes_map_to_the_settings_page_actions() {
        let editor = EditorSettings::default();
        assert!(matches!(
            action_for("font-size", Some("15"), &editor).unwrap(),
            Some(SettingsAction::SetTerminalFontSize(size)) if size == 15.0
        ));
        assert!(matches!(
            action_for("theme", None, &editor).unwrap(),
            Some(SettingsAction::SelectTheme(id)) if id == DEFAULT_THEME_ID
        ));
        let unchanged = on_off(editor.input_autodetection_enabled);
        assert!(action_for("input-autodetect", Some(&unchanged), &editor).unwrap().is_none());
        assert!(matches!(
            action_for("mac-option-as-alt", Some("left"), &editor).unwrap(),
            Some(SettingsAction::SelectMacOptionAsAlt(mode))
                if mode == MacOptionAsAlt::Left
        ));
        assert!(matches!(
            action_for("word-separator", None, &editor).unwrap(),
            Some(SettingsAction::SetWordSeparator(None))
        ));
        assert!(matches!(
            action_for("auto-save-delay", Some("2000"), &editor).unwrap(),
            Some(SettingsAction::SetAutoSaveDelay(ms)) if ms == 2000
        ));
    }
}
