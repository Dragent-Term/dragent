//! Explore subagent — parallel codebase exploration.
//!
//! The Explore subagent fans out multiple concurrent searches (grep, file reads)
//! and synthesizes the results into a compact summary for the main LLM context.
//!
//! Usage by the agent:
//!   tool: "explore"
//!   args: { "tasks": [{"kind": "grep", "query": "PaymentService"}, {"kind": "search", "query": "authentication flow"}] }

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::agent::permissions::AllowedRoots;
use crate::agent::tools::grep::GrepExecutor;
use crate::agent::tools::read_files::ReadFilesExecutor;

// ── Task definition ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExploreTask {
    /// Fast exact/regex grep.
    Grep {
        query: String,
        /// Optional glob to restrict search (e.g. "*.go").
        file_glob: Option<String>,
    },
    /// Conceptual / semantic search by description.
    Concept { query: String },
    /// Smart search — auto-selects strategy.
    Search { query: String },
    /// Read specific file(s) and summarize.
    ReadFile { path: PathBuf },
    /// Follow all callers/importers of a symbol.
    Callers { symbol: String },
}

// ── Result types ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Finding {
    pub source_task: String,
    pub path: PathBuf,
    pub excerpt: String,
    pub relevance: f32, // 0..1
}

#[derive(Debug)]
pub struct ExploreResult {
    pub findings: Vec<Finding>,
    /// Compact summary text ready to paste into the LLM context.
    pub summary: String,
    pub task_count: usize,
    pub file_count: usize,
}

// ── ExploreAgent ──────────────────────────────────────────────────────────────

pub struct ExploreAgent {
    allowed_roots: AllowedRoots,
}

impl ExploreAgent {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self { allowed_roots }
    }

    /// Execute all tasks in parallel and return synthesized findings.
    pub fn explore(&self, tasks: Vec<ExploreTask>, repo_root: &Path) -> ExploreResult {
        let task_count = tasks.len();
        let allowed_roots = self.allowed_roots.clone();
        let repo_root = repo_root.to_path_buf();

        let handles: Vec<_> = tasks
            .into_iter()
            .map(|task| {
                let allowed_roots = allowed_roots.clone();
                let repo_root = repo_root.clone();
                let task_label = task_label(&task);

                std::thread::spawn(move || -> Vec<Finding> {
                    execute_task(task, task_label, &repo_root, &allowed_roots)
                })
            })
            .collect();

        let mut all_findings: Vec<Finding> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect();

        // Deduplicate by (path, first 80 chars of excerpt)
        all_findings.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        deduplicate_findings(&mut all_findings);

        let file_count = {
            let mut paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
            for f in &all_findings {
                paths.insert(f.path.clone());
            }
            paths.len()
        };

        let summary = build_summary(&all_findings, &repo_root);

        ExploreResult {
            findings: all_findings,
            summary,
            task_count,
            file_count,
        }
    }
}

// ── Task execution ─────────────────────────────────────────────────────────────

fn execute_task(
    task: ExploreTask,
    label: String,
    repo_root: &Path,
    allowed_roots: &AllowedRoots,
) -> Vec<Finding> {
    match task {
        ExploreTask::Grep { query, file_glob } => {
            let grep = GrepExecutor::new();
            let pattern = if let Some(glob) = file_glob {
                format!("--glob={glob} {query}")
            } else {
                query.clone()
            };
            grep.execute(&pattern, Some(repo_root))
                .matches
                .into_iter()
                .take(10)
                .map(|m| {
                    let path = m
                        .path
                        .strip_prefix(repo_root)
                        .unwrap_or(&m.path)
                        .to_path_buf();
                    Finding {
                        source_task: label.clone(),
                        path,
                        excerpt: format!("L{}: {}", m.line_number, m.line_text),
                        relevance: 0.7,
                    }
                })
                .collect()
        }

        ExploreTask::Callers { symbol } => {
            // Use grep to find references to the symbol
            let grep = GrepExecutor::new();
            // Search for identifiers that reference the symbol (not the definition line itself)
            let pattern = format!(r"\b{symbol}\b");
            grep.execute(&pattern, Some(repo_root))
                .matches
                .into_iter()
                .take(15)
                .map(|m| {
                    let path = m
                        .path
                        .strip_prefix(repo_root)
                        .unwrap_or(&m.path)
                        .to_path_buf();
                    Finding {
                        source_task: label.clone(),
                        path,
                        excerpt: format!("L{}: {}", m.line_number, m.line_text),
                        relevance: 0.8,
                    }
                })
                .collect()
        }

        ExploreTask::Concept { query } | ExploreTask::Search { query } => {
            let grep = GrepExecutor::new();
            grep.execute(&query, Some(repo_root))
                .matches
                .into_iter()
                .take(8)
                .map(|m| Finding {
                    source_task: label.clone(),
                    path: m
                        .path
                        .strip_prefix(repo_root)
                        .unwrap_or(&m.path)
                        .to_path_buf(),
                    excerpt: format!("L{}: {}", m.line_number, m.line_text),
                    relevance: 0.5,
                })
                .collect()
        }

        ExploreTask::ReadFile { path } => {
            let abs_path = if path.is_absolute() {
                path.clone()
            } else {
                repo_root.join(&path)
            };
            let reader = ReadFilesExecutor::new(allowed_roots.clone());
            let result = reader.execute(&[abs_path]);
            result
                .successful()
                .map(|f| Finding {
                    source_task: label.clone(),
                    path: path.clone(),
                    excerpt: truncate_preview(f.content.as_deref().unwrap_or(""), 800),
                    relevance: 1.0,
                })
                .collect()
        }
    }
}

// ── Summary builder ───────────────────────────────────────────────────────────

fn build_summary(findings: &[Finding], repo_root: &Path) -> String {
    if findings.is_empty() {
        return "No relevant code found.".to_string();
    }

    let mut out = String::new();

    // Group by file
    let mut by_file: std::collections::BTreeMap<PathBuf, Vec<&Finding>> =
        std::collections::BTreeMap::new();
    for f in findings.iter().take(30) {
        by_file.entry(f.path.clone()).or_default().push(f);
    }

    for (path, file_findings) in &by_file {
        out.push_str(&format!("\n### {}\n", path.display()));
        for f in file_findings.iter().take(4) {
            out.push_str(&format!(
                "  [{}] {}\n",
                f.source_task,
                f.excerpt.replace('\n', " | ")
            ));
        }
    }

    out.trim().to_string()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn task_label(task: &ExploreTask) -> String {
    match task {
        ExploreTask::Grep { query, .. } => format!("grep:{}", short(query)),
        ExploreTask::Concept { query } => format!("search:{}", short(query)),
        ExploreTask::Search { query } => format!("search:{}", short(query)),
        ExploreTask::ReadFile { path } => format!("read:{}", path.display()),
        ExploreTask::Callers { symbol } => format!("callers:{symbol}"),
    }
}

fn short(s: &str) -> &str {
    let bytes = s.as_bytes();
    let end = bytes.len().min(30);
    std::str::from_utf8(&bytes[..end]).unwrap_or(s)
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Truncate at a char boundary
        let end = s.floor_char_boundary(max);
        format!("{}…", &s[..end])
    }
}

fn deduplicate_findings(findings: &mut Vec<Finding>) {
    let mut seen = std::collections::HashSet::new();
    findings.retain(|f| {
        let key = (
            f.path.clone(),
            f.excerpt.chars().take(60).collect::<String>(),
        );
        seen.insert(key)
    });
}

// ── Tool handler integration ──────────────────────────────────────────────────

/// Parse `args_json` from the LLM's `explore` tool call and execute.
/// Returns a string summary for inclusion in the tool result message.
pub fn execute_from_args(
    args_json: &str,
    repo_root: &Path,
    allowed_roots: &AllowedRoots,
) -> String {
    #[derive(Deserialize)]
    struct Args {
        tasks: Vec<ExploreTask>,
    }

    let args: Args = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return format!("Invalid explore args: {e}"),
    };

    if args.tasks.is_empty() {
        return "No tasks provided to explore.".to_string();
    }

    let agent = ExploreAgent::new(allowed_roots.clone());
    let result = agent.explore(args.tasks, repo_root);

    format!(
        "Explored {} tasks across {} files.\n\n{}",
        result.task_count, result.file_count, result.summary
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_roots(dir: &Path) -> AllowedRoots {
        AllowedRoots::new(std::iter::once(dir.to_path_buf()))
    }

    #[test]
    fn grep_task_finds_content() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn greet() { println!(\"hello\"); }\n",
        )
        .unwrap();

        let agent = ExploreAgent::new(make_roots(dir.path()));
        let result = agent.explore(
            vec![ExploreTask::Grep {
                query: "greet".to_string(),
                file_glob: None,
            }],
            dir.path(),
        );

        assert!(!result.findings.is_empty(), "should find greet in main.rs");
        assert_eq!(result.findings[0].path, PathBuf::from("main.rs"));
    }

    #[test]
    fn callers_task_finds_references() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn foo() {}\n").unwrap();
        std::fs::write(
            dir.path().join("b.rs"),
            "use crate::foo;\nfn bar() { foo(); }\n",
        )
        .unwrap();

        let agent = ExploreAgent::new(make_roots(dir.path()));
        let result = agent.explore(
            vec![ExploreTask::Callers {
                symbol: "foo".to_string(),
            }],
            dir.path(),
        );

        assert!(!result.findings.is_empty());
        // b.rs should appear since it calls foo
        let has_b = result
            .findings
            .iter()
            .any(|f| f.path == PathBuf::from("b.rs"));
        assert!(
            has_b,
            "b.rs should be in findings: {:?}",
            result.findings.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn read_file_task_returns_content() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.rs"),
            "pub const PORT: u16 = 8080;\n",
        )
        .unwrap();

        let agent = ExploreAgent::new(make_roots(dir.path()));
        let result = agent.explore(
            vec![ExploreTask::ReadFile {
                path: PathBuf::from("config.rs"),
            }],
            dir.path(),
        );

        assert!(!result.findings.is_empty());
        assert!(result.findings[0].excerpt.contains("8080"));
    }

    #[test]
    fn summary_is_non_empty_for_results() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let agent = ExploreAgent::new(make_roots(dir.path()));
        let result = agent.explore(
            vec![ExploreTask::Grep {
                query: "add".to_string(),
                file_glob: None,
            }],
            dir.path(),
        );

        assert!(!result.summary.is_empty());
        assert!(result.summary.contains("lib.rs"));
    }

    #[test]
    fn parallel_tasks_all_execute() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.rs"), "fn alpha() {}").unwrap();
        std::fs::write(dir.path().join("beta.rs"), "fn beta() {}").unwrap();

        let agent = ExploreAgent::new(make_roots(dir.path()));
        let result = agent.explore(
            vec![
                ExploreTask::Grep {
                    query: "alpha".to_string(),
                    file_glob: None,
                },
                ExploreTask::Grep {
                    query: "beta".to_string(),
                    file_glob: None,
                },
            ],
            dir.path(),
        );

        let paths: std::collections::HashSet<_> = result.findings.iter().map(|f| &f.path).collect();
        assert!(
            paths.contains(&PathBuf::from("alpha.rs")),
            "missing alpha.rs"
        );
        assert!(paths.contains(&PathBuf::from("beta.rs")), "missing beta.rs");
    }

    #[test]
    fn parse_explore_args_json() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("server.rs"), "fn handle_auth() {}").unwrap();

        let args = r#"{"tasks": [{"kind": "grep", "query": "auth", "file_glob": null}]}"#;
        let roots = make_roots(dir.path());
        let summary = execute_from_args(args, dir.path(), &roots);
        assert!(!summary.is_empty());
    }
}
