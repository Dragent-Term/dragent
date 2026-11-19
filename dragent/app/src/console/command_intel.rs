//! Lightweight command diagnostics and token hints for the input layer.

use std::ops::Range;

use crate::console::cmd_suggest::{is_known_path_command, specs, CommandSpec, FlagSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    UnknownCommand,
    UnknownFlag,
}

#[derive(Clone, Debug)]
pub struct CommandDiagnostic {
    pub byte_range: Range<usize>,
    pub kind: DiagnosticKind,
    pub message: String,
}

/// Shell-like tokenization with byte ranges (no quote handling beyond skipping empty).
fn tokenize(command: &str) -> Vec<(Range<usize>, &str)> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (byte_idx, ch) in command.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                tokens.push((s..byte_idx, &command[s..byte_idx]));
            }
        } else if start.is_none() {
            start = Some(byte_idx);
        }
    }
    if let Some(s) = start {
        tokens.push((s..command.len(), &command[s..command.len()]));
    }
    tokens
}

fn find_spec(name: &str) -> Option<&'static CommandSpec> {
    specs().find(|s| s.name == name)
}

fn flag_matches(spec: &FlagSpec, token: &str) -> bool {
    spec.names.iter().any(|n| *n == token)
}

fn find_flag_spec(spec: &CommandSpec, token: &str) -> Option<&'static FlagSpec> {
    spec.flags.iter().find(|f| flag_matches(f, token))
}

fn collect_specs_for_tokens(tokens: &[&str]) -> Option<&'static CommandSpec> {
    if tokens.is_empty() {
        return None;
    }
    let mut spec = find_spec(tokens[0])?;
    for part in tokens.iter().skip(1) {
        if part.starts_with('-') {
            continue;
        }
        spec = spec.subcommands.iter().find(|s| s.name == *part)?;
    }
    Some(spec)
}

pub fn diagnostics_for(command: &str) -> Vec<CommandDiagnostic> {
    use dragent_parser::{classify_command, lite_parse, EmptyRegistry};

    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let raw_tokens: Vec<(Range<usize>, &str)> = tokenize(trimmed);
    if raw_tokens.is_empty() {
        return Vec::new();
    }

    let offset = command.find(trimmed).unwrap_or(0);
    let mut out = Vec::new();

    // Use the parser to find the real command name, skipping leading KEY=VALUE env-var
    // assignments. Without this, "FOO=bar git push" underlines "FOO=bar" as unknown.
    let (group, _) = lite_parse(trimmed);
    let Some(classified) = group
        .pipelines
        .first()
        .and_then(|p| p.commands.first())
        .map(|cmd| classify_command(cmd.clone(), &EmptyRegistry))
    else {
        return out;
    };

    // Pure env-var assignment with no trailing command — nothing to diagnose.
    if classified.name.item.is_empty() {
        return out;
    }

    let env_var_count = classified.env_vars.len();
    let first = classified.name.item.as_str();

    let known = find_spec(first).is_some()
        || is_known_path_command(first)
        || looks_like_path(first)
        || is_common_builtin(first);

    if !known {
        // Underline the actual command token, not any preceding env-var.
        let cmd_range = raw_tokens
            .get(env_var_count)
            .map(|(r, _)| r.clone())
            .unwrap_or_else(|| raw_tokens[0].0.clone());
        out.push(CommandDiagnostic {
            byte_range: (offset + cmd_range.start)..(offset + cmd_range.end),
            kind: DiagnosticKind::UnknownCommand,
            message: format!("Unknown command '{first}'"),
        });
        return out;
    }

    // Skip env-var prefix tokens before looking up specs and checking flags.
    let parts: Vec<&str> = raw_tokens
        .iter()
        .skip(env_var_count)
        .map(|(_, t)| *t)
        .collect();
    let Some(active_spec) = collect_specs_for_tokens(&parts) else {
        return out;
    };

    for (range, token) in raw_tokens.iter().skip(env_var_count + 1) {
        if !token.starts_with('-') {
            continue;
        }
        if find_flag_spec(active_spec, token).is_none() {
            let byte_range = (offset + range.start)..(offset + range.end);
            out.push(CommandDiagnostic {
                byte_range,
                kind: DiagnosticKind::UnknownFlag,
                message: format!("Unknown flag '{token}' for {}", active_spec.name),
            });
        }
    }

    out
}

pub fn token_hint_at(command: &str, cursor_byte: usize) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let offset = command.find(trimmed).unwrap_or(0);
    let local_cursor = cursor_byte.saturating_sub(offset);

    let tokens: Vec<(Range<usize>, &str)> = tokenize(trimmed);
    for (range, token) in &tokens {
        if local_cursor >= range.start && local_cursor <= range.end {
            let parts: Vec<&str> = tokens.iter().map(|(_, t)| *t).collect();
            if *token == parts[0] {
                return find_spec(token).map(|s| s.description.to_string());
            }
            if token.starts_with('-') {
                if let Some(spec) = collect_specs_for_tokens(&parts) {
                    if let Some(flag) = find_flag_spec(spec, token) {
                        return Some(flag.description.to_string());
                    }
                }
            }
            if let Some(spec) = collect_specs_for_tokens(&parts) {
                if let Some(sub) = spec.subcommands.iter().find(|s| s.name == *token) {
                    return Some(sub.description.to_string());
                }
            }
        }
    }
    None
}

fn looks_like_path(token: &str) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("/")
        || token.starts_with("~/")
        || token.contains("/")
}

fn is_common_builtin(token: &str) -> bool {
    matches!(
        token,
        "cd" | "echo"
            | "exit"
            | "export"
            | "source"
            | "."
            | "unset"
            | "alias"
            | "unalias"
            | "type"
            | "which"
            | "jobs"
            | "fg"
            | "bg"
            | "kill"
            | "sudo"
            | "time"
            | "env"
            | "printenv"
    )
}

/// Map a diagnostic byte range to character columns on a single-line visual slice.
pub fn diagnostic_cols_on_line(
    line_byte_offset: usize,
    line_text: &str,
    diags: &[CommandDiagnostic],
) -> Vec<(usize, usize)> {
    let line_end = line_byte_offset + line_text.len();
    let mut cols = Vec::new();
    for d in diags {
        if d.byte_range.end <= line_byte_offset || d.byte_range.start >= line_end {
            continue;
        }
        let local_start = d.byte_range.start.saturating_sub(line_byte_offset);
        let local_end = d
            .byte_range
            .end
            .saturating_sub(line_byte_offset)
            .min(line_text.len());
        let start_col = line_text[..local_start.min(line_text.len())]
            .chars()
            .count();
        let end_col = line_text[..local_end.min(line_text.len())].chars().count();
        if start_col < end_col {
            cols.push((start_col, end_col));
        }
    }
    cols
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unknown_git_flag() {
        let diags = diagnostics_for("git status --not-a-flag");
        assert!(diags
            .iter()
            .any(|d| matches!(d.kind, DiagnosticKind::UnknownFlag)));
    }

    #[test]
    fn hints_on_command_name() {
        let hint = token_hint_at("git status", 1);
        assert!(hint.is_some());
    }
}
