//! Key specs as agents write them: `Return`, `cmd+s`, `ctrl+shift+t`,
//! `CmdOrCtrl+A`. Platforms map the result to their own key codes.

/// A key with no character of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    Return,
    Tab,
    Space,
    /// Deletes backwards (the key labelled Delete on a Mac).
    Backspace,
    /// Deletes forwards.
    Delete,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Named(Named),
    /// A character key, lowercase for letters.
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub key: Key,
    /// Command on macOS, the Windows key elsewhere.
    pub meta: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Read a key spec. Modifiers and the key are joined with `+`; `cmdorctrl`
/// (also `mod`, `primary`) is Command on macOS and Control elsewhere.
pub fn parse(spec: &str) -> Result<Combo, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("Name a key, such as Return or cmd+s.".into());
    }
    // `cmd++` presses plus: the last `+` is the key itself.
    let (modifiers, key) = match spec.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None => match spec.rfind('+') {
            Some(i) if i + 1 < spec.len() => (&spec[..i], &spec[i + 1..]),
            _ => ("", spec),
        },
    };
    let mut combo = Combo {
        key: key_named(key)?,
        meta: false,
        ctrl: false,
        alt: false,
        shift: false,
    };
    for modifier in modifiers
        .split('+')
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        match modifier.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" | "win" | "windows" => combo.meta = true,
            "ctrl" | "control" => combo.ctrl = true,
            "alt" | "option" | "opt" => combo.alt = true,
            "shift" => combo.shift = true,
            "cmdorctrl" | "commandorcontrol" | "mod" | "primary" => {
                if cfg!(target_os = "macos") {
                    combo.meta = true
                } else {
                    combo.ctrl = true
                }
            }
            other => {
                return Err(format!(
                    "“{other}” isn't a modifier; use cmd, ctrl, alt or shift."
                ))
            }
        }
    }
    Ok(combo)
}

fn key_named(key: &str) -> Result<Key, String> {
    let lower = key.trim().to_ascii_lowercase();
    let named = match lower.as_str() {
        "return" | "enter" => Named::Return,
        "tab" => Named::Tab,
        "space" | "spacebar" => Named::Space,
        "backspace" => Named::Backspace,
        "delete" | "del" | "forwarddelete" => Named::Delete,
        "escape" | "esc" => Named::Escape,
        "left" | "arrowleft" => Named::Left,
        "right" | "arrowright" => Named::Right,
        "up" | "arrowup" => Named::Up,
        "down" | "arrowdown" => Named::Down,
        "home" => Named::Home,
        "end" => Named::End,
        "pageup" | "page_up" | "pgup" => Named::PageUp,
        "pagedown" | "page_down" | "pgdn" => Named::PageDown,
        f if f.len() > 1 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => {
            match f[1..].parse::<u8>() {
                Ok(n @ 1..=20) => Named::F(n),
                _ => return Err(format!("“{key}” isn't a key this can press.")),
            }
        }
        _ => {
            let mut chars = key.chars();
            return match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(Key::Char(c.to_ascii_lowercase())),
                _ => Err(format!(
                    "“{key}” isn't a key name; use one like Return, Tab, Escape, Left, F5 or a single character."
                )),
            };
        }
    };
    Ok(Key::Named(named))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_read_like_shortcuts() {
        let save = parse("cmd+s").unwrap();
        assert!(save.meta && !save.ctrl && save.key == Key::Char('s'));
        let tab = parse("Ctrl+Shift+T").unwrap();
        assert!(tab.ctrl && tab.shift && tab.key == Key::Char('t'));
        assert_eq!(parse("Return").unwrap().key, Key::Named(Named::Return));
        assert_eq!(parse("esc").unwrap().key, Key::Named(Named::Escape));
        assert_eq!(parse("F5").unwrap().key, Key::Named(Named::F(5)));
        assert_eq!(parse("cmd++").unwrap().key, Key::Char('+'));
        let primary = parse("CmdOrCtrl+A").unwrap();
        assert_eq!(primary.meta, cfg!(target_os = "macos"));
        assert_eq!(primary.ctrl, !cfg!(target_os = "macos"));
        assert!(parse("hyper+x").is_err());
        assert!(parse("").is_err());
        assert!(parse("cmd+banana").is_err());
    }
}

/// A modifier-only chord used for pointer actions. No key is pressed here.
pub fn modifiers(spec: &str) -> Result<Combo, String> {
    if spec.trim().is_empty() {
        return parse("a");
    }
    parse(&format!("{spec}+a"))
}
