//! Interactive block output — links, `ls` entries, tooltips (ported from ConsolePane).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use once_cell::sync::Lazy;
use regex::Regex;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildAnchor, ClippedScrollStateHandle, ConstrainedBox, Container, CrossAxisAlignment,
    Element, Empty, Expanded, Fill, Flex, Hoverable, Image, MainAxisSize, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Stack, Text,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::geometry::vector::vec2f;
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::tool_tip::Tooltip;
use warpui_core::EventContext;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::image_cache::CacheOption;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::console::dir_listing::{
    DIR_GRID_ROW_SPACING, DirEntry, DirSort, DirView, EntryKind, dir_grid_column_count,
    entry_click_hint_label, link_click_hint_label, modifier_click_label,
};
use crate::console::escape_parse::ColoredLine;
use crate::terminal::model::block::Block;
use crate::terminal::view::actions::{EntryHint, LinkHint, TerminalAction};
use crate::theme::{TERMINAL_PADDING_LEFT, Theme, mono_font_size};

const BOOTSTRAP_LINK_BLOCK_ID: usize = usize::MAX;

pub static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https?://[^\s]+").expect("valid url regex"));

pub static MARKDOWN_LINK_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\[([^\]]+)\]\((https?://[^\)]+)\)").expect("valid markdown link regex")
});

// Quotes give paths containing spaces an unambiguous end. Detect the full
// path before the bare-path fallback, including when its text soft-wraps.
static QUOTED_PATH_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r#""((?:(?:~|\.{1,2})?/|[A-Za-z]:[\\/]|https?://)[^"\r\n]+)""#,
        "|",
        r#"'((?:(?:~|\.{1,2})?/|[A-Za-z]:[\\/]|https?://)[^'\r\n]+)'"#,
    ))
    .expect("valid quoted path regex")
});

/// Matches things that look like a file-system path in arbitrary terminal
/// output: a `dir/file.ext`-style relative path, a `dir/sub/` folder (the
/// trailing slash is what tells it apart from prose like "and/or"), an
/// absolute/home path, or a bare filename with a recognized source/config
/// extension — each optionally followed by a `:line` or `:line:col` suffix
/// (compiler/stack-trace style). A fast pre-filter only; see
/// `find_terminal_links` for the token-boundary check and
/// `resolve_terminal_path` for resolution.
pub static FILE_PATH_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r"(?:~|\.{1,2})?/[A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)*/?(?::\d+(?::\d+)?)?",
        r"|(?:[A-Za-z0-9_.\-]+/)+[A-Za-z0-9_.\-]+\.[A-Za-z0-9]{1,10}(?::\d+(?::\d+)?)?",
        r"|(?:[A-Za-z0-9_.\-]+/)+",
        r"|\b[A-Za-z0-9_.\-]+\.(?:",
        "rs|ts|tsx|js|jsx|mjs|cjs|py|pyi|go|rb|rake|php|java|kt|kts|swift|dart|",
        "c|h|cc|cpp|cxx|hpp|cs|json|jsonc|json5|yaml|yml|toml|md|mdx|",
        "html|htm|css|scss|sass|less|sh|bash|zsh|fish|sql|proto|xml|txt|lock|",
        "vue|svelte|lua|ex|exs|hs|clj|cljs|zig|nim|sol|graphql|gql|tf|tfvars|hcl",
        r")(?::\d+(?::\d+)?)?\b",
    ))
    .expect("valid file path regex")
});

/// A link-like span detected in terminal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalLinkKind {
    Url(String),
    FilePath {
        /// The raw matched text, including any `:line:col` suffix — this is
        /// what gets resolved against a block's cwd when opened.
        raw: String,
        line: Option<u32>,
        column: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLinkMatch {
    pub start: usize,
    pub end: usize,
    pub kind: TerminalLinkKind,
}

/// Split a `path[:line[:col]]` match into its path and optional position.
pub fn split_path_suffix(raw: &str) -> (&str, Option<u32>, Option<u32>) {
    let mut parts = raw.rsplitn(3, ':');
    let last = parts.next().unwrap_or(raw);
    let Ok(last_num) = last.parse::<u32>() else {
        return (raw, None, None);
    };
    let Some(mid) = parts.next() else {
        return (raw, None, None);
    };
    if let Ok(mid_num) = mid.parse::<u32>() {
        if let Some(path) = parts.next() {
            return (path, Some(mid_num), Some(last_num));
        }
    }
    let path_len = raw.len() - last.len() - 1;
    (&raw[..path_len], Some(last_num), None)
}

/// Find URL, Markdown-link, and file-path matches in `text`, in priority
/// order (Markdown links, then bare URLs, then file paths), skipping any
/// span already claimed by a higher-priority match.
pub fn find_terminal_links(text: &str) -> Vec<TerminalLinkMatch> {
    let mut matches: Vec<TerminalLinkMatch> = Vec::new();
    let mut claimed: Vec<std::ops::Range<usize>> = Vec::new();

    for caps in QUOTED_PATH_REGEX.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let path = caps.get(1).or_else(|| caps.get(2)).unwrap();
        let (_, line, column) = split_path_suffix(path.as_str());
        claimed.push(whole.start()..whole.end());
        matches.push(TerminalLinkMatch {
            start: path.start(),
            end: path.end(),
            kind: if path.as_str().starts_with("https://") || path.as_str().starts_with("http://") {
                TerminalLinkKind::Url(path.as_str().to_string())
            } else {
                TerminalLinkKind::FilePath {
                    raw: path.as_str().to_string(),
                    line,
                    column,
                }
            },
        });
    }

    for caps in MARKDOWN_LINK_REGEX.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let url = caps
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        claimed.push(whole.start()..whole.end());
        matches.push(TerminalLinkMatch {
            start: whole.start(),
            end: whole.end(),
            kind: TerminalLinkKind::Url(url),
        });
    }

    for mat in URL_REGEX.find_iter(text) {
        if claimed
            .iter()
            .any(|r| r.start < mat.end() && mat.start() < r.end)
        {
            continue;
        }
        let url = sanitize_terminal_url(mat.as_str());
        claimed.push(mat.start()..mat.end());
        matches.push(TerminalLinkMatch {
            start: mat.start(),
            end: mat.end(),
            kind: TerminalLinkKind::Url(url),
        });
    }

    for mat in FILE_PATH_REGEX.find_iter(text) {
        if claimed
            .iter()
            .any(|r| r.start < mat.end() && mat.start() < r.end)
        {
            continue;
        }
        // A path starts at a token boundary. Without this, when the leading
        // relative form fails to match (e.g. a row cut off before the
        // extension), the absolute form would match from the first `/` in
        // the middle of the word — `dragent/crates/x` becoming `/crates/x`.
        if text[..mat.start()]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '~'))
        {
            continue;
        }
        // …and ends at one: the folder form must not claim the `dir/` prefix
        // of a longer word (`and/` in "and/or", or `a/b/` of a cut-off
        // `a/b/file`). A trailing `.` is allowed — it's sentence punctuation.
        if text[mat.end()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/'))
        {
            continue;
        }
        let (path, line, column) = split_path_suffix(mat.as_str());
        if path.is_empty() {
            continue;
        }
        matches.push(TerminalLinkMatch {
            start: mat.start(),
            end: mat.end(),
            kind: TerminalLinkKind::FilePath {
                raw: mat.as_str().to_string(),
                line,
                column,
            },
        });
    }

    matches.sort_by_key(|m| m.start);
    matches
}

/// Resolve a raw terminal file-path match (as captured by
/// [`FILE_PATH_REGEX`]/[`split_path_suffix`]) against a block's cwd. Expands
/// `~`, leaves absolute paths untouched, and joins everything else onto
/// `cwd`. Does not touch the filesystem — callers decide how to handle a
/// path that doesn't actually exist.
pub fn resolve_terminal_path(raw_path: &str, cwd: &std::path::Path) -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(home_relative) = raw_path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(home_relative);
        }
    }
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

/// The string a link opens: the URL itself, or the raw matched path text
/// (including any `:line:col` suffix).
pub fn link_target(kind: &TerminalLinkKind) -> String {
    match kind {
        TerminalLinkKind::Url(url) => url.clone(),
        TerminalLinkKind::FilePath { raw, .. } => raw.clone(),
    }
}

/// Upper bound on how many soft-wrapped rows are joined into one logical
/// line for link detection, so a pathological never-ending wrapped line
/// can't make every hover scan the whole block.
const MAX_WRAPPED_RUN: usize = 64;

/// A quoted path on a card row has a fixed, repeated gutter. Removing only
/// that exact prefix preserves literal spaces in the path itself.
fn card_path_prefix(text: &str) -> Option<(&str, char)> {
    let rail = text.find('│')?;
    if !text[..rail].chars().all(char::is_whitespace) {
        return None;
    }
    let tail = &text[rail + '│'.len_utf8()..];
    let value = tail.trim_start_matches(' ');
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let path = &value[quote.len_utf8()..];
    if !(path.starts_with('/')
        || path.starts_with("~/")
        || path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with("https://")
        || path.starts_with("http://")
        || path
            .as_bytes()
            .get(1..3)
            .is_some_and(|v| v == b":/" || v == b":\\"))
    {
        return None;
    }
    Some((&text[..text.len() - value.len()], quote))
}

fn card_path_bounds(lines: &[ColoredLine], idx: usize) -> Option<(usize, usize)> {
    for first in (idx.saturating_sub(MAX_WRAPPED_RUN)..=idx).rev() {
        let text = lines.get(first)?.plain_text();
        let Some((prefix, quote)) = card_path_prefix(&text) else {
            continue;
        };
        // This row must actually start a multi-row quoted value.
        if text[prefix.len() + quote.len_utf8()..].contains(quote) {
            continue;
        }
        for last in first + 1..lines.len().min(first + MAX_WRAPPED_RUN + 1) {
            let row = lines[last].plain_text();
            let content = if lines[last - 1].wrapped {
                row.as_str()
            } else if let Some(content) = row.strip_prefix(prefix) {
                content
            } else {
                break;
            };
            if content.contains(quote) {
                if idx <= last {
                    return Some((first, last));
                }
                break;
            }
        }
    }
    None
}

/// `[first, last]` indices of the soft-wrap run containing `idx`: rows the
/// terminal auto-wrapped (no real newline) belong to the same logical line.
pub fn wrapped_run_bounds(lines: &[ColoredLine], idx: usize) -> (usize, usize) {
    if let Some(bounds) = card_path_bounds(lines, idx) {
        return bounds;
    }
    let mut first = idx;
    while first > 0 && lines[first - 1].wrapped && idx - first < MAX_WRAPPED_RUN {
        first -= 1;
    }
    let mut last = idx;
    while last + 1 < lines.len() && lines[last].wrapped && last - first < MAX_WRAPPED_RUN {
        last += 1;
    }
    (first, last)
}

/// A link found in a soft-wrap run, located as `(line index, char index)`
/// pairs. `end` is exclusive and sits on the line holding the link's last
/// character, so a path that wraps has `start.0 < end.0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLink {
    pub kind: TerminalLinkKind,
    pub start: (usize, usize),
    pub end: (usize, usize),
    /// Actual text segments; gutter/indent cells are never interactive.
    pub segments: Vec<(usize, usize, usize)>,
}

/// Detect links across `lines[first..=last]` joined as one logical line, so
/// a path the terminal wrapped mid-way is found whole instead of as two
/// unrelated fragments.
pub fn find_links_in_run(lines: &[ColoredLine], first: usize, last: usize) -> Vec<RunLink> {
    let mut joined = String::new();
    let mut line_starts: Vec<usize> = Vec::with_capacity(last + 1 - first);
    let mut chars = 0usize;
    let first_text = lines[first].plain_text();
    let prefix = card_path_prefix(&first_text).map(|(prefix, _)| prefix.to_string());
    let mut skipped = Vec::new();
    let mut lengths = Vec::new();
    for (index, line) in lines[first..=last].iter().enumerate() {
        line_starts.push(chars);
        let full = line.plain_text();
        let text = if index > 0 && !lines[first + index - 1].wrapped {
            prefix
                .as_deref()
                .and_then(|prefix| full.strip_prefix(prefix))
                .unwrap_or(&full)
        } else {
            &full
        };
        skipped.push(full.chars().count() - text.chars().count());
        lengths.push(text.chars().count());
        chars += text.chars().count();
        joined.push_str(text);
    }
    let locate = |char_idx: usize| -> (usize, usize) {
        let i = line_starts
            .partition_point(|&s| s <= char_idx)
            .saturating_sub(1);
        (first + i, skipped[i] + char_idx - line_starts[i])
    };
    find_terminal_links(&joined)
        .into_iter()
        .filter(|found| found.end > found.start)
        .map(|found| {
            let start_char = joined[..found.start].chars().count();
            let end_char = start_char + joined[found.start..found.end].chars().count();
            let (end_line, last_char) = locate(end_char - 1);
            RunLink {
                kind: found.kind,
                start: locate(start_char),
                end: (end_line, last_char + 1),
                segments: line_starts
                    .iter()
                    .enumerate()
                    .filter_map(|(index, &start)| {
                        let left = start_char.max(start);
                        let right = end_char.min(start + lengths[index]);
                        (left < right).then(|| {
                            (
                                first + index,
                                skipped[index] + left - start,
                                skipped[index] + right - start,
                            )
                        })
                    })
                    .collect(),
            }
        })
        .collect()
}

/// A link on a full-screen app's grid, as `(row, start_col, end_col)`
/// terminal-column segments — more than one when the link continues onto
/// following rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridLink {
    pub kind: TerminalLinkKind,
    pub segments: Vec<(usize, usize, usize)>,
}

/// Most rows one grid link may span.
const MAX_GRID_LINK_ROWS: usize = 8;
/// A row whose content ends this close to the right edge is "full": the app
/// broke it for width, not because the text ended.
const GRID_EDGE_MARGIN_COLS: usize = 6;

fn is_link_continuation_char(c: char) -> bool {
    c.is_alphanumeric() || "/._-~%?=&#:+@".contains(c)
}

/// Box-drawing / block glyphs TUIs use as gutters and borders (`│`, `▌`).
fn is_gutter_glyph(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '|' | '>')
}

/// Char index where a continuation row's text starts: past its indent and
/// an optional gutter glyph (Codex/Claude Code indent or border wrapped text).
fn continuation_start(chars: &[char]) -> usize {
    let mut i = chars.iter().take_while(|c| c.is_whitespace()).count();
    if chars.get(i).copied().is_some_and(is_gutter_glyph)
        && chars.get(i + 1).is_none_or(|c| c.is_whitespace())
    {
        i += 1;
        i += chars[i..].iter().take_while(|c| c.is_whitespace()).count();
    }
    i
}

/// Char index just past a row's content: trailing blanks and a right-hand
/// border glyph are not part of the text.
fn content_end(chars: &[char]) -> usize {
    let mut end = chars.len();
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end > 0 && is_gutter_glyph(chars[end - 1]) && end >= 2 && chars[end - 2].is_whitespace() {
        end -= 1;
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
    }
    end
}

/// Could row `r`'s text continue on row `r + 1`? Structural only: the
/// terminal soft-wrapped it, or it ends mid-token and the next row resumes
/// with a token. Whether a specific link really crosses is decided later.
fn grid_rows_may_join(lines: &[String], wrapped: &[bool], r: usize) -> bool {
    if wrapped.get(r).copied().unwrap_or(false) {
        return true;
    }
    let (Some(this), Some(next)) = (lines.get(r), lines.get(r + 1)) else {
        return false;
    };
    let this: Vec<char> = this.chars().collect();
    let end = content_end(&this);
    if end == 0 || !is_link_continuation_char(this[end - 1]) {
        return false;
    }
    let next: Vec<char> = next.chars().collect();
    next.get(continuation_start(&next))
        .copied()
        .is_some_and(is_link_continuation_char)
}

/// The link under `(row, col)` on a full-screen app's grid (Codex, Claude
/// Code, vim …). These apps wrap long text themselves — a path is split with
/// a real line break and the next row often starts with an indent or gutter —
/// so the terminal's soft-wrap flag alone would cut the link in half. A row
/// break inside a link is accepted when the terminal soft-wrapped it, when
/// the joined path exists on disk, or when the row is full (the app broke it
/// for width) — unless the part before the break is itself an existing file,
/// which means the next row is unrelated text.
pub fn grid_link_at(
    lines: &[String],
    wrapped: &[bool],
    cols: usize,
    cwd: Option<&std::path::Path>,
    row: usize,
    col: usize,
) -> Option<GridLink> {
    use super::grid_metrics::{char_index_at_terminal_column, terminal_column_at_char_index};

    let row_text = lines.get(row)?;
    let char_col = char_index_at_terminal_column(row_text, col);
    let is_wrapped = |r: usize| wrapped.get(r).copied().unwrap_or(false);

    // Joined text of rows `first..=last`, with each row's piece located as
    // (row, char start in row, char start in joined, char len).
    let build = |first: usize, last: usize| {
        let mut pieces: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut joined = String::new();
        let mut joined_chars = 0usize;
        for r in first..=last {
            let chars: Vec<char> = lines[r].chars().collect();
            let start = if r == first || is_wrapped(r - 1) {
                0
            } else {
                continuation_start(&chars)
            };
            let end = if r == last || is_wrapped(r) {
                chars.len()
            } else {
                content_end(&chars)
            }
            .max(start);
            joined.extend(&chars[start..end]);
            pieces.push((r, start, joined_chars, end - start));
            joined_chars += end - start;
        }
        (joined, pieces)
    };
    // The link covering the pointer in a joined run: (kind, start, end) in
    // joined chars.
    let link_in = |joined: &str, pieces: &[(usize, usize, usize, usize)]| {
        let &(_, row_start, row_joined, row_len) = pieces.iter().find(|p| p.0 == row)?;
        if char_col < row_start || char_col >= row_start + row_len {
            return None;
        }
        let pointer = row_joined + (char_col - row_start);
        find_terminal_links(joined).into_iter().find_map(|f| {
            let s = joined[..f.start].chars().count();
            let e = s + joined[f.start..f.end].chars().count();
            (pointer >= s && pointer < e).then_some((f.kind, s, e))
        })
    };
    // Is an app-made break after row `above`, inside a link spanning
    // `[link_start, link_end)` whose first `head_len` chars sit above the
    // break, a real continuation?
    let confirm_break = |above: usize, kind: &TerminalLinkKind, head: &str| {
        if is_wrapped(above) {
            return true;
        }
        let chars: Vec<char> = lines[above].chars().collect();
        let row_is_full = terminal_column_at_char_index(&lines[above], content_end(&chars))
            + GRID_EDGE_MARGIN_COLS
            >= cols;
        match (kind, cwd) {
            (TerminalLinkKind::FilePath { raw, .. }, Some(cwd)) => {
                let (whole, _, _) = split_path_suffix(raw);
                let (head, _, _) = split_path_suffix(head);
                if resolve_terminal_path(whole, cwd).exists() {
                    true
                } else if !head.is_empty() && resolve_terminal_path(head, cwd).is_file() {
                    false
                } else {
                    row_is_full
                }
            }
            _ => row_is_full,
        }
    };

    let (mut first, mut last) = (row, row);
    let (joined, pieces) = build(first, last);
    let (mut kind, mut link_start, mut link_end) = link_in(&joined, &pieces)?;
    let mut pieces = pieces;
    loop {
        let mut grew = false;
        let run_chars: usize = pieces.iter().map(|p| p.3).sum();
        // Downward: the link runs to the end of the last row.
        if link_end == run_chars
            && last + 1 < lines.len()
            && last + 1 - first < MAX_GRID_LINK_ROWS
            && grid_rows_may_join(lines, wrapped, last)
        {
            let (j, p) = build(first, last + 1);
            let boundary = p.last().map_or(0, |x| x.2);
            if let Some((k, s, e)) = link_in(&j, &p) {
                let head: String = j.chars().skip(s).take(boundary.saturating_sub(s)).collect();
                if s == link_start && e > boundary && confirm_break(last, &k, &head) {
                    (kind, link_start, link_end, pieces) = (k, s, e, p);
                    last += 1;
                    grew = true;
                }
            }
        }
        // Upward: the link starts at the beginning of the first row's text
        // (past any indent/gutter, which the first row's piece still holds).
        let first_chars: Vec<char> = lines[first].chars().collect();
        if link_start == continuation_start(&first_chars)
            && first > 0
            && last + 1 - first < MAX_GRID_LINK_ROWS
            && grid_rows_may_join(lines, wrapped, first - 1)
        {
            let (j, p) = build(first - 1, last);
            let boundary = p.get(1).map_or(0, |x| x.2);
            if let Some((k, s, e)) = link_in(&j, &p) {
                let head: String = j.chars().skip(s).take(boundary.saturating_sub(s)).collect();
                if s < boundary && confirm_break(first - 1, &k, &head) {
                    (kind, link_start, link_end, pieces) = (k, s, e, p);
                    first -= 1;
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    let segments = pieces
        .iter()
        .filter_map(|&(r, rs, js, len)| {
            let a = link_start.max(js);
            let b = link_end.min(js + len);
            (a < b).then(|| {
                let text = &lines[r];
                (
                    r,
                    terminal_column_at_char_index(text, rs + a - js),
                    terminal_column_at_char_index(text, rs + b - js),
                )
            })
        })
        .collect();
    Some(GridLink { kind, segments })
}

#[cfg(test)]
mod link_detect_tests {

    fn grid(rows: &[&str], cols: usize) -> Vec<String> {
        rows.iter().map(|r| format!("{r:<cols$}")).collect()
    }

    #[test]
    fn codex_style_hard_wrapped_path_is_one_link_from_both_rows() {
        // Codex breaks a long path with a real newline and indents the rest.
        let cols = 40;
        let lines = grid(
            &[
                "• Edited /Users/me/dev/dragent/crates/dr",
                "  agent_viewer/src/render/block.rs:12",
                "  next paragraph",
            ],
            cols,
        );
        let wrapped = vec![false; lines.len()];
        for (row, col) in [(0, 12), (0, 38), (1, 4), (1, 30)] {
            let link = grid_link_at(&lines, &wrapped, cols, None, row, col)
                .unwrap_or_else(|| panic!("no link at {row},{col}"));
            assert_eq!(
                link_target(&link.kind),
                "/Users/me/dev/dragent/crates/dragent_viewer/src/render/block.rs:12"
            );
            assert_eq!(link.segments.len(), 2, "{:?}", link.segments);
            assert_eq!(link.segments[0], (0, 9, 40));
            assert_eq!(link.segments[1], (1, 2, 37));
        }
    }

    #[test]
    fn gutter_glyph_before_continuation_is_skipped() {
        let cols = 31;
        let lines = grid(
            &["│ see https://example.com/a/b/c", "│ /d/e/f?x=1 done"],
            cols,
        );
        let link = grid_link_at(&lines, &[false, false], cols, None, 1, 4).expect("link");
        assert_eq!(
            link_target(&link.kind),
            "https://example.com/a/b/c/d/e/f?x=1"
        );
    }

    #[test]
    fn short_row_ending_in_a_path_does_not_swallow_the_next_line() {
        // Not full width: the app ended the line, it did not wrap it.
        let cols = 80;
        let lines = grid(&["Updated src/main.rs", "and then ran tests"], cols);
        let link = grid_link_at(&lines, &[false, false], cols, None, 0, 10).expect("link");
        assert_eq!(link_target(&link.kind), "src/main.rs");
        assert_eq!(link.segments, vec![(0, 8, 19)]);
        assert!(grid_link_at(&lines, &[false, false], cols, None, 1, 2).is_none());
    }

    #[test]
    fn existing_file_before_the_break_stops_the_join() {
        let dir = std::env::temp_dir().join(format!("dragent-gridlink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "").unwrap();
        let cols = 12;
        // Full row, but `a.rs` exists on its own and `a.rsnext` does not.
        let lines = grid(&["see ./a.rs", "next"], cols);
        let link = grid_link_at(&lines, &[false, false], 10, Some(&dir), 0, 6).expect("link");
        assert_eq!(link_target(&link.kind), "./a.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terminal_soft_wrap_joins_without_checks() {
        let cols = 20;
        let lines = vec![
            "x /very/long/path/t".to_string(),
            "hat/wraps.rs".to_string(),
        ];
        let link = grid_link_at(&lines, &[true, false], cols, None, 1, 3).expect("link");
        assert_eq!(link_target(&link.kind), "/very/long/path/that/wraps.rs");
    }

    use super::*;

    /// End to end through a real emulator: a path longer than the terminal
    /// is wide must be found whole from any of its rows, exactly as the hover
    /// path does it (`wrapped_run_bounds` + `find_links_in_run`).
    #[test]
    fn emulator_wrapped_path_is_one_link_from_every_row() {
        let theme = crate::theme::Theme::dark();
        let path =
            "/Users/me/dev/dragent/crates/dragent_viewer/src/terminal/render/block_output.rs";
        for cols in [20usize, 33, 40, 80] {
            let mut emu = dragent_terminal::Emulator::new(40, cols);
            emu.feed(format!("error at {path}:12:4 here\r\nnext line\r\n").as_bytes());
            let lines = crate::console::escape_parse::snapshot_emulator_lines(&emu, &theme);
            let rows: Vec<String> = lines.iter().map(|l| l.plain_text()).collect();
            let flags: Vec<bool> = lines.iter().map(|l| l.wrapped).collect();
            for idx in 0..lines.len() {
                let (first, last) = wrapped_run_bounds(&lines, idx);
                let links = find_links_in_run(&lines, first, last);
                if rows[idx].trim() == "next line" {
                    continue;
                }
                let targets: Vec<String> = links.iter().map(|l| link_target(&l.kind)).collect();
                assert!(
                    targets.iter().any(|t| t.starts_with(path)),
                    "cols={cols} row={idx} rows={rows:?} wrapped={flags:?} targets={targets:?}"
                );
            }
        }
    }
    use crate::console::escape_parse::TextSpan;

    fn line(text: &str, wrapped: bool) -> ColoredLine {
        ColoredLine {
            wrapped,
            spans: vec![TextSpan {
                text: text.to_string(),
                color: None,
                bg: None,
                flags: 0,
            }],
        }
    }

    #[test]
    fn wrapped_run_bounds_joins_only_soft_wrapped_rows() {
        let lines = vec![
            line("unrelated", false),
            line("dragent/app/src/agent/pipe", true),
            line("line/stream.rs", false),
            line("next", false),
        ];
        assert_eq!(wrapped_run_bounds(&lines, 1), (1, 2));
        assert_eq!(wrapped_run_bounds(&lines, 2), (1, 2));
        assert_eq!(wrapped_run_bounds(&lines, 0), (0, 0));
        assert_eq!(wrapped_run_bounds(&lines, 3), (3, 3));
    }

    #[test]
    fn path_split_by_a_soft_wrap_is_detected_whole() {
        let lines = vec![
            line("dragent/app/src/agent/pipe", true),
            line("line/stream.rs other", false),
        ];
        let found = find_links_in_run(&lines, 0, 1);
        assert_eq!(found.len(), 1);
        assert_eq!(
            link_target(&found[0].kind),
            "dragent/app/src/agent/pipeline/stream.rs"
        );
        assert_eq!(found[0].start, (0, 0));
        assert_eq!(found[0].end, (1, "line/stream.rs".len()));
    }

    #[test]
    fn hard_newline_does_not_join_paths() {
        let lines = vec![line("dragent/app/src/a.rs", false), line("b.rs", false)];
        assert_eq!(wrapped_run_bounds(&lines, 0), (0, 0));
        let found = find_links_in_run(&lines, 0, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(link_target(&found[0].kind), "dragent/app/src/a.rs");
    }

    fn only_path(text: &str) -> String {
        let found = find_terminal_links(text);
        assert_eq!(found.len(), 1, "expected one match in {text:?}: {found:?}");
        link_target(&found[0].kind)
    }

    #[test]
    fn folder_with_trailing_slash_is_matched_whole() {
        assert_eq!(
            only_path("        server/internal/execution/"),
            "server/internal/execution/"
        );
    }

    #[test]
    fn match_never_starts_mid_word() {
        // Cut off before its extension (e.g. a row that wrapped): no match,
        // rather than a truncated `/crates/...` that drops the first segment.
        let found = find_terminal_links(
            "modified:   dragent/crates/dragent_session_protocol/src/workspace_e",
        );
        assert!(found.is_empty(), "unexpected: {found:?}");
    }

    #[test]
    fn prose_slashes_are_not_folders() {
        assert!(find_terminal_links(r#"(use "git add" and/or "git commit -a")"#).is_empty());
    }

    #[test]
    fn absolute_folder_keeps_trailing_slash() {
        assert_eq!(only_path("cd /usr/local/bin/ now"), "/usr/local/bin/");
    }

    #[test]
    fn identical_paths_on_different_rows_have_distinct_starts() {
        let lines = vec![line("src/main.rs", false), line("src/main.rs", false)];
        let a = find_links_in_run(&lines, 0, 0);
        let b = find_links_in_run(&lines, 1, 1);
        assert_ne!(a[0].start, b[0].start);
    }

    #[test]
    fn split_path_suffix_handles_line_and_col() {
        assert_eq!(
            split_path_suffix("src/main.rs:42:10"),
            ("src/main.rs", Some(42), Some(10))
        );
    }

    #[test]
    fn split_path_suffix_handles_line_only() {
        assert_eq!(
            split_path_suffix("src/main.rs:42"),
            ("src/main.rs", Some(42), None)
        );
    }

    #[test]
    fn split_path_suffix_handles_no_suffix() {
        assert_eq!(
            split_path_suffix("src/main.rs"),
            ("src/main.rs", None, None)
        );
    }

    #[test]
    fn split_path_suffix_does_not_mistake_windows_drive_for_a_line_number() {
        // Not a case this codebase's paths hit (no Windows drive letters in
        // these regexes), but a numeric-looking non-suffix tail must not be
        // chopped off a path that doesn't actually have a `:line` suffix.
        assert_eq!(
            split_path_suffix("release-2024.txt"),
            ("release-2024.txt", None, None)
        );
    }

    #[test]
    fn finds_relative_path_with_slash() {
        let found = find_terminal_links("error in src/main.rs near the top");
        assert_eq!(found.len(), 1);
        assert!(
            matches!(&found[0].kind, TerminalLinkKind::FilePath { raw, .. } if raw == "src/main.rs")
        );
    }

    #[test]
    fn finds_path_with_line_and_col_suffix() {
        let found = find_terminal_links("thrown at src/lib.rs:120:5 during init");
        assert_eq!(found.len(), 1);
        match &found[0].kind {
            TerminalLinkKind::FilePath { raw, line, column } => {
                assert_eq!(raw, "src/lib.rs:120:5");
                assert_eq!(*line, Some(120));
                assert_eq!(*column, Some(5));
            }
            _ => panic!("expected a file path match"),
        }
    }

    #[test]
    fn finds_bare_filename_with_known_extension() {
        let found = find_terminal_links("missing field in Cargo.toml");
        assert_eq!(found.len(), 1);
        assert!(
            matches!(&found[0].kind, TerminalLinkKind::FilePath { raw, .. } if raw == "Cargo.toml")
        );
    }

    #[test]
    fn does_not_match_plain_prose_with_a_period() {
        let found = find_terminal_links("i.e. this should not match, nor should e.g. that");
        assert!(found.is_empty(), "unexpected matches: {found:?}");
    }

    #[test]
    fn url_takes_priority_over_overlapping_path_pattern() {
        let found =
            find_terminal_links("see https://github.com/foo/bar/blob/main/src/main.rs for details");
        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0].kind, TerminalLinkKind::Url(u) if u.starts_with("https://")));
    }

    #[test]
    fn markdown_link_takes_priority_over_bare_url() {
        let found = find_terminal_links("[docs](https://example.com/readme.md)");
        assert_eq!(found.len(), 1);
        assert!(
            matches!(&found[0].kind, TerminalLinkKind::Url(u) if u == "https://example.com/readme.md")
        );
    }

    #[test]
    fn finds_absolute_and_home_paths() {
        let found = find_terminal_links("check /etc/hosts and ~/.zshrc");
        assert_eq!(found.len(), 2);
        assert!(
            matches!(&found[0].kind, TerminalLinkKind::FilePath { raw, .. } if raw == "/etc/hosts")
        );
        assert!(
            matches!(&found[1].kind, TerminalLinkKind::FilePath { raw, .. } if raw == "~/.zshrc")
        );
    }

    #[test]
    fn resolve_terminal_path_joins_relative_against_cwd() {
        let cwd = std::path::Path::new("/home/user/project");
        assert_eq!(
            resolve_terminal_path("src/main.rs", cwd),
            std::path::PathBuf::from("/home/user/project/src/main.rs")
        );
    }

    #[test]
    fn resolve_terminal_path_leaves_absolute_paths_untouched() {
        let cwd = std::path::Path::new("/home/user/project");
        assert_eq!(
            resolve_terminal_path("/etc/hosts", cwd),
            std::path::PathBuf::from("/etc/hosts")
        );
    }
}

/// Placeholder editor type for consumers (the wasm web viewer) that don't embed
/// interactive editors inside blocks. The desktop app instantiates the render
/// context with its real `InputEditor` instead. `BlockRenderContext` never calls
/// methods on the editor — it only holds optional references — so any `Entity`
/// works.
pub struct NoEditor;
impl warpui::Entity for NoEditor {
    type Event = ();
}

pub struct BlockRenderContext<'a, E: warpui::Entity = NoEditor> {
    pub entry_hint: Option<&'a EntryHint>,
    pub link_hint: Option<&'a LinkHint>,
    /// The link/path currently under the pointer, persisted on the view (see
    /// `TerminalView::hovered_link` for why this can't live on the paint
    /// element itself). Seeds `BlockListElement`'s hover-underline each
    /// render, the same way `link_hint` seeds its "selected" highlight.
    pub hovered_link: Option<&'a LinkHint>,
    /// `ls` card hover, persisted on the view (see `DirHover`).
    pub dir_hover: crate::terminal::view::actions::DirHover,
    pub mouse_states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub shimmer_states: &'a RefCell<HashMap<String, ShimmerStateHandle>>,
    pub content_width_px: f32,
    pub theme: Theme,
    pub mono: FamilyId,
    pub dir_sort: DirSort,
    pub dir_view: DirView,
    pub feedback_editor: Option<&'a warpui::ViewHandle<E>>,
    /// Embedded editor for typing a custom answer to an `ask_user` question.
    pub question_editor: Option<&'a warpui::ViewHandle<E>>,
    /// Embedded editor for the active block output filter (when a filter is open).
    pub block_filter_editor: Option<&'a warpui::ViewHandle<E>>,
    /// Current match selection for the active block output filter.
    pub block_filter_match_index: usize,
    /// The copy button currently confirming a copy, by its
    /// `TerminalAction::CopyWithConfirmation` key.
    pub copied_key: Option<&'a str>,
    /// Persistent per-Mermaid-diagram canvas state (pan + zoom), keyed by
    /// "diagram:block_id:index" so it survives re-renders.
    pub diagram_states: &'a RefCell<HashMap<String, DiagramCanvasState>>,
}

type MouseStateHandle = warpui_core::elements::MouseStateHandle;
type ShimmerStateHandle = warpui_core::elements::shimmering_text::ShimmeringTextStateHandle;

/// Pan (scroll) + zoom state for one Mermaid diagram canvas. Cheap to clone
/// (both fields are shared handles), so it's vended from the map like the
/// mouse/shimmer state handles.
#[derive(Clone)]
pub struct DiagramCanvasState {
    pub scroll: ClippedScrollStateHandle,
    pub zoom: Rc<Cell<f32>>,
}

impl Default for DiagramCanvasState {
    fn default() -> Self {
        Self {
            scroll: ClippedScrollStateHandle::new(),
            zoom: Rc::new(Cell::new(1.0)),
        }
    }
}

#[derive(Clone, Copy)]
enum ZoomOp {
    In,
    Out,
    Reset,
}

impl<'a, E: warpui::Entity> BlockRenderContext<'a, E> {
    /// Whether the copy button named `key` should show its confirmation.
    pub fn is_copied(&self, key: &str) -> bool {
        self.copied_key == Some(key)
    }

    pub fn mouse_state(&self, key: &str) -> MouseStateHandle {
        let mut map = self.mouse_states.borrow_mut();
        map.entry(key.to_string())
            .or_insert_with(MouseStateHandle::default)
            .clone()
    }

    pub fn shimmer_state(&self, key: &str) -> ShimmerStateHandle {
        let mut map = self.shimmer_states.borrow_mut();
        map.entry(key.to_string())
            .or_insert_with(ShimmerStateHandle::new)
            .clone()
    }

    fn diagram_state(&self, key: &str) -> DiagramCanvasState {
        self.diagram_states
            .borrow_mut()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Render a Mermaid code block as a pan/zoom "diagram canvas": a panel bordered
    /// in the active tab's accent color, with a zoom toolbar, that scrolls the
    /// diagram so large ones can be inspected. Content is shown as-is (no diagram
    /// engine); zoom scales the monospace font size.
    pub fn render_mermaid_canvas(
        &self,
        code: &str,
        block_id: usize,
        idx: usize,
        diagrams_live: bool,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono;
        let key = format!("diagram:{block_id}:{idx}");
        let state = self.diagram_state(&key);
        let zoom = state.zoom.get().clamp(0.4, 4.0);
        let font_size = (12.5 * zoom).clamp(5.0, 50.0);

        let header_title = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new("diagram".to_string(), mono, crate::theme::text_size(10.5))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_child(Expanded::new(1.0, Empty::new().finish()).finish())
            .finish();
        let zoom_controls = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(self.zoom_button("−", &key, ZoomOp::Out))
            .with_child(
                Container::new(
                    Text::new(
                        format!("{}%", (zoom * 100.0).round() as i32),
                        mono,
                        crate::theme::text_size(9.5),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .with_padding_left(5.0)
                .with_padding_right(5.0)
                .finish(),
            )
            .with_child(self.zoom_button("+", &key, ZoomOp::In))
            .with_child(self.zoom_button("reset", &key, ZoomOp::Reset))
            .finish();
        let header = Stack::new()
            .with_child(header_title)
            .with_child(Align::new(zoom_controls).finish())
            .finish();

        let header_bar = Container::new(header)
            .with_padding_left(10.0)
            .with_padding_right(8.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .with_background_color(theme.surface_3())
            .with_border(Border::bottom(1.0).with_border_color(theme.outline()))
            .finish();

        // The source view (also used as a fallback). While the message is still
        // streaming the fenced block is incomplete, so we must NOT spawn a render
        // on a partial diagram — show source until the message completes.
        use crate::terminal::render::mermaid_render::{DiagramState, get_or_render};
        let source_view = |c: &str| -> Box<dyn Element> {
            Container::new(
                Text::new(c.to_string(), mono, font_size)
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(12.0)
            .with_padding_right(12.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
        };
        let body: Box<dyn Element> = if !diagrams_live {
            source_view(code)
        } else {
            match get_or_render(code) {
                DiagramState::Ready {
                    path,
                    width,
                    height,
                } => {
                    // Fit to the block width, scaled by zoom, aspect preserved. Width
                    // is capped at the available width so the image never overflows
                    // horizontally (which would bleed over the input bar) — no inner
                    // scrollable needed. Height grows naturally; the outer list scrolls.
                    // Diagrams are vector-rendered on demand (CacheOption::Original), so
                    // scaling up costs nothing in quality — always fill at least most of
                    // the available width by default instead of only ever shrinking down
                    // to the diagram's raw SVG viewBox size (which left small diagrams
                    // looking cramped).
                    let avail = if self.content_width_px > 80.0 {
                        self.content_width_px
                    } else {
                        640.0
                    };
                    let min_w = (avail * 0.7).max(360.0);
                    let base_w = width.max(min_w).min(avail);
                    let disp_w = (base_w * zoom).clamp(200.0, avail.max(base_w));
                    let disp_h = (disp_w * (height / width)).clamp(200.0, 8000.0);
                    let path_str = path.to_string_lossy().into_owned();
                    let path_for_render = path_str.clone();
                    let mouse = self.mouse_state(&format!("diagram-open:{key}"));
                    let clickable = Hoverable::new(mouse, move |st| {
                        let img = Image::new(
                            AssetSource::LocalFile {
                                path: path_for_render.clone(),
                            },
                            CacheOption::Original,
                        )
                        .contain()
                        .finish();
                        let sized = ConstrainedBox::new(img)
                            .with_max_width(disp_w)
                            .with_height(disp_h)
                            .finish();
                        let mut c = Container::new(sized);
                        if st.is_hovered() {
                            c = c.with_border(Border::all(1.5).with_border_color(theme.accent));
                        }
                        c.finish()
                    })
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |event_ctx, _app, _pos| {
                        event_ctx.dispatch_typed_action(TerminalAction::OpenDiagramViewer {
                            path: path_str.clone(),
                            width,
                            height,
                        });
                    })
                    .finish();
                    Container::new(clickable)
                        .with_padding_left(10.0)
                        .with_padding_right(10.0)
                        .with_padding_top(10.0)
                        .with_padding_bottom(10.0)
                        .finish()
                }
                DiagramState::Pending => Container::new(
                    Text::new(
                        "rendering diagram…".to_string(),
                        mono,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(16.0)
                .with_padding_bottom(16.0)
                .finish(),
                DiagramState::Failed(msg) => Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_child(
                            Text::new(
                                format!("diagram render failed: {msg}"),
                                mono,
                                crate::theme::text_size(10.5),
                            )
                            .with_color(theme.ansi_red())
                            .finish(),
                        )
                        .with_child(
                            Container::new(Empty::new().finish())
                                .with_margin_bottom(6.0)
                                .finish(),
                        )
                        .with_child(
                            Text::new(code.to_string(), mono, font_size)
                                .with_color(theme.main_text())
                                .finish(),
                        )
                        .finish(),
                )
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .finish(),
            }
        };
        let _ = &state.scroll;

        Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(header_bar)
                .with_child(body)
                .finish(),
        )
        .with_background_color(theme.surface_2())
        .with_border(Border::all(1.5).with_border_color(theme.accent))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
        .with_margin_top(4.0)
        .with_margin_bottom(4.0)
        .finish()
    }

    fn zoom_button(&self, label: &str, key: &str, op: ZoomOp) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono;
        let label = label.to_string();
        let mouse = self.mouse_state(&format!("zoom:{key}:{label}"));
        let zoom = self.diagram_state(key).zoom;
        Hoverable::new(mouse, move |st| {
            let hovered = st.is_hovered();
            let color = if hovered {
                theme.main_text()
            } else {
                theme.sub_text()
            };
            let mut c = Container::new(
                Text::new(label.clone(), mono, crate::theme::text_size(11.0))
                    .with_color(color)
                    .finish(),
            )
            .with_padding_left(6.0)
            .with_padding_right(6.0)
            .with_padding_top(1.0)
            .with_padding_bottom(1.0)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.0)));
            if hovered {
                c = c.with_background_color(theme.fg_overlay_1());
            }
            c.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |event_ctx, _app, _pos| {
            let cur = zoom.get();
            let next = match op {
                ZoomOp::In => cur * 1.2,
                ZoomOp::Out => cur / 1.2,
                ZoomOp::Reset => 1.0,
            };
            zoom.set(next.clamp(0.4, 4.0));
            event_ctx.notify();
        })
        .finish()
    }

    fn tooltip_styles(&self) -> UiComponentStyles {
        UiComponentStyles {
            font_family_id: Some(self.mono),
            font_size: Some(crate::theme::text_size(11.0)),
            font_color: Some(self.theme.main_text()),
            background: Some(Fill::Solid(self.theme.surface_2())),
            border_color: Some(Fill::Solid(self.theme.outline())),
            border_width: Some(1.0),
            border_radius: Some(CornerRadius::with_all(Radius::Pixels(6.0))),
            padding: Some(Coords {
                top: 5.0,
                bottom: 5.0,
                left: 8.0,
                right: 8.0,
            }),
            ..Default::default()
        }
    }

    fn with_tooltip(&self, child: Box<dyn Element>, hint_label: String) -> Box<dyn Element> {
        let tooltip = Tooltip::new(hint_label, self.tooltip_styles())
            .build()
            .finish();
        Stack::new()
            .with_child(child)
            .with_positioned_overlay_child(
                tooltip,
                OffsetPositioning::offset_from_parent(
                    vec2f(0.0, -6.0),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::BottomMiddle,
                ),
            )
            .finish()
    }

    fn render_link(&self, block_id: usize, url: String, base_color: ColorU) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono;
        let mouse_state = self.mouse_state(&format!("url:{block_id}:{url}"));
        let selected = self
            .link_hint
            .is_some_and(|h| h.block_id == block_id && h.url == url);
        let url_open = url.clone();
        let url_select = url.clone();

        let link_el = Hoverable::new(mouse_state, move |state| {
            let hovered = state.is_hovered();
            let color = if hovered || selected {
                theme.link()
            } else {
                base_color
            };
            let mut text = Text::new(url.clone(), mono, mono_font_size()).with_color(color);
            if hovered || selected {
                text = text.with_style(Properties {
                    weight: Weight::Semibold,
                    ..Default::default()
                });
            }
            let mut container = Container::new(text.finish())
                .with_padding_left(1.0)
                .with_padding_right(1.0)
                .with_padding_top(1.0)
                .with_padding_bottom(1.0);
            if selected {
                container = container
                    .with_background_color(theme.link_surface())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            } else if hovered {
                container = container
                    .with_background_color(theme.fg_overlay_1())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            } else {
                container =
                    container.with_border(Border::bottom(1.0).with_border_color(theme.link()));
            }
            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .with_propagate_drag()
        .on_mouse_down_with_modifiers(move |event_ctx: &mut EventContext, _app, pos, mods| {
            if mods.cmd || mods.ctrl {
                event_ctx.dispatch_typed_action(TerminalAction::OpenLink {
                    block_id,
                    url: url_open.clone(),
                });
            } else {
                event_ctx.dispatch_typed_action(TerminalAction::SelectLink {
                    block_id,
                    url: url_select.clone(),
                    occurrence: None,
                    absolute_position: pos,
                });
            }
        })
        .on_click_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, _mods| {
            event_ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
        })
        .finish();

        if selected {
            self.with_tooltip(link_el, link_click_hint_label())
        } else {
            link_el
        }
    }

    fn render_dir_grid_cell(&self, block_id: usize, entry: &DirEntry) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono;
        let is_dir = entry.kind == EntryKind::Directory;
        let dim = entry.name.starts_with('.');
        let name = entry.name.clone();
        let name_click = entry.name.clone();
        let hint_name = entry.name.clone();
        let entry_size = entry.size;
        let entry_modified = entry.modified.clone();
        let mouse_state = self.mouse_state(&format!("entry:{block_id}:{name}"));
        let selected = self
            .entry_hint
            .is_some_and(|h| h.block_id == block_id && h.name == name);

        let base_color = if dim {
            theme.disabled_text()
        } else if is_dir {
            theme.ansi_blue()
        } else {
            theme.main_text()
        };

        let (glyph, glyph_color) = file_glyph(&entry.name, is_dir, &theme);

        let mut meta_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        let mut has_meta = false;

        if let Some(sz) = entry_size {
            meta_row = meta_row.with_child(
                Text::new(fmt_size(sz), mono, crate::theme::text_size(9.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            );
            has_meta = true;
        }

        if let Some(ref mdf) = entry_modified {
            if has_meta {
                meta_row = meta_row
                    .with_child(Text::new("  ", mono, crate::theme::text_size(9.0)).finish());
            }
            meta_row = meta_row.with_child(
                Text::new(mdf.clone(), mono, crate::theme::text_size(9.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            );
            has_meta = true;
        }

        let name_hoverable = Hoverable::new(mouse_state, move |state| {
            let hovered = state.is_hovered();
            let text_color = if hovered {
                theme.main_text()
            } else {
                base_color
            };

            let name_label = Text::new(
                format!("{}{}", name, if is_dir { "/" } else { "" }),
                mono,
                crate::theme::text_size(11.5),
            )
            .with_color(text_color)
            .with_style(Properties {
                weight: if is_dir {
                    Weight::Semibold
                } else {
                    Weight::Normal
                },
                ..Default::default()
            })
            .finish();

            let glyph_label = Text::new(glyph, mono, crate::theme::text_size(10.0))
                .with_color(glyph_color)
                .finish();

            let first_row = Flex::row()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Container::new(glyph_label).with_padding_right(6.0).finish())
                .with_child(name_label)
                .finish();

            let mut container = Container::new(first_row)
                .with_padding_left(6.0)
                .with_padding_right(6.0)
                .with_padding_top(4.0)
                .with_padding_bottom(4.0);

            if selected {
                container = container
                    .with_background_color(theme.link_surface())
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            } else if hovered {
                container = container
                    .with_background_color(theme.fg_overlay_1())
                    .with_border(Border::all(1.0).with_border_color(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
            }

            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .with_propagate_drag()
        .on_mouse_down_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, mods| {
            let name_val = name_click.clone();
            if mods.cmd || mods.ctrl {
                event_ctx.dispatch_typed_action(TerminalAction::OpenDirEntry {
                    block_id,
                    name: name_val,
                    is_dir,
                });
            } else {
                event_ctx.dispatch_typed_action(TerminalAction::SelectDirEntry {
                    block_id,
                    name: name_val,
                    is_dir,
                });
            }
        })
        .on_click_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, _mods| {
            event_ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
        })
        .finish();

        let name_interactive = if selected {
            self.with_tooltip(name_hoverable, entry_click_hint_label(is_dir, &hint_name))
        } else {
            name_hoverable
        };

        let mut cell_col = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(name_interactive);

        if has_meta {
            cell_col = cell_col.with_child(
                Container::new(meta_row.finish())
                    .with_padding_left(16.0)
                    .with_padding_top(2.0)
                    .finish(),
            );
        }

        cell_col.finish()
    }

    fn append_colored_range(
        &self,
        text: &str,
        range: Range<usize>,
        color_ranges: &[(usize, usize, ColorU)],
        fallback: ColorU,
        current: &mut Option<Text>,
        children: &mut Vec<Box<dyn Element>>,
    ) {
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return;
        }
        let mut covered = false;
        for &(start, end, color) in color_ranges {
            let overlap_start = range.start.max(start);
            let overlap_end = range.end.min(end);
            if overlap_start < overlap_end
                && text.is_char_boundary(overlap_start)
                && text.is_char_boundary(overlap_end)
            {
                covered = true;
                append_text_fragment(
                    current,
                    children,
                    &text[overlap_start..overlap_end],
                    color,
                    self.mono,
                );
            }
        }
        if !covered {
            append_text_fragment(
                current,
                children,
                &text[range.start..range.end],
                fallback,
                self.mono,
            );
        }
        flush_text(current, children);
    }

    pub fn render_colored_line(
        &self,
        line: &ColoredLine,
        block_id: usize,
        horizontal_inset: f32,
    ) -> Box<dyn Element> {
        let theme = self.theme;
        let fallback = theme.main_text();

        let mut combined = String::new();
        let mut color_ranges: Vec<(usize, usize, ColorU)> = Vec::new();
        for span in &line.spans {
            if span.text.is_empty() {
                continue;
            }
            let start = combined.len();
            combined.push_str(&span.text);
            color_ranges.push((start, combined.len(), span.color.unwrap_or(fallback)));
        }

        let mut children: Vec<Box<dyn Element>> = Vec::new();
        let mut current_text: Option<Text> = None;
        let mut last_idx = 0usize;

        for mat in URL_REGEX.find_iter(&combined) {
            let start = mat.start();
            let end = mat.end();
            if start > last_idx {
                self.append_colored_range(
                    &combined,
                    last_idx..start,
                    &color_ranges,
                    fallback,
                    &mut current_text,
                    &mut children,
                );
            }
            flush_text(&mut current_text, &mut children);
            let url = sanitize_terminal_url(&combined[start..end]);
            let link_color = color_at_byte_offset(&color_ranges, start, fallback);
            children.push(self.render_link(block_id, url, link_color));
            last_idx = end;
        }

        if last_idx < combined.len() {
            self.append_colored_range(
                &combined,
                last_idx..combined.len(),
                &color_ranges,
                fallback,
                &mut current_text,
                &mut children,
            );
        }
        flush_text(&mut current_text, &mut children);

        let line_bottom_pad = if horizontal_inset == 0.0 { 3.0 } else { 2.0 };
        if children.is_empty() {
            return Container::new(Empty::new().finish())
                .with_padding_left(horizontal_inset)
                .with_padding_bottom(line_bottom_pad)
                .finish();
        }
        let inner = if children.len() == 1 {
            children.remove(0)
        } else {
            let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            for child in children {
                row = row.with_child(child);
            }
            row.finish()
        };
        Container::new(inner)
            .with_padding_left(horizontal_inset)
            .with_padding_bottom(line_bottom_pad)
            .finish()
    }

    fn sort_dir_entries(&self, entries: &mut [DirEntry]) {
        entries.sort_by(|a, b| {
            let a_dir = a.kind == EntryKind::Directory;
            let b_dir = b.kind == EntryKind::Directory;
            if a_dir != b_dir {
                return if a_dir {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            match self.dir_sort {
                DirSort::Size => b.size.unwrap_or(0).cmp(&a.size.unwrap_or(0)),
                DirSort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
    }

    pub fn render_dir_listing_grid(&self, block: &Block) -> Option<Box<dyn Element>> {
        // A failed command's output is error text, not a directory listing.
        if block.status.exit_code().unwrap_or(0) != 0 {
            return None;
        }
        let mut entries = block.listing(&self.theme)?.entries;
        if entries.is_empty() {
            return None;
        }
        self.sort_dir_entries(&mut entries);

        let theme = self.theme;
        let mono = self.mono;
        let block_id = block.id;

        let main_content: Box<dyn Element> = match self.dir_view {
            DirView::Grid => self.render_dir_grid_body(block_id, &entries),
            DirView::List => self.render_dir_list_body(block_id, &entries),
        };

        let entry_count = entries.len();
        let footer = Container::new(Box::new(
            Text::new(
                format!(
                    "{entry_count} items — click a name, {} to open",
                    modifier_click_label()
                ),
                mono,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.sub_text()),
        ))
        .with_padding_top(10.0)
        .finish();

        Some(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(main_content)
                .with_child(footer)
                .finish(),
        )
    }

    fn render_dir_grid_body(&self, block_id: usize, entries: &[DirEntry]) -> Box<dyn Element> {
        let column_count = dir_grid_column_count(self.content_width_px);

        let mut grid = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

        for (row_idx, chunk) in entries.chunks(column_count).enumerate() {
            if row_idx > 0 {
                grid = grid.with_child(
                    Container::new(Empty::new().finish())
                        .with_padding_top(DIR_GRID_ROW_SPACING)
                        .finish(),
                );
            }

            let mut row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Start);

            for col in 0..column_count {
                let slot = if let Some(entry) = chunk.get(col) {
                    Align::new(self.render_dir_grid_cell(block_id, entry))
                        .top_left()
                        .finish()
                } else {
                    Empty::new().finish()
                };
                row = row.with_child(Expanded::new(1.0, slot).finish());
            }

            grid = grid.with_child(row.finish());
        }

        Container::new(grid.finish())
            .with_padding_top(4.0)
            .with_padding_bottom(2.0)
            .finish()
    }

    fn render_dir_list_body(&self, block_id: usize, entries: &[DirEntry]) -> Box<dyn Element> {
        let theme = self.theme;
        let mono = self.mono;
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min);

        for entry in entries {
            let is_dir = entry.kind == EntryKind::Directory;
            let dim = entry.name.starts_with('.');
            let mouse_state = self.mouse_state(&format!("list:{}:{block_id}", entry.name));
            let name = entry.name.clone();
            let name_click = entry.name.clone();
            let hint_name = entry.name.clone();
            let entry_size = entry.size;
            let entry_modified = entry.modified.clone();
            let (glyph, glyph_color) = file_glyph(&entry.name, is_dir, &theme);
            let selected = self
                .entry_hint
                .is_some_and(|h| h.block_id == block_id && h.name == name);

            let base_color = if dim {
                theme.disabled_text()
            } else if is_dir {
                theme.ansi_blue()
            } else {
                theme.main_text()
            };

            let size_label = Text::new(
                entry_size.map(fmt_size).unwrap_or_default(),
                mono,
                crate::theme::text_size(10.0),
            )
            .with_color(theme.disabled_text())
            .finish();

            let modified_label = Text::new(
                entry_modified.clone().unwrap_or_default(),
                mono,
                crate::theme::text_size(10.0),
            )
            .with_color(theme.disabled_text())
            .finish();

            let name_hoverable = Hoverable::new(mouse_state, move |state| {
                let hovered = state.is_hovered();
                let text_color = if hovered {
                    theme.main_text()
                } else {
                    base_color
                };

                let name_label = Text::new(
                    format!("{}{}", name, if is_dir { "/" } else { "" }),
                    mono,
                    crate::theme::text_size(11.5),
                )
                .with_color(text_color)
                .with_style(Properties {
                    weight: if is_dir {
                        Weight::Semibold
                    } else {
                        Weight::Normal
                    },
                    ..Default::default()
                })
                .finish();

                let glyph_label = Text::new(glyph, mono, crate::theme::text_size(10.0))
                    .with_color(glyph_color)
                    .finish();

                let name_side = Flex::row()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(Container::new(glyph_label).with_padding_right(8.0).finish())
                    .with_child(name_label)
                    .finish();

                let mut container = Container::new(name_side)
                    .with_padding_left(8.0)
                    .with_padding_right(8.0)
                    .with_padding_top(4.0)
                    .with_padding_bottom(4.0);

                if selected {
                    container = container
                        .with_background_color(theme.link_surface())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
                } else if hovered {
                    container = container
                        .with_background_color(theme.fg_overlay_1())
                        .with_border(Border::all(1.0).with_border_color(theme.outline()))
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)));
                }

                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .with_propagate_drag()
            .on_mouse_down_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, mods| {
                let name_val = name_click.clone();
                if mods.cmd || mods.ctrl {
                    event_ctx.dispatch_typed_action(TerminalAction::OpenDirEntry {
                        block_id,
                        name: name_val,
                        is_dir,
                    });
                } else {
                    event_ctx.dispatch_typed_action(TerminalAction::SelectDirEntry {
                        block_id,
                        name: name_val,
                        is_dir,
                    });
                }
            })
            .on_click_with_modifiers(move |event_ctx: &mut EventContext, _app, _pos, _mods| {
                event_ctx.dispatch_typed_action(TerminalAction::ClearBlockHints);
            })
            .finish();

            let name_interactive = if selected {
                self.with_tooltip(name_hoverable, entry_click_hint_label(is_dir, &hint_name))
            } else {
                name_hoverable
            };

            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Expanded::new(1.0, name_interactive).finish())
                .with_child(Container::new(size_label).with_padding_right(12.0).finish())
                .with_child(modified_label)
                .finish();

            col = col.with_child(row);
        }

        Container::new(col.finish())
            .with_padding_top(4.0)
            .with_padding_bottom(2.0)
            .finish()
    }

    pub fn render_block_body(&self, block: &Block) -> Box<dyn Element> {
        if let Some(grid) = self.render_dir_listing_grid(block) {
            return grid;
        }
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);

        for line in block.lines.iter() {
            col = col.with_child(self.render_colored_line(line, block.id, 0.0));
        }
        col.finish()
    }

    pub fn render_bootstrap_lines(&self, lines: &[ColoredLine]) -> Box<dyn Element> {
        let mut col = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);
        for line in lines {
            col = col.with_child(self.render_colored_line(
                line,
                BOOTSTRAP_LINK_BLOCK_ID,
                TERMINAL_PADDING_LEFT,
            ));
        }
        Container::new(col.finish())
            .with_padding_left(0.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .finish()
    }
}

fn append_text_fragment(
    current: &mut Option<Text>,
    _children: &mut Vec<Box<dyn Element>>,
    text: &str,
    color: ColorU,
    mono: FamilyId,
) {
    if text.is_empty() {
        return;
    }
    if current.is_none() {
        *current = Some(Text::new(String::new(), mono, mono_font_size()));
    }
    if let Some(t) = current.as_mut() {
        t.add_text_with_highlights(text, color, Properties::default());
    }
}

fn flush_text(current: &mut Option<Text>, children: &mut Vec<Box<dyn Element>>) {
    if let Some(t) = current.take() {
        children.push(Box::new(t));
    }
}

fn color_at_byte_offset(
    ranges: &[(usize, usize, ColorU)],
    offset: usize,
    fallback: ColorU,
) -> ColorU {
    for &(start, end, color) in ranges {
        if offset >= start && offset < end {
            return color;
        }
    }
    fallback
}

pub fn sanitize_terminal_url(url: &str) -> String {
    url.trim_end_matches(|c: char| {
        matches!(c, ')' | ']' | '}' | '>' | '"' | '\'' | ',' | ';' | '.')
    })
    .to_string()
}

fn file_glyph(name: &str, is_dir: bool, theme: &Theme) -> (&'static str, ColorU) {
    if is_dir {
        ("▸", theme.ansi_blue())
    } else {
        let name_lower = name.to_lowercase();
        if name_lower.ends_with(".rs") {
            ("⬡", ColorU::new(239, 107, 48, 255))
        } else if name_lower.ends_with(".toml")
            || name_lower.ends_with(".json")
            || name_lower.ends_with(".yaml")
            || name_lower.ends_with(".yml")
            || name_lower.ends_with(".lock")
        {
            ("⚙", theme.sub_text())
        } else if name_lower.ends_with(".md") {
            ("✎", ColorU::new(178, 102, 255, 255))
        } else if name_lower.ends_with(".png")
            || name_lower.ends_with(".jpg")
            || name_lower.ends_with(".jpeg")
            || name_lower.ends_with(".gif")
            || name_lower.ends_with(".webp")
        {
            ("⌼", ColorU::new(0, 204, 204, 255))
        } else {
            ("·", theme.disabled_text())
        }
    }
}

fn fmt_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
