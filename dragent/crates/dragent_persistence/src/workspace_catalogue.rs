//! Durable evidence of intentionally created local-checkout workspaces.
//! Never infer workspace existence from recent projects or Git branch lists.
use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sql_types::{Integer, Text};
use std::path::Path;

use crate::types::{SerializedDocumentSession, SerializedWorkspace};

pub fn record_local_workspace(
    conn: &mut SqliteConnection,
    workspace: &SerializedWorkspace,
) -> Result<()> {
    anyhow::ensure!(
        workspace.organisation_id == crate::database::connection_organisation(conn)?,
        "cannot record a workspace owned by another organisation"
    );
    save_local_workspace(conn, workspace, true)
}

fn save_local_workspace(
    conn: &mut SqliteConnection,
    workspace: &SerializedWorkspace,
    reopen: bool,
) -> Result<()> {
    anyhow::ensure!(
        workspace.organisation_id == crate::database::connection_organisation(conn)?,
        "cannot save a workspace owned by another organisation"
    );
    // Linked worktrees have their own on-disk evidence; projectless terminal
    // and settings surfaces must not become local-branch recovery entries.
    if !workspace.root.join(".git").is_dir() {
        return Ok(());
    }
    let root = workspace
        .root
        .canonicalize()
        .unwrap_or_else(|_| workspace.root.clone());
    let mut record = workspace.clone();
    record.root = root.clone();
    diesel::sql_query(
        "INSERT INTO local_workspace_catalogue (root, workspace_json, closed, sort_order, organisation_id)
         VALUES (?1, ?2, 0, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM local_workspace_catalogue WHERE organisation_id = ?4), ?4)
         ON CONFLICT(organisation_id, root) DO UPDATE SET workspace_json = excluded.workspace_json,
           closed = CASE WHEN ?3 = 1 THEN 0 ELSE local_workspace_catalogue.closed END",
    )
    .bind::<Text, _>(root.to_string_lossy().as_ref())
    .bind::<Text, _>(serde_json::to_string(&record)?)
    .bind::<Integer, _>(i32::from(reopen))
    .bind::<Text, _>(&workspace.organisation_id)
    .execute(conn)
    .context("record local workspace")?;
    Ok(())
}

pub fn close_local_workspace(conn: &mut SqliteConnection, root: &Path) -> Result<()> {
    let owner = crate::database::connection_organisation(conn)?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    diesel::sql_query(
        "UPDATE local_workspace_catalogue SET closed = 1 WHERE root = ?1 AND organisation_id = ?2",
    )
    .bind::<Text, _>(root.to_string_lossy().as_ref())
    .bind::<Text, _>(crate::current_org())
    .execute(conn)
    .context("close local workspace recovery record")?;
    Ok(())
}

/// Backfill explicit memberships from older snapshots and preserve rename and
/// close actions. Absence from a snapshot never means deletion: that is exactly
/// the metadata-loss case this independent catalogue protects against.
pub fn record_document_workspaces(
    conn: &mut SqliteConnection,
    document: &SerializedDocumentSession,
) -> Result<()> {
    anyhow::ensure!(
        document.belongs_to_organisation(&crate::database::connection_organisation(conn)?),
        "cannot catalogue another organisation's snapshot"
    );
    conn.transaction(|conn| {
        for workspace in &document.workspaces {
            if !document.closed_workspace_roots.contains(&workspace.root) {
                // Only an explicit creation event may reopen a closed entry.
                // After a crash this snapshot can predate the close event.
                save_local_workspace(conn, workspace, false)?;
            }
        }
        for root in &document.closed_workspace_roots {
            close_local_workspace(conn, root)?;
        }
        Ok(())
    })
}

pub fn load_local_workspaces(conn: &mut SqliteConnection) -> Result<Vec<SerializedWorkspace>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        workspace_json: String,
    }
    diesel::sql_query(
        "SELECT workspace_json FROM local_workspace_catalogue WHERE closed = 0 AND organisation_id = ?1 ORDER BY sort_order, root",
    )
    .bind::<Text, _>(crate::current_org())
    .load::<Row>(conn)
    .context("load local workspace catalogue")?
    .into_iter()
    .map(|row| serde_json::from_str(&row.workspace_json).context("decode local workspace record"))
    .collect::<Result<Vec<SerializedWorkspace>>>()
    .map(|records| records.into_iter().filter(|record| record.organisation_id == crate::current_org()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{setup_connection, start_writer, ModelEvent, SerializedSession};

    fn workspace(root: std::path::PathBuf, id: usize) -> SerializedWorkspace {
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        SerializedWorkspace {
            organisation_id: dragent_paths::organisation_id(),
            id,
            root,
            title: format!("Local {id}"),
            tab_ids: vec![id],
            active_tab_id: id,
        }
    }

    #[test]
    fn recent_project_does_not_create_a_workspace_but_explicit_membership_does() {
        let temp = tempfile::tempdir().unwrap();
        let record = workspace(temp.path().join("project"), 4);
        let mut conn = setup_connection(&temp.path().join("catalogue.sqlite")).unwrap();
        let mut document = SerializedDocumentSession::default();
        document.open_project_roots.push(record.root.clone());
        record_document_workspaces(&mut conn, &document).unwrap();
        assert!(load_local_workspaces(&mut conn).unwrap().is_empty());
        document.workspaces.push(record.clone());
        record_document_workspaces(&mut conn, &document).unwrap();
        assert_eq!(load_local_workspaces(&mut conn).unwrap()[0].id, record.id);
    }

    #[test]
    fn writer_preserves_creation_even_when_ui_snapshots_are_coalesced() {
        let temp = tempfile::tempdir().unwrap();
        let record = workspace(temp.path().join("project"), 4);
        let path = temp.path().join("catalogue.sqlite");
        let writer = start_writer(path.clone()).unwrap();
        writer.send(ModelEvent::RecordLocalWorkspace(record.clone()));
        writer.snapshot(SerializedSession {
            tabs: Vec::new(),
            active_tab_id: 0,
            active_pane_uuid: None,
            chrome: Default::default(),
            document: Default::default(),
            layouts: Default::default(),
        });
        writer.flush();
        let mut conn = setup_connection(&path).unwrap();
        assert_eq!(
            load_local_workspaces(&mut conn).unwrap()[0].title,
            record.title
        );
        writer.send(ModelEvent::CloseLocalWorkspace(record.root.clone()));
        writer.flush();
        assert!(load_local_workspaces(&mut conn).unwrap().is_empty());
        let mut stale_document = SerializedDocumentSession::default();
        stale_document.workspaces.push(record.clone());
        record_document_workspaces(&mut conn, &stale_document).unwrap();
        assert!(load_local_workspaces(&mut conn).unwrap().is_empty());
        // An explicit new workspace in the same checkout can reopen the record.
        writer.send(ModelEvent::RecordLocalWorkspace(record));
        writer.flush();
        assert_eq!(load_local_workspaces(&mut conn).unwrap().len(), 1);
    }

    #[test]
    fn renaming_does_not_reorder_catalogue_and_empty_snapshot_does_not_erase_it() {
        let temp = tempfile::tempdir().unwrap();
        let mut first = workspace(temp.path().join("a"), 7);
        let second = workspace(temp.path().join("b"), 3);
        let mut conn = setup_connection(&temp.path().join("catalogue.sqlite")).unwrap();
        record_local_workspace(&mut conn, &first).unwrap();
        record_local_workspace(&mut conn, &second).unwrap();
        first.title = "Renamed workspace".into();
        record_local_workspace(&mut conn, &first).unwrap();
        record_document_workspaces(&mut conn, &SerializedDocumentSession::default()).unwrap();
        let records = load_local_workspaces(&mut conn).unwrap();
        assert_eq!(
            records.iter().map(|record| record.id).collect::<Vec<_>>(),
            vec![7, 3]
        );
        assert_eq!(records[0].title, first.title);
    }
}
