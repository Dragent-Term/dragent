//! Durable, metadata-only storage for the engineering workspace projection.

use crate::store_accept::preflight_execution_acceptance;
use crate::WorkspaceIntegrationIntent;
use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceCommandPayload, WorkspaceEventEnvelope,
};
use dragent_workspace_model::{
    ExecutionAttemptId, ExecutionReviewResolution, ProjectId, WorkItemId,
};
use uuid::Uuid;

#[derive(QueryableByName)]
struct WorkspaceIntegrationIntentRow {
    #[diesel(sql_type = Text)]
    attempt_id: String,
    #[diesel(sql_type = Text)]
    project_id: String,
    #[diesel(sql_type = Text)]
    work_item_id: String,
    #[diesel(sql_type = Text)]
    command_id: String,
    #[diesel(sql_type = Text)]
    command_json: String,
    #[diesel(sql_type = Text)]
    integration_plan_json: String,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    claim_owner: Option<String>,
    #[diesel(sql_type = Nullable<BigInt>)]
    claim_expires_at_unix_ms: Option<i64>,
    #[diesel(sql_type = BigInt)]
    created_at_unix_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    completed_at_unix_ms: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    last_error: Option<String>,
}

impl WorkspaceIntegrationIntentRow {
    fn decode(self) -> Result<WorkspaceIntegrationIntent> {
        let command: WorkspaceCommandEnvelope = serde_json::from_str(&self.command_json)
            .context("decode integration completion command")?;
        let intent = WorkspaceIntegrationIntent {
            attempt_id: ExecutionAttemptId(Uuid::parse_str(&self.attempt_id)?),
            project_id: ProjectId(Uuid::parse_str(&self.project_id)?),
            work_item_id: WorkItemId(Uuid::parse_str(&self.work_item_id)?),
            command_id: dragent_session_protocol::workspace::WorkspaceCommandId(Uuid::parse_str(
                &self.command_id,
            )?),
            command,
            integration_plan_json: self.integration_plan_json,
            state: self.state,
            claim_owner: self.claim_owner,
            claim_expires_at_unix_ms: self.claim_expires_at_unix_ms,
            created_at_unix_ms: self.created_at_unix_ms,
            completed_at_unix_ms: self.completed_at_unix_ms,
            last_error: self.last_error,
        };
        let command_attempt = match &intent.command.payload {
            WorkspaceCommandPayload::ResolveAgentExecutionReview {
                attempt_id,
                resolution: ExecutionReviewResolution::Integrated,
            } => Some(*attempt_id),
            _ => None,
        };
        anyhow::ensure!(
            intent.command_id == intent.command.command_id
                && intent.project_id == intent.command.project_id
                && intent.work_item_id == intent.command.work_item_id
                && command_attempt == Some(intent.attempt_id),
            "integration intent command identity is inconsistent"
        );
        Ok(intent)
    }
}

pub fn begin_workspace_integration(
    conn: &mut diesel::sqlite::SqliteConnection,
    intent: &WorkspaceIntegrationIntent,
) -> Result<()> {
    let WorkspaceCommandPayload::ResolveAgentExecutionReview {
        attempt_id,
        resolution: ExecutionReviewResolution::Integrated,
    } = &intent.command.payload
    else {
        anyhow::bail!("integration intent must contain an Integrated review command");
    };
    anyhow::ensure!(
        *attempt_id == intent.attempt_id
            && intent.command_id == intent.command.command_id
            && intent.project_id == intent.command.project_id
            && intent.work_item_id == intent.command.work_item_id
            && intent.state == "prepared"
            && !intent.integration_plan_json.is_empty(),
        "integration intent identity is incomplete"
    );
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        let current_revision = next_canonical_sequence(conn, intent.work_item_id)?
            .saturating_sub(1);
        anyhow::ensure!(
            intent.command.expected_revision == Some(current_revision),
            "workspace revision conflict before Git integration: expected {:?}, current {current_revision}",
            intent.command.expected_revision
        );
        preflight_execution_acceptance(
            conn,
            intent.project_id,
            intent.work_item_id,
            intent.command.actor_id,
            intent.attempt_id,
            ExecutionReviewResolution::Integrated,
            intent.command.timestamp_unix_ms,
        )?;
        insert_workspace_integration_intent(conn, intent)
    })
}

pub(crate) fn insert_workspace_integration_intent(
    conn: &mut diesel::sqlite::SqliteConnection,
    intent: &WorkspaceIntegrationIntent,
) -> Result<()> {
    let command_json = serde_json::to_string(&intent.command)?;
    diesel::sql_query(
        "INSERT INTO workspace_integration_intents
             (attempt_id, project_id, work_item_id, command_id, command_json,
              integration_plan_json, state, claim_owner, claim_expires_at_unix_ms,
              created_at_unix_ms, completed_at_unix_ms, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', NULL, NULL, ?7, NULL, NULL)
             ON CONFLICT(attempt_id) DO NOTHING",
    )
    .bind::<Text, _>(intent.attempt_id.0.to_string())
    .bind::<Text, _>(intent.project_id.0.to_string())
    .bind::<Text, _>(intent.work_item_id.0.to_string())
    .bind::<Text, _>(intent.command_id.0.to_string())
    .bind::<Text, _>(&command_json)
    .bind::<Text, _>(&intent.integration_plan_json)
    .bind::<BigInt, _>(intent.created_at_unix_ms)
    .execute(conn)
    .context("store workspace integration intent")?;
    let stored = load_workspace_integration_intent(conn, intent.attempt_id)?
        .context("workspace integration intent disappeared after insert")?;
    anyhow::ensure!(
        stored == *intent,
        "execution integration intent was reused with different content"
    );
    Ok(())
}

pub fn load_workspace_integration_intent(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
) -> Result<Option<WorkspaceIntegrationIntent>> {
    diesel::sql_query(
        "SELECT attempt_id, project_id, work_item_id, command_id, command_json,
                integration_plan_json, state, claim_owner, claim_expires_at_unix_ms, created_at_unix_ms,
                completed_at_unix_ms, last_error
         FROM workspace_integration_intents WHERE attempt_id = ?1",
    )
    .bind::<Text, _>(attempt_id.0.to_string())
    .load::<WorkspaceIntegrationIntentRow>(conn)
    .context("load workspace integration intent")?
    .into_iter()
    .next()
    .map(WorkspaceIntegrationIntentRow::decode)
    .transpose()
}

pub fn load_pending_workspace_integration_intents(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<WorkspaceIntegrationIntent>> {
    diesel::sql_query(
        "SELECT attempt_id, project_id, work_item_id, command_id, command_json,
                integration_plan_json, state, claim_owner, claim_expires_at_unix_ms, created_at_unix_ms,
                completed_at_unix_ms, last_error
         FROM workspace_integration_intents
         WHERE state IN ('prepared', 'applying', 'blocked') ORDER BY created_at_unix_ms",
    )
    .load::<WorkspaceIntegrationIntentRow>(conn)
    .context("load pending workspace integration intents")?
    .into_iter()
    .map(WorkspaceIntegrationIntentRow::decode)
    .collect()
}

/// Atomically lease one integration for the process that will inspect and, if
/// necessary, mutate the primary checkout. Expired leases are recoverable
/// after a process crash; a live owner cannot be displaced.
pub fn claim_workspace_integration(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
    owner: &str,
    now_unix_ms: i64,
    lease_duration_ms: i64,
) -> Result<Option<WorkspaceIntegrationIntent>> {
    anyhow::ensure!(!owner.trim().is_empty(), "integration claim owner is empty");
    anyhow::ensure!(
        lease_duration_ms > 0,
        "integration claim lease must be positive"
    );
    let expires = now_unix_ms.saturating_add(lease_duration_ms);
    conn.transaction::<_, anyhow::Error, _>(|conn| {
        diesel::sql_query(
            "UPDATE workspace_integration_intents
             SET state = 'applying', claim_owner = ?1, claim_expires_at_unix_ms = ?2,
                 last_error = NULL
             WHERE attempt_id = ?3
               AND (state = 'prepared'
                    OR (state = 'applying' AND claim_expires_at_unix_ms <= ?4)
                    OR (state = 'applying' AND claim_owner = ?1))",
        )
        .bind::<Text, _>(owner)
        .bind::<BigInt, _>(expires)
        .bind::<Text, _>(attempt_id.0.to_string())
        .bind::<BigInt, _>(now_unix_ms)
        .execute(conn)
        .context("claim workspace integration")?;
        let intent = load_workspace_integration_intent(conn, attempt_id)?;
        Ok(intent.filter(|intent| {
            intent.state == "applying" && intent.claim_owner.as_deref() == Some(owner)
        }))
    })
}

/// Return a successfully applied intent to its durable delivery state. The
/// exact canonical command is then safe to retry idempotently.
pub fn release_workspace_integration_claim(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
    owner: &str,
) -> Result<bool> {
    Ok(diesel::sql_query(
        "UPDATE workspace_integration_intents
         SET state = 'prepared', claim_owner = NULL, claim_expires_at_unix_ms = NULL,
             last_error = NULL
         WHERE attempt_id = ?1 AND state = 'applying' AND claim_owner = ?2",
    )
    .bind::<Text, _>(attempt_id.0.to_string())
    .bind::<Text, _>(owner)
    .execute(conn)
    .context("release workspace integration claim")?
        == 1)
}

/// Fail closed only if this worker still owns the lease. A stale worker may
/// not block a newer recovery attempt after its lease has been replaced.
pub fn block_claimed_workspace_integration(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
    owner: &str,
    reason: &str,
) -> Result<bool> {
    Ok(diesel::sql_query(
        "UPDATE workspace_integration_intents
         SET state = 'blocked', claim_owner = NULL, claim_expires_at_unix_ms = NULL,
             last_error = ?1
         WHERE attempt_id = ?2 AND state = 'applying' AND claim_owner = ?3",
    )
    .bind::<Text, _>(reason)
    .bind::<Text, _>(attempt_id.0.to_string())
    .bind::<Text, _>(owner)
    .execute(conn)
    .context("block claimed workspace integration intent")?
        == 1)
}

pub fn block_workspace_integration(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
    reason: &str,
) -> Result<bool> {
    Ok(diesel::sql_query(
        "UPDATE workspace_integration_intents
         SET state = 'blocked', claim_owner = NULL, claim_expires_at_unix_ms = NULL, last_error = ?1
         WHERE attempt_id = ?2 AND state = 'prepared'",
    )
    .bind::<Text, _>(reason)
    .bind::<Text, _>(attempt_id.0.to_string())
    .execute(conn)
    .context("block workspace integration intent")?
        > 0)
}

pub(crate) fn mark_workspace_integration_completed(
    conn: &mut diesel::sqlite::SqliteConnection,
    attempt_id: ExecutionAttemptId,
    command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
    completed_at_unix_ms: i64,
) -> Result<()> {
    let changed = diesel::sql_query(
        "UPDATE workspace_integration_intents
         SET state = 'completed', claim_owner = NULL, claim_expires_at_unix_ms = NULL,
             completed_at_unix_ms = ?1, last_error = NULL
         WHERE attempt_id = ?2 AND command_id = ?3 AND state = 'prepared'",
    )
    .bind::<BigInt, _>(completed_at_unix_ms)
    .bind::<Text, _>(attempt_id.0.to_string())
    .bind::<Text, _>(command_id.0.to_string())
    .execute(conn)
    .context("complete workspace integration intent")?;
    anyhow::ensure!(
        changed == 1,
        "Git integration intent was not available for completion"
    );
    Ok(())
}

pub(crate) fn next_canonical_sequence(
    conn: &mut diesel::sqlite::SqliteConnection,
    work_item_id: WorkItemId,
) -> Result<u64> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = BigInt)]
        sequence: i64,
    }
    let current = diesel::sql_query(
        "SELECT COALESCE(MAX(sequence), 0) AS sequence FROM canonical_workspace_events
         WHERE work_item_id = ?1",
    )
    .bind::<Text, _>(work_item_id.0.to_string())
    .get_result::<Row>(conn)
    .context("load canonical workspace sequence")?
    .sequence;
    let next = current
        .checked_add(1)
        .context("canonical workspace sequence exhausted")?;
    u64::try_from(next).context("negative canonical workspace sequence")
}

pub(crate) fn insert_canonical_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
    events: &[WorkspaceEventEnvelope],
) -> Result<()> {
    for event in events {
        let sequence =
            i64::try_from(event.sequence).context("canonical event sequence overflow")?;
        diesel::sql_query(
            "INSERT INTO canonical_workspace_events
             (event_id, project_id, work_item_id, sequence, envelope_json, created_at_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind::<Text, _>(event.event_id.0.to_string())
        .bind::<Text, _>(event.project_id.0.to_string())
        .bind::<Text, _>(event.work_item_id.0.to_string())
        .bind::<BigInt, _>(sequence)
        .bind::<Text, _>(serde_json::to_string(event)?)
        .bind::<BigInt, _>(event.timestamp_unix_ms)
        .execute(conn)
        .context("append canonical workspace event")?;
    }
    diesel::sql_query(
        "INSERT INTO canonical_workspace_command_outcomes
         (command_id, project_id, work_item_id, command_json, events_json, accepted_at_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind::<Text, _>(command.command_id.0.to_string())
    .bind::<Text, _>(command.project_id.0.to_string())
    .bind::<Text, _>(command.work_item_id.0.to_string())
    .bind::<Text, _>(serde_json::to_string(command)?)
    .bind::<Text, _>(serde_json::to_string(events)?)
    .bind::<BigInt, _>(command.timestamp_unix_ms)
    .execute(conn)
    .context("store canonical workspace command outcome")?;
    Ok(())
}
