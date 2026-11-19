//! A failed check's log, readable: GitHub's `run view --log-failed` prefixes
//! every line with its job, step and a timestamp, and keeps the colour codes
//! and workflow markers. The viewer shows the message alone, a heading where
//! each step starts, errors and warnings in their colours, and only the rows
//! on screen, so a long log never slows the window.

use std::sync::Arc;

use warpui::elements::{ScrollStateHandle, UniformListState};

/// Lines kept for showing: the end of a log is where it failed.
const MAX_LINES: usize = 5_000;
/// A line longer than this is cut for showing (Copy keeps it whole).
const MAX_LINE_CHARS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    Plain,
    /// Where a step starts.
    Step,
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LogLine {
    pub text: String,
    pub kind: LineKind,
}

/// The log open over the page.
pub(crate) struct CheckLog {
    pub run: u64,
    /// Shared, so drawing the visible rows never copies the log.
    pub lines: Arc<Vec<LogLine>>,
    /// Lines left out at the start (the log was longer than shown).
    pub skipped: usize,
    /// The whole log as it came, for Copy.
    pub raw: String,
    pub loading: bool,
    pub error: Option<String>,
    pub list: UniformListState,
    pub scroll: ScrollStateHandle,
}

impl CheckLog {
    pub(crate) fn loading(run: u64) -> Self {
        Self {
            run,
            lines: Arc::new(Vec::new()),
            skipped: 0,
            raw: String::new(),
            loading: true,
            error: None,
            list: UniformListState::default(),
            scroll: ScrollStateHandle::default(),
        }
    }

    /// The log arrived: keep its end, and open on its last line.
    pub(crate) fn fill(&mut self, raw: String) {
        let mut lines = parse(&raw);
        self.skipped = lines.len().saturating_sub(MAX_LINES);
        if self.skipped > 0 {
            lines.drain(..self.skipped);
        }
        self.lines = Arc::new(lines);
        self.raw = raw;
        self.loading = false;
        if !self.lines.is_empty() {
            self.list.scroll_to(self.lines.len() - 1);
        }
    }
}

/// `\x1b[31m` and friends.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters and intermediates, then one final letter.
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() || next == '~' {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// `2026-10-31T12:00:00.1234567Z message` → `message`.
fn strip_timestamp(text: &str) -> &str {
    let bytes = text.as_bytes();
    let looks_like_time = bytes.len() > 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[..4].iter().all(u8::is_ascii_digit);
    if !looks_like_time {
        return text;
    }
    match text.find(' ') {
        Some(space) if text[..space].ends_with('Z') => &text[space + 1..],
        _ => text,
    }
}

fn clip(text: &str) -> String {
    match text.char_indices().nth(MAX_LINE_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// The log as rows: the message of each line, a heading when a new step
/// starts, and GitHub's error and warning markers as colours.
pub(crate) fn parse(raw: &str) -> Vec<LogLine> {
    let mut lines = Vec::new();
    let mut step = String::new();
    for line in raw.lines() {
        let line = strip_ansi(line.trim_end_matches('\r'));
        // JOB \t STEP \t TIMESTAMP MESSAGE
        let mut parts = line.splitn(3, '\t');
        let (message, this_step) = match (parts.next(), parts.next(), parts.next()) {
            (Some(job), Some(step_name), Some(rest)) => (strip_timestamp(rest).to_string(), Some(format!("{job} › {step_name}"))),
            _ => (strip_timestamp(&line).to_string(), None),
        };
        if let Some(this_step) = this_step.filter(|this_step| *this_step != step) {
            lines.push(LogLine {
                text: this_step.clone(),
                kind: LineKind::Step,
            });
            step = this_step;
        }
        let (text, kind) = if let Some(rest) = message.strip_prefix("##[error]") {
            (rest.to_string(), LineKind::Error)
        } else if let Some(rest) = message.strip_prefix("##[warning]") {
            (rest.to_string(), LineKind::Warning)
        } else if let Some(rest) = message.strip_prefix("##[group]") {
            (rest.to_string(), LineKind::Plain)
        } else if message.starts_with("##[endgroup]") {
            continue;
        } else {
            (message, LineKind::Plain)
        };
        lines.push(LogLine {
            text: clip(&text),
            kind,
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_log_reads_as_messages_under_step_headings() {
        let raw = "build\tRun tests\t2026-10-31T12:00:00.1234567Z \u{1b}[31mrunning 3 tests\u{1b}[0m\n\
                   build\tRun tests\t2026-10-31T12:00:01.0000000Z ##[error]test login ... FAILED\n\
                   build\tRun tests\t2026-10-31T12:00:01.5000000Z ##[endgroup]\n\
                   build\tLint\t2026-10-31T12:00:02.0000000Z ##[warning]unused import\n";
        let lines = parse(raw);
        assert_eq!(
            lines,
            vec![
                LogLine { text: "build › Run tests".into(), kind: LineKind::Step },
                LogLine { text: "running 3 tests".into(), kind: LineKind::Plain },
                LogLine { text: "test login ... FAILED".into(), kind: LineKind::Error },
                LogLine { text: "build › Lint".into(), kind: LineKind::Step },
                LogLine { text: "unused import".into(), kind: LineKind::Warning },
            ]
        );
    }

    #[test]
    fn a_long_log_keeps_its_end_and_long_lines_are_cut() {
        let raw: String = (0..6_000).map(|n| format!("line {n}\n")).collect();
        let mut log = CheckLog::loading(7);
        log.fill(raw.clone());
        assert_eq!(log.lines.len(), MAX_LINES);
        assert_eq!(log.skipped, 1_000);
        assert_eq!(log.lines.last().unwrap().text, "line 5999");
        assert_eq!(log.raw, raw, "Copy gets the whole log");
        let long = parse(&"x".repeat(1_000));
        assert_eq!(long[0].text.chars().count(), MAX_LINE_CHARS + 1);
        assert!(long[0].text.ends_with('…'));
    }

    #[test]
    fn plain_text_without_columns_passes_through() {
        assert_eq!(parse("error: something broke")[0].text, "error: something broke");
        assert_eq!(strip_timestamp("2026-10-31T12:00:00Z ok"), "ok");
        assert_eq!(strip_timestamp("not a time"), "not a time");
    }
}
