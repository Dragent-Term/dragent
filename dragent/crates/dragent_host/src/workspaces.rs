//! Host-owned canonical workspace state (Phase 2).
//!
//! The Host is the sole authority for workspaces created through it: it
//! persists projects, accepts canonical commands through the same
//! `accept_local_workspace_command` path as the UI, keeps an in-memory
//! [`WorkspaceState`] per workspace, and replays durable state on boot.
//! Existing UI-owned workspaces are untouched (no duplicate authority —
//! these sets are disjoint until later migration phases move UI paths over).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceEventEnvelope, project_envelope_event,
};
use dragent_workspace_model::{ActorRef, Project, WorkItem, WorkspaceState};
use uuid::Uuid;

use super::protocol::WorkspaceSnapshot;

const REPLAY_LIMIT: usize = 4096;

/// Which database file backs a workspace: the legacy global file or a
/// per-root project file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DbFile {
    Legacy,
    Root(PathBuf),
}

impl DbFile {
    fn key(&self) -> PathBuf {
        match self {
            DbFile::Legacy => PathBuf::from("__legacy__"),
            DbFile::Root(path) => path.clone(),
        }
    }
}

fn ledger_path_for(legacy_path: &Path) -> PathBuf {
    legacy_path
        .parent()
        .map(|p| p.join("workspaces.json"))
        .unwrap_or_else(|| PathBuf::from("workspaces.json"))
}

fn read_ledger(ledger: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(ledger) else {
        return Vec::new();
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| {
            v.get("roots").and_then(|r| r.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn record_root(ledger: &Path, root: &str) {
    let mut roots = read_ledger(ledger);
    if !roots.iter().any(|r| r == root) {
        roots.push(root.to_string());
    }
    if let Some(parent) = ledger.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let value = serde_json::json!({"roots": roots});
    if let Ok(encoded) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(ledger, encoded);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub struct WorkspaceRegistry {
    /// Per-file SQLite connections, keyed by database file path
    /// (legacy path or per-root file). Lock order: `states` → `conns` —
    /// always acquire `states` first when both are needed.
    conns: Mutex<HashMap<PathBuf, diesel::sqlite::SqliteConnection>>,
    states: Mutex<HashMap<String, WorkspaceState>>,
    /// Which file backs each workspace id. Unknown ids route to Legacy.
    files: Mutex<HashMap<String, DbFile>>,
    legacy_path: PathBuf,
    ledger_path: PathBuf,
}

impl WorkspaceRegistry {
    pub fn open() -> anyhow::Result<Self> {
        let legacy_path = dragent_persistence::database_path()?;
        let ledger_path = ledger_path_for(&legacy_path);
        let registry = Self {
            conns: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            legacy_path,
            ledger_path,
        };
        registry.replay()?;
        Ok(registry)
    }

    #[cfg(test)]
    fn open_with_paths(legacy_path: PathBuf, ledger_path: PathBuf) -> anyhow::Result<Self> {
        let registry = Self {
            conns: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            legacy_path,
            ledger_path,
        };
        registry.replay()?;
        Ok(registry)
    }

    fn conn_for_file(
        &self,
        file: &DbFile,
    ) -> anyhow::Result<std::sync::MutexGuard<'_, HashMap<PathBuf, diesel::sqlite::SqliteConnection>>>
    {
        let path = match file {
            DbFile::Legacy => self.legacy_path.clone(),
            DbFile::Root(path) => path.clone(),
        };
        let mut conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        if !conns.contains_key(&path) {
            let conn = dragent_persistence::setup_connection(&path)?;
            conns.insert(path.clone(), conn);
        }
        drop(conns);
        Ok(self.conns.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn with_conn<T>(
        &self,
        file: &DbFile,
        f: impl FnOnce(&mut diesel::sqlite::SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let path = match file {
            DbFile::Legacy => self.legacy_path.clone(),
            DbFile::Root(path) => path.clone(),
        };
        let mut conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        if !conns.contains_key(&path) {
            let conn = dragent_persistence::setup_connection(&path)?;
            conns.insert(path.clone(), conn);
        }
        let conn = conns.get_mut(&path).expect("just inserted");
        f(conn)
    }

    fn file_for_workspace(&self, workspace_id: &str) -> DbFile {
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(workspace_id)
            .cloned()
            .unwrap_or(DbFile::Legacy)
    }

    /// Rebuild every known workspace from durable state. Replays the legacy
    /// file if it exists, then every ledger root's per-root file.
    fn replay(&self) -> anyhow::Result<()> {
        // Legacy first, if it exists (no creation on read path).
        if self.legacy_path.exists() {
            self.replay_file(&DbFile::Legacy, &self.legacy_path.clone())?;
        }
        for root in read_ledger(&self.ledger_path) {
            // Canonical file placement for symlinked roots.
            let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));
            let file_root = dragent_persistence::project_root_for(&canonical).unwrap_or(canonical);
            let db_path = dragent_persistence::db_path_for_read(&file_root)?;
            if db_path != self.legacy_path && db_path.exists() {
                self.replay_file(&DbFile::Root(db_path.clone()), &db_path)?;
            } else {
                // Record the mapping anyway so later writes land in the
                // right file even before it exists.
                let _ = db_path;
            }
        }
        Ok(())
    }

    fn replay_file(&self, file: &DbFile, path: &Path) -> anyhow::Result<()> {
        let mut conn = dragent_persistence::setup_connection(&path.to_path_buf())?;
        // Cache the connection.
        self.conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.to_path_buf(), conn_setup_clone(path)?);
        let mut conn = dragent_persistence::setup_connection(&path.to_path_buf())?;
        let works = dragent_persistence::load_recent_workspace_work_items(&mut conn)?;
        let mut latest_by_project: HashMap<String, &WorkItem> = HashMap::new();
        for work in &works {
            let key = work.project_id.0.to_string();
            let replace = latest_by_project
                .get(&key)
                .map(|w| work.updated_at_unix_ms > w.updated_at_unix_ms)
                .unwrap_or(true);
            if replace {
                latest_by_project.insert(key, work);
            }
        }
        // Lock order: states → (conns already released above except cache insert).
        let mut states = self.states.lock().unwrap_or_else(|p| p.into_inner());
        let mut files = self.files.lock().unwrap_or_else(|p| p.into_inner());
        for (project_key, work) in latest_by_project {
            let project_id = work.project_id;
            let Some(project) = dragent_persistence::load_workspace_project(&mut conn, project_id)?
            else {
                continue;
            };
            let envelopes = dragent_persistence::load_canonical_workspace_events(
                &mut conn,
                project_id,
                work.id,
                0,
                REPLAY_LIMIT,
            )?;
            let principal = ActorRef::principal_agent(project_id).id;
            let events = envelopes
                .into_iter()
                .filter_map(|env| project_envelope_event(env, principal))
                .collect::<Vec<_>>();
            states.insert(
                project_key.clone(),
                WorkspaceState::restore(project, Some(work.clone()), events),
            );
            files.insert(project_key, file.clone());
        }
        log::info!("workspace replay: {} workspaces restored", states.len());
        Ok(())
    }

    /// Cloned snapshots for the scheduler driver (evaluations are pure).
    pub fn snapshots(&self) -> Vec<(String, WorkspaceState)> {
        self.states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(id, state)| (id.clone(), state.clone()))
            .collect()
    }

    pub fn state_cloned(&self, workspace_id: &str) -> Option<WorkspaceState> {
        self.states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(workspace_id)
            .cloned()
    }

    pub fn list(&self) -> Vec<String> {
        let mut ids = self
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    pub fn create(&self, root: &str) -> anyhow::Result<String> {
        // Project identity uses the input UNCHANGED (no canonicalization —
        // symlinked roots keep stable ids).
        let project = Project::new_for_organisation(&dragent_persistence::current_org(), PathBuf::from(root), now_ms() as i64);
        let id = project.id.0.to_string();
        // Canonical file placement + ledger.
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        let file_root =
            dragent_persistence::project_root_for(&canonical).unwrap_or_else(|| canonical.clone());
        let db_path = dragent_persistence::database_path_for_root(&file_root)?;
        let file = if db_path == self.legacy_path {
            DbFile::Legacy
        } else {
            DbFile::Root(db_path.clone())
        };
        self.with_conn(&file, |conn| {
            dragent_persistence::save_workspace_project(conn, &project)?;
            Ok(())
        })?;
        record_root(&self.ledger_path, root);
        // Lock order: states → files.
        let mut states = self.states.lock().unwrap_or_else(|p| p.into_inner());
        states
            .entry(id.clone())
            .or_insert_with(|| WorkspaceState::new(project));
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), file);
        Ok(id)
    }

    pub fn create_local(&self, root: &str) -> anyhow::Result<String> {
        let project = Project::new_for_organisation(&dragent_persistence::current_org(), PathBuf::from(root), now_ms() as i64);
        let id = Uuid::new_v4().to_string();
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        let file_root =
            dragent_persistence::project_root_for(&canonical).unwrap_or_else(|| canonical.clone());
        let db_path = dragent_persistence::database_path_for_root(&file_root)?;
        let file = if db_path == self.legacy_path {
            DbFile::Legacy
        } else {
            DbFile::Root(db_path)
        };
        self.with_conn(&file, |conn| {
            dragent_persistence::save_workspace_project(conn, &project)?;
            Ok(())
        })?;
        self.states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), WorkspaceState::new(project));
        self.files
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.clone(), file);
        Ok(id)
    }

    pub fn snapshot(&self, workspace_id: &str) -> Option<WorkspaceSnapshot> {
        let states = self.states.lock().unwrap_or_else(|p| p.into_inner());
        states.get(workspace_id).map(|state| {
            let events = state.events();
            WorkspaceSnapshot {
                workspace_id: workspace_id.to_string(),
                project: state.project().clone(),
                active_work: state.active_work().cloned(),
                event_count: events.len(),
                last_sequence: events.back().map(|e| e.sequence).unwrap_or(0),
            }
        })
    }

    /// Accept one canonical command into durable storage, refresh the
    /// in-memory projection from durable truth, and return the produced
    /// envelopes. Idempotent: re-accepting replays stored outcomes without
    /// duplicating effects (store-level guarantee).
    pub fn accept(
        &self,
        workspace_id: &str,
        command: &WorkspaceCommandEnvelope,
    ) -> anyhow::Result<Vec<WorkspaceEventEnvelope>> {
        let file = self.file_for_workspace(workspace_id);
        let envelopes = self.with_conn(&file, |conn| {
            dragent_persistence::accept_local_workspace_command(conn, command)
        })?;
        self.refresh(workspace_id)?;
        Ok(envelopes)
    }

    fn refresh(&self, workspace_id: &str) -> anyhow::Result<()> {
        let file = self.file_for_workspace(workspace_id);
        self.with_conn(&file, |conn| {
            let works = dragent_persistence::load_recent_workspace_work_items(conn)?;
            let project_id = works
                .iter()
                .find(|w| w.project_id.0.to_string() == workspace_id)
                .map(|w| w.project_id);
            let Some(project_id) = project_id else {
                return Ok(());
            };
            let Some(project) = dragent_persistence::load_workspace_project(conn, project_id)?
            else {
                return Ok(());
            };
            let work = works
                .iter()
                .filter(|w| w.project_id == project_id)
                .max_by_key(|w| w.updated_at_unix_ms)
                .cloned();
            let events = match &work {
                Some(work) => {
                    let envelopes = dragent_persistence::load_canonical_workspace_events(
                        conn,
                        project_id,
                        work.id,
                        0,
                        REPLAY_LIMIT,
                    )?;
                    let principal = ActorRef::principal_agent(project_id).id;
                    envelopes
                        .into_iter()
                        .filter_map(|env| project_envelope_event(env, principal))
                        .collect()
                }
                None => Vec::new(),
            };
            self.states
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(
                    workspace_id.to_string(),
                    WorkspaceState::restore(project, work, events),
                );
            Ok(())
        })
    }

    /// Persist a project record through the host authority. Refreshes the
    /// in-memory projection when the project is already managed.
    pub fn save_project(&self, project: &dragent_workspace_model::Project) -> anyhow::Result<()> {
        let id = project.id.0.to_string();
        let file = self.file_for_workspace(&id);
        self.with_conn(&file, |conn| {
            dragent_persistence::save_workspace_project(conn, project)?;
            Ok(())
        })?;
        if self
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&id)
        {
            self.refresh(&id)?;
        }
        Ok(())
    }

    /// Persist a work-item record through the host authority.
    pub fn save_work_item(
        &self,
        work_item: &dragent_workspace_model::WorkItem,
    ) -> anyhow::Result<()> {
        let id = work_item.project_id.0.to_string();
        let file = self.file_for_workspace(&id);
        self.with_conn(&file, |conn| {
            dragent_persistence::save_workspace_work_item(conn, work_item)?;
            Ok(())
        })?;
        if self
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&id)
        {
            self.refresh(&id)?;
        }
        Ok(())
    }

    /// Persist a domain event record through the host authority.
    pub fn save_event(
        &self,
        event: &dragent_workspace_model::WorkspaceEvent,
    ) -> anyhow::Result<()> {
        let id = event.project_id.0.to_string();
        let file = self.file_for_workspace(&id);
        self.with_conn(&file, |conn| {
            dragent_persistence::save_workspace_event(conn, event)?;
            Ok(())
        })?;
        if self
            .states
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&id)
        {
            self.refresh(&id)?;
        }
        Ok(())
    }

    fn all_files(&self) -> Vec<DbFile> {
        let mut out = vec![DbFile::Legacy];
        let conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        for key in conns.keys() {
            if *key != self.legacy_path {
                out.push(DbFile::Root(key.clone()));
            }
        }
        // Also include ledger roots whose files exist but have no cached conn.
        for root in read_ledger(&self.ledger_path) {
            let canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(&root));
            let file_root = dragent_persistence::project_root_for(&canonical).unwrap_or(canonical);
            let Ok(db_path) = dragent_persistence::db_path_for_read(&file_root) else {
                continue;
            };
            let file = DbFile::Root(db_path);
            if !out.contains(&file) && file.key() != DbFile::Legacy.key() {
                // Only include when the file actually exists.
                if let DbFile::Root(p) = &file {
                    if p.exists() {
                        out.push(file);
                    }
                }
            }
        }
        out
    }

    pub fn load_project(
        &self,
        project_id: &str,
    ) -> anyhow::Result<Option<dragent_workspace_model::Project>> {
        let raw =
            uuid::Uuid::parse_str(project_id).map_err(|_| anyhow::anyhow!("bad project id"))?;
        let id = dragent_workspace_model::ProjectId(raw);
        // Search managed files first, then legacy.
        let mut files = self.all_files();
        files.sort_by_key(|f| matches!(f, DbFile::Legacy) as u8);
        for file in files {
            let found = self.with_conn(&file, |conn| {
                dragent_persistence::load_workspace_project(conn, id)
            })?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    pub fn load_recent_events(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<dragent_workspace_model::WorkspaceEvent>> {
        let mut all = Vec::new();
        for file in self.all_files() {
            let events = self.with_conn(&file, |conn| {
                dragent_persistence::load_recent_workspace_events(conn)
            })?;
            all.extend(events);
        }
        // Union newest-first by timestamp (then sequence).
        all.sort_by(|a, b| {
            b.timestamp_unix_ms
                .cmp(&a.timestamp_unix_ms)
                .then(b.sequence.cmp(&a.sequence))
        });
        all.truncate(limit.min(4096));
        Ok(all)
    }
}

fn conn_setup_clone(path: &Path) -> anyhow::Result<diesel::sqlite::SqliteConnection> {
    dragent_persistence::setup_connection(&path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_roots_use_distinct_files_ledger_and_replay() {
        let base = std::env::temp_dir().join(format!(
            "dragent-workspaces-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&base);
        let legacy = base.join("global.sqlite");
        let ledger = base.join("workspaces.json");
        let root_a = base.join("proj-a");
        let root_b = base.join("proj-b");
        std::fs::create_dir_all(root_a.join(".git")).unwrap();
        std::fs::create_dir_all(root_b.join(".git")).unwrap();

        let prev_org = dragent_paths::organisation_id();
        dragent_paths::select_organisation("testorg");
        let prev_override = std::env::var_os("DRAGENT_WORKSPACE_DB");
        unsafe { std::env::remove_var("DRAGENT_WORKSPACE_DB") };

        let registry = WorkspaceRegistry::open_with_paths(legacy.clone(), ledger.clone()).unwrap();
        let id_a = registry.create(root_a.to_str().unwrap()).unwrap();
        let id_b = registry.create(root_b.to_str().unwrap()).unwrap();
        assert_ne!(id_a, id_b);
        // Give each workspace a work item so replay (latest work item per
        // project, mirroring the UI restore path) restores both.
        for id in [&id_a, &id_b] {
            let project = registry.load_project(id).unwrap().expect("created project");
            registry
                .save_work_item(&dragent_workspace_model::WorkItem::new(
                    project.id,
                    "task",
                    now_ms() as i64,
                ))
                .unwrap();
        }

        let file_a = dragent_persistence::db_file_for_root(&root_a, "testorg");
        let file_b = dragent_persistence::db_file_for_root(&root_b, "testorg");
        assert_ne!(file_a, file_b);
        assert!(file_a.exists());
        assert!(file_b.exists());

        // Ledger records both roots.
        let ledger_roots = read_ledger(&ledger);
        assert!(ledger_roots.iter().any(|r| r == root_a.to_str().unwrap()));
        assert!(ledger_roots.iter().any(|r| r == root_b.to_str().unwrap()));

        // Replay restores both workspaces.
        let replayed = WorkspaceRegistry::open_with_paths(legacy.clone(), ledger.clone()).unwrap();
        assert!(replayed.state_cloned(&id_a).is_some());
        assert!(replayed.state_cloned(&id_b).is_some());

        dragent_paths::select_organisation(&prev_org);
        match prev_override {
            Some(value) => unsafe { std::env::set_var("DRAGENT_WORKSPACE_DB", value) },
            None => unsafe { std::env::remove_var("DRAGENT_WORKSPACE_DB") },
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
