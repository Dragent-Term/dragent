//! Lite parser: assembles lexer tokens into a lightweight shell AST.
//!
//! Architecture inspired by nushell's lite-parser (MIT license):
//! https://github.com/nushell/nushell/blob/main/crates/nu-parser/src/lite_parser.rs
//!
//! The output is intentionally minimal — a tree of `LiteCommand`s,
//! `LitePipeline`s, and `LiteGroup`s that preserves source spans but performs
//! no semantic analysis. The next stage (`classify`) adds command-name lookup.

use crate::lex::{lex, Token};
use crate::span::{HasSpan, Span, Spanned, SpannedItem};

// ── Lite AST ──────────────────────────────────────────────────────────────────

/// A simple command: one or more whitespace-separated words, each with its
/// source span.
///
/// Redirects and operators that terminate the command are NOT included in
/// `parts`; they become structure at the `LitePipeline`/`LiteGroup` level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiteCommand {
    /// The individual words (bare, quoted, or redirect targets).
    pub parts: Vec<Spanned<String>>,
    /// Trailing whitespace after the last part, if any (useful for completion).
    pub trailing_whitespace: Option<Span>,
}

impl LiteCommand {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Reconstruct the command text by space-joining all parts (lossy —
    /// quoting is preserved but inter-token whitespace is normalised).
    pub fn joined_by_space(&self) -> String {
        self.parts
            .iter()
            .map(|p| p.item.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl HasSpan for LiteCommand {
    fn span(&self) -> Span {
        Span::from_list(&self.parts.iter().map(|p| p.span).collect::<Vec<_>>())
    }
}

/// A sequence of commands connected by `|`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LitePipeline {
    pub commands: Vec<LiteCommand>,
}

impl LitePipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl HasSpan for LitePipeline {
    fn span(&self) -> Span {
        Span::from_list(&self.commands.iter().map(|c| c.span()).collect::<Vec<_>>())
    }
}

/// A group of pipelines separated by `;`, `&&`, `||`, newlines, or `&`.
///
/// Corresponds to nushell's `LiteBlock` — a top-level collection of pipelines
/// that were separated at the statement level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiteGroup {
    pub pipelines: Vec<LitePipeline>,
}

impl LiteGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.pipelines.is_empty()
    }
}

impl HasSpan for LiteGroup {
    fn span(&self) -> Span {
        Span::from_list(&self.pipelines.iter().map(|p| p.span()).collect::<Vec<_>>())
    }
}

// ── Parse error ───────────────────────────────────────────────────────────────

/// A non-fatal parse error collected during lite parsing.
///
/// The lite parser never aborts — it produces the best-effort tree and
/// accumulates errors for callers to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl ParseError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Lite-parse a shell input string into a `LiteGroup`.
///
/// Returns the group and any non-fatal errors encountered.  The caller should
/// render errors as warnings — the returned group always represents a complete
/// best-effort parse.
pub fn lite_parse(input: &str) -> (LiteGroup, Vec<ParseError>) {
    let tokens = lex(input);
    assemble(input, &tokens)
}

// ── Internal assembly ─────────────────────────────────────────────────────────

fn assemble(input: &str, tokens: &[Spanned<Token>]) -> (LiteGroup, Vec<ParseError>) {
    let mut group = LiteGroup::new();
    let errors: Vec<ParseError> = Vec::new();

    let mut current_pipeline = LitePipeline::new();
    let mut current_command = LiteCommand::new();

    for tok in tokens {
        match &tok.item {
            // ── Whitespace: update trailing-whitespace hint, don't push a part ─
            Token::Whitespace => {
                current_command.trailing_whitespace = Some(tok.span);
            }

            // ── Newlines and comments end the current command + pipeline ───────
            Token::Comment | Token::Newline => {
                flush_command(&mut current_command, &mut current_pipeline);
                flush_pipeline(&mut current_pipeline, &mut group);
            }

            // ── Statement separators end the current command + pipeline ─────────
            Token::Semicolon | Token::LogicalAnd | Token::LogicalOr | Token::Ampersand => {
                flush_command(&mut current_command, &mut current_pipeline);
                flush_pipeline(&mut current_pipeline, &mut group);
            }

            // ── Pipe ends the current command, stays within the pipeline ────────
            Token::Pipe => {
                flush_command(&mut current_command, &mut current_pipeline);
            }

            // ── Anything content-bearing becomes a command part ─────────────────
            Token::Bare
            | Token::SingleQuoted
            | Token::DoubleQuoted
            | Token::BacktickQuoted
            | Token::CommandSubstOpen
            | Token::OpenParen
            | Token::CloseParen
            | Token::RedirectOut
            | Token::RedirectAppend
            | Token::RedirectIn
            | Token::RedirectErr
            | Token::RedirectErrAppend
            | Token::RedirectBoth => {
                current_command.trailing_whitespace = None;
                let text = tok.span.slice(input).to_string();
                current_command.parts.push(text.spanned(tok.span));
            }
        }
    }

    // Flush whatever is left in the buffers.
    flush_command(&mut current_command, &mut current_pipeline);
    flush_pipeline(&mut current_pipeline, &mut group);

    (group, errors)
}

#[inline]
fn flush_command(cmd: &mut LiteCommand, pipeline: &mut LitePipeline) {
    if !cmd.is_empty() {
        pipeline.commands.push(std::mem::take(cmd));
    }
}

#[inline]
fn flush_pipeline(pipeline: &mut LitePipeline, group: &mut LiteGroup) {
    if !pipeline.is_empty() {
        group.pipelines.push(std::mem::take(pipeline));
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_command() {
        let (group, errs) = lite_parse("ls -la /tmp");
        assert!(errs.is_empty());
        assert_eq!(group.pipelines.len(), 1);
        let cmd = &group.pipelines[0].commands[0];
        assert_eq!(cmd.parts.len(), 3);
        assert_eq!(cmd.parts[0].item, "ls");
        assert_eq!(cmd.parts[1].item, "-la");
        assert_eq!(cmd.parts[2].item, "/tmp");
    }

    #[test]
    fn parse_pipeline_two_commands() {
        let (group, _) = lite_parse("ls | grep foo");
        assert_eq!(group.pipelines.len(), 1);
        let pipeline = &group.pipelines[0];
        assert_eq!(pipeline.commands.len(), 2);
        assert_eq!(pipeline.commands[0].parts[0].item, "ls");
        assert_eq!(pipeline.commands[1].parts[0].item, "grep");
        assert_eq!(pipeline.commands[1].parts[1].item, "foo");
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let (group, _) = lite_parse("cat file | sort | uniq -c");
        assert_eq!(group.pipelines[0].commands.len(), 3);
    }

    #[test]
    fn parse_semicolon_creates_separate_pipelines() {
        let (group, _) = lite_parse("cd /tmp; ls");
        assert_eq!(group.pipelines.len(), 2);
        assert_eq!(group.pipelines[0].commands[0].parts[0].item, "cd");
        assert_eq!(group.pipelines[1].commands[0].parts[0].item, "ls");
    }

    #[test]
    fn parse_logical_and_creates_separate_pipelines() {
        let (group, _) = lite_parse("mkdir dir && cd dir");
        assert_eq!(group.pipelines.len(), 2);
    }

    #[test]
    fn parse_logical_or_creates_separate_pipelines() {
        let (group, _) = lite_parse("which foo || echo missing");
        assert_eq!(group.pipelines.len(), 2);
    }

    #[test]
    fn parse_newline_creates_separate_pipelines() {
        let (group, _) = lite_parse("echo hello\necho world");
        assert_eq!(group.pipelines.len(), 2);
    }

    #[test]
    fn parse_quoted_string_is_single_part() {
        let (group, _) = lite_parse("echo 'hello world'");
        let cmd = &group.pipelines[0].commands[0];
        assert_eq!(cmd.parts.len(), 2);
        assert_eq!(cmd.parts[1].item, "'hello world'");
    }

    #[test]
    fn parse_double_quoted_is_single_part() {
        let (group, _) = lite_parse(r#"echo "hello world""#);
        let cmd = &group.pipelines[0].commands[0];
        assert_eq!(cmd.parts.len(), 2);
        assert_eq!(cmd.parts[1].item, "\"hello world\"");
    }

    #[test]
    fn parse_empty_input_is_empty_group() {
        let (group, errs) = lite_parse("");
        assert!(group.is_empty());
        assert!(errs.is_empty());
    }

    #[test]
    fn parse_whitespace_only_is_empty_group() {
        let (group, _) = lite_parse("   \t  ");
        assert!(group.is_empty());
    }

    #[test]
    fn parse_comment_only_is_empty_group() {
        let (group, _) = lite_parse("# just a comment");
        assert!(group.is_empty());
    }

    #[test]
    fn spans_point_into_source() {
        let input = "ls -la";
        let (group, _) = lite_parse(input);
        let parts = &group.pipelines[0].commands[0].parts;
        assert_eq!(parts[0].span.slice(input), "ls");
        assert_eq!(parts[1].span.slice(input), "-la");
    }

    #[test]
    fn trailing_whitespace_is_tracked() {
        let (group, _) = lite_parse("ls ");
        let cmd = &group.pipelines[0].commands[0];
        assert!(cmd.trailing_whitespace.is_some());
    }

    #[test]
    fn no_trailing_whitespace_when_cursor_at_word_end() {
        let (group, _) = lite_parse("ls");
        let cmd = &group.pipelines[0].commands[0];
        assert!(cmd.trailing_whitespace.is_none());
    }

    #[test]
    fn joined_by_space_reconstructs_command() {
        let (group, _) = lite_parse("git commit -m 'fix bug'");
        let joined = group.pipelines[0].commands[0].joined_by_space();
        assert_eq!(joined, "git commit -m 'fix bug'");
    }

    #[test]
    fn background_operator_separates_pipelines() {
        let (group, _) = lite_parse("sleep 10 &");
        assert_eq!(group.pipelines.len(), 1);
        assert_eq!(group.pipelines[0].commands[0].parts[0].item, "sleep");
    }

    #[test]
    fn redirect_is_included_as_part() {
        let (group, _) = lite_parse("ls > out.txt");
        let parts = &group.pipelines[0].commands[0].parts;
        assert!(parts.iter().any(|p| p.item == ">"));
        assert!(parts.iter().any(|p| p.item == "out.txt"));
    }

    #[test]
    fn multiline_script() {
        let input = "git add .\ngit commit -m 'fix'\ngit push";
        let (group, _) = lite_parse(input);
        assert_eq!(group.pipelines.len(), 3);
        assert_eq!(group.pipelines[0].commands[0].parts[0].item, "git");
        // "git commit -m 'fix'" → parts: [git, commit, -m, 'fix']
        assert_eq!(group.pipelines[1].commands[0].parts[3].item, "'fix'");
    }

    #[test]
    fn has_span_covers_parts() {
        let input = "ls -la";
        let (group, _) = lite_parse(input);
        let cmd_span = group.pipelines[0].commands[0].span();
        assert_eq!(cmd_span.start, 0);
        assert_eq!(cmd_span.end, input.len());
    }
}
