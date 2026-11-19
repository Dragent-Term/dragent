//! Principal shaping (hub).
//!
//! Prompt construction, streaming, validation, and normalization for the
//! Principal, split by area. All super::shape_principal paths keep
//! working through re-exports.

pub(crate) use super::principal_normalize::*;
pub(crate) use super::principal_stream::*;
pub(crate) use super::principal_text::*;
pub(crate) use super::principal_types::*;
pub(crate) use super::principal_validate::*;

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gather_mode_forbids_proposal_json_and_shape_mode_permits_it() {
        let gather = turn_mode_block(PrincipalTurnMode::Gather);
        assert!(gather.contains("Approve Shape"));
        assert!(gather.contains("turn_kind must stay conversation"));
        let shape = turn_mode_block(PrincipalTurnMode::Shape);
        assert!(shape.contains("APPROVED SHAPING"));
        assert!(
            shape.contains("revision discipline"),
            "Shape mode must point at the minimal-evolution rules"
        );
    }

    #[test]
    fn revision_discipline_demands_minimal_evolution() {
        assert!(SYSTEM_PROMPT.contains("Revision discipline"));
        assert!(SYSTEM_PROMPT.contains("Extend the plan; never rebuild or replace it"));
        assert!(SYSTEM_PROMPT.contains("byte-identical"));
        assert!(SYSTEM_PROMPT.contains("smallest reasonable change"));
    }

    #[test]
    fn revision_brief_marks_approvals_sacrosanct() {
        let approved = vec!["Builder".to_string()];
        let resolved = vec!["Postgres or SQLite? → Postgres".to_string()];
        let (_, prompt) = build_request(
            "Work",
            "Also implement the feature.",
            Some(&ShapingProposal {
                revision: 1,
                outcome: "Review it".into(),
                requirements: Vec::new(),
                open_decisions: Vec::new(),
                acceptance_criteria: Vec::new(),
                plan_summary: "Review".into(),
            }),
            None,
            &[],
            &[],
            &[],
            None,
            &[],
            &approved,
            &resolved,
            &[],
        );
        assert!(prompt.contains("Approved roles: Builder"));
        assert!(prompt.contains("Postgres or SQLite? → Postgres"));
        assert!(prompt.contains("never reopen"));
        assert!(prompt.contains("Role names are stable identities"));

        // First shaping turn stays clean: no sacrosanct section, no noise.
        let (_, fresh) = build_request(
            "Work",
            "Shape it.",
            None,
            None,
            &[],
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            &[],
        );
        assert!(!fresh.contains("Approved roles:"));
        assert!(!fresh.contains("Resolved decisions"));
        assert!(fresh.contains("(no recorded memory yet)"));
    }

    #[test]
    fn work_item_memory_feeds_the_prompt() {
        let memory = vec!["[decision] Chose Postgres for proration".to_string()];
        let (_, prompt) = build_request(
            "Work",
            "Also implement it.",
            None,
            None,
            &[],
            &[],
            &[],
            None,
            &[],
            &[],
            &[],
            &memory,
        );
        assert!(prompt.contains("Work-item memory"));
        assert!(prompt.contains("[decision] Chose Postgres for proration"));
    }

    fn continuity_proposal(role_names: &[&str], questions: &[&str]) -> PrincipalShapeResult {
        PrincipalShapeResult {
            turn_kind: PrincipalTurnKind::Proposal,
            assistant_message: "Extended the plan.".into(),
            outcome: "Ship it".into(),
            requirements: Vec::new(),
            open_decisions: questions.iter().map(|s| s.to_string()).collect(),
            acceptance_criteria: Vec::new(),
            plan_summary: "Plan".into(),
            plan_tasks: Vec::new(),
            proposed_roles: role_names
                .iter()
                .map(|name| PrincipalRoleDraft {
                    kind: AgentRoleKind::Worker,
                    name: name.to_string(),
                    mission: "M".into(),
                    rationale: "R".into(),
                    capabilities: Vec::new(),
                    tools: Vec::new(),
                    write_scope: Vec::new(),
                    preferred_model_route: None,
                })
                .collect(),
            repository_references: Vec::new(),
            requirements_complete: false,
            memory_notes: Vec::new(),
        }
    }

    #[test]
    fn continuity_guard_catches_silent_drops_but_allows_intent() {
        let approved = vec!["Builder".to_string()];
        let resolved = vec!["Postgres or SQLite?".to_string()];
        // Intact revision passes.
        let ok = continuity_proposal(&["Builder", "Reviewer"], &["Postgres or SQLite?"]);
        assert!(
            check_revision_continuity(&ok, "Also implement it.", true, &approved, &resolved)
                .is_ok()
        );
        // Dropped approved role fails…
        let dropped = continuity_proposal(&["Reviewer"], &["Postgres or SQLite?"]);
        let err =
            check_revision_continuity(&dropped, "Also implement it.", true, &approved, &resolved)
                .unwrap_err();
        assert!(err.starts_with("Principal revision dropped the approved role"));
        assert!(is_repairable_proposal_error(&err));
        // …unless the user explicitly asked for the removal.
        assert!(check_revision_continuity(
            &dropped,
            "Drop the Builder role, the reviewer covers it.",
            true,
            &approved,
            &resolved
        )
        .is_ok());
        // Dropped resolved decision fails…
        let reopened = continuity_proposal(&["Builder"], &["Something else?"]);
        let err =
            check_revision_continuity(&reopened, "Also implement it.", true, &approved, &resolved)
                .unwrap_err();
        assert!(err.starts_with("Principal revision dropped the resolved decision"));
        // …and a declared restructuring with reasons passes.
        let mut restructured = continuity_proposal(&["Fresh"], &[]);
        restructured.assistant_message =
            "I restructured the plan from scratch because the stack changed.".into();
        assert!(check_revision_continuity(
            &restructured,
            "Switch everything to Go.",
            true,
            &approved,
            &resolved
        )
        .is_ok());
        // First shaping turn and conversation turns are exempt.
        let mut convo = continuity_proposal(&[], &[]);
        convo.turn_kind = PrincipalTurnKind::Conversation;
        assert!(check_revision_continuity(
            &convo,
            "Also implement it.",
            true,
            &approved,
            &resolved
        )
        .is_ok());
        assert!(
            check_revision_continuity(&ok, "Also implement it.", false, &approved, &resolved)
                .is_ok()
        );
    }

    #[test]
    fn display_text_exposes_only_the_conversational_field() {
        assert_eq!(display_text("{\"assis", true), "Structuring the work…");
        assert_eq!(
            display_text(
                "{\"assistant_message\":\"I am revising the plan\",\"outcome\":\"secret structure\"}",
                true
            ),
            "I am revising the plan"
        );
        assert_eq!(
            display_text("{\"assistant_message\":\"Partial response", true),
            "Partial response"
        );
        // Complete payload: the parsed field, escapes resolved.
        assert_eq!(
            display_text(
                "{\"turn_kind\":\"conversation\",\"assistant_message\":\"Line one\\nLine \\\"two\\\"\"}",
                false
            ),
            "Line one\nLine \"two\""
        );
        // Plain prose (a model that ignored the schema) passes through.
        assert_eq!(display_text("Hello there", false), "Hello there");
    }

    #[test]
    fn downgrade_to_conversation_keeps_message_only() {
        let mut result = PrincipalShapeResult {
            turn_kind: PrincipalTurnKind::Proposal,
            assistant_message: "Here is what I found.".into(),
            outcome: "Outcome".into(),
            requirements: vec!["Req".into()],
            open_decisions: vec!["Q?".into()],
            acceptance_criteria: vec!["AC".into()],
            plan_summary: "Plan".into(),
            plan_tasks: Vec::new(),
            proposed_roles: Vec::new(),
            repository_references: vec!["a.rs".into()],
            requirements_complete: false,
            memory_notes: Vec::new(),
        };
        downgrade_to_conversation(&mut result);
        assert_eq!(result.turn_kind, PrincipalTurnKind::Conversation);
        assert_eq!(result.assistant_message, "Here is what I found.");
        assert!(result.outcome.is_empty());
        assert!(result.requirements.is_empty());
        assert!(result.plan_tasks.is_empty());
        assert!(result.repository_references.is_empty());
    }

    #[test]
    fn principal_allowlists_read_tools_and_rejects_writes() {
        for tool in [
            "read_file",
            "list_directory",
            "grep",
            "search_codebase",
            "directory_tree",
        ] {
            assert!(is_principal_read_tool(tool), "{tool} should be allowed");
        }
        for tool in [
            "write_file",
            "edit_file",
            "run_command",
            "ask_user",
            "apply_patch",
        ] {
            assert!(!is_principal_read_tool(tool), "{tool} must stay blocked");
        }
    }

    #[test]
    fn observed_files_ground_live_reads_for_validation() {
        let observed =
            observed_files_from_tool_args(r#"{"path":"server/billing.rs","pattern":"invoice"}"#);
        assert!(observed.contains(&"server/billing.rs".to_string()));
        let result = PrincipalShapeResult {
            turn_kind: PrincipalTurnKind::Proposal,
            assistant_message: "Ready".into(),
            outcome: "Ship billing".into(),
            requirements: vec![],
            open_decisions: vec![],
            acceptance_criteria: vec!["Invoices are downloadable".into()],
            plan_summary: "Implement and review".into(),
            plan_tasks: vec![PrincipalPlanTaskDraft {
                key: "implementation".into(),
                title: "Implement billing".into(),
                description: "Build the billing flow".into(),
                depends_on: vec![],
                review_target: None,
                assigned_role: "Backend Engineer".into(),
                acceptance_criteria: vec!["Invoices are downloadable".into()],
            }],
            proposed_roles: vec![PrincipalRoleDraft {
                kind: AgentRoleKind::Worker,
                name: "Backend Engineer".into(),
                mission: "Build billing".into(),
                rationale: "Own implementation".into(),
                capabilities: vec![],
                tools: vec![],
                write_scope: vec![],
                preferred_model_route: None,
            }],
            repository_references: vec!["server/billing.rs".into()],
            requirements_complete: false,
            memory_notes: Vec::new(),
        };
        assert!(
            validate_repository_references_with_observed(&result, Some("empty"), &observed).is_ok()
        );
        assert!(validate_repository_references_with_observed(&result, Some("empty"), &[]).is_err());
    }

    #[test]
    fn worker_questions_prefer_memory_before_findings() {
        assert_eq!(
            route_worker_question(
                "Should billing use Postgres proration?",
                "decided: billing uses Postgres with proration",
                "plan covers billing",
                "evidence from billing task",
            ),
            WorkerQuestionRoute::AnswerFromMemory
        );
        assert_eq!(
            route_worker_question(
                "How should quantum sync behave?",
                "billing uses Postgres",
                "plan covers billing",
                "evidence from billing task",
            ),
            WorkerQuestionRoute::NeedsFindings
        );
    }

    #[test]
    fn greeting_is_conversation_not_an_executable_shape() {
        assert!(conversational_reply("hello").is_some());
        assert!(conversational_reply("  Good morning! ").is_some());
        assert!(conversational_reply("howdy").is_some());
        assert!(conversational_reply("Howdy!").is_some());
        assert!(conversational_reply("thanks").is_some());
        assert!(conversational_reply("bye").is_some());
        assert!(conversational_reply("hello, build the authentication flow").is_none());
        assert!(conversational_reply("fix the failing tests").is_none());
        assert!(conversational_reply("ok go ahead").is_none());
    }

    #[test]
    fn shape_approval_text_needs_explicit_shaping_words() {
        assert!(is_shape_approval_text("approve shape"));
        assert!(is_shape_approval_text("Yes, approve shape please"));
        assert!(is_shape_approval_text("start shaping now"));
        assert!(is_shape_approval_text("please create the shape"));
        assert!(is_shape_approval_text("looks good, shape it"));
        // Not approval: research continues, questions, negations.
        assert!(!is_shape_approval_text("ok go ahead"));
        assert!(!is_shape_approval_text("go ahead and research"));
        assert!(!is_shape_approval_text("how will you shape it?"));
        assert!(!is_shape_approval_text("don't shape yet"));
        assert!(!is_shape_approval_text("do not start shaping"));
        assert!(!is_shape_approval_text("wait, not yet"));
        assert!(!is_shape_approval_text(""));
        // The Approve Shape resubmission wording must never re-trigger
        // approval routing (which lives only in the submit handler).
        assert!(!is_shape_approval_text(
            "The requirements are confirmed and approved. Build the work item shape now from our agreed requirements and scope."
        ));
    }

    #[test]
    fn only_structural_proposal_errors_are_retried() {
        assert!(is_repairable_proposal_error(
            "Principal plan task 'research' references unknown acceptance criterion"
        ));
        assert!(is_repairable_proposal_error(
            "Invalid Principal response: expected value"
        ));
        assert!(!is_repairable_proposal_error(
            "Principal stream failed: authentication required"
        ));
    }

    #[test]
    fn turn_memory_stays_bounded_and_skips_blanks() {
        let mut memory = Vec::new();
        push_turn_memory(&mut memory, "   ".into());
        assert!(memory.is_empty());
        push_turn_memory(&mut memory, "read src/main.rs → entry point".into());
        assert_eq!(memory.len(), 1);
        let long = "x".repeat(TURN_MEMORY_CHARS + 50);
        push_turn_memory(&mut memory, long);
        assert!(memory[1].ends_with('…'));
        for i in 0..(TURN_MEMORY_ENTRIES + 5) {
            push_turn_memory(&mut memory, format!("finding {i}"));
        }
        assert_eq!(memory.len(), TURN_MEMORY_ENTRIES);
        assert!(memory[0].contains("finding 5"));
    }

    #[test]
    fn assistant_message_is_never_truncated() {
        // Regression: long agent prose must return completely (terminal
        // parity) — the 600-char schema cap applies to short fields only.
        let long = vec!["Findings."; 200].join(" ");
        let json = format!(
            "{{\"turn_kind\":\"conversation\",\"assistant_message\":{}}}",
            serde_json::to_string(&long).unwrap()
        );
        let parsed = parse_response(&json).unwrap();
        assert_eq!(parsed.assistant_message, long);
        assert!(parsed.assistant_message.chars().count() > MAX_ITEM_CHARS);
    }

    #[test]
    fn missing_assistant_message_is_repaired_not_failed() {
        // Regression: a proposal missing `assistant_message` must still be
        // accepted — the shape is valid and reviewable, and the turn must not
        // surface "could not produce a consistent work proposal".
        let parsed = parse_response(
            r#"{"turn_kind":"proposal","outcome":"Ship billing","requirements":["Bill seats"],
                "acceptance_criteria":["Invoices are downloadable"],
                "plan_summary":"Implement and review",
                "plan_tasks":[{"key":"implementation","title":"Implement billing","description":"Build the billing flow","depends_on":[],"assigned_role":"Backend Engineer","acceptance_criteria":["Invoices are downloadable"]}],
                "proposed_roles":[{"kind":"worker","name":"Backend Engineer","mission":"Build billing","rationale":"Own implementation","write_scope":[]}]}"#,
        )
        .unwrap();
        assert!(!parsed.assistant_message.trim().is_empty());
        assert_eq!(parsed.turn_kind, PrincipalTurnKind::Proposal);
        // Alternate key spelling is accepted too.
        let aliased = parse_response(r#"{"turn_kind":"conversation","message":"Okay!"}"#).unwrap();
        assert_eq!(aliased.assistant_message, "Okay!");
    }

    #[test]
    fn prose_wrapped_json_is_salvaged() {
        let parsed = parse_response(
            "Here is the shape you approved:\n{\"turn_kind\":\"conversation\",\"assistant_message\":\"Ready.\"}\nLet me know if you want changes.",
        )
        .unwrap();
        assert_eq!(parsed.assistant_message, "Ready.");
    }

    #[test]
    fn truncation_detector_only_fires_on_cut_off_json() {
        // Cut off mid-object (no closing brace) → truncated.
        assert!(is_truncated_json(
            r#"{"turn_kind":"proposal","assistant_message":"Here is the shape","outcome":"Ship bill"#
        ));
        // Cut off mid-string inside a fenced block → truncated.
        assert!(is_truncated_json(
            "```json\n{\"turn_kind\":\"proposal\",\"assistant_message\":\"Partial…"
        ));
        // Balanced braces but invalid JSON is a schema problem, not a
        // truncation — the repair pass (not continuation) owns it.
        assert!(!is_truncated_json(r#"{"turn_kind":"proposal","outcome":}"#));
        // Complete objects are never truncation, even when invalid.
        assert!(!is_truncated_json(
            r#"{"turn_kind":"proposal","assistant_message":"Hi"}"#
        ));
        // Plain prose and empty streams are not truncation either.
        assert!(!is_truncated_json("I made a plan"));
        assert!(!is_truncated_json("   "));
        // The continuation prompt resumes the byte stream, never restarts.
        let prompt = shaping_continuation_prompt(r#"{"turn_kind":"proposal","out"#);
        assert!(prompt.contains("Continue EXACTLY"));
        assert!(prompt.contains(r#"{"turn_kind":"proposal","out"#));
    }

    #[test]
    fn conversation_turn_is_inert_even_if_structured_fields_leak_in() {
        let parsed = parse_response(
            r#"{
              "turn_kind": "conversation",
              "assistant_message": "I can help you explore that. What outcome matters most?",
              "outcome": "Speculative outcome",
              "acceptance_criteria": ["Speculative criterion"],
              "plan_summary": "Speculative plan",
              "plan_tasks": [{
                "key": "should-not-run",
                "title": "Should not run",
                "description": "This must be discarded",
                "assigned_role": "Architect",
                "acceptance_criteria": ["Speculative criterion"]
              }],
              "proposed_roles": [{
                "kind": "architecture",
                "name": "Architect",
                "mission": "Speculative role",
                "rationale": "Not approved"
              }]
            }"#,
        )
        .unwrap();

        assert_eq!(parsed.turn_kind, PrincipalTurnKind::Conversation);
        assert!(parsed.outcome.is_empty());
        assert!(parsed.acceptance_criteria.is_empty());
        assert!(parsed.plan_tasks.is_empty());
        assert!(parsed.proposed_roles.is_empty());
    }

    #[test]
    fn parses_fenced_structured_response_and_normalizes_items() {
        let parsed = parse_response(
            r#"```json
            {
              "turn_kind": "proposal",
              "assistant_message": "I updated the plan.",
              "outcome": "Ship organization billing",
              "requirements": ["Seat billing", "Seat billing", "  Invoices  "],
              "open_decisions": ["Proration policy?"],
              "acceptance_criteria": ["Admins can download invoices"],
              "plan_summary": "Implement behind a feature flag",
              "plan_tasks": [{
                "key": "implement billing",
                "title": "Implement billing",
                "description": "Build the billing service and collect test evidence",
                "depends_on": [],
                "assigned_role": "Backend Engineer",
                "acceptance_criteria": ["Admins can download invoices"]
              }],
              "proposed_roles": [{
                "name": "Backend Engineer",
                "mission": "Implement billing services",
                "rationale": "Own the billing boundary",
                "capabilities": ["Rust", "Rust"],
                "tools": ["read_file", "write_file", "edit_file", "run_command"],
                "write_scope": ["server/billing"],
                "preferred_model_route": "anthropic/claude-review"
              }]
            }
            ```"#,
        )
        .unwrap();

        assert_eq!(parsed.requirements, ["Seat billing", "Invoices"]);
        assert_eq!(parsed.open_decisions, ["Proration policy?"]);
        assert_eq!(parsed.proposed_roles.len(), 1);
        assert_eq!(parsed.proposed_roles[0].capabilities, ["Rust"]);
        assert_eq!(
            parsed.proposed_roles[0].tools,
            ["read_file", "write_file", "edit_file", "run_command"]
        );
        assert_eq!(
            parsed.proposed_roles[0].preferred_model_route.as_deref(),
            Some("anthropic/claude-review")
        );
        assert_eq!(parsed.plan_tasks[0].key, "implement-billing");
        assert!(parsed.repository_references.is_empty());
    }

    fn execution_tools_proposal() -> PrincipalShapeResult {
        let mut result = continuity_proposal(&["Builder", "Reviewer"], &[]);
        result.acceptance_criteria = vec!["Billing tests pass".into()];
        result.proposed_roles[0].tools = vec![
            "read_file".into(),
            "read_multiple_files".into(),
            "write_file".into(),
            "edit_file".into(),
            "run_command".into(),
        ];
        result.proposed_roles[0].write_scope = vec!["src/billing".into()];
        result.proposed_roles[1].kind = AgentRoleKind::Reviewer;
        result.proposed_roles[1].tools = vec!["read_file".into(), "grep".into()];
        for (index, role) in result.proposed_roles.iter_mut().enumerate() {
            role.preferred_model_route = Some("test/model".into());
            result.plan_tasks.push(PrincipalPlanTaskDraft {
                key: format!("task-{index}"),
                title: format!("Billing deliverable {index}"),
                description: "Implement or independently verify billing with evidence".into(),
                depends_on: if index == 0 {
                    vec![]
                } else {
                    vec!["task-0".into()]
                },
                review_target: (index == 1).then(|| "task-0".into()),
                assigned_role: role.name.clone(),
                acceptance_criteria: result.acceptance_criteria.clone(),
            });
        }
        result
    }

    /// The approved-shaping policy and the validator agree: a plan built
    /// exactly as `SHAPE_POLICY` describes passes every check by itself,
    /// before any repair.
    #[test]
    fn a_plan_built_by_the_shape_policy_passes_validation_unrepaired() {
        let shape = turn_mode_block(PrincipalTurnMode::Shape);
        for rule in [
            "character for character",
            "every role owns at least one task",
            "never null",
            "\"review-X\"",
            "\"security-review\"",
            "\"qa-validation\"",
            "Worker tasks always set review_target to null",
        ] {
            assert!(shape.contains(rule), "the policy must state: {rule}");
        }
        let role = |kind: &str, name: &str, tools: &[&str], scope: &[&str]| {
            serde_json::json!({
                "kind": kind, "name": name, "mission": format!("{name} for billing"),
                "rationale": "Bounded work on a suitable model", "capabilities": [],
                "tools": tools, "write_scope": scope, "preferred_model_route": "test/model"
            })
        };
        let task = |key: &str, depends_on: &[&str], target: Option<&str>, role: &str, criteria: &[&str]| {
            serde_json::json!({
                "key": key, "title": format!("Deliver {key}"),
                "description": "Bounded work with tests and evidence",
                "depends_on": depends_on, "review_target": target,
                "assigned_role": role, "acceptance_criteria": criteria
            })
        };
        let (monthly, pdf) = ("Seats are billed monthly", "Invoices download as PDF files");
        let proposal = serde_json::json!({
            "turn_kind": "proposal",
            "assistant_message": "Here is the shape.",
            "outcome": "Seat billing with invoices",
            "requirements": ["Bill seats"],
            "open_decisions": [],
            "acceptance_criteria": [monthly, pdf],
            "plan_summary": "Build, review each part, then secure and verify",
            "plan_tasks": [
                task("billing-model", &[], None, "Backend engineer", &[monthly]),
                task("invoice-pdf", &["billing-model"], None, "Backend engineer", &[pdf]),
                task("review-billing-model", &["billing-model"], Some("billing-model"), "Code reviewer", &[monthly]),
                task("review-invoice-pdf", &["invoice-pdf"], Some("invoice-pdf"), "Code reviewer", &[pdf]),
                task("security-review", &["billing-model", "invoice-pdf"], Some("invoice-pdf"), "Security reviewer", &[pdf]),
                task("qa-validation", &["billing-model", "invoice-pdf"], Some("invoice-pdf"), "QA", &[monthly, pdf]),
            ],
            "proposed_roles": [
                role("worker", "Backend engineer", &["read_file", "edit_file", "run_command"], &["src"]),
                role("reviewer", "Code reviewer", &["read_file", "grep"], &[]),
                role("reviewer", "Security reviewer", &["read_file", "grep"], &[]),
                role("reviewer", "QA", &["read_file", "run_command"], &[]),
            ],
            "repository_references": [],
            "memory_notes": []
        })
        .to_string();
        let mut raw = parse_shape_json(&proposal).unwrap();
        raw.acceptance_criteria = normalize_items(raw.acceptance_criteria);
        raw.proposed_roles = normalize_roles(raw.proposed_roles);
        raw.plan_tasks = normalize_plan_tasks(raw.plan_tasks);
        assert_eq!(validate_plan_tasks(&raw), Ok(()));
        let routes = [PrincipalModelRoute {
            route: "test/model".into(),
            label: "Test".into(),
            detail: "Test".into(),
        }];
        let parsed = finish_response(&proposal, &routes, None).unwrap();
        assert_eq!(parsed.plan_tasks, raw.plan_tasks, "nothing needed repairing");
    }

    /// The slips that used to reject a whole proposal (and cost the user a
    /// repeated model pass each) are fixed in place; ambiguity still fails.
    #[test]
    fn mechanical_plan_slips_are_repaired_instead_of_rejected() {
        let qa = |mut result: PrincipalShapeResult| {
            result.acceptance_criteria = vec![
                "Billing tests pass on CI".into(),
                "Invoices download as PDF files for every customer".into(),
            ];
            for task in &mut result.plan_tasks {
                task.acceptance_criteria = vec!["Billing tests pass on CI".into()];
            }
            result
        };
        // A reviewer task with no review target reviews its worker dependency;
        // an uncovered criterion goes to the review task.
        let mut result = qa(execution_tools_proposal());
        result.plan_tasks[1].review_target = None;
        let parsed = parse_response(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(parsed.plan_tasks[1].review_target.as_deref(), Some("task-0"));
        assert!(parsed.plan_tasks[1]
            .acceptance_criteria
            .contains(&"Invoices download as PDF files for every customer".to_string()));

        // A target missing from depends_on is added; a worker's stray target
        // is dropped.
        let mut result = qa(execution_tools_proposal());
        result.plan_tasks[1].depends_on.clear();
        result.plan_tasks[0].review_target = Some("task-1".into());
        let parsed = parse_response(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(parsed.plan_tasks[1].depends_on, vec!["task-0".to_string()]);
        assert_eq!(parsed.plan_tasks[0].review_target, None);

        // Criteria cited by position, key or different case resolve to the
        // exact top-level text.
        for cited in ["2", "AC2", "criterion-2", "invoices download as pdf files for every customer."] {
            let mut result = qa(execution_tools_proposal());
            result.plan_tasks[0].acceptance_criteria.push(cited.into());
            let parsed = parse_response(&serde_json::to_string(&result).unwrap()).unwrap();
            assert!(
                parsed.plan_tasks[0]
                    .acceptance_criteria
                    .contains(&"Invoices download as PDF files for every customer".to_string()),
                "{cited}"
            );
        }

        // Something that names no criterion is still rejected.
        let mut result = qa(execution_tools_proposal());
        result.plan_tasks[0].acceptance_criteria.push("Customers are happy".into());
        let error = parse_response(&serde_json::to_string(&result).unwrap()).unwrap_err();
        assert!(error.contains("unknown acceptance criterion"), "{error}");
        // So is a reviewer with nothing to review.
        let mut result = qa(execution_tools_proposal());
        result.plan_tasks[1].review_target = None;
        result.plan_tasks[1].depends_on.clear();
        let error = parse_response(&serde_json::to_string(&result).unwrap()).unwrap_err();
        assert!(error.contains("must target one of its dependencies"), "{error}");
    }

    #[test]
    fn execution_tool_identifiers_survive_proposal_validation_without_expansion() {
        let result = execution_tools_proposal();
        let original_roles = result.proposed_roles.clone();
        let parsed = finish_response(
            &serde_json::to_string(&result).unwrap(),
            &[PrincipalModelRoute {
                route: "test/model".into(),
                label: "Test model".into(),
                detail: "Implementation and review".into(),
            }],
            None,
        )
        .unwrap();
        assert_eq!(parsed.proposed_roles, original_roles);
        let mut no_tools = result;
        no_tools.proposed_roles[0].tools.clear();
        let parsed = parse_response(&serde_json::to_string(&no_tools).unwrap()).unwrap();
        assert!(parsed.proposed_roles[0].tools.is_empty());
    }

    #[test]
    fn proposals_reject_tool_families_aliases_and_interactive_tools() {
        for tool in [
            "editor",
            "filesystem",
            "shell",
            "terminal",
            "read",
            "write",
            "run",
            "Read_File",
            " read_file ",
            "read_*",
            "find",
            "delete_file",
            "unknown_tool",
            "ask_user",
            "todowrite",
            "spawn_subagent",
        ] {
            let mut result = execution_tools_proposal();
            result.proposed_roles[0].tools.push(tool.into());
            let original_roles = result.proposed_roles.clone();
            let error = parse_response(&serde_json::to_string(&result).unwrap()).unwrap_err();
            assert!(
                error.contains("unsupported tool identifier"),
                "{tool}: {error}"
            );
            assert!(error.contains(tool), "{tool}: {error}");
            assert!(is_repairable_proposal_error(&error));
            assert_eq!(result.proposed_roles, original_roles);
        }
    }

    #[test]
    fn invalid_tools_cannot_hide_beyond_normalization_limits() {
        let mut result = execution_tools_proposal();
        result.proposed_roles[0].tools = vec!["read_file".into(); MAX_ITEMS_PER_SECTION];
        result.proposed_roles[0].tools.push("editor".into());
        assert!(parse_response(&serde_json::to_string(&result).unwrap()).is_err());
        result.turn_kind = PrincipalTurnKind::Conversation;
        let parsed = parse_response(&serde_json::to_string(&result).unwrap()).unwrap();
        assert!(parsed.proposed_roles.is_empty());
    }

    #[test]
    fn reviewers_reject_write_and_execution_tools() {
        for tool in [
            "write_file",
            "edit_file",
            "repl",
            "move_file",
            "ask_user",
            "todowrite",
        ] {
            let mut result = execution_tools_proposal();
            result.proposed_roles[1].tools.push(tool.into());
            let error = parse_response(&serde_json::to_string(&result).unwrap()).unwrap_err();
            assert!(error.contains("Reviewer"), "{tool}: {error}");
            assert!(
                error.contains("unsupported tool identifier"),
                "{tool}: {error}"
            );
        }
    }

    #[test]
    fn qa_missioned_reviewers_may_run_commands_but_not_write_files() {
        // A specially-missioned Reviewer (QA) needs run_command to execute the
        // test suite; this doesn't relax the file-mutation ban other reviewer
        // missions (code review, security review) still get.
        let mut result = execution_tools_proposal();
        result.proposed_roles[1].tools.push("run_command".into());
        let parsed = parse_response(&serde_json::to_string(&result).unwrap()).unwrap();
        assert!(parsed.proposed_roles[1]
            .tools
            .iter()
            .any(|tool| tool == "run_command"));
        assert!(execution_role_tools(AgentRoleKind::Reviewer)
            .iter()
            .any(|tool| tool == "run_command"));
        for mutating in ["write_file", "edit_file", "apply_patch", "batch_edit"] {
            assert!(!execution_role_tools(AgentRoleKind::Reviewer)
                .iter()
                .any(|tool| tool == mutating));
        }
    }

    #[test]
    fn prompt_supplies_the_same_execution_catalog_used_for_validation() {
        let prompt = system_prompt(None);
        let (_, catalog) = prompt
            .split_once("available_execution_tools (exact identifiers by execution role kind):\n")
            .unwrap();
        let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap();
        for (label, kind) in [
            ("worker", AgentRoleKind::Worker),
            ("reviewer", AgentRoleKind::Reviewer),
        ] {
            let tools = execution_role_tools(kind);
            assert_eq!(catalog[label], serde_json::json!(tools));
            for excluded in ["ask_user", "todowrite", "editor", "find"] {
                assert!(!tools.iter().any(|tool| tool == excluded));
            }
        }
        assert!(SYSTEM_PROMPT.contains("When you genuinely need a user decision, use ask_user"));
        assert!(SYSTEM_PROMPT.contains("Track multi-step work with todowrite"));
        assert!(!SYSTEM_PROMPT.contains("tool family, not credentials"));
        assert!(SYSTEM_PROMPT.contains("require renewed approval"));
    }

    #[test]
    fn rejects_unstructured_or_incomplete_responses() {
        assert!(parse_response("I made a plan").is_err());
        let legacy_ambiguous =
            parse_response(r#"{"assistant_message":"Okay","outcome":"","plan_summary":"Plan"}"#)
                .unwrap();
        assert_eq!(legacy_ambiguous.turn_kind, PrincipalTurnKind::Conversation);
        assert!(legacy_ambiguous.plan_summary.is_empty());
        assert!(parse_response(
            r#"{"turn_kind":"proposal","assistant_message":"Okay","outcome":"","plan_summary":"Plan"}"#
        )
        .is_err());
    }

    #[test]
    fn specialized_role_labels_map_to_canonical_authority_kinds() {
        let architecture: PrincipalRoleDraft = serde_json::from_str(
            r#"{
              "kind": "architecture",
              "name": "Architect",
              "mission": "Design the boundary",
              "rationale": "Cross-system reasoning"
            }"#,
        )
        .unwrap();
        let security_reviewer: PrincipalRoleDraft = serde_json::from_str(
            r#"{
              "kind": "security_reviewer",
              "name": "Security reviewer",
              "mission": "Verify the boundary",
              "rationale": "Independent verification"
            }"#,
        )
        .unwrap();

        assert_eq!(architecture.kind, AgentRoleKind::Worker);
        assert_eq!(security_reviewer.kind, AgentRoleKind::Reviewer);
    }

    #[test]
    fn request_preserves_prior_conversation_for_explicit_confirmation() {
        let history = vec![
            (
                "user".into(),
                "Could we explore a safer deployment flow?".into(),
            ),
            (
                "principal".into(),
                "I would shape a staged rollout with rollback evidence.".into(),
            ),
        ];
        let (_, prompt) = build_request(
            "New work",
            "Yes, shape that work item.",
            None,
            None,
            &[],
            &[],
            &history,
            None,
            &[],
            &[],
            &[],
            &[],
        );

        let user_index = prompt.find("safer deployment flow").unwrap();
        let principal_index = prompt.find("staged rollout").unwrap();
        let confirmation_index = prompt.find("Yes, shape that work item.").unwrap();
        assert!(user_index < principal_index);
        assert!(principal_index < confirmation_index);
    }

    #[test]
    fn principal_model_routes_must_be_explicit_and_catalog_backed() {
        let mut result = parse_response(
            r#"{
              "turn_kind": "proposal",
              "assistant_message": "Ready for review",
              "outcome": "Ship billing",
              "requirements": ["Bill seats"],
              "open_decisions": [],
              "acceptance_criteria": ["Invoices are downloadable"],
              "plan_summary": "Design, implement, and review",
              "plan_tasks": [{
                "key": "architecture",
                "title": "Design billing",
                "description": "Produce the implementation contract",
                "depends_on": [],
                "assigned_role": "Architect",
                "acceptance_criteria": ["Invoices are downloadable"]
              }],
              "proposed_roles": [{
                "name": "Architect",
                "mission": "Design billing",
                "rationale": "Use stronger reasoning for cross-system architecture",
                "capabilities": [],
                "tools": [],
                "write_scope": [],
                "preferred_model_route": "large/reasoning"
              }]
            }"#,
        )
        .unwrap();
        let catalog = vec![PrincipalModelRoute {
            route: "large/reasoning".into(),
            label: "Large reasoning".into(),
            detail: "Architecture and review".into(),
        }];
        validate_model_routes(&result, &catalog).unwrap();

        result.proposed_roles[0].preferred_model_route = Some("unknown/model".into());
        assert!(validate_model_routes(&result, &catalog)
            .unwrap_err()
            .contains("unavailable model route"));
        result.proposed_roles[0].preferred_model_route = None;
        assert!(validate_model_routes(&result, &catalog)
            .unwrap_err()
            .contains("must select a model route"));
    }

    #[test]
    fn rejects_plan_tasks_that_cannot_drive_the_proposed_team() {
        let response = r#"{
          "turn_kind": "proposal",
          "assistant_message": "Ready for review",
          "outcome": "Ship billing",
          "requirements": ["Bill seats"],
          "open_decisions": [],
          "acceptance_criteria": ["Invoices are downloadable"],
          "plan_summary": "Implement and review",
          "plan_tasks": [{
            "key": "implementation",
            "title": "Implement billing",
            "description": "Build the billing flow",
            "depends_on": ["missing-task"],
            "assigned_role": "Backend Engineer",
            "acceptance_criteria": ["Invoices are downloadable"]
          }],
          "proposed_roles": [{
            "name": "Backend Engineer",
            "mission": "Build billing",
            "rationale": "Own implementation",
            "capabilities": [],
            "tools": [],
            "write_scope": []
          }]
        }"#;

        assert!(parse_response(response)
            .unwrap_err()
            .contains("invalid dependency"));
    }

    #[test]
    fn repository_context_is_added_only_when_available() {
        assert_eq!(system_prompt(None), system_prompt(Some("   ")));
        assert!(system_prompt(None).starts_with(SYSTEM_PROMPT));
        let prompt = system_prompt(Some("### Cargo.toml\n[package]"));
        assert!(prompt.contains("read-only, potentially incomplete evidence"));
        assert!(prompt.contains("### Cargo.toml\n[package]"));
    }

    #[test]
    fn repository_references_are_project_relative_deduplicated_and_grounded() {
        let mut parsed = parse_response(
            r#"{
              "turn_kind": "proposal",
              "assistant_message": "Ready for review",
              "outcome": "Ship billing",
              "requirements": ["Bill seats"],
              "open_decisions": [],
              "acceptance_criteria": ["Invoices are downloadable"],
              "plan_summary": "Implement and review",
              "plan_tasks": [{
                "key": "implementation",
                "title": "Implement billing",
                "description": "Build the billing flow",
                "depends_on": [],
                "assigned_role": "Backend Engineer",
                "acceptance_criteria": ["Invoices are downloadable"]
              }],
              "proposed_roles": [{
                "name": "Backend Engineer",
                "mission": "Build billing",
                "rationale": "Own implementation",
                "capabilities": [],
                "tools": [],
                "write_scope": []
              }],
              "repository_references": ["./server/billing.rs", "server/billing.rs"]
            }"#,
        )
        .unwrap();

        assert_eq!(parsed.repository_references, ["server/billing.rs"]);
        validate_repository_references(
            &parsed,
            Some("### Code likely relevant\n#### server/billing.rs\n"),
        )
        .unwrap();
        assert!(validate_repository_references(&parsed, Some("### Cargo.toml")).is_err());

        parsed.repository_references = vec!["../outside.rs".into()];
        assert!(normalize_repository_references(parsed.repository_references).is_err());
    }

    #[test]
    fn memory_notes_normalize_kinds_bounds_and_dedup() {
        assert_eq!(normalize_memory_kind("Decision"), "decision");
        assert_eq!(normalize_memory_kind("code-review"), "note");
        assert_eq!(normalize_memory_kind(""), "note");
        let notes = normalize_memory_notes(vec![
            PrincipalMemoryNote {
                kind: "preference".into(),
                text: "  Prefers Postgres  ".into(),
            },
            PrincipalMemoryNote {
                kind: "preference".into(),
                text: "Prefers Postgres".into(),
            },
            PrincipalMemoryNote {
                kind: "whatever".into(),
                text: "".into(),
            },
        ]);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].kind, "preference");
        assert_eq!(notes[0].text, "Prefers Postgres");
    }

    #[test]
    fn memory_notes_survive_conversation_turns() {
        let parsed = parse_response(
            r#"{"turn_kind":"conversation","assistant_message":"Noted.","memory_notes":[{"kind":"preference","text":"User prefers SQLite for local dev"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.turn_kind, PrincipalTurnKind::Conversation);
        assert_eq!(parsed.memory_notes.len(), 1);
        assert_eq!(parsed.memory_notes[0].kind, "preference");
    }
}
