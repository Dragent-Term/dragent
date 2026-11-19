use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::permissions::AllowedRoots;
use crate::agent::tools::grep::GrepExecutor;

const MAX_HITS: usize = 10;
const MAX_PREVIEW_CHARS: usize = 500;

pub use dragent_agent::files::{FragmentHit, SearchCodebaseResult};

pub struct SearchCodebaseExecutor {
    allowed_roots: AllowedRoots,
    grep_executor: GrepExecutor,
}

impl SearchCodebaseExecutor {
    pub fn new(allowed_roots: AllowedRoots) -> Self {
        Self {
            allowed_roots,
            grep_executor: GrepExecutor::new(),
        }
    }

    pub fn execute(&self, query: &str, repo_root: &Path) -> SearchCodebaseResult {
        if self.allowed_roots.check(repo_root).is_err() {
            log::warn!(
                "SearchCodebase blocked: {} not in allowed roots",
                repo_root.display()
            );
            return SearchCodebaseResult { hits: Vec::new() };
        }

        let mut hits: Vec<FragmentHit> = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        // ── Hybrid search: vector + BM25 + symbol lookups, RRF-fused ──────────
        crate::agent::context_builder::request_code_index(repo_root);
        let manager = dragent_index::get_manager();
        if manager.is_ready() {
            for result in manager.hybrid_search(repo_root, query, MAX_HITS) {
                let rel = result
                    .path
                    .strip_prefix(repo_root)
                    .unwrap_or(&result.path)
                    .to_path_buf();
                seen_paths.insert(rel.clone());
                hits.push(FragmentHit {
                    path: rel,
                    byte_start: result.byte_start,
                    byte_end: result.byte_end,
                    preview: result
                        .text
                        .lines()
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("\n")
                        .chars()
                        .take(MAX_PREVIEW_CHARS)
                        .collect::<String>(),
                    matched_symbol: result.matched_symbol,
                    score: Some(result.fused_score),
                });
            }
        }

        let hybrid_count = hits.len();
        let top_score = hits.first().and_then(|h| h.score).unwrap_or(0.0);

        // ── Grep (always runs, fills remaining slots with exact matches) ───────
        let remaining = MAX_HITS.saturating_sub(hits.len());
        if remaining > 0 {
            let grep_res = self.grep_executor.execute(query, Some(repo_root));
            for m in grep_res.matches.into_iter().take(remaining) {
                let rel = m
                    .path
                    .strip_prefix(repo_root)
                    .unwrap_or(&m.path)
                    .to_path_buf();
                if seen_paths.contains(&rel) {
                    continue;
                }
                seen_paths.insert(rel.clone());
                hits.push(FragmentHit {
                    path: rel,
                    byte_start: 0,
                    byte_end: m.line_text.len(),
                    preview: m
                        .line_text
                        .chars()
                        .take(MAX_PREVIEW_CHARS)
                        .collect::<String>(),
                    matched_symbol: None,
                    score: None,
                });
            }
        }

        let grep_count = hits.len() - hybrid_count;
        log::info!(
            "[search_codebase] query={:?} — {} hybrid hit(s) (index {}, top score {:.3}), {} grep hit(s)",
            query,
            hybrid_count,
            if manager.is_ready() { "ready" } else { "NOT ready — semantic skipped" },
            top_score,
            grep_count,
        );

        SearchCodebaseResult { hits }
    }
}
