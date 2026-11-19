//! Key routing while an AI rich-input panel is open above a live PTY session.
//!
//! Codex, Claude Code, and Cursor often enable mouse reporting or alternate-screen
//! modes while showing arrow-driven menus. Users should not have to collapse the
//! rich input panel to send those keys — we detect interactive PTY state and
//! forward navigation keys even when the rich editor holds focus.

use dragent_terminal::{MouseReportingMode, TermMode};
use warpui::keymap::Keystroke;

/// Keys commonly used by TUIs for menus, lists, and confirmations.
pub fn is_pty_interaction_key(keystroke: &Keystroke) -> bool {
    if keystroke.cmd || keystroke.ctrl || keystroke.alt {
        return false;
    }
    match keystroke.key.as_str() {
        "up" | "down" | "left" | "right" | "tab" | "space" | "home" | "end" | "pageup"
        | "pagedown" => true,
        "enter" => !keystroke.shift,
        _ => false,
    }
}

/// Heuristic: the active emulator is likely showing an interactive picker/menu.
pub fn pty_expects_interactive_input(
    mouse_reporting: MouseReportingMode,
    alt_active: bool,
    term_mode: TermMode,
) -> bool {
    mouse_reporting != MouseReportingMode::None
        || alt_active
        || term_mode.contains(TermMode::APP_CURSOR)
        || term_mode.contains(TermMode::APP_KEYPAD)
}

/// Rich-input state used to decide whether Enter should stay in the editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RichInputKeyContext {
    pub focused: bool,
    pub has_text: bool,
    /// A ghost (autocomplete) suffix is showing — → should accept it locally
    /// instead of being forwarded to the PTY.
    pub has_ghost: bool,
}

/// Forward this keystroke to the PTY while the AI panel is expanded.
///
/// - **Shift + navigation key** — always forward (explicit “send to terminal” override).
/// - **Navigation / confirm keys** — forward when the PTY looks interactive.
/// - **Enter** — stays in the rich editor when it is focused and has text (submit locally).
pub fn should_forward_key_to_pty_while_ai_panel_open(
    keystroke: &Keystroke,
    command_running: bool,
    mouse_reporting: MouseReportingMode,
    alt_active: bool,
    term_mode: TermMode,
    rich_input: RichInputKeyContext,
    ai_tool_running: bool,
) -> bool {
    if !command_running {
        return false;
    }
    // Shift+Up/Down are ALWAYS reserved for the AI rich input's command-history
    // recall — keep them local even over an interactive tool (otherwise the
    // interactive-PTY branch below would forward them and history wouldn't work).
    if keystroke.shift && (keystroke.key == "up" || keystroke.key == "down") {
        return false;
    }
    // Shift+navigation is otherwise the explicit "send to terminal" override.
    if keystroke.shift && is_pty_interaction_key(keystroke) {
        return true;
    }
    if keystroke.key == "enter" && !keystroke.shift && rich_input.focused && rich_input.has_text {
        return false;
    }
    // With a ghost autocomplete suggestion showing, the accept keys — Tab (the
    // standard completion key) and a bare → — stay in the editor so they accept
    // the suggestion, instead of being forwarded to the PTY. Parity with the
    // terminal command input's accept-ghost affordance.
    if rich_input.has_ghost
        && !keystroke.shift
        && (keystroke.key == "tab" || keystroke.key == "right")
    {
        return false;
    }
    if !is_pty_interaction_key(keystroke) {
        return false;
    }
    // A known AI tool with an EMPTY composer: the user is navigating the
    // tool's own UI (menus, pickers, confirmations). Ink apps (Gemini, Codex)
    // read raw keys without enabling mouse/app-cursor modes, so the
    // interactive-PTY heuristic below can't see them — forward outright.
    // Once the composer has text, arrows edit locally and Enter submits.
    if ai_tool_running && !rich_input.has_text {
        return true;
    }
    pty_expects_interactive_input(mouse_reporting, alt_active, term_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_terminal::TermMode;

    fn ks(key: &str) -> Keystroke {
        Keystroke {
            caps_lock: false,
            key: key.to_string(),
            ctrl: false,
            alt: false,
            cmd: false,
            shift: false,
            meta: false,
        }
    }

    fn ks_shift(key: &str) -> Keystroke {
        Keystroke {
            shift: true,
            ..ks(key)
        }
    }

    fn no_rich_input() -> RichInputKeyContext {
        RichInputKeyContext::default()
    }

    fn rich_input(focused: bool, has_text: bool) -> RichInputKeyContext {
        RichInputKeyContext {
            focused,
            has_text,
            has_ghost: false,
        }
    }

    fn rich_input_with_ghost(focused: bool, has_text: bool) -> RichInputKeyContext {
        RichInputKeyContext {
            focused,
            has_text,
            has_ghost: true,
        }
    }

    #[test]
    fn shift_arrow_forwards_while_running_except_up_down() {
        // Shift+Left/Right stay the explicit "send to terminal" override.
        assert!(should_forward_key_to_pty_while_ai_panel_open(
            &ks_shift("right"),
            true,
            MouseReportingMode::None,
            false,
            TermMode::DEFAULT,
            no_rich_input(),
            false,
        ));
        // Shift+Up/Down are reserved for rich-input command-history recall — even
        // over an interactive PTY (which would otherwise forward navigation keys).
        for key in ["up", "down"] {
            for (mouse, alt, mode) in [
                (MouseReportingMode::None, false, TermMode::DEFAULT),
                (MouseReportingMode::ButtonMotion, true, TermMode::APP_CURSOR),
            ] {
                assert!(
                    !should_forward_key_to_pty_while_ai_panel_open(
                        &ks_shift(key),
                        true,
                        mouse,
                        alt,
                        mode,
                        rich_input(true, true),
                        true,
                    ),
                    "shift+{key} should be reserved for history, not forwarded"
                );
            }
        }
    }

    #[test]
    fn plain_arrow_forwards_when_mouse_reporting_on() {
        assert!(should_forward_key_to_pty_while_ai_panel_open(
            &ks("down"),
            true,
            MouseReportingMode::ButtonMotion,
            false,
            TermMode::DEFAULT,
            no_rich_input(),
            false,
        ));
    }

    #[test]
    fn plain_arrow_stays_in_editor_when_pty_idle() {
        assert!(!should_forward_key_to_pty_while_ai_panel_open(
            &ks("down"),
            true,
            MouseReportingMode::None,
            false,
            TermMode::DEFAULT,
            no_rich_input(),
            false,
        ));
    }

    #[test]
    fn enter_forwards_on_alt_screen_when_editor_empty() {
        assert!(should_forward_key_to_pty_while_ai_panel_open(
            &ks("enter"),
            true,
            MouseReportingMode::None,
            true,
            TermMode::DEFAULT,
            rich_input(true, false),
            false,
        ));
    }

    #[test]
    fn enter_stays_in_editor_when_focused_with_text() {
        assert!(!should_forward_key_to_pty_while_ai_panel_open(
            &ks("enter"),
            true,
            MouseReportingMode::None,
            true,
            TermMode::DEFAULT,
            rich_input(true, true),
            false,
        ));
    }

    #[test]
    fn empty_composer_over_ai_tool_forwards_navigation() {
        // Gemini (Ink) shows menus WITHOUT mouse/app-cursor modes: with the
        // composer empty, arrows and Enter must drive the tool's UI.
        for key in ["up", "down", "enter", "tab", "space"] {
            assert!(
                should_forward_key_to_pty_while_ai_panel_open(
                    &ks(key),
                    true,
                    MouseReportingMode::None,
                    false,
                    TermMode::empty(),
                    rich_input(true, false),
                    true,
                ),
                "{key} should forward to the AI tool when composer is empty"
            );
        }
    }

    #[test]
    fn tab_and_right_accept_ghost_locally_even_over_interactive_pty() {
        // With a ghost suggestion showing, the accept keys (Tab, →) must stay in
        // the editor so they accept the suggestion — even when the PTY looks
        // interactive (alt-screen / mouse reporting), which would otherwise
        // forward those keys to the tool.
        for key in ["tab", "right"] {
            assert!(
                !should_forward_key_to_pty_while_ai_panel_open(
                    &ks(key),
                    true,
                    MouseReportingMode::ButtonMotion,
                    true,
                    TermMode::APP_CURSOR,
                    rich_input_with_ghost(true, true),
                    true,
                ),
                "{key} with a ghost suggestion must be accepted locally, not forwarded"
            );
            // Without a ghost, the key over an interactive PTY still forwards.
            assert!(
                should_forward_key_to_pty_while_ai_panel_open(
                    &ks(key),
                    true,
                    MouseReportingMode::ButtonMotion,
                    true,
                    TermMode::APP_CURSOR,
                    rich_input(true, true),
                    true,
                ),
                "{key} without a ghost still forwards to the tool"
            );
        }
        // Shift+→ remains the explicit "send to terminal" override even with a ghost.
        assert!(should_forward_key_to_pty_while_ai_panel_open(
            &ks_shift("right"),
            true,
            MouseReportingMode::None,
            false,
            TermMode::empty(),
            rich_input_with_ghost(true, true),
            true,
        ));
    }

    #[test]
    fn composing_text_keeps_arrows_and_enter_local() {
        for key in ["up", "down"] {
            assert!(
                !should_forward_key_to_pty_while_ai_panel_open(
                    &ks(key),
                    true,
                    MouseReportingMode::None,
                    false,
                    TermMode::empty(),
                    rich_input(true, true),
                    true,
                ),
                "{key} should stay in the editor while composing"
            );
        }
        // Enter submits the composed text locally.
        assert!(!should_forward_key_to_pty_while_ai_panel_open(
            &ks("enter"),
            true,
            MouseReportingMode::None,
            false,
            TermMode::empty(),
            rich_input(true, true),
            true,
        ));
        // Shift+Right remains the explicit "send to terminal" override
        // (Shift+Up/Down are reserved for history — covered by its own test).
        assert!(should_forward_key_to_pty_while_ai_panel_open(
            &ks_shift("right"),
            true,
            MouseReportingMode::None,
            false,
            TermMode::empty(),
            rich_input(true, true),
            true,
        ));
    }
}
