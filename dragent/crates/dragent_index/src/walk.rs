use super::chunker::Chunker;
use super::merkle::{MerkleNode, MerkleNodeType, MerkleTree};
use ignore::WalkBuilder;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FileTreeWalker;

/// Whether walks (and the watcher's event filter, which shares
/// `ignore_sources`) honor the repo's `.gitignore`. Settings-controlled;
/// takes effect on the next walk / watcher (re)start — the UI triggers a
/// re-index when it flips this.
static RESPECT_GITIGNORE: AtomicBool = AtomicBool::new(true);

pub fn set_respect_gitignore(respect: bool) {
    RESPECT_GITIGNORE.store(respect, Ordering::Relaxed);
}

pub fn respect_gitignore() -> bool {
    RESPECT_GITIGNORE.load(Ordering::Relaxed)
}

/// The user-global ignore file applied to every repo, editable from
/// Settings → Code and Index. One glob per line, gitignore syntax.
pub fn global_ignore_path() -> Option<PathBuf> {
    Some(dragent_paths::home().join("index.ignore"))
}

/// The ignore files that scope what gets indexed, in precedence order:
/// the repo's `.gitignore` (unless disabled in settings), the repo's
/// `.dragentignore` (same syntax — lets a team exclude private or
/// generated trees from semantic indexing without touching git), and a
/// user-global ignore file applied to every repo.
/// Both the initial walk and the file watcher consume this list so the two
/// paths can never disagree about what is indexable.
pub fn ignore_sources(repo_root: &Path) -> Vec<std::path::PathBuf> {
    let mut sources = Vec::new();
    if respect_gitignore() {
        sources.push(repo_root.join(".gitignore"));
    }
    sources.push(repo_root.join(".dragentignore"));
    if let Some(global) = global_ignore_path() {
        sources.push(global);
    }
    sources.retain(|p| p.is_file());
    sources
}

impl FileTreeWalker {
    /// Build a tree, filtering files with the provided `exclude` predicate.
    /// `exclude(rel_path, file_size_bytes)` returning `true` skips the file.
    pub fn build_tree_with_filter<F>(
        repo_root: &Path,
        max_files: usize,
        exclude: F,
    ) -> Result<MerkleTree, anyhow::Error>
    where
        F: Fn(&Path, u64) -> bool,
    {
        let mut walker = WalkBuilder::new(repo_root);
        walker.parents(false);
        walker.git_ignore(true);
        for ignore_file in ignore_sources(repo_root) {
            walker.add_ignore(ignore_file);
        }
        walker.hidden(true);

        let mut files_to_index = Vec::new();
        let mut file_count = 0;

        for entry in walker.build() {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if file_count >= max_files {
                    log::warn!(
                        "Reached max files limit ({}) during walk. Skipping remaining files.",
                        max_files
                    );
                    break;
                }

                if let Ok(rel) = path.strip_prefix(repo_root) {
                    let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    if !exclude(rel, file_size) {
                        files_to_index.push((rel.to_path_buf(), path.to_path_buf()));
                        file_count += 1;
                    }
                }
            }
        }

        Self::build_tree_from_files(repo_root, files_to_index)
    }

    pub fn build_tree(repo_root: &Path, max_files: usize) -> Result<MerkleTree, anyhow::Error> {
        Self::build_tree_with_filter(repo_root, max_files, |_, _| false)
    }

    pub fn build_tree_from_files(
        _repo_root: &Path,
        files: Vec<(PathBuf, PathBuf)>,
    ) -> Result<MerkleTree, anyhow::Error> {
        let mut fragments = BTreeMap::new();
        let mut leaf_nodes = HashMap::new();

        for (rel, abs) in files {
            // Read and chunk file
            let data = match std::fs::read(&abs) {
                Ok(d) => d,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    log::debug!("Skipping deleted file {}: {}", abs.display(), e);
                    continue;
                }
                Err(e) => {
                    log::error!("Failed to read file {}: {}", abs.display(), e);
                    continue;
                }
            };

            if Chunker::is_binary(&data) {
                continue;
            }

            if data.len() > 1024 * 1024 {
                // skip files > 1 MiB
                continue;
            }

            let text = match std::str::from_utf8(&data) {
                Ok(s) => s,
                Err(_) => continue, // not valid utf-8
            };

            // Skip minified files — any line > 2KiB indicates it's not readable source.
            if text.lines().any(|l| l.len() > 2048) {
                continue;
            }

            let file_frags = Chunker::chunk(&rel, text);
            let leaf_hash = MerkleTree::hash_leaf(&rel, &file_frags);

            leaf_nodes.insert(
                rel.clone(),
                MerkleNode {
                    hash: leaf_hash.clone(),
                    node_type: MerkleNodeType::FileLeaf,
                    relative_path: rel.clone(),
                    children: Vec::new(),
                },
            );

            fragments.insert(rel, file_frags);
        }

        // Now construct the Merkle tree from bottom up
        let mut tree_nodes = BTreeMap::new();

        // Group nodes by directory structure
        let mut dirs: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();

        // Ensure every path and all its ancestor directories are registered
        let mut all_paths = Vec::new();
        for path in leaf_nodes.keys() {
            all_paths.push(path.clone());
            let mut current = path.as_path();
            while let Some(parent) = current.parent() {
                if parent == Path::new("") {
                    break;
                }
                all_paths.push(parent.to_path_buf());
                current = parent;
            }
        }
        all_paths.sort();
        all_paths.dedup();

        // Map parents to children
        for path in &all_paths {
            if let Some(parent) = path.parent() {
                let parent_buf = parent.to_path_buf();
                dirs.entry(parent_buf).or_default().push(path.clone());
            }
        }

        // Process bottom-up by path depth (longest paths first)
        let mut sorted_paths = all_paths.clone();
        sorted_paths.sort_by_key(|p| std::cmp::Reverse(p.components().count()));

        for path in sorted_paths {
            if let Some(leaf) = leaf_nodes.get(&path) {
                tree_nodes.insert(leaf.hash.clone(), leaf.clone());
            } else {
                // Directory node
                let children_paths = dirs.entry(path.clone()).or_default();
                // Sort children paths to make hashing deterministic
                children_paths.sort();

                let mut child_hashes = Vec::new();
                for child_path in children_paths {
                    // Resolve child hash
                    let hash = if let Some(leaf) = leaf_nodes.get(child_path) {
                        leaf.hash.clone()
                    } else {
                        // Must be a directory we already processed because of bottom-up ordering
                        let mut found_hash = None;
                        for node in tree_nodes.values() {
                            if node.node_type == MerkleNodeType::Directory
                                && node.relative_path == *child_path
                            {
                                found_hash = Some(node.hash.clone());
                                break;
                            }
                        }
                        found_hash.unwrap_or_else(|| {
                            // Empty directory fallback hash
                            MerkleTree::hash_internal(MerkleNodeType::Directory, child_path, &[])
                        })
                    };
                    child_hashes.push(hash);
                }

                let dir_hash =
                    MerkleTree::hash_internal(MerkleNodeType::Directory, &path, &child_hashes);
                let dir_node = MerkleNode {
                    hash: dir_hash.clone(),
                    node_type: MerkleNodeType::Directory,
                    relative_path: path.clone(),
                    children: child_hashes,
                };
                tree_nodes.insert(dir_hash, dir_node);
            }
        }

        // Finally compute root node at root ""
        let mut root_children_paths = dirs.entry(PathBuf::from("")).or_default().clone();
        root_children_paths.sort();

        let mut root_child_hashes = Vec::new();
        for child_path in root_children_paths {
            let hash = if let Some(leaf) = leaf_nodes.get(&child_path) {
                leaf.hash.clone()
            } else {
                let mut found_hash = None;
                for node in tree_nodes.values() {
                    if node.node_type == MerkleNodeType::Directory
                        && node.relative_path == child_path
                    {
                        found_hash = Some(node.hash.clone());
                        break;
                    }
                }
                found_hash.expect("root child directory hash not found")
            };
            root_child_hashes.push(hash);
        }

        let root_hash =
            MerkleTree::hash_internal(MerkleNodeType::Directory, Path::new(""), &root_child_hashes);
        tree_nodes.insert(
            root_hash.clone(),
            MerkleNode {
                hash: root_hash.clone(),
                node_type: MerkleNodeType::Directory,
                relative_path: PathBuf::from(""),
                children: root_child_hashes,
            },
        );

        Ok(MerkleTree {
            root_hash,
            nodes: tree_nodes,
            fragments,
        })
    }
}
