use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::SettingsEvent;
use crate::setup::UiSettings;
use warpui::UpdateView;
use warpui::ViewContext;

/// Copy a user-picked theme YAML into `~/.dragent/themes/` after a
/// size guard and a structural check. Returns a human-readable status line.
/// The registry loads customs at startup, so a restart applies the import.
fn import_theme_file(path: &str) -> String {
    const MAX_BYTES: u64 = 256 * 1024;
    let src = std::path::Path::new(path);
    let ext_ok = src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    if !ext_ok {
        return "Import needs a .yaml theme file.".to_string();
    }
    let meta = std::fs::metadata(src);
    if meta.map(|m| m.len() > MAX_BYTES).unwrap_or(true) {
        return "Theme file is too large (256 KB max).".to_string();
    }
    let raw = match std::fs::read_to_string(src) {
        Ok(raw) => raw,
        Err(_) => return "Could not read that file.".to_string(),
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return "That file is not valid YAML.".to_string(),
    };
    for key in ["accent", "background", "foreground", "terminal_colors"] {
        if value.get(key).is_none() {
            return format!("Theme is missing `{key}` — not a valid theme file.");
        }
    }
    let stem: String = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("custom")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_lowercase().next().unwrap_or('_')
            } else {
                '_'
            }
        })
        .collect();
    let mut dir = dragent_paths::home();
    dir.push("themes");
    if std::fs::create_dir_all(&dir).is_err() {
        return "Could not create the themes folder.".to_string();
    }
    dir.push(format!("{stem}.yaml"));
    match std::fs::write(&dir, raw) {
        Ok(()) => format!("Theme imported as `{stem}` — restart Dragent to use it."),
        Err(_) => "Could not save the theme file.".to_string(),
    }
}

impl SettingsView {
    pub(crate) fn handle_theme(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::SelectTheme(id) => {
                self.theme_id = id.clone();
                let mut ui = UiSettings::load();
                ui.theme_id = id.clone();
                // A manual pick takes over from OS sync.
                ui.sync_os = false;
                ui.save();
                self.theme_sync_os = false;
                ctx.emit(SettingsEvent::ThemeChanged(id.clone()));
                let new_theme = self.theme();
                self.sync_machine_inputs(ctx);
                // EVERY editor must be re-themed, not just some: an editor caches
                // its text/placeholder/cursor colours at construction, so any one
                // missed here keeps the OLD theme's colours — which is why the
                // theme-search box stayed white-on-white after picking a light
                // theme (it was the one input you use *while* switching themes,
                // so it was the most visible omission).
                for editor in [
                    &self.search_editor,
                    &self.projects_search_editor,
                    &self.mcp_search_editor,
                    &self.skill_search_editor,
                    &self.shortcut_search_editor,
                    &self.theme_search_editor,
                    &self.index_search_editor,
                ] {
                    ctx.update_view(editor, |e, ectx| e.set_theme(new_theme, ectx));
                }
                for editor in self.provider_key_editors.values() {
                    ctx.update_view(editor, |e, ectx| e.set_theme(new_theme, ectx));
                }
            }
            SettingsAction::ToggleThemeSyncOs => {
                let mut ui = UiSettings::load();
                ui.sync_os = !ui.sync_os;
                ui.save();
                self.theme_sync_os = ui.sync_os;
                if ui.sync_os {
                    // Apply the OS match immediately; the poll timer and
                    // settings-open keep it in sync afterwards.
                    Self::apply_os_theme(self, ctx);
                    Self::spawn_os_theme_poll(ctx);
                }
                ctx.notify();
            }
            SettingsAction::ImportTheme => {
                ctx.open_file_picker(
                    move |result, ctx| match result {
                        Ok(paths) => {
                            if let Some(path) = paths.into_iter().next() {
                                ctx.dispatch_typed_action_deferred(
                                    SettingsAction::ThemeFilePicked { path },
                                );
                            }
                        }
                        Err(error) => {
                            log::warn!("[settings] theme import picker: {error}");
                        }
                    },
                    warpui_core::platform::file_picker::FilePickerConfiguration::new()
                        .set_allowed_file_types(vec![
                            warpui_core::platform::file_picker::FileType::Yaml,
                        ]),
                );
                ctx.notify();
            }
            SettingsAction::ThemeFilePicked { path } => {
                self.theme_status_message = Some(import_theme_file(path));
                ctx.notify();
            }
            SettingsAction::OpenThemeChooser => {
                self.theme_chooser_open = true;
                ctx.notify();
            }
            SettingsAction::CloseThemeChooser => {
                self.theme_chooser_open = false;
                ctx.notify();
            }
            SettingsAction::ToggleVimMode => {
                self.editor_settings.vim_mode_enabled = !self.editor_settings.vim_mode_enabled;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleInputAutodetect => {
                self.editor_settings.input_autodetection_enabled =
                    !self.editor_settings.input_autodetection_enabled;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectTerminalFont(family) => {
                self.editor_settings.terminal.font_family = family.clone();
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectInterfaceFont(family) => {
                self.editor_settings.terminal.ui_font_family = family.clone();
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetTerminalFontSize(size) => {
                use crate::console::editor_settings::{FONT_SIZE_MAX, FONT_SIZE_MIN};
                self.editor_settings.terminal.font_size =
                    size.round().clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetTerminalLineHeight(pct) => {
                use crate::console::editor_settings::{LINE_HEIGHT_PCT_MAX, LINE_HEIGHT_PCT_MIN};
                self.editor_settings.terminal.line_height_pct =
                    (*pct).clamp(LINE_HEIGHT_PCT_MIN, LINE_HEIGHT_PCT_MAX);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectCursorStyle(style) => {
                self.editor_settings.terminal.cursor_style = *style;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleCursorBlink => {
                self.editor_settings.terminal.cursor_blink =
                    !self.editor_settings.terminal.cursor_blink;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetCursorBlinkRate(ms) => {
                use crate::console::editor_settings::{
                    CURSOR_BLINK_RATE_MAX_MS, CURSOR_BLINK_RATE_MIN_MS,
                };
                self.editor_settings.terminal.cursor_blink_rate_ms =
                    (*ms).clamp(CURSOR_BLINK_RATE_MIN_MS, CURSOR_BLINK_RATE_MAX_MS);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectEditorFontWeight(weight) => {
                self.editor_settings.editor.font_weight = *weight;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetTabSize(size) => {
                use crate::console::editor_settings::{TAB_SIZE_MAX, TAB_SIZE_MIN};
                self.editor_settings.editor.tab_size = (*size).clamp(TAB_SIZE_MIN, TAB_SIZE_MAX);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleInsertSpaces => {
                self.editor_settings.editor.insert_spaces =
                    !self.editor_settings.editor.insert_spaces;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleLineNumbers => {
                self.editor_settings.editor.line_numbers =
                    !self.editor_settings.editor.line_numbers;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleShowWhitespace => {
                self.editor_settings.editor.show_whitespace =
                    !self.editor_settings.editor.show_whitespace;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleIndentGuides => {
                self.editor_settings.editor.indent_guides =
                    !self.editor_settings.editor.indent_guides;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetVimMode(enabled) => {
                self.editor_settings.vim_mode_enabled = *enabled;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectMacOptionAsAlt(mode) => {
                self.editor_settings.terminal.mac_option_as_alt = *mode;
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleJisYen => {
                self.editor_settings.terminal.jis_yen_to_backslash =
                    !self.editor_settings.terminal.jis_yen_to_backslash;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleClipboardOnSelect => {
                self.editor_settings.terminal.clipboard_on_select =
                    !self.editor_settings.terminal.clipboard_on_select;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleCopyTrimsGutter => {
                self.editor_settings.terminal.copy_trims_gutter =
                    !self.editor_settings.terminal.copy_trims_gutter;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetWordSeparator(sep) => {
                let normalized = sep
                    .clone()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                self.editor_settings.terminal.word_separator = normalized;
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleMiddleClickPaste => {
                let currently = self.editor_settings.terminal.middle_click_paste_enabled();
                self.editor_settings.terminal.middle_click_paste = Some(!currently);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetScrollbackLines(lines) => {
                use crate::console::editor_settings::{
                    SCROLLBACK_LINES_DEFAULT, SCROLLBACK_LINES_MAX, SCROLLBACK_LINES_MIN,
                };
                self.editor_settings.terminal.scrollback_lines = if *lines == 0 {
                    SCROLLBACK_LINES_DEFAULT
                } else {
                    (*lines).clamp(SCROLLBACK_LINES_MIN, SCROLLBACK_LINES_MAX)
                };
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleAutoSave => {
                self.editor_settings.editor.auto_save = !self.editor_settings.editor.auto_save;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SetAutoSaveDelay(ms) => {
                use crate::console::editor_settings::{
                    AUTO_SAVE_DELAY_MAX_MS, AUTO_SAVE_DELAY_MIN_MS,
                };
                self.editor_settings.editor.auto_save_delay_ms =
                    (*ms).clamp(AUTO_SAVE_DELAY_MIN_MS, AUTO_SAVE_DELAY_MAX_MS);
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleMinimap => {
                self.editor_settings.editor.minimap_enabled =
                    !self.editor_settings.editor.minimap_enabled;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::SelectEditorFontFamily(family) => {
                self.editor_settings.editor.font_family = family.clone();
                self.open_picker = None;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleWordWrap => {
                self.editor_settings.editor.word_wrap = !self.editor_settings.editor.word_wrap;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleSpellcheck => {
                self.editor_settings.editor.spellcheck_enabled =
                    !self.editor_settings.editor.spellcheck_enabled;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::ToggleMarkdownReview => {
                self.editor_settings.editor.markdown_review_tools =
                    !self.editor_settings.editor.markdown_review_tools;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            SettingsAction::TogglePreviewTabs => {
                self.editor_settings.editor.preview_tabs =
                    !self.editor_settings.editor.preview_tabs;
                self.editor_settings.save();
                ctx.emit(SettingsEvent::EditorSettingsChanged);
            }
            _ => {}
        }
    }
}
