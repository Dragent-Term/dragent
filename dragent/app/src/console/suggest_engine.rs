/// Advanced IntelliSense-style ghost-text suggestion engine.
///
/// Priority cascade (first matching provider wins):
///   0. Slash-command completions — /agent, /shell
///   1. History — the newest earlier command that starts with the buffer,
///      from this session, then the shell's history file and other sessions
///      (fish-style; `cd` entries only when the directory still exists)
///   2. `cd` path completions — history-weighted, git-rooted, then alphabetical
///   3. Package script completions — yarn/npm run/pnpm run from package.json
///   4. Makefile target completions — `make <target>`
///   5. Git branch completions — checkout/switch/merge/rebase
///   6. Command-DB completions — static specs, Dragent's CLI and agent CLIs
///   7. File-path arguments for any command
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use web_time::Instant;

// ─── CD directory visit history ───────────────────────────────────────────────

#[derive(Clone)]
struct CdVisit {
    count: usize,
    last_visit: Instant,
}

impl CdVisit {
    fn score(&self) -> f64 {
        // Recency decay: seconds since last visit (inverted, capped at 1 week).
        let age_secs = self.last_visit.elapsed().as_secs_f64().min(604_800.0);
        let recency = 1.0 - (age_secs / 604_800.0);
        (self.count as f64) * 3.0 + recency * 10.0
    }
}

// ─── Package script ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PackageScript {
    pub name: String,
    pub command: String,
}

// ─── SuggestEngine ───────────────────────────────────────────────────────────

pub struct SuggestEngine {
    cd_history: HashMap<PathBuf, CdVisit>,
    cached_package_scripts: Vec<PackageScript>,
    cached_makefile_targets: Vec<String>,
    cached_git_branches: Vec<String>,
    cached_package_manager: Option<PackageManager>,
    last_cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
enum PackageManager {
    Yarn,
    Pnpm,
    Npm,
}

impl Default for SuggestEngine {
    fn default() -> Self {
        Self {
            cd_history: HashMap::new(),
            cached_package_scripts: Vec::new(),
            cached_makefile_targets: Vec::new(),
            cached_git_branches: Vec::new(),
            cached_package_manager: None,
            last_cwd: None,
        }
    }
}

impl SuggestEngine {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Cache refresh ─────────────────────────────────────────────────────────

    /// Called whenever the working directory changes (after each command that
    /// might have been a `cd`, or on shell CWD updates from OSC 7).
    pub fn on_cwd_changed(&mut self, cwd: &Path) {
        if self.last_cwd.as_deref() == Some(cwd) {
            return;
        }
        self.last_cwd = Some(cwd.to_path_buf());

        self.cached_package_scripts = read_package_scripts(cwd);
        self.cached_makefile_targets = read_makefile_targets(cwd);
        self.cached_git_branches = read_git_branches(cwd);
        self.cached_package_manager = detect_package_manager(cwd);
    }

    /// Called when the user successfully runs a `cd <dir>` command so we can
    /// update the directory visit history.
    pub fn record_cd(&mut self, dir: PathBuf) {
        let visit = self.cd_history.entry(dir).or_insert(CdVisit {
            count: 0,
            last_visit: Instant::now(),
        });
        visit.count += 1;
        visit.last_visit = Instant::now();
    }

    // ── Primary suggest entry-point ───────────────────────────────────────────

    /// Returns a ghost-text suffix for the given `buffer`, or `None` if no
    /// high-confidence suggestion is available.
    ///
    /// `history` is the shell command history (oldest-first), used as a
    /// fallback provider.
    pub fn suggest(&self, buffer: &str, history: &[String], cwd: &Path) -> Option<String> {
        if buffer.is_empty() {
            return self.suggest_empty(cwd);
        }

        // 0. Slash-command provider
        if let Some(suffix) = suggest_slash_command(buffer) {
            return Some(suffix);
        }

        // 1. History: what the user actually runs beats a generic guess.
        if let Some(suffix) = self.suggest_from_history(buffer, history, cwd) {
            return Some(suffix);
        }

        // 2. cd path provider
        if let Some(suffix) = self.suggest_cd(buffer, cwd) {
            return Some(suffix);
        }

        // 2. Package script provider (yarn/npm/pnpm)
        if let Some(suffix) = self.suggest_package_script(buffer) {
            return Some(suffix);
        }

        // 3. Makefile target provider
        if let Some(suffix) = self.suggest_make(buffer) {
            return Some(suffix);
        }

        // 4. Git branch provider
        if let Some(suffix) = self.suggest_git_branch(buffer) {
            return Some(suffix);
        }

        // 5. Command-DB provider (static specs for 30+ tools)
        if let Some(suffix) = crate::console::cmd_suggest::ghost_suffix_from_cmd(buffer) {
            return Some(suffix);
        }

        // 7. General file-path argument provider — completes the path the user is
        //    typing as an argument to ANY command (cat/ls/vim/…), against the
        //    filesystem. Runs after the command-specific providers so a git
        //    branch or subcommand still wins.
        suggest_path_arg(buffer, cwd)
    }

    // ── Provider 0: empty buffer ──────────────────────────────────────────────

    fn suggest_empty(&self, cwd: &Path) -> Option<String> {
        use crate::console::dir_context;

        if dir_context::is_git_repo(cwd) {
            return Some("git status".to_string());
        }
        match self.cached_package_manager {
            Some(PackageManager::Yarn) => return Some("yarn dev".to_string()),
            Some(PackageManager::Pnpm) => return Some("pnpm dev".to_string()),
            Some(PackageManager::Npm) => return Some("npm start".to_string()),
            None => {}
        }
        if dir_context::is_rust_project(cwd) {
            return Some("cargo check".to_string());
        }
        if dir_context::is_python_project(cwd) {
            return Some("python ".to_string());
        }
        if !self.cached_makefile_targets.is_empty() {
            return Some(format!("make {}", self.cached_makefile_targets[0]));
        }
        None
    }

    // ── Provider 2: cd directories ───────────────────────────────────────────

    fn suggest_cd(&self, buffer: &str, cwd: &Path) -> Option<String> {
        use dragent_parser::lite_parse;

        let (group, _) = lite_parse(buffer);
        let cmd = group.pipelines.first()?.commands.first()?;
        let parts = &cmd.parts;

        if parts.first().map(|p| p.item.as_str()) != Some("cd") {
            return None;
        }

        let has_trailing_ws = cmd.trailing_whitespace.is_some();

        // Extract the typed path, stripping surrounding quotes when present
        // so that `cd "my dir"` correctly completes against "my dir".
        let path_fragment: String = match parts.get(1) {
            Some(arg) => {
                let raw = &arg.item;
                if (raw.starts_with('\'') && raw.ends_with('\''))
                    || (raw.starts_with('"') && raw.ends_with('"'))
                {
                    raw[1..raw.len() - 1].to_string()
                } else {
                    raw.clone()
                }
            }
            None => String::new(),
        };

        // A nested fragment (`src/co`, `~/dev/`) completes inside that folder.
        if let Some(slash) = path_fragment.rfind('/') {
            let scan_dir = resolve_scan_dir(&path_fragment[..=slash], cwd);
            let prefix = &path_fragment[slash + 1..];
            let candidates = self.cd_candidates(&scan_dir, prefix);
            let best = best_candidate(candidates)?;
            return Some(format!("{}/", &best.name[prefix.len()..]));
        }

        let candidates = self.cd_candidates(cwd, &path_fragment);
        let best = best_candidate(candidates)?;

        if !has_trailing_ws && path_fragment.is_empty() {
            // User typed just "cd" — ghost text shows " <dir>"
            Some(format!(" {}/", best.name))
        } else if best.name.starts_with(path_fragment.as_str()) {
            Some(format!("{}/", &best.name[path_fragment.len()..]))
        } else {
            None
        }
    }

    fn cd_candidates(&self, cwd: &Path, prefix: &str) -> Vec<CdCandidate> {
        let mut candidates: Vec<CdCandidate> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // First: subdirectories of CWD that match the prefix
        if let Ok(entries) = fs::read_dir(cwd) {
            let mut dir_names: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    if e.file_type().ok()?.is_dir() {
                        let name = e.file_name().to_string_lossy().to_string();
                        if !name.starts_with('.') || prefix.starts_with('.') {
                            Some(name)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .filter(|name| name.starts_with(prefix))
                .collect();

            dir_names.sort();

            for name in dir_names {
                let full_path = cwd.join(&name);
                let history_score = self
                    .cd_history
                    .get(&full_path)
                    .map(|v| v.score())
                    .unwrap_or(0.0);

                // Git repos get a 5-point bonus to float to top when no history
                let is_git = full_path.join(".git").exists();
                let git_bonus = if is_git { 5.0 } else { 0.0 };

                candidates.push(CdCandidate {
                    name: name.clone(),
                    score: history_score + git_bonus,
                });
                seen.insert(name);
            }
        }

        // Also include any history entries that are children of the parent dir
        // (for when the user has cd'd into dirs that no longer exist, or used
        // absolute paths)
        for (path, visit) in &self.cd_history {
            if let Some(parent) = path.parent() {
                if parent == cwd {
                    if let Some(name) = path.file_name() {
                        let name_str = name.to_string_lossy().to_string();
                        if name_str.starts_with(prefix) && !seen.contains(&name_str) {
                            candidates.push(CdCandidate {
                                name: name_str.clone(),
                                score: visit.score(),
                            });
                            seen.insert(name_str);
                        }
                    }
                }
            }
        }

        candidates
    }

    // ── Provider 3: package scripts ───────────────────────────────────────────

    fn suggest_package_script(&self, buffer: &str) -> Option<String> {
        if self.cached_package_scripts.is_empty() {
            return None;
        }

        // Detect which runner is being used
        let (runner_prefix, typed_script) = extract_script_prefix(buffer)?;

        // Find the best matching script
        let best = self
            .cached_package_scripts
            .iter()
            .filter(|s| s.name.starts_with(typed_script))
            .min_by_key(|s| s.name.len())?; // shortest match = most specific

        if best.name == typed_script {
            return None; // already complete
        }

        let suffix = &best.name[typed_script.len()..];
        if buffer == runner_prefix.trim_end() {
            // "yarn" alone → suggest "yarn dev" etc
            Some(format!(" {}", best.name))
        } else if buffer.ends_with(char::is_whitespace) {
            // "yarn run " → suggest the script name
            Some(best.name.clone())
        } else {
            // "yarn d" → suggest "ev"
            Some(suffix.to_string())
        }
    }

    // ── Provider 4: Makefile targets ──────────────────────────────────────────

    fn suggest_make(&self, buffer: &str) -> Option<String> {
        if self.cached_makefile_targets.is_empty() {
            return None;
        }

        let target_fragment = if buffer == "make" {
            return Some(format!(" {}", self.cached_makefile_targets[0]));
        } else if let Some(rest) = buffer.strip_prefix("make ") {
            rest
        } else {
            return None;
        };

        // Skip flags
        if target_fragment.starts_with('-') {
            return None;
        }

        let best = self
            .cached_makefile_targets
            .iter()
            .find(|t| t.starts_with(target_fragment))?;

        if best == target_fragment {
            return None;
        }

        Some(best[target_fragment.len()..].to_string())
    }

    // ── Provider 5: git branches ──────────────────────────────────────────────

    fn suggest_git_branch(&self, buffer: &str) -> Option<String> {
        if self.cached_git_branches.is_empty() {
            return None;
        }

        // Match "git checkout <partial>", "git switch <partial>",
        // "git merge <partial>", "git rebase <partial>"
        let branch_fragment = extract_git_branch_fragment(buffer)?;

        let best = self
            .cached_git_branches
            .iter()
            .find(|b| b.starts_with(branch_fragment))?;

        if best.as_str() == branch_fragment {
            return None;
        }

        if branch_fragment.is_empty() {
            Some(best.clone())
        } else {
            Some(best[branch_fragment.len()..].to_string())
        }
    }

    // ── Provider 1: history ───────────────────────────────────────────────────

    /// The rest of the newest earlier command that starts with `buffer`:
    /// this session's own commands first, then the shell's history file and
    /// the other sessions'. Case-sensitive, like the shell, so accepting it
    /// never produces a command the user didn't type. Multi-line commands are
    /// skipped (the ghost is one line), and so is a `cd` whose directory is
    /// gone.
    fn suggest_from_history(&self, buffer: &str, history: &[String], cwd: &Path) -> Option<String> {
        if buffer.trim().is_empty() {
            return None;
        }
        let pick = |entry: &str| -> Option<String> {
            let rest = entry.strip_prefix(buffer)?;
            if rest.is_empty() || entry.contains('\n') || !history_cd_target_exists(entry, cwd) {
                return None;
            }
            Some(rest.to_string())
        };
        history
            .iter()
            .rev()
            .find_map(|entry| pick(entry))
            .or_else(|| crate::console::shell_history::find_newest(pick))
    }
}

/// `cd DIR` from history is only worth suggesting while DIR exists from here.
fn history_cd_target_exists(entry: &str, cwd: &Path) -> bool {
    let Some(target) = entry.strip_prefix("cd ") else {
        return true;
    };
    let target = target
        .split(|c| matches!(c, '&' | ';' | '|'))
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '\'' || c == '"');
    if target.is_empty() || target == "-" || target == "~" || target.contains('$') {
        return true;
    }
    let path = match target.strip_prefix("~/") {
        Some(rest) => match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => return true,
        },
        None => cwd.join(target),
    };
    path.is_dir()
}

fn best_candidate(candidates: Vec<CdCandidate>) -> Option<CdCandidate> {
    // Highest score; on a tie the first (alphabetical) one, not the last.
    candidates.into_iter().fold(None, |best: Option<CdCandidate>, c| match best {
        Some(b) if b.score >= c.score => Some(b),
        _ => Some(c),
    })
}

// ─── Candidate helper ─────────────────────────────────────────────────────────

struct CdCandidate {
    name: String,
    score: f64,
}

// ─── Cache readers ────────────────────────────────────────────────────────────

fn read_package_scripts(cwd: &Path) -> Vec<PackageScript> {
    let pkg_path = cwd.join("package.json");
    let Ok(contents) = fs::read_to_string(&pkg_path) else {
        return Vec::new();
    };

    parse_package_scripts(&contents)
}

fn parse_package_scripts(json: &str) -> Vec<PackageScript> {
    // Lightweight hand-rolled JSON parser for the "scripts" key.
    // We avoid pulling in serde_json to keep the dependency footprint small.
    let Some(scripts_start) = find_key(json, "\"scripts\"") else {
        return Vec::new();
    };

    let after_key = &json[scripts_start..];
    let Some(brace_start) = after_key.find('{') else {
        return Vec::new();
    };

    let obj = &after_key[brace_start..];
    let mut scripts = Vec::new();
    let mut pos = 1; // skip opening '{'

    while pos < obj.len() {
        // Skip whitespace and commas
        let c = obj[pos..].chars().next().unwrap_or('}');
        if c == '}' {
            break;
        }
        if c == ',' || c.is_whitespace() {
            pos += 1;
            continue;
        }
        if c == '"' {
            // Parse key
            let Some((key, after_key_pos)) = parse_json_string(&obj[pos..]) else {
                break;
            };
            pos += after_key_pos;

            // Skip colon
            let Some(colon) = obj[pos..].find(':') else {
                break;
            };
            pos += colon + 1;

            // Skip whitespace
            while obj[pos..].starts_with(char::is_whitespace) {
                pos += 1;
            }

            // Parse value string
            if obj[pos..].starts_with('"') {
                if let Some((value, after_val)) = parse_json_string(&obj[pos..]) {
                    scripts.push(PackageScript {
                        name: key,
                        command: value,
                    });
                    pos += after_val;
                } else {
                    break;
                }
            } else {
                // Non-string value — skip to next comma/brace
                if let Some(skip) = obj[pos..].find(|c| c == ',' || c == '}') {
                    pos += skip;
                } else {
                    break;
                }
            }
        } else {
            pos += 1;
        }
    }

    // Sort by name length (shorter = more likely to be typed)
    scripts.sort_by(|a, b| a.name.len().cmp(&b.name.len()).then(a.name.cmp(&b.name)));
    scripts
}

fn find_key(json: &str, key: &str) -> Option<usize> {
    json.find(key).map(|i| i + key.len())
}

fn parse_json_string(s: &str) -> Option<(String, usize)> {
    if !s.starts_with('"') {
        return None;
    }
    let mut result = String::new();
    let mut chars = s[1..].char_indices();
    let mut escaped = false;

    for (i, c) in chars.by_ref() {
        if escaped {
            match c {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                '"' => result.push('"'),
                '\\' => result.push('\\'),
                _ => {
                    result.push('\\');
                    result.push(c);
                }
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some((result, i + 2)); // +1 for opening quote, +1 to advance past current char
        } else {
            result.push(c);
        }
    }
    None
}

fn read_makefile_targets(cwd: &Path) -> Vec<String> {
    let makefile_path = cwd.join("Makefile");
    let Ok(contents) = fs::read_to_string(&makefile_path) else {
        return Vec::new();
    };

    parse_makefile_targets(&contents)
}

fn parse_makefile_targets(content: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in content.lines() {
        // A Makefile target is a line that starts with a non-whitespace char,
        // contains a colon, and is not a variable assignment.
        if line.starts_with(char::is_whitespace) || line.starts_with('#') {
            continue;
        }

        let Some(colon_pos) = line.find(':') else {
            continue;
        };
        let target = line[..colon_pos].trim();

        // Skip internal targets (.PHONY, .DEFAULT, etc.), variable assignments,
        // and targets with spaces (multi-target lines are rarer to complete).
        if target.is_empty()
            || target.starts_with('.')
            || target.contains('=')
            || target.contains(' ')
            || target.contains('$')
            || target.contains('\\')
        {
            continue;
        }

        if seen.insert(target.to_string()) {
            targets.push(target.to_string());
        }
    }

    // Conventional targets bubble to the top
    let priority = [
        "build", "test", "clean", "install", "run", "all", "lint", "fmt", "help", "docker",
        "deploy",
    ];
    targets.sort_by(|a, b| {
        let pa = priority
            .iter()
            .position(|&p| p == a.as_str())
            .unwrap_or(usize::MAX);
        let pb = priority
            .iter()
            .position(|&p| p == b.as_str())
            .unwrap_or(usize::MAX);
        pa.cmp(&pb).then(a.cmp(b))
    });

    targets
}

fn read_git_branches(cwd: &Path) -> Vec<String> {
    let git_heads = cwd.join(".git").join("refs").join("heads");
    if !git_heads.exists() {
        // Traverse up to find git root
        let mut current = cwd.parent();
        while let Some(parent) = current {
            let alt = parent.join(".git").join("refs").join("heads");
            if alt.exists() {
                return collect_branches_from_dir(&alt);
            }
            current = parent.parent();
        }
        return Vec::new();
    }
    collect_branches_from_dir(&git_heads)
}

fn collect_branches_from_dir(heads_dir: &Path) -> Vec<String> {
    let mut branches: Vec<String> = Vec::new();
    collect_branches_recursive(heads_dir, heads_dir, &mut branches);
    branches.sort();
    branches
}

fn collect_branches_recursive(base: &Path, dir: &Path, branches: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_branches_recursive(base, &path, branches);
        } else if let Ok(rel) = path.strip_prefix(base) {
            let branch = rel.to_string_lossy().replace('\\', "/");
            branches.push(branch);
        }
    }
}

fn detect_package_manager(cwd: &Path) -> Option<PackageManager> {
    if cwd.join("yarn.lock").exists() {
        return Some(PackageManager::Yarn);
    }
    if cwd.join("pnpm-lock.yaml").exists() {
        return Some(PackageManager::Pnpm);
    }
    if cwd.join("package-lock.json").exists() || cwd.join("package.json").exists() {
        return Some(PackageManager::Npm);
    }
    None
}

// ─── Prefix extraction helpers ────────────────────────────────────────────────

/// Returns `(runner_prefix, typed_script_fragment)` for yarn/npm/pnpm run commands.
fn extract_script_prefix(buffer: &str) -> Option<(&str, &str)> {
    // "yarn <script>" — yarn runs scripts directly
    if buffer == "yarn" {
        return Some(("yarn", ""));
    }
    if let Some(rest) = buffer.strip_prefix("yarn ") {
        // But don't intercept yarn subcommands like "yarn add", "yarn install"
        let first_word = rest.split_whitespace().next().unwrap_or(rest);
        let yarn_subcommands = [
            "add",
            "remove",
            "install",
            "upgrade",
            "workspace",
            "workspaces",
            "link",
            "unlink",
            "publish",
            "pack",
            "cache",
            "config",
            "info",
            "audit",
            "outdated",
            "why",
            "create",
            "exec",
        ];
        if yarn_subcommands.contains(&first_word) {
            return None;
        }
        let fragment = if buffer.ends_with(' ') {
            ""
        } else {
            rest.split_whitespace().last().unwrap_or("")
        };
        return Some(("yarn", fragment));
    }

    // "npm run <script>"
    if buffer == "npm run" {
        return Some(("npm run", ""));
    }
    if let Some(rest) = buffer.strip_prefix("npm run ") {
        let fragment = if buffer.ends_with(' ') { "" } else { rest };
        return Some(("npm run", fragment));
    }

    // "pnpm run <script>" or "pnpm <script>"
    if buffer == "pnpm" {
        return Some(("pnpm", ""));
    }
    if let Some(rest) = buffer.strip_prefix("pnpm ") {
        let first_word = rest.split_whitespace().next().unwrap_or(rest);
        let pnpm_subcommands = [
            "add", "remove", "install", "update", "link", "unlink", "publish", "pack", "audit",
            "outdated", "create", "exec", "dlx", "store", "prune", "patch", "fetch", "rebuild",
        ];
        if pnpm_subcommands.contains(&first_word) {
            return None;
        }
        if let Some(run_rest) = rest.strip_prefix("run ") {
            let fragment = if buffer.ends_with(' ') { "" } else { run_rest };
            return Some(("pnpm run", fragment));
        }
        let fragment = if buffer.ends_with(' ') {
            ""
        } else {
            rest.split_whitespace().last().unwrap_or("")
        };
        return Some(("pnpm", fragment));
    }

    None
}

/// Extracts the branch fragment being typed for git branch-related commands.
fn extract_git_branch_fragment(buffer: &str) -> Option<&str> {
    let branch_cmds = [
        "git checkout ",
        "git switch ",
        "git merge ",
        "git rebase ",
        "git branch -d ",
        "git branch -D ",
        "git push origin ",
        "git push --force-with-lease origin ",
    ];

    for prefix in &branch_cmds {
        if buffer == prefix.trim_end() {
            return Some("");
        }
        if let Some(rest) = buffer.strip_prefix(prefix) {
            // Don't suggest if the user typed a flag
            if rest.starts_with('-') {
                return None;
            }
            return Some(rest);
        }
    }
    None
}

// ─── Slash-command provider ───────────────────────────────────────────────────

const SLASH_COMMANDS: &[&str] = &[
    "/agent",
    "/shell",
    "/clear",
    "/plan",
    "/debug",
    "/multitask",
    "/ask",
    "/model",
];

fn suggest_slash_command(buffer: &str) -> Option<String> {
    let trimmed = buffer.trim();
    if !trimmed.starts_with('/') || trimmed.contains(' ') {
        return None;
    }
    for &cmd in SLASH_COMMANDS {
        if cmd.starts_with(trimmed) && cmd.len() > trimmed.len() {
            return Some(cmd[trimmed.len()..].to_string());
        }
    }
    None
}

/// Complete the file path the user is typing as an argument to any command,
/// against the filesystem — so `cat src/ma` ghosts `in.rs`, `vim comp` ghosts
/// `onents/` (dir → trailing slash), etc. Returns the completion suffix.
///
/// Only fires for the LAST whitespace-separated token (never the command name),
/// skips flags (`-x`), and only when a real filesystem match exists — so it
/// stays quiet for non-path arguments like `git status`.
fn suggest_path_arg(buffer: &str, cwd: &Path) -> Option<String> {
    // Need "<command> <fragment>" with a non-empty fragment (no trailing space).
    let last_space = buffer.rfind(' ')?;
    let fragment = &buffer[last_space + 1..];
    if fragment.is_empty() || fragment.starts_with('-') {
        return None;
    }

    // Split into the directory to scan and the prefix within it.
    let (scan_dir, prefix): (PathBuf, &str) = match fragment.rfind('/') {
        Some(slash) => (
            resolve_scan_dir(&fragment[..=slash], cwd),
            &fragment[slash + 1..],
        ),
        None => (cwd.to_path_buf(), fragment),
    };

    let prefix_lower = prefix.to_lowercase();
    let prefix_chars = prefix.chars().count();
    let mut best: Option<(String, bool)> = None; // (name, is_dir)
    for entry in fs::read_dir(&scan_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden files only when the user is explicitly typing a dot.
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        if name.chars().count() <= prefix_chars || !name.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        // Prefer the shortest completion, then alphabetical — a stable pick.
        let better = match &best {
            None => true,
            Some((b, _)) => name.len() < b.len() || (name.len() == b.len() && name < *b),
        };
        if better {
            best = Some((name, is_dir));
        }
    }

    let (name, is_dir) = best?;
    let mut suffix: String = name.chars().skip(prefix_chars).collect();
    if is_dir {
        suffix.push('/');
    }
    Some(suffix)
}

/// Resolve the directory portion of a path fragment (with trailing `/`) to an
/// absolute path to scan, honoring `~/`, absolute, and cwd-relative forms.
fn resolve_scan_dir(dir_part: &str, cwd: &Path) -> PathBuf {
    if let Some(rest) = dir_part.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if dir_part.starts_with('/') {
        return PathBuf::from(dir_part);
    }
    cwd.join(dir_part)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_package_scripts_basic() {
        let json =
            r#"{"name":"app","scripts":{"dev":"vite","build":"vite build","test":"vitest"}}"#;
        let scripts = parse_package_scripts(json);
        assert!(scripts.iter().any(|s| s.name == "dev"));
        assert!(scripts.iter().any(|s| s.name == "build"));
        assert!(scripts.iter().any(|s| s.name == "test"));
    }

    #[test]
    fn parse_makefile_targets_basic() {
        let content = "build:\n\tcargo build\n\ntest:\n\tcargo test\n\n.PHONY: build test\n";
        let targets = parse_makefile_targets(content);
        assert!(targets.contains(&"build".to_string()));
        assert!(targets.contains(&"test".to_string()));
        assert!(!targets.iter().any(|t| t.starts_with('.')));
    }

    #[test]
    fn extract_script_prefix_yarn() {
        assert_eq!(extract_script_prefix("yarn "), Some(("yarn", "")));
        assert_eq!(extract_script_prefix("yarn dev"), Some(("yarn", "dev")));
        assert_eq!(extract_script_prefix("yarn add"), None);
    }

    #[test]
    fn extract_script_prefix_npm() {
        assert_eq!(extract_script_prefix("npm run "), Some(("npm run", "")));
        assert_eq!(extract_script_prefix("npm run bu"), Some(("npm run", "bu")));
    }

    #[test]
    fn extract_git_branch_fragment_checkout() {
        assert_eq!(extract_git_branch_fragment("git checkout "), Some(""));
        assert_eq!(
            extract_git_branch_fragment("git checkout main"),
            Some("main")
        );
        assert_eq!(extract_git_branch_fragment("git checkout -b"), None);
    }

    #[test]
    fn history_fallback() {
        let engine = SuggestEngine::new();
        let history = vec![
            "git status".to_string(),
            "git push origin main".to_string(),
            "git push origin main".to_string(),
        ];
        // "git push" should expand to " origin main" (most frequent)
        let result = engine.suggest("git push", &history, Path::new("/tmp"));
        assert_eq!(result, Some(" origin main".to_string()));
    }

    #[test]
    fn history_beats_static_specs() {
        let engine = SuggestEngine::new();
        let history = vec!["git commit --amend".to_string()];
        assert_eq!(
            engine.suggest("git c", &history, Path::new("/tmp")),
            Some("ommit --amend".to_string())
        );
    }

    #[test]
    fn history_is_newest_first_and_case_sensitive() {
        let engine = SuggestEngine::new();
        let history = vec!["npm run build".to_string(), "npm run dev".to_string()];
        assert_eq!(
            engine.suggest("npm r", &history, Path::new("/tmp")),
            Some("un dev".to_string())
        );
        // Not "un dev" glued onto "NPM r".
        assert_ne!(
            engine.suggest("NPM r", &history, Path::new("/tmp")),
            Some("un dev".to_string())
        );
    }

    #[test]
    fn history_skips_cd_to_missing_dir_and_multiline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        let engine = SuggestEngine::new();
        let history = vec!["cd real".to_string(), "cd gone-for-good".to_string()];
        assert_eq!(engine.suggest("cd ", &history, dir.path()), Some("real".to_string()));
        let history = vec!["echo hi".to_string(), "echo x\nb".to_string()];
        assert_eq!(engine.suggest("echo ", &history, dir.path()), Some("hi".to_string()));
    }

    #[test]
    fn cd_completes_nested_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(dir.path().join("src/config.rs"), "").unwrap();
        let engine = SuggestEngine::new();
        assert_eq!(engine.suggest("cd sr", &[], dir.path()), Some("c/".to_string()));
        assert_eq!(engine.suggest("cd src/co", &[], dir.path()), Some("mponents/".to_string()));
    }

    #[test]
    fn slash_command_ghost_partial() {
        assert_eq!(suggest_slash_command("/"), Some("agent".to_string()));
        assert_eq!(suggest_slash_command("/a"), Some("gent".to_string()));
        assert_eq!(suggest_slash_command("/ag"), Some("ent".to_string()));
        assert_eq!(suggest_slash_command("/age"), Some("nt".to_string()));
        assert_eq!(suggest_slash_command("/agen"), Some("t".to_string()));
        assert_eq!(suggest_slash_command("/s"), Some("hell".to_string()));
        assert_eq!(suggest_slash_command("/sh"), Some("ell".to_string()));
    }

    #[test]
    fn slash_command_ghost_exact_no_suffix() {
        assert_eq!(suggest_slash_command("/agent"), None);
        assert_eq!(suggest_slash_command("/shell"), None);
    }

    #[test]
    fn slash_command_ghost_no_match_for_shell_command() {
        assert_eq!(suggest_slash_command("git status"), None);
        assert_eq!(suggest_slash_command("/agent extra"), None);
    }
}
