//! Versioned wire contract for synchronized agentic-workspace state.
//! (Submodule of the `workspace` hub: protocol error + shared validators.)

use std::collections::HashSet;
use std::fmt;

use dragent_workspace_model::{AgentMessage, AgentMessageKind, AgentMessageRouting};

use super::workspace::WORKSPACE_PROTOCOL_VERSION;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceProtocolError {
    UnsupportedSchemaVersion { received: u32, supported: u32 },
    NilIdentifier(&'static str),
    InvalidTimestamp,
    InvalidSequence,
    PayloadWorkItemMismatch,
    PayloadTaskMismatch,
    PayloadActorMismatch,
    ExpectedRevisionRequired,
    EmptyField(&'static str),
    InvalidPayload(String),
}

impl fmt::Display for WorkspaceProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for WorkspaceProtocolError {}

pub(crate) fn validate_common(
    schema_version: u32,
    project_id: Uuid,
    actor_id: Uuid,
    correlation_id: Uuid,
    timestamp_unix_ms: i64,
) -> Result<(), WorkspaceProtocolError> {
    if schema_version != WORKSPACE_PROTOCOL_VERSION {
        return Err(WorkspaceProtocolError::UnsupportedSchemaVersion {
            received: schema_version,
            supported: WORKSPACE_PROTOCOL_VERSION,
        });
    }
    for (field, id) in [
        ("project_id", project_id),
        ("actor_id", actor_id),
        ("correlation_id", correlation_id),
    ] {
        if id.is_nil() {
            return Err(WorkspaceProtocolError::NilIdentifier(field));
        }
    }
    if timestamp_unix_ms <= 0 {
        return Err(WorkspaceProtocolError::InvalidTimestamp);
    }
    Ok(())
}

pub(crate) fn require_text(field: &'static str, value: &str) -> Result<(), WorkspaceProtocolError> {
    if value.trim().is_empty() {
        Err(WorkspaceProtocolError::EmptyField(field))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_message(message: &AgentMessage) -> Result<(), WorkspaceProtocolError> {
    require_text("message body", &message.body)?;
    for (field, id) in [
        ("message id", message.id.0),
        ("message thread id", message.thread_id.0),
        ("message sender id", message.sender_id.0),
    ] {
        if id.is_nil() {
            return Err(WorkspaceProtocolError::InvalidPayload(format!(
                "{field} must not be nil"
            )));
        }
    }
    if message.created_at_unix_ms <= 0 || message.updated_at_unix_ms <= 0 {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "message timestamps must be positive".into(),
        ));
    }
    if message.reply_to.is_some_and(|id| id.0.is_nil())
        || message.artifact_ids.iter().any(|id| id.0.is_nil())
        || message.evidence_ids.iter().any(|id| id.0.is_nil())
    {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "message references must not contain nil identifiers".into(),
        ));
    }
    match (&message.kind, &message.review_finding) {
        (AgentMessageKind::ReviewFinding, Some(finding)) => {
            require_text("review finding summary", &finding.summary)?;
            if finding.candidate_attempt_id.0.is_nil()
                || (finding.verdict == dragent_workspace_model::ReviewVerdict::NeedsRevision
                    && finding.findings.is_empty())
            {
                return Err(WorkspaceProtocolError::InvalidPayload(
                    "review finding requires a candidate and actionable findings".into(),
                ));
            }
            for item in &finding.findings {
                require_text("review finding item summary", &item.summary)?;
                if item.relative_path.as_ref().is_some_and(|path| {
                    path.is_absolute()
                        || path
                            .components()
                            .any(|component| matches!(component, std::path::Component::ParentDir))
                }) || item.line == Some(0)
                {
                    return Err(WorkspaceProtocolError::InvalidPayload(
                        "review finding locations must be safe project-relative positions".into(),
                    ));
                }
            }
        }
        (AgentMessageKind::ReviewFinding, None) => {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "review finding messages require structured findings".into(),
            ));
        }
        (_, Some(_)) => {
            return Err(WorkspaceProtocolError::InvalidPayload(
                "structured findings are only valid on review finding messages".into(),
            ));
        }
        (_, None) => {}
    }
    Ok(())
}

pub(crate) fn validate_message_routing(
    message: &AgentMessage,
    routing: Option<&AgentMessageRouting>,
) -> Result<(), WorkspaceProtocolError> {
    let operational = matches!(
        message.kind,
        AgentMessageKind::Handoff
            | AgentMessageKind::Blocker
            | AgentMessageKind::DecisionRequest
            | AgentMessageKind::ReviewFinding
            | AgentMessageKind::RevisionRequest
            | AgentMessageKind::Approval
    );
    if !operational && routing.is_none() {
        return Ok(());
    }
    let routing = routing.ok_or_else(|| {
        WorkspaceProtocolError::InvalidPayload(
            "operational agent messages require routing metadata".into(),
        )
    })?;
    let recipients = routing
        .recipient_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    if recipients.is_empty()
        || recipients.len() != routing.recipient_ids.len()
        || recipients.contains(&message.sender_id)
        || recipients.iter().any(|id| id.0.is_nil())
    {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "agent message recipients must be distinct, non-empty, and exclude the sender".into(),
        ));
    }
    // Every operational kind but `DecisionRequest` is always about exactly
    // one task (a worker's Blocker/Handoff/ReviewFinding/RevisionRequest/
    // Approval never exists independent of the attempt that raised it).
    // `DecisionRequest` is also how a human addresses a work-item-level
    // question to the Principal while agents are running ("which agent is
    // working right now?") — genuinely not about any single task, so it's
    // the one operational kind allowed to omit `task_id`.
    let task_id_required = operational && message.kind != AgentMessageKind::DecisionRequest;
    if routing.task_id.is_some_and(|id| id.0.is_nil())
        || routing.execution_attempt_id.is_some_and(|id| id.0.is_nil())
        || (task_id_required && routing.task_id.is_none())
        || (message.kind == AgentMessageKind::Approval && message.reply_to.is_none())
    {
        return Err(WorkspaceProtocolError::InvalidPayload(
            "agent message routing does not match its message kind".into(),
        ));
    }
    Ok(())
}
