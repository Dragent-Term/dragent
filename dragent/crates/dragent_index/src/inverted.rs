//! BM25 inverted index over codebase fragments.
//!
//! Built from file content after chunking. Lets the agent search the codebase
//! with ranked results instead of brute-force substring scanning.

use crate::merkle::MerkleTree;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;

/// A ranked search hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: PathBuf,
    /// Index into `tree.fragments[path]`.
    pub fragment_idx: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub score: f32,
    /// Matching symbol name (if hit came from symbol lookup).
    pub matched_symbol: Option<String>,
}

/// Per-document (fragment) term data used for BM25 scoring.
#[derive(Clone, Debug)]
struct FragmentEntry {
    path: PathBuf,
    fragment_idx: usize,
    byte_start: usize,
    byte_end: usize,
    /// Number of tokens in this fragment (document length).
    doc_len: u32,
}

/// `term → Vec<(entry_idx, term_freq)>`
type PostingList = Vec<(usize, u32)>;

/// BM25 inverted index over all indexed fragments.
#[derive(Debug)]
pub struct InvertedIndex {
    entries: Vec<FragmentEntry>,
    /// Lower-cased term → posting list [(entry_idx, tf)]
    postings: HashMap<String, PostingList>,
    avg_doc_len: f32,
}

impl InvertedIndex {
    /// Build the index from a MerkleTree by reading file content from disk.
    pub fn build(repo_root: &Path, tree: &MerkleTree) -> Self {
        let mut entries: Vec<FragmentEntry> = Vec::new();
        let mut postings: HashMap<String, PostingList> = HashMap::new();

        for (rel_path, frags) in &tree.fragments {
            let abs_path = repo_root.join(rel_path);
            let content = match std::fs::read_to_string(&abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (frag_idx, frag) in frags.iter().enumerate() {
                let start = frag.byte_start.min(content.len());
                let end = frag.byte_end.min(content.len());
                let text = &content[start..end];

                let entry_idx = entries.len();
                let (term_freqs, doc_len) = tokenize_tf(text);

                // Also index symbol names with a frequency boost
                let mut boosted = term_freqs;
                for sym in &frag.symbols {
                    let name_lower = sym.name.to_lowercase();
                    *boosted.entry(name_lower).or_insert(0) += 5; // 5× boost for symbol names
                    // Index individual camelCase / snake_case parts too
                    for part in split_identifier(&sym.name) {
                        *boosted.entry(part).or_insert(0) += 2;
                    }
                }
                // Index the path components (directory names, filename stem)
                for part in path_tokens(rel_path) {
                    *boosted.entry(part).or_insert(0) += 3;
                }

                for (term, tf) in &boosted {
                    postings
                        .entry(term.clone())
                        .or_default()
                        .push((entry_idx, *tf));
                }

                entries.push(FragmentEntry {
                    path: rel_path.clone(),
                    fragment_idx: frag_idx,
                    byte_start: frag.byte_start,
                    byte_end: frag.byte_end,
                    doc_len,
                });
            }
        }

        let avg_doc_len = if entries.is_empty() {
            1.0
        } else {
            entries.iter().map(|e| e.doc_len as f32).sum::<f32>() / entries.len() as f32
        };

        Self {
            entries,
            postings,
            avg_doc_len,
        }
    }

    /// Update the index incrementally when files are upserted.
    pub fn upsert_files(&mut self, repo_root: &Path, rel_paths: &[PathBuf], tree: &MerkleTree) {
        // Remove old entries for these paths
        self.remove_paths(rel_paths);

        // Re-add them
        for rel_path in rel_paths {
            let Some(frags) = tree.fragments.get(rel_path) else {
                continue;
            };
            let abs_path = repo_root.join(rel_path);
            let content = match std::fs::read_to_string(&abs_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (frag_idx, frag) in frags.iter().enumerate() {
                let start = frag.byte_start.min(content.len());
                let end = frag.byte_end.min(content.len());
                let text = &content[start..end];

                let entry_idx = self.entries.len();
                let (mut term_freqs, doc_len) = tokenize_tf(text);
                for sym in &frag.symbols {
                    let name_lower = sym.name.to_lowercase();
                    *term_freqs.entry(name_lower).or_insert(0) += 5;
                    for part in split_identifier(&sym.name) {
                        *term_freqs.entry(part).or_insert(0) += 2;
                    }
                }
                for part in path_tokens(rel_path) {
                    *term_freqs.entry(part).or_insert(0) += 3;
                }

                for (term, tf) in &term_freqs {
                    self.postings
                        .entry(term.clone())
                        .or_default()
                        .push((entry_idx, *tf));
                }

                self.entries.push(FragmentEntry {
                    path: rel_path.clone(),
                    fragment_idx: frag_idx,
                    byte_start: frag.byte_start,
                    byte_end: frag.byte_end,
                    doc_len,
                });
            }
        }

        self.recalc_avg_doc_len();
    }

    /// Remove all index entries for the given paths.
    pub fn remove_paths(&mut self, rel_paths: &[PathBuf]) {
        let remove_set: std::collections::HashSet<&PathBuf> = rel_paths.iter().collect();
        // Mark entries belonging to these paths as dead (set doc_len to u32::MAX as tombstone)
        for entry in self.entries.iter_mut() {
            if remove_set.contains(&entry.path) {
                entry.doc_len = u32::MAX; // tombstone
            }
        }
        // Remove from postings (lazy: filter out tombstones during search, compact periodically)
    }

    /// BM25 search. Returns up to `top_k` hits sorted by score descending.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchHit> {
        let query_lower = query.to_lowercase();
        let tokens: Vec<String> = tokenize_text(&query_lower);
        if tokens.is_empty() {
            return vec![];
        }

        let num_docs = self
            .entries
            .iter()
            .filter(|e| e.doc_len != u32::MAX)
            .count() as f32;
        if num_docs == 0.0 {
            return vec![];
        }

        let mut scores: HashMap<usize, f32> = HashMap::new();

        for token in &tokens {
            let Some(postings) = self.postings.get(token.as_str()) else {
                continue;
            };

            // IDF component
            let df = postings
                .iter()
                .filter(|(idx, _)| self.entries[*idx].doc_len != u32::MAX)
                .count() as f32;
            if df == 0.0 {
                continue;
            }
            let idf = ((num_docs - df + 0.5) / (df + 0.5) + 1.0).ln();

            for &(entry_idx, tf) in postings {
                let entry = &self.entries[entry_idx];
                if entry.doc_len == u32::MAX {
                    continue; // tombstone
                }
                let dl = entry.doc_len as f32;
                let tf_norm = (tf as f32 * (BM25_K1 + 1.0))
                    / (tf as f32 + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avg_doc_len));
                *scores.entry(entry_idx).or_insert(0.0) += idf * tf_norm;
            }
        }

        // Collect, sort, return top_k
        let mut ranked: Vec<(usize, f32)> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        ranked
            .into_iter()
            .take(top_k)
            .map(|(idx, score)| {
                let entry = &self.entries[idx];
                SearchHit {
                    path: entry.path.clone(),
                    fragment_idx: entry.fragment_idx,
                    byte_start: entry.byte_start,
                    byte_end: entry.byte_end,
                    score,
                    matched_symbol: None,
                }
            })
            .collect()
    }

    /// Look up fragments that define a symbol with the given name (case-insensitive prefix match).
    pub fn symbol_lookup<'a>(
        &'a self,
        name: &str,
        tree: &'a MerkleTree,
        top_k: usize,
    ) -> Vec<SearchHit> {
        let name_lower = name.to_lowercase();
        let mut hits = Vec::new();

        for (rel_path, frags) in &tree.fragments {
            for (frag_idx, frag) in frags.iter().enumerate() {
                for sym in &frag.symbols {
                    if sym.name.to_lowercase().starts_with(&name_lower) {
                        hits.push(SearchHit {
                            path: rel_path.clone(),
                            fragment_idx: frag_idx,
                            byte_start: frag.byte_start,
                            byte_end: frag.byte_end,
                            score: if sym.name.to_lowercase() == name_lower {
                                10.0
                            } else {
                                5.0
                            },
                            matched_symbol: Some(sym.name.clone()),
                        });
                        if hits.len() >= top_k {
                            return hits;
                        }
                    }
                }
            }
        }
        hits
    }

    fn recalc_avg_doc_len(&mut self) {
        let live: Vec<&FragmentEntry> = self
            .entries
            .iter()
            .filter(|e| e.doc_len != u32::MAX)
            .collect();
        self.avg_doc_len = if live.is_empty() {
            1.0
        } else {
            live.iter().map(|e| e.doc_len as f32).sum::<f32>() / live.len() as f32
        };
    }
}

// ── Tokenization helpers ──────────────────────────────────────────────────────

/// A token is worth indexing if it has 2+ chars, or if it is a single
/// alphabetic character (type parameters like `T`, `E`, `K`, short loop
/// variables like `i`, `n`). Single digits and lone underscores are dropped
/// because they are pure noise with no search value.
#[inline]
fn is_indexable(t: &str) -> bool {
    match t.len() {
        0 => false,
        1 => t.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false),
        _ => true,
    }
}

/// Tokenize text and return (term → frequency, total_token_count).
fn tokenize_tf(text: &str) -> (HashMap<String, u32>, u32) {
    let mut map: HashMap<String, u32> = HashMap::new();
    let mut total = 0u32;
    for token in tokenize_text(&text.to_lowercase()) {
        *map.entry(token).or_insert(0) += 1;
        total += 1;
    }
    (map, total.max(1))
}

/// Tokenize text on non-alphanumeric boundaries.
fn tokenize_text(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| is_indexable(t))
        .map(|t| t.to_lowercase())
        .collect()
}

/// Split a camelCase or snake_case identifier into lowercase parts.
fn split_identifier(name: &str) -> Vec<String> {
    if name.is_empty() {
        return vec![];
    }
    // First split on underscores
    let snake_parts: Vec<&str> = name.split('_').filter(|s| !s.is_empty()).collect();
    let mut parts: Vec<String> = Vec::new();
    for part in snake_parts {
        // Then split on camelCase
        let mut current = String::new();
        for ch in part.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.to_lowercase());
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current.to_lowercase());
        }
    }
    parts.into_iter().filter(|p| is_indexable(p)).collect()
}

/// Extract lowercase tokens from a path (directory names + file stem).
fn path_tokens(path: &Path) -> Vec<String> {
    let mut tokens = Vec::new();
    for component in path.components() {
        let s = component.as_os_str().to_string_lossy().to_lowercase();
        // Strip extension for the final component
        let stem = if let Some(dot) = s.rfind('.') {
            &s[..dot]
        } else {
            &s
        };
        for part in stem.split(|c: char| !c.is_alphanumeric()) {
            if is_indexable(part) {
                tokens.push(part.to_string());
            }
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::FileTreeWalker;
    use tempfile::tempdir;

    #[test]
    fn bm25_finds_function() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn greet_user() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("util.rs"),
            "fn format_date(ts: i64) -> String {\n    ts.to_string()\n}\n",
        )
        .unwrap();

        let tree = FileTreeWalker::build_tree(dir.path(), 100).unwrap();
        let index = InvertedIndex::build(dir.path(), &tree);

        let hits = index.search("greet_user", 10);
        assert!(!hits.is_empty(), "expected to find greet_user");
        assert_eq!(
            hits[0].path,
            PathBuf::from("main.rs"),
            "top hit should be main.rs"
        );
    }

    #[test]
    fn symbol_lookup_finds_definition() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("server.rs"),
            "pub struct HttpServer {\n    port: u16,\n}\n",
        )
        .unwrap();

        let tree = FileTreeWalker::build_tree(dir.path(), 100).unwrap();
        let index = InvertedIndex::build(dir.path(), &tree);

        let hits = index.symbol_lookup("HttpServer", &tree, 5);
        assert!(!hits.is_empty(), "expected HttpServer definition");
        assert_eq!(hits[0].matched_symbol.as_deref(), Some("HttpServer"));
    }

    #[test]
    fn split_identifier_handles_camel_and_snake() {
        assert_eq!(split_identifier("greetUser"), vec!["greet", "user"]);
        assert_eq!(split_identifier("format_date"), vec!["format", "date"]);
        assert_eq!(split_identifier("HttpServer"), vec!["http", "server"]);
    }

    #[test]
    fn single_char_alpha_tokens_are_indexed_and_searchable() {
        let dir = tempdir().unwrap();
        // T is a generic type parameter — common in Rust/Go/TypeScript
        std::fs::write(
            dir.path().join("generic.rs"),
            "pub fn identity<T>(x: T) -> T { x }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("other.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();

        let tree = FileTreeWalker::build_tree(dir.path(), 100).unwrap();
        let index = InvertedIndex::build(dir.path(), &tree);

        // Searching for the single-char token "t" should find generic.rs
        let hits = index.search("T", 10);
        assert!(!hits.is_empty(), "single-char token 'T' should be findable");
        assert!(
            hits.iter().any(|h| h.path == PathBuf::from("generic.rs")),
            "generic.rs should be in results for 'T'"
        );

        // Single digits should NOT be indexed (they are noise)
        assert!(is_indexable("a"), "single alpha char should be indexable");
        assert!(!is_indexable("1"), "single digit should not be indexable");
        assert!(
            !is_indexable("_"),
            "lone underscore should not be indexable"
        );
        assert!(!is_indexable(""), "empty string should not be indexable");
    }

    #[test]
    fn bm25_ranks_more_relevant_fragment_higher() {
        let dir = tempdir().unwrap();
        // a.rs mentions "config" once; b.rs defines a Config struct and uses it 3 times
        std::fs::write(dir.path().join("a.rs"), "fn load_config() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "struct Config { host: String, port: u16, timeout: u32 }\nfn apply_config(c: Config) {}\nfn read_config() -> Config {}\n").unwrap();

        let tree = FileTreeWalker::build_tree(dir.path(), 100).unwrap();
        let index = InvertedIndex::build(dir.path(), &tree);

        let hits = index.search("config", 10);
        assert!(hits.len() >= 2);
        // b.rs should rank higher because it has more config-related content
        let top_path = &hits[0].path;
        assert_eq!(top_path.as_os_str(), "b.rs", "b.rs should be ranked higher");
    }
}
