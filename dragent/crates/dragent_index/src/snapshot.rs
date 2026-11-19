use super::merkle::MerkleTree;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub struct IndexSnapshot {
    pub schema_version: u32,
    pub root_hash: String,
    pub tree: MerkleTree,
}

impl IndexSnapshot {
    // Bump when Fragment or MerkleTree schema changes (forces a rebuild on next startup).
    const CURRENT_SCHEMA_VERSION: u32 = 2;

    pub fn save(tree: &MerkleTree, path: &Path) -> Result<(), anyhow::Error> {
        let snapshot = IndexSnapshot {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            root_hash: tree.root_hash.clone(),
            tree: tree.clone(),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let bytes = bincode::serialize(&snapshot)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<MerkleTree, anyhow::Error> {
        let bytes = std::fs::read(path)?;
        let snapshot: IndexSnapshot = bincode::deserialize(&bytes)?;
        if snapshot.schema_version != Self::CURRENT_SCHEMA_VERSION {
            return Err(anyhow::anyhow!("Stale schema version"));
        }
        Ok(snapshot.tree)
    }
}
