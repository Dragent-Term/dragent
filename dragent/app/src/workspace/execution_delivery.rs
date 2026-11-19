//! Workspace execution delivery helpers.
//!
//! Responsible for:
//! - projecting workspace events into canonical WorkspaceEvent records
//! - validating runtime submission scope for tool delivery
//! - collecting failed worker-question submissions for retry/escalation
//! - extracting Principal replies from delivery outcomes
//! - recording tool-delivery results into pending acknowledgement state
//!
//! Not responsible for:
//! - driving execution (WorkspaceView launch/coordination methods)
//! - rendering delivery state
//!
//! All functions here are free functions with no View dependencies.

use std::collections::HashMap;

use dragent_session_protocol::workspace::{
    WorkspaceCommandId, WorkspaceEventEnvelope, WorkspaceEventPayload,
};
use dragent_workspace_model::{
    ActorRef, AgentMessage as WorkspaceAgentMessage, AgentMessageId, WorkItemId, WorkspaceEvent,
    WorkspaceEventKind,
};

use super::view::{PendingWorkspaceToolAcknowledgement, PRINCIPAL_ESCALATED_REPLY};

pub(crate) fn validate_runtime_submission_scope(
    pending: bool,
    active_work_item_id: WorkItemId,
    command_work_item_ids: &[WorkItemId],
) -> Result<WorkItemId, String> {
    if pending {
        return Err(
            "Workspace state is being committed; this command was not submitted. Retry shortly."
                .into(),
        );
    }
    let submitted_work_item_id = command_work_item_ids
        .first()
        .copied()
        .ok_or_else(|| "No workspace command was provided.".to_string())?;
    if command_work_item_ids
        .iter()
        .any(|work_item_id| *work_item_id != submitted_work_item_id)
    {
        return Err("A runtime command batch cannot cross work items.".into());
    }
    if submitted_work_item_id != active_work_item_id {
        return Err("The target work item is no longer active; switch back and retry.".into());
    }
    Ok(submitted_work_item_id)
}

pub(crate) fn take_failed_worker_question_submissions(
    command_ids: &[WorkspaceCommandId],
    succeeded: bool,
    submissions: &mut HashMap<WorkspaceCommandId, AgentMessageId>,
) -> Vec<AgentMessageId> {
    command_ids
        .iter()
        .filter_map(|command_id| submissions.remove(command_id))
        .filter(|_| !succeeded)
        .collect()
}

pub(crate) fn principal_replies_for_delivery(
    events: &[dragent_session_protocol::workspace::WorkspaceEventEnvelope],
) -> Vec<WorkspaceAgentMessage> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            WorkspaceEventPayload::MessagePosted { message, .. }
                if message.sender_id == ActorRef::principal_agent(event.project_id).id
                    && message.body != PRINCIPAL_ESCALATED_REPLY =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn record_workspace_tool_delivery_result(
    pending: &mut HashMap<WorkspaceCommandId, PendingWorkspaceToolAcknowledgement>,
    command_id: WorkspaceCommandId,
    delivered: bool,
) {
    if delivered {
        pending.remove(&command_id);
    } else if let Some(pending) = pending.get_mut(&command_id) {
        pending.delivery_in_flight = false;
    }
}

#[cfg(test)]
mod runtime_submission_tests {
    use super::{
        principal_replies_for_delivery, record_workspace_tool_delivery_result,
        take_failed_worker_question_submissions, validate_runtime_submission_scope,
        PendingWorkspaceToolAcknowledgement, PRINCIPAL_ESCALATED_REPLY,
    };
    use dragent_session_protocol::workspace::{
        CorrelationId, WorkspaceCommandId, WorkspaceEventEnvelope, WorkspaceEventId,
        WorkspaceEventPayload, WORKSPACE_PROTOCOL_VERSION,
    };
    use dragent_workspace_model::{
        ActorKind, ActorRef, AgentMessage, AgentMessageId, AgentMessageKind, MessageDeliveryState,
        ProjectId, ThreadId, WorkItemId,
    };
    use std::collections::HashMap;

    #[test]
    fn in_flight_submission_is_explicitly_retryable() {
        let work_item_id = WorkItemId::new();
        let error = validate_runtime_submission_scope(true, work_item_id, &[work_item_id])
            .expect_err("an in-flight mutation must reject another submission");
        assert!(error.contains("not submitted"));
        assert!(error.contains("Retry"));
    }

    #[test]
    fn submission_scope_rejects_cross_work_and_stale_tabs() {
        let active = WorkItemId::new();
        let other = WorkItemId::new();
        assert!(
            validate_runtime_submission_scope(false, active, &[active, other])
                .unwrap_err()
                .contains("cannot cross work items")
        );
        assert!(validate_runtime_submission_scope(false, active, &[other])
            .unwrap_err()
            .contains("no longer active"));
        assert_eq!(
            validate_runtime_submission_scope(false, active, &[active, active]).unwrap(),
            active
        );
    }

    #[test]
    fn failed_question_submission_is_removed_and_returned_for_error_delivery() {
        let command_id = WorkspaceCommandId::new();
        let message_id = AgentMessageId::new();
        let mut submissions = HashMap::from([(command_id, message_id)]);
        assert_eq!(
            take_failed_worker_question_submissions(&[command_id], false, &mut submissions,),
            vec![message_id]
        );
        assert!(submissions.is_empty());

        let mut accepted = HashMap::from([(command_id, message_id)]);
        assert!(
            take_failed_worker_question_submissions(&[command_id], true, &mut accepted,).is_empty()
        );
        assert!(accepted.is_empty());
    }

    #[test]
    fn parked_completion_keeps_principal_reply_for_worker_delivery() {
        let project_id = ProjectId::new();
        let work_item_id = WorkItemId::new();
        let principal = ActorRef::principal_agent(project_id);
        let event = |body: &str, sender_id| WorkspaceEventEnvelope {
            event_id: WorkspaceEventId::new(),
            schema_version: WORKSPACE_PROTOCOL_VERSION,
            sequence: 1,
            project_id,
            work_item_id,
            actor_id: sender_id,
            device_id: None,
            task_id: None,
            correlation_id: CorrelationId::new(),
            causation_id: None,
            timestamp_unix_ms: 1,
            payload: WorkspaceEventPayload::MessagePosted {
                message: AgentMessage {
                    id: AgentMessageId::new(),
                    thread_id: ThreadId::new(),
                    work_item_id,
                    sender_id,
                    kind: AgentMessageKind::StatusUpdate,
                    body: body.into(),
                    review_finding: None,
                    reply_to: None,
                    artifact_ids: Vec::new(),
                    evidence_ids: Vec::new(),
                    delivery_state: MessageDeliveryState::LocalOnly,
                    created_at_unix_ms: 1,
                    updated_at_unix_ms: 1,
                },
                routing: None,
            },
        };
        let worker = ActorRef::agent_instance(
            dragent_workspace_model::AgentInstanceId::new(),
            ActorKind::WorkerAgent,
        );
        let events = vec![
            event("answer", principal.id),
            event(PRINCIPAL_ESCALATED_REPLY, principal.id),
            event("worker update", worker.id),
        ];
        let replies = principal_replies_for_delivery(&events);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body, "answer");
    }

    #[test]
    fn acknowledgement_is_retained_until_delivery_succeeds() {
        let command_id = WorkspaceCommandId::new();
        let pending = PendingWorkspaceToolAcknowledgement {
            runtime_task_id: "local-run".into(),
            runtime_execution_id: "execution".into(),
            runtime_round: 1,
            call_id: "call".into(),
            success: "saved".into(),
            resolved: Some(("saved".into(), false)),
            delivery_in_flight: true,
            durable: true,
            delivery_attempts: 0,
        };
        let mut acknowledgements = HashMap::from([(command_id, pending)]);

        record_workspace_tool_delivery_result(&mut acknowledgements, command_id, false);
        assert!(!acknowledgements[&command_id].delivery_in_flight);

        record_workspace_tool_delivery_result(&mut acknowledgements, command_id, true);
        assert!(!acknowledgements.contains_key(&command_id));
    }
}

pub(crate) fn principal_workspace_run_id(work_item_id: WorkItemId) -> String {
    format!("local-workspace-principal-{}", work_item_id.0.simple())
}
