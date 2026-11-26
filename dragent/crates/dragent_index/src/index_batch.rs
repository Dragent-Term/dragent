use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use super::embedder::EmbedText;
use super::merkle::MerkleTree;
use super::snapshot::IndexSnapshot;
use super::store::VectorStore;
use super::walk::FileTreeWalker;
use crate::embed_cache::EmbeddingCache;
use crate::index_watcher::ChangeBatch;
use crate::index_watcher::DEFAULT_MODEL_IDLE_SECS;
use crate::index_watcher::EMBED_BATCH;
use crate::index_watcher::EMBED_BATCH_BREATHER;
use crate::index_watcher::FULL_RESCAN_THRESHOLD;
use crate::index_watcher::maybe_save_snapshot;
use crate::manager::RepoActivity;
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

pub(crate) fn repo_idle_timeout() -> Duration {
    let seconds = std::env::var("DRAGENT_INDEX_REPO_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(crate::index_watcher::DEFAULT_REPO_IDLE_SECS);
    Duration::from_secs(seconds)
}

pub(crate) fn model_idle_timeout() -> Duration {
    let seconds = std::env::var("DRAGENT_INDEX_MODEL_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MODEL_IDLE_SECS);
    Duration::from_secs(seconds)
}

/// Apply a coalesced batch: purge removals, re-chunk modified files, embed in
/// batches, upsert, and persist the snapshot once. The repo lock is held only
/// for tree/store mutations — never across model inference — so agent searches
/// stay responsive while the watcher digests a burst.
///
/// `cache` is the repo-shared content-hash embedding cache: chunks identical
/// to ones any worktree of this repo already embedded reuse their vector.
pub(crate) fn process_batch(
    repo_state: &Arc<Mutex<RepoState>>,
    embedder: &impl EmbedText,
    cache: &EmbeddingCache,
    batch: ChangeBatch,
    max_files: usize,
    activity: &RepoActivity,
) -> bool {
    if batch.len() > FULL_RESCAN_THRESHOLD {
        log::info!(
            "[index] {} pending changes exceeds threshold — full rescan",
            batch.len()
        );
        if let Err(e) = full_rescan(repo_state, embedder, cache, max_files) {
            log::warn!("[index] full rescan failed: {e}");
            return false;
        }
        {
            let state = repo_state.lock().unwrap();
            activity.set_counts(state.tree.fragments.len(), state.store.count());
        }
        return true;
    }

    // Removals + re-chunk modified files, collecting embed work. Old chunks of
    // modified files are purged here; their replacements land after embedding.
    let mut pairs: Vec<(super::merkle::Fragment, String)> = Vec::new();
    {
        let mut state = repo_state.lock().unwrap();
        let root = state.root.clone();
        for rel in &batch.removed {
            let _ = state.store.remove_file(&root.join(rel));
        }
        let removed: Vec<PathBuf> = batch.removed.iter().cloned().collect();
        state.tree.remove_files(&removed);

        for rel in &batch.modified {
            // Enforce the same max_files budget as the initial walk: existing
            // files keep updating, but growth stops at the cap (surfaced via
            // status as `capped` rather than silently indexing without bound).
            let is_new = !state.tree.fragments.contains_key(rel);
            if is_new && state.tree.fragments.len() >= max_files {
                if !activity.capped.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "[index] max_files ({max_files}) reached for {} — new files are not being indexed",
                        state.root.display()
                    );
                }
                continue;
            }
            // Re-chunk via the Merkle tree (respects binary / size limits).
            if state.tree.upsert_files(&root, &[rel.clone()]).is_err() {
                continue;
            }
            let abs = root.join(rel);
            let _ = state.store.remove_file(&abs);
            let Some(fragments) = state.tree.fragments.get(rel).cloned() else {
                continue; // skipped (binary / too large) — old chunks purged above
            };
            let Ok(content) = std::fs::read_to_string(&abs) else {
                continue;
            };
            for frag in fragments {
                let text = content.get(frag.byte_start..frag.byte_end).unwrap_or("");
                if !text.is_empty() {
                    pairs.push((frag, text.to_string()));
                }
            }
        }
    }

    // Reuse stored vectors for unchanged content, then embed the remainder
    // WITHOUT the lock; retake it briefly for the upserts.
    let mut slots = cached_embeddings(cache, &pairs);
    if let Err(e) = embed_missing(embedder, cache, &pairs, &mut slots) {
        // Skipped files stay stale until their next change or the
        // startup diff — never fatal to the watcher.
        log::warn!("[index] batch embed failed: {e}");
        return false;
    }
    {
        let state = repo_state.lock().unwrap();
        for ((frag, text), slot) in pairs.iter().zip(&slots) {
            let Some(emb) = slot else { continue };
            let abs = state.root.join(&frag.path);
            let _ = state.store.upsert(
                &frag.fragment_id,
                &abs,
                frag.byte_start,
                frag.byte_end,
                text,
                emb,
            );
        }
    }

    {
        let mut state = repo_state.lock().unwrap();
        if let Err(error) = ensure_store_matches_tree(&state.store, &state.root, &state.tree) {
            log::warn!("[index] batch left an incomplete store: {error}");
            return false;
        }
        maybe_save_snapshot(&mut state);
        activity.set_counts(state.tree.fragments.len(), state.store.count());
    }
    log::debug!(
        "[index] batch: {} modified ({} chunks), {} removed",
        batch.modified.len(),
        pairs.len(),
        batch.removed.len()
    );
    true
}

/// Rebuild the tree with the same walk (and exclusions) as startup, diff
/// against the current tree, and reconcile the store. Bounded by `max_files`
/// no matter how large the triggering burst was.
///
/// The repo mutex is held only for brief snapshot/apply windows — NEVER
/// across the walk or the embedding — so `CodeIndexManager::search` (agent
/// context prefetch, search_codebase) keeps answering during a rescan
/// instead of blocking for its full duration.
pub(crate) fn full_rescan(
    repo_state: &Arc<Mutex<RepoState>>,
    embedder: &impl EmbedText,
    cache: &EmbeddingCache,
    max_files: usize,
) -> Result<()> {
    // Brief lock: capture what the walk/diff needs.
    let (root, prev_tree, store_consistent) = {
        let state = repo_state.lock().unwrap();
        (
            state.root.clone(),
            state.tree.clone(),
            ensure_store_matches_tree(&state.store, &state.root, &state.tree).is_ok(),
        )
    };

    // Heavy work, lock-free.
    let current = FileTreeWalker::build_tree(&root, max_files)?;
    let rebuild_all = !store_consistent;
    let (to_embed, to_remove) = if rebuild_all {
        log::warn!(
            "[index] live tree/store mismatch for {} — full vector rebuild",
            root.display()
        );
        (current.fragments.keys().cloned().collect(), Vec::new())
    } else {
        diff_trees(Some(&prev_tree), &current)
    };
    log::info!(
        "[index] rescan: {} to embed, {} to remove for {}",
        to_embed.len(),
        to_remove.len(),
        root.display()
    );

    // Collect and embed before changing the searchable store or its snapshot.
    // If inference fails, the previous consistent index remains intact and the
    // rescan can be retried safely. Unchanged content reuses stored vectors.
    let mut pairs: Vec<(super::merkle::Fragment, String)> = Vec::new();
    for rel in &to_embed {
        let abs = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&abs) else {
            continue;
        };
        if let Some(frags) = current.fragments.get(rel) {
            for frag in frags {
                let text = content.get(frag.byte_start..frag.byte_end).unwrap_or("");
                if !text.is_empty() {
                    pairs.push((frag.clone(), text.to_string()));
                }
            }
        }
    }
    let embeddings = {
        let mut slots = cached_embeddings(cache, &pairs);
        embed_missing(embedder, cache, &pairs, &mut slots)?;
        slots
            .into_iter()
            .map(|slot| slot.ok_or_else(|| anyhow::anyhow!("embedding missing after embed")))
            .collect::<Result<Vec<_>>>()?
    };

    // Commit rows, tree, and snapshot together only after all embeddings exist.
    {
        let mut state = repo_state.lock().unwrap();
        if rebuild_all {
            state.store.clear()?;
        } else {
            for rel in to_remove.iter().chain(to_embed.iter()) {
                state.store.remove_file(&root.join(rel))?;
            }
        }
        for ((frag, text), embedding) in pairs.iter().zip(&embeddings) {
            state.store.upsert(
                &frag.fragment_id,
                &root.join(&frag.path),
                frag.byte_start,
                frag.byte_end,
                text,
                embedding,
            )?;
        }
        state.tree = current;
        ensure_store_matches_tree(&state.store, &root, &state.tree)?;
        IndexSnapshot::save(&state.tree, &state.snapshot_path)?;
        state.last_snapshot_save = std::time::Instant::now();
    }
    Ok(())
}

/// Fragment/path identities a complete store must contain for this tree.
/// Zero-length files intentionally have no embedding; whitespace-only source
/// is embedded so this manifest remains derivable from the snapshot alone.
pub(crate) fn expected_store_entries(root: &Path, tree: &MerkleTree) -> HashSet<(String, String)> {
    tree.fragments
        .values()
        .flatten()
        .filter(|fragment| fragment.byte_end > fragment.byte_start)
        .map(|fragment| {
            (
                fragment.fragment_id.clone(),
                root.join(&fragment.path).to_string_lossy().into_owned(),
            )
        })
        .collect()
}

pub(crate) fn ensure_store_matches_tree(
    store: &VectorStore,
    root: &Path,
    tree: &MerkleTree,
) -> Result<()> {
    let expected = expected_store_entries(root, tree);
    let actual = store.entries()?;
    if actual == expected {
        return Ok(());
    }
    anyhow::bail!(
        "vector store incomplete for {}: expected {} fragment/path rows, found {}",
        root.display(),
        expected.len(),
        actual.len()
    )
}

/// Split `pairs` into already-embedded (reused verbatim, no model call) and
/// still-missing slots, keyed by each fragment's content hash. Fast SQLite
/// Split `pairs` into already-embedded (reused verbatim from the repo-shared
/// cache, no model call) and still-missing slots, keyed by each fragment's
/// content hash. Lock-free cache reads only.
pub(crate) fn cached_embeddings(
    cache: &EmbeddingCache,
    pairs: &[(super::merkle::Fragment, String)],
) -> Vec<Option<Vec<f32>>> {
    pairs
        .iter()
        .map(|(frag, _)| cache.get(&frag.content_hash))
        .collect()
}

/// Embed every `None` slot, in `EMBED_BATCH` chunks, remembering each new
/// vector in the repo-shared cache for the next worktree. Slow model
/// inference — callers must NOT hold the repo lock across this (agent
/// searches stay responsive while a burst embeds). A fully-cached input
/// embeds nothing and returns without sleeping.
pub(crate) fn embed_missing(
    embedder: &impl EmbedText,
    cache: &EmbeddingCache,
    pairs: &[(super::merkle::Fragment, String)],
    slots: &mut [Option<Vec<f32>>],
) -> Result<()> {
    debug_assert_eq!(pairs.len(), slots.len());
    let pending: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter_map(|(i, slot)| slot.is_none().then_some(i))
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    for chunk in pending.chunks(EMBED_BATCH) {
        std::thread::sleep(EMBED_BATCH_BREATHER);
        let texts: Vec<String> = chunk.iter().map(|&i| pairs[i].1.clone()).collect();
        let embeddings = embedder.embed_texts(texts)?;
        if embeddings.len() != chunk.len() {
            anyhow::bail!(
                "embedding batch returned {} vectors for {} fragments",
                embeddings.len(),
                chunk.len()
            );
        }
        for (j, embedding) in embeddings.into_iter().enumerate() {
            cache.put(&pairs[chunk[j]].0.content_hash, &embedding);
            slots[chunk[j]] = Some(embedding);
        }
    }
    Ok(())
}

/// Returns (files_to_embed, files_to_remove) by diffing old vs new tree.
pub(crate) fn diff_trees(
    prev: Option<&MerkleTree>,
    current: &MerkleTree,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut to_embed: Vec<PathBuf> = Vec::new();
    let mut to_remove: Vec<PathBuf> = Vec::new();

    let prev_fragments = prev.map(|t| &t.fragments);

    for (rel, current_frags) in &current.fragments {
        let changed = match prev_fragments {
            None => true,
            Some(prev_map) => match prev_map.get(rel) {
                None => true, // new file
                Some(prev_frags) => {
                    // Compare by content hash — if fragment_ids match, content is identical.
                    let prev_ids: HashSet<&str> =
                        prev_frags.iter().map(|f| f.fragment_id.as_str()).collect();
                    let curr_ids: HashSet<&str> = current_frags
                        .iter()
                        .map(|f| f.fragment_id.as_str())
                        .collect();
                    prev_ids != curr_ids
                }
            },
        };
        if changed {
            to_embed.push(rel.clone());
        }
    }

    // Files that existed before but are gone now.
    if let Some(prev_map) = prev_fragments {
        for rel in prev_map.keys() {
            if !current.fragments.contains_key(rel) {
                to_remove.push(rel.clone());
            }
        }
    }

    (to_embed, to_remove)
}

/// Embed a subset of files from `tree` and upsert them into `store`.
/// Fragments whose content hash is already in the repo-shared `cache`
/// (e.g. an unchanged file in a second worktree of the same repo) reuse
/// their vector without calling the embedder.
pub(crate) fn embed_files(
    embedder: &impl EmbedText,
    cache: &EmbeddingCache,
    store: &VectorStore,
    root: &Path,
    tree: &MerkleTree,
    rel_paths: &[PathBuf],
) -> Result<()> {
    // Collect all (fragment, text) pairs for the files we need to embed.
    let mut pairs: Vec<(super::merkle::Fragment, String)> = Vec::new();

    for rel in rel_paths {
        let abs = root.join(rel);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some(frags) = tree.fragments.get(rel) {
            for frag in frags {
                let text = content.get(frag.byte_start..frag.byte_end).unwrap_or("");
                if !text.is_empty() {
                    pairs.push((frag.clone(), text.to_string()));
                }
            }
        }
    }

    if pairs.is_empty() {
        return Ok(());
    }

    let mut slots = cached_embeddings(cache, &pairs);
    embed_missing(embedder, cache, &pairs, &mut slots)?;
    for ((frag, text), slot) in pairs.iter().zip(&slots) {
        let Some(emb) = slot else {
            anyhow::bail!("embedding missing after embed");
        };
        let abs = root.join(&frag.path);
        store.upsert(
            &frag.fragment_id,
            &abs,
            frag.byte_start,
            frag.byte_end,
            text,
            emb,
        )?;
    }

    Ok(())
}

/// Canonical form of a repo root, so the SAME folder always maps to the same
/// key/DB regardless of how it was spelled. On Windows this collapses
/// case-insensitive differences (C:\\Users vs c:\\users), separator/prefix
/// variants, and the verbatim \\?\\ prefix that `std::fs::canonicalize`
/// emits — the exact cause of a folder (and its files) being indexed twice.
pub(crate) fn normalize_root(root: &Path) -> PathBuf {
    dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Delete a worktree's persisted index artifacts (vector DB + snapshot).
/// Failures are logged, not fatal: a leftover file is reconciled on the next
/// re-open (missing snapshot + populated store → full rebuild).
pub(crate) fn delete_index_artifacts(root: &Path) {
    for path in [db_path_for_repo(root), snapshot_path_for_repo(root)] {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[index] could not delete {}: {e}", path.display());
            }
        }
    }
}

pub(crate) fn db_path_for_repo(root: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    dragent_paths::home()
        .join("code_index")
        .join(format!("{}.db", &hash[..16]))
}

pub(crate) fn snapshot_path_for_repo(root: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(root.to_string_lossy().as_bytes()));
    dragent_paths::home()
        .join("code_index")
        .join(format!("{}.snap", &hash[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_watcher::ChangeBatch;
    use crate::manager::{RepoActivity, RepoState};
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    /// Deterministic fake: one small vector per text, derived from its
    /// content hash. Counts batch calls so tests prove reuse.
    struct FakeEmbedder {
        calls: AtomicUsize,
    }

    impl FakeEmbedder {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl EmbedText for FakeEmbedder {
        fn embed_texts(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(texts
                .iter()
                .map(|text| {
                    use sha2::{Digest, Sha256};
                    let digest = Sha256::digest(text.as_bytes());
                    digest[..8].iter().map(|b| f32::from(*b) / 255.0).collect()
                })
                .collect())
        }
    }

    fn write_files(root: &Path, files: &[(&str, &str)]) {
        for (rel, content) in files {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        }
    }

    fn rels(tree: &MerkleTree) -> Vec<PathBuf> {
        tree.fragments.keys().cloned().collect()
    }

    fn test_cache(dir: &Path) -> EmbeddingCache {
        EmbeddingCache::open(&dir.join("embed-cache-test.db")).unwrap()
    }

    /// All texts currently stored, via a top-k large enough to return every row.
    fn stored_texts(store: &VectorStore, query: &[f32]) -> Vec<String> {
        store
            .search(query, 10_000)
            .unwrap()
            .into_iter()
            .map(|hit| hit.text)
            .collect()
    }

    #[test]
    fn second_worktree_reuses_cache_and_keeps_own_rows() {
        let base = tempfile::tempdir().unwrap();
        let wt1 = base.path().join("wt1");
        let wt2 = base.path().join("wt2");
        let same = vec![
            ("src/a.rs", "pub fn alpha() -> u32 {\n    1\n}\n"),
            ("src/b.rs", "pub fn beta() -> u32 {\n    2\n}\n"),
        ];
        write_files(&wt1, &same);
        // Second worktree: identical content, different location.
        write_files(&wt2, &same);

        // One shared cache, but each worktree keeps its own store (own rows).
        let cache = test_cache(base.path());
        let store1 = VectorStore::open(&base.path().join("wt1.db")).unwrap();
        let store2 = VectorStore::open(&base.path().join("wt2.db")).unwrap();
        let fake = FakeEmbedder::new();

        let tree1 = FileTreeWalker::build_tree(&wt1, 10_000).unwrap();
        embed_files(&fake, &cache, &store1, &wt1, &tree1, &rels(&tree1)).unwrap();
        let first_pass_calls = fake.calls();
        assert!(first_pass_calls > 0, "first worktree must embed");
        assert!(cache.len() > 0, "vectors must land in the shared cache");

        let tree2 = FileTreeWalker::build_tree(&wt2, 10_000).unwrap();
        embed_files(&fake, &cache, &store2, &wt2, &tree2, &rels(&tree2)).unwrap();
        assert_eq!(
            fake.calls(),
            first_pass_calls,
            "unchanged files in a second worktree must not be re-embedded"
        );

        // …but the second worktree still owns its rows, under its own paths.
        let probe = fake.embed_texts(vec!["probe".into()]).unwrap().remove(0);
        let texts1 = stored_texts(&store1, &probe);
        let texts2 = stored_texts(&store2, &probe);
        assert_eq!(texts1.len(), texts2.len());
        assert!(texts1.iter().any(|t| t.contains("fn alpha")));
        assert!(texts2.iter().any(|t| t.contains("fn alpha")));
        for (fragment_id, path) in store2.entries().unwrap() {
            assert!(
                Path::new(&path).starts_with(&wt2),
                "worktree B must own B-absolute rows, got {path} ({fragment_id})"
            );
        }
        for (fragment_id, path) in store1.entries().unwrap() {
            assert!(
                Path::new(&path).starts_with(&wt1),
                "worktree A must own A-absolute rows, got {path} ({fragment_id})"
            );
        }
    }

    #[test]
    fn edit_in_b_only_leaves_a_untouched() {
        let base = tempfile::tempdir().unwrap();
        let wt1 = base.path().join("wt1");
        let wt2 = base.path().join("wt2");
        let v1 = "pub fn target() -> u32 {\n    1\n}\n";
        let v2 = "pub fn target() -> u32 {\n    2\n}\n";
        write_files(&wt1, &[("src/x.rs", v1)]);
        write_files(&wt2, &[("src/x.rs", v1)]);

        let cache = test_cache(base.path());
        let store1 = VectorStore::open(&base.path().join("wt1.db")).unwrap();
        let store2 = VectorStore::open(&base.path().join("wt2.db")).unwrap();
        let fake = FakeEmbedder::new();

        let tree1 = FileTreeWalker::build_tree(&wt1, 10_000).unwrap();
        embed_files(&fake, &cache, &store1, &wt1, &tree1, &rels(&tree1)).unwrap();
        let tree2a = FileTreeWalker::build_tree(&wt2, 10_000).unwrap();
        embed_files(&fake, &cache, &store2, &wt2, &tree2a, &rels(&tree2a)).unwrap();
        let shared_calls = fake.calls();

        // Edit X only in worktree B: only its diff re-embeds…
        write_files(&wt2, &[("src/x.rs", v2)]);
        let tree2b = FileTreeWalker::build_tree(&wt2, 10_000).unwrap();
        let (to_embed, _) = diff_trees(Some(&tree2a), &tree2b);
        assert_eq!(to_embed, vec![PathBuf::from("src/x.rs")]);
        embed_files(&fake, &cache, &store2, &wt2, &tree2b, &to_embed).unwrap();
        assert!(
            fake.calls() > shared_calls,
            "changed content must reach the embedder"
        );

        // …and a search in A still returns A's X, B returns B's.
        let probe = fake.embed_texts(vec!["probe".into()]).unwrap().remove(0);
        let texts1 = stored_texts(&store1, &probe);
        let texts2 = stored_texts(&store2, &probe);
        assert!(texts1.iter().any(|t| t.contains("    1")));
        assert!(!texts1.iter().any(|t| t.contains("    2")));
        assert!(texts2.iter().any(|t| t.contains("    2")));
    }

    #[test]
    fn changed_file_embeds_only_its_diff() {
        let base = tempfile::tempdir().unwrap();
        let wt = base.path().join("wt");
        write_files(&wt, &[("src/a.rs", "pub fn alpha() -> u32 {\n    1\n}\n")]);

        let cache = test_cache(base.path());
        let store = VectorStore::open(&base.path().join("wt.db")).unwrap();
        let fake = FakeEmbedder::new();

        let before = FileTreeWalker::build_tree(&wt, 10_000).unwrap();
        embed_files(&fake, &cache, &store, &wt, &before, &rels(&before)).unwrap();
        assert_eq!(cache.len(), 1);

        // Append a line: the diff names only the changed file…
        write_files(
            &wt,
            &[("src/a.rs", "pub fn alpha() -> u32 {\n    1\n}\n// tweak\n")],
        );
        let after = FileTreeWalker::build_tree(&wt, 10_000).unwrap();
        let (to_embed, _) = diff_trees(Some(&before), &after);
        assert_eq!(to_embed, vec![PathBuf::from("src/a.rs")]);
        let calls_before = fake.calls();
        embed_files(&fake, &cache, &store, &wt, &after, &to_embed).unwrap();
        assert!(
            fake.calls() > calls_before,
            "changed content must reach the embedder"
        );
    }

    /// The exact path the file watcher takes for one batch, with a fake
    /// embedder: B's edits land in B's store. (The watcher thread itself
    /// needs the real model, so it is not started in unit tests.)
    #[test]
    fn watcher_batch_path_applies_edits() {
        let base = tempfile::tempdir().unwrap();
        let wt = base.path().join("wt");
        write_files(&wt, &[("src/x.rs", "version one\n")]);

        let cache = test_cache(base.path());
        let store = VectorStore::open(&base.path().join("wt.db")).unwrap();
        let fake = FakeEmbedder::new();
        let tree = FileTreeWalker::build_tree(&wt, 10_000).unwrap();
        embed_files(&fake, &cache, &store, &wt, &tree, &rels(&tree)).unwrap();
        let calls_before = fake.calls();

        // A watcher batch for the edit B just made on disk.
        write_files(&wt, &[("src/x.rs", "version two\n")]);
        let repo_state = std::sync::Arc::new(std::sync::Mutex::new(RepoState {
            root: wt.clone(),
            tree,
            store,
            snapshot_path: base.path().join("wt.snap"),
            bm25_cache: None,
            last_snapshot_save: Instant::now(),
        }));
        let activity = RepoActivity::new(crate::RepoIndexState::Watching);
        let mut batch = ChangeBatch::default();
        batch.record(PathBuf::from("src/x.rs"), false);
        assert!(process_batch(
            &repo_state,
            &fake,
            &cache,
            batch,
            10_000,
            &activity
        ));
        assert!(
            fake.calls() > calls_before,
            "the batch must embed the changed file"
        );

        let state = repo_state.lock().unwrap();
        let probe = fake.embed_texts(vec!["probe".into()]).unwrap().remove(0);
        let texts = stored_texts(&state.store, &probe);
        assert!(texts.iter().any(|t| t.contains("version two")));
        assert!(!texts.iter().any(|t| t.contains("version one")));
    }
}
