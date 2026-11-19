//! Shell lexer: converts a raw source string into a flat sequence of tokens.
//!
//! Architecture inspired by nushell's nu-parser lexer (MIT license):
//! https://github.com/nushell/nushell/blob/main/crates/nu-parser/src/lex.rs
//!
//! The lexer is intentionally simple — it does not resolve variables or expand
//! globs. Its job is to identify token boundaries so that `lite_parse` can
//! assemble commands without re-scanning the source.

use crate::span::{Span, Spanned, SpannedItem};

/// A single lexeme produced by the shell lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// An unquoted word, flag, or other literal token (e.g. `ls`, `-la`, `$VAR`).
    Bare,
    /// A single-quoted string `'...'`.  The span covers the outer quotes.
    SingleQuoted,
    /// A double-quoted string `"..."`.  The span covers the outer quotes.
    DoubleQuoted,
    /// A backtick-quoted string `` `...` ``.  The span covers the backticks.
    BacktickQuoted,
    /// `|` — pipe between commands in a pipeline.
    Pipe,
    /// `||` — logical OR between pipelines.
    LogicalOr,
    /// `&&` — logical AND between pipelines.
    LogicalAnd,
    /// `;` — explicit command separator.
    Semicolon,
    /// `\n` (or `\r\n`) acting as a command separator.
    Newline,
    /// Horizontal whitespace (spaces, tabs) between tokens.
    Whitespace,
    /// `#` to end-of-line — a comment, including the `#`.
    Comment,
    /// `&` appearing as a standalone background operator.
    Ampersand,
    /// `>` — redirect stdout to a file.
    RedirectOut,
    /// `>>` — append stdout to a file.
    RedirectAppend,
    /// `<` — redirect a file to stdin.
    RedirectIn,
    /// `2>` — redirect stderr to a file.
    RedirectErr,
    /// `2>>` — append stderr to a file.
    RedirectErrAppend,
    /// `&>` — redirect both stdout and stderr to a file.
    RedirectBoth,
    /// `$(` — opens a command substitution.
    CommandSubstOpen,
    /// `(` — opens a subshell group.
    OpenParen,
    /// `)` — closes a command substitution or subshell group.
    CloseParen,
}

impl Token {
    /// True if this token carries meaningful command content (not whitespace,
    /// comments, or newlines).
    pub fn is_content(&self) -> bool {
        !matches!(self, Token::Whitespace | Token::Comment | Token::Newline)
    }

    /// True if this token ends a pipeline or command sequence.
    pub fn is_separator(&self) -> bool {
        matches!(
            self,
            Token::Pipe
                | Token::LogicalOr
                | Token::LogicalAnd
                | Token::Semicolon
                | Token::Newline
                | Token::Ampersand
        )
    }
}

/// Lex `input` into a flat list of `Spanned<Token>`s.
///
/// Every byte of `input` is covered by exactly one token.  The spans use byte
/// offsets relative to `input`, so `span.slice(input)` always recovers the
/// original text.
pub fn lex(input: &str) -> Vec<Spanned<Token>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        let start = i;

        match bytes[i] {
            // ── Whitespace ────────────────────────────────────────────────────
            b' ' | b'\t' => {
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                tokens.push(Token::Whitespace.spanned(Span::new(start, i)));
            }

            // ── Newlines ──────────────────────────────────────────────────────
            b'\n' | b'\r' => {
                i += 1;
                if bytes[start] == b'\r' && i < len && bytes[i] == b'\n' {
                    i += 1; // consume \r\n as a single Newline
                }
                tokens.push(Token::Newline.spanned(Span::new(start, i)));
            }

            // ── Comments ──────────────────────────────────────────────────────
            b'#' => {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                tokens.push(Token::Comment.spanned(Span::new(start, i)));
            }

            // ── Pipe or logical OR ────────────────────────────────────────────
            b'|' => {
                i += 1;
                if i < len && bytes[i] == b'|' {
                    i += 1;
                    tokens.push(Token::LogicalOr.spanned(Span::new(start, i)));
                } else {
                    tokens.push(Token::Pipe.spanned(Span::new(start, i)));
                }
            }

            // ── Ampersand, logical AND, or &> ─────────────────────────────────
            b'&' => {
                i += 1;
                if i < len && bytes[i] == b'&' {
                    i += 1;
                    tokens.push(Token::LogicalAnd.spanned(Span::new(start, i)));
                } else if i < len && bytes[i] == b'>' {
                    i += 1;
                    tokens.push(Token::RedirectBoth.spanned(Span::new(start, i)));
                } else {
                    tokens.push(Token::Ampersand.spanned(Span::new(start, i)));
                }
            }

            // ── Semicolon ─────────────────────────────────────────────────────
            b';' => {
                i += 1;
                tokens.push(Token::Semicolon.spanned(Span::new(start, i)));
            }

            // ── Stdout redirect ───────────────────────────────────────────────
            b'>' => {
                i += 1;
                if i < len && bytes[i] == b'>' {
                    i += 1;
                    tokens.push(Token::RedirectAppend.spanned(Span::new(start, i)));
                } else {
                    tokens.push(Token::RedirectOut.spanned(Span::new(start, i)));
                }
            }

            // ── Stdin redirect ────────────────────────────────────────────────
            b'<' => {
                i += 1;
                tokens.push(Token::RedirectIn.spanned(Span::new(start, i)));
            }

            // ── Stderr redirect: 2> or 2>> ────────────────────────────────────
            // Only matches when `2` is the start of a new token and the next
            // character is `>`.  If `2` appears mid-word the bare-word branch
            // below handles it (terminating on the subsequent `>`).
            b'2' if i + 1 < len && bytes[i + 1] == b'>' => {
                i += 2;
                if i < len && bytes[i] == b'>' {
                    i += 1;
                    tokens.push(Token::RedirectErrAppend.spanned(Span::new(start, i)));
                } else {
                    tokens.push(Token::RedirectErr.spanned(Span::new(start, i)));
                }
            }

            // ── Single-quoted string ──────────────────────────────────────────
            b'\'' => {
                i += 1;
                while i < len && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 1; // skip escaped character
                    }
                    i += 1;
                }
                if i < len {
                    i += 1; // consume closing quote
                }
                tokens.push(Token::SingleQuoted.spanned(Span::new(start, i)));
            }

            // ── Double-quoted string ──────────────────────────────────────────
            b'"' => {
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                tokens.push(Token::DoubleQuoted.spanned(Span::new(start, i)));
            }

            // ── Backtick-quoted string ────────────────────────────────────────
            b'`' => {
                i += 1;
                while i < len && bytes[i] != b'`' {
                    if bytes[i] == b'\\' && i + 1 < len {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                tokens.push(Token::BacktickQuoted.spanned(Span::new(start, i)));
            }

            // ── Command substitution $( ───────────────────────────────────────
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => {
                i += 2;
                tokens.push(Token::CommandSubstOpen.spanned(Span::new(start, i)));
            }

            // ── Parentheses ───────────────────────────────────────────────────
            b'(' => {
                i += 1;
                tokens.push(Token::OpenParen.spanned(Span::new(start, i)));
            }
            b')' => {
                i += 1;
                tokens.push(Token::CloseParen.spanned(Span::new(start, i)));
            }

            // ── Line continuation: backslash + newline ────────────────────────
            b'\\' if i + 1 < len && bytes[i + 1] == b'\n' => {
                i += 2;
                tokens.push(Token::Whitespace.spanned(Span::new(start, i)));
            }

            // ── Bare word ─────────────────────────────────────────────────────
            // Runs until any of the characters that start a new token type.
            _ => {
                while i < len {
                    match bytes[i] {
                        b' ' | b'\t' | b'\n' | b'\r' | b'|' | b'&' | b';' | b'<' | b'>' | b'\''
                        | b'"' | b'`' | b'(' | b')' | b'#' => break,
                        // `$(` starts command substitution — stop before the `$`
                        b'$' if i + 1 < len && bytes[i + 1] == b'(' => break,
                        // Backslash escapes the next character — consume both as bare
                        b'\\' if i + 1 < len => {
                            i += 2;
                        }
                        _ => i += 1,
                    }
                }
                tokens.push(Token::Bare.spanned(Span::new(start, i)));
            }
        }
    }

    tokens
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Vec<Token> {
        lex(input).into_iter().map(|t| t.item).collect()
    }

    #[test]
    fn lex_simple_command() {
        assert_eq!(
            kinds("ls -la"),
            vec![Token::Bare, Token::Whitespace, Token::Bare]
        );
    }

    #[test]
    fn lex_bare_spans_are_correct() {
        let input = "ls -la";
        let toks = lex(input);
        assert_eq!(toks[0].span.slice(input), "ls");
        assert_eq!(toks[2].span.slice(input), "-la");
    }

    #[test]
    fn lex_pipe() {
        assert_eq!(
            kinds("ls | grep"),
            vec![
                Token::Bare,
                Token::Whitespace,
                Token::Pipe,
                Token::Whitespace,
                Token::Bare,
            ]
        );
    }

    #[test]
    fn lex_logical_operators() {
        assert_eq!(
            kinds("a && b || c"),
            vec![
                Token::Bare,
                Token::Whitespace,
                Token::LogicalAnd,
                Token::Whitespace,
                Token::Bare,
                Token::Whitespace,
                Token::LogicalOr,
                Token::Whitespace,
                Token::Bare,
            ]
        );
    }

    #[test]
    fn lex_single_quoted_includes_quotes() {
        let input = "echo 'hello world'";
        let toks = lex(input);
        let quoted = toks.iter().find(|t| t.item == Token::SingleQuoted).unwrap();
        assert_eq!(quoted.span.slice(input), "'hello world'");
    }

    #[test]
    fn lex_double_quoted_includes_quotes() {
        let input = r#"echo "hello world""#;
        let toks = lex(input);
        let quoted = toks.iter().find(|t| t.item == Token::DoubleQuoted).unwrap();
        assert_eq!(quoted.span.slice(input), "\"hello world\"");
    }

    #[test]
    fn lex_redirect_out() {
        assert_eq!(
            kinds("cmd > file"),
            vec![
                Token::Bare,
                Token::Whitespace,
                Token::RedirectOut,
                Token::Whitespace,
                Token::Bare,
            ]
        );
    }

    #[test]
    fn lex_redirect_append() {
        assert!(kinds("cmd >> file").contains(&Token::RedirectAppend));
    }

    #[test]
    fn lex_redirect_stderr() {
        assert!(kinds("cmd 2> err.log").contains(&Token::RedirectErr));
    }

    #[test]
    fn lex_redirect_stderr_append() {
        assert!(kinds("cmd 2>> err.log").contains(&Token::RedirectErrAppend));
    }

    #[test]
    fn lex_redirect_both() {
        assert!(kinds("cmd &> all.log").contains(&Token::RedirectBoth));
    }

    #[test]
    fn lex_comment() {
        assert!(kinds("ls # list files").contains(&Token::Comment));
    }

    #[test]
    fn lex_command_subst() {
        assert!(kinds("echo $(date)").contains(&Token::CommandSubstOpen));
    }

    #[test]
    fn lex_ampersand_background() {
        assert!(kinds("sleep 10 &").contains(&Token::Ampersand));
    }

    #[test]
    fn lex_semicolon() {
        assert!(kinds("cd /tmp; ls").contains(&Token::Semicolon));
    }

    #[test]
    fn lex_newline() {
        assert!(kinds("echo hello\necho world").contains(&Token::Newline));
    }

    #[test]
    fn lex_crlf_is_single_newline() {
        let kinds = kinds("a\r\nb");
        assert_eq!(kinds.iter().filter(|t| **t == Token::Newline).count(), 1);
    }

    #[test]
    fn lex_line_continuation_is_whitespace() {
        let kinds = kinds("echo \\\nhello");
        assert!(!kinds.contains(&Token::Newline));
        assert!(kinds.contains(&Token::Whitespace));
    }

    #[test]
    fn lex_backtick_quoted() {
        let input = "echo `date`";
        let toks = lex(input);
        assert!(toks.iter().any(|t| t.item == Token::BacktickQuoted));
    }

    #[test]
    fn lex_number_not_treated_as_redirect() {
        // "200" should be a Bare word, not a redirect (200 is not followed by >)
        assert_eq!(
            kinds("echo 200"),
            vec![Token::Bare, Token::Whitespace, Token::Bare]
        );
    }

    #[test]
    fn lex_2_space_redirect_is_separate_tokens() {
        // "2 > file" should be Bare("2"), WS, RedirectOut, WS, Bare("file")
        let k = kinds("2 > file");
        assert_eq!(k[0], Token::Bare);
        assert_eq!(k[2], Token::RedirectOut);
    }

    #[test]
    fn every_byte_is_covered() {
        let input = "ls -la /tmp | grep foo 2>&1";
        let toks = lex(input);
        // All byte ranges should be contiguous and non-overlapping
        let mut pos = 0usize;
        for tok in &toks {
            assert_eq!(tok.span.start, pos, "gap before {tok:?}");
            pos = tok.span.end;
        }
        assert_eq!(pos, input.len(), "trailing bytes not covered");
    }
}
