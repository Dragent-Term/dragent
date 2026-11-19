use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use diesel::connection::SimpleConnection;
use diesel::sqlite::SqliteConnection;
use diesel::Connection;
use diesel_migrations::MigrationHarness;

use crate::MIGRATIONS;

const DB_FILE_NAME: &str = "dragent.sqlite";
/// Environment override for the workspace database path. Lets a dev build
/// run side-by-side with the released app (which must stay on the default
/// path): two processes sharing one SQLite file fight over write locks and
/// surface "database is locked" on ordinary commands like role approval.
/// Auth, settings, and sessions intentionally stay shared — only the
/// workspace authority database is isolated.
const DB_PATH_OVERRIDE_ENV: &str = "DRAGENT_WORKSPACE_DB";

/// Legacy project-local database directory, read only for migration.
pub const PROJECT_DIR_NAME: &str = ".dragent";
/// Legacy gitignore line; new databases never live in projects.
pub const PROJECT_IGNORE_LINE: &str = ".dragent/";

pub fn database_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(DB_PATH_OVERRIDE_ENV).map(PathBuf::from) {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
        }
        return Ok(path);
    }
    Ok(global_path()?)
}

fn global_path() -> Result<PathBuf> {
    let mut path = dragent_paths::organisation_dir();
    std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
    path.push(DB_FILE_NAME);
    Ok(path)
}

/// Legacy global path without creating anything (read path resolution).
fn global_path_no_create() -> Result<PathBuf> {
    let mut path = dragent_paths::organisation_dir();
    path.push(DB_FILE_NAME);
    Ok(path)
}

fn db_override_path() -> Option<PathBuf> {
    std::env::var_os(DB_PATH_OVERRIDE_ENV).map(PathBuf::from)
}

/// Filesystem-safe org segment (mirrors `dragent_host::worktrees::sanitize_segment`).
pub fn sanitize_org_id(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if (ch == '_' || ch == '.') && !out.is_empty() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') || out.ends_with('.') {
        out.pop();
    }
    if out.chars().count() > 48 {
        out = out.chars().take(48).collect();
        while out.ends_with('-') || out.ends_with('.') {
            out.pop();
        }
    }
    if out.is_empty() {
        out.push_str("local");
    }
    out
}

/// Exact current organisation ID; defaults to `local`.
pub fn current_org() -> String {
    dragent_paths::organisation_id()
}

/// Per-project organisation database; exact IDs use collision-resistant keys.
pub fn db_file_for_root(root: &Path, org: &str) -> PathBuf {
    let name = if org == "local" {
        "local".to_owned()
    } else {
        dragent_paths::organisation_key(org)
    };
    dragent_paths::workspace(root).join(format!("{name}.sqlite"))
}

/// Move all old project-local runtime files on first access. User-authored
/// files in `.dragent` are left in place.
fn migrate_project_database(root: &Path) -> Result<()> {
    dragent_paths::with_migration_lock(|| migrate_project_database_inner(root))?;
    Ok(())
}

fn migrate_project_database_inner(root: &Path) -> std::io::Result<()> {
    let old_dir = root.join(PROJECT_DIR_NAME);
    if !old_dir.is_dir() || is_dragent_home(&old_dir) {
        return Ok(());
    }
    let new_dir = dragent_paths::workspace(root);
    for entry in std::fs::read_dir(&old_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.ends_with(".sqlite")
            || name_text.ends_with(".sqlite-wal")
            || name_text.ends_with(".sqlite-shm")
            || name_text.ends_with(".sqlite-journal")
            || name_text == "checkpoints"
        {
            dragent_paths::move_preserving_existing(&entry.path(), &new_dir.join(&name))?;
            dragent_paths::preserve_conflict(
                &entry.path(),
                &new_dir.join("legacy-conflicts").join(&name),
            )?;
        }
    }
    let _ = std::fs::remove_dir(&old_dir);
    Ok(())
}

/// A project rooted at the user's home directory has Dragent's own data
/// directory as its `.dragent`; its live databases are not legacy project files.
fn is_dragent_home(dir: &Path) -> bool {
    let home = dragent_paths::home();
    match (dir.canonicalize(), home.canonicalize()) {
        (Ok(dir), Ok(home)) => dir == home,
        _ => dir == home,
    }
}

/// Override wins; otherwise use the per-project directory under ~/.dragent.
pub fn database_path_for_root(root: &Path) -> Result<PathBuf> {
    database_path_for_root_in_organisation(root, &current_org())
}

pub(crate) fn database_path_for_root_in_organisation(
    root: &Path,
    organisation: &str,
) -> Result<PathBuf> {
    if let Some(path) = db_override_path() {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
        }
        return Ok(path);
    }
    migrate_project_database(root)?;
    let dir = dragent_paths::workspace(root);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(db_file_for_root(root, organisation))
}

/// Walk up for a `.git` file-or-dir. File inputs start at their parent;
/// directory inputs start at themselves. Returns the repo root.
pub fn project_root_for(path: &Path) -> Option<PathBuf> {
    let start: &Path = if path.is_dir() {
        path
    } else if path.exists() {
        path.parent().unwrap_or(path)
    } else {
        match (path.parent(), path.extension()) {
            (Some(parent), Some(_)) => parent,
            _ => path,
        }
    };
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").symlink_metadata().is_ok() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Read resolution: override, else the existing per-root file, else the
/// legacy global path. Creates nothing.
pub fn db_path_for_read(root: &Path) -> Result<PathBuf> {
    if let Some(path) = db_override_path() {
        return Ok(path);
    }
    let probe = project_root_for(root).unwrap_or_else(|| root.to_path_buf());
    migrate_project_database(&probe)?;
    let candidate = db_file_for_root(&probe, &current_org());
    if candidate.exists() {
        return Ok(candidate);
    }
    Ok(global_path_no_create()?)
}

/// Write resolution: override → project data under ~/.dragent; projectless
/// workspaces use the global database in ~/.dragent.
pub fn db_path_for_write(root: &Path) -> Result<PathBuf> {
    if db_override_path().is_some() {
        return database_path();
    }
    match project_root_for(root) {
        Some(probe) => database_path_for_root(&probe),
        None => database_path(),
    }
}

/// Retained for API compatibility; Dragent no longer edits project gitignore.
pub fn ensure_repo_gitignore(_root: &Path) -> Result<()> {
    Ok(())
}

/// Copy-once pane migration between two open connections: skips when the
/// target already has rows for `pane`; otherwise transactionally copies via
/// `copy_pane_blocks`. Returns rows copied.
pub fn migrate_pane_conns(
    legacy_conn: &mut SqliteConnection,
    target_conn: &mut SqliteConnection,
    pane: uuid::Uuid,
) -> Result<usize> {
    if crate::session_store::pane_has_rows(target_conn, pane)? {
        return Ok(0);
    }
    if !crate::session_store::pane_has_rows(legacy_conn, pane)? {
        return Ok(0);
    }
    crate::session_store::copy_pane_blocks(legacy_conn, target_conn, pane)
}

/// Migrate one pane's blocks from the real legacy global file (even under
/// `DRAGENT_WORKSPACE_DB`) into the per-root file for `root`/`org`.
pub fn migrate_pane_to_root(root: &Path, org: &str, pane: uuid::Uuid) -> Result<usize> {
    let legacy_path = dragent_paths::organisation_dir_for(org).join(DB_FILE_NAME);
    if !legacy_path.exists() {
        return Ok(0);
    }
    let probe = project_root_for(root).unwrap_or_else(|| root.to_path_buf());
    migrate_project_database(&probe)?;
    let dir = dragent_paths::workspace(&probe);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let target_path = db_file_for_root(&probe, org);
    if legacy_path == target_path {
        return Ok(0);
    }
    let mut legacy_conn = setup_connection_in_organisation(&legacy_path, org)?;
    let mut target_conn = setup_connection_in_organisation(&target_path, org)?;
    migrate_pane_conns(&mut legacy_conn, &mut target_conn, pane)
}

/// Migrate then read one pane's blocks from its per-root database.
/// Returns `None` when both block lists are empty so callers fall back.
pub fn restore_pane_blocks(
    cwd: &Path,
    org: &str,
    pane: uuid::Uuid,
) -> Option<(
    Vec<crate::types::PersistedBlock>,
    Vec<crate::types::PersistedAgentBlock>,
)> {
    let _ = migrate_pane_to_root(cwd, org, pane);
    let probe = project_root_for(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let path = if let Some(override_path) = db_override_path() {
        override_path
    } else {
        let candidate = db_file_for_root(&probe, org);
        if candidate.exists() {
            candidate
        } else {
            let legacy = dragent_paths::organisation_dir_for(org).join(DB_FILE_NAME);
            if legacy.exists() {
                legacy
            } else {
                return None;
            }
        }
    };
    let mut conn = setup_connection_in_organisation(&path, org).ok()?;
    if connection_organisation(&mut conn).ok()?.as_str() != org {
        return None;
    }
    let blocks = crate::session_store::load_blocks_for_pane(&mut conn, pane).ok()?;
    let agent_blocks = crate::session_store::load_agent_blocks_for_pane(&mut conn, pane).ok()?;
    if blocks.is_empty() && agent_blocks.is_empty() {
        return None;
    }
    Some((blocks, agent_blocks))
}

pub fn legacy_json_path() -> Result<PathBuf> {
    let mut path = dragent_paths::organisation_dir();
    path.push("session.json");
    Ok(path)
}

pub fn setup_connection(path: &PathBuf) -> Result<SqliteConnection> {
    setup_connection_in_organisation(path, &current_org())
}

pub(crate) fn setup_connection_in_organisation(
    path: &PathBuf,
    organisation: &str,
) -> Result<SqliteConnection> {
    let url = path
        .to_str()
        .with_context(|| format!("invalid database path {}", path.display()))?;
    let mut conn = SqliteConnection::establish(url)
        .with_context(|| format!("connect to sqlite at {}", path.display()))?;
    conn.batch_execute(
        "PRAGMA foreign_keys = ON;
         -- Generous busy timeout: brief cross-process contention (e.g. a
         -- second app instance briefly touching the same database) retries
         -- instead of surfacing SQLITE_BUSY to the user. Sustained
         -- contention still fails fast enough to notice.
         PRAGMA busy_timeout = 10000;
         PRAGMA journal_mode = WAL;
         -- A successful commit must survive a machine crash, not only an
         -- application crash. NORMAL can lose committed workspace snapshots
         -- when power is lost before the next WAL checkpoint.
         PRAGMA synchronous = FULL;
         PRAGMA wal_autocheckpoint = 256;",
    )
    .context("sqlite pragmas")?;
    conn.run_pending_migrations(MIGRATIONS)
        .map_err(|e| anyhow::anyhow!("run sqlite migrations: {e}"))?;
    use diesel::RunQueryDsl;
    diesel::sql_query(
        "INSERT OR IGNORE INTO organisation_context (id, organisation_id) VALUES (1, ?1)",
    )
    .bind::<diesel::sql_types::Text, _>(organisation)
    .execute(&mut conn)?;
    Ok(conn)
}

/// A connection remains owned by its original organisation even when another
/// context becomes active. Background writers must not follow UI selection.
pub(crate) fn connection_organisation(conn: &mut SqliteConnection) -> Result<String> {
    use diesel::RunQueryDsl;
    #[derive(diesel::QueryableByName)]
    struct Owner {
        #[diesel(sql_type = diesel::sql_types::Text)]
        organisation_id: String,
    }
    Ok(
        diesel::sql_query("SELECT organisation_id FROM organisation_context WHERE id = 1")
            .get_result::<Owner>(conn)?
            .organisation_id,
    )
}

pub fn establish_readonly(path: &PathBuf) -> Result<SqliteConnection> {
    setup_connection(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn workspace_db_path_override_isolates_dev_builds() {
        let _guard = env_lock().lock().unwrap();
        let dir = std::env::temp_dir().join(format!("dragent-db-override-{}", std::process::id()));
        let custom = dir.join("nested").join("dev.sqlite");
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::set_var(DB_PATH_OVERRIDE_ENV, &custom);
        let resolved = database_path().expect("override resolves");
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
        assert_eq!(resolved, custom);
        assert!(custom.parent().unwrap().is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_file_for_root_uses_private_workspace_directory() {
        // Reads DRAGENT_HOME, which other tests change under this lock.
        let _guard = env_lock().lock().unwrap();
        let root = Path::new("/projects/acme");
        assert_eq!(
            db_file_for_root(root, "local"),
            dragent_paths::workspace(root).join("local.sqlite")
        );
        assert_eq!(
            db_file_for_root(root, "My Org!"),
            dragent_paths::workspace(root).join(format!(
                "{}.sqlite",
                dragent_paths::organisation_key("My Org!")
            ))
        );
    }

    #[test]
    fn sanitize_org_id_mirrors_host_rule() {
        assert_eq!(sanitize_org_id("Acme Corp!"), "acme-corp");
        assert_eq!(sanitize_org_id(""), "local");
        assert_eq!(sanitize_org_id("---"), "local");
        assert_eq!(sanitize_org_id("a_b.c"), "a_b.c");
    }

    #[test]
    fn a_project_at_the_home_directory_never_moves_dragents_own_databases() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let user_home = dir.path().join("user");
        let app_home = user_home.join(PROJECT_DIR_NAME);
        std::fs::create_dir_all(&app_home).unwrap();
        std::fs::write(app_home.join(DB_FILE_NAME), b"live session").unwrap();
        let previous_home = std::env::var_os("DRAGENT_HOME");
        std::env::set_var("DRAGENT_HOME", &app_home);
        let result = migrate_project_database_inner(&user_home);
        match previous_home {
            Some(value) => std::env::set_var("DRAGENT_HOME", value),
            None => std::env::remove_var("DRAGENT_HOME"),
        }
        result.unwrap();
        assert_eq!(
            std::fs::read(app_home.join(DB_FILE_NAME)).unwrap(),
            b"live session"
        );
    }

    #[test]
    fn database_path_for_root_override_wins() {
        let _guard = env_lock().lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let custom = dir.path().join("custom.sqlite");
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::set_var(DB_PATH_OVERRIDE_ENV, &custom);
        let root = dir.path().join("proj");
        let resolved = database_path_for_root(&root).unwrap();
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
        assert_eq!(resolved, custom);
    }

    #[test]
    fn database_path_for_root_creates_dir() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::remove_var(DB_PATH_OVERRIDE_ENV);
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("DRAGENT_HOME");
        std::env::set_var("DRAGENT_HOME", dir.path().join("app-home"));
        let root = dir.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        let prev_org = dragent_paths::organisation_id();
        dragent_paths::select_organisation("test-org");
        let resolved = database_path_for_root(&root).unwrap();
        dragent_paths::select_organisation(&prev_org);
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
        assert_eq!(resolved, db_file_for_root(&root, "test-org"));
        assert!(dragent_paths::workspace(&root).is_dir());
        assert!(!root.join(".dragent").exists());
        match previous_home {
            Some(value) => std::env::set_var("DRAGENT_HOME", value),
            None => std::env::remove_var("DRAGENT_HOME"),
        }
    }

    #[test]
    fn database_path_for_root_leaves_dragent_home_in_place() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::remove_var(DB_PATH_OVERRIDE_ENV);
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("DRAGENT_HOME");
        std::env::set_var("DRAGENT_HOME", dir.path().join(".dragent"));
        let global = dir.path().join(".dragent").join("dragent.sqlite");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        std::fs::write(&global, b"live").unwrap();
        let resolved = database_path_for_root(dir.path());
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
        match previous_home {
            Some(value) => std::env::set_var("DRAGENT_HOME", value),
            None => std::env::remove_var("DRAGENT_HOME"),
        }
        resolved.unwrap();
        assert!(global.is_file());
    }

    #[test]
    fn ensure_repo_gitignore_does_not_modify_projects() {
        let dir = tempfile::tempdir().unwrap();
        ensure_repo_gitignore(dir.path()).unwrap();
        assert!(!dir.path().join(".gitignore").exists());

        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        ensure_repo_gitignore(dir.path()).unwrap();
        ensure_repo_gitignore(dir.path()).unwrap();
        assert!(!dir.path().join(".gitignore").exists());
    }

    #[test]
    fn project_root_for_walks_up_and_handles_git_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(project_root_for(&nested), Some(root.clone()));

        let dir2 = tempfile::tempdir().unwrap();
        let root2 = dir2.path().join("wt");
        let nested2 = root2.join("sub");
        std::fs::create_dir_all(&nested2).unwrap();
        std::fs::write(root2.join(".git"), "gitdir: /elsewhere").unwrap();
        assert_eq!(project_root_for(&nested2), Some(root2.clone()));

        let f = nested2.join("main.rs");
        std::fs::write(&f, "fn main(){}").unwrap();
        assert_eq!(project_root_for(&f), Some(root2));

        let dir3 = tempfile::tempdir().unwrap();
        assert_eq!(project_root_for(&dir3.path().join("x")), None);
    }

    #[test]
    fn read_write_resolution_prefers_per_root_then_legacy() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::remove_var(DB_PATH_OVERRIDE_ENV);
        let prev_org = dragent_paths::organisation_id();
        dragent_paths::select_organisation("local");
        let dir = tempfile::tempdir().unwrap();
        let previous_home = std::env::var_os("DRAGENT_HOME");
        std::env::set_var("DRAGENT_HOME", dir.path().join("app-home"));
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let read = db_path_for_read(&root).unwrap();
        assert!(read.ends_with("dragent.sqlite"));
        let written = db_path_for_write(&root).unwrap();
        assert_eq!(
            written,
            dragent_paths::workspace(&root).join("local.sqlite")
        );
        assert!(!root.join(".dragent").exists());
        assert!(!root.join(".gitignore").exists());
        match previous_home {
            Some(value) => std::env::set_var("DRAGENT_HOME", value),
            None => std::env::remove_var("DRAGENT_HOME"),
        }
        dragent_paths::select_organisation(&prev_org);
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
    }

    #[test]
    fn pane_migration_is_copy_once_with_order() {
        let _guard = env_lock().lock().unwrap();
        let previous = std::env::var_os(DB_PATH_OVERRIDE_ENV);
        std::env::remove_var(DB_PATH_OVERRIDE_ENV);
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("legacy.sqlite");
        let target_path = dir.path().join("target.sqlite");
        let mut legacy = setup_connection(&legacy_path).unwrap();
        let mut target = setup_connection(&target_path).unwrap();
        let pane = uuid::Uuid::new_v4();
        for i in 0..3 {
            crate::session_store::save_block(
                &mut legacy,
                pane,
                &crate::types::PersistedBlock {
                    command: format!("cmd{i}"),
                    cwd: PathBuf::from("/tmp"),
                    lines: vec![],
                    status: crate::types::PersistedRunStatus::Success,
                    exit_code: Some(0),
                    started_at_unix_ms: None,
                    duration_ms: None,
                    expanded: false,
                    bookmarked: false,
                    created_at_unix_ms: i,
                },
                i as i32,
            )
            .unwrap();
        }
        let copied = migrate_pane_conns(&mut legacy, &mut target, pane).unwrap();
        assert_eq!(copied, 3);
        let blocks = crate::session_store::load_blocks_for_pane(&mut target, pane).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].command, "cmd0");
        let again = migrate_pane_conns(&mut legacy, &mut target, pane).unwrap();
        assert_eq!(again, 0);
        match previous {
            Some(value) => std::env::set_var(DB_PATH_OVERRIDE_ENV, value),
            None => std::env::remove_var(DB_PATH_OVERRIDE_ENV),
        }
    }
}
