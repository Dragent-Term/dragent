use std::ops::Range;

use regex::RegexBuilder;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferSearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferMatch {
    /// Character offsets, matching the editor's cursor/selection coordinate
    /// system. Regex reports bytes, so the engine converts at this boundary.
    pub range: Range<usize>,
}

pub fn find_buffer_matches(
    text: &str,
    query: &str,
    options: BufferSearchOptions,
) -> Result<Vec<BufferMatch>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let pattern = if options.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let regex = RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|error| error.to_string())?;

    let chars = if options.whole_word {
        text.chars().collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    // find_iter yields matches in byte order, so each match's char offset is
    // computed incrementally from the previous match. Rescanning the whole
    // prefix per match (`text[..start].chars().count()`) made opening the
    // search bar quadratic — seconds on large buffers whose seeded query has
    // thousands of occurrences.
    let mut last_byte = 0usize;
    let mut last_char = 0usize;
    Ok(regex
        .find_iter(text)
        .filter_map(|found| {
            if found.start() == found.end() {
                return None;
            }
            last_char += text[last_byte..found.start()].chars().count();
            last_byte = found.start();
            let start = last_char;
            last_char += text[found.start()..found.end()].chars().count();
            last_byte = found.end();
            let end = last_char;
            if options.whole_word && (!word_boundary(&chars, start) || !word_boundary(&chars, end))
            {
                return None;
            }
            Some(BufferMatch { range: start..end })
        })
        .collect())
}

fn word_boundary(chars: &[char], index: usize) -> bool {
    let left = index.checked_sub(1).and_then(|index| chars.get(index));
    let right = chars.get(index);
    left.is_none_or(|ch| !is_word(*ch)) || right.is_none_or(|ch| !is_word(*ch))
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_case_insensitive_matches_as_character_ranges() {
        let matches =
            find_buffer_matches("Écho echo ECHO", "echo", BufferSearchOptions::default()).unwrap();
        assert_eq!(
            matches.iter().map(|m| m.range.clone()).collect::<Vec<_>>(),
            vec![5..9, 10..14]
        );
    }

    #[test]
    fn char_offsets_stay_correct_across_multibyte_gaps_between_matches() {
        // Each 😀 is 4 bytes but 1 char. The incremental byte→char tracking
        // must produce the same offsets as a full prefix count would.
        let text = "😀😀 needle 😀 needle 😀😀 needle";
        let matches = find_buffer_matches(text, "needle", BufferSearchOptions::default()).unwrap();
        assert_eq!(
            matches.iter().map(|m| m.range.clone()).collect::<Vec<_>>(),
            vec![3..9, 12..18, 22..28]
        );

        // Whole-word mode shares the same tracking path.
        let options = BufferSearchOptions {
            whole_word: true,
            ..BufferSearchOptions::default()
        };
        let matches = find_buffer_matches(text, "needle", options).unwrap();
        assert_eq!(
            matches.iter().map(|m| m.range.clone()).collect::<Vec<_>>(),
            vec![3..9, 12..18, 22..28]
        );
    }

    #[test]
    fn supports_case_whole_word_and_regex_modes() {
        let options = BufferSearchOptions {
            case_sensitive: true,
            whole_word: true,
            regex: true,
        };
        let matches = find_buffer_matches("foo foo_bar foo42 foo.", r"foo", options).unwrap();
        assert_eq!(
            matches.iter().map(|m| m.range.clone()).collect::<Vec<_>>(),
            vec![0..3, 18..21]
        );
    }

    #[test]
    fn reports_invalid_regex_without_panicking() {
        let error = find_buffer_matches(
            "anything",
            "(",
            BufferSearchOptions {
                regex: true,
                ..BufferSearchOptions::default()
            },
        )
        .unwrap_err();
        assert!(!error.is_empty());
    }
}
