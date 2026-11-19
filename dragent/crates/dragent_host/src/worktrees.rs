//! Worktree-backed workspaces + teardown saga (Phase 6.1).
//!
//! The Host owns one [`WorktreeManager`] per org directory (the endpoint
//! file's parent). Each home is either a real `git worktree` checkout under
//! `<org>/worktrees/<workspace>/` or a standalone session directory under
//! `<org>/sessions/<workspace>/`. The JSON ledger at `<org>/worktrees.json`
//! is the authority; every mutation persists atomically (tmp + rename)
//! before any destructive step, so a crash mid-teardown resumes on the next
//! [`WorktreeManager::open`] via [`WorktreeManager::reconcile`].
//!
//! Teardown order is load-bearing: tombstone-persist first, dispose sessions
//! via the caller hook, `git worktree remove --force`, verify unregistered
//! (prune + re-verify once, else warning), guarded `remove_dir_all`, drop
//! from the ledger, persist. Branches are never deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::protocol::{GitBinding, HomeKind, TeardownReport, WorktreeHome, WorktreeState};

const LEDGER_FILENAME: &str = "worktrees.json";
const WORKTREES_DIRNAME: &str = "worktrees";
const SESSIONS_DIRNAME: &str = "sessions";
const MARKER_FILENAME: &str = ".dragent-home.json";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Outcome of [`WorktreeManager::reconcile`]: crash-resumed tombstones and
/// pruned stranded checkouts. Non-fatal problems accumulate in `warnings`.
#[derive(Debug, Clone, Default)]
pub struct ReconcileReport {
    pub resumed: usize,
    pub stranded: usize,
    pub warnings: Vec<String>,
}

/// Filesystem-safe segment: lowercase alphanumerics plus `-`, `_`, `.`;
/// everything else becomes `-` (collapsed), trimmed, truncated to `max`
/// chars, falling back to `fallback` when nothing survives.
pub fn sanitize_segment(raw: &str, max: usize, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // suppress leading separators
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch == '_' || ch == '.') && !out.is_empty() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') || out.ends_with('.') {
        out.pop();
    }
    if out.chars().count() > max {
        out = out.chars().take(max).collect();
        while out.ends_with('-') || out.ends_with('.') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str(fallback);
    }
    out
}

/// Filesystem/branch-safe branch stem (64 chars).
pub fn sanitize_branch(branch: &str) -> String {
    sanitize_segment(branch, 64, "branch")
}

/// Ledger-safe flow label (48 chars).
pub fn sanitize_flow(flow: &str) -> String {
    sanitize_segment(flow, 48, "flow")
}

/// Parse `git worktree list --porcelain` output into checkout paths: one
/// `worktree <path>` line per entry.
pub fn parse_worktree_paths(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Read the `.dragent-home.json` marker id inside `dir`, if present.
fn read_marker_id(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(MARKER_FILENAME)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("id")?.as_str().map(|s| s.to_string())
}

fn write_marker(dir: &Path, id: &str) -> anyhow::Result<()> {
    let marker = dir.join(MARKER_FILENAME);
    let tmp = marker.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(
        &tmp,
        format!("{{\"id\":{id}}}\n", id = serde_json::to_string(id)?),
    )?;
    std::fs::rename(&tmp, &marker)?;
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    let output = crate::git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("git {} failed to spawn: {e}", args.join(" ")))?;
    Ok(output)
}

fn git_stderr(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if text.is_empty() {
        format!("exit {}", output.status.code().unwrap_or(-1))
    } else {
        text.lines().next().unwrap_or("git failed").to_string()
    }
}

/// Canonical owner repo root for `repo_root` (fails outside git repos).
fn canonical_repo_root(repo_root: &Path) -> anyhow::Result<PathBuf> {
    let output = run_git(repo_root, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        anyhow::bail!(
            "not a git repository: {} ({})",
            repo_root.display(),
            git_stderr(&output)
        );
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

fn branch_exists(repo_root: &Path, branch: &str) -> bool {
    run_git(
        repo_root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// True when `path` is currently registered as a worktree of `repo_root`.
/// A failing `git worktree list` (owner repo gone) counts as unregistered.
fn worktree_registered(repo_root: &str, path: &Path) -> bool {
    let output = match run_git(Path::new(repo_root), &["worktree", "list", "--porcelain"]) {
        Ok(output) if output.status.success() => output,
        _ => return false,
    };
    let porcelain = String::from_utf8_lossy(&output.stdout);
    parse_worktree_paths(&porcelain)
        .iter()
        .any(|listed| paths_equal(listed, path))
}

fn home_healthy(home: &WorktreeHome) -> bool {
    let path = Path::new(&home.path);
    if !path.exists() {
        return false;
    }
    // Root homes are user directories: presence is health. No marker (we
    // never write into user dirs) and no git-registration check (the main
    // checkout is not a registered linked worktree).
    if matches!(home.kind, HomeKind::Root) {
        return true;
    }
    // Agent checkouts carry no marker: git's registration is the proof.
    if matches!(home.kind, HomeKind::AgentCheckout) {
        return home
            .repo
            .as_ref()
            .is_some_and(|repo| worktree_registered(&repo.repo_root, path));
    }
    if read_marker_id(path).as_deref() != Some(home.id.as_str()) {
        return false;
    }
    match &home.repo {
        Some(repo) => worktree_registered(&repo.repo_root, path),
        None => true,
    }
}

/// Guarded directory removal: refuses (with a warning, not an error) when
/// the marker id mismatches — never delete a directory we do not own. An
/// empty `expected_id` skips the ownership check. Missing paths are a no-op;
/// `remove_dir_all` failures become warnings.
fn remove_dir_guarded(path: &Path, expected_id: &str, warnings: &mut Vec<String>) {
    if !path.exists() {
        return;
    }
    if !expected_id.is_empty() && read_marker_id(path).as_deref() != Some(expected_id) {
        warnings.push(format!(
            "refusing to remove {}: marker mismatch (expected {expected_id})",
            path.display()
        ));
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(path) {
        // A concurrent remover may have beaten us — re-check before warning.
        if Path::new(path).exists() {
            warnings.push(format!("failed to remove {}: {e}", path.display()));
        }
    }
}

fn persist_ledger(ledger_path: &Path, homes: &HashMap<String, WorktreeHome>) -> anyhow::Result<()> {
    let tmp = ledger_path.with_extension(format!("tmp-{}", std::process::id()));
    let file = std::fs::File::create(&tmp)?;
    serde_json::to_writer_pretty(&file, homes)
        .map_err(|e| anyhow::anyhow!("encode worktree ledger: {e}"))?;
    drop(file);
    std::fs::rename(&tmp, ledger_path)?;
    Ok(())
}

/// Host-owned worktree + standalone-session authority. Interior-mutability
/// lets the host server share one manager behind a mutex.
pub struct WorktreeManager {
    worktrees_dir: PathBuf,
    sessions_dir: PathBuf,
    ledger_path: PathBuf,
    homes: Mutex<HashMap<String, WorktreeHome>>,
}

impl WorktreeManager {
    /// Open (creating) `<org>/worktrees/`, `<org>/sessions/`, and the
    /// `worktrees.json` ledger, then reconcile crash leftovers.
    pub fn open(org_dir: &Path) -> anyhow::Result<Self> {
        let manager = Self::open_deferred_reconcile(org_dir)?;
        let report = manager.reconcile();
        manager.log_reconcile(&report);
        Ok(manager)
    }

    /// Open the ledger before a surviving PTY daemon is adopted. The host
    /// calls `reconcile_with` after constructing its supervisor so tombstoned
    /// sessions can be disposed before their directories are removed.
    pub fn open_deferred_reconcile(org_dir: &Path) -> anyhow::Result<Self> {
        let worktrees_dir = org_dir.join(WORKTREES_DIRNAME);
        let sessions_dir = org_dir.join(SESSIONS_DIRNAME);
        let ledger_path = org_dir.join(LEDGER_FILENAME);
        std::fs::create_dir_all(&worktrees_dir)?;
        std::fs::create_dir_all(&sessions_dir)?;
        // Canonicalize so `home.path` strings match what git records for
        // `worktree add/list` (git resolves symlinks such as /var ->
        // /private/var on macOS; unresolved strings would never compare
        // equal to `git worktree list --porcelain` output).
        let worktrees_dir = worktrees_dir.canonicalize().unwrap_or(worktrees_dir);
        let sessions_dir = sessions_dir.canonicalize().unwrap_or(sessions_dir);
        let homes: HashMap<String, WorktreeHome> = match std::fs::read_to_string(&ledger_path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("decode {}: {e}", ledger_path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", ledger_path.display())),
        };
        let manager = Self {
            worktrees_dir,
            sessions_dir,
            ledger_path,
            homes: Mutex::new(homes),
        };
        Ok(manager)
    }

    fn log_reconcile(&self, report: &ReconcileReport) {
        if report.resumed > 0 || report.stranded > 0 || !report.warnings.is_empty() {
            log::info!(
                "worktree reconcile: resumed={} stranded={} warnings={:?}",
                report.resumed,
                report.stranded,
                report.warnings
            );
        }
    }

    fn persist(&self, homes: &HashMap<String, WorktreeHome>) -> anyhow::Result<()> {
        persist_ledger(&self.ledger_path, homes)
    }

    /// Live homes for one workspace, oldest first. Tombstoned homes are
    /// invisible (they only exist between crash and reconcile).
    pub fn list(&self, workspace_id: &str) -> Vec<WorktreeHome> {
        let homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<WorktreeHome> = homes
            .values()
            .filter(|h| matches!(h.state, WorktreeState::Live) && h.workspace_id == workspace_id)
            .cloned()
            .collect();
        out.sort_by_key(|h| h.created_at_unix_ms);
        out
    }

    /// One live home by id, regardless of workspace (callers enforce
    /// ownership against `home.workspace_id`).
    pub fn get(&self, home_id: &str) -> Option<WorktreeHome> {
        let homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        homes
            .get(home_id)
            .filter(|h| matches!(h.state, WorktreeState::Live))
            .cloned()
    }

    /// Create a real `git worktree` checkout for `workspace_id`. The checkout
    /// dir and the new branch share a sanitized stem; when the path or the
    /// branch already exists the stem gains a `-{n}` suffix until both are
    /// free. The requested branch is never reused — `git worktree add -b`
    /// always mints the (possibly suffixed) name.
    pub fn create_worktree(
        &self,
        workspace_id: &str,
        flow: &str,
        repo_root: &str,
        branch: &str,
    ) -> anyhow::Result<WorktreeHome> {
        if workspace_id.trim().is_empty() {
            anyhow::bail!("workspace_id must not be empty");
        }
        let canonical = canonical_repo_root(Path::new(repo_root))?;
        let base = sanitize_branch(branch);
        let mut n: u32 = 1;
        let (dir, branch_name) = loop {
            let stem = if n == 1 {
                base.clone()
            } else {
                sanitize_segment(&format!("{base}-{n}"), 64, "branch")
            };
            let dir = self.worktrees_dir.join(workspace_id).join(&stem);
            if !dir.exists() && !branch_exists(&canonical, &stem) {
                break (dir, stem);
            }
            n += 1;
            if n > 10_000 {
                anyhow::bail!("could not find a free worktree name for {base}");
            }
        };
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let dir_arg = dir.to_string_lossy().to_string();
        let output = run_git(
            &canonical,
            &["worktree", "add", "-b", &branch_name, "--", &dir_arg],
        )?;
        if !output.status.success() {
            anyhow::bail!("git worktree add failed: {}", git_stderr(&output));
        }
        let id = format!("wt-{}", uuid::Uuid::new_v4());
        let home = WorktreeHome {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            flow: sanitize_flow(flow),
            path: dir_arg,
            repo: Some(GitBinding {
                repo_root: canonical.to_string_lossy().to_string(),
                branch: branch_name,
            }),
            ports: Vec::new(),
            sessions: Vec::new(),
            state: WorktreeState::Live,
            created_at_unix_ms: now_ms(),
            kind: HomeKind::Worktree,
        };
        write_marker(&dir, &id)?;
        {
            let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            homes.insert(id, home.clone());
            self.persist(&homes)?;
        }
        Ok(home)
    }

    /// Create a standalone session directory (no git) and track it as a
    /// ledger home with `repo: None` so teardown/sessions/ports stay uniform.
    pub fn create_standalone_session(
        &self,
        workspace_id: &str,
        label: &str,
    ) -> anyhow::Result<WorktreeHome> {
        if workspace_id.trim().is_empty() {
            anyhow::bail!("workspace_id must not be empty");
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let id = format!("sess-{uuid}");
        let short: String = uuid.chars().filter(|c| *c != '-').take(8).collect();
        let stem = sanitize_segment(label, 32, "session");
        let dir = self
            .sessions_dir
            .join(workspace_id)
            .join(format!("{stem}-{short}"));
        std::fs::create_dir_all(&dir)?;
        write_marker(&dir, &id)?;
        let home = WorktreeHome {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            flow: sanitize_flow(label),
            path: dir.to_string_lossy().to_string(),
            repo: None,
            ports: Vec::new(),
            sessions: Vec::new(),
            state: WorktreeState::Live,
            created_at_unix_ms: now_ms(),
            kind: HomeKind::Standalone,
        };
        {
            let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            homes.insert(id, home.clone());
            self.persist(&homes)?;
        }
        Ok(home)
    }

    /// Register an EXISTING directory as a terminal-session home without
    /// creating anything on disk: no new checkout, no `git worktree add`,
    /// no `git init`. The directory must exist; git provenance is attached
    /// read-only when it resolves inside a checkout. Idempotent per
    /// (workspace, canonical path): repeated calls return the same home.
    /// This is what lets plain project roots (and the projectless cwd)
    /// host daemon-owned terminal sessions that outlive the UI process.
    pub fn ensure_root_home(&self, workspace_id: &str, root: &str) -> anyhow::Result<WorktreeHome> {
        if workspace_id.trim().is_empty() {
            anyhow::bail!("workspace_id must not be empty");
        }
        let canonical = Path::new(root)
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("project root unavailable: {e}"))?;
        if !canonical.is_dir() {
            anyhow::bail!("project root is not a directory: {}", canonical.display());
        }
        {
            let homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(home) = homes.values().find(|h| {
                matches!(h.state, WorktreeState::Live)
                    && h.workspace_id == workspace_id
                    && Path::new(&h.path) == canonical.as_path()
            }) {
                return Ok(home.clone());
            }
        }
        // Read-only git probe: a checkout root carries its toplevel as the
        // binding; anything else registers repo-less. Never mutates git.
        // The toplevel is canonicalized like the home path (macOS resolves
        // /var -> /private/var for one but not the other).
        let repo = run_git(&canonical, &["rev-parse", "--show-toplevel"])
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let toplevel = Path::new(&raw)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(&raw))
                    .to_string_lossy()
                    .into_owned();
                let branch = run_git(&canonical, &["symbolic-ref", "--short", "-q", "HEAD"])
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|b| !b.is_empty())
                    .unwrap_or_default();
                GitBinding {
                    repo_root: toplevel,
                    branch,
                }
            });
        let id = format!("home-{}", uuid::Uuid::new_v4());
        let home = WorktreeHome {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            flow: sanitize_flow("project"),
            path: canonical.to_string_lossy().to_string(),
            repo,
            ports: Vec::new(),
            sessions: Vec::new(),
            state: WorktreeState::Live,
            created_at_unix_ms: now_ms(),
            kind: HomeKind::Root,
        };
        {
            let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            // Re-check under the write lock so concurrent ensures converge.
            if let Some(home) = homes.values().find(|h| {
                matches!(h.state, WorktreeState::Live)
                    && h.workspace_id == workspace_id
                    && Path::new(&h.path) == canonical.as_path()
            }) {
                return Ok(home.clone());
            }
            homes.insert(id, home.clone());
            self.persist(&homes)?;
        }
        Ok(home)
    }

    /// Adopt an existing linked worktree (an agent checkout the app created)
    /// into the ledger without touching it. Refuses a path git does not list
    /// as a linked worktree of `repo_root`. Idempotent per canonical path.
    pub fn adopt_checkout(
        &self,
        workspace_id: &str,
        repo_root: &str,
        path: &str,
        flow: &str,
    ) -> anyhow::Result<WorktreeHome> {
        if workspace_id.trim().is_empty() {
            anyhow::bail!("workspace_id must not be empty");
        }
        let canonical = Path::new(path)
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("checkout unavailable: {e}"))?;
        let repo_root = Path::new(repo_root)
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("repository unavailable: {e}"))?;
        if !worktree_registered(&repo_root.to_string_lossy(), &canonical) {
            anyhow::bail!(
                "{} is not a linked worktree of {}",
                canonical.display(),
                repo_root.display()
            );
        }
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(home) = homes.values().find(|h| {
            matches!(h.state, WorktreeState::Live) && Path::new(&h.path) == canonical.as_path()
        }) {
            return Ok(home.clone());
        }
        let id = format!("agent-{}", uuid::Uuid::new_v4());
        let home = WorktreeHome {
            id: id.clone(),
            workspace_id: workspace_id.to_string(),
            flow: sanitize_flow(flow),
            path: canonical.to_string_lossy().to_string(),
            repo: Some(GitBinding {
                repo_root: repo_root.to_string_lossy().to_string(),
                // Agent checkouts are detached at a baseline commit.
                branch: String::new(),
            }),
            ports: Vec::new(),
            sessions: Vec::new(),
            state: WorktreeState::Live,
            created_at_unix_ms: now_ms(),
            kind: HomeKind::AgentCheckout,
        };
        homes.insert(id, home.clone());
        self.persist(&homes)?;
        Ok(home)
    }

    /// Attach `session_id` to `home_id` with move semantics: the id is
    /// stripped from every home first, then pushed onto the target. Errors
    /// on unknown (or non-live) homes. Returns the updated target.
    pub fn attach_session(&self, home_id: &str, session_id: &str) -> anyhow::Result<WorktreeHome> {
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        let live = homes
            .get(home_id)
            .is_some_and(|h| matches!(h.state, WorktreeState::Live));
        if !live {
            anyhow::bail!("unknown worktree {home_id}");
        }
        for home in homes.values_mut() {
            home.sessions.retain(|s| s != session_id);
        }
        let target = homes.get_mut(home_id).expect("target checked live above");
        target.sessions.push(session_id.to_string());
        let updated = target.clone();
        self.persist(&homes)?;
        Ok(updated)
    }

    /// Detach `session_id` from `home_id` (no-op when absent). Errors on
    /// unknown (or non-live) homes. Returns the updated home.
    pub fn detach_session(&self, home_id: &str, session_id: &str) -> anyhow::Result<WorktreeHome> {
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        let Some(target) = homes.get_mut(home_id) else {
            anyhow::bail!("unknown worktree {home_id}");
        };
        if !matches!(target.state, WorktreeState::Live) {
            anyhow::bail!("unknown worktree {home_id}");
        }
        target.sessions.retain(|s| s != session_id);
        let updated = target.clone();
        self.persist(&homes)?;
        Ok(updated)
    }

    /// Probe a free loopback port and reserve it on the home. The socket is
    /// released before returning (TOCTOU accepted — reservation, not a bind).
    pub fn allocate_port(&self, home_id: &str) -> anyhow::Result<u16> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| anyhow::anyhow!("port probe bind failed: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| anyhow::anyhow!("port probe addr failed: {e}"))?
            .port();
        drop(listener);
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        let Some(target) = homes.get_mut(home_id) else {
            anyhow::bail!("unknown worktree {home_id}");
        };
        if !matches!(target.state, WorktreeState::Live) {
            anyhow::bail!("unknown worktree {home_id}");
        }
        target.ports.push(port);
        self.persist(&homes)?;
        Ok(port)
    }

    /// Release a reserved port (no-op when absent). Errors on unknown (or
    /// non-live) homes.
    pub fn release_port(&self, home_id: &str, port: u16) -> anyhow::Result<()> {
        let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
        let Some(target) = homes.get_mut(home_id) else {
            anyhow::bail!("unknown worktree {home_id}");
        };
        if !matches!(target.state, WorktreeState::Live) {
            anyhow::bail!("unknown worktree {home_id}");
        }
        target.ports.retain(|p| *p != port);
        self.persist(&homes)?;
        Ok(())
    }

    /// Ordered teardown saga: tombstone-persist first (crash-safe), dispose
    /// sessions via the caller hook, remove the worktree registration and/or
    /// directory, drop from the ledger, persist. Branches are kept by design
    /// (`report.branch_kept`); cleanup failures become `report.warnings`.
    pub fn teardown<F>(
        &self,
        home_id: &str,
        reason: &str,
        dispose_sessions: F,
    ) -> anyhow::Result<TeardownReport>
    where
        F: FnOnce(&[String]),
    {
        let snapshot = {
            let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            let Some(home) = homes.get_mut(home_id) else {
                anyhow::bail!("unknown worktree {home_id}");
            };
            home.state = WorktreeState::Tombstoned {
                reason: reason.to_string(),
            };
            let snapshot = home.clone();
            self.persist(&homes)?;
            snapshot
        };
        dispose_sessions(&snapshot.sessions);
        let mut warnings = Vec::new();
        self.remove_home_files(&snapshot, &mut warnings);
        if warnings.is_empty() {
            let mut homes = self.homes.lock().unwrap_or_else(|p| p.into_inner());
            homes.remove(home_id);
            self.persist(&homes)?;
        }
        Ok(TeardownReport {
            worktree_id: home_id.to_string(),
            sessions_detached: snapshot.sessions.len(),
            ports_released: snapshot.ports.len(),
            branch_kept: snapshot.repo.as_ref().map(|r| r.branch.clone()),
            warnings,
        })
    }

    /// Remove a home's on-disk footprint: `git worktree remove --force` when
    /// the checkout still exists, verify unregistered (prune + re-verify
    /// once, else warning), then guarded directory removal. Standalone
    /// sessions skip straight to the guarded removal.
    fn remove_home_files(&self, home: &WorktreeHome, warnings: &mut Vec<String>) {
        // Root homes are pre-existing user directories: teardown disposes
        // sessions (caller hook) and drops the registration, but NEVER
        // touches the filesystem — no `git worktree remove`, no deletion.
        if matches!(home.kind, HomeKind::Root) {
            return;
        }
        let path = Path::new(&home.path);
        if let Some(repo) = &home.repo {
            let repo_root = Path::new(&repo.repo_root);
            // Agent checkouts are locked while in use so git's own pruning
            // can't take them; release the lock before removing.
            if matches!(home.kind, HomeKind::AgentCheckout) && path.exists() {
                let _ = run_git(repo_root, &["worktree", "unlock", "--", &home.path]);
            }
            if path.exists() {
                match run_git(
                    repo_root,
                    &["worktree", "remove", "--force", "--", &home.path],
                ) {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => warnings.push(format!(
                        "git worktree remove failed for {}: {}",
                        path.display(),
                        git_stderr(&output)
                    )),
                    Err(e) => warnings.push(format!(
                        "git worktree remove failed for {}: {e:#}",
                        path.display()
                    )),
                }
            }
            if worktree_registered(&repo.repo_root, path) {
                let _ = run_git(repo_root, &["worktree", "prune"]);
                if worktree_registered(&repo.repo_root, path) {
                    warnings.push(format!(
                        "worktree {} still registered after prune",
                        path.display()
                    ));
                    return;
                }
            }
        }
        // No marker in an agent checkout: once git has let go of it (checked
        // above) the leftover directory is ours to remove.
        let expected_id = if matches!(home.kind, HomeKind::AgentCheckout) {
            ""
        } else {
            home.id.as_str()
        };
        remove_dir_guarded(path, expected_id, warnings);
    }

    /// Resume tombstoned teardowns and prune stranded checkouts whose
    /// directory, marker, or git registration went missing out of band.
    pub fn reconcile(&self) -> ReconcileReport {
        self.reconcile_with(|_, _| {})
    }

    pub fn reconcile_with(
        &self,
        mut dispose_sessions: impl FnMut(&[String], &str),
    ) -> ReconcileReport {
        let mut report = ReconcileReport::default();
        let ids: Vec<String> = {
            self.homes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .keys()
                .cloned()
                .collect()
        };
        for id in ids {
            let home = {
                self.homes
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&id)
                    .cloned()
            };
            let Some(home) = home else { continue };
            match home.state {
                WorktreeState::Tombstoned { .. } => {
                    match self.teardown(&id, "reconcile-resume", |sessions| {
                        dispose_sessions(sessions, &id)
                    }) {
                        Ok(done) => {
                            if done.warnings.is_empty() {
                                report.resumed += 1;
                            }
                            report.warnings.extend(done.warnings);
                        }
                        Err(e) => report
                            .warnings
                            .push(format!("reconcile resume {id} failed: {e:#}")),
                    }
                }
                WorktreeState::Live => {
                    if home_healthy(&home) {
                        continue;
                    }
                    // Use the same tombstone saga as an explicit teardown.
                    // A cleanup failure must leave the ledger entry so the
                    // next host can retry it instead of orphaning the PTY.
                    match self.teardown(&id, "reconcile-stranded", |sessions| {
                        dispose_sessions(sessions, &id)
                    }) {
                        Ok(done) => {
                            if done.warnings.is_empty() {
                                report.stranded += 1;
                            }
                            report.warnings.extend(done.warnings);
                        }
                        Err(e) => report
                            .warnings
                            .push(format!("reconcile stranded {id} failed: {e:#}")),
                    }
                }
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_disposes_tombstoned_sessions_before_removing_home() {
        let base = std::env::temp_dir().join(format!("dragent-reconcile-{}", uuid::Uuid::new_v4()));
        let manager = WorktreeManager::open_deferred_reconcile(&base).unwrap();
        let home = manager
            .create_standalone_session("workspace", "test")
            .unwrap();
        manager.attach_session(&home.id, "terminal-1").unwrap();
        {
            let mut homes = manager.homes.lock().unwrap();
            homes.get_mut(&home.id).unwrap().state = WorktreeState::Tombstoned {
                reason: "interrupted".to_string(),
            };
            manager.persist(&homes).unwrap();
        }
        let mut closed = Vec::new();
        let report = manager.reconcile_with(|sessions, id| {
            assert!(
                Path::new(&home.path).exists(),
                "close PTY before removing its cwd"
            );
            closed.push((id.to_string(), sessions.to_vec()));
        });
        assert_eq!(
            closed,
            vec![(home.id.clone(), vec!["terminal-1".to_string()])]
        );
        assert_eq!(report.resumed, 1);
        assert!(!Path::new(&home.path).exists());
        assert!(manager.get(&home.id).is_none());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn sanitize_collapses_and_truncates() {
        assert_eq!(
            sanitize_segment("Feature/Add Login!", 64, "x"),
            "feature-add-login"
        );
        assert_eq!(sanitize_segment("a///b___c", 64, "x"), "a-b___c");
        assert_eq!(sanitize_segment("---", 64, "fallback"), "fallback");
        assert_eq!(sanitize_segment("", 8, "fallback"), "fallback");
        assert_eq!(sanitize_segment("abcdefghij", 4, "x"), "abcd");
        assert_eq!(sanitize_branch("My/Branch!!"), "my-branch");
        assert_eq!(sanitize_flow(""), "flow");
    }

    #[test]
    fn porcelain_parses_worktree_lines_only() {
        let out =
            "worktree /repo/wt-a\nHEAD abc123\nbranch refs/heads/a\n\nworktree /repo/wt-b\nbare\n";
        assert_eq!(
            parse_worktree_paths(out),
            vec![PathBuf::from("/repo/wt-a"), PathBuf::from("/repo/wt-b")]
        );
        assert!(parse_worktree_paths("").is_empty());
    }

    #[test]
    fn marker_round_trip_and_mismatch_refusal() {
        let dir = std::env::temp_dir().join(format!("dragent-wt-marker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_marker(&dir, "wt-123").unwrap();
        assert_eq!(read_marker_id(&dir).as_deref(), Some("wt-123"));
        // Mismatched guard refuses; matching guard removes.
        let mut warnings = Vec::new();
        remove_dir_guarded(&dir, "wt-other", &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(dir.exists());
        remove_dir_guarded(&dir, "wt-123", &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(!dir.exists());
    }

    #[test]
    fn ledger_round_trip_across_reopen() {
        let org = std::env::temp_dir().join(format!("dragent-wt-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&org);
        let created = {
            let manager = WorktreeManager::open(&org).unwrap();
            assert!(manager.list("ws").is_empty());
            manager.create_standalone_session("ws", "notebook").unwrap()
        };
        let reopened = WorktreeManager::open(&org).unwrap();
        let homes = reopened.list("ws");
        assert_eq!(homes.len(), 1);
        assert_eq!(homes[0].id, created.id);
        assert_eq!(homes[0].repo, None);
        let _ = std::fs::remove_dir_all(&org);
    }
}
