//! Terminal colours for agent text. Files, line positions and command output
//! take the theme's ANSI palette, the same colours the terminal itself uses,
//! so a conversation doesn't read as one flat colour.

use std::ops::Range;

use warpui::color::ColorU;
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui_core::elements::{Highlight, HighlightedRange, Text};
use warpui_core::text_layout::TextStyle;

use crate::terminal::render::block_output::{find_terminal_links, TerminalLinkKind, FILE_PATH_REGEX};
use crate::theme::Theme;

/// File paths print in magenta, as ripgrep prints them.
pub fn file_color(theme: &Theme) -> ColorU {
    theme.ansi(5)
}

/// The `:12`, `:12-40` or `:12:3` after a path, green like ripgrep's line numbers.
pub fn position_color(theme: &Theme) -> ColorU {
    theme.ansi(2)
}

/// When all of `text` names a file or folder (`src/a.rs`, `a.rs:12`,
/// `a.rs:12-40`), the byte index where its position suffix starts, or
/// `text.len()` when it has none.
pub fn file_reference(text: &str) -> Option<usize> {
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    let end = position_start(text);
    let path = &text[..end];
    let found = FILE_PATH_REGEX.find(path)?;
    (found.start() == 0 && found.end() == path.len()).then_some(end)
}

/// Where a trailing `:line[-line|:col]` starts; `text.len()` when there is none.
fn position_start(text: &str) -> usize {
    let start = text
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == ':')
        .len();
    let tail = &text[start..];
    match tail.find(':') {
        Some(colon) if tail[colon + 1..].starts_with(|c: char| c.is_ascii_digit()) => {
            start + colon
        }
        _ => text.len(),
    }
}

/// A file reference as text: the path in the file colour, its position after it.
pub fn file_text(text: &str, font: FamilyId, size: f32, theme: &Theme) -> Text {
    let end = file_reference(text).unwrap_or(text.len());
    let mut el = Text::new(String::new(), font, size).with_color(theme.main_text());
    el.add_text_with_highlights(&text[..end], file_color(theme), Properties::default());
    if end < text.len() {
        el.add_text_with_highlights(&text[end..], position_color(theme), Properties::default());
    }
    el
}

/// Text in `color` except for byte ranges painted their own colour. The ranges
/// are sorted and don't overlap.
pub fn painted_text(
    text: String,
    paint: &[(Range<usize>, ColorU, bool)],
    font: FamilyId,
    size: f32,
    color: ColorU,
) -> Text {
    let mut highlights = Vec::with_capacity(paint.len());
    // Highlights index characters, the ranges bytes: walk both together once.
    let mut chars = text.char_indices().enumerate().peekable();
    for (range, color, bold) in paint {
        let mut indices = Vec::new();
        while let Some(&(index, (byte, _))) = chars.peek() {
            if byte >= range.end {
                break;
            }
            if byte >= range.start {
                indices.push(index);
            }
            chars.next();
        }
        if indices.is_empty() {
            continue;
        }
        let mut highlight = Highlight::new()
            .with_foreground_color(*color)
            .with_text_style(TextStyle::new().with_foreground_color(*color));
        if *bold {
            highlight = highlight.with_properties(Properties {
                weight: Weight::Bold,
                ..Default::default()
            });
        }
        highlights.push(HighlightedRange {
            highlight,
            highlight_indices: indices,
        });
    }
    Text::new(text, font, size)
        .with_color(color)
        .with_highlights(highlights)
}

/// Above this much output, the panel shows it in one colour: colouring runs
/// on every frame and a huge log shouldn't slow scrolling.
const MAX_PAINTED_OUTPUT: usize = 128 * 1024;

/// Command output as plain text plus the colours to paint it with. Colours
/// the program wrote itself (SGR escapes) are kept. Text it left uncoloured
/// gets terminal accents: paths, links, errors, warnings, passes and diff lines.
/// Other control sequences are dropped, never executed against the layout.
pub fn command_output(raw: &str, theme: &Theme) -> (String, Vec<(Range<usize>, ColorU, bool)>) {
    let (text, written) = parse_sgr(raw, theme);
    if text.len() > MAX_PAINTED_OUTPUT {
        return (text, Vec::new());
    }
    let mut paint = Vec::new();
    let mut accents = accents(&text, theme).into_iter().peekable();
    for (range, color, bold) in written {
        if let Some(color) = color {
            paint.push((range, color, bold));
            continue;
        }
        // Uncoloured stretch: accents inside it, split at its edges.
        let mut at = range.start;
        while let Some((accent, accent_color)) = accents.peek().cloned() {
            if accent.start >= range.end {
                break;
            }
            if accent.end <= at {
                accents.next();
                continue;
            }
            let start = accent.start.max(at);
            let end = accent.end.min(range.end);
            if start > at && bold {
                paint.push((at..start, theme.main_text(), true));
            }
            paint.push((start..end, accent_color, bold));
            at = end;
            if accent.end <= range.end {
                accents.next();
            } else {
                break;
            }
        }
        if at < range.end && bold {
            paint.push((at..range.end, theme.main_text(), true));
        }
    }
    (text, paint)
}

/// Strips escape sequences, returning the text and each stretch's SGR colour
/// (`None` when the program left it the default colour) and boldness.
#[allow(clippy::type_complexity)]
fn parse_sgr(raw: &str, theme: &Theme) -> (String, Vec<(Range<usize>, Option<ColorU>, bool)>) {
    let mut text = String::with_capacity(raw.len());
    let mut runs: Vec<(Range<usize>, Option<ColorU>, bool)> = Vec::new();
    let (mut color, mut bold) = (None, false);
    let mut run_start = 0;
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if !ch.is_control() || ch == '\n' || ch == '\t' {
                text.push(ch);
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut last = None;
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        last = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if last != Some('m') {
                    continue;
                }
                let (next_color, next_bold) = apply_sgr(&params, color, bold, theme);
                if (next_color, next_bold) != (color, bold) {
                    if text.len() > run_start {
                        runs.push((run_start..text.len(), color, bold));
                    }
                    run_start = text.len();
                    (color, bold) = (next_color, next_bold);
                }
            }
            // OSC, DCS, PM, APC: skip to BEL or ST.
            Some(']' | 'P' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    if text.len() > run_start {
        runs.push((run_start..text.len(), color, bold));
    }
    (text, runs)
}

fn apply_sgr(
    params: &str,
    mut color: Option<ColorU>,
    mut bold: bool,
    theme: &Theme,
) -> (Option<ColorU>, bool) {
    let codes: Vec<u16> = params
        .split([';', ':'])
        .map(|code| code.parse().unwrap_or(0))
        .collect();
    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => (color, bold) = (None, false),
            1 => bold = true,
            22 => bold = false,
            code @ 30..=37 => color = Some(theme.ansi(usize::from(code - 30))),
            code @ 90..=97 => color = Some(theme.ansi(usize::from(code - 90 + 8))),
            39 => color = None,
            38 => match codes.get(i + 1) {
                Some(5) => {
                    color = codes.get(i + 2).map(|n| palette_256(*n as u8, theme));
                    i += 2;
                }
                Some(2) => {
                    if let [r, g, b] = codes.get(i + 2..i + 5).unwrap_or_default() {
                        color = Some(ColorU::new(*r as u8, *g as u8, *b as u8, 255));
                    }
                    i += 4;
                }
                _ => {}
            },
            // Background colours are left out: the panel keeps its own.
            48 => {
                i += match codes.get(i + 1) {
                    Some(5) => 2,
                    Some(2) => 4,
                    _ => 0,
                }
            }
            _ => {}
        }
        i += 1;
    }
    (color, bold)
}

fn palette_256(n: u8, theme: &Theme) -> ColorU {
    match n {
        0..=15 => theme.ansi(usize::from(n)),
        16..=231 => {
            let n = n - 16;
            let level = |c: u8| if c == 0 { 0 } else { 55 + c * 40 };
            ColorU::new(level(n / 36), level((n / 6) % 6), level(n % 6), 255)
        }
        _ => {
            let v = 8 + (n - 232) * 10;
            ColorU::new(v, v, v, 255)
        }
    }
}

const FAILURE_WORDS: &[&str] = &[
    "error", "Error", "ERROR", "errors", "FAIL", "FAILED", "failed", "Failed", "failure",
    "failures", "panicked", "fatal", "Fatal", "FATAL", "✗", "✘", "×",
];
const WARNING_WORDS: &[&str] = &[
    "warning", "Warning", "WARNING", "warnings", "WARN", "warn", "deprecated", "DEPRECATED",
];
const SUCCESS_WORDS: &[&str] = &[
    "PASS", "PASSED", "passed", "ok", "OK", "success", "Success", "succeeded", "✓", "√", "✔",
    "Finished",
];

/// Terminal-style accents for uncoloured output, sorted and non-overlapping.
fn accents(text: &str, theme: &Theme) -> Vec<(Range<usize>, ColorU)> {
    let diff = text
        .lines()
        .any(|line| line.starts_with("@@ ") || line.starts_with("diff --git "));
    let mut out = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        if diff {
            let color = if body.starts_with("+++") || body.starts_with("---") {
                None
            } else if body.starts_with('+') {
                Some(theme.ansi(2))
            } else if body.starts_with('-') {
                Some(theme.ansi(1))
            } else if body.starts_with("@@") {
                Some(theme.ansi(6))
            } else {
                None
            };
            if let Some(color) = color {
                out.push((offset..offset + body.len(), color));
                offset += line.len();
                continue;
            }
        }
        line_accents(body, offset, theme, &mut out);
        offset += line.len();
    }
    out
}

fn line_accents(line: &str, offset: usize, theme: &Theme, out: &mut Vec<(Range<usize>, ColorU)>) {
    let mut found: Vec<(Range<usize>, ColorU)> = Vec::new();
    for link in find_terminal_links(line) {
        match link.kind {
            TerminalLinkKind::Url(_) => found.push((link.start..link.end, theme.link())),
            TerminalLinkKind::FilePath { raw, .. } => {
                let split = link.start + position_start(&raw);
                found.push((link.start..split, file_color(theme)));
                if split < link.end {
                    found.push((split..link.end, position_color(theme)));
                }
            }
        }
    }
    let mut previous = "";
    for word in line.split(char::is_whitespace) {
        // `word` is a slice of `line`, so its offset is the pointer distance.
        let start = word.as_ptr() as usize - line.as_ptr() as usize;
        let mut core = word.trim_end_matches([':', '.', ',', ';', '!']);
        let mut lead = 0;
        if let Some(inner) = core.strip_prefix(['[', '(']) {
            core = inner.trim_end_matches([']', ')']);
            lead = 1;
        }
        let color = if previous == "0" {
            // "0 failed" is good news.
            None
        } else if FAILURE_WORDS.contains(&core)
            || core.starts_with("error[")
            || core.starts_with("Error[")
        {
            Some(theme.ansi(1))
        } else if WARNING_WORDS.contains(&core) || core.starts_with("warning[") {
            Some(theme.ansi(3))
        } else if SUCCESS_WORDS.contains(&core) {
            Some(theme.ansi(2))
        } else {
            None
        };
        if !word.is_empty() {
            previous = word;
        }
        let Some(color) = color else { continue };
        let range = start + lead..start + lead + core.len();
        if !found.iter().any(|(r, _)| r.start < range.end && range.start < r.end) {
            found.push((range, color));
        }
    }
    found.sort_by_key(|(range, _)| range.start);
    out.extend(
        found
            .into_iter()
            .map(|(range, color)| (offset + range.start..offset + range.end, color)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn painted<'a>(text: &'a str, paint: &[(Range<usize>, ColorU, bool)]) -> Vec<(&'a str, ColorU)> {
        paint.iter().map(|(r, c, _)| (&text[r.clone()], *c)).collect()
    }

    #[test]
    fn whole_file_references_split_into_path_and_position() {
        assert_eq!(file_reference("src/checkout/payment.ts"), Some(23));
        assert_eq!(file_reference("payment.ts:12"), Some(10));
        assert_eq!(file_reference("dragent/app/src/cli/team.rs:1269-1303"), Some(27));
        assert_eq!(file_reference("a.rs:12:3"), Some(4));
        assert_eq!(file_reference("Cargo.toml"), Some(10));
        assert_eq!(file_reference("expires_at"), None);
        assert_eq!(file_reference("1.24"), None);
        assert_eq!(file_reference("cargo test"), None);
        assert_eq!(file_reference("v1.2.3"), None);
    }

    #[test]
    fn colours_the_program_wrote_are_kept() {
        let theme = Theme::default();
        let (text, paint) = command_output("\x1b[32mgreen\x1b[0m plain \x1b[1;38;5;9mbold red\x1b[m", &theme);
        assert_eq!(text, "green plain bold red");
        assert_eq!(painted(&text, &paint), vec![("green", theme.ansi(2)), ("bold red", theme.ansi(9))]);
        assert!(paint[1].2);
    }

    #[test]
    fn uncoloured_output_gets_terminal_accents() {
        let theme = Theme::default();
        let (text, paint) = command_output(
            "PASS checkout/payment.test.ts\nerror[E0425]: cannot find value\n  --> src/main.rs:4:5\nwarning: unused\ntest result: ok. 3 passed; 0 failed",
            &theme,
        );
        assert_eq!(
            painted(&text, &paint),
            vec![
                ("PASS", theme.ansi(2)),
                ("checkout/payment.test.ts", file_color(&theme)),
                ("error[E0425]", theme.ansi(1)),
                ("src/main.rs", file_color(&theme)),
                (":4:5", position_color(&theme)),
                ("warning", theme.ansi(3)),
                ("ok", theme.ansi(2)),
                ("passed", theme.ansi(2)),
            ]
        );
    }

    #[test]
    fn diffs_colour_added_and_removed_lines() {
        let theme = Theme::default();
        let (text, paint) = command_output("@@ -1 +1 @@\n-old\n+new\n same", &theme);
        assert_eq!(
            painted(&text, &paint),
            vec![("@@ -1 +1 @@", theme.ansi(6)), ("-old", theme.ansi(1)), ("+new", theme.ansi(2))]
        );
    }

    #[test]
    fn control_sequences_never_reach_the_text() {
        let theme = Theme::default();
        let (text, _) = command_output("\x1b[?1049h\x1b[2J\x1b]8;;https://x.y\x07label\x1b]8;;\x1b\\\r\nnext", &theme);
        assert_eq!(text, "label\nnext");
    }
}
