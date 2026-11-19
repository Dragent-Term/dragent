//! What Dragent stores about a project or worktree outside of it, so that
//! deleting one leaves nothing behind. The project's own folder is never
//! touched here.

use std::path::{Path, PathBuf};

use super::persistence::WriterHandles;

pub(crate) struct ProjectData {
    /// The root as each store may have spelled it: stores hash the path text.
    paths: Vec<PathBuf>,
    workspace_dir: PathBuf,
}

impl ProjectData {
    /// Resolve while `root` still exists: the canonical spelling several
    /// stores key by is unavailable once its folder is gone.
    pub(crate) fn locate(root: &Path) -> Self {
        let mut paths = vec![root.to_path_buf()];
        if let Ok(canonical) = root.canonicalize() {
            if let Some(plain) = canonical.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
                paths.push(PathBuf::from(plain));
            }
            paths.push(canonical);
        }
        paths.sort();
        paths.dedup();
        Self {
            paths,
            workspace_dir: dragent_paths::workspace(root),
        }
    }

    /// Delete the root's code index, workspace database, agent memory and
    /// agent conversations.
    pub(crate) fn forget(self, writer: &WriterHandles) {
        for path in &self.paths {
            dragent_index::get_manager().remove_index(path);
        }
        writer.forget_project_data(self.workspace_dir);
        let dirs: Vec<PathBuf> = self
            .paths
            .iter()
            .flat_map(|path| {
                [
                    crate::agent::memory_store::project_root(path),
                    dragent_agent::session_store::project_sessions_dir(path),
                ]
            })
            .collect();
        crate::bg::detach(move || {
            for dir in dirs {
                if let Err(error) = dragent_paths::remove_data_dir(&dir) {
                    log::warn!("[project-data] could not delete {}: {error}", dir.display());
                }
            }
        });
    }
}
