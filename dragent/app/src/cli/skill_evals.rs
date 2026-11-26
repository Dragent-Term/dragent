//! Offline evaluation suite for Dragent's bundled skills: the static half.
//!
//! The live half (running a real agent through a scenario) is
//! `crates/dragent_host/skill-evals/run.py`. This module holds the checks
//! that run without a model:
//!
//! * every `dragent <family> <subcommand>` invocation inside code blocks of
//!   every bundled `SKILL.md` and `references/*.md` names a family this
//!   build has and a subcommand that family's help or parser accepts, with
//!   flags that command accepts;
//! * every `skill-evals/scenarios/*.json` file parses, names an existing
//!   skill, and its `expect`/`forbid` commands exist in this build;
//! * every skill has at least one positive scenario and the required
//!   boundary and failure-recovery cases are present.
//!
//! The allowed command tables below are checked against this build's own
//! help texts and parser sources (`allowed_tables_match_this_build`), so a
//! renamed command or flag fails loudly here instead of silently rotting.
//! Real drift found in a skill is not fixed here (skills are out of scope);
//! it is listed in `KNOWN_DRIFT` with a reason, which keeps the suite green
//! while the drift stays visible.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// ── Locations ────────────────────────────────────────────────────────────────

/// This crate's manifest dir (`dragent/app`); the evals live beside it.
fn app_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scenarios_dir() -> PathBuf {
    app_root().join("../crates/dragent_host/skill-evals/scenarios")
}

/// The skill packages as committed in this checkout. This reads the `skills/`
/// tree from disk rather than the embedded catalog on purpose: the checkout
/// is the source of truth, and these checks must verify exactly what is
/// committed here.
fn skills_root() -> PathBuf {
    app_root().join("../crates/dragent_host/skills")
}

/// `(skill name, relative path, file content)` for every `SKILL.md` and
/// `references/*.md` in this checkout.
fn skill_texts() -> Vec<(String, String, String)> {
    let root = skills_root();
    let mut skills: Vec<String> = std::fs::read_dir(&root)
        .unwrap_or_else(|_| panic!("skills dir missing: {}", root.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("SKILL.md").is_file())
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect();
    skills.sort();
    let mut texts = Vec::new();
    for skill in skills {
        let dir = root.join(&skill);
        let content = std::fs::read_to_string(dir.join("SKILL.md"))
            .unwrap_or_else(|_| panic!("cannot read {skill}/SKILL.md"));
        texts.push((skill.clone(), "SKILL.md".to_string(), content));
        let references = dir.join("references");
        if references.is_dir() {
            let mut stack = vec![references.clone()];
            while let Some(folder) = stack.pop() {
                let mut entries: Vec<PathBuf> = std::fs::read_dir(&folder)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.path())
                    .collect();
                entries.sort();
                for path in entries {
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "md") {
                        let relative = path
                            .strip_prefix(&dir)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/");
                        let content = std::fs::read_to_string(&path).unwrap_or_default();
                        texts.push((skill.clone(), relative, content));
                    }
                }
            }
        }
    }
    texts.sort();
    texts
}

fn bundled_skill_names() -> Vec<String> {
    let mut names: Vec<String> = skill_texts()
        .iter()
        .map(|(skill, _, _)| skill.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn app_source(relative: &str) -> PathBuf {
    app_root().join("src").join(relative)
}

fn host_source(relative: &str) -> PathBuf {
    app_root().join("../crates/dragent_host/src").join(relative)
}

// ── Command tables ───────────────────────────────────────────────────────────

/// Families that take no subcommand: bare `dragent <family>` (plus flags).
const BARE_FAMILIES: &[&str] = &["capabilities", "status", "version", "help", "mcp"];

/// Families whose invocation is family plus flags or positionals, with no
/// subcommand word (`dragent standup --json`, `dragent feedback --draft …`,
/// `dragent open PATH`, `dragent open-url URL`).
const NO_SUBCOMMAND_FAMILIES: &[&str] = &["standup", "doctor", "feedback", "open", "open-url"];

/// Flags every command accepts through the shared dispatcher or output
/// layer (`--json`/`--text`/`--quiet` in `cli/mod.rs` and
/// `team_command_support.rs`; `--idempotency-key` in `cli/idempotency.rs`;
/// `--help` reroutes to help).
const GLOBAL_FLAGS: &[&str] = &[
    "--json",
    "--text",
    "--quiet",
    "--help",
    "-h",
    "--idempotency-key",
];

/// `family -> subcommands this build accepts` (canonical names; aliases
/// are in `SUBCOMMAND_ALIASES`).
fn allowed_subcommands(family: &str) -> &'static [&'static str] {
    match family {
        "browser" => &[
            "list",
            "open",
            "status",
            "snapshot",
            "click",
            "hover",
            "type",
            "press",
            "scroll",
            "wait",
            "dialog",
            "eval",
            "console",
            "screenshot",
            "back",
            "forward",
            "reload",
        ],
        "computer" => &[
            "permissions",
            "capabilities",
            "apps",
            "windows",
            "snapshot",
            "click",
            "set-value",
            "perform-action",
            "type",
            "paste",
            "press-key",
            "hotkey",
            "key",
            "scroll",
            "drag",
            "focus",
            "menus",
            "menu",
            "window",
            "guide",
        ],
        "panes" => &[
            "list", "open", "move", "unsplit", "swap", "resize", "equalize", "focus", "close",
        ],
        "team" => &["create", "list", "show", "agents", "stop", "remove", "guide"],
        "tasks" => &[
            "list",
            "add",
            "show",
            "edit",
            "reassign",
            "cancel",
            "reopen",
            "done",
            "block",
            "wait",
            "start",
            "read",
            "keys",
            "diff",
            "integrate",
        ],
        "msg" => &["send", "reply", "inbox"],
        "issues" => &[
            "list", "show", "create", "update", "comment", "link", "delete", "restore",
        ],
        "integrations" => &["list", "describe", "search", "show", "comment", "status", "create", "api"],
        "automations" => &[
            "list",
            "get",
            "create",
            "update",
            "preview",
            "run",
            "run-get",
            "runs",
            "cancel",
            "pause",
            "resume",
            "delete",
            "prompt",
            "copy-files",
        ],
        "standup" => &["setup"],
        "setup" => &["check", "init", "launch", "status", "stop", "cleanup"],
        "plugins" => &[
            "inspect", "install", "list", "status", "check", "update", "disable", "enable",
            "accounts", "remove", "sync",
        ],
        "work" => &[
            "status",
            "ask",
            "report",
            "message",
            "handoff",
            "knowledge",
            "note",
            "submit",
        ],
        "agents" => &[
            "list", "running", "read", "send", "stop", "resume", "start-anyway", "start", "accounts",
        ],
        "workspaces" => &["list", "create", "delete"],
        "projects" => &["list", "add"],
        "settings" => &["list", "get", "set", "reset"],
        "theme" => &["list", "set"],
        "org" => &["list", "switch"],
        "machines" => &["list", "show"],
        "requests" => &["show"],
        _ => &[],
    }
}

/// `family -> flags this build accepts`, besides [`GLOBAL_FLAGS`].
fn allowed_flags(family: &str) -> &'static [&'static str] {
    match family {
        "browser" => &[
            "--submit",
            "--clear",
            "--new",
            "--pane",
            "--await",
            "--errors",
            "--idle",
            "--timeout",
            "--for",
            "--gone",
            "--selector",
            "--url",
            "--ref-gone",
        ],
        "computer" => &[
            "--app",
            "--window-id",
            "--window-index",
            "--restore-window",
            "--no-screenshot",
            "--element",
            "--element-index",
            "--window",
            "--mouse-button",
            "--click-count",
            "--from-element",
            "--to-element",
            "--from-element-index",
            "--to-element-index",
            "--action",
            "--name",
            "--modifiers",
            "--width",
            "--height",
            "--minimize",
            "--restore",
            "--zoom",
            "--button",
            "--direction",
            "--text",
            "--value",
            "--key",
            "--x",
            "--y",
            "--from-x",
            "--from-y",
            "--to-x",
            "--to-y",
            "--amount",
            "--count",
            "--stdin",
            "--text-stdin",
            "--value-stdin",
        ],
        "panes" => &[
            "--browser",
            "--terminal",
            "--right",
            "--left",
            "--down",
            "--below",
            "--up",
            "--above",
            "--tab",
            "--focus",
            "--url",
            "--label",
            "--target",
            "--beside",
            "--with",
            "--share",
        ],
        "team" => &[
            "--agents",
            "--model",
            "--cost",
            "--max-parallel",
            "--project",
            "--team",
        ],
        "tasks" => &[
            "--after",
            "--agent",
            "--model",
            "--owns",
            "--review",
            "--qa",
            "--integrated",
            "--title",
            "--prompt",
            "--prompt-file",
            "--feedback",
            "--note",
            "--summary",
            "--summary-file",
            "--file",
            "--check",
            "--approve",
            "--changes",
            "--patch",
            "--timeout",
            "--lines",
            "--keep-uncommitted",
            "--team",
        ],
        "msg" => &["--kind", "--reply", "--all", "--peek", "--team"],
        "issues" => &[
            "--status",
            "--assignee",
            "--label",
            "--project",
            "--search",
            "--title",
            "--description",
            "--description-file",
            "--priority",
            "--clear-assignee",
            "--pr",
            "--item",
        ],
        "integrations" => &["--body", "--body-file", "--title", "--in", "--project", "--account", "--query"],
        "automations" => &[
            "--name",
            "--prompt",
            "--prompt-file",
            "--file",
            "--schedule",
            "--timezone",
            "--count",
            "--limit",
            "--from",
            "--missed",
            "--overlap",
            "--max-retries",
            "--retry-delay",
            "--retention",
            "--agent",
            "--project",
            "--session",
            "--workspace",
            "--base-branch",
            "--push",
            "--branch",
            "--path",
            "--continue-session",
            "--watch",
            "--watch-query",
            "--check-every",
            "--no-watch",
            "--services",
            "--tag",
            "--no-tags",
            "--paused",
            "--set",
        ],
        "doctor" => &["--check", "--project"],
        "plugins" => &["--ref", "--tool", "--args", "--server", "--account", "--from"],
        "standup" => &[
            "--project",
            "--since",
            "--limit",
            "--setup",
            "--text",
            "--json",
            "--quiet",
        ],
        "work" => &["--kind", "--summary", "--summary-file"],
        "agents" => &[
            "--agent",
            "--lines",
            "--no-enter",
            "--workspace",
            "--prompt",
            "--prompt-file",
            "--model",
            "--from",
            "--account",
            "--project",
        ],
        "workspaces" => &[
            "--project",
            "--agent",
            "--prompt",
            "--prompt-file",
            "--name",
            "--branch",
            "--base-branch",
            "--pr",
            "--local",
            "--command",
            "--tag",
            "--model",
            "--force",
        ],
        "projects" => &["--into", "--project"],
        "feedback" => &[
            "--kind",
            "--observed",
            "--expected",
            "--steps",
            "--frequency",
            "--cause",
            "--evidence",
            "--body",
            "--body-file",
            "--draft",
        ],
        // Families handled in `cli/mod.rs` or with no extra flags in skills.
        "requests" | "status" | "capabilities" | "settings" | "theme" | "org" | "machines"
        | "open" | "open-url" | "mcp" => &[],
        _ => &[],
    }
}

/// `(family, alias, canonical)`: alternate subcommand spellings the parsers
/// accept. Each entry is checked against the parser source, so a removed
/// alias fails the table test instead of silently passing skills.
const SUBCOMMAND_ALIASES: &[(&str, &str, &str)] = &[
    ("browser", "goto", "open"),
    ("browser", "navigate", "open"),
    ("browser", "read", "snapshot"),
    ("browser", "fill", "type"),
    ("browser", "js", "eval"),
    ("browser", "logs", "console"),
    ("computer", "list-apps", "apps"),
    ("computer", "list-windows", "windows"),
    ("computer", "get-app-state", "snapshot"),
    ("computer", "type-text", "type"),
    ("computer", "paste-text", "paste"),
    ("computer", "perform-secondary-action", "perform_action"),
    ("panes", "ls", "list"),
    ("panes", "new", "open"),
    ("panes", "split", "open"),
    ("panes", "show", "move"),
    ("panes", "detach", "unsplit"),
    ("panes", "even", "equalize"),
    ("team", "new", "create"),
    ("team", "start", "create"),
    ("team", "status", "show"),
    ("team", "forget", "remove"),
    ("team", "playbook", "guide"),
    ("tasks", "new", "add"),
    ("tasks", "change", "edit"),
    ("tasks", "move", "reassign"),
    ("tasks", "redo", "reopen"),
    ("tasks", "finish", "done"),
    ("tasks", "submit", "done"),
    ("tasks", "blocked", "block"),
    ("tasks", "run", "start"),
    ("tasks", "apply", "integrate"),
    ("tasks", "screen", "read"),
    ("tasks", "press", "keys"),
    ("integrations", "services", "list"),
    ("integrations", "find", "search"),
    ("integrations", "read", "show"),
    ("integrations", "get", "show"),
    ("integrations", "move", "status"),
    ("integrations", "new", "create"),
    ("automations", "run_get", "run-get"),
    ("automations", "show-run", "run-get"),
    ("workspaces", "new", "create"),
    ("workspaces", "remove", "delete"),
];

/// `(family, alias, canonical)`: alternate family spellings the dispatcher
/// accepts (`KNOWN_COMMANDS` plus the plural/singular pairs).
const FAMILY_ALIASES: &[(&str, &str)] = &[
    ("pane", "panes"),
    ("teams", "team"),
    ("task", "tasks"),
    ("message", "msg"),
    ("integration", "integrations"),
    ("automation", "automations"),
    ("workspace", "workspaces"),
    ("project", "projects"),
    ("agent", "agents"),
    ("request", "requests"),
];

fn canonical_family(family: &str) -> &str {
    FAMILY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == family)
        .map(|(_, canonical)| *canonical)
        .unwrap_or(family)
}

fn canonical_subcommand(family: &str, sub: &str) -> String {
    // Underscore and hyphen spellings reach the same tool (`set_value` /
    // `set-value`, `run_get` / `run-get`).
    let normalized = sub.replace('_', "-");
    SUBCOMMAND_ALIASES
        .iter()
        .find(|(alias_family, alias, _)| *alias_family == family && *alias == normalized)
        .map(|(_, _, canonical)| (*canonical).to_string())
        .unwrap_or(normalized)
}

/// Real drift found in a skill: `(file suffix, invocation prefix, why it is
/// wrong and what should happen instead)`. Skills themselves are out of
/// scope for this task, so entries here keep the suite green while the
/// drift stays visible. An entry that matches nothing fails the suite, so
/// fixed skills must be unlisted here.
const KNOWN_DRIFT: &[(&str, &str, &str)] = &[
    // No known drift: every bundled skill invocation currently resolves.
];

// ── Help texts and parser sources ────────────────────────────────────────────

/// This build's own help text for `family`, from the same `run` functions
/// the dispatcher calls with `help`. Side-effect free: every branch below
/// returns help before touching the app or any store.
fn family_help(family: &str) -> String {
    let help = vec!["help".to_string()];
    match family {
        "browser" => super::browser::run(&help).unwrap_or_default(),
        "computer" => super::computer::run(&help).unwrap_or_default(),
        "panes" => super::panes::run(&help).unwrap_or_default(),
        "work" => super::work::run(&help).unwrap_or_default(),
        "standup" => super::standup::run(&help).unwrap_or_default(),
        "doctor" => super::doctor::run(&["--help".to_string()]).unwrap_or_default(),
        "setup" => super::setup::run(&help).unwrap_or_default(),
        "plugins" => super::plugins::run(&help).unwrap_or_default(),
        "team" => super::team::run("team", &help).unwrap_or_default(),
        "tasks" => super::team::run("tasks", &help).unwrap_or_default(),
        "msg" => super::team::run("msg", &help).unwrap_or_default(),
        "integrations" => super::integrations::run(&help).unwrap_or_default(),
        "automations" => crate::automations::cli::run(&help).unwrap_or_default(),
        "feedback" => super::account::feedback(&["--help".to_string()]).unwrap_or_default(),
        _ => super::HELP.to_string(),
    }
}

/// Parser sources that accept the family's subcommands and flags, for the
/// cases a one-line help summary does not spell out (e.g. automations
/// `--from`, `runs <id> <run-id>`).
fn family_sources(family: &str) -> Vec<PathBuf> {
    match family {
        "browser" => vec![app_source("cli/browser.rs")],
        "computer" => vec![app_source("cli/computer.rs")],
        "panes" => vec![app_source("cli/panes.rs")],
        "work" => vec![app_source("cli/work.rs")],
        "standup" => vec![app_source("cli/standup.rs")],
        "doctor" => vec![app_source("cli/doctor.rs")],
        "setup" => vec![app_source("cli/setup.rs")],
        "plugins" => vec![app_source("cli/plugins.rs")],
        "team" | "tasks" | "msg" => vec![host_source("team_commands.rs")],
        "integrations" => vec![host_source("integration_commands.rs")],
        "automations" => vec![app_source("automations/cli.rs")],
        "agents" => vec![app_source("cli/agents.rs")],
        "feedback" => vec![app_source("cli/account.rs")],
        // Handled in `cli/mod.rs` (families, subcommands and `Flags`).
        _ => vec![app_source("cli/mod.rs")],
    }
}

fn read_sources(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        out.push_str(&std::fs::read_to_string(path).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// Every family in the capability report plus the top-level-only commands.
fn known_families() -> BTreeSet<String> {
    let mut families = BTreeSet::new();
    let report = super::capabilities::report();
    if let Some(commands) = report["commands"].as_array() {
        for command in commands {
            if let Some(name) = command["name"].as_str() {
                families.insert(name.to_string());
            }
        }
    }
    families.insert("help".to_string());
    families.insert("version".to_string());
    // `capabilities --json` lists `mcp` under commands already; keep the
    // alias spellings the dispatcher accepts out of this set: they resolve
    // through `canonical_family`.
    families
}

// ── Extracting invocations from skills ───────────────────────────────────────

struct Invocation {
    file: String,
    line: usize,
    family: String,
    subcommand: Option<String>,
    flags: Vec<String>,
    raw: String,
}

/// Split a command line the way a shell would, for tokenizing skill
/// examples. Quotes group words; nothing else is special here.
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for ch in line.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                in_word = true;
            }
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            _ => {
                current.push(ch);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// Cut a `#` comment outside quotes (`#` starts a comment only after
/// whitespace, so ids like `team-1a2b` and URLs survive).
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (index, ch) in line.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '#' && previous.is_whitespace() {
            return line[..index].trim_end();
        }
        previous = ch;
    }
    line
}

/// Every `dragent …` invocation inside fenced code blocks of the bundled
/// skill texts (`SKILL.md` plus `references/*.md`), as committed in this
/// checkout (see [`skill_texts`]).
fn skill_invocations() -> Vec<Invocation> {
    let mut invocations = Vec::new();
    for (skill, relative, content) in skill_texts() {
        let name = format!("{skill}/{relative}");
        // Join backslash continuations first, keeping the start line.
        let mut logical: Vec<(usize, String)> = Vec::new();
        let mut in_block = false;
        let mut pending: Option<(usize, String)> = None;
        for (index, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_block = !in_block;
                if !in_block {
                    if let Some((start, text)) = pending.take() {
                        logical.push((start, text));
                    }
                }
                continue;
            }
            if !in_block {
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((start, mut text)) = pending.take() {
                text.push(' ');
                text.push_str(trimmed);
                if text.ends_with('\\') {
                    text.pop();
                    pending = Some((start, text));
                } else {
                    logical.push((start, text));
                }
            } else if trimmed.ends_with('\\') {
                let mut text = trimmed.to_string();
                text.pop();
                pending = Some((index + 1, text));
            } else {
                logical.push((index + 1, trimmed.to_string()));
            }
        }
        if let Some((start, text)) = pending.take() {
            logical.push((start, text));
        }
        for (start, line) in logical {
            // A shell pipe after the command (`… | head -60`) is not
            // part of the invocation.
            let line = match line.find(" | ") {
                Some(index) => line[..index].to_string(),
                None => line,
            };
            let line = strip_comment(line.trim()).trim().to_string();
            if !line.starts_with("dragent ") {
                continue;
            }
            let mut words = split_words(&line);
            // `dragent --dragent-cli …` is how launchers call the binary.
            if words.get(1).is_some_and(|word| word == super::CLI_ARG) {
                words.remove(1);
            }
            if words.len() < 2 {
                continue;
            }
            let family = canonical_family(&words[1]).to_string();
            let mut subcommand = None;
            let mut flags = Vec::new();
            let rest = &words[2..];
            let values = match rest.first() {
                Some(first) if !first.starts_with("--") => {
                    subcommand = Some(first.clone());
                    &rest[1..]
                }
                _ => rest,
            };
            for word in values {
                if let Some(flag) = word.strip_prefix("--") {
                    let flag = flag.split('=').next().unwrap_or(flag);
                    flags.push(format!("--{flag}"));
                }
            }
            invocations.push(Invocation {
                file: name.clone(),
                line: start,
                family,
                subcommand,
                flags,
                raw: line,
            });
        }
    }
    invocations
}

// ── Scenario files ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct Scenario {
    id: String,
    request: String,
    expected_skill: String,
    must_not_use_skills: Vec<String>,
    fixture: String,
    expect: Vec<String>,
    forbid: Vec<String>,
    success: String,
    notes: String,
}

const FIXTURES: &[&str] = &[
    "empty",
    "page",
    "server",
    "team-context",
    "service-results",
    "no-app",
];

/// Boundary and failure-recovery cases the suite must contain (spec names).
const REQUIRED_SCENARIOS: &[(&str, &str)] = &[
    (
        "panes-arrange-no-team",
        "arrange panes without starting a team",
    ),
    (
        "standup-recap-no-messages",
        "summarize existing workers without messaging them",
    ),
    (
        "browser-embedded-not-computer",
        "test an embedded page rather than desktop control",
    ),
    (
        "computer-native-app",
        "operate an external native app through desktop control",
    ),
    (
        "integrations-read-only",
        "read an issue without posting a comment",
    ),
    (
        "doctor-unavailable-service",
        "diagnose an unavailable service without reinstalling everything",
    ),
    (
        "automate-draft-paused",
        "draft an automation without enabling future runs",
    ),
    (
        "page-update-no-duplicate",
        "update a page without creating a duplicate",
    ),
    (
        "orchestrate-takeover-no-create",
        "recover a team without starting another lead",
    ),
    (
        "skills-update-keeps-custom",
        "preserve customized skill resources during an update",
    ),
    (
        "panes-timeout-requests-show",
        "a timed-out command checked with requests show before retrying",
    ),
    ("browser-stale-ref", "a stale pane/element id"),
    (
        "integrations-ambiguous-search",
        "an ambiguous search result",
    ),
    ("integrations-pagination", "pagination"),
    (
        "orchestrate-two-agents-same-team",
        "two agents acting on the same team",
    ),
    ("integrate-conflict-recovery", "an integration conflict"),
];

fn load_scenarios() -> BTreeMap<String, Vec<Scenario>> {
    let dir = scenarios_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("scenarios dir missing: {}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no scenario files in {}", dir.display());
    let mut scenarios = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("cannot read {}", file.display()));
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("invalid JSON: {}", file.display()));
        let list = parsed.as_array().cloned().unwrap_or_else(|| vec![parsed]);
        let mut decoded = Vec::new();
        for (index, value) in list.iter().enumerate() {
            decoded.push(
                serde_json::from_value::<Scenario>(value.clone())
                    .unwrap_or_else(|error| panic!("{}[{}]: {error}", file.display(), index)),
            );
        }
        scenarios.insert(file.display().to_string(), decoded);
    }
    scenarios
}

fn check_command(
    families: &BTreeSet<String>,
    command: &str,
    context: &str,
    errors: &mut Vec<String>,
) {
    let mut words = command.split_whitespace();
    let Some(family) = words.next() else {
        errors.push(format!("{context}: empty command"));
        return;
    };
    let family = canonical_family(family);
    if !families.contains(family) {
        errors.push(format!("{context}: unknown command family in `{command}`"));
        return;
    }
    let Some(sub) = words.next() else {
        return;
    };
    if sub.starts_with("--") || NO_SUBCOMMAND_FAMILIES.contains(&family) {
        return;
    }
    let canonical = canonical_subcommand(family, sub);
    if !allowed_subcommands(family).contains(&canonical.as_str()) {
        errors.push(format!(
            "{context}: unknown subcommand `{sub}` for `{family}` in `{command}`"
        ));
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn skill_evals_dir_is_not_a_skill_package() {
    // `crates/dragent_host/build.rs` only embeds `skills/`; the evals must
    // never become an installable skill.
    assert!(
        !dragent_host::agent_skills::BUNDLED_SKILLS
            .iter()
            .any(|skill| skill.name == "skill-evals"),
        "skill-evals must not be embedded as a skill package"
    );
    assert!(
        !app_root()
            .join("../crates/dragent_host/skill-evals/SKILL.md")
            .exists(),
        "skill-evals must not grow a SKILL.md"
    );
}

#[test]
fn allowed_tables_match_this_build() {
    // The tables above are only as honest as this check: every allowed
    // family, subcommand and flag must appear in that family's own help
    // text or parser source.
    let families = known_families();
    let mut errors = Vec::new();
    for probe in [
        "browser",
        "computer",
        "panes",
        "team",
        "tasks",
        "msg",
        "integrations",
        "automations",
        "standup",
        "doctor",
        "setup",
        "plugins",
        "work",
        "agents",
        "workspaces",
        "projects",
        "settings",
        "theme",
        "org",
        "machines",
        "requests",
        "feedback",
    ] {
        let canonical = canonical_family(probe);
        if !families.contains(canonical) && !BARE_FAMILIES.contains(&canonical) {
            errors.push(format!(
                "allowed family `{canonical}` is not a command in this build"
            ));
            continue;
        }
        let help = family_help(canonical);
        let source = read_sources(&family_sources(canonical));
        for sub in allowed_subcommands(canonical) {
            if !help.contains(sub) && !source.contains(&format!("\"{sub}\"")) {
                errors.push(format!(
                    "allowed subcommand `{canonical} {sub}` is in neither its help nor its parser source"
                ));
            }
        }
        for alias in SUBCOMMAND_ALIASES
            .iter()
            .filter(|(family, ..)| *family == canonical)
        {
            if !source.contains(&format!("\"{}\"", alias.1)) {
                errors.push(format!(
                    "alias `{} {}` is not in its parser source",
                    alias.0, alias.1
                ));
            }
        }
        for flag in allowed_flags(canonical) {
            let bare = flag.trim_start_matches('-');
            if !help.contains(flag)
                && !source.contains(flag)
                && !source.contains(&format!("\"{bare}\""))
            {
                errors.push(format!(
                    "allowed flag `{flag}` for `{canonical}` is in neither its help nor its parser source"
                ));
            }
        }
    }
    for flag in GLOBAL_FLAGS {
        // Sources store the stripped name (`help`) or the full flag
        // (`--idempotency-key`); accept either spelling.
        let bare = flag.trim_start_matches('-');
        let quoted = format!("\"{bare}\"");
        let sources = read_sources(&[
            app_source("cli/mod.rs"),
            app_source("cli/idempotency.rs"),
            host_source("team_command_support.rs"),
        ]);
        if !super::HELP.contains(flag) && !sources.contains(flag) && !sources.contains(&quoted) {
            errors.push(format!(
                "global flag `{flag}` matches no shared parsing source"
            ));
        }
    }
    assert!(
        errors.is_empty(),
        "skill-evals command tables drifted from this build:\n{}",
        errors.join("\n")
    );
}

#[test]
fn skills_use_only_known_commands() {
    let families = known_families();
    let invocations = skill_invocations();
    assert!(
        !invocations.is_empty(),
        "no dragent invocations extracted from bundled skills"
    );
    let mut errors = Vec::new();
    for invocation in &invocations {
        let context = format!("{}:{}", invocation.file, invocation.line);
        if !families.contains(&invocation.family) {
            errors.push(format!(
                "{context}: unknown family `{}` in `{}`",
                invocation.family, invocation.raw
            ));
            continue;
        }
        if BARE_FAMILIES.contains(&invocation.family.as_str()) {
            continue;
        }
        match &invocation.subcommand {
            None => {
                if !NO_SUBCOMMAND_FAMILIES.contains(&invocation.family.as_str()) {
                    errors.push(format!(
                        "{context}: `{}` needs a subcommand in `{}`",
                        invocation.family, invocation.raw
                    ));
                }
            }
            Some(sub) => {
                // Every family answers its own help; `help` as a subcommand
                // is valid everywhere it appears in the skills.
                if !matches!(sub.as_str(), "help" | "--help" | "-h") {
                    let canonical = canonical_subcommand(&invocation.family, sub);
                    if !allowed_subcommands(&invocation.family).contains(&canonical.as_str()) {
                        errors.push(format!(
                            "{context}: unknown subcommand `{sub}` for `{}` in `{}`",
                            invocation.family, invocation.raw
                        ));
                    }
                }
            }
        }
        for flag in &invocation.flags {
            if !GLOBAL_FLAGS.contains(&flag.as_str())
                && !allowed_flags(&invocation.family).contains(&flag.as_str())
            {
                errors.push(format!(
                    "{context}: unknown flag `{flag}` for `{}` in `{}`",
                    invocation.family, invocation.raw
                ));
            }
        }
    }
    let mut unlisted = Vec::new();
    for error in errors {
        let known = KNOWN_DRIFT.iter().any(|(file, prefix, _)| {
            invocations.iter().any(|invocation| {
                invocation.file.ends_with(file)
                    && invocation.raw.starts_with(prefix)
                    && error.contains(&invocation.raw)
            })
        });
        if !known {
            unlisted.push(error);
        }
    }
    // Every allowlist entry must still match something: fixed skills are
    // unlisted instead of going stale.
    for (file, prefix, reason) in KNOWN_DRIFT {
        let stale = !invocations.iter().any(|invocation| {
            invocation.file.ends_with(file) && invocation.raw.starts_with(prefix)
        });
        if stale {
            unlisted.push(format!(
                "stale allowlist entry {file} `{prefix}…`: {reason}"
            ));
        }
    }
    assert!(
        unlisted.is_empty(),
        "unknown commands in bundled skills:\n{}",
        unlisted.join("\n")
    );
}

#[test]
fn scenarios_parse_and_reference_real_commands() {
    let families: BTreeSet<String> = known_families();
    let skills: BTreeSet<String> = bundled_skill_names().into_iter().collect();
    let scenarios = load_scenarios();
    let mut errors = Vec::new();
    let mut ids = BTreeSet::new();
    for (file, list) in &scenarios {
        for scenario in list {
            let context = format!("{file} scenario `{}`", scenario.id);
            if scenario.id.trim().is_empty() {
                errors.push(format!("{context}: empty id"));
            }
            if !ids.insert(scenario.id.clone()) {
                errors.push(format!("{context}: duplicate scenario id"));
            }
            for field in [&scenario.request, &scenario.success, &scenario.notes] {
                if field.trim().is_empty() {
                    errors.push(format!("{context}: empty request/success/notes"));
                    break;
                }
            }
            if !skills.contains(&scenario.expected_skill) {
                errors.push(format!(
                    "{context}: unknown expected_skill `{}`",
                    scenario.expected_skill
                ));
            }
            for skill in &scenario.must_not_use_skills {
                if !skills.contains(skill) {
                    errors.push(format!("{context}: unknown must_not_use_skill `{skill}`"));
                }
                if *skill == scenario.expected_skill {
                    errors.push(format!("{context}: expected_skill is also forbidden"));
                }
            }
            if !FIXTURES.contains(&scenario.fixture.as_str()) {
                errors.push(format!("{context}: unknown fixture `{}`", scenario.fixture));
            }
            if scenario.expect.is_empty() {
                errors.push(format!("{context}: empty expect list"));
            }
            for command in scenario.expect.iter().chain(scenario.forbid.iter()) {
                check_command(&families, command, &context, &mut errors);
            }
        }
    }
    assert!(
        errors.is_empty(),
        "invalid scenario files:\n{}",
        errors.join("\n")
    );
}

#[test]
fn scenarios_cover_every_skill_and_boundary() {
    let scenarios = load_scenarios();
    let covered: BTreeSet<&str> = scenarios
        .values()
        .flatten()
        .map(|scenario| scenario.expected_skill.as_str())
        .collect();
    let mut errors = Vec::new();
    for skill in bundled_skill_names() {
        if !covered.contains(skill.as_str()) {
            errors.push(format!("skill `{skill}` has no positive scenario"));
        }
    }
    let ids: BTreeSet<&str> = scenarios
        .values()
        .flatten()
        .map(|scenario| scenario.id.as_str())
        .collect();
    for (id, what) in REQUIRED_SCENARIOS {
        if !ids.contains(id) {
            errors.push(format!("missing scenario `{id}` ({what})"));
        }
    }
    assert!(
        errors.is_empty(),
        "scenario coverage gaps:\n{}",
        errors.join("\n")
    );
}
