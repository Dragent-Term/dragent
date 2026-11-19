//! Per-block output filter state.
//!
//! Mirrors Warp's "Filter block output" affordance: an inline bar under a block
//! header that hides output lines not matching a query. Supports case-insensitive
//! substring matching and an optional regex mode, plus a live match count.

use regex::Regex;

use crate::terminal::model::block::Block;
use crate::theme::Theme;

/// Return the output lines currently visible for a block. Running blocks are
/// read from their live emulator; completed blocks use their stored snapshot.
/// This is the canonical source for filtering, counting, and copy actions.
pub fn block_output_lines(
    block: &Block,
    theme: &Theme,
) -> Vec<crate::console::escape_parse::ColoredLine> {
    let mut lines = if let Some(emu) = &block.emulator {
        crate::console::escape_parse::snapshot_emulator_lines(emu, theme)
    } else {
        (*block.lines).clone()
    };

    if block.emulator.is_some() {
        #[cfg(not(windows))]
        if !lines.is_empty() {
            lines.remove(0);
        }
        #[cfg(windows)]
        {
            let leading_blanks = lines
                .iter()
                .take_while(|line| line.plain_text().trim().is_empty())
                .count();
            lines.drain(..leading_blanks);
        }
    }
    lines
}

/// Filter state for a single block's output. Present in the view's
/// `block_filters` map only while the bar is shown for that block.
#[derive(Default, Clone)]
pub struct BlockFilterState {
    /// Current query text (raw, as typed).
    pub query: String,
    /// When true, `query` is interpreted as a regular expression.
    pub use_regex: bool,
    /// Cached compiled pattern when `use_regex` is on and `query` compiles.
    compiled: Option<Regex>,
    /// True when regex mode is on but `query` failed to compile.
    pub regex_error: bool,
}

impl BlockFilterState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the query is empty (no filtering — every line passes).
    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    fn recompile(&mut self) {
        self.compiled = None;
        self.regex_error = false;
        if self.use_regex && !self.query.is_empty() {
            match Regex::new(&self.query) {
                Ok(re) => self.compiled = Some(re),
                Err(_) => self.regex_error = true,
            }
        }
    }

    /// Sync the query text (from the embedded filter editor) and recompile.
    pub fn set_query(&mut self, query: &str) {
        if self.query != query {
            self.query = query.to_string();
            self.recompile();
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn toggle_regex(&mut self) {
        self.use_regex = !self.use_regex;
        self.recompile();
    }

    /// Whether a plain-text line passes the filter. An empty query, or an
    /// invalid regex, matches everything (so the user never loses their output
    /// mid-typing an incomplete pattern).
    pub fn matches(&self, line: &str) -> bool {
        if self.query.is_empty() {
            return true;
        }
        if self.use_regex {
            match &self.compiled {
                Some(re) => re.is_match(line),
                None => true,
            }
        } else {
            line.to_lowercase().contains(&self.query.to_lowercase())
        }
    }

    /// Char-index ranges `[start, end)` within `line` (indexed by `char`, not
    /// byte) that the query matches, left-to-right and non-overlapping. Used to
    /// highlight the matched substrings inside a kept line.
    ///
    /// Kept consistent with [`Self::matches`]: substring mode folds case with
    /// the same `to_lowercase` rules (mapping folded positions back to original
    /// char indices), and regex mode uses the same compiled pattern. Returns
    /// empty when the query is empty or an invalid regex matches everything.
    pub fn match_char_ranges(&self, line: &str) -> Vec<(usize, usize)> {
        if self.query.is_empty() {
            return Vec::new();
        }

        if self.use_regex {
            let Some(re) = &self.compiled else {
                return Vec::new();
            };
            // Map each byte offset that starts a char to that char's index so the
            // regex's byte spans become char-index spans.
            let byte_to_char: std::collections::HashMap<usize, usize> = line
                .char_indices()
                .enumerate()
                .map(|(ci, (b, _))| (b, ci))
                .collect();
            let total_chars = line.chars().count();
            let mut ranges = Vec::new();
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue;
                }
                let cs = byte_to_char.get(&m.start()).copied().unwrap_or(0);
                let ce = byte_to_char.get(&m.end()).copied().unwrap_or(total_chars);
                if cs < ce {
                    ranges.push((cs, ce));
                }
            }
            return ranges;
        }

        // Substring mode: fold both sides to lowercase, remembering which original
        // char each folded char came from (folding can change length, e.g. 'İ').
        let mut folded: Vec<char> = Vec::new();
        let mut origin: Vec<usize> = Vec::new();
        for (ci, ch) in line.chars().enumerate() {
            for lc in ch.to_lowercase() {
                folded.push(lc);
                origin.push(ci);
            }
        }
        let needle: Vec<char> = self.query.chars().flat_map(|c| c.to_lowercase()).collect();
        if needle.is_empty() || folded.len() < needle.len() {
            return Vec::new();
        }

        let mut ranges = Vec::new();
        let mut i = 0;
        while i + needle.len() <= folded.len() {
            if folded[i..i + needle.len()] == needle[..] {
                let start_char = origin[i];
                let end_char = origin[i + needle.len() - 1] + 1;
                ranges.push((start_char, end_char));
                i += needle.len(); // non-overlapping
            } else {
                i += 1;
            }
        }
        ranges
    }

    /// Number of lines in `lines` that match the current query.
    pub fn count_matches<'a>(&self, lines: impl IntoIterator<Item = &'a str>) -> usize {
        if self.query.is_empty() {
            return 0;
        }
        lines.into_iter().filter(|l| self.matches(l)).count()
    }
}

/// Count matches against the same line snapshot used by the block renderer.
/// Running blocks keep their output in the live emulator and leave `lines`
/// empty until completion; counting only `block.lines` therefore incorrectly
/// reports zero matches while a command is still producing output.
pub fn count_block_matches(block: &Block, filter: &BlockFilterState, theme: &Theme) -> usize {
    let lines = block_output_lines(block, theme);

    lines
        .iter()
        .filter(|line| {
            let text = line.plain_text();
            filter.matches(&text)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_all() {
        let f = BlockFilterState::new();
        assert!(f.matches("anything"));
        assert!(f.is_empty());
    }

    #[test]
    fn substring_is_case_insensitive() {
        let mut f = BlockFilterState::new();
        f.set_query("Error");
        assert!(f.matches("fatal error: boom"));
        assert!(f.matches("ERROR"));
        assert!(!f.matches("all good"));
    }

    #[test]
    fn regex_mode_matches_and_counts() {
        let mut f = BlockFilterState::new();
        f.toggle_regex();
        f.set_query("TS\\d+");
        assert!(f.matches("error TS2339"));
        assert!(!f.matches("plain line"));
        let lines = ["error TS2339", "error TS1005", "note: ok"];
        assert_eq!(f.count_matches(lines), 2);
    }

    #[test]
    fn invalid_regex_matches_everything() {
        let mut f = BlockFilterState::new();
        f.toggle_regex();
        f.set_query("("); // invalid
        assert!(f.regex_error);
        assert!(f.matches("still shown"));
    }

    #[test]
    fn match_char_ranges_substring_case_insensitive() {
        let mut f = BlockFilterState::new();
        f.set_query("err");
        // "an Error and an error" — matches "Err" (3..6) and "err" (16..19).
        let line = "an Error and an error";
        assert_eq!(f.match_char_ranges(line), vec![(3, 6), (16, 19)]);
    }

    #[test]
    fn match_char_ranges_regex() {
        let mut f = BlockFilterState::new();
        f.toggle_regex();
        f.set_query("TS\\d+");
        let line = "error TS2339 here";
        assert_eq!(f.match_char_ranges(line), vec![(6, 12)]);
    }

    #[test]
    fn match_char_ranges_wide_chars_use_char_indices() {
        let mut f = BlockFilterState::new();
        f.set_query("ok");
        // Leading wide char occupies one char index but two columns; the range is
        // reported in CHAR indices (caller converts to columns).
        let line = "世ok";
        assert_eq!(f.match_char_ranges(line), vec![(1, 3)]);
    }

    #[test]
    fn match_char_ranges_empty_when_no_query() {
        let f = BlockFilterState::new();
        assert!(f.match_char_ranges("anything").is_empty());
    }

    #[test]
    fn set_query_updates_and_recompiles() {
        let mut f = BlockFilterState::new();
        f.set_query("errx");
        f.set_query("err");
        assert_eq!(f.query(), "err");
        assert!(f.matches("error"));
    }
}
