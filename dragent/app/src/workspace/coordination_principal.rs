//! Coordination-turn prompt builder and structured output for the Principal.
//!
//! Shaping produces the plan/roles/contracts the human approves. Once
//! coordination starts, the Principal keeps taking turns in the SAME
//! conversation whenever a worker raises a `Blocker` or `DecisionRequest` —
//! this module builds that turn's prompt and parses the structured reply.
//! The impure driver that actually calls the provider and turns the reply
//! into `PostMessage`/`RaiseAttention` commands lives in
//! `app/src/workspace/view.rs` (`continue_principal_coordination`) — the
//! same split `shape_principal.rs` has from its own driver.

use dragent_workspace_model::{
    AgentMessage, AgentMessageKind, AttentionKind, ExecutionAttempt, Plan, WorkContract,
};

use super::shape_principal::{PrincipalPlanTaskDraft, PrincipalRoleDraft, PRINCIPAL_READ_TOOLS};

/// A role the Principal proposes mid-coordination, in response to a live
/// chat request — same draft shape Shaping already uses, paired with the
/// new tasks that role owns. The driver (`continue_principal_coordination`)
/// decides whether it launches immediately or needs a human Approve click:
/// see `work_item_surface.rs`'s `plan_team_and_tasks_section` for the
/// existing Approve flow this either matches or bypasses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProposedRoleDraft {
    pub role: PrincipalRoleDraft,
    pub tasks: Vec<PrincipalPlanTaskDraft>,
}

/// The Principal's decision after reading a message addressed to it —
/// a worker's Blocker/DecisionRequest, or a human's live chat message during
/// coordination. It can reply and/or escalate to the human, and — additive
/// only — add new tasks to an already-approved role or propose a brand-new
/// role. It can never edit or remove an existing task, contract, or role;
/// a change to something already running gets escalated with a clear
/// summary instead, and the human uses the existing Plan tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoordinationTurnResult {
    pub(crate) reply_body: String,
    pub escalate: bool,
    pub escalation_kind: Option<AttentionKind>,
    pub escalation_summary: Option<String>,
    pub add_tasks: Vec<PrincipalPlanTaskDraft>,
    pub propose_roles: Vec<ProposedRoleDraft>,
}

fn message_kind_label(kind: AgentMessageKind) -> &'static str {
    match kind {
        AgentMessageKind::Discussion => "discussion",
        AgentMessageKind::Handoff => "handoff",
        AgentMessageKind::Blocker => "blocker",
        AgentMessageKind::DecisionRequest => "decision request",
        AgentMessageKind::StatusUpdate => "status update",
        AgentMessageKind::ReviewFinding => "review finding",
        AgentMessageKind::RevisionRequest => "revision request",
        AgentMessageKind::Approval => "approval",
    }
}

/// Build the system/prompt pair for one coordination turn — same shape as
/// `shape_principal::build_request`: a JSON-embedded snapshot of the durable
/// state the Principal needs, plus the one message it must respond to.
pub(super) fn build_coordination_request(
    work_title: &str,
    plan: &Plan,
    contracts: &[WorkContract],
    role_names: &std::collections::HashMap<dragent_workspace_model::WorkContractId, String>,
    related_attempts: &[&ExecutionAttempt],
    target_message: &AgentMessage,
    thread_history: &[&AgentMessage],
    shaping_memory: &[String],
) -> (String, String) {
    let plan_tasks = plan
        .tasks
        .iter()
        .map(|task| {
            serde_json::json!({
                "key": task.id.0.to_string(),
                "title": task.title,
                "status": task.status,
                "depends_on": task.depends_on.iter().map(|id| id.0.to_string()).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let contracts = contracts
        .iter()
        .map(|contract| {
            serde_json::json!({
                "contract_id": contract.id.0.to_string(),
                // Role name for `add_tasks[].assigned_role` — assign a new
                // task to an existing role by this exact name; a role with no
                // task depending on unfinished work anywhere else starts
                // immediately with no human approval click, so only propose a
                // new role in `propose_roles` when none of these already fit.
                "name": role_names.get(&contract.id).cloned().unwrap_or_default(),
                "mission": contract.mission,
                "task_ids": contract.task_ids.iter().map(|id| id.0.to_string()).collect::<Vec<_>>(),
                "write_scope": contract.write_scope,
                "tools": contract.tools,
            })
        })
        .collect::<Vec<_>>();
    let dependency_attempts = related_attempts
        .iter()
        .map(|attempt| {
            serde_json::json!({
                "task_id": attempt.task_id.0.to_string(),
                "attempt_number": attempt.attempt_number,
                "status": attempt.status,
            })
        })
        .collect::<Vec<_>>();
    let state = serde_json::to_string_pretty(&serde_json::json!({
        "plan_tasks": plan_tasks,
        "contracts": contracts,
        "dependency_attempts": dependency_attempts,
    }))
    .unwrap_or_else(|_| "null".to_string());
    let thread_history = thread_history
        .iter()
        .map(|message| {
            serde_json::json!({
                "kind": message_kind_label(message.kind),
                "body": message.body,
                "review_finding": message.review_finding,
            })
        })
        .collect::<Vec<_>>();
    let thread_history =
        serde_json::to_string_pretty(&thread_history).unwrap_or_else(|_| "[]".to_string());
    let shaping_memory = if shaping_memory.is_empty() {
        "(no recorded memory yet)".to_string()
    } else {
        shaping_memory.join("\n")
    };
    let target = serde_json::json!({
        "kind": message_kind_label(target_message.kind),
        "body": target_message.body,
        "review_finding": target_message.review_finding,
    });
    let prompt = format!(
        "Work item: {work_title}\n\nCurrent plan, contracts, and the latest attempt states — the target task's dependency closure if the message you must respond to is about one specific task, or every currently active attempt across the whole work item if it isn't (a general status/steering question from the human, e.g. \"which agent is working right now?\", has no single task and needs the full picture):\n{state}\n\nWork-item memory (durable understanding from Shaping — decisions, preferences, prior changes; why the plan looks the way it does):\n{shaping_memory}\n\nThis thread's prior exchange (oldest to newest):\n{thread_history}\n\nMessage you must respond to now:\n{target}\n\nIf this is a handoff (one worker passing its result to another), you are the checkpoint: check the claimed result against the task's acceptance criteria and the actual repository state with your tools, record anything durable it learned with `record_knowledge`, correct the receiving agent with `message_agent` if the handoff is wrong or incomplete, and keep reply_body to a short acknowledgement — never escalate a clean handoff. If this is a general question or a directive rather than a worker's blocker, just answer or acknowledge it plainly in reply_body using the attempt states above — most such messages need no escalation at all. If the human is asking for new work (e.g. \"add a task to write tests for the auth module\"), use add_tasks/propose_roles below instead of just describing it in reply_body — an unactioned description is not what was asked for. Otherwise decide: reply directly if this is within your authority to resolve from the plan, contracts, and context above; escalate to the human only when the answer requires information, judgment, or authority you do not have (a real product/business decision, a change the plan does not already cover, or something a live human must approve). You may do both — acknowledge in reply_body and still escalate. Return exactly one JSON object with this schema: {{\"reply_body\": \"message back into this thread, or empty string if none\", \"escalate\": true or false, \"escalation_kind\": one of \"decision\", \"approval\", \"permission\", \"conflict\", \"budget\", \"failure\", \"review\", or null when escalate is false, \"escalation_summary\": \"one sentence for the human describing exactly what they must decide, or null when escalate is false\", \"add_tasks\": [] or a list of new tasks for an ALREADY-LISTED role's exact name — {{\"key\": \"short-local-id\", \"title\": \"...\", \"description\": \"...\", \"depends_on\": [existing task keys/ids or sibling add_tasks keys this needs first], \"assigned_role\": \"exact existing role name\", \"acceptance_criteria\": []}}, \"propose_roles\": [] or a list of brand-new roles only when no existing role fits — {{\"role\": {{\"kind\": \"worker\" or \"reviewer\", \"name\": \"...\", \"mission\": \"...\", \"rationale\": \"...\", \"capabilities\": [], \"tools\": [], \"write_scope\": [\"path/it/owns/\"]}}, \"tasks\": [same task shape as add_tasks, assigned_role set to this new role's name]}}}}. Do not include markdown fences or additional keys."
    );
    (system_prompt(), prompt)
}

fn system_prompt() -> String {
    format!(
        "You are the Principal agent mediating an active coordination run. Workers execute tasks in their own isolated worktrees; passing independent reviews are resolved by the workspace authority under the approved work contract. Never claim verification without evidence. \
        You do not write code, run commands, or edit files yourself: you only read the plan, contracts, dependency state, and this thread's history, then reply and/or escalate. \
        You may call read-only research tools ({}) to check the current state of the repository before replying, exactly like during Shaping. \
        You are the architect and principal engineer of this work item and you have live control-center tools — use them instead of guessing: `list_agents` (who is doing what right now, what is blocked, what remains), `inspect_agent` (one agent's recent steps, results, open requests, errors, files written), `list_blockers`, `work_status` (definition of done), `read_knowledge` / `record_knowledge` (the shared ledger every agent shares — record decisions, discoveries, root causes, and answers so nobody rediscovers them), `message_agent` (send an instruction or correction to a worker or reviewer), and `retry_task` (retry a blocked independent review or authorize another attempt for an exhausted task after fixing its cause), and `revise_task_instructions` (clarify an idle or failed task within its approved scope). Answer questions about agents from `list_agents`/`inspect_agent`, never from memory. When a worker is blocked on something the plan, contracts, repository, or knowledge already answer, answer it yourself with `message_agent`; escalate to the human only for a genuine product, security, budget, or authorization decision. \
        Resolve what you can from the plan and contracts already approved by the user — that approval already covers routine coordination. Escalate only when a live human decision is genuinely required: a product or business call, something outside what the plan and contracts already establish, or a risk the user asked to be kept in the loop on. \
        You may add new tasks to an already-approved role, or propose a brand-new role, in direct response to a live human request — you may also clarify an idle task with `revise_task_instructions`, but never reassign or remove tasks, change contract authority, or edit running tasks. A new role whose tasks depend on nothing still in progress starts immediately with no human approval click; a role with such a dependency still needs one, same as any role proposed during Shaping. For a blocked review, investigate its structured findings, resolve the cause, then call `retry_task` on the reviewer. A reply alone does not restart the review. If a task needs clearer instructions within scope, use `revise_task_instructions`. Permission, model, budget, scope, or acceptance-criteria changes still require human approval. \
        Return exactly one JSON object matching the schema given in the prompt — no prose outside it, no markdown fences.",
        PRINCIPAL_READ_TOOLS.join(", ")
    )
}

fn parse_escalation_kind(value: &str) -> AttentionKind {
    match value.trim().to_ascii_lowercase().as_str() {
        "approval" => AttentionKind::Approval,
        "permission" => AttentionKind::Permission,
        "conflict" => AttentionKind::Conflict,
        "budget" => AttentionKind::Budget,
        "failure" => AttentionKind::Failure,
        "review" => AttentionKind::Review,
        _ => AttentionKind::Decision,
    }
}

fn strip_code_fence(trimmed: &str) -> &str {
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    trimmed.strip_suffix("```").unwrap_or(trimmed).trim()
}

/// Parse the coordination-turn JSON, tolerating prose around the object the
/// same way `shape_principal::parse_shape_json` does — models sometimes
/// preface structured output with a sentence despite instructions not to.
pub(super) fn parse_coordination_response(content: &str) -> Result<CoordinationTurnResult, String> {
    #[derive(serde::Deserialize)]
    struct RawResult {
        #[serde(default)]
        reply_body: String,
        #[serde(default)]
        escalate: bool,
        #[serde(default)]
        escalation_kind: Option<String>,
        #[serde(default)]
        escalation_summary: Option<String>,
        #[serde(default)]
        add_tasks: Vec<PrincipalPlanTaskDraft>,
        #[serde(default)]
        propose_roles: Vec<RawProposedRole>,
    }

    #[derive(serde::Deserialize)]
    struct RawProposedRole {
        role: PrincipalRoleDraft,
        #[serde(default)]
        tasks: Vec<PrincipalPlanTaskDraft>,
    }

    let trimmed = strip_code_fence(content.trim());
    let raw = serde_json::from_str::<RawResult>(trimmed)
        .or_else(|_| {
            let start = trimmed.find('{');
            let end = trimmed.rfind('}');
            match (start, end) {
                (Some(start), Some(end)) if end > start => {
                    serde_json::from_str::<RawResult>(&trimmed[start..=end])
                }
                _ => Err(serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "no JSON object found",
                ))),
            }
        })
        .map_err(|error| format!("Could not parse Principal coordination response: {error}"))?;

    if !raw.escalate
        && raw.reply_body.trim().is_empty()
        && raw.add_tasks.is_empty()
        && raw.propose_roles.is_empty()
    {
        return Err("Principal coordination response neither replied, escalated, nor acted".into());
    }

    Ok(CoordinationTurnResult {
        reply_body: raw.reply_body,
        escalate: raw.escalate,
        escalation_kind: raw
            .escalate
            .then(|| parse_escalation_kind(raw.escalation_kind.as_deref().unwrap_or("decision"))),
        escalation_summary: raw.escalate.then_some(raw.escalation_summary).flatten(),
        add_tasks: raw.add_tasks,
        propose_roles: raw
            .propose_roles
            .into_iter()
            .map(|raw_role| ProposedRoleDraft {
                role: raw_role.role,
                tasks: raw_role.tasks,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_direct_reply_with_no_escalation() {
        let result = parse_coordination_response(
            r#"{"reply_body": "Use the existing retry helper.", "escalate": false, "escalation_kind": null, "escalation_summary": null}"#,
        )
        .expect("parse");
        assert_eq!(result.reply_body, "Use the existing retry helper.");
        assert!(!result.escalate);
        assert_eq!(result.escalation_kind, None);
        assert_eq!(result.escalation_summary, None);
    }

    #[test]
    fn parses_an_escalation_with_kind_and_summary() {
        let result = parse_coordination_response(
            r#"{"reply_body": "Acknowledged.", "escalate": true, "escalation_kind": "budget", "escalation_summary": "Task exceeds the approved token budget; raise it or descope."}"#,
        )
        .expect("parse");
        assert!(result.escalate);
        assert_eq!(result.escalation_kind, Some(AttentionKind::Budget));
        assert_eq!(
            result.escalation_summary.as_deref(),
            Some("Task exceeds the approved token budget; raise it or descope.")
        );
    }

    #[test]
    fn defaults_an_unrecognized_or_missing_escalation_kind_to_decision() {
        let result = parse_coordination_response(
            r#"{"reply_body": "", "escalate": true, "escalation_kind": "something-new", "escalation_summary": "Needs a human call."}"#,
        )
        .expect("parse");
        assert_eq!(result.escalation_kind, Some(AttentionKind::Decision));
    }

    #[test]
    fn tolerates_prose_and_code_fences_around_the_json_object() {
        let result = parse_coordination_response(
            "Here is my decision:\n```json\n{\"reply_body\": \"ok\", \"escalate\": false}\n```",
        )
        .expect("parse");
        assert_eq!(result.reply_body, "ok");
    }

    #[test]
    fn rejects_a_response_that_neither_replies_nor_escalates() {
        assert!(parse_coordination_response(r#"{"reply_body": "", "escalate": false}"#).is_err());
    }

    #[test]
    fn rejects_unparseable_content() {
        assert!(parse_coordination_response("not json at all").is_err());
    }

    #[test]
    fn parses_add_tasks_and_propose_roles() {
        let result = parse_coordination_response(
            r#"{
                "reply_body": "Added a task and a new role for this.",
                "escalate": false,
                "add_tasks": [{
                    "key": "add-tests",
                    "title": "Write auth tests",
                    "description": "Cover the new auth path",
                    "depends_on": [],
                    "assigned_role": "Backend Engineer",
                    "acceptance_criteria": []
                }],
                "propose_roles": [{
                    "role": {
                        "kind": "worker",
                        "name": "Database Migrator",
                        "mission": "Own the schema migration",
                        "rationale": "No one else is scoped for it",
                        "capabilities": [],
                        "tools": [],
                        "write_scope": ["migrations/"]
                    },
                    "tasks": [{
                        "key": "migrate",
                        "title": "Write the migration",
                        "description": "Add the new column",
                        "depends_on": [],
                        "assigned_role": "Database Migrator",
                        "acceptance_criteria": []
                    }]
                }]
            }"#,
        )
        .expect("parse");
        assert_eq!(result.add_tasks.len(), 1);
        assert_eq!(result.add_tasks[0].title, "Write auth tests");
        assert_eq!(result.propose_roles.len(), 1);
        assert_eq!(result.propose_roles[0].role.name, "Database Migrator");
        assert_eq!(result.propose_roles[0].tasks.len(), 1);
    }

    #[test]
    fn an_action_only_turn_with_no_reply_text_is_still_valid() {
        let result = parse_coordination_response(
            r#"{
                "reply_body": "",
                "escalate": false,
                "add_tasks": [{
                    "key": "add-tests",
                    "title": "Write auth tests",
                    "description": "Cover the new auth path",
                    "depends_on": [],
                    "assigned_role": "Backend Engineer",
                    "acceptance_criteria": []
                }]
            }"#,
        )
        .expect("parse");
        assert!(result.reply_body.is_empty());
        assert_eq!(result.add_tasks.len(), 1);
    }
}
