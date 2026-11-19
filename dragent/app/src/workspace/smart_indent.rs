//! Smart, language-aware indentation for Enter and closing delimiters.
//!
//! Architecture (per the smart-indent spec): a lightweight lexical scanner.
//! `dragent_editor` does have real tree-sitter grammars (via `arborium`,
//! see `folding.rs`), but its parses run asynchronously and can briefly lag
//! the buffer right after an edit — fine for folding, wrong for a per-
//! keystroke, latency-critical path like "what should Enter do right now".
//! So indentation stays on this disciplined scanner instead: it classifies
//! every character as code / string / comment, so delimiters inside
//! literals and comments never affect structure ("syntax wins", not raw
//! characters). Indentation for a new line comes from the unclosed
//! delimiter stack at the reference line — which naturally handles blank
//! lines, nesting depth, JSX tags (TS/JS) and Python colon blocks — with the
//! previous line's literal indentation preserved so existing file style is
//! respected. Closing delimiters dedent their blank line to the opener's
//! indentation.
//!
//! (Comment-toggle used to live here too, reusing this same scanner for its
//! JSX-awareness — that's not a per-keystroke path, so it was moved to
//! `dragent_editor::comment_style_at`, which asks the real parsed tree
//! instead. See that function's doc comment for why.)
//!
//! All functions are pure over `&[char]` so they are unit-testable headlessly.

use dragent_editor::Language;

/// One indentation level from settings (Editor → Tab size): spaces, or a tab
/// character when Insert Spaces is off. Read live on every call so a
/// settings change applies to all open files immediately — never cached.
pub fn indent_unit() -> String {
    let prefs = crate::console::editor_settings::EditorSettings::global();
    indent_unit_for(prefs.editor.clamped_tab_size(), prefs.editor.insert_spaces)
}

/// Pure constructor for one indent level (testable without globals).
pub fn indent_unit_for(tab_size: usize, insert_spaces: bool) -> String {
    if insert_spaces {
        " ".repeat(tab_size.clamp(1, 8))
    } else {
        "\t".to_string()
    }
}

/// What pressing Enter should insert after the newline.
pub struct NewlineIndent {
    /// Indentation for the new line — the cursor lands right after it.
    pub indent: String,
    /// When the cursor sits directly between a delimiter pair, the closer is
    /// moved to its own line at the opener's indentation (block
    /// expansion). This holds the trailing newline-plus-indent fragment.
    pub closer_line: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Code,
    LineComment,
    BlockComment,
    StringLit(char),
}

fn is_block_language(language: Language) -> bool {
    !matches!(language, Language::Markdown | Language::PlainText)
}

fn hash_line_comments(language: Language) -> bool {
    matches!(
        language,
        Language::Python | Language::Shell | Language::Yaml
    )
}

/// Languages whose markup uses JSX-style `<tag> … </tag>` nesting.
fn jsx_tags(language: Language) -> bool {
    matches!(
        language,
        Language::TypeScript | Language::JavaScript | Language::Html
    )
}

fn colon_blocks(language: Language) -> bool {
    language == Language::Python
}

fn matching_closer(open: char) -> Option<char> {
    match open {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

fn line_start(chars: &[char], index: usize) -> usize {
    chars[..index.min(chars.len())]
        .iter()
        .rposition(|&ch| ch == '\n')
        .map_or(0, |position| position + 1)
}

fn line_end(chars: &[char], index: usize) -> usize {
    let index = index.min(chars.len());
    chars[index..]
        .iter()
        .position(|&ch| ch == '\n')
        .map_or(chars.len(), |position| index + position)
}

fn line_is_blank(chars: &[char], start: usize, end: usize) -> bool {
    chars[start..end.min(chars.len())]
        .iter()
        .all(|ch| ch.is_whitespace())
}

/// Scan `chars[..end]` and return the stack of unclosed opening delimiters
/// in code context as `(opener, index)`. `'<'` entries are JSX opening tags.
/// Also returns the last non-whitespace CODE character at or before `end`.
fn scan(chars: &[char], end: usize, language: Language) -> (Vec<(char, usize)>, Option<char>) {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut context = Context::Code;
    let mut last_code_char: Option<char> = None;
    let mut index = 0usize;
    let end = end.min(chars.len());

    while index < end {
        let ch = chars[index];
        match context {
            Context::Code => match ch {
                '#' if hash_line_comments(language) => context = Context::LineComment,
                '/' if chars.get(index + 1) == Some(&'/') => {
                    context = Context::LineComment;
                    index += 1;
                }
                '/' if chars.get(index + 1) == Some(&'*') => {
                    context = Context::BlockComment;
                    index += 1;
                }
                '"' | '\'' | '`' => context = Context::StringLit(ch),
                '{' | '[' | '(' => {
                    stack.push((ch, index));
                    last_code_char = Some(ch);
                }
                '}' | ']' | ')' => {
                    stack.pop();
                    last_code_char = Some(ch);
                }
                '<' if jsx_tags(language) => {
                    index = scan_jsx_tag(chars, index, &mut stack);
                    last_code_char = Some('>');
                }
                '\\' => {
                    index += 1; // skip escaped character
                }
                ch if !ch.is_whitespace() => last_code_char = Some(ch),
                _ => {}
            },
            Context::LineComment => {
                if ch == '\n' {
                    context = Context::Code;
                }
            }
            Context::BlockComment => {
                if ch == '*' && chars.get(index + 1) == Some(&'/') {
                    context = Context::Code;
                    index += 1;
                }
            }
            Context::StringLit(quote) => {
                if ch == '\\' {
                    index += 1;
                } else if ch == quote {
                    context = Context::Code;
                }
            }
        }
        index += 1;
    }
    (stack, last_code_char)
}

/// Handle `<` in code context for JSX languages. Pushes a `'<'` stack entry
/// for an opening tag, pops one for a closing tag, and returns the index of
/// the closing `>` (or `index` unchanged when this is not a tag — e.g.
/// generics like `Array<T>`, comparisons).
fn scan_jsx_tag(chars: &[char], open_index: usize, stack: &mut Vec<(char, usize)>) -> usize {
    let end = line_end(chars, open_index);
    let next = chars.get(open_index + 1).copied();

    // A `<` directly after an identifier or `>` is a generic parameter
    // (`Array<T>`, `Map<K, V>`), not JSX.
    let prev = open_index
        .checked_sub(1)
        .and_then(|index| chars.get(index))
        .copied();
    if prev.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || ch == '>') {
        return open_index;
    }

    match next {
        Some('/') => {
            // Closing tag `</name>` (also `</>`): pop the innermost JSX tag.
            let Some(gt) = (open_index..end).find(|&index| chars[index] == '>') else {
                return open_index;
            };
            while matches!(stack.last(), Some(('<', _))) {
                stack.pop();
            }
            gt
        }
        Some(ch) if ch.is_alphanumeric() || ch == '_' || ch == '>' => {
            // Opening tag `<name …>` — must close on the same line to count.
            let Some(gt_rel) = chars[open_index..end].iter().position(|&ch| ch == '>') else {
                return open_index;
            };
            let gt = open_index + gt_rel;
            let self_closing = gt > 0 && chars[gt - 1] == '/';
            if !self_closing {
                stack.push(('<', open_index));
            }
            gt
        }
        _ => open_index,
    }
}

/// Indentation (whitespace prefix) of the line containing `index`.
fn line_indent(chars: &[char], index: usize) -> String {
    let start = line_start(chars, index);
    chars[start..index.min(chars.len())]
        .iter()
        .take_while(|ch| ch.is_whitespace())
        .collect()
}

/// Enter pressed with the cursor directly between a delimiter pair
/// (`{|}`, `(|)`, `[|]`): move the closer to its own line at the opener's
/// indentation and indent the inner line one level.
fn between_delimiters(
    chars: &[char],
    cursor: usize,
    language: Language,
    unit: &str,
) -> Option<NewlineIndent> {
    if cursor == 0 || cursor >= chars.len() {
        return None;
    }
    let open = chars[cursor - 1];
    let Some(close) = matching_closer(open) else {
        return None;
    };
    if chars[cursor] != close {
        return None;
    }
    // The opener must be in code context (top of the unclosed stack).
    let (stack, _) = scan(chars, cursor, language);
    if !stack.last().is_some_and(|(ch, _)| *ch == open) {
        return None;
    }
    let opener_line_indent = line_indent(chars, cursor - 1);
    Some(NewlineIndent {
        indent: format!("{opener_line_indent}{unit}"),
        closer_line: Some(format!("\n{opener_line_indent}")),
    })
}

/// Compute the indentation for a new line at `cursor`, using the live
/// settings indent width.
pub fn newline_indent(chars: &[char], cursor: usize, language: Language) -> NewlineIndent {
    newline_indent_with(chars, cursor, language, &indent_unit())
}

/// Pure variant with an explicit indent unit (hermetic tests, previews).
pub fn newline_indent_with(
    chars: &[char],
    cursor: usize,
    language: Language,
    unit: &str,
) -> NewlineIndent {
    let cursor = cursor.min(chars.len());
    if !is_block_language(language) {
        return NewlineIndent {
            indent: line_indent(chars, cursor),
            closer_line: None,
        };
    }
    if let Some(expansion) = between_delimiters(chars, cursor, language, unit) {
        return expansion;
    }

    // Reference line: the cursor's own line when it already has code before
    // the cursor (line split), otherwise the nearest previous non-blank line
    // — blank lines inside a block keep the structural indentation.
    let own_line_start = line_start(chars, cursor);
    let own_line_end = line_end(chars, cursor);
    let own_has_code = chars[own_line_start..cursor]
        .iter()
        .any(|ch| !ch.is_whitespace());

    let (ref_line_start, ref_line_end) = if own_has_code {
        (own_line_start, own_line_end)
    } else {
        let mut start = own_line_start;
        while start > 0 {
            let previous_start = line_start(chars, start - 1);
            if !line_is_blank(chars, previous_start, start - 1) {
                start = previous_start;
                break;
            }
            start = previous_start;
        }
        (start, line_end(chars, start))
    };

    // One scan to the end of the reference line, snapshotting the depth where
    // that line starts so the line's own delta can be applied to its literal
    // indentation (respects the file's existing indent style).
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut context = Context::Code;
    let mut last_code_char: Option<char> = None;
    let mut depth_at_ref_start = 0usize;
    let mut index = 0usize;
    let scan_end = ref_line_end.min(cursor.max(ref_line_end));
    while index < scan_end {
        if index == ref_line_start {
            depth_at_ref_start = stack.len();
        }
        let ch = chars[index];
        match context {
            Context::Code => match ch {
                '#' if hash_line_comments(language) => context = Context::LineComment,
                '/' if chars.get(index + 1) == Some(&'/') => {
                    context = Context::LineComment;
                    index += 1;
                }
                '/' if chars.get(index + 1) == Some(&'*') => {
                    context = Context::BlockComment;
                    index += 1;
                }
                '"' | '\'' | '`' => context = Context::StringLit(ch),
                '{' | '[' | '(' => {
                    stack.push((ch, index));
                    last_code_char = Some(ch);
                }
                '}' | ']' | ')' => {
                    stack.pop();
                    last_code_char = Some(ch);
                }
                '<' if jsx_tags(language) => {
                    index = scan_jsx_tag(chars, index, &mut stack);
                    last_code_char = Some('>');
                }
                '\\' => {
                    index += 1;
                }
                ch if !ch.is_whitespace() => last_code_char = Some(ch),
                _ => {}
            },
            Context::LineComment => {
                if ch == '\n' {
                    context = Context::Code;
                }
            }
            Context::BlockComment => {
                if ch == '*' && chars.get(index + 1) == Some(&'/') {
                    context = Context::Code;
                    index += 1;
                }
            }
            Context::StringLit(quote) => {
                if ch == '\\' {
                    index += 1;
                } else if ch == quote {
                    context = Context::Code;
                }
            }
        }
        index += 1;
    }

    let mut level = stack.len();
    // Python: a trailing colon opens a block (dict/slice colons are rare and
    // the same limitation a lexical fallback has).
    if colon_blocks(language) && last_code_char == Some(':') {
        level += 1;
    }

    let base = chars[ref_line_start..ref_line_end.min(cursor.max(ref_line_start))]
        .iter()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    let delta = level as isize - depth_at_ref_start as isize;
    let indent = if delta > 0 {
        format!("{base}{}", unit.repeat(delta as usize))
    } else if delta < 0 {
        // Drop trailing indent units; keep whatever literal prefix remains.
        let mut indent = base.clone();
        for _ in 0..(-delta) {
            if !unit.is_empty() && indent.ends_with(&unit) {
                indent.truncate(indent.len() - unit.len());
            } else if indent.ends_with('\t') {
                indent.truncate(indent.len() - 1);
            } else {
                break;
            }
        }
        indent
    } else {
        base
    };

    NewlineIndent {
        indent,
        closer_line: None,
    }
}

/// Indentation the line should have when the user types `closer`
/// (`}`, `)`, `]`) on a blank line: the indentation of the line containing
/// the matching opener. Returns `None` when there is no open delimiter.
pub fn closer_line_indent(
    chars: &[char],
    cursor: usize,
    closer: char,
    language: Language,
) -> Option<String> {
    let open = match closer {
        '}' => '{',
        ')' => '(',
        ']' => '[',
        _ => return None,
    };
    let (stack, _) = scan(chars, cursor.min(chars.len()), language);
    let &(_, opener_index) = stack.iter().rev().find(|(ch, _)| *ch == open)?;
    let start = line_start(chars, opener_index);
    Some(
        chars[start..opener_index]
            .iter()
            .take_while(|ch| ch.is_whitespace())
            .collect(),
    )
}

/// Indentation for a JSX closing tag (`</div>`) typed on a blank line:
/// the indentation of the line holding the matching opening tag.
pub fn jsx_closing_tag_indent(chars: &[char], cursor: usize) -> Option<String> {
    let cursor = cursor.min(chars.len());
    let row_start = line_start(chars, cursor);
    let prefix: Vec<char> = chars[row_start..cursor].to_vec();
    let content_start = prefix.iter().take_while(|ch| ch.is_whitespace()).count();
    let tag_name: String = prefix[content_start..]
        .iter()
        .skip(2) // `</`
        .take_while(|ch| ch.is_alphanumeric() || **ch == '_' || **ch == '-' || **ch == '.')
        .collect();
    if prefix.get(content_start) != Some(&'<')
        || prefix.get(content_start + 1) != Some(&'/')
        || tag_name.is_empty()
    {
        return None;
    }

    // Walk upward counting same-tag nesting until the matching opener.
    let mut needed = 1usize;
    let mut boundary = row_start;
    while boundary > 0 {
        let start = line_start(chars, boundary - 1);
        let content: String = chars[start..boundary - 1].iter().collect();
        let trimmed = content.trim();

        let opens = trimmed.starts_with(&format!("<{tag_name}"))
            && trimmed[tag_name.len() + 1..]
                .chars()
                .next()
                .is_some_and(|ch| matches!(ch, ' ' | '>' | '/'))
            && !trimmed.starts_with("</");
        let closes = trimmed.starts_with(&format!("</{tag_name}"));
        if opens {
            needed -= 1;
            if needed == 0 {
                let indent: String = content
                    .chars()
                    .take_while(|ch| ch.is_whitespace())
                    .collect();
                return Some(indent);
            }
        } else if closes {
            needed += 1;
        }
        boundary = start;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: Language = Language::TypeScript;
    const RUST: Language = Language::Rust;
    const PY: Language = Language::Python;

    /// Hermetic indent width: tests must not depend on the machine's
    /// personal settings (the global `indent_unit()` reads the live
    /// user config, which is 2 spaces on some machines and 4 on others).
    const UNIT: &str = "    ";

    fn indent_after(text: &str, language: Language) -> String {
        let chars: Vec<char> = text.chars().collect();
        newline_indent_with(&chars, chars.len(), language, UNIT).indent
    }

    fn vc(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn enter(text: &str, language: Language) -> String {
        // Simulate Enter at the end of `text` and return the resulting buffer.
        let chars = vc(text);
        let result = newline_indent_with(&chars, chars.len(), language, UNIT);
        let closer = result.closer_line.unwrap_or_default();
        format!("{text}\n{}{closer}", result.indent)
    }

    #[test]
    fn same_level_indent_is_preserved() {
        let text = "function run() {\n    const first = 1;";
        assert_eq!(indent_after(text, TS), "    ");
    }

    #[test]
    fn opening_brace_increases_indentation() {
        assert_eq!(indent_after("if (ready) {", TS), "    ");
        assert_eq!(indent_after("fn main() {", RUST), "    ");
    }

    #[test]
    fn nested_blocks_accumulate() {
        let text = "function run() {\n    if (ready) {";
        assert_eq!(indent_after(text, TS), "        ");
    }

    #[test]
    fn brace_in_string_does_not_increase() {
        let text = "const value = \"{\";";
        assert_eq!(indent_after(text, TS), "");
    }

    #[test]
    fn brace_in_comment_does_not_increase() {
        let text = "// start block {";
        assert_eq!(indent_after(text, TS), "");
    }

    #[test]
    fn brace_in_template_literal_does_not_increase() {
        let text = "const message = `\nhello {";
        assert_eq!(indent_after(text, TS), "");
    }

    #[test]
    fn blank_lines_keep_block_indentation() {
        let text = "function run() {\n    const x = 1;\n\n";
        assert_eq!(indent_after(text, TS), "    ");
    }

    #[test]
    fn between_braces_expands_block() {
        let result = newline_indent_with(&vc("function run() {}"), 16, TS, UNIT);
        assert_eq!(result.indent, "    ");
        assert_eq!(result.closer_line.as_deref(), Some("\n"));
    }

    #[test]
    fn between_brackets_expands() {
        let result = newline_indent_with(&vc("const users = []"), 15, TS, UNIT);
        assert_eq!(result.indent, "    ");
        assert_eq!(result.closer_line.as_deref(), Some("\n"));
    }

    #[test]
    fn rust_match_and_impl_use_brace_rules() {
        assert_eq!(indent_after("match result {", RUST), "    ");
        assert_eq!(indent_after("impl Editor {", RUST), "    ");
    }

    #[test]
    fn python_colon_increases_indentation() {
        assert_eq!(indent_after("if ready:", PY), "    ");
        assert_eq!(indent_after("def calculate_total():", PY), "    ");
    }

    #[test]
    fn python_dict_opener_still_indents_as_continuation() {
        // A `{` opens a dict — the continuation line indents one level even
        // though Python has no brace-block rule.
        assert_eq!(indent_after("data = {", PY), "    ");
    }

    #[test]
    fn jsx_tag_increases_indentation() {
        assert_eq!(indent_after("return (", TS), "    ");
        assert_eq!(indent_after("<div>", TS), "    ");
        assert_eq!(indent_after("const x = <Dialog>", TS), "    ");
    }

    #[test]
    fn jsx_closing_tag_does_not_increase() {
        let text = "<div>\n    content\n    </div>";
        assert_eq!(indent_after(text, TS), "");
    }

    #[test]
    fn generics_do_not_count_as_jsx() {
        let text = "const items: Array<string> = [];";
        assert_eq!(indent_after(text, TS), "");
    }

    #[test]
    fn arrow_functions_do_not_count_as_jsx() {
        assert_eq!(indent_after("const f = () =>", TS), "");
    }

    #[test]
    fn closer_line_indent_matches_opener_line() {
        let text = "function run() {\n    if (ready) {\n        performWork();\n";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(
            closer_line_indent(&chars, chars.len(), '}', TS),
            Some("    ".to_string())
        );
    }

    #[test]
    fn closer_line_indent_none_at_top_level() {
        let chars = vc("fn main() {}");
        assert_eq!(closer_line_indent(&chars, chars.len(), '}', RUST), None);
    }

    #[test]
    fn jsx_closing_tag_indent_matches_opener() {
        let text = "<div>\n    <section>\n        content\n        </section";
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(
            jsx_closing_tag_indent(&chars, chars.len()),
            Some("    ".to_string())
        );
    }

    #[test]
    fn full_enter_between_braces_produces_block() {
        let chars = vc("function run() {}");
        let result = newline_indent_with(&chars, 16, TS, UNIT);
        assert_eq!(result.indent, "    ");
        assert_eq!(result.closer_line.as_deref(), Some("\n"));
    }

    #[test]
    fn markdown_stays_plain() {
        let chars = vc("# Heading {");
        let result = newline_indent_with(&chars, chars.len(), Language::Markdown, UNIT);
        assert_eq!(result.indent, "");
        assert!(result.closer_line.is_none());
    }

    #[test]
    fn indent_width_flows_from_settings() {
        // The Editor → Tab size setting reaches every construct: 2-space,
        // 4-space, and tab units all shape the same code identically.
        let chars = vc("function run() {}");
        let two = newline_indent_with(&chars, 16, TS, "  ");
        assert_eq!(two.indent, "  ");
        assert_eq!(two.closer_line.as_deref(), Some("\n"));
        let tab = newline_indent_with(&chars, 16, TS, "\t");
        assert_eq!(tab.indent, "\t");

        // The reference line's literal base ("    ") is preserved; the
        // configured unit appends exactly one level on top of it.
        let nested = vc("if (ready) {\n    if (set) {");
        let deep_two = newline_indent_with(&nested, nested.len(), TS, "  ");
        assert_eq!(deep_two.indent, "      ");
        let deep_tab = newline_indent_with(&nested, nested.len(), TS, "\t");
        assert_eq!(deep_tab.indent, "    \t");
    }

    #[test]
    fn indent_unit_for_builds_widths() {
        assert_eq!(indent_unit_for(4, true), "    ");
        assert_eq!(indent_unit_for(2, true), "  ");
        assert_eq!(indent_unit_for(4, false), "\t");
        // Clamped, never empty.
        assert_eq!(indent_unit_for(0, true), " ");
        assert_eq!(indent_unit_for(99, true), "        ");
    }
}
