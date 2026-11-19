//! Keystroke → PTY byte encoding for full-screen TUI passthrough.
//!
//! # CSI modifier sequences
//! When Shift, Alt, or Ctrl is combined with a navigation key the terminal must
//! send a CSI modifier parameter instead of the bare escape.  The modifier code
//! is `shift | (alt << 1) | (ctrl << 2) + 1`, e.g. Ctrl = 5, Shift = 2.
//!
//! This approach is modelled after Alacritty's key-binding encoder
//! (Apache 2.0, <https://github.com/alacritty/alacritty>).

use dragent_terminal::TermMode;
use warpui::keymap::Keystroke;

/// Build the xterm CSI modifier parameter (2–8) or return `None` when no
/// modifier is active. The value follows the XTerm convention:
///   `shift | (alt << 1) | (ctrl << 2)  + 1`
fn csi_modifier(keystroke: &Keystroke) -> Option<u8> {
    let bits =
        (keystroke.shift as u8) | ((keystroke.alt as u8) << 1) | ((keystroke.ctrl as u8) << 2);
    if bits == 0 {
        None
    } else {
        Some(bits + 1)
    }
}

/// Encode a keystroke for the PTY using the emulator's active mode flags.
pub fn encode_keystroke(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let app_keypad = mode.contains(TermMode::APP_KEYPAD);
    let _ = app_keypad; // keypad SS3 expansion in a later sprint

    if keystroke.ctrl && !keystroke.cmd && !keystroke.alt {
        match keystroke.key.to_ascii_lowercase().as_str() {
            "c" => return Some(vec![0x03]),
            "d" => return Some(vec![0x04]),
            "l" => return Some(vec![0x0c]),
            "u" => return Some(vec![0x15]),
            "z" => return Some(vec![0x1a]),
            _ => {}
        }
    }

    if keystroke.ctrl && !keystroke.cmd && !keystroke.alt && keystroke.key.len() == 1 {
        let c = keystroke.key.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }

    if keystroke.ctrl && !keystroke.cmd && !keystroke.alt {
        let bytes: Option<Vec<u8>> = match keystroke.key.as_str() {
            "[" => Some(vec![0x1b]),
            "\\" => Some(vec![0x1c]),
            "]" => Some(vec![0x1d]),
            "^" => Some(vec![0x1e]),
            "_" => Some(vec![0x1f]),
            " " | "2" | "@" => Some(vec![0x00]),
            _ => None,
        };
        if bytes.is_some() {
            return bytes;
        }
    }

    // Navigation keys combined with Shift / Alt / Ctrl send CSI modifier
    // sequences (`ESC [ 1 ; <mod> <letter>` or `ESC [ <code> ; <mod> ~`).
    // This must be checked BEFORE the plain match below so that e.g.
    // Ctrl+End returns `\x1b[1;5F` and not the bare `\x1b[F`.
    //
    // Inspired by Alacritty's approach (Apache 2.0,
    // https://github.com/alacritty/alacritty): modifier presence overrides
    // the app-cursor SS3 form — CSI is always used when a modifier is present.
    if !keystroke.cmd {
        if let Some(m) = csi_modifier(keystroke) {
            match keystroke.key.as_str() {
                "up" => return Some(format!("\x1b[1;{m}A").into_bytes()),
                "down" => return Some(format!("\x1b[1;{m}B").into_bytes()),
                "right" => return Some(format!("\x1b[1;{m}C").into_bytes()),
                "left" => return Some(format!("\x1b[1;{m}D").into_bytes()),
                "home" => return Some(format!("\x1b[1;{m}H").into_bytes()),
                "end" => return Some(format!("\x1b[1;{m}F").into_bytes()),
                "pageup" => return Some(format!("\x1b[5;{m}~").into_bytes()),
                "pagedown" => return Some(format!("\x1b[6;{m}~").into_bytes()),
                "delete" => return Some(format!("\x1b[3;{m}~").into_bytes()),
                "insert" => return Some(format!("\x1b[2;{m}~").into_bytes()),
                "f1" => return Some(format!("\x1b[1;{m}P").into_bytes()),
                "f2" => return Some(format!("\x1b[1;{m}Q").into_bytes()),
                "f3" => return Some(format!("\x1b[1;{m}R").into_bytes()),
                "f4" => return Some(format!("\x1b[1;{m}S").into_bytes()),
                "f5" => return Some(format!("\x1b[15;{m}~").into_bytes()),
                "f6" => return Some(format!("\x1b[17;{m}~").into_bytes()),
                "f7" => return Some(format!("\x1b[18;{m}~").into_bytes()),
                "f8" => return Some(format!("\x1b[19;{m}~").into_bytes()),
                "f9" => return Some(format!("\x1b[20;{m}~").into_bytes()),
                "f10" => return Some(format!("\x1b[21;{m}~").into_bytes()),
                "f11" => return Some(format!("\x1b[23;{m}~").into_bytes()),
                "f12" => return Some(format!("\x1b[24;{m}~").into_bytes()),
                _ => {}
            }
        }
    }

    match keystroke.key.as_str() {
        "enter" => Some(vec![b'\r']),
        "backspace" => Some(vec![0x7f]),
        "tab" => Some(if keystroke.shift {
            b"\x1b[Z".to_vec()
        } else {
            vec![b'\t']
        }),
        "escape" => Some(vec![0x1b]),
        "space" => Some(vec![b' ']),
        "up" => Some(if app_cursor {
            b"\x1bOA".to_vec()
        } else {
            b"\x1b[A".to_vec()
        }),
        "down" => Some(if app_cursor {
            b"\x1bOB".to_vec()
        } else {
            b"\x1b[B".to_vec()
        }),
        "right" => Some(if app_cursor {
            b"\x1bOC".to_vec()
        } else {
            b"\x1b[C".to_vec()
        }),
        "left" => Some(if app_cursor {
            b"\x1bOD".to_vec()
        } else {
            b"\x1b[D".to_vec()
        }),
        "home" => Some(if app_cursor {
            b"\x1bOH".to_vec()
        } else {
            b"\x1b[H".to_vec()
        }),
        "end" => Some(if app_cursor {
            b"\x1bOF".to_vec()
        } else {
            b"\x1b[F".to_vec()
        }),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(b"\x1b[15~".to_vec()),
        "f6" => Some(b"\x1b[17~".to_vec()),
        "f7" => Some(b"\x1b[18~".to_vec()),
        "f8" => Some(b"\x1b[19~".to_vec()),
        "f9" => Some(b"\x1b[20~".to_vec()),
        "f10" => Some(b"\x1b[21~".to_vec()),
        "f11" => Some(b"\x1b[23~".to_vec()),
        "f12" => Some(b"\x1b[24~".to_vec()),
        _ if keystroke.ctrl || keystroke.cmd || keystroke.meta => None,
        key if key.chars().count() == 1 => {
            let mut c = key.chars().next()?;
            if keystroke.shift && c.is_ascii_lowercase() {
                c = c.to_ascii_uppercase();
            }
            let mut bytes = Vec::new();
            if keystroke.alt {
                bytes.push(0x1b);
            }
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            Some(bytes)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_csi_when_normal_cursor() {
        let ks = Keystroke {
            caps_lock: false,
            key: "up".to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn arrow_ss3_when_app_cursor() {
        let ks = Keystroke {
            caps_lock: false,
            key: "up".to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        let mode = TermMode::APP_CURSOR;
        assert_eq!(encode_keystroke(&ks, mode), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn ctrl_c_sends_etx() {
        let ks = Keystroke {
            caps_lock: false,
            key: "c".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(encode_keystroke(&ks, TermMode::DEFAULT), Some(vec![0x03]));
    }

    #[test]
    fn ctrl_l_sends_ff() {
        let ks = Keystroke {
            caps_lock: false,
            key: "l".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(encode_keystroke(&ks, TermMode::DEFAULT), Some(vec![0x0c]));
    }

    // Inspired by Alacritty (Apache 2.0, https://github.com/alacritty/alacritty):
    // navigation keys with modifiers must send CSI modifier sequences so that TUI
    // apps like Claude Code can distinguish e.g. Ctrl+End from plain End.

    #[test]
    fn ctrl_end_sends_csi_5f() {
        let ks = Keystroke {
            caps_lock: false,
            key: "end".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[1;5F".to_vec())
        );
    }

    #[test]
    fn ctrl_home_sends_csi_5h() {
        let ks = Keystroke {
            caps_lock: false,
            key: "home".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[1;5H".to_vec())
        );
    }

    #[test]
    fn shift_end_sends_csi_2f() {
        let ks = Keystroke {
            caps_lock: false,
            key: "end".to_string(),
            ctrl: false,
            alt: false,
            shift: true,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[1;2F".to_vec())
        );
    }

    #[test]
    fn ctrl_up_sends_csi_modifier() {
        let ks = Keystroke {
            caps_lock: false,
            key: "up".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[1;5A".to_vec())
        );
    }

    #[test]
    fn ctrl_pagedown_sends_csi_modifier() {
        let ks = Keystroke {
            caps_lock: false,
            key: "pagedown".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[6;5~".to_vec())
        );
    }

    #[test]
    fn ctrl_end_overrides_app_cursor_mode() {
        // Modifier sequences always use CSI, never SS3, regardless of app_cursor.
        let ks = Keystroke {
            caps_lock: false,
            key: "end".to_string(),
            ctrl: true,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::APP_CURSOR),
            Some(b"\x1b[1;5F".to_vec())
        );
    }

    #[test]
    fn plain_end_respects_app_cursor() {
        let ks = Keystroke {
            caps_lock: false,
            key: "end".to_string(),
            ctrl: false,
            alt: false,
            shift: false,
            cmd: false,
            meta: false,
        };
        assert_eq!(
            encode_keystroke(&ks, TermMode::APP_CURSOR),
            Some(b"\x1bOF".to_vec())
        );
        assert_eq!(
            encode_keystroke(&ks, TermMode::DEFAULT),
            Some(b"\x1b[F".to_vec())
        );
    }
}
