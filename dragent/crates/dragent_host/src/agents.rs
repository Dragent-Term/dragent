//! Agent runs in terminal sessions (Phase 6.3).
//!
//! The host owns one [`AgentRegistry`] per org directory (the endpoint
//! file's parent). Agent *configs* persist in `agent-configs.json` (atomic
//! tmp + rename, seeded catalog on first open); agent *runs* are in-memory
//! records keyed by session id strings (`ar-<uuid>`) attached to the owning
//! worktree home via the worktree manager (move semantics on attach).
//!
//! Deliberately thin over terminals: runs reference session id strings only
//! and launch through the SAME [`TerminalRegistry`](super::terminals::TerminalRegistry)
//! path `CreateTerminal` uses (no private spawn). Liveness and screen output
//! resolve via the terminal registry (the GetTerminal pattern), never from
//! these records.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::protocol::{
    AgentConfig, AgentOverrides, AgentReadiness, AgentRun, AgentRunState, PromptTransport,
    TerminalSession,
};

pub const CONFIG_FILENAME: &str = "agent-configs.json";
/// Default context budget when the dispatch leaves it at zero.
pub const CONTEXT_DEFAULT_CHARS: usize = 36_000;
/// Hard ceiling for any context budget.
pub const CONTEXT_MAX_CHARS: usize = 360_000;
/// Heredoc delimiter for [`PromptTransport::Stdin`] launches.
pub const STDIN_EOF_DELIMITER: &str = "DRAGENT_AGENT_PROMPT_EOF";

/// Bourne single-quote escaping: wrap in `'...'`, splicing `'\''` for quotes.
pub fn shell_quote(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('\'');
    let mut first = true;
    for chunk in raw.split('\'') {
        if !first {
            out.push_str("'\\''");
        }
        first = false;
        out.push_str(chunk);
    }
    out.push('\'');
    out
}

/// Build the shell command for one launch: sorted env overlay prefix +
/// command + args + prompt_args (only when the prompt is non-empty) + the
/// payload as a quoted argv element ([`PromptTransport::Argv`]) or a quoted
/// heredoc ([`PromptTransport::Stdin`]). An empty prompt returns the base
/// command (env prefix + command + args) with no transport artifact.
pub fn build_launch(config: &AgentConfig, prompt: &str) -> String {
    let mut shell = String::new();
    for (key, value) in &config.env {
        shell.push_str(key);
        shell.push('=');
        shell.push_str(&shell_quote(value));
        shell.push(' ');
    }
    shell.push_str(&shell_quote(&config.command));
    for arg in &config.args {
        shell.push(' ');
        shell.push_str(&shell_quote(arg));
    }
    if prompt.is_empty() {
        return shell;
    }
    for arg in &config.prompt_args {
        shell.push(' ');
        shell.push_str(&shell_quote(arg));
    }
    match config.prompt_transport {
        PromptTransport::Argv => {
            shell.push(' ');
            shell.push_str(&shell_quote(prompt));
        }
        PromptTransport::Stdin => {
            shell.push_str(&format!(
                " <<'{STDIN_EOF_DELIMITER}'\n{prompt}\n{STDIN_EOF_DELIMITER}"
            ));
        }
    }
    shell
}

/// `launch` behind a wait for memory in its own terminal. One line joined to
/// `launch`'s first, so a prompt it carries below still belongs to it.
pub fn hold_launch(helper: &Path, label: &str, launch: &str) -> String {
    let helper = helper.to_string_lossy();
    let org = dragent_paths::organisation_arg();
    let wait = crate::agent_sessions::WAIT_ARG;
    if cfg!(windows) {
        let quote = |text: &str| crate::agent_sessions::quote(text, "pwsh");
        let (helper, label, org) = (quote(&helper), quote(label), quote(&org));
        format!("& {helper} {wait} {label} {org}; if ($LASTEXITCODE -eq 0) {{ {launch} }}")
    } else {
        let (helper, label, org) = (shell_quote(&helper), shell_quote(label), shell_quote(&org));
        format!("{helper} {wait} {label} {org} && {launch}")
    }
}

/// Merge per-launch overrides over a resolved config: non-blank command
/// replaces, extra args append, `model` appends `--model <value>`, env
/// entries overlay. The id/preset/order survive untouched.
pub fn apply_overrides(config: &AgentConfig, overrides: &AgentOverrides) -> AgentConfig {
    let mut merged = config.clone();
    if let Some(command) = overrides.command.as_deref() {
        if !command.trim().is_empty() {
            merged.command = command.to_string();
        }
    }
    merged.args.extend(overrides.extra_args.iter().cloned());
    if let Some(model) = overrides.model.as_deref() {
        if !model.trim().is_empty() {
            merged.args.push("--model".to_string());
            merged.args.push(model.to_string());
        }
    }
    for (key, value) in &overrides.env {
        merged.env.insert(key.clone(), value.clone());
    }
    merged
}

/// Char-boundary-safe tail: the last `max` chars (whole string when short).
pub fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    text.chars().skip(count - max).collect()
}

/// Assemble the final prompt: context section + attachment manifest + task.
/// Empty sections vanish; all empty yields an empty prompt.
pub fn assemble_prompt(context: &str, staged: &[StagedAttachment], task: &str) -> String {
    let mut sections = Vec::new();
    if !context.trim().is_empty() {
        sections.push(format!("## Context\n{context}"));
    }
    if !staged.is_empty() {
        let mut manifest = String::from("## Attachments");
        for item in staged {
            manifest.push_str(&format!("\n- {} (staged as {})", item.source, item.name));
        }
        sections.push(manifest);
    }
    if !task.trim().is_empty() {
        sections.push(format!("## Task\n{task}"));
    }
    sections.join("\n\n")
}

/// One staged attachment: flat copy of `source` into the home dir as `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAttachment {
    pub source: String,
    pub name: String,
}

/// Collision-free flat file name inside `dir`: `report.md`, `report-2.md`, …
pub fn unique_name(dir: &Path, file_name: &str) -> String {
    if !dir.join(file_name).exists() {
        return file_name.to_string();
    }
    let (stem, ext) = match file_name.rfind('.') {
        Some(dot) if dot > 0 => (&file_name[..dot], &file_name[dot..]),
        _ => (file_name, ""),
    };
    let mut n = 2u32;
    loop {
        let candidate = format!("{stem}-{n}{ext}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Copy each attachment flat into `home_dir` (deduped via [`unique_name`]).
/// Rejects directories and missing paths, failing fast on the first bad one.
pub fn stage_attachments(
    home_dir: &Path,
    attachments: &[String],
) -> anyhow::Result<Vec<StagedAttachment>> {
    let mut staged = Vec::new();
    for source in attachments {
        let src = Path::new(source);
        let metadata = std::fs::metadata(src)
            .map_err(|_| anyhow::anyhow!("attachment not found: {source}"))?;
        if metadata.is_dir() {
            anyhow::bail!("attachment is a directory: {source}");
        }
        let file_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("attachment has no file name: {source}"))?;
        let name = unique_name(home_dir, file_name);
        std::fs::copy(src, home_dir.join(&name))
            .map_err(|e| anyhow::anyhow!("stage attachment {source}: {e}"))?;
        staged.push(StagedAttachment {
            source: source.clone(),
            name,
        });
    }
    Ok(staged)
}

/// Read context files (absolute, or relative to `home_dir`), concatenated
/// with path headers and cut to a `budget`-char tail. Unreadable files and
/// directories are skipped; a zero budget reads nothing.
pub fn read_context(home_dir: &Path, paths: &[String], budget: usize) -> String {
    if budget == 0 || paths.is_empty() {
        return String::new();
    }
    let mut combined = String::new();
    for raw in paths {
        let path = Path::new(raw);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            home_dir.join(path)
        };
        if full.is_dir() {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&full) else {
            continue;
        };
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }
        combined.push_str(&format!("--- {raw} ---\n{body}"));
    }
    tail_chars(&combined, budget)
}

/// Bare command names only (no separators). Searches `dirs` in order.
pub fn which_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat"] {
            let with_ext = dir.join(format!("{name}.{ext}"));
            if with_ext.is_file() {
                return Some(with_ext);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Portable `which` over `PATH`: bare names only, exec-bit on unix, PATHEXT
/// (`exe`/`cmd`/`bat`) candidates on Windows.
pub fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    which_in(name, &dirs)
}

fn seed_catalog() -> Vec<AgentConfig> {
    let mut order = 0u32;
    let mut next = |slug: &str, label: &str, command: &str, args: &[&str]| {
        order += 1;
        AgentConfig {
            id: format!("preset-{slug}"),
            preset_id: Some(slug.to_string()),
            label: label.to_string(),
            command: command.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            prompt_transport: PromptTransport::Argv,
            prompt_args: Vec::new(),
            env: BTreeMap::new(),
            order,
        }
    };
    vec![
        next(
            "claude",
            "Claude",
            "claude",
            &["--dangerously-skip-permissions"],
        ),
        next("codex", "Codex", "codex", &[]),
        next("gemini", "Gemini", "gemini", &[]),
        next("opencode", "OpenCode", "opencode", &[]),
        next("amp", "Amp", "amp", &[]),
        next("copilot", "Copilot", "copilot", &[]),
    ]
}

/// Launch metadata captured at record time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    pub agent: String,
    pub label: String,
    pub workspace_id: String,
    pub started_at: u64,
    pub instance_id: Option<String>,
    pub lease_id: Option<String>,
}

/// Project a terminal session + metadata pair into an [`AgentRun`] record.
/// The run id IS the terminal session id string attached to the owning home;
/// liveness and screen output resolve later via the terminal registry (the
/// GetTerminal pattern), never from this record.
pub fn describe_run(session: &TerminalSession, meta: &RunMeta) -> AgentRun {
    AgentRun {
        id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        worktree_id: session.worktree_id.clone(),
        agent: meta.agent.clone(),
        label: meta.label.clone(),
        state: AgentRunState::Running,
        started_at: meta.started_at,
        output: String::new(),
        memory_bytes: None,
        instance_id: meta.instance_id.clone(),
        lease_id: meta.lease_id.clone(),
    }
}

struct RunEntry {
    run: AgentRun,
    meta: RunMeta,
}

/// Org-scoped agent authority: persisted configs + live run records.
pub struct AgentRegistry {
    config_path: PathBuf,
    configs: Mutex<Vec<AgentConfig>>,
    runs: Mutex<HashMap<String, RunEntry>>,
}

impl AgentRegistry {
    /// Open (creating) `<org>/agent-configs.json`, seeding the preset
    /// catalog on first open. Every mutation persists atomically.
    pub fn open(org_dir: &Path) -> anyhow::Result<Self> {
        let config_path = org_dir.join(CONFIG_FILENAME);
        let configs: Vec<AgentConfig> = match std::fs::read_to_string(&config_path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("decode {}: {e}", config_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let seeded = seed_catalog();
                persist_configs(&config_path, &seeded)?;
                seeded
            }
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", config_path.display())),
        };
        Ok(Self {
            config_path,
            configs: Mutex::new(configs),
            runs: Mutex::new(HashMap::new()),
        })
    }

    fn persist_locked(&self, configs: &[AgentConfig]) -> anyhow::Result<()> {
        persist_configs(&self.config_path, configs)
    }

    /// All configs, lowest `order` first.
    pub fn list_configs(&self) -> Vec<AgentConfig> {
        let mut configs = self
            .configs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        configs.sort_by_key(|c| c.order);
        configs
    }

    /// Add a user config: validates non-blank label + command, assigns
    /// `agent-<uuid>` and `max(order)+1`, persists before returning.
    pub fn add_config(
        &self,
        label: &str,
        command: &str,
        args: Vec<String>,
        prompt_transport: PromptTransport,
        prompt_args: Vec<String>,
        env: BTreeMap<String, String>,
    ) -> anyhow::Result<AgentConfig> {
        if label.trim().is_empty() {
            anyhow::bail!("agent label must not be empty");
        }
        if command.trim().is_empty() {
            anyhow::bail!("agent command must not be empty");
        }
        let mut configs = self.configs.lock().unwrap_or_else(|p| p.into_inner());
        let order = configs.iter().map(|c| c.order).max().unwrap_or(0) + 1;
        let config = AgentConfig {
            id: format!("agent-{}", uuid::Uuid::new_v4()),
            preset_id: None,
            label: label.to_string(),
            command: command.to_string(),
            args,
            prompt_transport,
            prompt_args,
            env,
            order,
        };
        configs.push(config.clone());
        self.persist_locked(&configs)?;
        Ok(config)
    }

    /// Resolve an agent reference: exact id first, else the lowest-order
    /// config whose preset matches (preset id or `preset-<slug>` id).
    pub fn resolve(&self, agent_ref: &str) -> Option<AgentConfig> {
        let configs = self.configs.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(hit) = configs.iter().find(|c| c.id == agent_ref) {
            return Some(hit.clone());
        }
        configs
            .iter()
            .filter(|c| {
                c.preset_id.as_deref() == Some(agent_ref) || c.id == format!("preset-{agent_ref}")
            })
            .min_by_key(|c| c.order)
            .cloned()
    }

    /// Record a freshly launched run against its terminal session.
    pub fn record_run(&self, session: &TerminalSession, meta: RunMeta) -> AgentRun {
        let run = describe_run(session, &meta);
        let mut runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        runs.insert(
            run.id.clone(),
            RunEntry {
                run: run.clone(),
                meta,
            },
        );
        run
    }

    /// Launch metadata for one run, if still tracked.
    pub fn run_meta(&self, run_id: &str) -> Option<RunMeta> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .map(|e| e.meta.clone())
    }

    /// Base record for one run (state/output fill in via the terminal
    /// registry at serve time).
    pub fn get_record(&self, run_id: &str) -> Option<AgentRun> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(run_id)
            .map(|e| e.run.clone())
    }

    /// Base records for one workspace, oldest first.
    pub fn list_records(&self, workspace_id: &str) -> Vec<AgentRun> {
        let runs = self.runs.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<AgentRun> = runs
            .values()
            .filter(|e| e.run.workspace_id == workspace_id)
            .map(|e| e.run.clone())
            .collect();
        out.sort_by_key(|r| r.started_at);
        out
    }

    /// All tracked run ids for one workspace (runtime counts).
    pub fn run_ids_for_workspace(&self, workspace_id: &str) -> Vec<String> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|e| e.run.workspace_id == workspace_id)
            .map(|e| e.run.id.clone())
            .collect()
    }

    /// Every tracked run as (workspace_id, session_id) for host-wide
    /// capacity accounting. Liveness resolves against the daemon per id.
    pub fn all_tracked_runs(&self) -> Vec<(String, String)> {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|e| (e.run.workspace_id.clone(), e.run.id.clone()))
            .collect()
    }

    /// Total tracked runs (runtime snapshot).
    pub fn run_count(&self) -> usize {
        self.runs.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Drop one run record (the terminal itself closes through the terminal
    /// registry — teardown and CloseTerminal own that half).
    pub fn drop_run(&self, run_id: &str) -> bool {
        self.runs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(run_id)
            .is_some()
    }

    /// Drop every run in `session_ids` (teardown fan-out and CloseTerminal
    /// share this). Returns the dropped run ids.
    pub fn drop_runs_for_sessions(&self, session_ids: &[String]) -> Vec<String> {
        let mut dropped = Vec::new();
        for id in session_ids {
            if self.drop_run(id) {
                dropped.push(id.clone());
            }
        }
        dropped
    }
}

/// Honest install probe: resolve, then portable `which` on the command.
/// Unknown agents report `installed: false`, never an error.
pub fn check_readiness(registry: &AgentRegistry, agent_ref: &str) -> AgentReadiness {
    let configs = registry.configs.lock().unwrap_or_else(|p| p.into_inner());
    let resolved = configs
        .iter()
        .find(|c| c.id == agent_ref)
        .cloned()
        .or_else(|| {
            configs
                .iter()
                .filter(|c| {
                    c.preset_id.as_deref() == Some(agent_ref)
                        || c.id == format!("preset-{agent_ref}")
                })
                .min_by_key(|c| c.order)
                .cloned()
        });
    drop(configs);
    match resolved {
        None => AgentReadiness {
            agent: agent_ref.to_string(),
            label: agent_ref.to_string(),
            installed: false,
            detail: format!("unknown agent {agent_ref}"),
        },
        Some(config) => {
            if config.command.contains('/') || config.command.contains('\\') {
                return AgentReadiness {
                    agent: config.id.clone(),
                    label: config.label.clone(),
                    installed: false,
                    detail: format!("command {:?} is not a bare binary name", config.command),
                };
            }
            // A copy chosen for this machine counts when it is still there.
            #[cfg(not(target_family = "wasm"))]
            let chosen = crate::host_tools::launch_words(&config.command).and_then(|words| {
                let program = PathBuf::from(&words[0]);
                if program.is_absolute() { Some(program) } else { which(&words[0]) }
            });
            #[cfg(target_family = "wasm")]
            let chosen: Option<PathBuf> = None;
            match chosen.or_else(|| which(&config.command)) {
                Some(path) => AgentReadiness {
                    agent: config.id.clone(),
                    label: config.label.clone(),
                    installed: true,
                    detail: path.to_string_lossy().to_string(),
                },
                None => AgentReadiness {
                    agent: config.id.clone(),
                    label: config.label.clone(),
                    installed: false,
                    detail: format!("{} not found on PATH", config.command),
                },
            }
        }
    }
}

fn persist_configs(path: &Path, configs: &[AgentConfig]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let file = std::fs::File::create(&tmp)?;
    serde_json::to_writer_pretty(&file, configs)
        .map_err(|e| anyhow::anyhow!("encode agent configs: {e}"))?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Clamp a requested context budget: zero means the default, above the max
/// clamps to the max.
pub fn clamp_context_chars(requested: usize) -> usize {
    if requested == 0 {
        CONTEXT_DEFAULT_CHARS
    } else {
        requested.min(CONTEXT_MAX_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn echo_argv(extra_env: &[(&str, &str)]) -> AgentConfig {
        AgentConfig {
            id: "agent-echo".to_string(),
            preset_id: None,
            label: "Echo".to_string(),
            command: "echo".to_string(),
            args: vec![],
            prompt_transport: PromptTransport::Argv,
            prompt_args: vec![],
            env: extra_env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            order: 1,
        }
    }

    #[test]
    fn quoting_escapes_single_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("a'b'c"), "'a'\\''b'\\''c'");
    }

    #[test]
    fn argv_launch_appends_prompt_args_and_quoted_payload() {
        let mut config = echo_argv(&[]);
        config.prompt_args = vec!["--prompt".to_string()];
        let shell = build_launch(&config, "do the thing");
        assert_eq!(shell, "'echo' '--prompt' 'do the thing'");
    }

    #[test]
    fn stdin_launch_uses_heredoc_and_empty_prompt_returns_base() {
        let mut config = echo_argv(&[]);
        config.command = "cat".to_string();
        config.prompt_transport = PromptTransport::Stdin;
        let shell = build_launch(&config, "hello\nworld");
        assert_eq!(
            shell,
            "'cat' <<'DRAGENT_AGENT_PROMPT_EOF'\nhello\nworld\nDRAGENT_AGENT_PROMPT_EOF"
        );
        assert_eq!(build_launch(&config, ""), "'cat'");
        let argv = echo_argv(&[]);
        assert_eq!(build_launch(&argv, ""), "'echo'");
    }

    #[test]
    fn env_overlay_prefix_is_sorted() {
        let config = echo_argv(&[("OMEGA", "1"), ("ALPHA", "a b"), ("MID", "it's")]);
        let shell = build_launch(&config, "hi");
        assert!(shell.starts_with("ALPHA='a b' MID='it'\\''s' OMEGA='1' 'echo' 'hi'"));
    }

    #[test]
    fn resolve_prefers_id_then_lowest_order_preset() {
        let dir = std::env::temp_dir().join(format!("dragent-ag-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let registry = AgentRegistry::open(&dir).unwrap();
        // Seed catalog: preset-claude order 1. Add a user config whose id
        // collides with nothing but whose label mentions claude.
        let custom = registry
            .add_config(
                "Custom",
                "echo",
                vec![],
                PromptTransport::Argv,
                vec![],
                BTreeMap::new(),
            )
            .unwrap();
        assert!(custom.id.starts_with("agent-"));
        assert_eq!(custom.order, 7);
        // Exact id wins even when a preset slug matches something else.
        assert_eq!(registry.resolve(&custom.id).unwrap().id, custom.id);
        // Preset slug resolves to the seeded preset (lowest order).
        let hit = registry.resolve("claude").unwrap();
        assert_eq!(hit.id, "preset-claude");
        assert!(registry.resolve("nope").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_chars_is_char_boundary_safe_and_prompt_assembly_drops_empties() {
        assert_eq!(tail_chars("abcdef", 3), "def");
        assert_eq!(tail_chars("abc", 9), "abc");
        assert_eq!(tail_chars("", 4), "");
        // Multibyte: last 2 chars of "héllo😀" are "o😀", never split bytes.
        assert_eq!(tail_chars("héllo😀", 2), "o😀");
        let staged = vec![StagedAttachment {
            source: "/tmp/report.md".to_string(),
            name: "report.md".to_string(),
        }];
        let full = assemble_prompt("ctx", &staged, "task");
        assert!(full.contains("## Context\nctx"));
        assert!(full.contains("- /tmp/report.md (staged as report.md)"));
        assert!(full.contains("## Task\ntask"));
        assert!(!assemble_prompt("", &[], "task").contains("Context"));
        assert!(!assemble_prompt("ctx", &[], "task").contains("Attachments"));
        assert_eq!(assemble_prompt("", &[], ""), "");
    }

    #[test]
    fn staging_copies_flat_dedups_rejects_dirs_and_missing() {
        let base = std::env::temp_dir().join(format!("dragent-ag-stage-{}", std::process::id()));
        let src = base.join("src");
        let home = base.join("home");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(src.join("report.md"), "body").unwrap();
        std::fs::write(home.join("report.md"), "existing").unwrap();
        let staged = stage_attachments(
            &home,
            &[src.join("report.md").to_string_lossy().to_string()],
        )
        .unwrap();
        assert_eq!(staged[0].name, "report-2.md");
        assert_eq!(
            std::fs::read_to_string(home.join("report-2.md")).unwrap(),
            "body"
        );
        // Dirs and missing paths fail fast.
        assert!(stage_attachments(&home, &[src.to_string_lossy().to_string()]).is_err());
        assert!(
            stage_attachments(&home, &[src.join("ghost.md").to_string_lossy().to_string()])
                .is_err()
        );
        // add_config validation: blank label/command rejected.
        let registry = AgentRegistry::open(&base.join("org")).unwrap();
        assert!(
            registry
                .add_config(
                    "",
                    "echo",
                    vec![],
                    PromptTransport::Argv,
                    vec![],
                    BTreeMap::new()
                )
                .is_err()
        );
        assert!(
            registry
                .add_config(
                    "x",
                    "  ",
                    vec![],
                    PromptTransport::Argv,
                    vec![],
                    BTreeMap::new()
                )
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn which_finds_bare_names_only_over_explicit_dirs() {
        let base = std::env::temp_dir().join(format!("dragent-ag-which-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = base.join("fakeagent");
            std::fs::write(&bin, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            let plain = base.join("notexec");
            std::fs::write(&plain, "x").unwrap();
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(which_in("fakeagent", &[base.clone()]).unwrap(), bin);
            assert!(which_in("notexec", &[base.clone()]).is_none());
        }
        assert!(which_in("has/slash", &[base.clone()]).is_none());
        assert!(which_in("", &[base.clone()]).is_none());
        assert!(which_in("definitely-not-a-real-binary-xyz", &[base.clone()]).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
