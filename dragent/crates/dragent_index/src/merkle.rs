use crate::symbols::Symbol;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum MerkleNodeType {
    FileLeaf,
    Directory,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MerkleNode {
    pub hash: String,
    pub node_type: MerkleNodeType,
    /// Path relative to the repo root.
    pub relative_path: PathBuf,
    /// Children sorted by relative_path name.
    pub children: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fragment {
    pub fragment_id: String,
    pub path: PathBuf,
    pub byte_start: usize,
    pub byte_end: usize,
    pub content_hash: String,
    /// Symbols defined within this fragment (functions, classes, structs, etc.).
    #[serde(default)]
    pub symbols: Vec<Symbol>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MerkleTree {
    pub root_hash: String,
    pub nodes: BTreeMap<String, MerkleNode>,
    pub fragments: BTreeMap<PathBuf, Vec<Fragment>>,
}

impl MerkleTree {
    pub fn new() -> Self {
        let root_hash = Self::hash_internal(MerkleNodeType::Directory, &PathBuf::from(""), &[]);
        let mut nodes = BTreeMap::new();
        nodes.insert(
            root_hash.clone(),
            MerkleNode {
                hash: root_hash.clone(),
                node_type: MerkleNodeType::Directory,
                relative_path: PathBuf::from(""),
                children: Vec::new(),
            },
        );

        Self {
            root_hash,
            nodes,
            fragments: BTreeMap::new(),
        }
    }

    pub fn hash_internal(
        node_type: MerkleNodeType,
        path: &Path,
        child_hashes: &[String],
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(match node_type {
            MerkleNodeType::FileLeaf => b"file\0" as &[u8],
            MerkleNodeType::Directory => b"dir\0" as &[u8],
        });
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        for child in child_hashes {
            hasher.update(child.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    pub fn hash_leaf(path: &Path, fragments: &[Fragment]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"leaf\0");
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        for f in fragments {
            hasher.update(f.fragment_id.as_bytes());
            hasher.update(f.content_hash.as_bytes());
            hasher.update(f.byte_start.to_be_bytes());
            hasher.update(f.byte_end.to_be_bytes());
        }
        hex::encode(hasher.finalize())
    }

    pub fn remove_files(&mut self, deletions: &[PathBuf]) -> usize {
        if deletions.is_empty() {
            return 0;
        }
        for path in deletions {
            self.fragments.remove(path);
        }
        self.recompute_ancestors(deletions)
    }

    pub fn upsert_files(
        &mut self,
        repo_root: &Path,
        upsertions: &[PathBuf],
    ) -> anyhow::Result<usize> {
        if upsertions.is_empty() {
            return Ok(0);
        }
        use crate::chunker::Chunker;
        for path in upsertions {
            let abs_path = repo_root.join(path);
            let data = match std::fs::read(&abs_path) {
                Ok(d) => d,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File was deleted before we could read it — normal race with
                    // git temp files or rapid deletes; not an indexing error.
                    log::debug!("Skipping deleted file {}: {}", abs_path.display(), e);
                    continue;
                }
                Err(e) => {
                    log::error!("Failed to read file {}: {}", abs_path.display(), e);
                    continue;
                }
            };

            if Chunker::is_binary(&data) {
                self.fragments.remove(path);
                continue;
            }

            if data.len() > 1024 * 1024 {
                // skip files > 1 MiB
                self.fragments.remove(path);
                continue;
            }

            let text = match std::str::from_utf8(&data) {
                Ok(s) => s,
                Err(_) => {
                    self.fragments.remove(path);
                    continue; // not valid utf-8
                }
            };

            let file_frags = Chunker::chunk(path, text);
            self.fragments.insert(path.clone(), file_frags);
        }
        let count = self.recompute_ancestors(upsertions);
        Ok(count)
    }

    pub fn recompute_nodes_and_root(&mut self) {
        let paths: Vec<PathBuf> = self.fragments.keys().cloned().collect();
        self.recompute_ancestors(&paths);
    }

    pub fn recompute_ancestors(&mut self, changed_paths: &[PathBuf]) -> usize {
        if changed_paths.is_empty() {
            return 0;
        }

        let mut ancestor_dirs = std::collections::HashSet::new();
        for path in changed_paths {
            let mut current = path.as_path();
            while let Some(parent) = current.parent() {
                if parent == Path::new("") || parent.to_string_lossy().is_empty() {
                    break;
                }
                ancestor_dirs.insert(parent.to_path_buf());
                current = parent;
            }
        }
        ancestor_dirs.insert(PathBuf::from(""));

        let mut path_to_node = std::collections::HashMap::new();
        for node in self.nodes.values() {
            path_to_node.insert(node.relative_path.clone(), node.clone());
        }

        for path in changed_paths {
            if let Some(file_frags) = self.fragments.get(path) {
                let leaf_hash = Self::hash_leaf(path, file_frags);
                path_to_node.insert(
                    path.clone(),
                    MerkleNode {
                        hash: leaf_hash,
                        node_type: MerkleNodeType::FileLeaf,
                        relative_path: path.clone(),
                        children: Vec::new(),
                    },
                );
            } else {
                path_to_node.remove(path);
            }
        }

        let mut sorted_dirs: Vec<PathBuf> = ancestor_dirs.into_iter().collect();
        sorted_dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));

        for dir_path in &sorted_dirs {
            let mut children_paths = Vec::new();
            for p in path_to_node.keys() {
                if p == dir_path {
                    continue;
                }
                let is_direct_child = if dir_path == Path::new("") {
                    p.parent().is_none() || p.parent() == Some(Path::new(""))
                } else {
                    p.parent() == Some(dir_path)
                };
                if is_direct_child {
                    children_paths.push(p.clone());
                }
            }

            children_paths.sort();

            let mut child_hashes = Vec::new();
            for child_path in children_paths {
                if let Some(child_node) = path_to_node.get(&child_path) {
                    child_hashes.push(child_node.hash.clone());
                }
            }

            if child_hashes.is_empty() && dir_path != Path::new("") {
                path_to_node.remove(dir_path);
                continue;
            }

            let dir_hash = Self::hash_internal(MerkleNodeType::Directory, dir_path, &child_hashes);
            path_to_node.insert(
                dir_path.clone(),
                MerkleNode {
                    hash: dir_hash,
                    node_type: MerkleNodeType::Directory,
                    relative_path: dir_path.clone(),
                    children: child_hashes,
                },
            );
        }

        let mut new_nodes = BTreeMap::new();
        for node in path_to_node.values() {
            new_nodes.insert(node.hash.clone(), node.clone());
        }
        self.nodes = new_nodes;

        if let Some(root_node) = path_to_node.get(Path::new("")) {
            self.root_hash = root_node.hash.clone();
        } else {
            self.root_hash = Self::hash_internal(MerkleNodeType::Directory, Path::new(""), &[]);
        }

        changed_paths.len() + sorted_dirs.len()
    }
}
