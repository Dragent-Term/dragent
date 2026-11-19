//! Minimal shell path escaping for clipboard drag-drop (Dragent-owned, no warp_util).

use std::borrow::Cow;

use lazy_static::lazy_static;
use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellFamily {
    Posix,
    PowerShell,
}

impl ShellFamily {
    pub fn escape<'s>(&self, input: &'s str) -> Cow<'s, str> {
        if input.is_empty() {
            return "''".into();
        }
        match self {
            Self::Posix => POSIX_SHELL_ESCAPE_PATTERN.replace_all(input, "\\$1"),
            Self::PowerShell => POWERSHELL_SHELL_ESCAPE_PATTERN.replace_all(input, "`$1"),
        }
    }
}

lazy_static! {
    static ref POSIX_SHELL_ESCAPE_PATTERN: Regex =
        Regex::new(r#"([ "\$'\\#=\[\]!><|;{}()\*\?&`~]|\n|\t)"#)
            .expect("POSIX shell escape regex should be valid");
    static ref POWERSHELL_SHELL_ESCAPE_PATTERN: Regex =
        Regex::new(r#"([ "\$'#=\[\]!><|;{}()\*&`@,]|\n|\t)"#)
            .expect("PowerShell shell escape regex should be valid");
}
