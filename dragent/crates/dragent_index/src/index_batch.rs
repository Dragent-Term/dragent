use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;

use super::embedder::Embedder;
use super::merkle::MerkleTree;
use super::snapshot::IndexSnapshot;
use super::store::VectorStore;
use super::walk::FileTreeWalker;
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
pub(crate) fn process_batch(
    repo_state: &Arc<Mutex<RepoState>>,
    embedder: &Embedder,
    batch: ChangeBatch,
    max_files: usize,
    activity: &RepoActivity,
) -> bool {
    if batch.len() > FULL_RESCAN_THRESHOLD {
        log::info!(
            "[index] {} pending changes exceeds threshold — full rescan",
            batch.len()
        );
        if let Err(e) = full_rescan(repo_state, embedder, max_files) {
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

    // Embed WITHOUT the lock; retake it briefly per batch for the upserts.
    for chunk in pairs.chunks(EMBED_BATCH) {
        std::thread::sleep(EMBED_BATCH_BREATHER);
        let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
        match embedder.embed(texts.clone()) {
            Ok(embeddings) => {
                let state = repo_state.lock().unwrap();
                for (i, emb) in embeddings.iter().enumerate() {
                    let (frag, text) = &chunk[i];
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
            // Skipped files stay stale until their next change or the
            // startup diff — never fatal to the watcher.
            Err(e) => {
                log::warn!("[index] batch embed failed: {e}");
                return false;
            }
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
    embedder: &Embedder,
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
    // rescan can be retried safely.
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
    let embeddings = embed_pairs(embedder, &pairs)?;

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

fn embed_pairs(
    embedder: &Embedder,
    pairs: &[(super::merkle::Fragment, String)],
) -> Result<Vec<Vec<f32>>> {
    let mut all = Vec::with_capacity(pairs.len());
    for batch in pairs.chunks(EMBED_BATCH) {
        std::thread::sleep(EMBED_BATCH_BREATHER);
        let texts = batch.iter().map(|(_, text)| text.clone()).collect();
        let embeddings = embedder.embed(texts)?;
        if embeddings.len() != batch.len() {
            anyhow::bail!(
                "embedding batch returned {} vectors for {} fragments",
                embeddings.len(),
                batch.len()
            );
        }
        all.extend(embeddings);
    }
    Ok(all)
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
pub(crate) fn embed_files(
    embedder: &Embedder,
    store: &VectorStore,
    root: &Path,
    tree: &MerkleTree,
    rel_paths: &[PathBuf],
) -> Result<()> {
    const BATCH: usize = EMBED_BATCH;

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

    for batch in pairs.chunks(BATCH) {
        std::thread::sleep(EMBED_BATCH_BREATHER);
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let embeddings = embedder.embed(texts.clone())?;

        for (i, emb) in embeddings.iter().enumerate() {
            let (frag, text) = &batch[i];
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

/// Delete a repo's persisted index artifacts (vector DB + snapshot). Failures
/// are logged, not fatal: a leftover file is reconciled on the next re-open
/// (missing snapshot + populated store → full rebuild).
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
