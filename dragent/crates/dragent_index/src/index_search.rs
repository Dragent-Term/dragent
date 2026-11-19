use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::inverted::InvertedIndex;
use super::merkle::MerkleTree;
use super::store::SearchResult;
use crate::CodeIndexManager;
use crate::manager::RepoState;

// ── Thread priority ───────────────────────────────────────────────────────────

/// Drop the calling thread to background (Utility) scheduling so indexing —
/// walking, chunking, and especially ONNX embedding — never competes with the
/// UI thread for cores. On Apple platforms the ONNX Runtime's intra-op pool
/// inherits the QoS of the thread that creates the session, so calling this
/// before `Embedder::load` covers inference threads too. The terminal must
/// never be the reason the app stutters.

// ── Global static ─────────────────────────────────────────────────────────────

/// Default per-repo file budget, used when a parked/unknown root has to be
/// re-registered without its original budget.

/// RRF smoothing constant: `score += 1 / (k + rank)` per list (rank 1-based).
pub const RRF_K: f32 = 60.0;

/// One fused hybrid hit. `path` is absolute; `vector_score`/`bm25_score` are
/// the per-list raw scores when the fragment appeared in that list.
#[derive(Debug, Clone)]
pub struct HybridHit {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub vector_score: Option<f32>,
    pub bm25_score: Option<f32>,
    pub fused_score: f32,
    pub matched_symbol: Option<String>,
}

/// Fuse ranked lists with reciprocal rank fusion. Each list holds keys best
/// first; every appearance adds `1 / (k + rank)` (rank 1-based). Items present
/// in several lists therefore outrank items in only one, and within a single
/// list the earlier rank wins.
pub fn reciprocal_rank_fusion<K>(lists: &[Vec<K>], k: f32) -> HashMap<K, f32>
where
    K: Eq + std::hash::Hash + Clone,
{
    let mut fused: HashMap<K, f32> = HashMap::new();
    for list in lists {
        for (idx, key) in list.iter().enumerate() {
            let rank = idx as f32 + 1.0;
            *fused.entry(key.clone()).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    fused
}

/// Identifier-ish tokens in a query that are worth exact symbol lookups.
/// Splits on non-identifier characters, drops pure numbers and 1-char noise,
/// and keeps tokens that look like code names (len ≥ 3, or 2 chars with an
/// underscore / uppercase letter). Deduped case-insensitively, order kept.
pub fn symbol_tokens(query: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for raw in query.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if raw.len() < 2 || raw.chars().all(|c| c.is_numeric()) {
            continue;
        }
        let identifierish =
            raw.len() >= 3 || raw.contains('_') || raw.chars().any(|c| c.is_uppercase());
        if !identifierish {
            continue;
        }
        if seen.insert(raw.to_lowercase()) {
            out.push(raw.to_string());
        }
    }
    out
}

/// Fragment identity used as the RRF fusion key: relative path + byte range.
type FragKey = (PathBuf, usize, usize);

impl CodeIndexManager {
    /// Semantic search over a registered repo. Returns empty slice if not yet indexed.
    pub fn search(&self, repo_root: &Path, query: &str, top_k: usize) -> Vec<SearchResult> {
        let repo_state = self.resolve_repo_state(repo_root).map(|(_, state)| state);
        let Some(repo_state) = repo_state else {
            return vec![];
        };
        let embedder = self.acquire_embedder(Duration::from_secs(60));
        let Some(embedder) = embedder else {
            return vec![];
        };

        let embedding = match embedder.embed_query(query) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[index] query embed failed: {e}");
                return vec![];
            }
        };

        repo_state
            .lock()
            .unwrap()
            .store
            .search(&embedding, top_k)
            .unwrap_or_default()
    }

    /// Hybrid search: vector similarity + BM25 + exact symbol lookups, fused
    /// with reciprocal rank fusion (k = [`RRF_K`]).
    ///
    /// The BM25 index is built lazily from the repo's Merkle tree, cached in
    /// memory keyed by `tree.root_hash`, and rebuilt when the tree changes.
    /// Degrades gracefully: BM25-only when the embedder is unavailable, empty
    /// when the repo is not indexed.
    pub fn hybrid_search(&self, repo_root: &Path, query: &str, top_k: usize) -> Vec<HybridHit> {
        if query.trim().is_empty() || top_k == 0 {
            return vec![];
        }
        let Some((_resolved_root, repo_state)) = self.resolve_repo_state(repo_root) else {
            return vec![];
        };
        // Snapshot what BM25 needs without holding the mutex across disk IO.
        let (root, tree) = {
            let state = repo_state.lock().unwrap();
            (state.root.clone(), state.tree.clone())
        };
        let bm25 = self.bm25_for_repo(&repo_state, &root, &tree);

        // Over-fetch per list so the fusion has something to fuse; trim to
        // top_k at the end.
        let fetch = top_k.saturating_mul(3).max(20);

        // ── BM25 list ──
        let bm25_hits = bm25.search(query, fetch);

        // ── Symbol list (exact lookups for identifier-ish tokens) ──
        let mut symbol_hits = Vec::new();
        for token in symbol_tokens(query) {
            symbol_hits.extend(bm25.symbol_lookup(&token, &tree, 5));
            if symbol_hits.len() >= fetch {
                break;
            }
        }
        symbol_hits.truncate(fetch);

        // ── Vector list (skipped when the embedder is unavailable) ──
        let vector_hits: Vec<SearchResult> = match self.acquire_embedder(Duration::from_secs(60)) {
            Some(embedder) => match embedder.embed_query(query) {
                Ok(embedding) => repo_state
                    .lock()
                    .unwrap()
                    .store
                    .search(&embedding, fetch)
                    .unwrap_or_default(),
                Err(e) => {
                    log::warn!("[index] query embed failed: {e}");
                    vec![]
                }
            },
            None => {
                log::debug!("[index] embedder unavailable — BM25-only hybrid search");
                vec![]
            }
        };

        // ── Fuse ──
        let rel_of = |abs: &Path| {
            abs.strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| abs.to_path_buf())
        };
        let vector_keys: Vec<FragKey> = vector_hits
            .iter()
            .map(|h| (rel_of(&h.path), h.byte_start, h.byte_end))
            .collect();
        let bm25_keys: Vec<FragKey> = bm25_hits
            .iter()
            .map(|h| (h.path.clone(), h.byte_start, h.byte_end))
            .collect();
        let symbol_keys: Vec<FragKey> = symbol_hits
            .iter()
            .map(|h| (h.path.clone(), h.byte_start, h.byte_end))
            .collect();
        let fused = reciprocal_rank_fusion(&[vector_keys, bm25_keys, symbol_keys], RRF_K);

        let vector_scores: HashMap<FragKey, f32> = vector_hits
            .iter()
            .map(|h| ((rel_of(&h.path), h.byte_start, h.byte_end), h.score))
            .collect();
        let bm25_scores: HashMap<FragKey, f32> = bm25_hits
            .iter()
            .map(|h| ((h.path.clone(), h.byte_start, h.byte_end), h.score))
            .collect();
        // Prefer the exact-match symbol name when several tokens hit one fragment.
        let mut symbol_names: HashMap<FragKey, (f32, String)> = HashMap::new();
        for h in &symbol_hits {
            if let Some(name) = &h.matched_symbol {
                let key = (h.path.clone(), h.byte_start, h.byte_end);
                let entry = symbol_names.entry(key).or_insert((f32::MIN, String::new()));
                if h.score > entry.0 {
                    *entry = (h.score, name.clone());
                }
            }
        }
        let vector_texts: HashMap<FragKey, &str> = vector_hits
            .iter()
            .map(|h| ((rel_of(&h.path), h.byte_start, h.byte_end), h.text.as_str()))
            .collect();

        let mut hits: Vec<HybridHit> = fused
            .into_iter()
            .filter_map(|(key, fused_score)| {
                let (rel, byte_start, byte_end) = key.clone();
                let text = match vector_texts.get(&key) {
                    Some(t) => (*t).to_string(),
                    None => read_fragment(&root, &rel, byte_start, byte_end)?,
                };
                if text.is_empty() {
                    return None;
                }
                Some(HybridHit {
                    path: root.join(&rel),
                    byte_start,
                    byte_end,
                    text,
                    vector_score: vector_scores.get(&key).copied(),
                    bm25_score: bm25_scores.get(&key).copied(),
                    fused_score,
                    matched_symbol: symbol_names.get(&key).map(|(_, n)| n.clone()),
                })
            })
            .collect();

        hits.sort_by(|a, b| {
            b.fused_score
                .partial_cmp(&a.fused_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        hits
    }

    /// Lazily-built per-repo BM25, keyed by `tree.root_hash` and cached in
    /// memory on the repo state. Rebuilt when the tree changed; the lock is
    /// held only for the hash check and cache update, never across the build.
    fn bm25_for_repo(
        &self,
        repo_state: &Arc<Mutex<RepoState>>,
        root: &Path,
        tree: &MerkleTree,
    ) -> Arc<InvertedIndex> {
        {
            let state = repo_state.lock().unwrap();
            if let Some((hash, index)) = &state.bm25_cache
                && *hash == tree.root_hash
            {
                return Arc::clone(index);
            }
        }
        let fresh = Arc::new(InvertedIndex::build(root, tree));
        {
            let mut state = repo_state.lock().unwrap();
            // Another thread may have rebuilt meanwhile — keep the newest, but
            // only when it still describes the current tree.
            if state.tree.root_hash == tree.root_hash {
                state.bm25_cache = Some((tree.root_hash.clone(), Arc::clone(&fresh)));
            }
        }
        fresh
    }
}

/// Read one fragment's text from disk; `None` when unreadable so the caller
/// can drop the hit instead of returning an empty preview.
fn read_fragment(root: &Path, rel: &Path, byte_start: usize, byte_end: usize) -> Option<String> {
    let content = std::fs::read_to_string(root.join(rel)).ok()?;
    let start = byte_start.min(content.len());
    let end = byte_end.min(content.len()).max(start);
    Some(content.get(start..end).unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_item_in_both_lists_outranks_item_in_one() {
        let fused = reciprocal_rank_fusion(
            &[
                vec!["a".to_string(), "b".to_string()],
                vec!["a".to_string(), "c".to_string()],
            ],
            RRF_K,
        );
        assert!(fused["a"] > fused["b"]);
        assert!(fused["a"] > fused["c"]);
    }

    #[test]
    fn rrf_preserves_rank_order_within_a_list() {
        let fused = reciprocal_rank_fusion(
            &[vec!["a".to_string(), "b".to_string(), "c".to_string()]],
            RRF_K,
        );
        assert!(fused["a"] > fused["b"]);
        assert!(fused["b"] > fused["c"]);
        // Rank 1 with k=60 scores exactly 1/61.
        assert!((fused["a"] - 1.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn symbol_tokens_keeps_identifiers_drops_noise() {
        let tokens = symbol_tokens("HttpServer http_server handle request x 42");
        assert!(tokens.contains(&"HttpServer".to_string()));
        assert!(tokens.contains(&"http_server".to_string()));
        assert!(tokens.contains(&"handle".to_string()));
        assert!(!tokens.contains(&"x".to_string()), "single chars are noise");
        assert!(
            !tokens.contains(&"42".to_string()),
            "pure numbers are noise"
        );
        // Deduplicated case-insensitively, first spelling wins.
        let dupes = symbol_tokens("Server server SERVER");
        assert_eq!(dupes, vec!["Server".to_string()]);
    }

    #[test]
    fn symbol_lookup_attributes_matched_symbol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("server.rs"),
            "pub struct HttpServer {\n    port: u16,\n}\n",
        )
        .unwrap();
        let tree = crate::walk::FileTreeWalker::build_tree(dir.path(), 100).unwrap();
        let index = InvertedIndex::build(dir.path(), &tree);

        // End-to-end through the same helpers hybrid_search uses: tokens from
        // the query, then exact symbol lookup with attribution.
        let mut found: Option<String> = None;
        for token in symbol_tokens("where is HttpServer defined") {
            for hit in index.symbol_lookup(&token, &tree, 5) {
                if hit.matched_symbol.is_some() {
                    found = hit.matched_symbol.clone();
                    break;
                }
            }
        }
        assert_eq!(found.as_deref(), Some("HttpServer"));
    }
}
