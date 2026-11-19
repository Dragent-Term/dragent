// Pure data types (all targets). `ModelEvent` lives here too, so the wasm build
// gets the write-request type without pulling in diesel.
pub mod types;
pub use types::*;

// Native-only: the diesel/SQLite storage backend and the process-wide writer.
// On wasm, `wasm_stub` supplies this crate's public API as empty/no-op so the
// app's persistence call sites type-check without a real database.
#[cfg(not(target_family = "wasm"))]
pub mod database;
#[cfg(not(target_family = "wasm"))]
pub mod event_log;
#[cfg(not(target_family = "wasm"))]
pub mod host_verification;
#[cfg(not(target_family = "wasm"))]
pub mod schema;
#[cfg(not(target_family = "wasm"))]
pub mod session_store;
mod store_accept;
mod store_agent;
mod store_execution;
mod store_integration;
mod store_outcomes;
mod store_projects;
mod store_tests_accept;
mod store_tests_contracts;
mod store_tests_lifecycle;
mod store_tests_roles;
#[cfg(not(target_family = "wasm"))]
pub mod working_copy_store;
#[cfg(not(target_family = "wasm"))]
pub mod workspace_catalogue;
#[cfg(not(target_family = "wasm"))]
pub mod workspace_store;
#[cfg(not(target_family = "wasm"))]
pub mod writer;

#[cfg(target_family = "wasm")]
mod wasm_stub;
#[cfg(target_family = "wasm")]
pub use wasm_stub::*;

#[cfg(not(target_family = "wasm"))]
pub use native::*;

#[cfg(not(target_family = "wasm"))]
mod native {
    use std::sync::{Arc, Mutex, OnceLock};

    pub use crate::database::{
        current_org, database_path, database_path_for_root, db_file_for_root, db_path_for_read,
        db_path_for_write, ensure_repo_gitignore, establish_readonly, legacy_json_path,
        migrate_pane_conns, migrate_pane_to_root, project_root_for, restore_pane_blocks,
        sanitize_org_id, setup_connection, PROJECT_DIR_NAME, PROJECT_IGNORE_LINE,
    };
    pub use crate::event_log::{append, latest_seq, read_range, ConversationEvent};
    pub use crate::session_store::{
        clear_all_session_data, conversation_summaries_for_pane, copy_pane_blocks,
        delete_agent_blocks_for_pane, delete_blocks_for_pane, import_legacy_json,
        load_agent_blocks_for_pane, load_blocks_for_pane, load_conversation, load_session,
        load_window_frame, max_sort_order_for_pane, pane_has_rows, recent_commands,
        save_command_history, save_session_snapshot, save_window_frame,
    };
    pub use crate::types::PersistedSession;
    pub use crate::working_copy_store::{load_document_session, load_working_copies};
    pub use crate::workspace_store::{
        accept_local_post_message, accept_local_workspace_command,
        accept_local_workspace_commands_with_tool_outcome, begin_workspace_integration,
        block_claimed_workspace_integration, block_workspace_integration,
        claim_workspace_integration, load_canonical_workspace_events,
        load_pending_workspace_integration_intents, load_pending_workspace_tool_outcomes,
        load_recent_workspace_events, load_recent_workspace_work_items, load_workspace_project,
        mark_workspace_tool_outcome_delivered, park_workspace_tool_outcome,
        park_workspace_tool_outcomes_for_run, release_workspace_integration_claim,
        save_workspace_event, save_workspace_project, save_workspace_work_item,
    };
    pub use crate::writer::{
        start_writer, WriterConns, WriterHandles, MAX_BLOCKS_PER_PANE, MAX_CONVERSATIONS_PER_PANE,
    };

    pub const MIGRATIONS: diesel_migrations::EmbeddedMigrations =
        diesel_migrations::embed_migrations!("migrations");

    pub(crate) struct InitializedPersistence {
        session: Option<PersistedSession>,
        local_workspaces: Vec<crate::types::SerializedWorkspace>,
        window_frame: Option<crate::types::SerializedWindowFrame>,
        writer: WriterHandles,
        /// The session loaded at initialization was handed out; later
        /// requests read the database again.
        handed_out: std::sync::atomic::AtomicBool,
    }

    static INITIALIZED: OnceLock<
        Mutex<std::collections::HashMap<String, Arc<InitializedPersistence>>>,
    > = OnceLock::new();

    /// Single-flight SQLite open + migrations + writer thread. Safe to call from any thread;
    /// concurrent callers block until the first init completes (no duplicate opens).
    pub(crate) fn ensure_initialized() -> Arc<InitializedPersistence> {
        let organisation = current_org();
        let mut contexts = INITIALIZED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        contexts
            .entry(organisation)
            .or_insert_with(|| {
                let started = std::time::Instant::now();
                let (session, local_workspaces, window_frame, writer) = initialize()
                    .unwrap_or_else(|err| {
                        log::error!("failed to initialize persistence: {err:#}");
                        (None, Vec::new(), None, WriterHandles::noop())
                    });
                log::info!(
                    "persistence ready in {:.0}ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
                Arc::new(InitializedPersistence {
                    session,
                    local_workspaces,
                    window_frame,
                    writer,
                    handed_out: std::sync::atomic::AtomicBool::new(false),
                })
            })
            .clone()
    }

    /// Start DB init on a background thread as early as possible (call from `main`).
    pub fn warm_in_background() {
        std::thread::Builder::new()
            .name("dragent-persistence-init".into())
            .spawn(|| {
                let _ = ensure_initialized();
            })
            .ok();
    }

    pub fn loaded_session() -> Option<PersistedSession> {
        ensure_initialized().session.clone()
    }

    pub fn loaded_local_workspaces() -> Vec<crate::types::SerializedWorkspace> {
        ensure_initialized().local_workspaces.clone()
    }

    /// The selected organisation's saved session and local workspace
    /// catalogue. The first request per organisation gets what initialization
    /// loaded; later ones (returning after an organisation switch) re-read
    /// the database, so they show what was saved since, never the state the
    /// organisation was in when the app launched.
    pub fn current_state() -> (
        Option<PersistedSession>,
        Vec<crate::types::SerializedWorkspace>,
    ) {
        let initialized = ensure_initialized();
        if !initialized
            .handed_out
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return (
                initialized.session.clone(),
                initialized.local_workspaces.clone(),
            );
        }
        initialized.writer.flush();
        let read = || -> anyhow::Result<_> {
            let mut conn = setup_connection(&database_path()?)?;
            let session = load_session(&mut conn)?;
            let local_workspaces = crate::workspace_catalogue::load_local_workspaces(&mut conn)?;
            Ok((session, local_workspaces))
        };
        read().unwrap_or_else(|err| {
            // Never restore an empty workspace over saved data because a
            // read failed: fall back to this organisation's launch state.
            log::error!("re-reading the saved session failed: {err:#}");
            (
                initialized.session.clone(),
                initialized.local_workspaces.clone(),
            )
        })
    }

    /// Read only the small geometry record. Window creation must not wait for
    /// migrations, transcript hydration, or the persistence initialization lock.
    /// A locked/legacy database uses the default frame rather than stalling UI.
    pub fn launch_window_frame() -> Option<crate::types::SerializedWindowFrame> {
        use diesel::Connection;
        let path = database_path().ok()?;
        if !path.is_file() {
            return None;
        }
        let mut conn = diesel::SqliteConnection::establish(path.to_str()?).ok()?;
        use diesel::connection::SimpleConnection;
        conn.batch_execute("PRAGMA busy_timeout = 0; PRAGMA query_only = ON;").ok()?;
        load_window_frame(&mut conn).ok().flatten()
    }

    /// Main window geometry saved by the previous run, if any.
    pub fn loaded_window_frame() -> Option<crate::types::SerializedWindowFrame> {
        ensure_initialized().window_frame
    }

    /// Writer handle for the process lifetime (sender clone; join handle owned by static).
    pub fn shared_writer() -> WriterHandles {
        ensure_initialized().writer.clone_for_workspace()
    }

    /// Flush all writes through the process-wide writer before shutdown.
    /// Terminal transcripts are recovery data and must survive normal exit.
    pub fn flush_session_on_close() {
        shared_writer().flush();
    }

    pub fn initialize() -> anyhow::Result<(
        Option<PersistedSession>,
        Vec<crate::types::SerializedWorkspace>,
        Option<crate::types::SerializedWindowFrame>,
        WriterHandles,
    )> {
        let path = database_path()?;
        let mut conn = setup_connection(&path)?;

        let mut persisted = load_session(&mut conn)?;
        if persisted.is_none() {
            if let Ok(legacy_path) = legacy_json_path() {
                if legacy_path.exists() {
                    if let Ok(json) = std::fs::read_to_string(&legacy_path) {
                        if import_legacy_json(&mut conn, &json).is_ok() {
                            let _ = std::fs::rename(
                                &legacy_path,
                                legacy_path.with_extension("json.bak"),
                            );
                            log::info!("migrated session.json to sqlite");
                            persisted = load_session(&mut conn)?;
                        }
                    }
                }
            }
        }

        if let Some(saved) = &persisted {
            crate::workspace_catalogue::record_document_workspaces(
                &mut conn,
                &saved.session.document,
            )?;
        }
        let local_workspaces = crate::workspace_catalogue::load_local_workspaces(&mut conn)?;
        let window_frame = load_window_frame(&mut conn).unwrap_or_else(|err| {
            log::warn!("ignoring unreadable window frame: {err:#}");
            None
        });
        let writer = start_writer(path)?;
        Ok((persisted, local_workspaces, window_frame, writer))
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn empty_temporary_database_loads_without_touching_user_state() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("persistence-load.sqlite3");
            let mut connection = super::setup_connection(&path).unwrap();
            assert!(super::load_session(&mut connection).unwrap().is_none());
            let writer = super::start_writer(path).unwrap();
            writer.flush();
        }
    }
}
