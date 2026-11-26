// The real indexing engine (ONNX embeddings, SQLite vector store, filesystem
// walker + watcher) is native-only. On wasm, `wasm_stub` supplies the small
// surface the app touches (`get_manager()` + a no-op manager, `walk`) so the
// viewer type-checks without any indexing machinery.
#[cfg(not(target_family = "wasm"))]
pub mod chunker;
#[cfg(not(target_family = "wasm"))]
pub mod embedder;
#[cfg(not(target_family = "wasm"))]
mod embed_cache;
mod index_batch;
mod index_embed;
mod index_repos;
mod index_search;
mod index_watcher;
#[cfg(not(target_family = "wasm"))]
pub mod inverted;
#[cfg(not(target_family = "wasm"))]
pub mod manager;
#[cfg(not(target_family = "wasm"))]
pub mod merkle;
#[cfg(not(target_family = "wasm"))]
pub mod repo_key;
#[cfg(not(target_family = "wasm"))]
pub mod snapshot;
#[cfg(not(target_family = "wasm"))]
pub mod store;
#[cfg(not(target_family = "wasm"))]
pub mod symbols;
#[cfg(not(target_family = "wasm"))]
pub mod walk;

#[cfg(not(target_family = "wasm"))]
pub use manager::{
    CODEBASE_INDEX_MANAGER, CodeIndexManager, RepoIndexState, RepoIndexStatus, get_manager,
};
#[cfg(not(target_family = "wasm"))]
pub use {
    chunker::Chunker,
    embedder::{EmbedText, Embedder},
    index_search::{HybridHit, RRF_K, reciprocal_rank_fusion, symbol_tokens},
    inverted::{InvertedIndex, SearchHit},
    merkle::{Fragment, MerkleNode, MerkleNodeType, MerkleTree},
    repo_key::repo_index_key,
    snapshot::IndexSnapshot,
    store::{SearchResult, VectorStore},
    symbols::{Symbol, SymbolKind, extract_symbols},
    walk::FileTreeWalker,
};

#[cfg(target_family = "wasm")]
mod wasm_stub;
#[cfg(target_family = "wasm")]
pub use wasm_stub::*;

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn merkle_empty_repo() {
        let tree = MerkleTree::new();
        assert!(!tree.root_hash.is_empty());
        assert_eq!(tree.nodes.len(), 1);
        let root = tree.nodes.get(&tree.root_hash).unwrap();
        assert_eq!(root.node_type, MerkleNodeType::Directory);
        assert_eq!(root.relative_path, PathBuf::from(""));
        assert!(root.children.is_empty());
    }

    #[test]
    fn merkle_single_file_build() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("a.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tree = FileTreeWalker::build_tree(dir.path(), 10).unwrap();
        assert!(!tree.root_hash.is_empty());
        assert!(!tree.fragments.is_empty());
        assert_eq!(tree.nodes.len(), 2);
    }

    #[test]
    fn store_upsert_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let store = VectorStore::open(&db).unwrap();
        assert_eq!(store.count(), 0);

        let emb = vec![1.0f32, 0.0, 0.0];
        store
            .upsert(
                "frag1",
                &PathBuf::from("/foo.rs"),
                0,
                10,
                "fn foo() {}",
                &emb,
            )
            .unwrap();
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn store_keeps_identical_fragment_ids_from_different_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = VectorStore::open(&dir.path().join("duplicates.db")).unwrap();
        let embedding = vec![1.0f32, 0.0, 0.0];

        store
            .upsert(
                "same-fragment",
                &PathBuf::from("/a.rs"),
                0,
                4,
                "same",
                &embedding,
            )
            .unwrap();
        store
            .upsert(
                "same-fragment",
                &PathBuf::from("/b.rs"),
                0,
                4,
                "same",
                &embedding,
            )
            .unwrap();

        assert_eq!(store.count(), 2);
        let entries = store.entries().unwrap();
        assert!(entries.contains(&("same-fragment".into(), "/a.rs".into())));
        assert!(entries.contains(&("same-fragment".into(), "/b.rs".into())));
    }

    #[test]
    fn old_store_schema_is_cleared_for_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("old.db");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_meta (version INTEGER NOT NULL);
                 INSERT INTO schema_meta VALUES (4);
                 CREATE TABLE chunks (
                    fragment_id TEXT PRIMARY KEY,
                    path TEXT NOT NULL,
                    byte_start INTEGER NOT NULL,
                    byte_end INTEGER NOT NULL,
                    text TEXT NOT NULL,
                    embedding BLOB NOT NULL
                 );
                 INSERT INTO chunks VALUES ('stale', '/stale.rs', 0, 5, 'stale', X'0000');",
            )
            .unwrap();
        drop(connection);

        let store = VectorStore::open(&db).unwrap();
        assert!(store.schema_was_reset());
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn store_cosine_search() {
        let dir = tempfile::tempdir().unwrap();
        let store = VectorStore::open(&dir.path().join("v.db")).unwrap();

        // Two embeddings: one aligned with query, one orthogonal.
        let aligned = vec![1.0f32, 0.0, 0.0];
        let ortho = vec![0.0f32, 1.0, 0.0];
        let query = vec![1.0f32, 0.0, 0.0];

        store
            .upsert("a", &PathBuf::from("/a.rs"), 0, 5, "a", &aligned)
            .unwrap();
        store
            .upsert("b", &PathBuf::from("/b.rs"), 0, 5, "b", &ortho)
            .unwrap();

        let results = store.search(&query, 2).unwrap();
        assert_eq!(results[0].path, PathBuf::from("/a.rs"));
        assert!(results[0].score > results[1].score);
    }

    #[test]
    #[ignore] // Downloads ~130MB + runs inference. Run: cargo test -p dragent_index embedder_real -- --ignored
    fn embedder_real_load_and_embed() {
        use crate::embedder::Embedder;
        let emb = Embedder::load().expect("model should load");
        // Passages (no instruction) — this is the call that crashed on the
        // quantized model with a SkipLayerNormalization "Missing Input" error.
        let passages = emb
            .embed(vec!["fn main() {}".to_string(), "hello world".to_string()])
            .expect("embed passages should not crash");
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[0].len(), 384, "BGE-small is 384-dim");
        // Query path (with the BGE instruction prefix).
        let q = emb.embed_query("program entry point").expect("embed query");
        assert_eq!(q.len(), 384);
    }

    #[test]
    fn store_search_skips_dimension_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let store = VectorStore::open(&dir.path().join("v.db")).unwrap();

        // A matching-dim chunk and a stale chunk with a different dimension.
        store
            .upsert(
                "ok",
                &PathBuf::from("/ok.rs"),
                0,
                1,
                "ok",
                &[1.0f32, 0.0, 0.0],
            )
            .unwrap();
        store
            .upsert(
                "bad",
                &PathBuf::from("/bad.rs"),
                0,
                1,
                "bad",
                &[1.0f32, 0.0],
            )
            .unwrap();

        let results = store.search(&[1.0f32, 0.0, 0.0], 10).unwrap();
        // The mismatched-dimension chunk is skipped, not silently truncated.
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, PathBuf::from("/ok.rs"));
    }
}
