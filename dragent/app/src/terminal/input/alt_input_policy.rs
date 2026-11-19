//! Mouse and scroll routing for alt-screen and block output.
//! Also hosts terminal-input pref helpers (mac Option as Alt,
//! JIS yen, clipboard-on-select, middle-click, word separators, gutter trim).

use dragent_terminal::MouseReportingMode;
use warpui::keymap::Keystroke;

/// Whether macOS Option should act as Alt for this keystroke, per settings.
/// Non-macOS platforms always return the incoming `alt` unchanged except when
/// the pref explicitly forces True/False.
pub fn apply_mac_option_policy(mut keystroke: Keystroke) -> Keystroke {
    let treats = crate::console::editor_settings::EditorSettings::global()
        .terminal
        .mac_option_as_alt
        .treats_option_as_alt();
    if !treats {
        keystroke.alt = false;
    }
    keystroke
}

/// Whether the JIS ¥ key should be mapped to backslash for the PTY.
pub fn jis_yen_to_backslash_enabled() -> bool {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .jis_yen_to_backslash
}

/// Map a JIS Yen keystroke to backslash bytes when the pref is on.
pub fn yen_keystroke_bytes(keystroke: &Keystroke) -> Option<Vec<u8>> {
    if !jis_yen_to_backslash_enabled() {
        return None;
    }
    let key = keystroke.key.as_str();
    // JIS keyboards report ¥ (U+00A5) or fullwidth ¥ (U+FFE5) where US
    // keyboards have backslash.
    if key == "¥" || key == "￥" || key == "\\" && keystroke_is_jis_yen(keystroke) {
        return Some(vec![0x5c]);
    }
    // WarpUI may surface the Yen physical key as `KeyCode::Yen` with a
    // backslash-ish key label; the explicit char checks above cover it.
    None
}

fn keystroke_is_jis_yen(_keystroke: &Keystroke) -> bool {
    // Placeholder for KeyCode-aware detection: WarpUI `Keystroke` carries only
    // the logical key here, so the char checks in `yen_keystroke_bytes` are
    // authoritative. Kept as a hook for future KeyCode plumbing.
    false
}

/// Whether terminal drag-selection should auto-copy to the clipboard.
pub fn clipboard_on_select_enabled() -> bool {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .clipboard_on_select
}

/// Whether terminal copy should trim gutter whitespace.
pub fn copy_trims_gutter_enabled() -> bool {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .copy_trims_gutter
}

/// Whether middle-click should paste the clipboard.
pub fn middle_click_paste_enabled() -> bool {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .middle_click_paste_enabled()
}

/// Extra word-break chars for double-click selection, if configured.
pub fn extra_word_separators() -> Option<String> {
    crate::console::editor_settings::EditorSettings::global()
        .terminal
        .extra_word_separators()
        .map(str::to_string)
}

/// Trim gutter indentation from copied terminal text when the pref is on.
/// Gutter output is leading whitespace / line-number rails; trimming keeps
/// only the relative indentation (mirrors `trim_copy_indentation` semantics
/// for the input editor).
pub fn maybe_trim_gutter(text: String) -> String {
    if !copy_trims_gutter_enabled() {
        return text;
    }
    trim_gutter_indentation(&text)
}

fn trim_gutter_indentation(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return text.to_string();
    }
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    if min_indent == 0 {
        return text.to_string();
    }
    let trailing_newline = text.ends_with('\n');
    let mut out: Vec<String> = lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                l[min_indent.min(l.len())..].to_string()
            }
        })
        .collect();
    let mut joined = out.join("\n");
    if trailing_newline {
        joined.push('\n');
    }
    let _ = std::mem::replace(&mut out, Vec::new());
    joined
}

/// Whether a pointer press/drag should be encoded for the PTY instead of updating selection.
///
/// Alt/Option is the local-selection override modifier (matches what terminal multiplexers
/// and Claude Code advertise as "option+click to native select"). Shift passes through to PTY.
pub fn should_passthrough_mouse(alt_held: bool, mouse_reporting: MouseReportingMode) -> bool {
    if alt_held {
        return false;
    }
    mouse_reporting != MouseReportingMode::None
}

/// Whether a left-button gesture on the alt screen belongs to the app, so its
/// own click and drag selection run. Otherwise we select locally: the app did
/// not ask for the mouse, Option is held, or this is a view-only shared session.
pub fn alt_left_button_goes_to_app(
    passthrough: bool,
    app_wants_mouse: bool,
    read_only: bool,
) -> bool {
    passthrough && app_wants_mouse && !read_only
}

/// Whether wheel events should be encoded as terminal mouse wheel buttons.
pub fn should_encode_wheel_as_mouse(mouse_reporting: MouseReportingMode) -> bool {
    mouse_reporting != MouseReportingMode::None
}

/// Fallback scroll when wheel is not sent as mouse: repeated arrow key sequences.
pub fn scroll_arrow_bytes(delta_y: f32, app_cursor: bool) -> Vec<u8> {
    let seq: &[u8] = if delta_y > 0.0 {
        if app_cursor {
            b"\x1bOA"
        } else {
            b"\x1b[A"
        }
    } else if app_cursor {
        b"\x1bOB"
    } else {
        b"\x1b[B"
    };
    // A browser/trackpad delta is a physical distance, not a count of
    // terminal keypresses. Repeating by `ceil(delta)` makes a single large
    // trackpad event turn into a very fast scroll burst.
    seq.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_button_goes_to_an_app_that_asked_for_the_mouse() {
        assert!(alt_left_button_goes_to_app(true, true, false));
        // The app didn't ask (or Option is held): select locally.
        assert!(!alt_left_button_goes_to_app(false, true, false));
        assert!(!alt_left_button_goes_to_app(true, false, false));
        // A view-only viewer can copy but can't drive the host's app.
        assert!(!alt_left_button_goes_to_app(true, true, true));
    }

    #[test]
    fn alt_blocks_passthrough() {
        assert!(!should_passthrough_mouse(
            true,
            MouseReportingMode::ButtonMotion
        ));
    }

    #[test]
    fn auto_follows_mouse_mode() {
        assert!(should_passthrough_mouse(false, MouseReportingMode::Normal));
        assert!(!should_passthrough_mouse(false, MouseReportingMode::None));
    }

    #[test]
    fn scroll_delta_is_one_terminal_step() {
        assert_eq!(scroll_arrow_bytes(1.0, false), b"\x1b[A");
        assert_eq!(scroll_arrow_bytes(48.0, false), b"\x1b[A");
        assert_eq!(scroll_arrow_bytes(-48.0, true), b"\x1bOB");
    }

    #[test]
    fn scroll_auto_uses_mouse_when_reporting() {
        assert!(should_encode_wheel_as_mouse(MouseReportingMode::Normal));
        assert!(!should_encode_wheel_as_mouse(MouseReportingMode::None));
    }
}
