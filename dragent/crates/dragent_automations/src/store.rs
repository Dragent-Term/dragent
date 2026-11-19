//! SQLite storage for automations and their runs, one file per organisation.
//!
//! Each row keeps the full record as JSON plus the columns queries need.
//! Run status changes go through [`Store::transition`], which only applies
//! when the run is still in one of the expected statuses, so a cancel and a
//! finishing worker can never both win.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{
    Automation, PromptSource, PromptVersion, Run, RunFilter, RunProgress, RunStatus, RunWorkspace,
    DEFAULT_COPY_PATTERNS,
};

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS automations (
    id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    record TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS automation_runs (
    id TEXT PRIMARY KEY,
    seq INTEGER NOT NULL UNIQUE,
    automation_id TEXT NOT NULL,
    status TEXT NOT NULL,
    scheduled_at INTEGER,
    record TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS automation_runs_by_automation
    ON automation_runs (automation_id, seq DESC);
CREATE INDEX IF NOT EXISTS automation_runs_by_status
    ON automation_runs (status, scheduled_at);
-- One planned occurrence per automation.
CREATE UNIQUE INDEX IF NOT EXISTS automation_runs_one_scheduled
    ON automation_runs (automation_id) WHERE status = 'scheduled';
-- Workspaces runs put in the sidebar. Outlives the run history, so the
-- header can still say which automation started one.
CREATE TABLE IF NOT EXISTS run_workspaces (
    path TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    automation_id TEXT NOT NULL,
    automation_name TEXT NOT NULL,
    run_at INTEGER,
    kept_at INTEGER
);
CREATE TABLE IF NOT EXISTS automation_prompt_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    automation_id TEXT NOT NULL,
    prompt TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS automation_prompt_versions_by_automation
    ON automation_prompt_versions (automation_id, id DESC);
-- Per project: gitignored files copied into each new worktree.
CREATE TABLE IF NOT EXISTS project_settings (
    project_root TEXT PRIMARY KEY,
    copy_patterns TEXT NOT NULL
);
-- Small values shared by the app and the command line (failure badge,
-- plan allowance, sync bookkeeping).
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

fn run_workspace_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunWorkspace> {
    Ok(RunWorkspace {
        path: std::path::PathBuf::from(row.get::<_, String>(0)?),
        run_id: row.get(1)?,
        automation_id: row.get(2)?,
        automation_name: row.get(3)?,
        run_at_ms: row.get(4)?,
    })
}

fn prompt_version_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptVersion> {
    Ok(PromptVersion {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        prompt: row.get(2)?,
        source: PromptSource::parse(&row.get::<_, String>(3)?),
        created_at_ms: row.get(4)?,
    })
}

fn decode<T: serde::de::DeserializeOwned>(text: String) -> Result<T> {
    serde_json::from_str(&text).context("decode stored automation record")
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000; PRAGMA synchronous = FULL;",
        )?;
        conn.execute_batch(SCHEMA)
            .context("create automation tables")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ── Automations ──────────────────────────────────────────────────────────

    pub fn list_automations(&self) -> Result<Vec<Automation>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT record FROM automations ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| decode(row?)).collect()
    }

    pub fn get_automation(&self, id: &str) -> Result<Option<Automation>> {
        let conn = self.conn();
        let record: Option<String> = conn
            .query_row(
                "SELECT record FROM automations WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        record.map(decode).transpose()
    }

    pub fn save_automation(&self, automation: &Automation) -> Result<()> {
        self.conn().execute(
            "INSERT INTO automations (id, enabled, created_at, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, record = excluded.record",
            params![
                automation.id,
                automation.enabled,
                automation.created_at_ms,
                serde_json::to_string(automation)?
            ],
        )?;
        Ok(())
    }

    /// Remove an automation with its run history and prompt history.
    pub fn delete_automation(&self, id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM automation_runs WHERE automation_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM automation_prompt_versions WHERE automation_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM automations WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Changes whenever another connection (the app, or the command line)
    /// commits, so a reader can tell its view is stale.
    pub fn data_version(&self) -> i64 {
        self.conn()
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap_or(0)
    }

    // ── Prompt history ───────────────────────────────────────────────────────

    /// Record a prompt as the newest version, unless it matches it already.
    pub fn add_prompt_version(
        &self,
        automation_id: &str,
        prompt: &str,
        source: PromptSource,
        now_ms: i64,
    ) -> Result<Option<PromptVersion>> {
        let conn = self.conn();
        let latest: Option<String> = conn
            .query_row(
                "SELECT prompt FROM automation_prompt_versions
                 WHERE automation_id = ?1 ORDER BY id DESC LIMIT 1",
                params![automation_id],
                |row| row.get(0),
            )
            .optional()?;
        if latest.as_deref() == Some(prompt) {
            return Ok(None);
        }
        conn.execute(
            "INSERT INTO automation_prompt_versions (automation_id, prompt, source, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![automation_id, prompt, source.as_str(), now_ms],
        )?;
        Ok(Some(PromptVersion {
            id: conn.last_insert_rowid(),
            automation_id: automation_id.to_string(),
            prompt: prompt.to_string(),
            source,
            created_at_ms: now_ms,
        }))
    }

    /// Newest first.
    pub fn prompt_versions(&self, automation_id: &str) -> Result<Vec<PromptVersion>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, automation_id, prompt, source, created_at
             FROM automation_prompt_versions WHERE automation_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![automation_id], prompt_version_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn prompt_version(&self, id: i64) -> Result<Option<PromptVersion>> {
        self.conn()
            .query_row(
                "SELECT id, automation_id, prompt, source, created_at
                 FROM automation_prompt_versions WHERE id = ?1",
                params![id],
                prompt_version_row,
            )
            .optional()
            .map_err(Into::into)
    }

    // ── Project settings and shared values ──────────────────────────────────

    /// Gitignored files copied into the project's new worktrees.
    pub fn copy_patterns(&self, project_root: &Path) -> Result<Vec<String>> {
        let stored: Option<String> = self
            .conn()
            .query_row(
                "SELECT copy_patterns FROM project_settings WHERE project_root = ?1",
                params![project_root.to_string_lossy()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(match stored {
            Some(json) => decode(json)?,
            None => DEFAULT_COPY_PATTERNS
                .iter()
                .map(|p| p.to_string())
                .collect(),
        })
    }

    pub fn set_copy_patterns(&self, project_root: &Path, patterns: &[String]) -> Result<()> {
        self.conn().execute(
            "INSERT INTO project_settings (project_root, copy_patterns) VALUES (?1, ?2)
             ON CONFLICT(project_root) DO UPDATE SET copy_patterns = excluded.copy_patterns",
            params![
                project_root.to_string_lossy(),
                serde_json::to_string(patterns)?
            ],
        )?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.conn()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ── Run workspaces ───────────────────────────────────────────────────────

    /// Remember the workspace a run put in the sidebar (the latest run wins
    /// for a workspace several runs share).
    pub fn record_run_workspace(&self, workspace: &RunWorkspace) -> Result<()> {
        self.conn().execute(
            "INSERT INTO run_workspaces (path, run_id, automation_id, automation_name, run_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                run_id = excluded.run_id,
                automation_id = excluded.automation_id,
                automation_name = excluded.automation_name,
                run_at = excluded.run_at",
            params![
                workspace.path.to_string_lossy(),
                workspace.run_id,
                workspace.automation_id,
                workspace.automation_name,
                workspace.run_at_ms,
            ],
        )?;
        Ok(())
    }

    pub fn run_workspaces(&self) -> Result<Vec<RunWorkspace>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT path, run_id, automation_id, automation_name, run_at
             FROM run_workspaces ORDER BY path",
        )?;
        let rows = stmt.query_map([], run_workspace_row)?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    // ── Runs ─────────────────────────────────────────────────────────────────

    /// Store a new run, assigning its sequence number. `None` when the
    /// automation already has a planned occurrence and this is another one.
    pub fn insert_run(&self, mut run: Run) -> Result<Option<Run>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        run.seq = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM automation_runs",
            [],
            |row| row.get(0),
        )?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO automation_runs (id, seq, automation_id, status, scheduled_at, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.id,
                run.seq,
                run.automation_id,
                run.status.as_str(),
                run.scheduled_at_ms,
                serde_json::to_string(&run)?
            ],
        )?;
        tx.commit()?;
        Ok((inserted == 1).then_some(run))
    }

    pub fn delete_run(&self, id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM automation_runs WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_run(&self, id: &str) -> Result<Option<Run>> {
        let conn = self.conn();
        let record: Option<String> = conn
            .query_row(
                "SELECT record FROM automation_runs WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        record.map(decode).transpose()
    }

    /// Apply `change` if the run is still in one of `from`. Returns the
    /// updated run, or `None` when it had already moved on.
    pub fn transition(
        &self,
        id: &str,
        from: &[RunStatus],
        change: impl FnOnce(&mut Run),
    ) -> Result<Option<Run>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let record: Option<String> = tx
            .query_row(
                "SELECT record FROM automation_runs WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(record) = record else {
            return Ok(None);
        };
        let mut run: Run = decode(record)?;
        if !from.contains(&run.status) {
            return Ok(None);
        }
        change(&mut run);
        tx.execute(
            "UPDATE automation_runs SET status = ?2, scheduled_at = ?3, record = ?4 WHERE id = ?1",
            params![
                run.id,
                run.status.as_str(),
                run.scheduled_at_ms,
                serde_json::to_string(&run)?
            ],
        )?;
        tx.commit()?;
        Ok(Some(run))
    }

    fn query_runs(&self, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Run>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(args, |row| row.get::<_, String>(0))?;
        rows.map(|row| decode(row?)).collect()
    }

    pub fn runs_in_status(&self, statuses: &[RunStatus]) -> Result<Vec<Run>> {
        let mut runs = Vec::new();
        for status in statuses {
            runs.extend(self.query_runs(
                "SELECT record FROM automation_runs WHERE status = ?1 ORDER BY seq",
                params![status.as_str()],
            )?);
        }
        runs.sort_by_key(|run| run.seq);
        Ok(runs)
    }

    pub fn scheduled_run(&self, automation_id: &str) -> Result<Option<Run>> {
        Ok(self
            .query_runs(
                "SELECT record FROM automation_runs WHERE automation_id = ?1 AND status = 'scheduled'",
                params![automation_id],
            )?
            .pop())
    }

    /// Planned occurrences whose time has come, oldest first.
    pub fn due_runs(&self, now_ms: i64, limit: usize) -> Result<Vec<Run>> {
        self.query_runs(
            "SELECT record FROM automation_runs
             WHERE status = 'scheduled' AND scheduled_at <= ?1 ORDER BY scheduled_at, seq LIMIT ?2",
            params![now_ms, limit as i64],
        )
    }

    /// The latest occurrence time ever planned for an automation; the next
    /// plan must come after it, so a skipped occurrence isn't planned again.
    pub fn last_planned_at(&self, automation_id: &str) -> Result<Option<i64>> {
        Ok(self.conn().query_row(
            "SELECT MAX(scheduled_at) FROM automation_runs WHERE automation_id = ?1",
            params![automation_id],
            |row| row.get(0),
        )?)
    }

    /// The latest run that is not just planned.
    pub fn latest_run(&self, automation_id: &str) -> Result<Option<Run>> {
        Ok(self
            .query_runs(
                "SELECT record FROM automation_runs
                 WHERE automation_id = ?1 AND status != 'scheduled' ORDER BY seq DESC LIMIT 1",
                params![automation_id],
            )?
            .pop())
    }

    /// An automation's started runs, newest first.
    pub fn started_runs(&self, automation_id: &str, limit: usize) -> Result<Vec<Run>> {
        self.query_runs(
            "SELECT record FROM automation_runs
             WHERE automation_id = ?1 AND status = 'started' ORDER BY seq DESC LIMIT ?2",
            params![automation_id, limit as i64],
        )
    }

    /// Each automation's most recent run that actually happened.
    pub fn latest_runs(&self) -> Result<HashMap<String, Run>> {
        Ok(self
            .query_runs(
                "SELECT record FROM automation_runs WHERE seq IN (
                    SELECT MAX(seq) FROM automation_runs
                    WHERE status != 'scheduled' GROUP BY automation_id)",
                [],
            )?
            .into_iter()
            .map(|run| (run.automation_id.clone(), run))
            .collect())
    }

    /// Runs that finished at or after `since_ms` (started, failed, …).
    pub fn finished_since(&self, since_ms: i64) -> Result<Vec<Run>> {
        self.query_runs(
            "SELECT record FROM automation_runs
             WHERE status IN ('started', 'failed', 'skipped', 'cancelled')
               AND json_extract(record, '$.finished_at_ms') >= ?1",
            params![since_ms],
        )
    }

    /// Started runs whose agent is still followed (see
    /// [`RunProgress::closed`]) that started at or after `since_ms`,
    /// oldest first.
    pub fn followed_runs(&self, since_ms: i64) -> Result<Vec<Run>> {
        self.query_runs(
            "SELECT record FROM automation_runs
             WHERE status = 'started'
               AND json_extract(record, '$.finished_at_ms') >= ?1
               AND COALESCE(json_extract(record, '$.progress.closed'), 0) = 0
             ORDER BY seq",
            params![since_ms],
        )
    }

    /// Runs with a result to show (their agent finished, failed, or waits
    /// for someone; or they failed to start) since `since_ms`, newest first.
    pub fn inbox_runs(&self, since_ms: i64, limit: usize) -> Result<Vec<Run>> {
        let runs = self.query_runs(
            "SELECT record FROM automation_runs
             WHERE status IN ('started', 'failed')
               AND json_extract(record, '$.finished_at_ms') >= ?1
             ORDER BY seq DESC LIMIT ?2",
            params![since_ms, limit as i64],
        )?;
        Ok(runs
            .into_iter()
            .filter(|run| run.result_at_ms().is_some())
            .collect())
    }

    /// Note that someone opened a run's result.
    pub fn mark_run_read(&self, id: &str, now_ms: i64) -> Result<Option<Run>> {
        self.transition(id, &[RunStatus::Started, RunStatus::Failed], |run| {
            run.read_at_ms = Some(now_ms)
        })
    }

    /// File a run's result away from the inbox, or bring it back.
    pub fn set_run_archived(&self, id: &str, archived: bool) -> Result<Option<Run>> {
        self.transition(id, &[RunStatus::Started, RunStatus::Failed], |run| {
            run.archived = Some(archived)
        })
    }

    /// The latest started run whose agent works in `pane`.
    pub fn run_in_pane(&self, pane: &str) -> Result<Option<Run>> {
        Ok(self
            .query_runs(
                "SELECT record FROM automation_runs
                 WHERE status = 'started'
                   AND (json_extract(record, '$.pane_uuid') = ?1
                        OR json_extract(record, '$.terminal.pane_uuid') = ?1)
                 ORDER BY seq DESC LIMIT 1",
                params![pane],
            )?
            .pop())
    }

    /// The latest started run whose agent works in the host terminal
    /// `session_id`.
    pub fn run_in_terminal(&self, session_id: &str) -> Result<Option<Run>> {
        Ok(self
            .query_runs(
                "SELECT record FROM automation_runs
                 WHERE status = 'started'
                   AND json_extract(record, '$.terminal.session_id') = ?1
                 ORDER BY seq DESC LIMIT 1",
                params![session_id],
            )?
            .pop())
    }

    /// Record how a started run's agent is getting on. `None` when the run
    /// isn't a started one (anymore).
    pub fn set_progress(&self, id: &str, progress: RunProgress) -> Result<Option<Run>> {
        self.transition(id, &[RunStatus::Started], |run| {
            run.progress = Some(progress)
        })
    }

    /// Note the window pane a started run's agent works in.
    pub fn set_pane(&self, id: &str, pane: &str) -> Result<Option<Run>> {
        self.transition(id, &[RunStatus::Started], |run| {
            run.pane_uuid = Some(pane.to_string())
        })
    }

    /// Run history, newest first. `before_seq` pages backwards.
    pub fn list_runs(
        &self,
        automation_id: &str,
        filter: RunFilter,
        before_seq: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Run>> {
        let runs = self.query_runs(
            "SELECT record FROM automation_runs
             WHERE automation_id = ?1 AND status != 'scheduled' AND seq < ?2
             ORDER BY seq DESC",
            params![automation_id, before_seq.unwrap_or(i64::MAX)],
        )?;
        Ok(runs
            .into_iter()
            .filter(|run| filter.matches(run.status))
            .take(limit)
            .collect())
    }

    /// Drop the oldest finished runs beyond the newest `keep` past runs.
    /// The planned occurrence, runs still waiting or starting, and started
    /// runs whose agent is still being followed are never dropped. Returns
    /// how many were removed.
    pub fn prune_runs(&self, automation_id: &str, keep: usize) -> Result<usize> {
        let conn = self.conn();
        let older: Vec<Run> = {
            let mut stmt = conn.prepare(
                "SELECT record FROM automation_runs
                 WHERE automation_id = ?1 AND status != 'scheduled'
                 ORDER BY seq DESC LIMIT -1 OFFSET ?2",
            )?;
            let rows = stmt.query_map(params![automation_id, keep as i64], |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|row| decode(row?)).collect::<Result<_>>()?
        };
        let mut removed = 0;
        for run in older.iter().filter(|run| run.is_settled()) {
            removed += conn.execute("DELETE FROM automation_runs WHERE id = ?1", params![run.id])?;
        }
        Ok(removed)
    }

    /// The run with this id, or else the runs whose id starts with it. At
    /// most two prefix matches: enough to tell a unique prefix from an
    /// ambiguous one.
    pub fn runs_matching(&self, id: &str) -> Result<Vec<Run>> {
        if let Some(run) = self.get_run(id)? {
            return Ok(vec![run]);
        }
        if id.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT record FROM automation_runs WHERE substr(id, 1, ?2) = ?1
             ORDER BY seq DESC LIMIT 2",
        )?;
        let rows = stmt.query_map(params![id, id.chars().count() as i64], |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| decode(row?)).collect()
    }

    /// Run counts per history filter.
    pub fn run_counts(&self, automation_id: &str) -> Result<HashMap<RunFilter, usize>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM automation_runs WHERE automation_id = ?1 GROUP BY status",
        )?;
        let rows = stmt.query_map(params![automation_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut counts: HashMap<RunFilter, usize> =
            RunFilter::ALL.iter().map(|filter| (*filter, 0)).collect();
        for row in rows {
            let (status, count) = row?;
            let Some(status) = RunStatus::parse(&status) else {
                continue;
            };
            for filter in RunFilter::ALL {
                if filter.matches(status) {
                    *counts.entry(filter).or_default() += count as usize;
                }
            }
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RunSnapshot, Trigger, WorkspaceMode};
    use std::path::PathBuf;

    pub(crate) fn automation() -> Automation {
        Automation::new(
            "Daily digest",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "Summarise yesterday",
            WorkspaceMode::default(),
            1,
        )
    }

    fn run(automation: &Automation, status: RunStatus) -> Run {
        Run {
            id: uuid::Uuid::new_v4().to_string(),
            seq: 0,
            automation_id: automation.id.clone(),
            status,
            trigger: Trigger::Schedule,
            snapshot: RunSnapshot {
                ..automation.snapshot()
            },
            name: "run".into(),
            scheduled_at_ms: Some(100),
            deadline_at_ms: None,
            started_at_ms: None,
            finished_at_ms: None,
            workspace_path: None,
            branch: None,
            terminal: None,
            error: None,
            continued: false,
            warning: None,
            pane_uuid: None,
            progress: None,
            read_at_ms: None,
            archived: None,
            attempt: 0,
            retry_of: None,
        }
    }

    #[test]
    fn automations_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automations.sqlite");
        let mut saved = automation();
        {
            let store = Store::open(&path).unwrap();
            store.save_automation(&saved).unwrap();
            saved.name = "Renamed".into();
            saved.enabled = false;
            store.save_automation(&saved).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.list_automations().unwrap(), vec![saved.clone()]);
        assert_eq!(store.get_automation(&saved.id).unwrap(), Some(saved));
    }

    #[test]
    fn only_one_planned_occurrence_per_automation() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let first = store
            .insert_run(run(&automation, RunStatus::Scheduled))
            .unwrap();
        assert_eq!(first.as_ref().map(|r| r.seq), Some(1));
        assert!(store
            .insert_run(run(&automation, RunStatus::Scheduled))
            .unwrap()
            .is_none());
        let manual = store
            .insert_run(run(&automation, RunStatus::Queued))
            .unwrap()
            .unwrap();
        assert_eq!(manual.seq, 2);
    }

    #[test]
    fn transitions_only_apply_from_expected_statuses() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let queued = store
            .insert_run(run(&automation, RunStatus::Queued))
            .unwrap()
            .unwrap();
        let claimed = store
            .transition(&queued.id, &[RunStatus::Queued], |run| {
                run.status = RunStatus::PreparingWorkspace
            })
            .unwrap();
        assert_eq!(claimed.unwrap().status, RunStatus::PreparingWorkspace);
        // A second claim loses.
        assert!(store
            .transition(&queued.id, &[RunStatus::Queued], |run| {
                run.status = RunStatus::PreparingWorkspace
            })
            .unwrap()
            .is_none());
    }

    #[test]
    fn history_pages_newest_first_and_counts_by_filter() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        for status in [
            RunStatus::Started,
            RunStatus::Failed,
            RunStatus::Started,
            RunStatus::Skipped,
            RunStatus::Scheduled,
        ] {
            store.insert_run(run(&automation, status)).unwrap();
        }
        let all = store
            .list_runs(&automation.id, RunFilter::All, None, 10)
            .unwrap();
        assert_eq!(
            all.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![4, 3, 2, 1]
        );
        let page = store
            .list_runs(&automation.id, RunFilter::All, Some(3), 1)
            .unwrap();
        assert_eq!(page[0].seq, 2);
        let started = store
            .list_runs(&automation.id, RunFilter::Started, None, 10)
            .unwrap();
        assert_eq!(started.len(), 2);
        let counts = store.run_counts(&automation.id).unwrap();
        assert_eq!(counts[&RunFilter::All], 4);
        assert_eq!(counts[&RunFilter::Started], 2);
        assert_eq!(counts[&RunFilter::Cancelled], 0);
        assert_eq!(store.latest_run(&automation.id).unwrap().unwrap().seq, 4);
        assert!(store.scheduled_run(&automation.id).unwrap().is_some());
    }

    #[test]
    fn deleting_an_automation_removes_its_runs() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        store.save_automation(&automation).unwrap();
        store
            .insert_run(run(&automation, RunStatus::Started))
            .unwrap();
        store.delete_automation(&automation.id).unwrap();
        assert!(store.list_automations().unwrap().is_empty());
        assert!(store
            .list_runs(&automation.id, RunFilter::All, None, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn run_workspaces_follow_the_latest_run() {
        let store = Store::open_in_memory().unwrap();
        let mut placed = RunWorkspace {
            path: PathBuf::from("/repo"),
            run_id: "run-1".into(),
            automation_id: "a".into(),
            automation_name: "Nightly".into(),
            run_at_ms: Some(10),
        };
        store.record_run_workspace(&placed).unwrap();
        placed.run_id = "run-2".into();
        placed.run_at_ms = Some(20);
        store.record_run_workspace(&placed).unwrap();
        assert_eq!(store.run_workspaces().unwrap(), vec![placed]);
    }

    #[test]
    fn prompt_history_skips_repeats_and_lists_newest_first() {
        let store = Store::open_in_memory().unwrap();
        assert!(store
            .add_prompt_version("a", "one", PromptSource::Human, 1)
            .unwrap()
            .is_some());
        assert!(store
            .add_prompt_version("a", "one", PromptSource::Human, 2)
            .unwrap()
            .is_none());
        let two = store
            .add_prompt_version("a", "two", PromptSource::Agent, 3)
            .unwrap()
            .unwrap();
        let history = store.prompt_versions("a").unwrap();
        assert_eq!(
            history
                .iter()
                .map(|v| v.prompt.as_str())
                .collect::<Vec<_>>(),
            vec!["two", "one"]
        );
        assert_eq!(
            store.prompt_version(two.id).unwrap().unwrap().source,
            PromptSource::Agent
        );
        store.delete_automation("a").unwrap();
        assert!(store.prompt_versions("a").unwrap().is_empty());
    }

    #[test]
    fn copy_patterns_default_until_set() {
        let store = Store::open_in_memory().unwrap();
        let root = PathBuf::from("/repo");
        assert!(store
            .copy_patterns(&root)
            .unwrap()
            .contains(&".env".to_string()));
        store
            .set_copy_patterns(&root, &["secrets.json".into()])
            .unwrap();
        assert_eq!(
            store.copy_patterns(&root).unwrap(),
            vec!["secrets.json".to_string()]
        );
        store.set_meta("k", "1").unwrap();
        store.set_meta("k", "2").unwrap();
        assert_eq!(store.meta("k").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn latest_runs_and_recent_outcomes() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let mut started = run(&automation, RunStatus::Started);
        started.finished_at_ms = Some(1_000);
        store.insert_run(started).unwrap();
        let mut failed = run(&automation, RunStatus::Failed);
        failed.finished_at_ms = Some(5_000);
        let failed = store.insert_run(failed).unwrap().unwrap();
        store
            .insert_run(run(&automation, RunStatus::Scheduled))
            .unwrap();
        let latest = store.latest_runs().unwrap();
        assert_eq!(latest[&automation.id].id, failed.id);
        assert_eq!(store.finished_since(2_000).unwrap().len(), 1);
        assert_eq!(store.finished_since(0).unwrap().len(), 2);
    }

    #[test]
    fn runs_are_found_by_pane_and_followed_until_closed() {
        use crate::model::{AgentState, RunProgress, RunTerminal};
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let mut hosted = run(&automation, RunStatus::Started);
        hosted.finished_at_ms = Some(1_000);
        hosted.terminal = Some(RunTerminal {
            workspace_id: "w".into(),
            home_id: "h".into(),
            session_id: "s".into(),
            pane_uuid: Some("pane-a".into()),
        });
        let hosted = store.insert_run(hosted).unwrap().unwrap();
        let mut windowed = run(&automation, RunStatus::Started);
        windowed.finished_at_ms = Some(2_000);
        let windowed = store.insert_run(windowed).unwrap().unwrap();
        store.set_pane(&windowed.id, "pane-b").unwrap().unwrap();

        assert_eq!(store.run_in_pane("pane-a").unwrap().unwrap().id, hosted.id);
        assert_eq!(
            store.run_in_pane("pane-b").unwrap().unwrap().id,
            windowed.id
        );
        assert!(store.run_in_pane("pane-c").unwrap().is_none());
        assert_eq!(store.followed_runs(0).unwrap().len(), 2);

        let done = RunProgress {
            state: AgentState::Done,
            at_ms: 3_000,
            reply: Some("All clear".into()),
            closed: true,
            settled: true,
        };
        let updated = store
            .set_progress(&hosted.id, done.clone())
            .unwrap()
            .unwrap();
        assert_eq!(updated.progress, Some(done));
        let followed = store.followed_runs(0).unwrap();
        assert_eq!(followed.len(), 1);
        assert_eq!(followed[0].id, windowed.id);
        assert_eq!(store.followed_runs(2_500).unwrap().len(), 0);
    }

    #[test]
    fn the_inbox_holds_results_until_read_and_files_empty_ones_away() {
        use crate::model::{AgentState, RunProgress};
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let progress = |state: AgentState, at_ms: i64, reply: Option<&str>| RunProgress {
            state,
            at_ms,
            reply: reply.map(str::to_owned),
            closed: false,
            settled: RunProgress::settles(state),
        };
        let mut working = run(&automation, RunStatus::Started);
        working.finished_at_ms = Some(1_000);
        working.progress = Some(progress(AgentState::Working, 1_000, None));
        let working = store.insert_run(working).unwrap().unwrap();
        let mut done = run(&automation, RunStatus::Started);
        done.finished_at_ms = Some(2_000);
        done.progress = Some(progress(
            AgentState::Done,
            3_000,
            Some("Two flaky tests fixed."),
        ));
        let done = store.insert_run(done).unwrap().unwrap();
        let mut quiet = run(&automation, RunStatus::Started);
        quiet.finished_at_ms = Some(2_500);
        quiet.progress = Some(progress(
            AgentState::Done,
            3_500,
            Some("**Nothing to report** today."),
        ));
        let quiet = store.insert_run(quiet).unwrap().unwrap();
        let mut failed = run(&automation, RunStatus::Failed);
        failed.finished_at_ms = Some(4_000);
        let failed = store.insert_run(failed).unwrap().unwrap();

        // A run still working has no result yet.
        let inbox: Vec<String> = store
            .inbox_runs(0, 50)
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(
            inbox,
            vec![failed.id.clone(), quiet.id.clone(), done.id.clone()]
        );
        assert!(!inbox.contains(&working.id));

        let done = store.get_run(&done.id).unwrap().unwrap();
        assert!(done.is_unread() && !done.is_archived());
        let read = store.mark_run_read(&done.id, 3_100).unwrap().unwrap();
        assert!(!read.is_unread());
        // A newer result is unread again.
        let mut later = read.clone();
        later.progress = Some(progress(AgentState::NeedsInput, 5_000, None));
        assert!(later.is_unread());

        let quiet = store.get_run(&quiet.id).unwrap().unwrap();
        assert!(quiet.is_archived(), "nothing to report files itself away");
        let restored = store.set_run_archived(&quiet.id, false).unwrap().unwrap();
        assert!(!restored.is_archived(), "someone's choice wins");
        assert!(store
            .set_run_archived(&failed.id, true)
            .unwrap()
            .unwrap()
            .is_archived());
    }

    #[test]
    fn retention_prunes_old_runs_but_keeps_the_plan() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        store.save_automation(&automation).unwrap();
        // Oldest first: a queued run, a followed agent, then five failures.
        let waiting = store
            .insert_run(run(&automation, RunStatus::Queued))
            .unwrap()
            .unwrap();
        let mut working = run(&automation, RunStatus::Started);
        working.progress = Some(crate::model::RunProgress {
            state: crate::model::AgentState::Working,
            at_ms: 1_000,
            reply: None,
            closed: false,
            settled: false,
        });
        let working = store.insert_run(working).unwrap().unwrap();
        for _ in 0..5 {
            store
                .insert_run(run(&automation, RunStatus::Failed))
                .unwrap();
        }
        store
            .insert_run(run(&automation, RunStatus::Scheduled))
            .unwrap();
        assert_eq!(store.prune_runs(&automation.id, 2).unwrap(), 3);
        let kept = store
            .list_runs(&automation.id, RunFilter::All, None, 10)
            .unwrap();
        assert_eq!(kept.len(), 4, "two newest failures, the queued run and the working one");
        assert!(store.get_run(&waiting.id).unwrap().is_some());
        assert!(store.get_run(&working.id).unwrap().is_some());
        assert!(store.scheduled_run(&automation.id).unwrap().is_some());
        assert_eq!(store.prune_runs(&automation.id, 10).unwrap(), 0);
    }

    #[test]
    fn runs_are_found_by_prefix() {
        let store = Store::open_in_memory().unwrap();
        let automation = automation();
        let first = store
            .insert_run(run(&automation, RunStatus::Failed))
            .unwrap()
            .unwrap();
        let ids = |id: &str| -> Vec<String> {
            store.runs_matching(id).unwrap().into_iter().map(|run| run.id).collect()
        };
        assert_eq!(ids(&first.id), vec![first.id.clone()]);
        assert_eq!(ids(&first.id[..8]), vec![first.id.clone()], "a unique prefix finds the run");
        assert!(ids("does-not-exist").is_empty());
        assert!(ids("").is_empty());
        assert!(ids("%").is_empty(), "no wildcards");
        for id in ["abc-1", "abc-2"] {
            let mut twin = run(&automation, RunStatus::Failed);
            twin.id = id.into();
            store.insert_run(twin).unwrap();
        }
        assert_eq!(ids("abc").len(), 2, "an ambiguous prefix finds both");
    }

    #[test]
    fn existing_stored_automations_migrate_with_defaults() {
        // An automation saved before the dependability policies existed must
        // keep working: its JSON loads, validates, and carries the old
        // behaviour (skip missed, skip overlap, no retries, keep 100 runs).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automations.sqlite");
        let record = r#"{"id":"old","name":"Nightly","project_root":"/repo","schedule":"0 9 * * *",
            "agent":"claude","prompt":"Go","workspace":{"kind":"project_checkout"},"enabled":true,
            "revision":1,"created_at_ms":0,"updated_at_ms":0}"#;
        let old: Automation = serde_json::from_str(record).unwrap();
        {
            let store = Store::open(&path).unwrap();
            store.save_automation(&old).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let loaded = store.get_automation("old").unwrap().unwrap();
        assert_eq!(loaded.missed_policy, crate::model::MissedPolicy::Skip);
        assert_eq!(loaded.overlap_policy, crate::model::OverlapPolicy::Skip);
        assert_eq!(loaded.max_retries, 0);
        assert_eq!(loaded.run_retention, 100);
        assert!(loaded.validate().is_ok());
        assert!(loaded.next_occurrence_ms(0).is_some());
    }
}
