//! Durable, metadata-only storage for the engineering workspace projection.

use crate::accept_local_workspace_command;
use crate::workspace_store::MAX_CANONICAL_REPLAY_EVENTS;
use crate::WorkspaceToolOutcome;
use anyhow::{Context, Result};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Nullable, Text};
use dragent_session_protocol::workspace::{
    WorkspaceCommandEnvelope, WorkspaceCommandPayload, WorkspaceEventEnvelope,
};
use dragent_workspace_model::{ActorRef, ProjectId, WorkItemId, WorkspaceEventKind};
use uuid::Uuid;

pub fn accept_local_post_message(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
) -> Result<Vec<WorkspaceEventEnvelope>> {
    if !matches!(
        &command.payload,
        WorkspaceCommandPayload::PostMessage { .. }
    ) {
        anyhow::bail!("local message authority only accepts post_message commands");
    }
    accept_local_workspace_command(conn, command)
}

/// Temporary migration bridge for the existing desktop projection. Keeping it
/// in the authority transaction closes the crash window between canonical
/// acceptance and the legacy restart UI. Canonical replay will replace this.
pub(crate) fn insert_compact_projection_events(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
    kinds: &[WorkspaceEventKind],
) -> Result<()> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = BigInt)]
        sequence: i64,
    }
    let first_sequence = diesel::sql_query(
        "SELECT COALESCE(MAX(sequence), -1) + 1 AS sequence FROM workspace_events
         WHERE project_id = ?1 AND work_item_id = ?2",
    )
    .bind::<Text, _>(command.project_id.0.to_string())
    .bind::<Text, _>(command.work_item_id.0.to_string())
    .get_result::<Row>(conn)
    .context("allocate compact workspace projection sequence")?
    .sequence;
    for (offset, kind) in kinds.iter().enumerate() {
        let offset = i64::try_from(offset).context("compact projection batch overflow")?;
        let sequence = first_sequence
            .checked_add(offset)
            .context("compact workspace sequence exhausted")?;
        diesel::sql_query(
            "INSERT INTO workspace_events
             (project_id, work_item_id, sequence, timestamp_unix_ms, kind_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind::<Text, _>(command.project_id.0.to_string())
        .bind::<Text, _>(command.work_item_id.0.to_string())
        .bind::<BigInt, _>(sequence)
        .bind::<BigInt, _>(command.timestamp_unix_ms)
        .bind::<Text, _>(serde_json::to_string(kind)?)
        .execute(conn)
        .context("append compact workspace projection event")?;
    }
    Ok(())
}

/// Load a validated, bounded canonical event page for one work item. The
/// exclusive cursor makes reconnect/restart replay deterministic without ever
/// loading another work item's history into the projection.
pub fn load_canonical_workspace_events(
    conn: &mut diesel::sqlite::SqliteConnection,
    project_id: ProjectId,
    work_item_id: WorkItemId,
    after_sequence: u64,
    limit: usize,
) -> Result<Vec<WorkspaceEventEnvelope>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        envelope_json: String,
    }

    if limit == 0 {
        return Ok(Vec::new());
    }
    let after_sequence =
        i64::try_from(after_sequence).context("canonical replay cursor overflow")?;
    let limit = i64::try_from(limit)
        .unwrap_or(i64::MAX)
        .min(MAX_CANONICAL_REPLAY_EVENTS);
    diesel::sql_query(
        "SELECT envelope_json FROM canonical_workspace_events
         WHERE project_id = ?1 AND work_item_id = ?2 AND sequence > ?3
         ORDER BY sequence ASC LIMIT ?4",
    )
    .bind::<Text, _>(project_id.0.to_string())
    .bind::<Text, _>(work_item_id.0.to_string())
    .bind::<BigInt, _>(after_sequence)
    .bind::<BigInt, _>(limit)
    .load::<Row>(conn)
    .context("load canonical workspace event page")?
    .into_iter()
    .map(|row| {
        let event: WorkspaceEventEnvelope =
            serde_json::from_str(&row.envelope_json).context("decode canonical workspace event")?;
        event
            .validate()
            .map_err(|error| anyhow::anyhow!("invalid stored canonical event: {error}"))?;
        if event.project_id != project_id || event.work_item_id != work_item_id {
            anyhow::bail!("canonical workspace event escaped its requested scope");
        }
        Ok(event)
    })
    .collect()
}

pub(crate) fn load_command_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
) -> Result<Option<Vec<WorkspaceEventEnvelope>>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        command_json: String,
        #[diesel(sql_type = Text)]
        events_json: String,
    }
    let row = diesel::sql_query(
        "SELECT command_json, events_json FROM canonical_workspace_command_outcomes
         WHERE command_id = ?1",
    )
    .bind::<Text, _>(command.command_id.0.to_string())
    .load::<Row>(conn)
    .context("load canonical workspace command outcome")?
    .into_iter()
    .next();
    let Some(row) = row else {
        return Ok(None);
    };
    let accepted: WorkspaceCommandEnvelope =
        serde_json::from_str(&row.command_json).context("decode accepted workspace command")?;
    if accepted != *command {
        anyhow::bail!("workspace command id was reused with different content");
    }
    let events = serde_json::from_str(&row.events_json)
        .context("decode canonical workspace command outcome")?;
    Ok(Some(events))
}

pub(crate) fn insert_workspace_tool_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    outcome: &WorkspaceToolOutcome,
) -> Result<()> {
    let round = i64::from(outcome.runtime_round);
    diesel::sql_query(
        "INSERT INTO workspace_tool_outcomes
         (command_id, project_id, work_item_id, runtime_run_id,
          runtime_execution_id, runtime_round, provider_call_id, output,
          is_error, source_message_id, accepted_at_unix_ms,
          delivered_at_unix_ms, terminal_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(command_id) DO NOTHING",
    )
    .bind::<Text, _>(outcome.command_id.0.to_string())
    .bind::<Text, _>(outcome.project_id.0.to_string())
    .bind::<Text, _>(outcome.work_item_id.0.to_string())
    .bind::<Text, _>(&outcome.runtime_run_id)
    .bind::<Text, _>(&outcome.runtime_execution_id)
    .bind::<BigInt, _>(round)
    .bind::<Text, _>(&outcome.provider_call_id)
    .bind::<Nullable<Text>, _>(outcome.output.as_deref())
    .bind::<Nullable<Integer>, _>(outcome.is_error.map(i32::from))
    .bind::<Nullable<Text>, _>(outcome.source_message_id.map(|id| id.to_string()))
    .bind::<BigInt, _>(outcome.accepted_at_unix_ms)
    .bind::<Nullable<BigInt>, _>(outcome.delivered_at_unix_ms)
    .bind::<Nullable<Text>, _>(outcome.terminal_reason.as_deref())
    .execute(conn)
    .context("store workspace tool outcome")?;

    let stored = load_workspace_tool_outcome(conn, outcome.command_id)?
        .context("workspace tool outcome disappeared after insert")?;
    let same_identity = stored.command_id == outcome.command_id
        && stored.project_id == outcome.project_id
        && stored.work_item_id == outcome.work_item_id
        && stored.runtime_run_id == outcome.runtime_run_id
        && stored.runtime_execution_id == outcome.runtime_execution_id
        && stored.runtime_round == outcome.runtime_round
        && stored.provider_call_id == outcome.provider_call_id
        && stored.source_message_id == outcome.source_message_id
        && stored.accepted_at_unix_ms == outcome.accepted_at_unix_ms;
    let compatible_result = outcome.output.is_none()
        || (stored.output == outcome.output && stored.is_error == outcome.is_error);
    if !same_identity || !compatible_result {
        anyhow::bail!("workspace command outcome was reused for a different provider tool call");
    }
    Ok(())
}

pub(crate) fn resolve_workspace_tool_outcomes_from_command(
    conn: &mut diesel::sqlite::SqliteConnection,
    command: &WorkspaceCommandEnvelope,
) -> Result<()> {
    let WorkspaceCommandPayload::PostMessage { message, .. } = &command.payload else {
        return Ok(());
    };
    let Some(reply_to) = message.reply_to else {
        return Ok(());
    };
    // An escalation is not an answer. Keep the durable call parked until the
    // Principal records the actual decision (including a relayed user answer).
    let principal = ActorRef::principal_agent(command.project_id).id;
    if command.actor_id != principal
        || message.sender_id != principal
        || message.work_item_id != command.work_item_id
        || message.body.trim().is_empty()
        || message.body.trim() == "Escalated to the user for a decision."
    {
        return Ok(());
    }
    diesel::sql_query(
        "UPDATE workspace_tool_outcomes
         SET output = ?1, is_error = 0
         WHERE source_message_id = ?2
           AND project_id = ?3 AND work_item_id = ?4
           AND output IS NULL
           AND delivered_at_unix_ms IS NULL
           AND terminal_reason IS NULL",
    )
    .bind::<Text, _>(&message.body)
    .bind::<Text, _>(reply_to.0.to_string())
    .bind::<Text, _>(command.project_id.0.to_string())
    .bind::<Text, _>(command.work_item_id.0.to_string())
    .execute(conn)
    .context("resolve workspace tool outcome from reply")?;
    Ok(())
}

#[derive(QueryableByName)]
struct WorkspaceToolOutcomeRow {
    #[diesel(sql_type = Text)]
    command_id: String,
    #[diesel(sql_type = Text)]
    project_id: String,
    #[diesel(sql_type = Text)]
    work_item_id: String,
    #[diesel(sql_type = Text)]
    runtime_run_id: String,
    #[diesel(sql_type = Text)]
    runtime_execution_id: String,
    #[diesel(sql_type = BigInt)]
    runtime_round: i64,
    #[diesel(sql_type = Text)]
    provider_call_id: String,
    #[diesel(sql_type = Nullable<Text>)]
    output: Option<String>,
    #[diesel(sql_type = Nullable<Integer>)]
    is_error: Option<i32>,
    #[diesel(sql_type = Nullable<Text>)]
    source_message_id: Option<String>,
    #[diesel(sql_type = BigInt)]
    accepted_at_unix_ms: i64,
    #[diesel(sql_type = Nullable<BigInt>)]
    delivered_at_unix_ms: Option<i64>,
    #[diesel(sql_type = Nullable<Text>)]
    terminal_reason: Option<String>,
}

impl WorkspaceToolOutcomeRow {
    fn decode(self) -> Result<WorkspaceToolOutcome> {
        Ok(WorkspaceToolOutcome {
            command_id: dragent_session_protocol::workspace::WorkspaceCommandId(Uuid::parse_str(
                &self.command_id,
            )?),
            project_id: ProjectId(Uuid::parse_str(&self.project_id)?),
            work_item_id: WorkItemId(Uuid::parse_str(&self.work_item_id)?),
            runtime_run_id: self.runtime_run_id,
            runtime_execution_id: self.runtime_execution_id,
            runtime_round: u32::try_from(self.runtime_round)
                .context("workspace tool outcome has invalid runtime round")?,
            provider_call_id: self.provider_call_id,
            output: self.output,
            is_error: self.is_error.map(|value| value != 0),
            source_message_id: self
                .source_message_id
                .map(|value| Uuid::parse_str(&value))
                .transpose()?,
            accepted_at_unix_ms: self.accepted_at_unix_ms,
            delivered_at_unix_ms: self.delivered_at_unix_ms,
            terminal_reason: self.terminal_reason,
        })
    }
}

fn load_workspace_tool_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
) -> Result<Option<WorkspaceToolOutcome>> {
    diesel::sql_query(
        "SELECT command_id, project_id, work_item_id, runtime_run_id,
                runtime_execution_id, runtime_round, provider_call_id, output,
                is_error, source_message_id, accepted_at_unix_ms,
                delivered_at_unix_ms, terminal_reason
         FROM workspace_tool_outcomes WHERE command_id = ?1",
    )
    .bind::<Text, _>(command_id.0.to_string())
    .load::<WorkspaceToolOutcomeRow>(conn)
    .context("load workspace tool outcome")?
    .into_iter()
    .next()
    .map(WorkspaceToolOutcomeRow::decode)
    .transpose()
}

pub fn load_pending_workspace_tool_outcomes(
    conn: &mut diesel::sqlite::SqliteConnection,
) -> Result<Vec<WorkspaceToolOutcome>> {
    diesel::sql_query(
        "SELECT command_id, project_id, work_item_id, runtime_run_id,
                runtime_execution_id, runtime_round, provider_call_id, output,
                is_error, source_message_id, accepted_at_unix_ms,
                delivered_at_unix_ms, terminal_reason
         FROM workspace_tool_outcomes
         WHERE delivered_at_unix_ms IS NULL
           AND terminal_reason IS NULL
         ORDER BY accepted_at_unix_ms ASC",
    )
    .load::<WorkspaceToolOutcomeRow>(conn)
    .context("load pending workspace tool outcomes")?
    .into_iter()
    .map(WorkspaceToolOutcomeRow::decode)
    .collect()
}

pub fn mark_workspace_tool_outcome_delivered(
    conn: &mut diesel::sqlite::SqliteConnection,
    command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
    runtime_execution_id: &str,
    provider_call_id: &str,
    delivered_at_unix_ms: i64,
) -> Result<bool> {
    Ok(diesel::sql_query(
        "UPDATE workspace_tool_outcomes SET delivered_at_unix_ms = ?1
         WHERE command_id = ?2 AND runtime_execution_id = ?3
           AND provider_call_id = ?4 AND output IS NOT NULL
           AND delivered_at_unix_ms IS NULL AND terminal_reason IS NULL",
    )
    .bind::<BigInt, _>(delivered_at_unix_ms)
    .bind::<Text, _>(command_id.0.to_string())
    .bind::<Text, _>(runtime_execution_id)
    .bind::<Text, _>(provider_call_id)
    .execute(conn)
    .context("mark workspace tool outcome delivered")?
        > 0)
}

pub fn park_workspace_tool_outcome(
    conn: &mut diesel::sqlite::SqliteConnection,
    command_id: dragent_session_protocol::workspace::WorkspaceCommandId,
    runtime_execution_id: &str,
    reason: &str,
) -> Result<bool> {
    Ok(diesel::sql_query(
        "UPDATE workspace_tool_outcomes SET terminal_reason = ?1
         WHERE command_id = ?2 AND runtime_execution_id = ?3
           AND delivered_at_unix_ms IS NULL AND terminal_reason IS NULL",
    )
    .bind::<Text, _>(reason)
    .bind::<Text, _>(command_id.0.to_string())
    .bind::<Text, _>(runtime_execution_id)
    .execute(conn)
    .context("park workspace tool outcome")?
        > 0)
}

pub fn park_workspace_tool_outcomes_for_run(
    conn: &mut diesel::sqlite::SqliteConnection,
    runtime_run_id: &str,
    runtime_execution_id: &str,
    reason: &str,
) -> Result<usize> {
    diesel::sql_query(
        "UPDATE workspace_tool_outcomes SET terminal_reason = ?1
         WHERE runtime_run_id = ?2 AND runtime_execution_id = ?3
           AND delivered_at_unix_ms IS NULL AND terminal_reason IS NULL",
    )
    .bind::<Text, _>(reason)
    .bind::<Text, _>(runtime_run_id)
    .bind::<Text, _>(runtime_execution_id)
    .execute(conn)
    .context("park workspace tool outcomes for run")
}
