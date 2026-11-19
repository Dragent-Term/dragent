//! End-to-end demo: lex → lite_parse → classify
//!
//! Usage:
//!   cargo run -p dragent_parser --example parse -- "ls -la | grep foo"
//!   cargo run -p dragent_parser --example parse -- "git add . && git commit -m 'fix'"
//!   cargo run -p dragent_parser --example parse -- "FOO=bar cargo build 2> err.log"

use dragent_parser::{classify, lex, lite_parse, CommandKind, SliceRegistry, Token};

// A small set of common shell built-ins to make Known/External distinction visible.
const KNOWN: &[&str] = &[
    "ls", "cd", "echo", "cat", "grep", "head", "tail", "sort", "uniq", "wc", "mkdir", "rm", "cp",
    "mv", "find", "which", "git", "cargo", "make", "export", "source", "printf", "true", "false",
    "test",
];

fn main() {
    let input: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if input.is_empty() {
        eprintln!("Usage: parse \"<shell command>\"");
        std::process::exit(1);
    }

    println!("Input: {:?}\n", input);

    // ── Stage 1: lex ──────────────────────────────────────────────────────────
    println!("─── Tokens ───────────────────────────────────────────");
    let tokens = lex(&input);
    for tok in &tokens {
        let kind = match &tok.item {
            Token::Bare => "Bare",
            Token::SingleQuoted => "SingleQuoted",
            Token::DoubleQuoted => "DoubleQuoted",
            Token::BacktickQuoted => "BacktickQuoted",
            Token::Pipe => "Pipe",
            Token::LogicalOr => "LogicalOr",
            Token::LogicalAnd => "LogicalAnd",
            Token::Semicolon => "Semicolon",
            Token::Newline => "Newline",
            Token::Whitespace => "Whitespace",
            Token::Comment => "Comment",
            Token::Ampersand => "Ampersand",
            Token::RedirectOut => "RedirectOut",
            Token::RedirectAppend => "RedirectAppend",
            Token::RedirectIn => "RedirectIn",
            Token::RedirectErr => "RedirectErr",
            Token::RedirectErrAppend => "RedirectErrAppend",
            Token::RedirectBoth => "RedirectBoth",
            Token::CommandSubstOpen => "CommandSubstOpen",
            Token::OpenParen => "OpenParen",
            Token::CloseParen => "CloseParen",
        };
        if !matches!(tok.item, Token::Whitespace) {
            println!(
                "  [{:>4}..{:<4}]  {:20}  {:?}",
                tok.span.start,
                tok.span.end,
                kind,
                tok.span.slice(&input)
            );
        }
    }

    // ── Stage 2: lite_parse ───────────────────────────────────────────────────
    println!("\n─── Lite AST ─────────────────────────────────────────");
    let (group, errors) = lite_parse(&input);
    if !errors.is_empty() {
        for e in &errors {
            println!("  WARN: {} at {:?}", e.message, e.span);
        }
    }
    for (pi, pipeline) in group.pipelines.iter().enumerate() {
        println!("  Pipeline[{pi}]");
        for (ci, cmd) in pipeline.commands.iter().enumerate() {
            println!("    Command[{ci}]");
            for part in &cmd.parts {
                println!(
                    "      part  [{:>4}..{:<4}]  {:?}",
                    part.span.start, part.span.end, part.item
                );
            }
            if let Some(ws) = cmd.trailing_whitespace {
                println!("      trailing_whitespace at {}..{}", ws.start, ws.end);
            }
        }
    }

    // ── Stage 3: classify ─────────────────────────────────────────────────────
    println!("\n─── Classified AST ───────────────────────────────────");
    let registry = SliceRegistry(KNOWN);
    let (group2, _) = lite_parse(&input);
    let block = classify(group2, &registry);
    for (pi, pipeline) in block.pipelines.iter().enumerate() {
        println!("  Pipeline[{pi}]");
        for cmd in &pipeline.commands {
            let kind_str = match cmd.kind {
                CommandKind::Known => "Known    ",
                CommandKind::External => "External ",
                CommandKind::EnvAssignment => "EnvAssign",
            };
            if !cmd.env_vars.is_empty() {
                let vars: Vec<_> = cmd.env_vars.iter().map(|v| v.item.as_str()).collect();
                println!("    env   {}", vars.join(" "));
            }
            println!("    cmd   [{}]  {:?}", kind_str, cmd.name.item);
            for arg in &cmd.args {
                println!("      arg  {:?}", arg.item);
            }
        }
    }

    println!();
}
