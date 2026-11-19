//! Early persistence init — delegates to `dragent_persistence::ensure_initialized`.

use dragent_persistence::{
    shared_writer, warm_in_background, PersistedSession, WriterHandles,
};
use warpui::fonts::FamilyId;
use warpui_core::fonts::Cache as FontCache;

pub use dragent_persistence::warm_in_background as warm_persistence;

/// App chrome uses a readable proportional UI face.
pub fn load_ui_font(cache: &mut FontCache) -> FamilyId {
    crate::fonts::ui_family(cache)
}

pub fn load_mono_font(cache: &mut FontCache) -> FamilyId {
    crate::fonts::code_family(cache)
}

/// Session + writer — blocks only until the single-flight init finishes.
pub fn load_persistence() -> (Option<PersistedSession>, WriterHandles) {
    // One recovery per organisation selection: every window of a launch or
    // switch shares it (recovery recreates missing worktrees, so it must not
    // repeat per window), and the next switch loads that organisation's own,
    // latest save. A process-wide cache here restored the first account's
    // workspaces into every organisation switched to afterwards.
    type Selection = (String, u64);
    static RECOVERED: std::sync::Mutex<Option<(Selection, Option<PersistedSession>)>> =
        std::sync::Mutex::new(None);
    let selection = (
        dragent_persistence::current_org(),
        dragent_paths::organisation_selection(),
    );
    let mut recovered = RECOVERED.lock().unwrap_or_else(|p| p.into_inner());
    let session = match recovered.as_ref() {
        Some((loaded, session)) if *loaded == selection => session.clone(),
        _ => {
            let (saved, local_workspaces) = dragent_persistence::current_state();
            let session =
                super::worktree_recovery::recover_workspace_checkouts(saved, &local_workspaces);
            *recovered = Some((selection, session.clone()));
            session
        }
    };
    (session, shared_writer())
}
