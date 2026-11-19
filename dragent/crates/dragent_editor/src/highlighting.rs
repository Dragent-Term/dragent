use crate::Language;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Comment,
    Keyword,
    Function,
    Type,
    String,
    Number,
    Constant,
    Operator,
    Property,
    Macro,
    Delimiter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HighlightState {
    block_comment: Option<BlockComment>,
    string_quote: Option<StringQuote>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockComment {
    SlashStar,
    Html,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StringQuote {
    Single,
    Double,
    Backtick,
}

impl StringQuote {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            b'\'' => Some(Self::Single),
            b'"' => Some(Self::Double),
            b'`' => Some(Self::Backtick),
            _ => None,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Single => b'\'',
            Self::Double => b'"',
            Self::Backtick => b'`',
        }
    }
}

pub struct LexicalHighlighter;

impl LexicalHighlighter {
    pub fn highlight_line(language: Language, line: &str) -> Vec<HighlightSpan> {
        Self::highlight_line_with_state(language, line, &mut HighlightState::default())
    }

    /// Highlight one physical source line while preserving block-comment and
    /// quoted-string state from preceding lines. Callers rendering a document
    /// should retain this state as they advance through its lines.
    pub fn highlight_line_with_state(
        language: Language,
        line: &str,
        state: &mut HighlightState,
    ) -> Vec<HighlightSpan> {
        if language == Language::PlainText {
            return Vec::new();
        }
        let protected_ranges = protected_ranges(language, line, state);
        if language == Language::Markdown && protected_ranges.is_empty() {
            return highlight_markdown(line);
        }

        let mut spans = Vec::new();
        let mut code_start = 0;
        for (range, kind) in protected_ranges {
            scan_code_with_offset(
                language,
                &line[code_start..range.start],
                code_start,
                &mut spans,
            );
            spans.push(HighlightSpan {
                range: range.clone(),
                kind,
            });
            code_start = range.end;
        }
        scan_code_with_offset(language, &line[code_start..], code_start, &mut spans);
        spans.sort_by_key(|span| span.range.start);
        spans
    }

    /// Lexical state at the beginning of each physical line. This keeps
    /// viewport and folded rendering deterministic without storing display
    /// delimiters in the source buffer or guessing from the visible line.
    pub fn line_start_states(language: Language, source: &str) -> Vec<HighlightState> {
        let mut state = HighlightState::default();
        // `lines().count()` made this helper traverse the entire document once
        // merely to reserve, then a second time to compute states. `split`
        // also preserves the final empty physical line after a trailing `\n`,
        // keeping the cache aligned with the editor's visual-line map.
        let mut states = Vec::new();
        for line in source.split('\n') {
            states.push(state);
            let _ = Self::highlight_line_with_state(language, line, &mut state);
        }
        states
    }
}

fn scan_code_with_offset(
    language: Language,
    code: &str,
    offset: usize,
    spans: &mut Vec<HighlightSpan>,
) {
    let start = spans.len();
    scan_code(language, code, spans);
    for span in &mut spans[start..] {
        span.range.start += offset;
        span.range.end += offset;
    }
}

fn scan_code(language: Language, code: &str, spans: &mut Vec<HighlightSpan>) {
    let bytes = code.as_bytes();
    let mut index = 0;
    let mut expect_function = false;
    let mut expect_type = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'\'' | b'"' | b'`') {
            let end = quoted_end(bytes, index, byte);
            spans.push(HighlightSpan {
                range: index..end,
                kind: TokenKind::String,
            });
            index = end;
            continue;
        }
        if byte.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'.' | b'_'))
            {
                index += 1;
            }
            spans.push(HighlightSpan {
                range: start..index,
                kind: TokenKind::Number,
            });
            continue;
        }
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let word = &code[start..index];
            let kind = if next_non_space(bytes, index) == Some(b'!') {
                Some(TokenKind::Macro)
            } else if expect_function {
                expect_function = false;
                Some(TokenKind::Function)
            } else if expect_type {
                expect_type = false;
                Some(TokenKind::Type)
            } else if function_keywords(language).contains(&word) {
                expect_function = true;
                Some(TokenKind::Keyword)
            } else if type_keywords(language).contains(&word) {
                expect_type = true;
                Some(TokenKind::Keyword)
            } else if keywords(language).contains(&word) {
                Some(TokenKind::Keyword)
            } else if constants(language).contains(&word) {
                Some(TokenKind::Constant)
            } else if next_non_space(bytes, index) == Some(b'(') {
                Some(TokenKind::Function)
            } else {
                None
            };
            if let Some(kind) = kind {
                spans.push(HighlightSpan {
                    range: start..index,
                    kind,
                });
            }
            continue;
        }
        if matches!(
            byte,
            b'+' | b'-' | b'*' | b'/' | b'=' | b'!' | b'<' | b'>' | b'&' | b'|' | b'?' | b':'
        ) {
            spans.push(HighlightSpan {
                range: index..index + 1,
                kind: TokenKind::Operator,
            });
        } else if matches!(byte, b'{' | b'}' | b'[' | b']') {
            spans.push(HighlightSpan {
                range: index..index + 1,
                kind: TokenKind::Delimiter,
            });
        }
        index += 1;
    }
}

fn protected_ranges(
    language: Language,
    line: &str,
    state: &mut HighlightState,
) -> Vec<(Range<usize>, TokenKind)> {
    let (line_markers, block_marker): (&[&str], Option<(&str, &str, BlockComment)>) = match language
    {
        Language::Python | Language::Shell | Language::Yaml | Language::Toml => (&["#"], None),
        Language::Html | Language::Markdown => (&[], Some(("<!--", "-->", BlockComment::Html))),
        Language::Css => (&[], Some(("/*", "*/", BlockComment::SlashStar))),
        Language::Hcl => (&["#", "//"], Some(("/*", "*/", BlockComment::SlashStar))),
        Language::Rust
        | Language::TypeScript
        | Language::JavaScript
        | Language::Go
        | Language::Json => (&["//"], Some(("/*", "*/", BlockComment::SlashStar))),
        Language::PlainText => (&[], None),
    };
    let mut ranges = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if let Some(block) = state.block_comment {
            let end_marker = match block {
                BlockComment::SlashStar => "*/",
                BlockComment::Html => "-->",
            };
            let end = line[index..]
                .find(end_marker)
                .map(|end| index + end + end_marker.len())
                .unwrap_or(line.len());
            ranges.push((index..end, TokenKind::Comment));
            index = end;
            if index < line.len() || line.ends_with(end_marker) {
                state.block_comment = None;
            }
            continue;
        }

        if let Some(quote) = state.string_quote {
            let closing_end = closing_quote_end(bytes, index, quote.byte());
            let end = closing_end.unwrap_or(line.len());
            ranges.push((index..end, TokenKind::String));
            index = end;
            if closing_end.is_some() {
                state.string_quote = None;
            }
            continue;
        }

        if let Some(marker) = line_markers
            .iter()
            .find(|marker| bytes[index..].starts_with(marker.as_bytes()))
        {
            let _ = marker;
            ranges.push((index..line.len(), TokenKind::Comment));
            break;
        }
        if let Some((start_marker, end_marker, block)) = block_marker {
            if bytes[index..].starts_with(start_marker.as_bytes()) {
                let end = line[index + start_marker.len()..]
                    .find(end_marker)
                    .map(|end| index + start_marker.len() + end + end_marker.len())
                    .unwrap_or(line.len());
                ranges.push((index..end, TokenKind::Comment));
                index = end;
                if index == line.len() && !line.ends_with(end_marker) {
                    state.block_comment = Some(block);
                }
                continue;
            }
        }

        if let Some(quote) = StringQuote::from_byte(bytes[index]) {
            let closing_end = closing_quote_end(bytes, index + 1, quote.byte());
            let end = closing_end.unwrap_or(line.len());
            ranges.push((index..end, TokenKind::String));
            index = end;
            if closing_end.is_none() {
                state.string_quote = Some(quote);
            }
            continue;
        }
        index += 1;
    }
    ranges
}

fn closing_quote_end(bytes: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut index = start;
    while index < bytes.len() {
        if bytes[index] == quote && !is_escaped(bytes, index) {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut slash_count = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        slash_count += 1;
        cursor -= 1;
    }
    slash_count % 2 == 1
}

fn highlight_markdown(line: &str) -> Vec<HighlightSpan> {
    let trimmed = line.trim_start();
    let offset = line.len() - trimmed.len();
    if trimmed.starts_with('#') {
        return vec![HighlightSpan {
            range: offset..line.len(),
            kind: TokenKind::Keyword,
        }];
    }
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(start) = line[search_from..].find('`') {
        let start = search_from + start;
        if let Some(end) = line[start + 1..].find('`') {
            let end = start + 2 + end;
            spans.push(HighlightSpan {
                range: start..end,
                kind: TokenKind::String,
            });
            search_from = end;
        } else {
            break;
        }
    }
    spans
}

fn quoted_end(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == quote && bytes[index - 1] != b'\\' {
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn next_non_space(bytes: &[u8], mut index: usize) -> Option<u8> {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    bytes.get(index).copied()
}

fn function_keywords(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["fn"],
        Language::Python => &["def"],
        Language::Go => &["func"],
        Language::TypeScript | Language::JavaScript => &["function"],
        _ => &[],
    }
}

fn type_keywords(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["struct", "enum", "trait", "type"],
        Language::TypeScript | Language::JavaScript => &["class", "interface", "type", "enum"],
        Language::Python => &["class"],
        Language::Go => &["type", "struct", "interface"],
        _ => &[],
    }
}

fn keywords(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &[
            "as", "async", "await", "break", "const", "continue", "crate", "else", "extern", "for",
            "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
            "return", "self", "Self", "static", "super", "unsafe", "use", "where", "while",
        ],
        Language::TypeScript | Language::JavaScript => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "finally",
            "for",
            "from",
            "if",
            "import",
            "in",
            "instanceof",
            "let",
            "new",
            "of",
            "return",
            "static",
            "switch",
            "throw",
            "try",
            "typeof",
            "var",
            "while",
            "yield",
        ],
        Language::Python => &[
            "and", "as", "assert", "async", "await", "break", "continue", "del", "elif", "else",
            "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
            "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
        ],
        Language::Go => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "go",
            "goto",
            "if",
            "import",
            "map",
            "package",
            "range",
            "return",
            "select",
            "switch",
            "var",
        ],
        Language::Json => &[],
        Language::Html => &[],
        Language::Css => &["important"],
        Language::Shell => &[
            "case", "do", "done", "elif", "else", "esac", "fi", "for", "function", "if", "in",
            "then", "until", "while",
        ],
        Language::Hcl => &[
            "data",
            "dynamic",
            "locals",
            "module",
            "output",
            "provider",
            "resource",
            "terraform",
            "variable",
        ],
        Language::Yaml | Language::Toml | Language::Markdown | Language::PlainText => &[],
    }
}

fn constants(language: Language) -> &'static [&'static str] {
    match language {
        Language::Python => &["False", "None", "True"],
        Language::Rust => &["false", "true"],
        Language::Go => &["false", "iota", "nil", "true"],
        Language::TypeScript | Language::JavaScript | Language::Json => {
            &["false", "null", "true", "undefined"]
        }
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_function_string_and_comment() {
        let spans = LexicalHighlighter::highlight_line(
            Language::Rust,
            "pub fn greet() { println!(\"hi\"); } // note",
        );
        assert!(spans.iter().any(|span| span.kind == TokenKind::Function));
        assert!(spans.iter().any(|span| span.kind == TokenKind::String));
        assert!(spans.iter().any(|span| span.kind == TokenKind::Comment));
    }

    #[test]
    fn ignores_comment_markers_inside_strings() {
        let spans = LexicalHighlighter::highlight_line(
            Language::TypeScript,
            "const url = \"https://example.com\";",
        );
        assert!(!spans.iter().any(|span| span.kind == TokenKind::Comment));
    }

    #[test]
    fn keeps_typescript_and_jsx_block_comments_one_color_across_lines() {
        for language in [Language::TypeScript, Language::JavaScript] {
            let mut state = HighlightState::default();
            let first = LexicalHighlighter::highlight_line_with_state(
                language,
                "{/* React block comment",
                &mut state,
            );
            let middle = LexicalHighlighter::highlight_line_with_state(
                language,
                "const fake = 42;",
                &mut state,
            );
            let last = LexicalHighlighter::highlight_line_with_state(
                language,
                "ends here */}",
                &mut state,
            );

            assert!(first.iter().any(|span| span.kind == TokenKind::Comment));
            assert_eq!(
                middle,
                vec![HighlightSpan {
                    range: 0.."const fake = 42;".len(),
                    kind: TokenKind::Comment,
                }]
            );
            assert_eq!(last[0].kind, TokenKind::Comment);
            assert_eq!(last[0].range, 0.."ends here */".len());
            assert_eq!(state, HighlightState::default());
        }
    }

    #[test]
    fn recognizes_each_supported_comment_family() {
        let cases = [
            (Language::Rust, "// rust"),
            (Language::TypeScript, "// typescript"),
            (Language::JavaScript, "// javascript"),
            (Language::Python, "# python"),
            (Language::Go, "// go"),
            (Language::Json, "// jsonc"),
            (Language::Html, "<!-- html -->"),
            (Language::Css, "/* css */"),
            (Language::Shell, "# shell"),
            (Language::Yaml, "# yaml"),
            (Language::Toml, "# toml"),
            (Language::Hcl, "# terraform"),
        ];
        for (language, source) in cases {
            let spans = LexicalHighlighter::highlight_line(language, source);
            assert!(
                spans.iter().any(|span| span.kind == TokenKind::Comment),
                "missing comment highlighting for {language:?}"
            );
        }
    }

    #[test]
    fn supports_code_after_a_closed_block_comment() {
        let spans = LexicalHighlighter::highlight_line(
            Language::TypeScript,
            "/* explanation */ const ready = true;",
        );
        assert_eq!(spans[0].kind, TokenKind::Comment);
        assert!(spans.iter().any(|span| span.kind == TokenKind::Keyword));
        assert!(spans.iter().any(|span| span.kind == TokenKind::Constant));
    }

    #[test]
    fn preserves_quoted_strings_across_physical_lines() {
        for language in [
            Language::Rust,
            Language::TypeScript,
            Language::JavaScript,
            Language::Python,
            Language::Go,
            Language::Json,
            Language::Html,
            Language::Css,
            Language::Shell,
            Language::Yaml,
            Language::Toml,
            Language::Hcl,
            Language::Markdown,
        ] {
            let mut state = HighlightState::default();
            let first = LexicalHighlighter::highlight_line_with_state(
                language,
                "const description = \"NimbleDoc AI is an evidence-grounded legal workspace",
                &mut state,
            );
            let middle = LexicalHighlighter::highlight_line_with_state(
                language,
                "      products, agents, and // source-level citations",
                &mut state,
            );
            let last = LexicalHighlighter::highlight_line_with_state(
                language,
                "      permissions, provenance, and human review.\"; const ready = true;",
                &mut state,
            );

            assert!(first.iter().any(|span| span.kind == TokenKind::String));
            assert_eq!(
                middle,
                vec![HighlightSpan {
                    range: 0.."      products, agents, and // source-level citations".len(),
                    kind: TokenKind::String,
                }],
                "continuation line was not entirely a string for {language:?}"
            );
            assert_eq!(last[0].kind, TokenKind::String);
            assert_eq!(
                &"      permissions, provenance, and human review.\"; const ready = true;"
                    [last[0].range.clone()],
                "      permissions, provenance, and human review.\""
            );
            assert_eq!(state, HighlightState::default());
        }
    }

    #[test]
    fn escaped_quotes_do_not_end_multiline_string_state() {
        let mut state = HighlightState::default();
        let _ = LexicalHighlighter::highlight_line_with_state(
            Language::JavaScript,
            "const value = \"first line with an escaped \\\" quote",
            &mut state,
        );
        let continuation = LexicalHighlighter::highlight_line_with_state(
            Language::JavaScript,
            "  still part of the same string\";",
            &mut state,
        );

        assert_eq!(continuation[0].kind, TokenKind::String);
        assert_eq!(state, HighlightState::default());
    }

    fn text_of<'a>(line: &'a str, spans: &[HighlightSpan]) -> Vec<(&'a str, TokenKind)> {
        spans
            .iter()
            .map(|span| (&line[span.range.clone()], span.kind))
            .collect()
    }

    #[test]
    fn fallback_never_guesses_types_from_capitalization() {
        // Capitalized words are prose in markup (`<div>Hey bro</div>`) and
        // values in CSS (`Arial`). Only tree-sitter knows what a type is, so
        // the lexical fallback leaves them uncolored in every language.
        for (language, line, word) in [
            (Language::TypeScript, "<div>Hey bro</div>", "Hey"),
            (Language::TypeScript, "    Hey Hello World", "Hello"),
            (Language::Html, "  Hey Hello World", "Hello"),
            (Language::Css, "  font-family: Inter, Arial;", "Arial"),
            (Language::Go, "\tvar x Foo", "Foo"),
        ] {
            let spans = LexicalHighlighter::highlight_line(language, line);
            assert!(
                !text_of(line, &spans).iter().any(|(text, _)| *text == word),
                "{language:?}: {word} colored in {line:?}"
            );
        }
        // Keywords and calls still color.
        let line = "if (ok) { formatName(user) }";
        let kinds = text_of(
            line,
            &LexicalHighlighter::highlight_line(Language::TypeScript, line),
        );
        assert!(kinds
            .iter()
            .any(|(t, k)| *t == "if" && *k == TokenKind::Keyword));
        assert!(kinds
            .iter()
            .any(|(t, k)| *t == "formatName" && *k == TokenKind::Function));
    }

    #[test]
    fn distinguishes_rust_macros_and_scope_delimiters() {
        let spans = LexicalHighlighter::highlight_line(Language::Rust, "vec![value]");
        assert!(spans.iter().any(|span| span.kind == TokenKind::Macro));
        assert_eq!(
            spans
                .iter()
                .filter(|span| span.kind == TokenKind::Delimiter)
                .count(),
            2
        );
    }
}
