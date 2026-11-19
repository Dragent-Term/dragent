//! Environment detection: git repo, runtime kind + version.

use crate::process_ext::NoWindow;
use dragent_editor::LineChangeKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Script names from package.json (yarn/npm/pnpm/bun).
pub fn package_script_names(dir: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(dir.join("package.json")) else {
        return Vec::new();
    };
    parse_package_script_names(&raw)
}

fn parse_package_script_names(json: &str) -> Vec<String> {
    let Some(scripts_start) = json.find("\"scripts\"") else {
        return Vec::new();
    };
    let after_key = &json[scripts_start..];
    let Some(brace_start) = after_key.find('{') else {
        return Vec::new();
    };
    let obj = &after_key[brace_start..];
    let mut names = Vec::new();
    let mut pos = 1;
    while pos < obj.len() {
        let c = obj[pos..].chars().next().unwrap_or('}');
        if c == '}' {
            break;
        }
        if c == ',' || c.is_whitespace() {
            pos += 1;
            continue;
        }
        if c == '"' {
            let Some((key, after_key_pos)) = parse_json_string(&obj[pos..]) else {
                break;
            };
            names.push(key);
            pos += after_key_pos;
            if let Some(colon) = obj[pos..].find(':') {
                pos += colon + 1;
            }
            if let Some(skip) = obj[pos..].find(|ch| ch == ',' || ch == '}') {
                pos += skip;
            }
        } else {
            pos += 1;
        }
    }
    names
}

fn parse_json_string(s: &str) -> Option<(String, usize)> {
    if !s.starts_with('"') {
        return None;
    }
    let mut end = 1;
    let chars: Vec<char> = s.chars().collect();
    while end < chars.len() {
        if chars[end] == '"' && chars[end - 1] != '\\' {
            return Some((chars[1..end].iter().collect(), end + 1));
        }
        end += 1;
    }
    None
}

/// Walk ancestors to find the nearest `.git` directory. Returns the git root.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    // Remote project folders have no local `.git`; their machine knows.
    if crate::remote_fs::is_remote(start) {
        return crate::remote_fs::git_root(start);
    }
    // Never climb into the home folder (a dotfiles repository there would
    // make every unversioned folder "in" it) or Dragent's own data folder;
    // git is held to the same ceilings.
    git_root_below(&normalize_path(start), &dragent_host::git_discovery_ceilings())
}

/// The nearest folder from `start` up holding `.git`, without climbing into
/// any of `ceilings` (`start` itself may be one).
fn git_root_below(start: &Path, ceilings: &[PathBuf]) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor != start && ceilings.iter().any(|ceiling| ceiling == ancestor) {
            break;
        }
        if ancestor.join(".git").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Canonicalize a path for stable comparisons (falls back to the original on failure).
pub fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Read the current branch name from `.git/HEAD`.
pub fn read_git_branch(git_root: &Path) -> Option<String> {
    let head = crate::remote_fs::read_to_string_cached(&git_root.join(".git/HEAD"))?;
    let head = head.trim();
    // "ref: refs/heads/<branch>"
    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }
    // detached HEAD — show short hash
    Some(head.chars().take(7).collect())
}

/// Reads git status for ahead/behind. Returns "(↑N ahead)" style string.
pub fn git_status_label(git_root: &Path) -> Option<String> {
    let out = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["status", "--short", "--branch", "--porcelain"])
        .current_dir(git_root)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?;
    // "## main...origin/main [ahead 3, behind 1]"
    if let Some(bracket) = first.find('[') {
        let info = &first[bracket + 1..first.rfind(']').unwrap_or(first.len())];
        let mut parts = Vec::new();
        for part in info.split(',') {
            let p = part.trim();
            if let Some(n) = p.strip_prefix("ahead ") {
                parts.push(format!("↑{n} ahead"));
            } else if let Some(n) = p.strip_prefix("behind ") {
                parts.push(format!("↓{n} behind"));
            }
        }
        if !parts.is_empty() {
            return Some(parts.join(", "));
        }
    }
    None
}

/// Return workspace-relative paths with uncommitted Git changes.
pub fn git_changed_paths(git_root: &Path) -> std::collections::HashSet<std::path::PathBuf> {
    let Ok(out) = crate::git_cmd::GitCommand::new()
        .no_window()
        .args(["status", "--short", "--untracked-files=all"])
        .current_dir(git_root)
        .output()
    else {
        return Default::default();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.get(3..))
        .map(|path| path.trim().trim_matches('"').into())
        .collect()
}

/// Parse changed new-file line ranges from both the working tree and index.
/// Zero-context hunks make this cheap and independent of the editor buffer.
pub fn git_changed_line_ranges(
    git_root: &Path,
) -> std::collections::HashMap<std::path::PathBuf, std::collections::HashMap<usize, LineChangeKind>>
{
    let mut result = std::collections::HashMap::new();
    for args in [
        &["diff", "--no-color", "--unified=0"][..],
        &["diff", "--cached", "--no-color", "--unified=0"][..],
    ] {
        let Ok(out) = crate::git_cmd::GitCommand::new()
            .no_window()
            .args(args)
            .current_dir(git_root)
            .output()
        else {
            continue;
        };
        parse_git_diff_hunks(&String::from_utf8_lossy(&out.stdout), &mut result);
    }
    result
}

fn parse_git_diff_hunks(
    diff: &str,
    result: &mut std::collections::HashMap<
        std::path::PathBuf,
        std::collections::HashMap<usize, LineChangeKind>,
    >,
) {
    let mut path: Option<std::path::PathBuf> = None;
    for line in diff.lines() {
        if let Some(next) = line.strip_prefix("+++ b/") {
            path = Some(next.into());
            continue;
        }
        let Some(hunk) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some(plus) = hunk.split_whitespace().find(|part| part.starts_with('+')) else {
            continue;
        };
        let plus = plus.trim_start_matches('+');
        let mut parts = plus.split(',');
        let new_start = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let new_count = parts
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let Some(path) = path.clone() else { continue };
        let lines = result.entry(path).or_default();
        if new_count == 0 {
            lines.insert(new_start.max(1), LineChangeKind::Deleted);
        } else {
            let old = hunk
                .split_whitespace()
                .find(|part| part.starts_with('-'))
                .and_then(|part| part.trim_start_matches('-').split(',').nth(1))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let kind = if old > 0 {
                LineChangeKind::Modified
            } else {
                LineChangeKind::Added
            };
            for line_number in new_start..new_start.saturating_add(new_count) {
                lines.insert(line_number, kind);
            }
        }
    }
}

/// Check if this git repo looks like a monorepo (has a top-level `packages/`
/// or `apps/` directory with more than one sub-directory).
pub fn is_monorepo(git_root: &Path) -> bool {
    for dir in &["packages", "apps", "services", "libs"] {
        let candidate = git_root.join(dir);
        if candidate.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&candidate) {
                let count = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .count();
                if count > 1 {
                    return true;
                }
            }
        }
    }
    // pnpm workspaces / rush / nx
    git_root.join("pnpm-workspace.yaml").exists()
        || git_root.join("rush.json").exists()
        || git_root.join("nx.json").exists()
        || git_root.join("lerna.json").exists()
}

/// A detected runtime environment.
#[derive(Clone, Debug)]
pub struct RuntimeInfo {
    pub kind: RuntimeKind,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Node,
    Deno,
    Bun,
    Rust,
    Go,
    Python,
    Java,
    Ruby,
    Php,
    Swift,
    Kotlin,
}

impl RuntimeKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Deno => "Deno",
            Self::Bun => "Bun",
            Self::Rust => "Rust",
            Self::Go => "Go",
            Self::Python => "Python",
            Self::Java => "Java",
            Self::Ruby => "Ruby",
            Self::Php => "PHP",
            Self::Swift => "Swift",
            Self::Kotlin => "Kotlin",
        }
    }
}

/// Detect all runtimes present in a directory.
///
/// File-presence checks are synchronous; version commands are spawned with a
/// short timeout so the call does not block the UI thread for long.
pub fn detect_runtimes(dir: &Path) -> Vec<RuntimeInfo> {
    let mut runtimes = Vec::new();

    // Node / Bun / Deno
    if dir.join("package.json").exists() || dir.join("node_modules").exists() {
        if dir.join("bun.lockb").exists() || dir.join("bunfig.toml").exists() {
            if let Some(v) = run_version(&["bun", "--version"]) {
                runtimes.push(RuntimeInfo {
                    kind: RuntimeKind::Bun,
                    version: v,
                });
            }
        } else if dir.join("deno.json").exists() || dir.join("deno.jsonc").exists() {
            if let Some(v) = run_version(&["deno", "--version"]) {
                runtimes.push(RuntimeInfo {
                    kind: RuntimeKind::Deno,
                    version: v,
                });
            }
        } else if let Some(v) = node_version(dir) {
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Node,
                version: v,
            });
        }
    }

    // Rust
    if dir.join("Cargo.toml").exists() {
        if let Some(v) = run_version(&["rustc", "--version"]) {
            // "rustc 1.77.0 (xxxxxx 2024-...)" → "1.77.0"
            let v = v.split_whitespace().nth(1).unwrap_or(&v).to_string();
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Rust,
                version: v,
            });
        }
    }

    // Go
    if dir.join("go.mod").exists() {
        if let Some(v) = run_version(&["go", "version"]) {
            // "go version go1.22.0 darwin/arm64" → "1.22.0"
            let v = v
                .split_whitespace()
                .find(|p| p.starts_with("go1") || p.starts_with("go2"))
                .map(|p| p.trim_start_matches("go").to_string())
                .unwrap_or(v);
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Go,
                version: v,
            });
        }
    }

    // Java / Kotlin
    if dir.join("pom.xml").exists()
        || dir.join("build.gradle").exists()
        || dir.join("build.gradle.kts").exists()
    {
        let is_kotlin = dir.join("build.gradle.kts").exists()
            || dir
                .read_dir()
                .ok()
                .and_then(|mut e| {
                    e.find(|f| {
                        f.as_ref()
                            .map(|f| f.path().extension().and_then(|x| x.to_str()) == Some("kt"))
                            .unwrap_or(false)
                    })
                })
                .is_some();

        if is_kotlin {
            if let Some(v) = run_version(&["kotlin", "-version"]) {
                let v = extract_first_semver(&v).unwrap_or(v);
                runtimes.push(RuntimeInfo {
                    kind: RuntimeKind::Kotlin,
                    version: v,
                });
            }
        }
        if let Some(v) = run_version(&["java", "-version"]) {
            // stderr: 'openjdk version "21.0.2"'
            let v = v
                .split('"')
                .nth(1)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| extract_first_semver(&v).unwrap_or(v));
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Java,
                version: v,
            });
        }
    }

    // Python
    if dir.join("pyproject.toml").exists()
        || dir.join("requirements.txt").exists()
        || dir.join("setup.py").exists()
        || dir.join("Pipfile").exists()
    {
        if let Some(v) =
            run_version(&["python3", "--version"]).or_else(|| run_version(&["python", "--version"]))
        {
            let v = v.split_whitespace().nth(1).unwrap_or(&v).to_string();
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Python,
                version: v,
            });
        }
    }

    // Ruby
    if dir.join("Gemfile").exists() {
        if let Some(v) = run_version(&["ruby", "--version"]) {
            let v = v.split_whitespace().nth(1).unwrap_or(&v).to_string();
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Ruby,
                version: v,
            });
        }
    }

    // PHP
    if dir.join("composer.json").exists() {
        if let Some(v) = run_version(&["php", "--version"]) {
            let v = v.split_whitespace().nth(1).unwrap_or(&v).to_string();
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Php,
                version: v,
            });
        }
    }

    // Swift
    if dir.join("Package.swift").exists() {
        if let Some(v) = run_version(&["swift", "--version"]) {
            let v = extract_first_semver(&v).unwrap_or(v);
            runtimes.push(RuntimeInfo {
                kind: RuntimeKind::Swift,
                version: v,
            });
        }
    }

    runtimes
}

/// Project name: from `package.json` "name" field, `Cargo.toml` [package] name,
/// or the directory name.
pub fn project_name(dir: &Path, git_root: &Path) -> String {
    // package.json
    if let Ok(raw) = std::fs::read_to_string(dir.join("package.json")) {
        if let Some(name) = json_string_field(&raw, "name") {
            if !name.is_empty() {
                return name;
            }
        }
    }
    // Cargo.toml workspace or root
    for candidate in &[dir.join("Cargo.toml"), git_root.join("Cargo.toml")] {
        if let Ok(raw) = std::fs::read_to_string(candidate) {
            if let Some(name) = toml_string_field(&raw, "name") {
                if !name.is_empty() {
                    return name;
                }
            }
        }
    }
    // fall back to git root dir name
    git_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn run_version(args: &[&str]) -> Option<String> {
    let (cmd, rest) = args.split_first()?;
    let out = Command::new(cmd)
        .no_window()
        .args(rest)
        .stderr(std::process::Stdio::piped()) // java -version writes to stderr
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !stdout.is_empty() {
        return Some(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !stderr.is_empty() {
        return Some(stderr);
    }
    None
}

fn node_version(dir: &Path) -> Option<String> {
    // Respect .node-version / .nvmrc
    for f in &[".node-version", ".nvmrc"] {
        if let Ok(v) = std::fs::read_to_string(dir.join(f)) {
            let v = v.trim().trim_start_matches('v').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    run_version(&["node", "--version"]).map(|v| v.trim_start_matches('v').to_string())
}

fn extract_first_semver(s: &str) -> Option<String> {
    for token in s.split_whitespace() {
        let t = token.trim_start_matches('v');
        if t.chars().next()?.is_ascii_digit() && t.contains('.') {
            return Some(t.to_string());
        }
    }
    None
}

fn json_string_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = raw.find(&needle)?;
    let after = raw[pos + needle.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// A one-click script/command discovered for a project directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickAction {
    pub label: String,
    pub command: String,
}

/// Discover runnable quick actions for `dir` (package.json scripts, cargo, make, go, …).
pub fn discover_quick_actions(dir: &Path) -> Vec<QuickAction> {
    let mut actions = Vec::new();

    if dir.join("package.json").exists() {
        let pm = detect_package_manager(dir);
        let mut names = package_script_names(dir);
        sort_script_names(&mut names);
        for name in names {
            actions.push(QuickAction {
                label: name.clone(),
                command: format_package_script_command(pm, &name),
            });
        }
    }

    if dir.join("Cargo.toml").exists() {
        for cmd in ["run", "test", "build", "check"] {
            actions.push(QuickAction {
                label: format!("cargo {cmd}"),
                command: format!("cargo {cmd}"),
            });
        }
    }

    for target in makefile_quick_targets(dir) {
        actions.push(QuickAction {
            label: format!("make {target}"),
            command: format!("make {target}"),
        });
    }

    if dir.join("go.mod").exists() {
        actions.push(QuickAction {
            label: "go run".into(),
            command: "go run .".into(),
        });
        actions.push(QuickAction {
            label: "go test".into(),
            command: "go test ./...".into(),
        });
    }

    actions.dedup_by(|a, b| a.command == b.command);
    actions.truncate(10);
    actions
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PackageManager {
    Npm,
    Yarn,
    Pnpm,
    Bun,
}

fn detect_package_manager(dir: &Path) -> PackageManager {
    if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        PackageManager::Bun
    } else if dir.join("pnpm-lock.yaml").exists() {
        PackageManager::Pnpm
    } else if dir.join("yarn.lock").exists() {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

fn format_package_script_command(pm: PackageManager, script: &str) -> String {
    match pm {
        PackageManager::Bun => format!("bun run {script}"),
        PackageManager::Pnpm => format!("pnpm run {script}"),
        PackageManager::Yarn => format!("yarn {script}"),
        PackageManager::Npm => format!("npm run {script}"),
    }
}

fn sort_script_names(names: &mut [String]) {
    const PREFERRED: &[&str] = &[
        "dev", "start", "serve", "preview", "watch", "build", "test", "lint", "check", "format",
    ];
    names.sort_by(|a, b| {
        let ai = PREFERRED.iter().position(|p| *p == a.as_str());
        let bi = PREFERRED.iter().position(|p| *p == b.as_str());
        match (ai, bi) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });
}

fn makefile_quick_targets(dir: &Path) -> Vec<String> {
    let path = if dir.join("Makefile").exists() {
        dir.join("Makefile")
    } else if dir.join("makefile").exists() {
        dir.join("makefile")
    } else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    const TARGETS: &[&str] = &[
        "dev", "start", "run", "build", "test", "lint", "check", "install",
    ];
    TARGETS
        .iter()
        .filter(|target| {
            content.lines().any(|line| {
                let line = line.trim();
                line.starts_with(&format!("{target}:"))
                    || (line.starts_with(".PHONY")
                        && line.split_whitespace().skip(1).any(|word| word == **target))
            })
        })
        .map(|s| s.to_string())
        .collect()
}

fn toml_string_field(raw: &str, key: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with(key) {
            if let Some(rest) = line.strip_prefix(key) {
                let rest = rest.trim();
                if let Some(rest) = rest.strip_prefix('=') {
                    let rest = rest.trim().trim_matches('"');
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod git_root_tests {
    use super::*;

    /// A folder under the home folder that isn't a repository must not be
    /// treated as part of a repository at the home folder (a dotfiles repo):
    /// its `git status` would list every file the user has.
    #[test]
    fn a_repository_at_a_ceiling_does_not_claim_folders_below_it() {
        let home = tempfile::tempdir().unwrap();
        let home = normalize_path(home.path());
        std::fs::create_dir(home.join(".git")).unwrap();
        let session = home.join(".dragent/automation-sessions/create-1");
        std::fs::create_dir_all(&session).unwrap();
        let project = home.join("dev/app");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        let ceilings = [home.clone()];

        assert_eq!(git_root_below(&session, &ceilings), None);
        assert_eq!(git_root_below(&project.join("src"), &ceilings), Some(project));
        // Opening the home folder itself still finds its repository.
        assert_eq!(git_root_below(&home, &ceilings), Some(home.clone()));
        // Without the ceiling the session folder would have been "in" it.
        assert_eq!(git_root_below(&session, &[]), Some(home));
    }

    /// git itself honours the same ceiling, so commands run in such a folder
    /// fail fast instead of scanning the home folder.
    #[test]
    fn git_does_not_climb_past_the_ceiling() {
        let home = tempfile::tempdir().unwrap();
        let home = normalize_path(home.path());
        let init = Command::new("git").arg("init").arg("-q").arg(&home).status().unwrap();
        assert!(init.success());
        let session = home.join("sessions/one");
        std::fs::create_dir_all(&session).unwrap();
        let toplevel = |ceiling: Option<&Path>| {
            let mut command = Command::new("git");
            command.args(["rev-parse", "--show-toplevel"]).current_dir(&session);
            command.env_remove("GIT_CEILING_DIRECTORIES");
            if let Some(ceiling) = ceiling {
                command.env("GIT_CEILING_DIRECTORIES", ceiling);
            }
            command.output().unwrap().status.success()
        };
        assert!(toplevel(None), "without a ceiling git finds the outer repository");
        assert!(!toplevel(Some(&home)), "with the ceiling it does not");
    }
}
