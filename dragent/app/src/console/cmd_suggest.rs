use crate::console::fs_suggest::{FsEntry, FsSuggestRequest, MatchType};
use crate::process_ext::NoWindow;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

static FUZZY: Lazy<SkimMatcherV2> = Lazy::new(SkimMatcherV2::default);

/// All executable names found in $PATH, built once on first access.
static PATH_COMMANDS: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut commands = HashSet::new();
    let path_var = match std::env::var("PATH") {
        Ok(v) => v,
        Err(_) => return commands,
    };
    for dir in path_var.split(':') {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = entry.metadata() {
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                commands.insert(name.to_string());
            }
        }
    }
    commands
});

/// Returns true if `name` exists as an executable in the system PATH.
pub fn is_known_path_command(name: &str) -> bool {
    PATH_COMMANDS.contains(name)
}

// ── Live --help parsing ───────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct ParsedHelp {
    subcommands: Vec<(String, String)>,
    flags: Vec<(String, String)>,
}

enum HelpEntry {
    InProgress,
    Done(ParsedHelp),
    Failed,
}

static HELP_CACHE: Lazy<Mutex<HashMap<String, HelpEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Query the help cache for `cmd`. If a cache entry exists returns suggestions
/// filtered by `target`. If no entry exists, fires a background parse and
/// returns `None` (caller gets nothing this frame, results appear next frame).
fn query_help_cache(cmd: &str, target: &str) -> Option<Vec<FsEntry>> {
    // Phase 1: read under shared lock.
    {
        let cache = HELP_CACHE.lock().ok()?;
        match cache.get(cmd) {
            Some(HelpEntry::Done(parsed)) => {
                return Some(entries_from_parsed(parsed, target));
            }
            Some(HelpEntry::InProgress) => return Some(vec![]),
            Some(HelpEntry::Failed) => return Some(vec![]),
            None => {} // fall through to spawn
        }
    }
    // Phase 2: nothing in cache yet — mark in-progress and spawn parser.
    {
        let mut cache = HELP_CACHE.lock().ok()?;
        // Another thread may have beaten us between the two lock acquisitions.
        if cache.contains_key(cmd) {
            return Some(vec![]);
        }
        cache.insert(cmd.to_string(), HelpEntry::InProgress);
    }
    let cmd_owned = cmd.to_string();
    crate::bg::detach(move || {
        let entry = run_and_parse_help(&cmd_owned);
        if let Ok(mut cache) = HELP_CACHE.lock() {
            cache.insert(cmd_owned, entry);
        }
    });
    None
}

fn entries_from_parsed(parsed: &ParsedHelp, target: &str) -> Vec<FsEntry> {
    let mut entries: Vec<FsEntry> = if target.starts_with('-') {
        parsed
            .flags
            .iter()
            .filter(|(name, _)| name.starts_with(target))
            .map(|(name, desc)| FsEntry {
                name: name.clone(),
                is_dir: false,
                display_path: String::new(),
                description: if desc.is_empty() {
                    None
                } else {
                    Some(desc.clone())
                },
                match_type: MatchType::Prefix,
                fuzzy_score: 50.0,
            })
            .collect()
    } else {
        parsed
            .subcommands
            .iter()
            .filter(|(name, _)| target.is_empty() || name.starts_with(target))
            .map(|(name, desc)| FsEntry {
                name: name.clone(),
                is_dir: false,
                display_path: String::new(),
                description: if desc.is_empty() {
                    None
                } else {
                    Some(desc.clone())
                },
                match_type: MatchType::Prefix,
                fuzzy_score: 50.0,
            })
            .collect()
    };
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Upper bound on captured `--help` output. Real help text is a few KiB; the cap
/// exists so a command that IGNORES `--help` and streams forever cannot OOM us.
/// The classic offender is BSD/macOS `yes`, which treats `--help` as the string to
/// repeat and prints it infinitely — `Command::output()` (unbounded) would buffer
/// that until the process is OOM-killed.
const MAX_HELP_BYTES: u64 = 256 * 1024;

/// Runs `<cmd> --help` (falls back to `-h`) in a dedicated thread with a
/// 3-second timeout so a misbehaving binary can never stall the cache thread.
fn run_and_parse_help(cmd: &str) -> HelpEntry {
    use std::sync::mpsc;
    use std::time::Duration;

    let cmd_owned = cmd.to_string();
    let (tx, rx) = mpsc::sync_channel(1);

    crate::bg::detach(move || {
        let text = run_help_capped(&cmd_owned, "--help")
            .filter(|t| !t.trim().is_empty())
            .or_else(|| run_help_capped(&cmd_owned, "-h"));
        let _ = tx.send(text);
    });

    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(Some(text)) if !text.trim().is_empty() => HelpEntry::Done(parse_help_text(&text)),
        _ => HelpEntry::Failed,
    }
}

/// Spawn `<cmd> <flag>` and capture at most [`MAX_HELP_BYTES`] of stdout+stderr,
/// then kill the child. Unlike `Command::output()` this is bounded, so a command
/// that streams unbounded output (e.g. `yes --help`) is truncated and stopped
/// instead of buffered to death. stdout and stderr are read concurrently to avoid
/// a pipe-buffer deadlock.
fn run_help_capped(cmd: &str, flag: &str) -> Option<String> {
    use std::io::Read;

    let mut child = std::process::Command::new(cmd)
        .no_window()
        .arg(flag)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let mut stdout = child.stdout.take()?;
    let stderr = child.stderr.take();

    // Drain stderr on its own thread so a child that fills the stderr pipe can't
    // block us while we're reading stdout (and vice-versa).
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut e) = stderr {
            let _ = e.by_ref().take(MAX_HELP_BYTES).read_to_end(&mut buf);
        }
        buf
    });

    let mut out = Vec::new();
    let _ = (&mut stdout).take(MAX_HELP_BYTES).read_to_end(&mut out);

    // We have our cap (or EOF); stop the child now so an infinite producer dies
    // immediately rather than lingering and accumulating in the OS pipe buffer.
    let _ = child.kill();
    let _ = child.wait();

    let mut err = stderr_handle.join().unwrap_or_default();
    out.append(&mut err);
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn parse_help_text(text: &str) -> ParsedHelp {
    let mut subcommands: Vec<(String, String)> = Vec::new();
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut in_commands_section = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Section header detection — not indented deeply and ends with ':'
        let lower = trimmed.to_lowercase();
        let indent = line.len() - line.trim_start().len();
        if trimmed.ends_with(':') && indent < 4 && !trimmed.starts_with('-') {
            in_commands_section = lower.contains("command") || lower.contains("subcommand");
            continue;
        }

        // Flag lines (start with '-' after trimming)
        if trimmed.starts_with('-') {
            if let Some((names_part, desc)) = split_help_line(trimmed) {
                // Prefer the long form (--flag) as the key when both are present
                let flag_key = names_part
                    .split(',')
                    .map(|s| s.trim().split_whitespace().next().unwrap_or("").to_string())
                    .filter(|s| s.starts_with('-'))
                    .max_by_key(|s| s.len())
                    .unwrap_or_default();
                if !flag_key.is_empty() && flag_key.len() < 40 {
                    flags.push((flag_key, desc.trim().to_string()));
                }
            } else {
                // Flag with no description
                let flag = trimmed
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(',');
                if flag.starts_with('-') && flag.len() < 40 {
                    flags.push((flag.to_string(), String::new()));
                }
            }
            continue;
        }

        // Subcommand lines inside a commands section
        if in_commands_section {
            if let Some((name_part, desc)) = split_help_line(trimmed) {
                let name = name_part
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty()
                    && name.len() < 30
                    && !name.contains(':')
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                {
                    subcommands.push((name, desc.trim().to_string()));
                }
            }
        }
    }

    ParsedHelp { subcommands, flags }
}

/// Splits a help-output line on the first run of 2+ spaces.
/// Returns `(left_part, right_description)` or `None` if no such split exists.
fn split_help_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b' ' && bytes[i + 1] == b' ' {
            let left = line[..i].trim();
            let right = line[i..].trim();
            if !left.is_empty() && !right.is_empty() {
                return Some((left, right));
            }
        }
        i += 1;
    }
    None
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub relevance: f32,
    pub subcommands: &'static [CommandSpec],
    pub flags: &'static [FlagSpec],
}

#[derive(Debug, Clone)]
pub struct FlagSpec {
    pub names: &'static [&'static str],
    pub description: &'static str,
    pub relevance: f32,
}

/// Every coding-agent CLI Dragent can launch that has no hand-written spec,
/// built from [`PromptAgent::ALL`](crate::workspace::PromptAgent) so a newly
/// added agent is suggested without touching the spec tables. Their flags
/// come from the live `--help` cache once installed.
static AGENT_SPECS: Lazy<Vec<CommandSpec>> = Lazy::new(|| {
    crate::workspace::PromptAgent::ALL
        .iter()
        .filter_map(|agent| Some((agent.cli()?, agent.label())))
        .filter(|(cli, _)| !static_specs().any(|spec| spec.name == *cli))
        .map(|(cli, label)| CommandSpec {
            name: cli,
            description: Box::leak(format!("{label} — coding agent").into_boxed_str()),
            relevance: 84.0,
            subcommands: &[],
            flags: &[],
        })
        .collect()
});

/// The keys `--agent` accepts (`dragent agents list`), Dragent's own first.
fn agent_keys() -> impl Iterator<Item = (&'static str, &'static str)> {
    crate::workspace::PromptAgent::ALL
        .iter()
        .map(|agent| (agent.command_key(), agent.label()))
}

/// All command specs: the cmd_specs_* shards in order, then the agent CLIs.
pub fn specs() -> impl Iterator<Item = &'static CommandSpec> {
    static_specs().chain(AGENT_SPECS.iter())
}

fn static_specs() -> impl Iterator<Item = &'static CommandSpec> {
    super::cmd_specs_01::SPECS_01
        .iter()
        .chain(super::cmd_specs_02::SPECS_02)
        .chain(super::cmd_specs_03::SPECS_03)
        .chain(super::cmd_specs_04::SPECS_04)
        .chain(super::cmd_specs_05::SPECS_05)
        .chain(super::cmd_specs_06::SPECS_06)
        .chain(super::cmd_specs_07::SPECS_07)
        .chain(super::cmd_specs_08::SPECS_08)
        .chain(super::cmd_specs_09::SPECS_09)
        .chain(super::cmd_specs_10::SPECS_10)
        .chain(super::cmd_specs_11::SPECS_11)
        .chain(super::cmd_specs_12::SPECS_12)
        .chain(super::cmd_specs_13::SPECS_13)
        .chain(super::cmd_specs_14::SPECS_14)
}

pub fn build_cmd_suggest(buffer: &str) -> Option<FsSuggestRequest> {
    let tokens: Vec<&str> = buffer.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    let is_trailing_space = buffer.ends_with(char::is_whitespace);

    let mut current_spec: Option<&CommandSpec> = None;
    let mut word_idx = 0;

    let is_last_token = tokens.len() == 1 && !is_trailing_space;

    // Find the base command by exact name match.
    for spec in specs() {
        if spec.name == tokens[0] {
            current_spec = Some(spec);
            break;
        }
    }

    if current_spec.is_none() && is_last_token {
        // The user is still typing the command name. Try prefix matching first,
        // then fall back to fuzzy matching so "gti" → "git", "carfo" → "cargo", etc.
        let mut entries = Vec::new();
        for spec in specs() {
            if spec.name.starts_with(tokens[0]) {
                entries.push(FsEntry {
                    name: spec.name.to_string(),
                    is_dir: false,
                    display_path: String::new(),
                    description: Some(spec.description.to_string()),
                    match_type: MatchType::Prefix,
                    fuzzy_score: spec.relevance,
                });
            }
        }
        // Fuzzy fallback against SPECS: only when no prefix match found.
        if entries.is_empty() {
            for spec in specs() {
                if let Some(score) = FUZZY.fuzzy_match(spec.name, tokens[0]) {
                    entries.push(FsEntry {
                        name: spec.name.to_string(),
                        is_dir: false,
                        display_path: String::new(),
                        description: Some(spec.description.to_string()),
                        match_type: MatchType::Fuzzy,
                        fuzzy_score: score as f32,
                    });
                }
            }
        }
        // PATH fallback: prefix-match against every executable in $PATH.
        // Only fires when SPECS gave nothing at all, so SPECS always wins.
        if entries.is_empty() {
            let prefix = tokens[0];
            let mut path_entries: Vec<FsEntry> = PATH_COMMANDS
                .iter()
                .filter(|name| name.starts_with(prefix))
                .map(|name| FsEntry {
                    name: name.clone(),
                    is_dir: false,
                    display_path: String::new(),
                    description: Some("System command".to_string()),
                    match_type: MatchType::Prefix,
                    fuzzy_score: 40.0,
                })
                .collect();
            // If even prefix gave nothing, try fuzzy against PATH.
            if path_entries.is_empty() {
                path_entries = PATH_COMMANDS
                    .iter()
                    .filter_map(|name| {
                        FUZZY.fuzzy_match(name, prefix).map(|score| FsEntry {
                            name: name.clone(),
                            is_dir: false,
                            display_path: String::new(),
                            description: Some("System command".to_string()),
                            match_type: MatchType::Fuzzy,
                            fuzzy_score: score as f32,
                        })
                    })
                    .collect();
            }
            path_entries.sort_by(|a, b| a.match_type.cmp(&b.match_type).then(a.name.cmp(&b.name)));
            entries.extend(path_entries);
        }
        if entries.is_empty() {
            return None;
        }
        entries.sort_by(|a, b| {
            // Prefer prefix matches over fuzzy, then by score.
            a.match_type.cmp(&b.match_type).then(
                b.fuzzy_score
                    .partial_cmp(&a.fuzzy_score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        return Some(FsSuggestRequest {
            replace_start: buffer.len() - tokens[0].len(),
            replace_end: buffer.len(),
            entries,
            browse_root_label: "Commands".to_string(),
        });
    }

    // Command not in SPECS but user is past the command name (has typed a space
    // or more tokens). Check the live --help cache for dynamic completions.
    // A spec with nothing in it (the agent CLIs) does the same.
    let current_spec = match current_spec {
        Some(spec) if !spec.subcommands.is_empty() || !spec.flags.is_empty() => spec,
        _ => return help_cache_suggest(buffer, &tokens, is_trailing_space),
    };

    // `--agent KEY` / `--agents a,b`: Dragent's agent keys.
    if let Some(request) = agent_key_suggest(buffer, &tokens, is_trailing_space) {
        return Some(request);
    }
    let mut active_spec = current_spec;
    word_idx += 1;

    // Collect flags already present in the buffer so we never re-suggest them.
    let already_used_flags: Vec<&str> = tokens[1..]
        .iter()
        .filter(|t| t.starts_with('-'))
        .copied()
        .collect();

    // Traverse subcommand hierarchy, tracking the last token we actually consumed.
    let mut last_consumed_was_flag = false;
    while word_idx < tokens.len() {
        let is_last = word_idx == tokens.len() - 1 && !is_trailing_space;
        if is_last {
            break;
        }
        let word = tokens[word_idx];
        let mut found_sub = false;
        for sub in active_spec.subcommands {
            if sub.name == word {
                active_spec = sub;
                found_sub = true;
                last_consumed_was_flag = false;
                break;
            }
        }
        if !found_sub {
            if word.starts_with('-') {
                last_consumed_was_flag = true;
            } else {
                // Non-flag, non-subcommand word — user is writing a free-form value.
                // Stop traversal and suppress structured suggestions.
                return None;
            }
        }
        word_idx += 1;
    }

    let target_word = if is_trailing_space || word_idx >= tokens.len() {
        ""
    } else {
        tokens[tokens.len() - 1]
    };

    // If the last consumed token was a flag and we now have a trailing space,
    // the user is typing that flag's value — suppress further suggestions.
    if last_consumed_was_flag && is_trailing_space && target_word.is_empty() {
        return None;
    }

    let mut entries = Vec::new();

    if target_word.starts_with('-') || target_word.is_empty() {
        for flag in active_spec.flags {
            for name in flag.names {
                if already_used_flags.contains(name) {
                    continue;
                }
                if name.starts_with(target_word) {
                    entries.push(FsEntry {
                        name: name.to_string(),
                        is_dir: false,
                        display_path: String::new(),
                        description: Some(flag.description.to_string()),
                        match_type: MatchType::Prefix,
                        fuzzy_score: flag.relevance,
                    });
                }
            }
        }
    }

    if !target_word.starts_with('-') {
        // Prefix matches for subcommands.
        for sub in active_spec.subcommands {
            if sub.name.starts_with(target_word) {
                entries.push(FsEntry {
                    name: sub.name.to_string(),
                    is_dir: false,
                    display_path: String::new(),
                    description: Some(sub.description.to_string()),
                    match_type: MatchType::Prefix,
                    fuzzy_score: sub.relevance,
                });
            }
        }
        // Fuzzy fallback for subcommands when no prefix match was found and
        // the user has typed at least two characters (avoids noise on single chars).
        if entries.is_empty() && target_word.len() >= 2 {
            for sub in active_spec.subcommands {
                if let Some(score) = FUZZY.fuzzy_match(sub.name, target_word) {
                    entries.push(FsEntry {
                        name: sub.name.to_string(),
                        is_dir: false,
                        display_path: String::new(),
                        description: Some(sub.description.to_string()),
                        match_type: MatchType::Fuzzy,
                        // Blend the fuzzy score with command relevance so
                        // high-priority subcommands still win on ties.
                        fuzzy_score: (score as f32).min(sub.relevance),
                    });
                }
            }
        }
    }

    if entries.is_empty() {
        return None;
    }

    entries.sort_by(|a, b| {
        a.match_type.cmp(&b.match_type).then(
            b.fuzzy_score
                .partial_cmp(&a.fuzzy_score)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let replace_start = if target_word.is_empty() {
        buffer.len()
    } else {
        buffer.len() - target_word.len()
    };

    Some(FsSuggestRequest {
        replace_start,
        replace_end: buffer.len(),
        entries,
        browse_root_label: "Commands".to_string(),
    })
}

fn help_cache_suggest(
    buffer: &str,
    tokens: &[&str],
    is_trailing_space: bool,
) -> Option<FsSuggestRequest> {
    let cmd = tokens[0];
    if !PATH_COMMANDS.contains(cmd) {
        return None;
    }
    let target = if is_trailing_space {
        ""
    } else {
        tokens.last().copied().unwrap_or("")
    };
    let entries = query_help_cache(cmd, target)?;
    if entries.is_empty() {
        return None;
    }
    Some(FsSuggestRequest {
        replace_start: buffer.len() - target.len(),
        replace_end: buffer.len(),
        entries,
        browse_root_label: format!("{} —", cmd),
    })
}

/// Agent keys after `dragent … --agent ` (or `--agents a,`), and after
/// `--agent=`.
fn agent_key_suggest(
    buffer: &str,
    tokens: &[&str],
    is_trailing_space: bool,
) -> Option<FsSuggestRequest> {
    if tokens.first() != Some(&"dragent") {
        return None;
    }
    let (flag, value) = if is_trailing_space {
        (*tokens.last()?, "")
    } else if let Some((flag, value)) = tokens.last()?.split_once('=') {
        (flag, value)
    } else if tokens.len() >= 2 {
        (tokens[tokens.len() - 2], *tokens.last()?)
    } else {
        return None;
    };
    if !matches!(flag, "--agent" | "--agents") {
        return None;
    }
    // `--agents claude,co` completes the part after the last comma.
    let typed = value.rsplit(',').next().unwrap_or(value);
    let entries: Vec<FsEntry> = agent_keys()
        .filter(|(key, _)| key.starts_with(typed))
        .map(|(key, label)| FsEntry {
            name: key.to_string(),
            is_dir: false,
            display_path: String::new(),
            description: Some(label.to_string()),
            match_type: MatchType::Prefix,
            fuzzy_score: 50.0,
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(FsSuggestRequest {
        replace_start: buffer.len() - typed.len(),
        replace_end: buffer.len(),
        entries,
        browse_root_label: "Agents".to_string(),
    })
}

pub fn ghost_suffix_from_cmd(buffer: &str) -> Option<String> {
    let req = build_cmd_suggest(buffer)?;
    let top_entry = req.entries.first()?;

    // Fuzzy-matched entries require the user to delete and retype — not suitable
    // for ghost text (which appends a suffix to the current buffer).
    if top_entry.match_type == MatchType::Fuzzy {
        return None;
    }

    // The word being completed is exactly what the request replaces.
    let target_word = buffer.get(req.replace_start..req.replace_end).unwrap_or("");

    if top_entry.name.starts_with(target_word) && top_entry.name.len() > target_word.len() {
        // Partial word: append the remaining suffix.
        Some(top_entry.name[target_word.len()..].to_string())
    } else if !buffer.ends_with(char::is_whitespace)
        && !buffer.contains(' ')
        && specs().any(|s| s.name == target_word && !s.subcommands.is_empty())
    {
        // The user has finished typing a known command name but hasn't pressed
        // space yet. Ghost the most relevant subcommand as " <subcommand>" so
        // Tab auto-completes to the full "command subcommand" string.
        Some(format!(" {}", top_entry.name))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ghost_suffix_from_cmd() {
        assert_eq!(ghost_suffix_from_cmd("gi"), Some("t".to_string()));
        assert_eq!(ghost_suffix_from_cmd("git st"), Some("atus".to_string()));
        assert_eq!(ghost_suffix_from_cmd("git p"), Some("ush".to_string()));
        assert_eq!(ghost_suffix_from_cmd("cargo "), Some("build".to_string()));
        assert_eq!(ghost_suffix_from_cmd("docker "), Some("run".to_string()));
        assert_eq!(ghost_suffix_from_cmd("kubectl "), Some("get".to_string()));
        assert_eq!(
            ghost_suffix_from_cmd("kubectl get "),
            Some("pods".to_string())
        );
        assert_eq!(
            ghost_suffix_from_cmd("terraform "),
            Some("init".to_string())
        );
        assert_eq!(ghost_suffix_from_cmd("aws s"), Some("3".to_string()));
        assert_eq!(ghost_suffix_from_cmd("drag"), Some("ent".to_string()));
        assert_eq!(ghost_suffix_from_cmd("dragent "), Some("workspaces".to_string()));
        assert_eq!(
            ghost_suffix_from_cmd("dragent automation"),
            Some("s".to_string())
        );
        assert_eq!(
            ghost_suffix_from_cmd("dragent workspaces "),
            Some("list".to_string())
        );
        assert_eq!(
            ghost_suffix_from_cmd("dragent agents send "),
            Some("--no-enter".to_string())
        );
    }

    #[test]
    fn every_dragent_command_is_suggested() {
        let dragent = specs().find(|s| s.name == "dragent").unwrap();
        for command in crate::cli::COMMANDS {
            assert!(
                dragent.subcommands.iter().any(|s| s.name == command),
                "`dragent {command}` is missing from cmd_specs_14"
            );
        }
        assert_eq!(ghost_suffix_from_cmd("dragent tas"), Some("ks".to_string()));
        assert_eq!(ghost_suffix_from_cmd("dragent tasks d"), Some("one".to_string()));
        assert_eq!(ghost_suffix_from_cmd("dragent msg in"), Some("box".to_string()));
        assert_eq!(ghost_suffix_from_cmd("dragent agents acc"), Some("ounts".to_string()));
    }

    #[test]
    fn every_agent_cli_is_suggested() {
        for agent in crate::workspace::PromptAgent::ALL {
            let Some(cli) = agent.cli() else { continue };
            assert!(specs().any(|s| s.name == cli), "agent CLI `{cli}` has no spec");
        }
        assert_eq!(ghost_suffix_from_cmd("cursor-ag"), Some("ent".to_string()));
        assert_eq!(ghost_suffix_from_cmd("qoderc"), Some("li".to_string()));
    }

    #[test]
    fn agent_flag_completes_agent_keys() {
        assert_eq!(
            ghost_suffix_from_cmd("dragent workspaces create --agent cod"),
            Some("ex".to_string())
        );
        assert_eq!(
            ghost_suffix_from_cmd("dragent team create x --agents claude,gem"),
            Some("ini".to_string())
        );
        assert_eq!(
            ghost_suffix_from_cmd("dragent agents start --agent=cur"),
            Some("sor-agent".to_string())
        );
    }
}
