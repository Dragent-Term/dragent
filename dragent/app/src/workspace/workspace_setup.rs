//! Automatic init for new worktrees.
//!
//! After a worktree is created, this runs its repository setup in the
//! background when the config exists and the project is trusted, and
//! records the outcome for `dragent setup status`. Untrusted projects and
//! worktrees without a config are left alone.

use std::path::Path;

/// Run init in the background for a just-created worktree, when its
/// repository has a config and the project is trusted.
pub(crate) fn maybe_init_worktree(worktree: &Path) {
    // A remote worktree's folder here is an empty stand-in: its setup
    // would have to run on its machine, which this build doesn't do.
    if crate::remote_fs::is_remote(worktree) {
        return;
    }
    let worktree = worktree.to_path_buf();
    std::thread::spawn(move || {
        let main = dragent_workspace_setup::paths::main_checkout_of(&worktree);
        let store = super::trust_store::WorkspaceTrustStore::load();
        let trusted = store.is_trusted(&worktree)
            || main.as_deref().is_some_and(|main| store.is_trusted(main));
        if !trusted {
            return;
        }
        let config = match dragent_workspace_setup::config::load_merged(&worktree) {
            Ok(Some(config)) => config,
            _ => return,
        };
        let state_dir = dragent_paths::organisation_dir().join("workspace-setup");
        dragent_workspace_setup::orchestrate::run_init(
            &config,
            &worktree,
            main.as_deref(),
            Some(&state_dir),
        );
    });
}
