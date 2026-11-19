//! Gathers local project context before the first LLM API call.
//!
//! The LLM starts blind — it only knows the shell and CWD from the system
//! prompt. This module runs instant local operations (no network, no cost) to
//! give the LLM the same starting picture a developer has when they open the
//! project: directory structure, what language/framework it uses, and which
//! files are most likely relevant to the user's query.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use web_time::Instant;

pub struct ProjectContext {
    /// Compact file tree (2 levels deep, filtered).
    pub file_tree: String,
    /// Contents of key project definition files (go.mod, Cargo.toml, etc.).
    pub config_files: Vec<(String, String)>,
    /// Query-relevant code fragments pulled from the semantic index (empty when
    /// the query is blank or the index isn't ready yet).
    pub relevant_code: Vec<RelevantChunk>,
}

/// A single code fragment surfaced by the semantic index for the user's query.
pub struct RelevantChunk {
    /// Path shown to the model (relative to cwd when possible).
    pub path: String,
    /// A short preview of the fragment's text.
    pub preview: String,
}

/// Cache for the static (query-independent) parts of the project context.
/// Keyed by CWD. Invalidated when the file watcher fires or CWD changes.
struct StaticContextCache {
    fingerprint: u64,
    file_tree: String,
    config_files: Vec<(String, String)>,
}

static STATIC_CONTEXT_CACHE: once_cell::sync::Lazy<Mutex<HashMap<PathBuf, StaticContextCache>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
static STATIC_CONTEXT_WARMING: once_cell::sync::Lazy<Mutex<HashSet<PathBuf>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));

/// Pre-build and cache the static context (file tree + config files) for `cwd`.
/// Call this eagerly at session start so the first query doesn't pay for it.
pub fn warm_static_context(cwd: &Path) {
    let cwd = cwd.to_path_buf();
    if STATIC_CONTEXT_CACHE
        .lock()
        .map(|cache| cache.contains_key(&cwd))
        .unwrap_or(false)
    {
        return;
    }
    let should_start = STATIC_CONTEXT_WARMING
        .lock()
        .map(|mut warming| warming.insert(cwd.clone()))
        .unwrap_or(true);
    if !should_start {
        return;
    }
    crate::bg::detach(move || {
        let t = Instant::now();
        let file_tree = build_file_tree(&cwd);
        let config_files = read_config_files(&cwd);
        let fingerprint = workspace_fingerprint(&cwd);
        log::info!(
            "[context] static context pre-built in {}ms",
            t.elapsed().as_millis()
        );
        if let Ok(mut guard) = STATIC_CONTEXT_CACHE.lock() {
            guard.insert(
                cwd.clone(),
                StaticContextCache {
                    fingerprint,
                    file_tree,
                    config_files,
                },
            );
        }
        if let Ok(mut warming) = STATIC_CONTEXT_WARMING.lock() {
            warming.remove(&cwd);
        }
    });
}

/// Invalidate the static cache (call when files change on disk).
pub fn invalidate_static_context() {
    if let Ok(mut guard) = STATIC_CONTEXT_CACHE.lock() {
        guard.clear();
    }
}

/// Resolve the package manager declared by the repository's lockfile. Lockfile
/// choice matters: npm and Yarn can install different dependency trees and can
/// expose different workspace/script behavior.
pub fn preferred_package_manager(cwd: &Path) -> Option<&'static str> {
    if cwd.join("yarn.lock").exists() {
        Some("yarn")
    } else if cwd.join("pnpm-lock.yaml").exists() {
        Some("pnpm")
    } else if cwd.join("bun.lockb").exists() || cwd.join("bun.lock").exists() {
        Some("bun")
    } else if cwd.join("package-lock.json").exists() {
        Some("npm")
    } else {
        None
    }
}

/// True for purely conversational messages (greetings, thanks, quick pings)
/// that need NO project context — skipping the file tree + code search for
/// these saves thousands of prompt tokens per "hello". Deliberately
/// conservative: anything that even smells like a task gets full context.
pub fn is_conversational(query: &str) -> bool {
    let q = query.trim().to_lowercase();
    // `/test` or `/:ping` is a command (often a skill), never chit-chat.
    if q.starts_with('/') {
        return false;
    }
    // Long messages are never treated as chit-chat.
    if q.chars().count() > 40 || q.split_whitespace().count() > 5 {
        return false;
    }
    let stripped: String = q
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '\'')
        .collect();
    let stripped = stripped.trim();
    const EXACT: &[&str] = &[
        "hello",
        "hi",
        "hy",
        "hii",
        "hey",
        "yo",
        "sup",
        "hola",
        "ping",
        "test",
        "ok",
        "okay",
        "thanks",
        "thank you",
        "thanks a lot",
        "thank you so much",
        "ty",
        "thx",
        "cool",
        "nice",
        "great",
        "awesome",
        "good",
        "lol",
        "haha",
        "good morning",
        "good afternoon",
        "good evening",
        "good night",
        "how are you",
        "how are you doing",
        "what's up",
        "whats up",
        "are you there",
        "you there",
        "hello there",
        "hi there",
        "hey there",
        "bye",
        "goodbye",
        "see you",
        "morning",
        "evening",
    ];
    if EXACT.contains(&stripped) {
        return true;
    }
    // Greeting word + nothing task-like after it ("hey claude", "hi dragent").
    let mut words = stripped.split_whitespace();
    let first = words.next().unwrap_or("");
    if matches!(first, "hello" | "hi" | "hey" | "thanks" | "yo") && words.clone().count() <= 2 {
        const TASK_HINTS: &[&str] = &[
            "fix", "add", "build", "run", "make", "write", "create", "help", "why", "what", "how",
            "can", "please", "check", "update", "test",
        ];
        return !words.any(|w| TASK_HINTS.contains(&w));
    }
    false
}

/// Make sure the repository around `cwd` is indexed, because this agent is
/// about to search it. Embedding is the most expensive thing the app does, and
/// only this agent reads the result, so nothing else asks for it: terminals
/// and worktrees running other tools cost nothing here. The first search of a
/// repository falls back to text search while the index builds.
pub(crate) fn request_code_index(cwd: &Path) {
    /// Long enough not to run `git` on every call, and short enough that a
    /// repository released for being idle comes back on its next use.
    const ASK_AGAIN_AFTER: std::time::Duration = std::time::Duration::from_secs(300);
    static LAST_ASKED: once_cell::sync::Lazy<Mutex<HashMap<PathBuf, Instant>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));
    if let Ok(mut asked) = LAST_ASKED.lock() {
        match asked.get(cwd) {
            Some(at) if at.elapsed() < ASK_AGAIN_AFTER => return,
            _ => asked.insert(cwd.to_path_buf(), Instant::now()),
        };
    }
    let cwd = cwd.to_path_buf();
    crate::bg::detach(move || {
        // The git work tree is the outermost boundary, so a nested crate
        // shares its repository's index rather than embedding every file again.
        let root = crate::git_status::repo_root(&cwd);
        dragent_index::get_manager().register_repo(root, 10_000);
    });
}

impl ProjectContext {
    /// Build project context. Static parts (file tree, config files) are served
    /// from cache if available.
    pub fn build(cwd: &Path, query: &str) -> Self {
        let t0 = Instant::now();

        let fingerprint = workspace_fingerprint(cwd);
        let cached = STATIC_CONTEXT_CACHE.lock().ok().and_then(|g| {
            g.get(cwd)
                .filter(|c| c.fingerprint == fingerprint)
                .map(|c| (c.file_tree.clone(), c.config_files.clone()))
        });

        let (file_tree, config_files) = if let Some((ft, cf)) = cached {
            log::info!(
                "[context] static context served from cache ({} chars tree, {} config files)",
                ft.len(),
                cf.len()
            );
            (ft, cf)
        } else {
            let t1 = Instant::now();
            let file_tree = build_file_tree(cwd);
            log::info!(
                "[context] file_tree built in {}ms ({} chars)",
                t1.elapsed().as_millis(),
                file_tree.len()
            );

            let t2 = Instant::now();
            let config_files = read_config_files(cwd);
            log::info!(
                "[context] read {} config file(s) in {}ms",
                config_files.len(),
                t2.elapsed().as_millis()
            );

            if let Ok(mut guard) = STATIC_CONTEXT_CACHE.lock() {
                guard.insert(
                    cwd.to_path_buf(),
                    StaticContextCache {
                        fingerprint,
                        file_tree: file_tree.clone(),
                        config_files: config_files.clone(),
                    },
                );
            }
            (file_tree, config_files)
        };

        // Query-relevant code: use the semantic index to pre-fetch the fragments
        // most related to the user's query, so the agent starts each turn already
        // aware of the relevant code instead of having to call search_codebase.
        let relevant_code = Self::retrieve_relevant_code(cwd, query);

        if relevant_code.is_empty() {
            log::info!(
                "[context] total context build: {}ms (0 relevant chunks — index not ready or no query match)",
                t0.elapsed().as_millis()
            );
        } else {
            log::info!(
                "[context] total context build: {}ms — injected {} relevant chunk(s) from index: {:?}",
                t0.elapsed().as_millis(),
                relevant_code.len(),
                relevant_code.iter().map(|c| c.path.as_str()).collect::<Vec<_>>()
            );
        }
        Self {
            file_tree,
            config_files,
            relevant_code,
        }
    }

    /// Top-K semantically relevant fragments for `query`. Empty when the query is
    /// blank or the index isn't ready (embedder still loading / repo not yet
    /// indexed) — retrieval is a best-effort enhancement, never a hard dependency.
    ///
    /// The user's context-scope setting (Settings → Agent → General) restricts
    /// which files may be surfaced: the whole indexed workspace, only files
    /// under the pane's cwd, or only git-tracked files.
    fn retrieve_relevant_code(cwd: &Path, query: &str) -> Vec<RelevantChunk> {
        const TOP_K: usize = 4;
        const PREVIEW_CHARS: usize = 480;

        if query.trim().is_empty() {
            return Vec::new();
        }
        request_code_index(cwd);
        let manager = dragent_index::get_manager();
        if !manager.is_ready() {
            return Vec::new();
        }

        let scope = crate::settings::AppSettings::global().agent.context_scope;
        // Over-fetch when a filter applies so filtering still yields TOP_K.
        let fetch = match scope {
            crate::settings::ContextScope::Workspace => TOP_K,
            _ => TOP_K * 4,
        };

        manager
            .search(cwd, query, fetch)
            .into_iter()
            .filter(|r| match scope {
                crate::settings::ContextScope::Workspace => true,
                crate::settings::ContextScope::CurrentDir => r.path.starts_with(cwd),
                crate::settings::ContextScope::GitTracked => is_git_tracked(cwd, &r.path),
            })
            .take(TOP_K)
            .map(|r| {
                let path = r
                    .path
                    .strip_prefix(cwd)
                    .unwrap_or(&r.path)
                    .to_string_lossy()
                    .into_owned();
                let preview: String = r
                    .text
                    .lines()
                    .take(12)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .chars()
                    .take(PREVIEW_CHARS)
                    .collect();
                RelevantChunk { path, preview }
            })
            .collect()
    }

    /// Format as a block to inject into the system prompt.
    pub fn to_system_prompt_block(&self) -> String {
        let mut out = String::new();

        out.push_str("\n\n## Project context (gathered automatically before this conversation)\n");

        if !self.file_tree.is_empty() {
            out.push_str("\n### Directory structure\n```\n");
            out.push_str(&cap_chars(&self.file_tree, 12_000));
            out.push_str("```\n");
        }

        for (name, content) in &self.config_files {
            out.push_str(&format!(
                "\n### {name}\n```\n{}\n```\n",
                cap_chars(content, 4_096)
            ));
        }

        if !self.relevant_code.is_empty() {
            out.push_str(
                "\n### Code likely relevant to the request (from semantic search — verify before relying on it)\n",
            );
            for chunk in &self.relevant_code {
                out.push_str(&format!(
                    "\n#### {}\n```\n{}\n```\n",
                    chunk.path,
                    cap_chars(&chunk.preview, 1_600)
                ));
            }
        }

        cap_chars(&out, 28_000)
    }
}

fn cap_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    const MARKER: &str = "\n…(context truncated; use tools to inspect more)…";
    if max <= MARKER.chars().count() {
        return MARKER.chars().take(max).collect();
    }
    let mut out: String = value.chars().take(max - MARKER.chars().count()).collect();
    out.push_str(MARKER);
    out
}

// ── File tree ─────────────────────────────────────────────────────────────────

const MAX_TREE_LINES: usize = 400;

fn build_file_tree(root: &Path) -> String {
    let mut lines = Vec::new();
    let mut count = 0usize;
    collect_tree(root, root, 0, &mut lines, &mut count);
    if count >= MAX_TREE_LINES {
        lines.push(format!(
            "... (tree truncated at {MAX_TREE_LINES} entries — use list_directory to explore further)"
        ));
    }
    lines.join("\n")
}

fn collect_tree(root: &Path, path: &Path, depth: usize, out: &mut Vec<String>, count: &mut usize) {
    if depth > 2 || *count >= MAX_TREE_LINES {
        return;
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    // Never materialize every entry in a broad directory (for example the
    // user's home or Library directory) merely to produce a 400-line prompt.
    let remaining = MAX_TREE_LINES.saturating_sub(*count);
    let mut entries: Vec<_> = entries.flatten().take(remaining).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if *count >= MAX_TREE_LINES {
            return;
        }
        let p = entry.path();
        let name = p.file_name().unwrap_or_default().to_string_lossy();

        if name.starts_with('.') {
            continue;
        }
        if matches!(
            name.as_ref(),
            "node_modules" | "target" | "vendor" | "dist" | ".git"
        ) {
            continue;
        }

        let relative = p.strip_prefix(root).unwrap_or(&p).to_string_lossy();
        let indent = "  ".repeat(depth);
        if p.is_dir() {
            out.push(format!("{indent}{name}/"));
            *count += 1;
            collect_tree(root, &p, depth + 1, out, count);
        } else {
            out.push(format!("{indent}{relative}"));
            *count += 1;
        }
    }
}

/// Cheap freshness token for the static context. It samples the same bounded
/// tree used for the prompt plus key config files, so edits invalidate cached
/// context without requiring a filesystem watcher to be perfectly reliable.
fn workspace_fingerprint(root: &Path) -> u64 {
    let mut paths = Vec::new();
    collect_fingerprint_paths(root, 0, &mut paths);
    for name in CONFIG_FILES {
        paths.push(root.join(name));
    }
    paths.sort();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);
        if let Ok(metadata) = std::fs::metadata(&path) {
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified() {
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
                    .hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn collect_fingerprint_paths(path: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 2 || out.len() >= MAX_TREE_LINES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_TREE_LINES {
            return;
        }
        let p = entry.path();
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules" | "target" | "vendor" | "dist" | ".git"
            )
        {
            continue;
        }
        out.push(p.clone());
        if p.is_dir() {
            collect_fingerprint_paths(&p, depth + 1, out);
        }
    }
}

// ── Config files ──────────────────────────────────────────────────────────────

const CONFIG_FILES: &[&str] = &[
    // The project's primary agent rules — injected first so the model sees
    // them before any manifest.
    ".dragentrules",
    ".dragent/AGENTS.md",
    ".dragent/AGENT.md",
    ".dragent/agent.md",
    "go.mod",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "requirements.txt",
    "Makefile",
    "docker-compose.yml",
    "README.md",
];

fn read_config_files(root: &Path) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for name in CONFIG_FILES {
        let path = root.join(name);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let capped = if content.len() > 2048 {
                format!("{}…", crate::agent::tools_background::utf8_prefix(&content, 2048))
            } else {
                content
            };
            results.push((name.to_string(), capped));
        }
    }
    results
}

/// True when `path` is tracked by the git repository containing `cwd`.
/// Used by the "Git-tracked only" context scope; a failed git invocation
/// (no git, not a repo) conservatively reports untracked.
fn is_git_tracked(cwd: &Path, path: &Path) -> bool {
    use crate::process_ext::NoWindow;
    crate::git_cmd::GitCommand::new()
        .no_window()
        .arg("-C")
        .arg(cwd)
        .arg("ls-files")
        .arg("--error-unmatch")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod conversational_tests {
    use super::is_conversational;

    #[test]
    fn greetings_and_thanks_are_conversational() {
        for m in [
            "hello",
            "Hello",
            "hi",
            "hy",
            "hii",
            "hey!",
            "Hey there",
            "thanks",
            "Thank you",
            "good morning",
            "how are you?",
            "ok",
            "yo",
            "ping",
        ] {
            assert!(is_conversational(m), "should be conversational: {m}");
        }
    }

    #[test]
    fn tasks_are_never_conversational() {
        // False positives here would silently strip project context from real
        // work — the conservative direction is everything below gets context.
        for m in [
            "fix the failing test",
            "/test",  // a skill command, not chit-chat
            "/:ping", // likewise
            "hello world program in rust", // >3 words after greeting
            "hey can you fix this",
            "hi, why is the build failing?",
            "add a login page",
            "refactor terminal_view.rs",
            "run the tests",
            "what does this function do?",
            "help",
            "continue",
            "deploy the server",
        ] {
            assert!(!is_conversational(m), "should be a TASK: {m}");
        }
    }

    #[test]
    fn long_messages_are_never_conversational() {
        assert!(!is_conversational(
            "hello I would like you to build me a complete authentication system"
        ));
    }
}

#[cfg(test)]
mod context_budget_tests {
    use super::{cap_chars, preferred_package_manager, ProjectContext, RelevantChunk};

    #[test]
    fn cap_chars_preserves_short_values_and_marks_long_values() {
        assert_eq!(cap_chars("short", 20), "short");
        let capped = cap_chars(&"x".repeat(100), 40);
        assert!(capped.chars().count() <= 40);
        assert!(capped.contains("context truncated"));
    }

    #[test]
    fn project_context_prompt_has_a_bounded_size() {
        let context = ProjectContext {
            file_tree: "tree".repeat(10_000),
            config_files: vec![("Cargo.toml".into(), "config".repeat(10_000))],
            relevant_code: vec![RelevantChunk {
                path: "src/lib.rs".into(),
                preview: "code".repeat(2_000),
            }],
        };
        assert!(context.to_system_prompt_block().chars().count() <= 28_000);
    }

    #[test]
    fn preferred_package_manager_follows_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        assert_eq!(preferred_package_manager(dir.path()), Some("npm"));
        std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(preferred_package_manager(dir.path()), Some("yarn"));
    }
}
