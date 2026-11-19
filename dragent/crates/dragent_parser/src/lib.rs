//! Shell command parser for dragent.
//!
//! Implements a three-stage parsing pipeline inspired by nushell's architecture
//! (MIT license, <https://github.com/nushell/nushell>).
//!
//! ```text
//! source text
//!     │
//!     ▼  lex()
//! Vec<Spanned<Token>>          — every byte classified, all spans valid
//!     │
//!     ▼  lite_parse()
//! LiteGroup { pipelines }      — commands, pipelines, statement groups
//!     │
//!     ▼  classify()
//! ClassifiedBlock { pipelines} — each command tagged Known/External/EnvAssign
//! ```
//!
//! ## Quick start
//!
//! ```rust
//! use dragent_parser::{lite_parse, classify, SliceRegistry, CommandKind};
//!
//! let (group, _errors) = lite_parse("ls -la | grep foo");
//! let registry = SliceRegistry(&["ls", "grep"]);
//! let block = classify(group, &registry);
//!
//! let pipeline = &block.pipelines[0];
//! assert_eq!(pipeline.commands[0].name.item, "ls");
//! assert_eq!(pipeline.commands[0].kind, CommandKind::Known);
//! assert_eq!(pipeline.commands[1].name.item, "grep");
//! ```

pub mod classify;
pub mod lex;
pub mod lite_parse;
pub mod span;

pub use classify::{
    classify, classify_command, ClassifiedBlock, ClassifiedCommand, ClassifiedPipeline,
    CommandKind, CommandRegistry, EmptyRegistry, SliceRegistry,
};
pub use lex::{lex, Token};
pub use lite_parse::{lite_parse, LiteCommand, LiteGroup, LitePipeline, ParseError};
pub use span::{HasSpan, Span, Spanned, SpannedItem};

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod integration {
    use super::*;

    fn reg() -> SliceRegistry<'static> {
        SliceRegistry(&["ls", "grep", "head", "cat", "sort", "git", "cd", "echo"])
    }

    #[test]
    fn full_three_stage_pipeline() {
        let (group, errs) = lite_parse("ls -la /tmp | grep foo | head -1");
        assert!(errs.is_empty());
        let block = classify(group, &reg());
        assert_eq!(block.pipelines.len(), 1);
        let cmds = &block.pipelines[0].commands;
        assert_eq!(cmds.len(), 3);

        assert_eq!(cmds[0].name.item, "ls");
        assert_eq!(cmds[0].kind, CommandKind::Known);
        assert_eq!(cmds[0].args.len(), 2);

        assert_eq!(cmds[1].name.item, "grep");
        assert_eq!(cmds[1].kind, CommandKind::Known);
        assert_eq!(cmds[1].args[0].item, "foo");

        assert_eq!(cmds[2].name.item, "head");
        assert_eq!(cmds[2].args[0].item, "-1");
    }

    #[test]
    fn env_var_prefix_then_external() {
        let (group, _) = lite_parse("DEBUG=1 FOO=bar cargo build --release");
        let block = classify(group, &reg());
        let cmd = &block.pipelines[0].commands[0];
        assert_eq!(cmd.env_vars.len(), 2);
        assert_eq!(cmd.name.item, "cargo");
        assert_eq!(cmd.kind, CommandKind::External);
        assert_eq!(cmd.args.len(), 2);
    }

    #[test]
    fn multiline_script_three_pipelines() {
        let (group, _) = lite_parse("git add .\ngit commit -m 'fix'\ngit push");
        assert_eq!(group.pipelines.len(), 3);
        let block = classify(group, &reg());
        assert_eq!(block.pipelines.len(), 3);
        for p in &block.pipelines {
            assert_eq!(p.commands[0].name.item, "git");
            assert_eq!(p.commands[0].kind, CommandKind::Known);
        }
    }

    #[test]
    fn semicolon_separated_commands() {
        let (group, _) = lite_parse("cd /tmp; ls -la; echo done");
        let block = classify(group, &reg());
        assert_eq!(block.pipelines.len(), 3);
    }

    #[test]
    fn redirect_stays_in_command_parts() {
        let input = "ls > out.txt";
        let (group, _) = lite_parse(input);
        let cmd = &group.pipelines[0].commands[0];
        // ">" and "out.txt" appear as parts after "ls"
        assert!(cmd.parts.iter().any(|p| p.item == ">"));
        assert!(cmd.parts.iter().any(|p| p.item == "out.txt"));
    }

    #[test]
    fn spans_are_valid_for_all_tokens() {
        let input = "git commit -m 'fix the bug' && git push origin main";
        let tokens = lex(input);
        for tok in &tokens {
            // Every span slice must not panic and must round-trip
            let _text = tok.span.slice(input);
            assert!(tok.span.start <= tok.span.end);
            assert!(tok.span.end <= input.len());
        }
    }

    #[test]
    fn quoted_arg_with_spaces_is_one_part() {
        let (group, _) = lite_parse("echo 'hello world'");
        let cmd = &group.pipelines[0].commands[0];
        assert_eq!(cmd.parts.len(), 2);
    }

    #[test]
    fn background_job_does_not_leak_ampersand_into_command() {
        let (group, _) = lite_parse("sleep 10 &");
        // The `&` should end the pipeline, not appear as a command arg
        let cmd = &group.pipelines[0].commands[0];
        assert_eq!(cmd.parts[0].item, "sleep");
        assert!(cmd.parts.iter().all(|p| p.item != "&"));
    }
}
