use std::borrow::Cow;

/// Strip common ANSI CSI sequences and control chars for plain-text display (Phase 1).
pub fn strip_for_display<'a>(line: &'a str) -> Cow<'a, str> {
    // Fast path: If there are no ANSI escape chars or hidden control characters,
    // do absolutely zero allocations and return the slice directly.
    if !line.contains('\x1b')
        && !line
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Cow::Borrowed(line);
    }

    // Slow path: Allocated only if modification is strictly necessary
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }

    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_for_display() {
        let clean1 = strip_for_display("hello");
        assert_eq!(clean1, "hello");
        assert!(matches!(clean1, Cow::Borrowed(_)));

        let clean2 = strip_for_display("\x1b[31mred\x1b[0m");
        assert_eq!(clean2, "red");
        assert!(matches!(clean2, Cow::Owned(_)));

        let clean3 = strip_for_display("hello\x1b[1Gworld");
        assert_eq!(clean3, "helloworld");
        assert!(matches!(clean3, Cow::Owned(_)));

        let clean4 = strip_for_display("hello\rworld");
        assert_eq!(clean4, "helloworld"); // control char \r should be stripped
        assert!(matches!(clean4, Cow::Owned(_)));

        let clean5 = strip_for_display("hello\tworld\n");
        assert_eq!(clean5, "hello\tworld\n"); // \t and \n should be preserved
        assert!(matches!(clean5, Cow::Borrowed(_)));
    }
}
