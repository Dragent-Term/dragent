//! Durable, metadata-only storage for the engineering workspace projection.

use crate::workspace_store::MAX_PERSISTED_EVENTS;
use crate::workspace_store::MAX_RESTORED_WORK_ITEMS;
use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use dragent_workspace_model::{
    Project, ProjectId, WorkItem, WorkItemId, WorkStatus, WorkspaceEvent, WorkspaceEventKind,
};
use std::path::Path;
use uuid::Uuid;

pub fn save_workspace_project(
    conn: &mut diesel::sqlite::SqliteConnection,
    project: &Project,
) -> Result<()> {
    anyhow::ensure!(
        project.organisation_id == crate::database::connection_organisation(conn)?,
        "cannot save a project owned by another organisation"
    );
    diesel::sql_query(
        "INSERT INTO workspace_projects (id, root, display_name, created_at_unix_ms, updated_at_unix_ms, organisation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET root = excluded.root, display_name = excluded.display_name,
           updated_at_unix_ms = excluded.updated_at_unix_ms",
    )
    .bind::<Text, _>(project.id.0.to_string())
    .bind::<Text, _>(project.root.to_string_lossy().into_owned())
    .bind::<Text, _>(&project.display_name)
    .bind::<BigInt, _>(project.created_at_unix_ms)
    .bind::<BigInt, _>(project.updated_at_unix_ms)
    .bind::<Text, _>(&project.organisation_id)
    .execute(conn)
    .context("save workspace project")?;
    Ok(())
}

/// Forget only Dragent's local project records. The repository on disk is untouched.
pub fn delete_workspace_project(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
) -> Result<()> {
    let id = project_id.0.to_string();
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        for table in [
            "workspace_integration_intents",
            "workspace_tool_outcomes",
            "canonical_workspace_command_outcomes",
            "canonical_workspace_events",
            "workspace_events",
            "workspace_work_items",
        ] {
            diesel::sql_query(format!("DELETE FROM {table} WHERE project_id = ?1"))
                .bind::<Text, _>(&id)
                .execute(conn)
                .with_context(|| format!("delete {table} for project"))?;
        }
        diesel::sql_query("DELETE FROM workspace_projects WHERE id = ?1")
            .bind::<Text, _>(&id)
            .execute(conn)
            .context("delete workspace project")?;
        Ok(())
    })
}

#[cfg(test)]
mod delete_tests {
    use super::*;

    #[test]
    fn deleting_project_forgets_database_records_but_keeps_folder() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("project");
        std::fs::create_dir(&folder).unwrap();
        let mut conn =
            crate::database::setup_connection(&temp.path().join("state.sqlite")).unwrap();
        let project = Project::new(folder.clone(), 1);
        save_workspace_project(&mut conn, &project).unwrap();
        diesel::sql_query("INSERT INTO workspace_work_items (id, project_id, title, status_json, created_at_unix_ms, updated_at_unix_ms) VALUES ('work-1', ?1, 'task', '\"draft\"', 1, 1)")
            .bind::<Text, _>(project.id.0.to_string()).execute(&mut conn).unwrap();
        diesel::sql_query("INSERT INTO canonical_workspace_events (event_id, project_id, work_item_id, sequence, envelope_json, created_at_unix_ms) VALUES ('event-1', ?1, 'work-1', 1, '{}', 1)")
            .bind::<Text, _>(project.id.0.to_string()).execute(&mut conn).unwrap();

        delete_workspace_project(&mut conn, project.id).unwrap();

        assert!(load_workspace_project(&mut conn, project.id)
            .unwrap()
            .is_none());
        assert!(load_recent_workspace_work_items(&mut conn)
            .unwrap()
            .is_empty());
        assert!(folder.is_dir());
    }
}

pub fn save_workspace_work_item(
    conn: &mut diesel::sqlite::SqliteConnection,
    work_item: &WorkItem,
) -> Result<()> {
    let status_json = serde_json::to_string(&work_item.status)?;
    diesel::sql_query(
        "INSERT INTO workspace_work_items (id, project_id, title, status_json, created_at_unix_ms, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET title = excluded.title, status_json = excluded.status_json,
           updated_at_unix_ms = excluded.updated_at_unix_ms",
    )
    .bind::<Text, _>(work_item.id.0.to_string())
    .bind::<Text, _>(work_item.project_id.0.to_string())
    .bind::<Text, _>(&work_item.title)
    .bind::<Text, _>(status_json)
    .bind::<BigInt, _>(work_item.created_at_unix_ms)
    .bind::<BigInt, _>(work_item.updated_at_unix_ms)
    .execute(conn)
    .context("save workspace work item")?;
    Ok(())
}

pub fn save_workspace_event(
    conn: &mut diesel::sqlite::SqliteConnection,
    event: &WorkspaceEvent,
) -> Result<()> {
    let kind_json = serde_json::to_string(&event.kind)?;
    diesel::sql_query(
        "INSERT INTO workspace_events (project_id, work_item_id, sequence, timestamp_unix_ms, kind_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind::<Text, _>(event.project_id.0.to_string())
    .bind::<Nullable<Text>, _>(event.work_item_id.map(|id| id.0.to_string()))
    .bind::<BigInt, _>(event.sequence as i64)
    .bind::<BigInt, _>(event.timestamp_unix_ms)
    .bind::<Text, _>(kind_json)
    .execute(conn)
    .context("append workspace event")?;

    diesel::sql_query(
        "DELETE FROM workspace_events WHERE id IN
         (SELECT id FROM workspace_events ORDER BY id DESC LIMIT -1 OFFSET ?1)",
    )
    .bind::<BigInt, _>(MAX_PERSISTED_EVENTS)
    .execute(conn)
    .context("trim workspace events")?;
    Ok(())
}

pub fn load_workspace_project(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
) -> Result<Option<Project>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        root: String,
        #[diesel(sql_type = Text)]
        display_name: String,
        #[diesel(sql_type = BigInt)]
        created_at_unix_ms: i64,
        #[diesel(sql_type = BigInt)]
        updated_at_unix_ms: i64,
    }
    let row = diesel::sql_query(
        "SELECT root, display_name, created_at_unix_ms, updated_at_unix_ms
         FROM workspace_projects WHERE id = ?1 AND organisation_id = ?2",
    )
    .bind::<Text, _>(project_id.0.to_string())
    .bind::<Text, _>(crate::current_org())
    .load::<Row>(conn)
    .context("load workspace project")?
    .into_iter()
    .next();
    Ok(row.map(|row| Project {
        organisation_id: crate::current_org(),
        id: project_id,
        root: Path::new(&row.root).to_path_buf(),
        display_name: row.display_name,
        created_at_unix_ms: row.created_at_unix_ms,
        updated_at_unix_ms: row.updated_at_unix_ms,
    }))
}

/// Restore only a bounded set of recently active work items. File contents and
/// terminal output are deliberately absent from this projection.
pub fn load_recent_workspace_work_items(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<WorkItem>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Text)]
        project_id: String,
        #[diesel(sql_type = Text)]
        title: String,
        #[diesel(sql_type = Text)]
        status_json: String,
        #[diesel(sql_type = BigInt)]
        created_at_unix_ms: i64,
        #[diesel(sql_type = BigInt)]
        updated_at_unix_ms: i64,
    }

    diesel::sql_query(
        "SELECT id, project_id, title, status_json, created_at_unix_ms, updated_at_unix_ms
         FROM workspace_work_items
         ORDER BY updated_at_unix_ms DESC
         LIMIT ?1",
    )
    .bind::<BigInt, _>(MAX_RESTORED_WORK_ITEMS)
    .load::<Row>(conn)
    .context("load recent workspace work items")?
    .into_iter()
    .map(|row| {
        Ok(WorkItem {
            id: WorkItemId(Uuid::parse_str(&row.id).context("decode workspace work item id")?),
            project_id: ProjectId(
                Uuid::parse_str(&row.project_id).context("decode workspace project id")?,
            ),
            title: row.title,
            status: serde_json::from_str::<WorkStatus>(&row.status_json)
                .context("decode workspace work status")?,
            created_at_unix_ms: row.created_at_unix_ms,
            updated_at_unix_ms: row.updated_at_unix_ms,
        })
    })
    .collect()
}

pub(crate) fn load_workspace_work_item(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
) -> Result<Option<WorkItem>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        title: String,
        #[diesel(sql_type = Text)]
        status_json: String,
        #[diesel(sql_type = BigInt)]
        created_at_unix_ms: i64,
        #[diesel(sql_type = BigInt)]
        updated_at_unix_ms: i64,
    }

    let row = diesel::sql_query(
        "SELECT title, status_json, created_at_unix_ms, updated_at_unix_ms
         FROM workspace_work_items WHERE id = ?1 AND project_id = ?2",
    )
    .bind::<Text, _>(work_item_id.0.to_string())
    .bind::<Text, _>(project_id.0.to_string())
    .load::<Row>(conn)
    .context("load workspace work item")?
    .into_iter()
    .next();
    row.map(|row| {
        Ok(WorkItem {
            id: work_item_id,
            project_id,
            title: row.title,
            status: serde_json::from_str(&row.status_json)
                .context("decode workspace work status")?,
            created_at_unix_ms: row.created_at_unix_ms,
            updated_at_unix_ms: row.updated_at_unix_ms,
        })
    })
    .transpose()
}

/// Load the bounded durable event tail in insertion order. `WorkspaceState`
/// performs project/work identity filtering before exposing it to renderers.
pub fn load_recent_workspace_events(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<WorkspaceEvent>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        project_id: String,
        #[diesel(sql_type = Nullable<Text>)]
        work_item_id: Option<String>,
        #[diesel(sql_type = BigInt)]
        sequence: i64,
        #[diesel(sql_type = BigInt)]
        timestamp_unix_ms: i64,
        #[diesel(sql_type = Text)]
        kind_json: String,
    }

    diesel::sql_query(
        "SELECT project_id, work_item_id, sequence, timestamp_unix_ms, kind_json
         FROM (
           SELECT id, project_id, work_item_id, sequence, timestamp_unix_ms, kind_json
           FROM workspace_events ORDER BY id DESC LIMIT ?1
         ) ORDER BY id ASC",
    )
    .bind::<BigInt, _>(MAX_PERSISTED_EVENTS)
    .load::<Row>(conn)
    .context("load recent workspace events")?
    .into_iter()
    .map(|row| {
        Ok(WorkspaceEvent {
            sequence: u64::try_from(row.sequence).context("negative workspace event sequence")?,
            timestamp_unix_ms: row.timestamp_unix_ms,
            project_id: ProjectId(
                Uuid::parse_str(&row.project_id).context("decode event project id")?,
            ),
            work_item_id: row
                .work_item_id
                .map(|id| {
                    Uuid::parse_str(&id)
                        .map(WorkItemId)
                        .context("decode event work item id")
                })
                .transpose()?,
            kind: serde_json::from_str::<WorkspaceEventKind>(&row.kind_json)
                .context("decode workspace event kind")?,
        })
    })
    .collect()
}
