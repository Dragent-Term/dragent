//! Durable, local-only recovery snapshots for unsaved editor working copies.

use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use std::path::Path;

use crate::types::{PersistedWorkingCopy, SerializedDocumentSession};

pub fn save_working_copy(
    conn: &mut diesel::sqlite::SqliteConnection,
    copy: &PersistedWorkingCopy,
) -> Result<()> {
    log::debug!(
        "[editor-recovery] write path={} revision={} bytes={}",
        copy.relative_path.display(),
        copy.revision,
        copy.draft_text.len()
    );
    diesel::sql_query(
        "INSERT INTO working_copies
         (workspace_root, relative_path, draft_text, base_hash, base_size,
          base_modified_unix_ms, revision, updated_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(workspace_root, relative_path) DO UPDATE SET
          draft_text = excluded.draft_text,
          base_hash = excluded.base_hash,
          base_size = excluded.base_size,
          base_modified_unix_ms = excluded.base_modified_unix_ms,
          revision = excluded.revision,
          updated_at_unix_ms = excluded.updated_at_unix_ms
         WHERE excluded.revision >= working_copies.revision",
    )
    .bind::<Text, _>(copy.workspace_root.to_string_lossy().into_owned())
    .bind::<Text, _>(copy.relative_path.to_string_lossy().into_owned())
    .bind::<Text, _>(&copy.draft_text)
    .bind::<BigInt, _>(copy.base_hash as i64)
    .bind::<BigInt, _>(copy.base_size as i64)
    .bind::<Nullable<BigInt>, _>(copy.base_modified_unix_ms)
    .bind::<BigInt, _>(copy.revision as i64)
    .bind::<BigInt, _>(copy.updated_at_unix_ms)
    .execute(conn)
    .context("save editor working copy")?;
    Ok(())
}

pub fn delete_working_copy(
    conn: &mut diesel::sqlite::SqliteConnection,
    workspace_root: &Path,
    relative_path: &Path,
) -> Result<()> {
    log::debug!(
        "[editor-recovery] delete root={} path={}",
        workspace_root.display(),
        relative_path.display()
    );
    diesel::sql_query(
        "DELETE FROM working_copies WHERE workspace_root = ?1 AND relative_path = ?2",
    )
    .bind::<Text, _>(workspace_root.to_string_lossy().into_owned())
    .bind::<Text, _>(relative_path.to_string_lossy().into_owned())
    .execute(conn)
    .context("delete editor working copy")?;
    Ok(())
}

pub fn load_working_copies(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<PersistedWorkingCopy>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        workspace_root: String,
        #[diesel(sql_type = Text)]
        relative_path: String,
        #[diesel(sql_type = Text)]
        draft_text: String,
        #[diesel(sql_type = BigInt)]
        base_hash: i64,
        #[diesel(sql_type = BigInt)]
        base_size: i64,
        #[diesel(sql_type = Nullable<BigInt>)]
        base_modified_unix_ms: Option<i64>,
        #[diesel(sql_type = BigInt)]
        revision: i64,
        #[diesel(sql_type = BigInt)]
        updated_at_unix_ms: i64,
    }

    let rows = diesel::sql_query(
        "SELECT workspace_root, relative_path, draft_text, base_hash, base_size,
                base_modified_unix_ms, revision, updated_at_unix_ms
         FROM working_copies ORDER BY updated_at_unix_ms ASC",
    )
    .load::<Row>(conn)
    .context("load editor working copies")?;
    Ok(rows
        .into_iter()
        .map(|row| PersistedWorkingCopy {
            workspace_root: row.workspace_root.into(),
            relative_path: row.relative_path.into(),
            draft_text: row.draft_text,
            base_hash: row.base_hash as u64,
            base_size: row.base_size.max(0) as u64,
            base_modified_unix_ms: row.base_modified_unix_ms,
            revision: row.revision.max(0) as u64,
            updated_at_unix_ms: row.updated_at_unix_ms,
        })
        .collect())
}

pub fn save_document_session(
    conn: &mut diesel::sqlite::SqliteConnection,
    state: &SerializedDocumentSession,
) -> Result<()> {
    let json = serde_json::to_string(state)?;
    diesel::sql_query(
        "INSERT INTO workspace_ui_state (id, state_json, updated_at_unix_ms)
         VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json,
          updated_at_unix_ms = excluded.updated_at_unix_ms",
    )
    .bind::<Text, _>(json)
    .bind::<BigInt, _>(chrono::Utc::now().timestamp_millis())
    .execute(conn)
    .context("save document workspace state")?;
    Ok(())
}

pub fn load_document_session(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<SerializedDocumentSession> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        state_json: String,
    }
    let row = diesel::sql_query("SELECT state_json FROM workspace_ui_state WHERE id = 1")
        .load::<Row>(conn)
        .context("load document workspace state")?
        .into_iter()
        .next();
    match row {
        Some(row) => {
            serde_json::from_str(&row.state_json).context("decode document workspace state")
        }
        None => Ok(SerializedDocumentSession::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::setup_connection;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn database() -> (std::path::PathBuf, diesel::sqlite::SqliteConnection) {
        let path = std::env::temp_dir().join(format!(
            "dragent-working-copy-{}.sqlite",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let connection = setup_connection(&path).unwrap();
        (path, connection)
    }

    fn copy(revision: u64, draft: &str) -> PersistedWorkingCopy {
        PersistedWorkingCopy {
            workspace_root: "/workspace".into(),
            relative_path: "src/main.rs".into(),
            draft_text: draft.into(),
            base_hash: 42,
            base_size: 8,
            base_modified_unix_ms: Some(100),
            revision,
            updated_at_unix_ms: revision as i64,
        }
    }

    #[test]
    fn latest_revision_wins_and_delete_is_explicit() {
        let (path, mut conn) = database();
        save_working_copy(&mut conn, &copy(2, "newer")).unwrap();
        save_working_copy(&mut conn, &copy(1, "stale")).unwrap();
        let loaded = load_working_copies(&mut conn).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].draft_text, "newer");

        delete_working_copy(&mut conn, Path::new("/workspace"), Path::new("src/main.rs")).unwrap();
        assert!(load_working_copies(&mut conn).unwrap().is_empty());
        drop(conn);
        let _ = std::fs::remove_file(path);
    }
}
