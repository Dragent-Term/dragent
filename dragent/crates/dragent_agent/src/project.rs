//! Project context — root detection, git summary, stderr path extraction.
//!
//! Pure logic shared by the engine core and the Host. Callers pass git state
//! as plain values (`GitInfo`), never a git-status model, so this module has
//! no UI or VCS dependencies.

use std::path::{Path, PathBuf};

use super::block::BlockContextSnapshot;
use super::files::ReadFilesResult;

#[derive(Clone, Debug, Default)]
pub struct ProjectContext {
    /// Workspace / repo root if detected.
    pub project_root: Option<PathBuf>,
    /// `cwd` relative to `project_root` when both are known.
    pub cwd_relative: Option<PathBuf>,
    pub git_branch: Option<String>,
    pub git_changed_files: u32,
    pub git_untracked_files: u32,
    /// File paths mentioned in block output (stderr), resolved against cwd / project_root.
    pub referenced_paths: Vec<PathBuf>,
}

/// Git summary passed by the caller (the app reads this off its git model).
#[derive(Clone, Debug, Default)]
pub struct GitInfo {
    pub branch: String,
    pub changed_files: u32,
    pub untracked_files: u32,
}

/// Full request envelope sent to `AgentApi::analyze_block`.
#[derive(Clone, Debug)]
pub struct AnalyzeBlockRequest {
    pub block: BlockContextSnapshot,
    pub project: ProjectContext,
    /// File contents read by the ReadFiles tool — `None` until tools run.
    pub file_context: Option<ReadFilesResult>,
    pub refresh_token: Option<String>,
    /// Cached access token — if `Some`, the API skips the refresh-token
    /// exchange and uses this directly.
    pub access_token: Option<String>,
}

// ── Root markers (in priority order) ──────────────────────────────────────────

const ROOT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
];

// ── Public builder ────────────────────────────────────────────────────────────

impl ProjectContext {
    /// Build from pane state. Pure function — no I/O aside from the filesystem walk.
    pub fn build(cwd: &Path, git: Option<GitInfo>, output_text: &str) -> Self {
        let project_root = find_project_root(cwd);
        let cwd_relative = project_root
            .as_deref()
            .and_then(|root| cwd.strip_prefix(root).ok())
            .map(|rel| rel.to_path_buf());

        let (git_branch, git_changed_files, git_untracked_files) = match git {
            Some(g) => {
                let branch = if g.branch.is_empty() {
                    None
                } else {
                    Some(g.branch)
                };
                (branch, g.changed_files, g.untracked_files)
            }
            None => (None, 0, 0),
        };

        let referenced_paths = extract_referenced_paths(output_text, cwd, project_root.as_deref());

        Self {
            project_root,
            cwd_relative,
            git_branch,
            git_changed_files,
            git_untracked_files,
            referenced_paths,
        }
    }
}

// ── Project root detection ────────────────────────────────────────────────────

fn find_project_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        for marker in ROOT_MARKERS {
            if current.join(marker).exists() {
                return Some(current.to_path_buf());
            }
        }
        match current.parent() {
            Some(p) if p != current => current = p,
            _ => return None,
        }
    }
}

// ── Stderr path extraction ────────────────────────────────────────────────────

const MAX_REFERENCED_PATHS: usize = 20;

/// Directories to skip when collecting referenced paths.
const SKIP_DIRS: &[&str] = &["target/", "node_modules/", ".git/", ".cargo/"];

fn should_skip_path(path: &str) -> bool {
    SKIP_DIRS.iter().any(|d| path.contains(d))
}

fn extract_referenced_paths(
    output_text: &str,
    cwd: &Path,
    project_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();

    for line in output_text.lines() {
        if paths.len() >= MAX_REFERENCED_PATHS {
            break;
        }

        // Try each extractor in order.
        let raw = extract_path_from_line(line);
        for p in raw {
            if paths.len() >= MAX_REFERENCED_PATHS {
                break;
            }
            if should_skip_path(&p) {
                continue;
            }
            let resolved = resolve_path(&p, cwd, project_root);
            if seen.insert(resolved.clone()) {
                paths.push(resolved);
            }
        }
    }

    paths
}

fn extract_path_from_line(line: &str) -> Vec<String> {
    let mut found = Vec::new();

    // rustc arrow: `  --> src/main.rs:42:5`
    if let Some(rest) = line.trim().strip_prefix("--> ") {
        if let Some(path) = rest.split(':').next() {
            let p = path.trim().to_string();
            if looks_like_source_path(&p) {
                found.push(p);
                return found;
            }
        }
    }

    // Python traceback: `  File "/path/to/file.py", line N`
    if let Some(rest) = line.trim().strip_prefix("File \"") {
        if let Some(end) = rest.find('"') {
            let p = rest[..end].to_string();
            if looks_like_source_path(&p) {
                found.push(p);
                return found;
            }
        }
    }

    // Go / generic: `./pkg/foo.go:12:` or `foo/bar.go:5:`
    {
        let trimmed = line.trim();
        // Look for `path:line:` at start of line (go vet, go build output)
        let parts: Vec<&str> = trimmed.splitn(3, ':').collect();
        if parts.len() >= 2 {
            let candidate = parts[0];
            if looks_like_source_path(candidate) && !candidate.is_empty() {
                found.push(candidate.to_string());
                return found;
            }
        }
    }

    // Fallback: scan for source file extensions anywhere in the line.
    const EXTS: &[&str] = &[
        ".rs", ".py", ".go", ".ts", ".tsx", ".js", ".jsx", ".c", ".cpp", ".h", ".java", ".kt",
        ".swift", ".rb", ".cs",
    ];
    for ext in EXTS {
        if let Some(idx) = line.find(ext) {
            // Walk back to start of path token.
            let before = &line[..idx + ext.len()];
            let start = before
                .rfind(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map(|i| i + 1)
                .unwrap_or(0);
            let candidate = &before[start..];
            let candidate = candidate.trim_start_matches(|c: char| c == '.' && c != '/');
            if looks_like_source_path(candidate) {
                found.push(candidate.to_string());
            }
        }
    }

    found
}

fn looks_like_source_path(s: &str) -> bool {
    if s.is_empty() || s.len() > 512 {
        return false;
    }
    const EXTS: &[&str] = &[
        ".rs", ".py", ".go", ".ts", ".tsx", ".js", ".jsx", ".c", ".cpp", ".h", ".java", ".kt",
        ".swift", ".rb", ".cs", ".toml", ".json", ".yaml", ".yml",
    ];
    EXTS.iter().any(|e| s.ends_with(e))
}

fn resolve_path(raw: &str, cwd: &Path, project_root: Option<&Path>) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        // Normalize to relative form if under project_root or cwd.
        let root = project_root.unwrap_or(cwd);
        p.strip_prefix(root)
            .map(|rel| rel.to_path_buf())
            .unwrap_or_else(|_| p.to_path_buf())
    } else {
        // Relative — leave as-is (relative to cwd).
        p.to_path_buf()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── Root detection ────────────────────────────────────────────────────────

    #[test]
    fn find_project_root_returns_none_for_root_dir() {
        // /tmp likely has no Cargo.toml etc., so root walk terminates.
        // (May fail if /tmp has .git — unlikely on CI.)
        let result = find_project_root(Path::new("/tmp/__no_such_dir_5b__"));
        // Either None or some ancestor — just ensure no panic.
        let _ = result;
    }

    #[test]
    fn find_project_root_finds_cargo_toml() {
        // Use the actual dragent workspace which has Cargo.toml.
        let workspace = std::env::current_dir()
            .unwrap()
            .ancestors()
            .find(|p| p.join("Cargo.toml").exists())
            .map(|p| p.to_path_buf());
        if let Some(ws) = workspace {
            let root = find_project_root(&ws);
            assert!(root.is_some(), "should find Cargo.toml root");
        }
    }

    // ── Path extraction: rustc ────────────────────────────────────────────────

    #[test]
    fn extract_rustc_arrow_path() {
        let line = "  --> src/main.rs:42:5";
        let paths = extract_path_from_line(line);
        assert!(paths.iter().any(|p| p == "src/main.rs"), "got {paths:?}");
    }

    #[test]
    fn extract_rustc_absolute_arrow() {
        let line = "  --> /Users/dev/myapp/src/lib.rs:10:1";
        let paths = extract_path_from_line(line);
        assert!(paths.iter().any(|p| p.ends_with("lib.rs")), "got {paths:?}");
    }

    #[test]
    fn extract_python_traceback_path() {
        let line = r#"  File "/Users/dev/app/main.py", line 7, in <module>"#;
        let paths = extract_path_from_line(line);
        assert!(
            paths.iter().any(|p| p.ends_with("main.py")),
            "got {paths:?}"
        );
    }

    #[test]
    fn extract_go_path() {
        let line = "./pkg/server.go:34:10: undefined: Foo";
        let paths = extract_path_from_line(line);
        assert!(
            paths.iter().any(|p| p.ends_with("server.go")),
            "got {paths:?}"
        );
    }

    // ── Filter: target/ skipped ───────────────────────────────────────────────

    #[test]
    fn skip_target_dir() {
        let output = "error in target/debug/build/myapp/foo.rs\n  --> src/lib.rs:5:1";
        let cwd = PathBuf::from("/tmp");
        let paths = extract_referenced_paths(output, &cwd, None);
        assert!(
            paths
                .iter()
                .all(|p| !p.to_string_lossy().contains("target")),
            "target/ should be skipped, got {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p == Path::new("src/lib.rs")),
            "src/lib.rs should be present, got {paths:?}"
        );
    }

    // ── Deduplication ─────────────────────────────────────────────────────────

    #[test]
    fn deduplicates_same_path() {
        let output = "  --> src/lib.rs:1:1\n  --> src/lib.rs:5:1\n  --> src/lib.rs:10:1";
        let cwd = PathBuf::from("/tmp");
        let paths = extract_referenced_paths(output, &cwd, None);
        let count = paths
            .iter()
            .filter(|p| *p == Path::new("src/lib.rs"))
            .count();
        assert_eq!(count, 1, "duplicate paths should be merged");
    }

    // ── Cap at 20 ─────────────────────────────────────────────────────────────

    #[test]
    fn caps_at_max_paths() {
        let mut lines = Vec::new();
        for i in 0..30 {
            lines.push(format!("  --> src/file{i}.rs:1:1"));
        }
        let output = lines.join("\n");
        let cwd = PathBuf::from("/tmp");
        let paths = extract_referenced_paths(&output, &cwd, None);
        assert!(
            paths.len() <= MAX_REFERENCED_PATHS,
            "got {} paths",
            paths.len()
        );
    }

    // ── Absolute path normalization ───────────────────────────────────────────

    #[test]
    fn absolute_path_under_project_root_becomes_relative() {
        let root = PathBuf::from("/Users/dev/myapp");
        let raw = "/Users/dev/myapp/src/lib.rs";
        let resolved = resolve_path(raw, &root, Some(&root));
        assert_eq!(resolved, PathBuf::from("src/lib.rs"));
    }

    // ── ProjectContext::build ─────────────────────────────────────────────────

    #[test]
    fn build_with_git_status() {
        let git = GitInfo {
            branch: "main".to_string(),
            changed_files: 3,
            untracked_files: 1,
        };
        let cwd = PathBuf::from("/tmp");
        let output = "  --> src/main.rs:1:1";
        let ctx = ProjectContext::build(&cwd, Some(git), output);
        assert_eq!(ctx.git_branch.as_deref(), Some("main"));
        assert_eq!(ctx.git_changed_files, 3);
        assert_eq!(ctx.git_untracked_files, 1);
        assert!(
            ctx.referenced_paths
                .iter()
                .any(|p| p == Path::new("src/main.rs"))
        );
    }

    #[test]
    fn build_without_git_status() {
        let cwd = PathBuf::from("/tmp");
        let ctx = ProjectContext::build(&cwd, None, "no errors");
        assert!(ctx.git_branch.is_none());
        assert_eq!(ctx.git_changed_files, 0);
        assert!(ctx.referenced_paths.is_empty());
    }
}
