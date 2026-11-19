use std::path::PathBuf;

use crate::console::escape_parse::ColoredLine;

pub const BLOCK_CONTEXT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const TRUNCATION_SENTINEL: &str = "\n… [output truncated]";

/// Immutable snapshot of a block at completion time — the AI substrate unit.
#[derive(Clone, Debug)]
pub struct BlockContext {
    pub block_id: usize,
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    /// Complete plain text block output (no ANSI).
    pub output_text: String,
    /// Number of output lines in the complete snapshot.
    pub output_line_count: usize,
    /// Kept for wire compatibility; complete block snapshots are never truncated.
    pub output_truncated: bool,
    /// Derived: true when exit_code is Some and non-zero.
    pub failed: bool,
}

impl BlockContext {
    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn success(&self) -> bool {
        !self.failed && self.exit_code == Some(0)
    }
}

/// Exit codes 128+N mean the process was killed by Unix signal N.
/// These are user-initiated interruptions (Ctrl+C = 130, SIGTERM = 143, SIGKILL = 137),
/// not genuine command failures, and should not be treated as errors.
pub fn is_signal_exit(code: i32) -> bool {
    code > 128 && code <= 192
}

pub fn failed_from_exit_code(exit_code: Option<i32>) -> bool {
    matches!(exit_code, Some(n) if n != 0 && !is_signal_exit(n))
}

/// Returns true if `command` is known to exit 1 to signal a *successful but noteworthy*
/// result rather than an actual error.
///
/// Examples: `grep` exits 1 when there are no matches; `yarn outdated` exits 1 when
/// packages are outdated; `diff` exits 1 when files differ.  These are not failures.
pub fn is_informational_exit(command: &str, exit_code: i32) -> bool {
    // Conventionally only exit code 1 carries informational meaning.
    // Any higher non-signal code is almost always a real error.
    if exit_code != 1 {
        return false;
    }

    let mut tokens = command.split_whitespace();
    let prog = match tokens.next() {
        Some(p) => p.rsplit('/').next().unwrap_or(p),
        None => return false,
    };
    let sub = tokens.next().unwrap_or("");

    match prog {
        // Grep family: exit 1 = no matches found
        "grep" | "egrep" | "fgrep" | "zgrep" | "rg" | "ag" => true,

        // Diff tools: exit 1 = files differ
        "diff" | "diff3" | "cmp" | "colordiff" => true,

        // POSIX test builtins: exit 1 = condition is false
        "test" | "[" => true,

        // Package managers: `outdated` and `audit` exit 1 when results exist,
        // not because something went wrong with the command itself.
        "yarn" | "npm" | "pnpm" | "bun" => {
            matches!(sub, "outdated" | "audit")
        }

        // git grep: exit 1 = no matches (same as grep)
        "git" => sub == "grep",

        // Ruby Bundler
        "bundle" | "bundler" => matches!(sub, "outdated" | "audit"),

        _ => false,
    }
}

/// Join `ColoredLine` spans into plain text, one line per row, `\n` separated.
/// Trailing blank lines are stripped.
pub fn colored_lines_to_plain_text(lines: &[ColoredLine]) -> String {
    let mut text_lines: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect())
        .collect();
    while text_lines
        .last()
        .map(|l: &String| l.trim().is_empty())
        .unwrap_or(false)
    {
        text_lines.pop();
    }
    text_lines.join("\n")
}

/// Apply byte limit; set truncated flag.
/// Truncation preserves valid UTF-8 by snapping to a char boundary.
pub fn truncate_output_text(text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str(TRUNCATION_SENTINEL);
    (out, true)
}

/// Build a `BlockContext` from a completed block's metadata and full snapshotted lines.
pub fn block_context_from_lines(
    block_id: usize,
    command: String,
    cwd: PathBuf,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    lines: &[ColoredLine],
) -> BlockContext {
    let output_line_count = lines.len();
    let plain = colored_lines_to_plain_text(lines);
    let output_text = plain;
    let failed = failed_from_exit_code(exit_code)
        && !exit_code.is_some_and(|c| is_informational_exit(&command, c));
    BlockContext {
        block_id,
        command,
        cwd,
        exit_code,
        duration_ms,
        output_text,
        output_line_count,
        output_truncated: false,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::escape_parse::{ColoredLine, TextSpan};

    fn line(text: &str) -> ColoredLine {
        ColoredLine {
            wrapped: false,
            spans: vec![TextSpan {
                text: text.to_string(),
                color: None,
                bg: None,
                flags: 0,
            }],
        }
    }

    #[test]
    fn plain_text_joins_lines() {
        let lines = vec![line("line1"), line("line2")];
        assert_eq!(colored_lines_to_plain_text(&lines), "line1\nline2");
    }

    #[test]
    fn plain_text_strips_trailing_blank_lines() {
        let lines = vec![line("a"), line(""), line("")];
        assert_eq!(colored_lines_to_plain_text(&lines), "a");
    }

    #[test]
    fn plain_text_empty_input() {
        assert_eq!(colored_lines_to_plain_text(&[]), "");
    }

    #[test]
    fn truncate_within_limit() {
        let s = "hello".to_string();
        let (out, truncated) = truncate_output_text(s.clone(), 100);
        assert_eq!(out, s);
        assert!(!truncated);
    }

    #[test]
    fn truncate_over_limit_sets_flag_and_sentinel() {
        let s = "x".repeat(100 * 1024);
        let (out, truncated) = truncate_output_text(s, BLOCK_CONTEXT_MAX_OUTPUT_BYTES);
        assert!(truncated);
        assert!(out.len() <= BLOCK_CONTEXT_MAX_OUTPUT_BYTES + TRUNCATION_SENTINEL.len());
        assert!(out.ends_with(TRUNCATION_SENTINEL));
    }

    #[test]
    fn truncate_preserves_utf8() {
        // "你" is 3 bytes; put it just past the boundary.
        let base = "a".repeat(BLOCK_CONTEXT_MAX_OUTPUT_BYTES - 1);
        let s = base + "你";
        let (out, truncated) = truncate_output_text(s, BLOCK_CONTEXT_MAX_OUTPUT_BYTES);
        assert!(truncated);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn failed_exit_codes() {
        assert!(!failed_from_exit_code(Some(0)));
        assert!(failed_from_exit_code(Some(1)));
        assert!(failed_from_exit_code(Some(127)));
        assert!(!failed_from_exit_code(None));
        // Signal exits (Ctrl+C = 130, SIGTERM = 143, SIGKILL = 137, SIGTSTP = 146) are not failures.
        assert!(!failed_from_exit_code(Some(130)));
        assert!(!failed_from_exit_code(Some(143)));
        assert!(!failed_from_exit_code(Some(137)));
        assert!(!failed_from_exit_code(Some(146)));
    }

    #[test]
    fn signal_exit_boundaries() {
        assert!(!is_signal_exit(0));
        assert!(!is_signal_exit(1));
        assert!(!is_signal_exit(128));
        assert!(is_signal_exit(129));
        assert!(is_signal_exit(130));
        assert!(is_signal_exit(192));
        assert!(!is_signal_exit(193));
    }

    #[test]
    fn informational_exits_not_treated_as_failure() {
        // grep: exit 1 = no matches
        assert!(is_informational_exit("grep foo bar.txt", 1));
        assert!(is_informational_exit("rg pattern", 1));
        assert!(!is_informational_exit("grep foo", 2)); // exit 2 is a real error

        // diff: exit 1 = files differ
        assert!(is_informational_exit("diff a.txt b.txt", 1));
        assert!(is_informational_exit("cmp a b", 1));

        // yarn/npm/pnpm outdated and audit
        assert!(is_informational_exit("yarn outdated", 1));
        assert!(is_informational_exit("npm outdated", 1));
        assert!(is_informational_exit("pnpm outdated", 1));
        assert!(is_informational_exit("bun outdated", 1));
        assert!(is_informational_exit("yarn audit", 1));
        assert!(is_informational_exit("npm audit", 1));

        // yarn install or run are real failures at exit 1
        assert!(!is_informational_exit("yarn install", 1));
        assert!(!is_informational_exit("npm run build", 1));

        // git grep: exit 1 = no matches
        assert!(is_informational_exit("git grep pattern", 1));
        // git diff without --exit-code always exits 0; exit 1 would be unusual → not informational
        assert!(!is_informational_exit("git diff HEAD", 1));

        // test builtin
        assert!(is_informational_exit("test -f file.txt", 1));
        assert!(is_informational_exit("[ -z \"$VAR\" ]", 1));

        // bundle outdated
        assert!(is_informational_exit("bundle outdated", 1));
    }

    #[test]
    fn block_context_yarn_outdated_not_failed() {
        let ctx = block_context_from_lines(
            1,
            "yarn outdated".to_string(),
            PathBuf::new(),
            Some(1),
            None,
            &[],
        );
        assert!(!ctx.failed, "yarn outdated exit 1 should not be a failure");
    }

    #[test]
    fn block_context_real_failure_still_detected() {
        let ctx = block_context_from_lines(
            1,
            "yarn install".to_string(),
            PathBuf::new(),
            Some(1),
            None,
            &[],
        );
        assert!(ctx.failed, "yarn install exit 1 should be a failure");
    }

    #[test]
    fn block_context_from_lines_fields() {
        let lines = vec![line("output")];
        let ctx = block_context_from_lines(
            42,
            "echo hi".to_string(),
            PathBuf::from("/home/user"),
            Some(0),
            Some(100),
            &lines,
        );
        assert_eq!(ctx.block_id, 42);
        assert_eq!(ctx.command, "echo hi");
        assert_eq!(ctx.cwd, PathBuf::from("/home/user"));
        assert_eq!(ctx.exit_code, Some(0));
        assert_eq!(ctx.duration_ms, Some(100));
        assert_eq!(ctx.output_text, "output");
        assert_eq!(ctx.output_line_count, 1);
        assert!(!ctx.output_truncated);
        assert!(!ctx.failed);
    }

    #[test]
    fn block_context_failed_flag() {
        let ctx = block_context_from_lines(1, String::new(), PathBuf::new(), Some(1), None, &[]);
        assert!(ctx.failed);
        assert!(!ctx.success());
    }
}
