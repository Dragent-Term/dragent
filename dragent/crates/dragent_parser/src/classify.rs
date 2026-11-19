//! Command classification: enriches a `LiteGroup` with semantic command kinds.
//!
//! Architecture inspired by nushell's nu-parser (MIT license):
//! https://github.com/nushell/nushell/blob/main/crates/nu-parser/src/parser.rs
//!
//! Takes a `LiteGroup` (output of `lite_parse`) and a `CommandRegistry`,
//! and produces a `ClassifiedBlock` where every command name is tagged as
//! `Known`, `External`, or `EnvAssignment`.

use crate::lite_parse::{LiteCommand, LiteGroup};
use crate::span::{Span, Spanned, SpannedItem};

// ── Output types ──────────────────────────────────────────────────────────────

/// The semantic kind of a command name after registry lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    /// Found in the `CommandRegistry` (shell built-in or known external).
    Known,
    /// Not in the registry — will be searched for on `$PATH` at runtime.
    External,
    /// The "command" consists solely of `KEY=VALUE` env-var assignments with
    /// no trailing command name.
    EnvAssignment,
}

/// A single command enriched with source spans and a semantic kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedCommand {
    /// The command name with its source location.
    pub name: Spanned<String>,
    /// All arguments (everything after the command name, in source order).
    pub args: Vec<Spanned<String>>,
    /// `KEY=VALUE` env-var assignments that precede the command name.
    pub env_vars: Vec<Spanned<String>>,
    /// Whether the name was found in the registry.
    pub kind: CommandKind,
}

impl ClassifiedCommand {
    /// Smallest span that covers the command name and all its arguments.
    pub fn span(&self) -> Span {
        let mut spans = vec![self.name.span];
        spans.extend(self.args.iter().map(|a| a.span));
        spans.extend(self.env_vars.iter().map(|e| e.span));
        Span::from_list(&spans)
    }
}

/// A pipeline of classified commands connected by `|`.
#[derive(Debug, Clone)]
pub struct ClassifiedPipeline {
    pub commands: Vec<ClassifiedCommand>,
}

/// A block of classified pipelines (the top-level output of `classify`).
#[derive(Debug, Clone)]
pub struct ClassifiedBlock {
    pub pipelines: Vec<ClassifiedPipeline>,
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// A registry of known command names (built-ins, aliases, loaded completions).
///
/// Implement this trait to supply your own command database. The `classify`
/// function uses it only for a name lookup — the registry is not mutated.
pub trait CommandRegistry {
    fn has_command(&self, name: &str) -> bool;
}

/// A registry that treats every command as external (nothing is "known").
///
/// Useful when you only need span information and don't have a registry.
pub struct EmptyRegistry;

impl CommandRegistry for EmptyRegistry {
    fn has_command(&self, _name: &str) -> bool {
        false
    }
}

/// A registry backed by a static slice of name strings.
///
/// Handy for unit tests and for hard-coding shell built-ins.
pub struct SliceRegistry<'a>(pub &'a [&'a str]);

impl<'a> CommandRegistry for SliceRegistry<'a> {
    fn has_command(&self, name: &str) -> bool {
        self.0.iter().any(|&n| n == name)
    }
}

// ── Classification logic ──────────────────────────────────────────────────────

/// Classify a single `LiteCommand` against a registry.
///
/// Leading `KEY=VALUE` tokens are peeled off as `env_vars`.  The first
/// non-assignment token is the command name.  Everything after that is `args`.
pub fn classify_command(cmd: LiteCommand, registry: &dyn CommandRegistry) -> ClassifiedCommand {
    let mut parts = cmd.parts.into_iter();
    let mut env_vars: Vec<Spanned<String>> = Vec::new();
    let mut name_part: Option<Spanned<String>> = None;

    // Peel leading KEY=VALUE env-var assignments.
    for part in parts.by_ref() {
        if is_env_assignment(&part.item) {
            env_vars.push(part);
        } else {
            name_part = Some(part);
            break;
        }
    }

    let args: Vec<Spanned<String>> = parts.collect();

    let (name, kind) = match name_part {
        Some(n) => {
            let kind = if registry.has_command(&n.item) {
                CommandKind::Known
            } else {
                CommandKind::External
            };
            (n, kind)
        }
        None => {
            // Pure env-var line with no command (e.g. `export FOO=bar` parsed
            // without `export`, or a standalone assignment).
            let span = env_vars.first().map(|v| v.span).unwrap_or(Span::unknown());
            (String::new().spanned(span), CommandKind::EnvAssignment)
        }
    };

    ClassifiedCommand {
        name,
        args,
        env_vars,
        kind,
    }
}

/// Classify an entire `LiteGroup` against a registry.
///
/// Empty commands and empty pipelines are dropped from the output.
pub fn classify(group: LiteGroup, registry: &dyn CommandRegistry) -> ClassifiedBlock {
    ClassifiedBlock {
        pipelines: group
            .pipelines
            .into_iter()
            .map(|pipeline| ClassifiedPipeline {
                commands: pipeline
                    .commands
                    .into_iter()
                    .filter(|cmd| !cmd.is_empty())
                    .map(|cmd| classify_command(cmd, registry))
                    .collect(),
            })
            .filter(|p| !p.commands.is_empty())
            .collect(),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` if `s` looks like a `KEY=VALUE` shell assignment.
///
/// The key must be non-empty, start with an ASCII letter or `_`, and contain
/// only ASCII alphanumerics or `_` before the `=`.
fn is_env_assignment(s: &str) -> bool {
    match s.find('=') {
        None | Some(0) => false,
        Some(eq) => {
            let key = &s[..eq];
            key.chars()
                .next()
                .map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lite_parse::lite_parse;

    const BUILTINS: &[&str] = &[
        "cd", "echo", "ls", "grep", "git", "head", "cat", "sort", "uniq",
    ];

    fn reg() -> SliceRegistry<'static> {
        SliceRegistry(BUILTINS)
    }

    #[test]
    fn classify_known_command() {
        let (group, _) = lite_parse("ls -la");
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.name.item, "ls");
        assert_eq!(cmd.kind, CommandKind::Known);
        assert_eq!(cmd.args.len(), 1);
        assert_eq!(cmd.args[0].item, "-la");
    }

    #[test]
    fn classify_external_command() {
        let (group, _) = lite_parse("cargo build");
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.name.item, "cargo");
        assert_eq!(cmd.kind, CommandKind::External);
        assert_eq!(cmd.args[0].item, "build");
    }

    #[test]
    fn classify_single_env_var_prefix() {
        let (group, _) = lite_parse("FOO=bar cargo test");
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.env_vars.len(), 1);
        assert_eq!(cmd.env_vars[0].item, "FOO=bar");
        assert_eq!(cmd.name.item, "cargo");
        assert_eq!(cmd.kind, CommandKind::External);
    }

    #[test]
    fn classify_multiple_env_vars() {
        let (group, _) = lite_parse("DEBUG=1 FOO=bar cargo build");
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.env_vars.len(), 2);
        assert_eq!(cmd.name.item, "cargo");
    }

    #[test]
    fn classify_pipeline() {
        let (group, _) = lite_parse("ls | grep foo");
        let block = classify(group, &reg());
        assert_eq!(block.pipelines.len(), 1);
        assert_eq!(block.pipelines[0].commands.len(), 2);
        assert_eq!(block.pipelines[0].commands[0].kind, CommandKind::Known);
        assert_eq!(block.pipelines[0].commands[1].kind, CommandKind::Known);
    }

    #[test]
    fn classify_three_stage_pipeline() {
        let (group, _) = lite_parse("cat file | sort | uniq -c");
        let block = classify(group, &reg());
        let pipeline = &block.pipelines[0];
        assert_eq!(pipeline.commands.len(), 3);
        assert_eq!(pipeline.commands[0].name.item, "cat");
        assert_eq!(pipeline.commands[1].name.item, "sort");
        assert_eq!(pipeline.commands[2].name.item, "uniq");
    }

    #[test]
    fn classify_multi_pipeline_group() {
        let (group, _) = lite_parse("cd /tmp; ls -la");
        let block = classify(group, &reg());
        assert_eq!(block.pipelines.len(), 2);
    }

    #[test]
    fn classify_empty_input() {
        let (group, _) = lite_parse("");
        let block = classify(group, &reg());
        assert!(block.pipelines.is_empty());
    }

    #[test]
    fn empty_registry_makes_all_external() {
        let (group, _) = lite_parse("ls");
        let block = classify(group, &EmptyRegistry);
        assert_eq!(block.pipelines[0].commands[0].kind, CommandKind::External);
    }

    #[test]
    fn env_assignment_only_has_kind_env_assignment() {
        // A bare assignment with no trailing command — uncommon but valid.
        let (group, _) = lite_parse("FOO=bar");
        let block = classify(group, &reg());
        // The "command" is env-assignment-only; no name part.
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.kind, CommandKind::EnvAssignment);
        assert!(cmd.name.item.is_empty());
    }

    #[test]
    fn is_env_assignment_examples() {
        assert!(is_env_assignment("FOO=bar"));
        assert!(is_env_assignment("_VAR=value"));
        assert!(is_env_assignment("MY_VAR_123=xyz"));
        assert!(is_env_assignment("A="));

        assert!(!is_env_assignment("=bar"));
        assert!(!is_env_assignment("123=val"));
        assert!(!is_env_assignment("no-equals"));
        assert!(!is_env_assignment(""));
        assert!(!is_env_assignment("FOO-BAR=baz")); // hyphen not allowed in key
    }

    #[test]
    fn span_of_classified_command() {
        let input = "ls -la";
        let (group, _) = lite_parse(input);
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        // Span should cover at least the start of "ls" through the end of "-la"
        let span = cmd.span();
        assert_eq!(span.start, 0);
        assert_eq!(span.end, input.len());
    }
}
