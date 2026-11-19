//! Client-side approval policy for agent tool calls.
//!
//! Whether a call asks first depends on the user's approval mode (Settings
//! → Agent → General) and on what the call does ([`effect`]), decided here
//! the same way whichever route the model runs on, through Dragent's
//! servers or straight to a provider with the user's own key:
//!
//! * **Ask** — file changes and actions (shell commands, moving or deleting
//!   files, delegating, pull requests, posts to connected services) ask.
//! * **Accept edits** — file edits run; actions ask.
//! * **Don't ask** — nothing asks.
//!
//! Reads never ask, with one exception that no mode waives: reading a file
//! *outside the project directory* prompts, protecting secrets outside the
//! workspace (~/.ssh, other projects). Persisted always-deny rules also
//! still apply — the controller enforces them before this policy is
//! consulted.
//!
//! Independently, "Auto-run safe commands" lets read-only shell commands
//! (`ls`, `cat`, `git status` …) through without asking; switching it off
//! makes every shell command ask (except in Don't ask).

use super::autonomy::{AgentAutonomySettings, ApprovalMode};

/// Tools that change files or system state: the read-only and plan gates refuse them.
const MUTATING_TOOLS: &[&str] = &[
    "run_command",
    "write_file",
    "edit_file",
    "batch_edit",
    "apply_patch",
    "create_directory",
    "move_file",
    "delete_file",
    "github_create_pull_request",
    "integrations_comment",
    "integrations_set_status",
    "integrations_create",
    "integrations_api",
    "issues_create",
    "issues_update",
    "issues_comment",
    "issues_link",
];

/// Tools that post to a connected service (Linear, Jira, Slack, …) under
/// the user's account, where their team sees it. `integrations_api` calls a
/// service's own API, which can write: it's treated as one whatever the call.
pub const SERVICE_WRITE_TOOLS: &[&str] = &[
    "integrations_comment",
    "integrations_set_status",
    "integrations_create",
    "integrations_api",
    "issues_create",
    "issues_update",
    "issues_comment",
    "issues_link",
];

pub fn is_service_write(tool_name: &str) -> bool {
    SERVICE_WRITE_TOOLS.contains(&tool_name)
}

/// Returns whether a tool can change files, execute arbitrary commands, or
/// delegate work that may mutate the workspace.
pub fn is_mutating_tool(tool_name: &str, command: Option<&str>) -> bool {
    if tool_name == "computer" {
        return !matches!(command, Some("permissions" | "capabilities" | "apps" | "windows" | "snapshot"));
    }
    if tool_name == "run_command" {
        return !command.map(is_safe_readonly_command).unwrap_or(false);
    }
    MUTATING_TOOLS.contains(&tool_name)
        || matches!(tool_name, "repl" | "spawn_subagent" | "apply_patch")
}

/// Runtime capability gate shared by all controller modes. Prompt text is
/// guidance; this decision is enforced before a local tool can start.
pub fn capability_denial(
    tool_name: &str,
    command: Option<&str>,
    read_only: bool,
    plan_gate: bool,
) -> Option<&'static str> {
    if read_only && is_mutating_tool(tool_name, command) {
        return Some("Tool refused: the current mode is read-only.");
    }
    if plan_gate && is_mutating_tool(tool_name, command) {
        return Some(
            "Tool refused: this mode requires an approved plan before changes or commands can run.",
        );
    }
    None
}

/// First tokens of shell commands that only read state. A command qualifies
/// only when it has no shell metacharacters (pipes, redirects, substitution)
/// that could smuggle in a write.
const SAFE_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "less", "more", "pwd", "which", "whereis", "file", "stat", "wc",
    "du", "df", "ps", "printenv", "date", "whoami", "uname", "hostname", "id", "echo", "grep",
    "rg", "fd", "find", "tree", "basename", "dirname", "realpath", "readlink", "uptime", "history",
];
// `env` is deliberately absent: `env <command>` executes an arbitrary program.

/// git subcommands that never mutate the repo.
const SAFE_GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "tag",
    "remote",
    "blame",
    "shortlog",
    "describe",
    "rev-parse",
    "ls-files",
    "ls-remote",
    "reflog",
];

/// Shell metacharacters that can chain or redirect — a command containing any
/// of these is never auto-classified as read-only, even if the first token is.
fn has_shell_metacharacters(command: &str) -> bool {
    command.contains(';')
        || command.contains("&&")
        || command.contains("||")
        || command.contains('|')
        || command.contains('>')
        || command.contains('<')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('&')
}

/// Conservative read-only classification of a shell command line.
pub fn is_safe_readonly_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || has_shell_metacharacters(trimmed) {
        return false;
    }
    let mut tokens = trimmed.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    // Strip a leading path ("/bin/ls" → "ls").
    let program = first.rsplit('/').next().unwrap_or(first);

    // Allowlisted programs whose *flags* can still mutate or execute.
    match program {
        // find's action primaries delete files, write files, or run programs.
        "find" => {
            const FIND_MUTATING: &[&str] = &[
                "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf",
                "-fls",
            ];
            if trimmed
                .split_whitespace()
                .any(|t| FIND_MUTATING.contains(&t))
            {
                return false;
            }
        }
        // fd can execute a command per result.
        "fd" => {
            if trimmed.split_whitespace().any(|t| {
                matches!(t, "-x" | "--exec" | "-X" | "--exec-batch")
                    || t.starts_with("--exec=")
                    || t.starts_with("--exec-batch=")
            }) {
                return false;
            }
        }
        _ => {}
    }

    if program == "git" {
        // Skip global flags to find the subcommand; -C/-c/--git-dir/--work-tree
        // take a value token that must be skipped too.
        let mut skip_value = false;
        for token in tokens {
            if skip_value {
                skip_value = false;
                continue;
            }
            if let Some(flag) = token.strip_prefix('-') {
                let flag = flag.trim_start_matches('-');
                skip_value =
                    matches!(flag, "C" | "c" | "git-dir" | "work-tree") && !token.contains('=');
                continue;
            }
            return SAFE_GIT_SUBCOMMANDS.contains(&token);
        }
        return false;
    }
    SAFE_COMMANDS.contains(&program)
}

/// Database CLI programs. A worker running one of these against a real
/// database is a side effect no Work Contract pre-authorization should be
/// able to wave through silently.
const DATABASE_PROGRAMS: &[&str] = &[
    "psql",
    "mysql",
    "mariadb",
    "mongosh",
    "mongo",
    "redis-cli",
    "sqlite3",
    "sqlcmd",
];

/// Project migration-runner invocations, matched as substrings since they're
/// always run through another program (`npm run`, `python manage.py`, a
/// local binary) rather than as a program name of their own.
const MIGRATION_MARKERS: &[&str] = &[
    "db:migrate",
    "migrate:latest",
    "migrate:up",
    "prisma migrate",
    "alembic upgrade",
    "manage.py migrate",
    "knex migrate",
    "typeorm migration:run",
    "rails db:migrate",
];

/// Whether a shell command line runs a database client or a project's own
/// migration tooling against a real database.
fn is_database_command(command: &str) -> bool {
    let trimmed = command.trim();
    if let Some(first) = trimmed.split_whitespace().next() {
        let program = first.rsplit('/').next().unwrap_or(first);
        if DATABASE_PROGRAMS.contains(&program) {
            return true;
        }
    }
    MIGRATION_MARKERS
        .iter()
        .any(|marker| trimmed.contains(marker))
}

/// Command patterns whose effect is either irreversible at a scale beyond a
/// single task, or steps outside the isolated per-agent worktree the whole
/// review-then-merge pipeline depends on. `git push` is included even
/// without `--force`: any push reaches a real remote directly, bypassing the
/// user's own review-and-accept step entirely, not just the local worktree.
fn is_breaking_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    if lower.contains("git ") && lower.contains("push") {
        return true;
    }
    const BREAKING_MARKERS: &[&str] = &[
        "rm -rf /",
        "rm -rf ~",
        "rm -rf *",
        "rm -rf .git",
        " mkfs",
        "dd if=",
        ":(){ :|:& };:",
    ];
    BREAKING_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Absolute ceiling on what a Work Contract's pre-authorization may waive.
/// Everything else — reads, in-scope edits, commands within the task's write
/// scope — can be pre-approved once during Shaping so coordination doesn't
/// ping the user for routine work. These cannot: external side effects
/// (a PR bypasses the user's own review step), real database writes, file
/// deletion, and shell commands that are destructive at a scale beyond this
/// task or that reach outside the isolated worktree. A live decision is
/// required every time, no matter what was pre-approved at planning time.
pub fn is_always_high_risk(tool_name: &str, command: Option<&str>) -> bool {
    match tool_name {
        "github_create_pull_request" | "delete_file" => true,
        name if is_service_write(name) => true,
        "run_command" => command
            .is_some_and(|command| is_database_command(command) || is_breaking_command(command)),
        _ => false,
    }
}

/// What a tool call does, as far as asking first is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Reads, searches and the agent's own bookkeeping.
    Read,
    /// Changes files in the project.
    Edit,
    /// Runs something or reaches past the project: shell commands, moving or
    /// deleting files, delegating, pull requests, posts to connected
    /// services, and any tool Dragent doesn't know.
    Act,
}

/// Built-in tools that only read or keep the agent's own notes.
const READ_TOOLS: &[&str] = &[
    "read_file",
    "read_multiple_files",
    "list_directory",
    "list_directory_with_sizes",
    "directory_tree",
    "search_files",
    "get_file_info",
    "list_allowed_directories",
    "grep",
    "search_codebase",
    "fetch_url",
    "web_search",
    "skill",
    "sleep",
    "manage_task",
    "mcp_auth",
    "ask_user",
    "todowrite",
];

/// Built-in tools that change files inside the project.
const EDIT_TOOLS: &[&str] = &[
    "write_file",
    "edit_file",
    "batch_edit",
    "apply_patch",
    "create_directory",
];

/// The coordination lead's tools that only look.
const COORDINATION_READS: &[&str] = &[
    "team_guide",
    "team_status",
    "team_wait",
    "task_keys",
    "task_read",
    "task_show",
    "msg_inbox",
];

/// Words in an MCP server's tool names that mark a read, as Dragent's
/// servers judge them.
const MCP_READ_WORDS: &[&str] = &[
    "read", "search", "list", "get", "status", "info", "logs", "whoami", "doctor",
];

/// What a call does. `command` is the shell command line of `run_command`.
pub fn effect(tool_name: &str, command: Option<&str>) -> Effect {
    if tool_name == "run_command" {
        return if command.is_some_and(is_safe_readonly_command) {
            Effect::Read
        } else {
            Effect::Act
        };
    }
    if READ_TOOLS.contains(&tool_name)
        || tool_name.starts_with("prompt__")
        || tool_name.starts_with("resource__")
    {
        return Effect::Read;
    }
    if EDIT_TOOLS.contains(&tool_name) {
        return Effect::Edit;
    }
    if tool_name.starts_with("integrations_") && !is_service_write(tool_name) {
        return Effect::Read;
    }
    if let Some(name) = tool_name.strip_prefix("coord_") {
        if COORDINATION_READS.contains(&name) {
            return Effect::Read;
        }
    }
    if tool_name.starts_with("mcp__") {
        let lower = tool_name.to_ascii_lowercase();
        if MCP_READ_WORDS.iter().any(|word| lower.contains(word)) {
            return Effect::Read;
        }
    }
    Effect::Act
}

/// Decide whether a tool call must pause for user approval.
///
/// `command` is the shell command line for `run_command` calls (None for
/// other tools). `out_of_project_read` marks a read reaching outside the
/// workspace, which asks in every mode.
pub fn approval_required(
    settings: &AgentAutonomySettings,
    tool_name: &str,
    command: Option<&str>,
    out_of_project_read: bool,
) -> bool {
    // Computer use has its own app allowlist and per-action confirmation.
    // Capability/plan gates and persisted deny still run before this decision.
    if tool_name == "computer" {
        return out_of_project_read;
    }
    if out_of_project_read {
        return true;
    }
    let mode = settings.approval_mode();
    // Every shell command asks when the auto-run grant is off.
    if tool_name == "run_command" && !settings.auto_run_safe_commands && mode != ApprovalMode::Auto
    {
        return true;
    }
    match (mode, effect(tool_name, command)) {
        (_, Effect::Read) | (ApprovalMode::Auto, _) => false,
        (ApprovalMode::AcceptEdits, Effect::Edit) => false,
        _ => true,
    }
}

/// Declarative approval evaluation with attribution (ruleset parity at the
/// decision level): first matching rule wins, and every outcome names the
/// rule that produced it, so a surprising prompt/denial/auto-run is traceable
/// in logs. Enforcement points (capability gates, plan gate, approval cards,
/// auto-reject posts) stay where they are — this only answers *what* and
/// *why*. Precedence mirrors the historical inline composition exactly.
pub struct ApprovalQuery<'a> {
    pub settings: &'a AgentAutonomySettings,
    pub tool_name: &'a str,
    pub command: Option<&'a str>,
    pub out_of_project_read: bool,
    pub external_read_allowed: bool,
    pub persisted_deny: bool,
    pub persisted_allow: bool,
    pub force_command_approval: bool,
}

pub enum ApprovalDecision {
    Allow { rule: &'static str },
    Ask { rule: &'static str },
    Deny { rule: &'static str },
}

impl ApprovalDecision {
    pub fn rule(&self) -> &'static str {
        match self {
            ApprovalDecision::Allow { rule }
            | ApprovalDecision::Ask { rule }
            | ApprovalDecision::Deny { rule } => rule,
        }
    }

    pub fn asked(&self) -> bool {
        matches!(self, ApprovalDecision::Ask { .. })
    }
}

pub fn decide(q: &ApprovalQuery) -> ApprovalDecision {
    // Persisted deny outranks everything, including the out-of-project guard.
    if q.persisted_deny {
        return ApprovalDecision::Deny {
            rule: "persisted always-deny rule",
        };
    }
    // The one prompt even Auto mode does not waive — unless its paths are
    // already covered by a persisted directory grant.
    if q.out_of_project_read && !q.external_read_allowed {
        return ApprovalDecision::Ask {
            rule: "out-of-project read guard",
        };
    }
    if q.out_of_project_read && q.external_read_allowed {
        return ApprovalDecision::Allow {
            rule: "persisted external read grant",
        };
    }
    if q.force_command_approval && q.tool_name == "run_command" {
        return ApprovalDecision::Ask {
            rule: "enforced command approval",
        };
    }
    if !approval_required(q.settings, q.tool_name, q.command, q.out_of_project_read) {
        return ApprovalDecision::Allow {
            rule: "no approval required",
        };
    }
    if !q.force_command_approval && q.persisted_allow {
        return ApprovalDecision::Allow {
            rule: "persisted always-allow rule",
        };
    }
    ApprovalDecision::Ask {
        rule: "approval policy",
    }
}

/// Hard gate for Ask (read-only) mode: true when a tool call would mutate
/// files or system state and must be refused outright. Read-only shell
/// commands pass; anything that writes, executes arbitrary code (`repl`),
/// or delegates to workers (which can write) is blocked.
pub fn blocked_in_read_only(tool_name: &str, command: Option<&str>) -> bool {
    is_mutating_tool(tool_name, command)
}

/// Extract the `command` argument from a `run_command` tool call's JSON args.
pub fn command_from_args(args_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(args_json)
        .ok()
        .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(String::from))
}

#[cfg(test)]
mod tests {
    use super::super::autonomy::AgentAutonomySettings;
    use super::*;

    fn settings(mode: ApprovalMode, auto_run_safe: bool) -> AgentAutonomySettings {
        AgentAutonomySettings {
            auto_run_safe_commands: auto_run_safe,
            ..Default::default()
        }
        .with_approval_mode(mode)
    }

    #[test]
    fn safe_command_classification() {
        assert!(is_safe_readonly_command("ls -la"));
        assert!(is_safe_readonly_command("cat Cargo.toml"));
        assert!(is_safe_readonly_command("/bin/ls"));
        assert!(is_safe_readonly_command("git status"));
        assert!(is_safe_readonly_command("git log --oneline -5"));
        assert!(is_safe_readonly_command("git -C /tmp status"));

        // Allowlisted programs with mutating flags are rejected.
        assert!(is_safe_readonly_command("find . -name foo.rs"));
        assert!(!is_safe_readonly_command("find . -name x -delete"));
        assert!(!is_safe_readonly_command("find . -exec rm {} +"));
        assert!(!is_safe_readonly_command("find . -fprintf out.txt %p"));
        assert!(is_safe_readonly_command("fd pattern src"));
        assert!(!is_safe_readonly_command("fd pattern -x rm"));
        assert!(!is_safe_readonly_command("fd pattern --exec=rm"));
        // env executes arbitrary programs — never safe.
        assert!(!is_safe_readonly_command("env rm -rf /"));
        assert!(!is_safe_readonly_command("env"));

        assert!(!is_safe_readonly_command("rm -rf /"));
        assert!(!is_safe_readonly_command("git push"));
        assert!(!is_safe_readonly_command("git commit -m x"));
        assert!(!is_safe_readonly_command("cat foo > bar"));
        assert!(!is_safe_readonly_command("ls; rm -rf /"));
        assert!(!is_safe_readonly_command("ls && rm x"));
        assert!(!is_safe_readonly_command("echo $(rm x)"));
        assert!(!is_safe_readonly_command("cat foo | sh"));
        assert!(!is_safe_readonly_command(""));
    }

    #[test]
    fn decide_attributes_each_outcome_to_its_rule() {
        // Helper to build queries without lifetime friction in tests.
        fn q(
            tool_name: &'static str,
            settings: &'static AgentAutonomySettings,
            tweak: impl FnOnce(&mut ApprovalQuery),
        ) -> ApprovalDecision {
            let mut query = ApprovalQuery {
                settings,
                tool_name,
                command: None,
                out_of_project_read: false,
                external_read_allowed: false,
                persisted_deny: false,
                persisted_allow: false,
                force_command_approval: false,
            };
            tweak(&mut query);
            decide(&query)
        }
        // Static settings per mode (leaked once, reused across cases).
        let suggest: &'static AgentAutonomySettings =
            Box::leak(Box::new(settings(ApprovalMode::Ask, true)));

        assert!(matches!(
            q("read_file", suggest, |_| {}),
            ApprovalDecision::Allow { .. }
        ));
        assert_eq!(
            q("read_file", suggest, |q| q.persisted_deny = true).rule(),
            "persisted always-deny rule"
        );
        assert_eq!(
            q("read_file", suggest, |q| {
                q.out_of_project_read = true;
            })
            .rule(),
            "out-of-project read guard"
        );
        assert_eq!(
            q("read_file", suggest, |q| {
                q.out_of_project_read = true;
                q.external_read_allowed = true;
            })
            .rule(),
            "persisted external read grant"
        );
        assert_eq!(
            q("run_command", suggest, |q| q.force_command_approval = true).rule(),
            "enforced command approval"
        );
        assert_eq!(q("write_file", suggest, |_| {}).rule(), "approval policy");
        assert_eq!(
            q("write_file", suggest, |q| q.persisted_allow = true).rule(),
            "persisted always-allow rule"
        );
    }

    #[test]
    fn capabilities_are_enforced_independently_of_approval_settings() {
        assert!(capability_denial("write_file", None, true, false).is_some());
        assert!(capability_denial("run_command", Some("cargo test"), false, true).is_some());
        assert!(capability_denial("read_file", None, true, true).is_none());
        assert!(capability_denial("run_command", Some("git status"), true, false).is_none());
    }

    #[test]
    fn dont_ask_waives_everything_except_out_of_project_reads() {
        let s = settings(ApprovalMode::Auto, true);
        assert!(!approval_required(&s, "run_command", Some("rm -rf /"), false));
        assert!(!approval_required(&s, "write_file", None, false));
        assert!(!approval_required(&s, "github_create_pull_request", None, false));
        // Turning off auto-run of safe commands doesn't make Don't ask ask.
        let s = settings(ApprovalMode::Auto, false);
        assert!(!approval_required(&s, "run_command", Some("ls"), false));
        // The out-of-project read guard survives every mode.
        assert!(approval_required(&s, "read_file", None, true));
    }

    #[test]
    fn ask_mode_asks_for_changes_and_actions_but_never_reads() {
        let s = settings(ApprovalMode::Ask, true);
        for tool in ["write_file", "edit_file", "apply_patch", "create_directory"] {
            assert!(approval_required(&s, tool, None, false), "{tool}");
        }
        for tool in [
            "move_file",
            "delete_file",
            "repl",
            "spawn_subagent",
            "github_create_pull_request",
            "integrations_comment",
            "integrations_api",
            "coord_team_create",
            "mcp__notes__create_page",
            "some_unknown_tool",
        ] {
            assert!(approval_required(&s, tool, None, false), "{tool}");
        }
        assert!(approval_required(&s, "run_command", Some("cargo build"), false));
        // Read-only commands run while the auto-run grant is on.
        assert!(!approval_required(&s, "run_command", Some("git diff"), false));
        for tool in [
            "read_file",
            "read_multiple_files",
            "directory_tree",
            "grep",
            "integrations_search",
            "integrations_describe",
            "coord_team_status",
            "mcp__notes__search_pages",
            "prompt__review",
        ] {
            assert!(!approval_required(&s, tool, None, false), "{tool}");
        }
        // …unless a read reaches outside the project, safe command or not.
        assert!(approval_required(&s, "read_file", None, true));
        assert!(approval_required(&s, "run_command", Some("find /outside -name docs"), true));
    }

    #[test]
    fn accept_edits_runs_edits_and_asks_for_actions() {
        let s = settings(ApprovalMode::AcceptEdits, true);
        for tool in ["write_file", "edit_file", "batch_edit", "apply_patch", "create_directory"] {
            assert!(!approval_required(&s, tool, None, false), "{tool}");
        }
        assert!(approval_required(&s, "run_command", Some("npm install"), false));
        assert!(approval_required(&s, "delete_file", None, false));
        assert!(approval_required(&s, "move_file", None, false));
        assert!(approval_required(&s, "integrations_create", None, false));
        assert!(!approval_required(&s, "run_command", Some("ls"), false));
    }

    #[test]
    fn without_auto_run_every_command_asks() {
        let s = settings(ApprovalMode::Ask, false);
        assert!(approval_required(&s, "run_command", Some("ls"), false));
        assert!(approval_required(&s, "run_command", Some("git status"), false));
        // Non-command tools are unaffected by the auto-run toggle.
        assert!(!approval_required(&s, "read_file", None, false));
    }

    #[test]
    fn read_only_gate_blocks_mutations_allows_reads() {
        // Mutating tools are blocked.
        for t in [
            "write_file",
            "edit_file",
            "batch_edit",
            "apply_patch",
            "create_directory",
            "move_file",
            "repl",
            "spawn_subagent",
        ] {
            assert!(
                blocked_in_read_only(t, None),
                "{t} must be blocked in Ask mode"
            );
        }
        // Read-only tools pass.
        for t in [
            "read_file",
            "grep",
            "search_codebase",
            "list_directory",
            "web_search",
            "fetch_url",
        ] {
            assert!(
                !blocked_in_read_only(t, None),
                "{t} must be allowed in Ask mode"
            );
        }
        // Shell: read-only commands pass, mutating ones don't.
        assert!(!blocked_in_read_only("run_command", Some("git status")));
        assert!(!blocked_in_read_only("run_command", Some("ls -la")));
        assert!(blocked_in_read_only("run_command", Some("rm -rf build")));
        assert!(blocked_in_read_only("run_command", Some("cargo build")));
        assert!(blocked_in_read_only("run_command", Some("echo hi > file")));
        assert!(blocked_in_read_only("run_command", None));
    }

    #[test]
    fn high_risk_ceiling_covers_pr_database_deletion_and_breaking_commands() {
        assert!(is_always_high_risk("github_create_pull_request", None));
        assert!(is_always_high_risk("delete_file", None));
        assert!(is_always_high_risk(
            "run_command",
            Some("psql -c 'drop table users'")
        ));
        assert!(is_always_high_risk(
            "run_command",
            Some("python manage.py migrate")
        ));
        assert!(is_always_high_risk(
            "run_command",
            Some("git push origin main")
        ));
        assert!(is_always_high_risk("run_command", Some("rm -rf /")));
        assert!(is_always_high_risk("run_command", Some("rm -rf ~")));

        // Routine work stays waivable by contract pre-authorization.
        assert!(!is_always_high_risk("read_file", None));
        assert!(!is_always_high_risk("edit_file", None));
        assert!(!is_always_high_risk("run_command", Some("git status")));
        assert!(!is_always_high_risk("run_command", Some("cargo test")));
        assert!(!is_always_high_risk("run_command", None));
    }

    #[test]
    fn command_extraction() {
        assert_eq!(
            command_from_args(r#"{"command":"ls -la"}"#).as_deref(),
            Some("ls -la")
        );
        assert_eq!(command_from_args("not json"), None);
        assert_eq!(command_from_args(r#"{"path":"x"}"#), None);
    }
}

#[cfg(test)]
mod computer_policy_tests {
    use super::*;
    #[test]
    fn desktop_reads_obey_mode_and_actions_use_their_own_confirmation() {
        assert!(capability_denial("computer", Some("snapshot"), true, false).is_none());
        assert!(capability_denial("computer", Some("click"), true, false).is_some());
        assert!(capability_denial("computer", None, true, false).is_some());
        assert!(capability_denial("computer", Some("type"), false, true).is_some());
        let settings = AgentAutonomySettings::default().with_approval_mode(ApprovalMode::Ask);
        assert!(!approval_required(&settings, "computer", Some("click"), false));
    }
}
