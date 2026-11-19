use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, params};

pub struct SearchResult {
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub score: f32,
}

pub struct VectorStore {
    conn: Connection,
    /// True when `open` found a stale/absent schema version and wiped the DB.
    /// The caller must then also discard its snapshot and re-embed everything,
    /// otherwise the snapshot would claim files are indexed while the store is
    /// empty (a permanent-empty-index desync).
    schema_reset: bool,
}

impl VectorStore {
    // Schema version — bump when the embedding model or storage format changes so
    // old DBs (and, via `schema_reset`, their snapshots) are discarded.
    //   v2: BGESmallENV15Q (384 dims)
    //   v3: BGESmallENV15 full-precision (quantized model crashed at inference)
    //   v4: max_length 512 → 256 (truncation changes embeddings of long chunks)
    //   v5: (fragment_id, path) composite key. Identical chunks in different
    //       files must not overwrite one another.
    const SCHEMA_VERSION: i64 = 5;

    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;

        // Check schema version; wipe and rebuild if stale.
        conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_meta (version INTEGER NOT NULL);")?;
        let stored_version: Option<i64> = conn
            .query_row("SELECT version FROM schema_meta LIMIT 1", [], |r| r.get(0))
            .ok();
        let schema_reset = stored_version != Some(Self::SCHEMA_VERSION);
        if schema_reset {
            conn.execute_batch(
                "DROP TABLE IF EXISTS chunks;
                 DROP INDEX IF EXISTS idx_chunks_path;
                 DELETE FROM schema_meta;",
            )?;
            conn.execute(
                "INSERT INTO schema_meta (version) VALUES (?1)",
                rusqlite::params![Self::SCHEMA_VERSION],
            )?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS chunks (
                fragment_id  TEXT NOT NULL,
                path         TEXT NOT NULL,
                byte_start   INTEGER NOT NULL,
                byte_end     INTEGER NOT NULL,
                text         TEXT NOT NULL,
                embedding    BLOB NOT NULL,
                PRIMARY KEY (fragment_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_path ON chunks(path);",
        )?;
        Ok(Self { conn, schema_reset })
    }

    /// Whether `open` wiped a stale-version DB. When true the caller should treat
    /// any previous snapshot as invalid and re-embed the whole repo.
    pub fn schema_was_reset(&self) -> bool {
        self.schema_reset
    }

    pub fn upsert(
        &self,
        fragment_id: &str,
        path: &Path,
        byte_start: usize,
        byte_end: usize,
        text: &str,
        embedding: &[f32],
    ) -> Result<()> {
        let blob: &[u8] = bytemuck::cast_slice(embedding);
        self.conn.execute(
            "INSERT OR REPLACE INTO chunks \
             (fragment_id, path, byte_start, byte_end, text, embedding) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                fragment_id,
                path.to_string_lossy().as_ref(),
                byte_start as i64,
                byte_end as i64,
                text,
                blob,
            ],
        )?;
        Ok(())
    }

    /// Drop every chunk. Used by force re-index: the caller deletes the
    /// snapshot so all files re-embed, and clearing here guarantees no stale
    /// rows survive for files deleted since the last snapshot.
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM chunks", [])?;
        Ok(())
    }

    pub fn remove_file(&self, path: &Path) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chunks WHERE path = ?1",
            params![path.to_string_lossy().as_ref()],
        )?;
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Exact identity set persisted in the vector store. Used at startup to
    /// verify that the Merkle snapshot and searchable rows describe the same
    /// index before an incremental diff is trusted.
    pub(crate) fn entries(&self) -> Result<HashSet<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT fragment_id, path FROM chunks")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.filter_map(|row| row.ok()).collect())
    }

    /// Brute-force cosine similarity search. Fast enough for <100K chunks.
    /// Embeddings are stored L2-normalised so dot product == cosine similarity.
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, byte_start, byte_end, text, embedding FROM chunks")?;

        let mut scored: Vec<(f32, SearchResult)> = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let byte_start: i64 = row.get(1)?;
                let byte_end: i64 = row.get(2)?;
                let text: String = row.get(3)?;
                let blob: Vec<u8> = row.get(4)?;
                Ok((path, byte_start as usize, byte_end as usize, text, blob))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(path, byte_start, byte_end, text, blob)| {
                let emb: &[f32] = bytemuck::cast_slice(&blob);
                // Skip any chunk whose stored dimension doesn't match the query
                // (e.g. a leftover from a different model) rather than silently
                // scoring a truncated vector.
                if emb.len() != query.len() {
                    return None;
                }
                let score = dot(query, emb);
                Some((
                    score,
                    SearchResult {
                        path: PathBuf::from(path),
                        byte_start,
                        byte_end,
                        text,
                        score,
                    },
                ))
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored.into_iter().map(|(_, r)| r).collect())
    }
}

#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    a[..len].iter().zip(&b[..len]).map(|(x, y)| x * y).sum()
}
