//! Pure selection logic for when the Principal's coordination conversation
//! owes a turn.
//!
//! Shaping is a real, live Principal conversation. Coordination must be one
//! too: every worker `Blocker`/`DecisionRequest` routes through the Principal,
//! always, with no bypass straight to a human `AttentionItem`. This module
//! answers only "is there an unanswered message the Principal must act on" —
//! it never spawns anything and never talks to a provider. The impure driver
//! that turns this into an actual model call lives in
//! `app/src/workspace/view.rs` (`continue_principal_coordination`), mirroring
//! how `next_workspace_launch_action` (execution scheduling) and
//! `continue_workspace_launch` (its impure driver) are split.

use dragent_workspace_model::{
    ActorId, ActorRef, AgentMessage, AgentMessageId, AgentMessageKind, AgentMessageRouting,
    ExecutionAttemptId, WorkStatus, WorkspaceState,
};

/// How long an exhausted-automatic-retry's Blocker may sit unanswered before
/// the hardcoded human-escalation fallback fires anyway. Coordination must
/// never go silently stuck forever, even if the Principal-coordination driver
/// itself has a bug.
pub const PRINCIPAL_COORDINATION_ESCALATION_TIMEOUT_MS: i64 = 15 * 60 * 1_000;

fn is_answered(
    messages: &[(&AgentMessage, &AgentMessageRouting)],
    principal_id: ActorId,
    message_id: AgentMessageId,
) -> bool {
    messages.iter().any(|(message, _)| {
        message.sender_id == principal_id && message.reply_to == Some(message_id)
    })
}

fn is_open_principal_request(
    message: &AgentMessage,
    routing: &AgentMessageRouting,
    principal_id: ActorId,
) -> bool {
    // Handoffs are included so the Principal sees every worker→worker
    // result as it happens (review it, record what was learned, redirect if
    // wrong) instead of only hearing about problems. `Discussion` is how the
    // human chats with the Principal at work-item level — not every human
    // message is a decision request, and all of them deserve an answer.
    (matches!(
        message.kind,
        AgentMessageKind::Blocker
            | AgentMessageKind::DecisionRequest
            | AgentMessageKind::Handoff
            | AgentMessageKind::Discussion
    ) || message
        .review_finding
        .as_ref()
        .is_some_and(|finding| finding.verdict == dragent_workspace_model::ReviewVerdict::Blocked))
        && message.sender_id != principal_id
        && routing.recipient_ids.contains(&principal_id)
}

/// Is there an unanswered coordination message the Principal owes a reply to?
/// Pure, no I/O — mirrors `next_workspace_launch_action`'s contract exactly:
/// same state in, same answer out, safe to call on every reactive tick.
///
/// Gated on `Executing | Paused`, not just `Executing`: pausing stops new
/// scheduling, but must not also freeze the one thing that could unstick a
/// paused work item's open blocker. This selector only reads and will only
/// ever cause a message to be posted — it never spawns a new worker session.
pub fn next_principal_coordination_message(
    state: &WorkspaceState,
) -> Option<(&AgentMessage, &AgentMessageRouting)> {
    next_principal_coordination_message_excluding(state, &std::collections::HashSet::new())
}

/// `next_principal_coordination_message` skipping `excluded` — messages the
/// host already answered this session whose reply the authority rejected.
/// Without this, a rejected reply leaves the message "unanswered" and the
/// selector re-offers it on every tick: a paid provider turn per tick and
/// the same question answered over and over in the conversation.
pub fn next_principal_coordination_message_excluding<'a>(
    state: &'a WorkspaceState,
    excluded: &std::collections::HashSet<AgentMessageId>,
) -> Option<(&'a AgentMessage, &'a AgentMessageRouting)> {
    let work = state.active_work()?;
    if !matches!(work.status, WorkStatus::Executing | WorkStatus::Paused) {
        return None;
    }
    let principal_id = ActorRef::principal_agent(state.project().id).id;
    let local_user_id = ActorRef::local_user(state.project().id, None).id;
    let messages = state.agent_messages();
    // The human's own messages come first: they must never wait behind a
    // backlog of worker traffic to get an answer. Within a class, oldest.
    messages
        .iter()
        .copied()
        .filter(|(message, routing)| {
            is_open_principal_request(message, routing, principal_id)
                && !excluded.contains(&message.id)
                && !is_answered(&messages, principal_id, message.id)
        })
        .min_by_key(|(message, _)| {
            (
                message.sender_id != local_user_id,
                message.created_at_unix_ms,
                message.id.0,
            )
        })
}

/// Scoped variant of the same question for one specific execution attempt:
/// is *its* Blocker/DecisionRequest to the Principal still open? Used by the
/// exhausted-automatic-retry escalation path to decide whether the hardcoded
/// human-escalation `AttentionItem` should defer to a live Principal turn
/// first, rather than bypassing the Principal unconditionally.
pub fn principal_coordination_message_for_attempt(
    state: &WorkspaceState,
    attempt_id: ExecutionAttemptId,
) -> Option<(&AgentMessage, &AgentMessageRouting)> {
    let principal_id = ActorRef::principal_agent(state.project().id).id;
    let messages = state.agent_messages();
    messages
        .iter()
        .copied()
        .filter(|(message, routing)| {
            routing.execution_attempt_id == Some(attempt_id)
                && is_open_principal_request(message, routing, principal_id)
                && !is_answered(&messages, principal_id, message.id)
        })
        .min_by_key(|(message, _)| (message.created_at_unix_ms, message.id.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_workspace_model::{
        MessageDeliveryState, Project, ProjectId, ThreadId, WorkItem, WorkItemId, WorkspaceEvent,
        WorkspaceEventKind,
    };
    use uuid::Uuid;

    fn state_with_work(
        project_id: ProjectId,
        status: WorkStatus,
        messages: Vec<(AgentMessage, AgentMessageRouting)>,
    ) -> WorkspaceState {
        let project = Project {
            organisation_id: "local".into(),
            id: project_id,
            root: std::path::PathBuf::from("/workspace/principal-coordination-test"),
            display_name: "test".into(),
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let work = WorkItem {
            id: WorkItemId::new(),
            project_id: project.id,
            title: "Test work".into(),
            status,
            created_at_unix_ms: 0,
            updated_at_unix_ms: 0,
        };
        let events = messages
            .into_iter()
            .enumerate()
            .map(|(index, (message, routing))| WorkspaceEvent {
                sequence: index as u64 + 1,
                timestamp_unix_ms: message.created_at_unix_ms,
                project_id: project.id,
                work_item_id: Some(work.id),
                kind: WorkspaceEventKind::AgentMessagePosted { message, routing },
            })
            .collect::<Vec<_>>();
        WorkspaceState::restore(project, Some(work), events)
    }

    fn message(
        work_item_id: WorkItemId,
        principal_id: ActorId,
        sender_id: ActorId,
        kind: AgentMessageKind,
        reply_to: Option<AgentMessageId>,
        created_at_unix_ms: i64,
    ) -> (AgentMessage, AgentMessageRouting) {
        (
            AgentMessage {
                id: AgentMessageId::new(),
                thread_id: ThreadId::new(),
                work_item_id,
                sender_id,
                kind,
                body: "test".into(),
                review_finding: None,
                reply_to,
                artifact_ids: Vec::new(),
                evidence_ids: Vec::new(),
                delivery_state: MessageDeliveryState::LocalOnly,
                created_at_unix_ms,
                updated_at_unix_ms: created_at_unix_ms,
            },
            AgentMessageRouting {
                recipient_ids: vec![principal_id],
                task_id: None,
                execution_attempt_id: None,
            },
        )
    }

    #[test]
    fn selects_nothing_outside_executing_or_paused() {
        let state = state_with_work(ProjectId::new(), WorkStatus::Planning, Vec::new());
        assert!(next_principal_coordination_message(&state).is_none());
    }

    #[test]
    fn blocked_review_goes_to_principal_but_a_passing_review_does_not() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let (mut review, routing) = message(
            WorkItemId::new(),
            principal_id,
            ActorId::new(),
            AgentMessageKind::ReviewFinding,
            None,
            10,
        );
        review.review_finding = Some(dragent_workspace_model::StructuredReviewFinding {
            candidate_attempt_id: ExecutionAttemptId::new(),
            verdict: dragent_workspace_model::ReviewVerdict::Blocked,
            summary: "Need the existing test fixture location".into(),
            findings: vec![],
        });
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(review.clone(), routing.clone())],
        );
        assert!(next_principal_coordination_message(&state).is_some());
        review.review_finding.as_mut().unwrap().verdict =
            dragent_workspace_model::ReviewVerdict::Pass;
        let state = state_with_work(project_id, WorkStatus::Executing, vec![(review, routing)]);
        assert!(next_principal_coordination_message(&state).is_none());
    }

    #[test]
    fn selects_oldest_unanswered_blocker_or_decision_request() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let worker_id = ActorId(Uuid::new_v4());
        let work_item_id = WorkItemId::new();
        let (later, later_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Blocker,
            None,
            200,
        );
        let (earlier, earlier_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::DecisionRequest,
            None,
            100,
        );
        let earlier_id = earlier.id;
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(later, later_routing), (earlier, earlier_routing)],
        );

        let selected = next_principal_coordination_message(&state).expect("selection");
        assert_eq!(selected.0.id, earlier_id);
    }

    #[test]
    fn human_messages_are_answered_before_older_worker_requests() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let local_user_id = ActorRef::local_user(project_id, None).id;
        let worker_id = ActorId(Uuid::new_v4());
        let work_item_id = WorkItemId::new();
        let (blocker, blocker_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Blocker,
            None,
            100,
        );
        let (chat, chat_routing) = message(
            work_item_id,
            principal_id,
            local_user_id,
            AgentMessageKind::Discussion,
            None,
            200,
        );
        let chat_id = chat.id;
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(blocker, blocker_routing), (chat, chat_routing)],
        );

        let selected = next_principal_coordination_message(&state).expect("selection");
        assert_eq!(selected.0.id, chat_id);
    }

    #[test]
    fn selects_worker_handoffs_but_not_status_updates() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let worker_id = ActorId(Uuid::new_v4());
        let work_item_id = WorkItemId::new();
        let (status, status_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::StatusUpdate,
            None,
            100,
        );
        let (handoff, handoff_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Handoff,
            None,
            200,
        );
        let handoff_id = handoff.id;
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(status, status_routing), (handoff, handoff_routing)],
        );

        let selected = next_principal_coordination_message(&state).expect("selection");
        assert_eq!(selected.0.id, handoff_id);
    }

    #[test]
    fn excludes_already_answered_messages() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let worker_id = ActorId(Uuid::new_v4());
        let work_item_id = WorkItemId::new();
        let (blocker, routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Blocker,
            None,
            100,
        );
        let blocker_id = blocker.id;
        let (reply, reply_routing) = message(
            work_item_id,
            principal_id,
            principal_id,
            AgentMessageKind::Discussion,
            Some(blocker_id),
            150,
        );
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(blocker, routing), (reply, reply_routing)],
        );

        assert!(next_principal_coordination_message(&state).is_none());
    }

    #[test]
    fn excludes_messages_the_principal_itself_sent() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let work_item_id = WorkItemId::new();
        let (own_message, routing) = message(
            work_item_id,
            principal_id,
            principal_id,
            AgentMessageKind::Blocker,
            None,
            100,
        );
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(own_message, routing)],
        );

        assert!(next_principal_coordination_message(&state).is_none());
    }

    #[test]
    fn scoped_lookup_matches_only_its_own_attempt() {
        let project_id = ProjectId::new();
        let principal_id = ActorRef::principal_agent(project_id).id;
        let worker_id = ActorId(Uuid::new_v4());
        let work_item_id = WorkItemId::new();
        let attempt_id = ExecutionAttemptId::new();
        let other_attempt_id = ExecutionAttemptId::new();
        let (matching, mut matching_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Blocker,
            None,
            100,
        );
        matching_routing.execution_attempt_id = Some(attempt_id);
        let matching_id = matching.id;
        let (other, mut other_routing) = message(
            work_item_id,
            principal_id,
            worker_id,
            AgentMessageKind::Blocker,
            None,
            50,
        );
        other_routing.execution_attempt_id = Some(other_attempt_id);
        let state = state_with_work(
            project_id,
            WorkStatus::Executing,
            vec![(matching, matching_routing), (other, other_routing)],
        );

        let selected =
            principal_coordination_message_for_attempt(&state, attempt_id).expect("selection");
        assert_eq!(selected.0.id, matching_id);
        assert!(
            principal_coordination_message_for_attempt(&state, ExecutionAttemptId::new()).is_none()
        );
    }
}
