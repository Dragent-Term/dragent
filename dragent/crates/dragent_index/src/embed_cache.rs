//! Shared content-hash embedding cache: one SQLite file per repository.
//!
//! Worktrees of one repo keep their own path→fragment rows and their own
//! watcher (so a search in worktree A always returns A's code), but they
//! share this cache keyed by chunk content hash. A new worktree therefore
//! re-embeds nothing for unchanged files: every identical chunk hits the
//! cache instead of the model. The cache is a pure performance artifact —
//! hits and misses never affect correctness, and losing the file only costs
//! re-embedding.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, params};

/// Content hash → L2-normalised embedding vector, shared by all worktrees of
/// one repo. Interior mutability so one `Arc` can serve every watcher thread.
pub(crate) struct EmbeddingCache {
    conn: Mutex<Connection>,
}

impl EmbeddingCache {
    fn open_inner(path: &Path) -> Result<Connection> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // Two worktrees' watchers may write concurrently; wait rather than
        // failing the batch with SQLITE_BUSY.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (
                content_hash TEXT PRIMARY KEY,
                embedding    BLOB NOT NULL
            );",
        )?;
        Ok(conn)
    }

    /// Open (or create) the persistent cache file.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: Mutex::new(Self::open_inner(path)?),
        })
    }

    /// Best-effort fallback when the cache file cannot be opened (read-only
    /// home, full disk): an empty memory cache. Correctness is unaffected —
    /// every lookup just misses and the model embeds.
    pub(crate) fn open_memory() -> Self {
        let conn = Connection::open_in_memory()
            .and_then(|conn| {
                conn.execute_batch(
                    "CREATE TABLE embeddings (
                        content_hash TEXT PRIMARY KEY,
                        embedding    BLOB NOT NULL
                    );",
                )?;
                Ok(conn)
            })
            .expect("in-memory cache must open");
        Self {
            conn: Mutex::new(conn),
        }
    }

    /// Cached vector for this exact chunk content, if any.
    pub(crate) fn get(&self, content_hash: &str) -> Option<Vec<f32>> {
        let conn = self.conn.lock().ok()?;
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT embedding FROM embeddings WHERE content_hash = ?1",
                params![content_hash],
                |row| row.get(0),
            )
            .ok()?;
        Some(bytemuck::cast_slice(&blob).to_vec())
    }

    /// Remember a freshly embedded vector. Failures are logged, never fatal:
    /// the row's store upsert already holds the vector for this worktree.
    pub(crate) fn put(&self, content_hash: &str, embedding: &[f32]) {
        let blob: &[u8] = bytemuck::cast_slice(embedding);
        if let Ok(conn) = self.conn.lock() {
            if let Err(e) = conn.execute(
                "INSERT OR REPLACE INTO embeddings (content_hash, embedding) VALUES (?1, ?2)",
                params![content_hash, blob],
            ) {
                log::warn!("[index] embedding cache put failed: {e}");
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.conn
            .lock()
            .ok()
            .and_then(|conn| {
                conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| {
                    r.get::<_, i64>(0)
                })
                .ok()
            })
            .unwrap_or(0) as usize
    }
}

/// On-disk cache file for one repo, shared by all its worktrees: the key is
/// the repo identity (`repo_key` — the git common dir), NOT the worktree
/// path, so a second worktree finds the first one's embeddings.
pub(crate) fn cache_path_for_repo(repo_key: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(repo_key.to_string_lossy().as_bytes()));
    dragent_paths::home()
        .join("code_index")
        .join(format!("embed-cache-{}.db", &hash[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_and_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = EmbeddingCache::open(&dir.path().join("c.db")).unwrap();
        assert_eq!(cache.get("nope"), None);
        cache.put("abc", &[1.0, 2.0, 3.0]);
        assert_eq!(cache.get("abc"), Some(vec![1.0, 2.0, 3.0]));
        assert_eq!(cache.len(), 1);
        // Overwrite wins.
        cache.put("abc", &[4.0]);
        assert_eq!(cache.get("abc"), Some(vec![4.0]));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_persists_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.db");
        EmbeddingCache::open(&path).unwrap().put("abc", &[1.0]);
        // A second opener (another worktree's watcher) sees the first one's rows.
        let again = EmbeddingCache::open(&path).unwrap();
        assert_eq!(again.get("abc"), Some(vec![1.0]));
    }

    #[test]
    fn cache_paths_differ_per_repo_key() {
        let a = cache_path_for_repo(Path::new("/repo/.git"));
        let b = cache_path_for_repo(Path::new("/other/.git"));
        assert_ne!(a, b);
        // Same key, however spelled, maps to one file.
        assert_eq!(
            cache_path_for_repo(Path::new("/repo/.git")),
            cache_path_for_repo(Path::new("/repo/.git"))
        );
    }
}
