//! Text-editing helpers for the command input buffer.

/// Last word from a command string (for Alt+. insert).
pub fn last_word_of(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .split_whitespace()
        .next_back()
        .map(|word| word.trim_start_matches('-').to_string())
}

pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Start of the word left of `cursor` (char index).
pub fn word_start(chars: &[char], cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut i = cursor.saturating_sub(1);
    while i > 0 && chars[i].is_whitespace() {
        i -= 1;
    }
    while i > 0 && is_word_char(chars[i - 1]) {
        i -= 1;
    }
    i
}

pub fn word_end(chars: &[char], cursor: usize) -> usize {
    let len = chars.len();
    let mut i = cursor.min(len);
    if i < len && chars[i].is_whitespace() {
        if i == 0 {
            return 0;
        }
        i -= 1;
        while i > 0 && chars[i].is_whitespace() {
            i -= 1;
        }
        while i < len && is_word_char(chars[i]) {
            i += 1;
        }
        return i;
    }
    while i < len && is_word_char(chars[i]) {
        i += 1;
    }
    i
}

pub fn line_start(chars: &[char], cursor: usize) -> usize {
    let mut i = cursor.min(chars.len());
    while i > 0 && chars[i - 1] != '\n' {
        i -= 1;
    }
    i
}

pub fn line_end(chars: &[char], cursor: usize) -> usize {
    let len = chars.len();
    let mut i = cursor.min(len);
    while i < len && chars[i] != '\n' {
        i += 1;
    }
    i
}

pub fn matching_close_bracket(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_word() {
        assert_eq!(
            last_word_of("cargo build --release").as_deref(),
            Some("release")
        );
    }

    #[test]
    fn word_bounds() {
        let s = "git status";
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(word_start(&chars, 3), 0);
        assert_eq!(word_end(&chars, 3), 3);
        assert_eq!(word_start(&chars, 6), 4);
        assert_eq!(word_end(&chars, 6), 10);
    }
}
