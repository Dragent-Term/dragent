/// Pure fuzzy-match algorithm for command history search.
/// No UI or model state — only data structures and scoring logic.

#[derive(Debug, Clone)]
pub struct HistoryMatch {
    pub command: String,
    pub match_ranges: Vec<std::ops::Range<usize>>,
    pub score: f32,
    pub count: usize,
    pub recency: usize, // 0 = most recent
}

#[derive(Debug, Default, Clone)]
pub struct HistorySearchState {
    pub open: bool,
    pub query: String,
    pub saved_buffer: String,
    pub results: Vec<HistoryMatch>,
    pub selected: usize,
}

impl HistorySearchState {
    pub fn refresh(&mut self, history: &[String]) {
        self.results = search(history, &self.query);
    }

    pub fn selected_command(&self) -> Option<&str> {
        self.results.get(self.selected).map(|m| m.command.as_str())
    }

    pub fn move_up(&mut self) {
        if !self.results.is_empty() && self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.results.is_empty() && self.selected + 1 < self.results.len() {
            self.selected += 1;
        }
    }
}

/// Return top 8 history matches for `query`.
///
/// Empty query → the 8 most recent unique commands.
/// Non-empty query → subsequence match scored by several bonuses.
pub fn search(history: &[String], query: &str) -> Vec<HistoryMatch> {
    if history.is_empty() {
        return Vec::new();
    }

    // Build de-duplicated list: most-recent occurrence wins for recency,
    // count accumulates across all occurrences.
    // We iterate from the end (most recent) so the first time we see a
    // command it gets recency = 0.
    let mut seen: std::collections::HashMap<&str, (usize, usize)> =
        std::collections::HashMap::new();
    for (rev_idx, cmd) in history.iter().rev().enumerate() {
        let entry = seen.entry(cmd.as_str()).or_insert((rev_idx, 0));
        entry.1 += 1; // increment count
    }

    if query.is_empty() {
        // Show 8 most recent unique commands.
        let mut unique: Vec<HistoryMatch> = seen
            .into_iter()
            .map(|(cmd, (recency, count))| HistoryMatch {
                command: cmd.to_string(),
                match_ranges: Vec::new(),
                score: -(recency as f32),
                count,
                recency,
            })
            .collect();
        unique.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        unique.truncate(8);
        return unique;
    }

    // Subsequence fuzzy match with scoring.
    let query_lower = query.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();

    let mut matches: Vec<HistoryMatch> = seen
        .into_iter()
        .filter_map(|(cmd, (recency, count))| fuzzy_match(cmd, &query_chars, recency, count))
        .collect();

    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(8);
    matches
}

/// Attempt a subsequence match of `query_chars` against `cmd`.
/// Returns `None` if no match, or a `HistoryMatch` with score and ranges.
fn fuzzy_match(
    cmd: &str,
    query_chars: &[char],
    recency: usize,
    count: usize,
) -> Option<HistoryMatch> {
    let cmd_lower = cmd.to_lowercase();
    let cmd_chars: Vec<char> = cmd_lower.chars().collect();
    let cmd_bytes: Vec<usize> = {
        let mut offsets = Vec::with_capacity(cmd.len());
        let mut byte_pos = 0;
        for c in cmd.chars() {
            offsets.push(byte_pos);
            byte_pos += c.len_utf8();
        }
        offsets
    };

    // Forward subsequence pass: find first valid match positions.
    let mut match_positions: Vec<usize> = Vec::with_capacity(query_chars.len());
    let mut ci = 0usize;
    for &qc in query_chars {
        let found = cmd_chars[ci..].iter().position(|&cc| cc == qc);
        match found {
            Some(rel) => {
                ci += rel;
                match_positions.push(ci);
                ci += 1;
            }
            None => return None,
        }
    }

    // Score the match.
    let mut score = 0.0f32;

    // Consecutive character bonus.
    let mut consecutive = 1usize;
    for i in 1..match_positions.len() {
        if match_positions[i] == match_positions[i - 1] + 1 {
            consecutive += 1;
            score += consecutive as f32 * 3.0;
        } else {
            consecutive = 1;
        }
    }

    // Word boundary bonus: matched char follows ' ', '-', '_', '/', or is at pos 0.
    for &pos in &match_positions {
        if pos == 0 {
            score += 5.0;
        } else {
            let prev = cmd_chars[pos - 1];
            if prev == ' ' || prev == '-' || prev == '_' || prev == '/' {
                score += 4.0;
            }
        }
    }

    // Early position bonus (penalise late-starting matches).
    if let Some(&first) = match_positions.first() {
        score += (20.0 - first.min(20) as f32).max(0.0);
    }

    // Recency bonus (0 = most recent = max bonus).
    score += (50.0 - recency.min(50) as f32).max(0.0);

    // Frequency bonus.
    score += (count.min(10) as f32) * 2.0;

    // Convert char positions to byte ranges.
    let match_ranges: Vec<std::ops::Range<usize>> = match_positions
        .iter()
        .map(|&char_pos| {
            let byte_start = cmd_bytes[char_pos];
            let byte_end = byte_start + cmd.chars().nth(char_pos).map_or(1, |c| c.len_utf8());
            byte_start..byte_end
        })
        .collect();

    Some(HistoryMatch {
        command: cmd.to_string(),
        match_ranges,
        score,
        count,
        recency,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_recent() {
        let history = vec!["ls".into(), "git status".into(), "cargo build".into()];
        let results = search(&history, "");
        assert!(results.len() <= 8);
        // Most recent should rank first.
        assert_eq!(results[0].command, "cargo build");
    }

    #[test]
    fn subsequence_match() {
        let history = vec!["git status".into(), "git log".into(), "ls".into()];
        let results = search(&history, "gst");
        assert!(!results.is_empty());
        assert_eq!(results[0].command, "git status");
    }

    #[test]
    fn no_match_returns_empty() {
        let history = vec!["ls".into(), "pwd".into()];
        let results = search(&history, "zzz");
        assert!(results.is_empty());
    }

    #[test]
    fn dedup_increments_count() {
        let history = vec!["ls".into(), "ls".into(), "ls".into()];
        let results = search(&history, "");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].count, 3);
    }
}
