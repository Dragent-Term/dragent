//! Principal proposal validation: routes, tools, plans, questions.
//!
//! Moved verbatim from shape_principal; same behavior.

//! Principal shaping turn for the Shape workspace.
//!
//! The model proposes structured work and may research the project with
//! read-only tools (same terminal-agent rhythm: Working → Reading/Searching →
//! Streaming); it still cannot execute commands, edit files, create runtime
//! agents, or grant authority. Parsing and normalization happen off the UI
//! thread before the result reaches workspace state.

use super::principal_normalize::*;
use super::principal_stream::*;
use super::principal_text::*;
use super::principal_types::*;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

pub(crate) fn finish_response_with_observed(
    content: &str,
    available_model_routes: &[PrincipalModelRoute],
    repository_context: Option<&str>,
    observed_files: &[String],
) -> Result<PrincipalShapeResult, String> {
    let result = parse_response(content)?;
    validate_model_routes(&result, available_model_routes)?;
    validate_repository_references_with_observed(&result, repository_context, observed_files)?;
    Ok(result)
}

pub(crate) fn validate_model_routes(
    result: &PrincipalShapeResult,
    available_model_routes: &[PrincipalModelRoute],
) -> Result<(), String> {
    let allowed = available_model_routes
        .iter()
        .map(|model| model.route.as_str())
        .collect::<HashSet<_>>();
    for role in &result.proposed_roles {
        let Some(route) = role.preferred_model_route.as_deref() else {
            return Err(format!(
                "Principal role '{}' must select a model route before plan approval",
                role.name
            ));
        };
        if !allowed.contains(route) {
            return Err(format!(
                "Principal role '{}' selected unavailable model route '{}'",
                role.name, route
            ));
        }
    }
    Ok(())
}

pub(crate) fn execution_role_tools(kind: AgentRoleKind) -> Vec<String> {
    let mut tools = [
        "read_file",
        "write_file",
        "edit_file",
        "list_directory",
        "grep",
        "search_codebase",
        "run_command",
        "batch_edit",
        "apply_patch",
        "fetch_url",
        "web_search",
        "skill",
        "repl",
        "sleep",
    ]
    .into_iter()
    .map(str::to_string)
    .chain(
        crate::agent::local_tools::filesystem_tool_defs()
            .into_iter()
            .filter(|(name, _, _)| {
                matches!(
                    name.as_str(),
                    "read_multiple_files"
                        | "create_directory"
                        | "list_directory_with_sizes"
                        | "directory_tree"
                        | "move_file"
                        | "search_files"
                        | "get_file_info"
                        | "list_allowed_directories"
                )
            })
            .map(|(name, _, _)| name),
    )
    // Connected MCP integrations are offered to roles by their exact tool
    // names, like every other identifier — the Principal grants only what a
    // role's mission needs, and the contract allowlist is what the provider
    // sees.
    .chain(
        crate::agent::mcp::all_tool_defs()
            .into_iter()
            .map(|(name, _, _)| name),
    )
    .filter(|name| {
        // Reviewer roles never get file-mutating tools (write_file, edit_file,
        // apply_patch, batch_edit, delete_file, ...) — a reviewer inspects a
        // candidate checkout, it doesn't change it. `run_command` is granted
        // anyway despite `is_mutating_tool` conservatively treating it as
        // mutating with no command in hand yet at proposal time: a
        // specially-missioned Reviewer (QA) needs to run the test/build/lint
        // suite to do its job, and command execution isn't gated by the
        // read-only-checkout contract the way file writes are.
        kind != AgentRoleKind::Reviewer
            || name == "run_command"
            || !crate::agent::approval_policy::is_mutating_tool(name, None)
    })
    .collect::<Vec<_>>();
    tools.sort();
    tools.dedup();
    tools
}

pub(crate) fn validate_role_tools(roles: &[PrincipalRoleDraft]) -> Result<(), String> {
    let worker_tools = execution_role_tools(AgentRoleKind::Worker);
    let reviewer_tools = execution_role_tools(AgentRoleKind::Reviewer);
    for role in roles {
        let allowed = if role.kind == AgentRoleKind::Reviewer {
            &reviewer_tools
        } else {
            &worker_tools
        };
        for tool in &role.tools {
            if !allowed.contains(tool) {
                return Err(format!(
                    "Principal proposal role '{}' selected unsupported tool identifier '{}'; choose only exact identifiers from available_execution_tools for this role kind. Tool families and aliases are not accepted; changes to approved tools require renewed approval",
                    role.name, tool
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn system_prompt(repository_context: Option<&str>) -> String {
    let mut prompt = SYSTEM_PROMPT.to_string();
    prompt.push_str("\n\navailable_execution_tools (exact identifiers by execution role kind):\n");
    prompt.push_str(
        &serde_json::json!({
            "worker": execution_role_tools(AgentRoleKind::Worker),
            "reviewer": execution_role_tools(AgentRoleKind::Reviewer),
        })
        .to_string(),
    );
    if let Some(context) = repository_context.filter(|context| !context.trim().is_empty()) {
        prompt.push_str("\n\nThe following repository context was gathered locally under the user's configured context scope. Treat it as read-only, potentially incomplete evidence. Base repository-specific claims only on this context, distinguish assumptions from observed facts, and list every project-relative file that materially informed the proposal in repository_references. Never invent a path.\n");
        prompt.push_str(context);
    }
    prompt
}

pub(crate) const SYSTEM_PROMPT: &str = r#"You are the Principal shaping agent for a collaborative engineering workspace.
Your job in this turn is to research the project with read-only tools and structure the work. You may call read_file, read_multiple_files, list_directory, directory_tree, get_file_info, grep, find, search_files, and search_codebase to establish findings. Track multi-step work with todowrite ({"todos":[{"content": task, "status": pending|in_progress|completed|cancelled, "priority": high|medium|low}]}) — use it proactively only when the turn requires 3+ distinct steps or the user gives multiple tasks. Capture todos when such work starts, keep exactly one `in_progress` at a time, and mark `completed` only after the work is actually done. Never call todowrite for greetings, chit-chat, single questions, approvals, confirmations, or text-only replies; a turn with nothing to track needs no task list. Act, don't narrate: never end a turn with only a promise of future work ("I will check…", "I can next research…"). When the user asks you to research, proceed, go ahead, or continue, your FIRST actions in this turn must be todowrite plus read calls (read_file, list_directory, grep, …); write the summary only after tool results arrive. A text-only reply is acceptable solely for greetings, thanks, farewells, or messages with no actionable request — for those, reply briefly and call no tools at all. When you genuinely need a user decision, use ask_user. Do not claim to run agents, execute commands, edit files, or complete implementation. Research findings must ground the proposal: base repository-specific claims only on observed tool evidence, distinguish assumptions from observed facts, and list every project-relative file that materially informed the proposal in repository_references. Never invent a path. When a missing product, security, data-loss, authorization, budget, or externally visible decision would materially change the result, call ask_user with 1–4 questions (each: question, optional header, options array where each option is a label string or {"label": label, "description": one-line meaning}) instead of guessing — the turn parks for the user's decision and resumes with the combined answers. Synthesize after at most 12 findings and return the final JSON.

Return exactly one JSON object with this schema:
{
  "turn_kind": "conversation or proposal",
  "assistant_message": "concise response to the user",
  "outcome": "clear desired outcome",
  "requirements": ["complete requirement"],
  "open_decisions": ["question that still needs a decision"],
  "acceptance_criteria": ["observable acceptance criterion"],
  "plan_summary": "short implementation approach",
  "plan_tasks": [
    {
      "key": "stable-kebab-case-key",
      "title": "concrete deliverable",
      "description": "bounded work and expected evidence",
      "depends_on": ["another-task-key"],
      "review_target": "for a reviewer-role task: the worker task key it reviews (also in depends_on); otherwise null",
      "assigned_role": "exact proposed role name",
      "acceptance_criteria": ["exact acceptance criterion text"]
    }
  ],
  "proposed_roles": [
    {
      "kind": "worker",
      "name": "specific role name",
      "mission": "bounded responsibility for this work",
      "rationale": "why this role is needed and why its model is the safest cost/capability choice",
      "capabilities": ["capability needed"],
      "tools": ["exact identifier from available_execution_tools for this role kind"],
      "write_scope": ["safe project-relative path"],
      "preferred_model_route": "exact route from available_model_routes (never null)"
    }
  ],
  "repository_references": ["project-relative/file.rs"],
  "memory_notes": [
    {"kind": "decision", "text": "Chose Postgres over SQLite for billing proration"}
  ]
}

You are first a conversational engineering collaborator. A conversation does not automatically mean the user wants to build something — they may be asking a general question, requesting an explanation, discussing an existing project, asking for advice, continuing a prior discussion, or just acknowledging what you said ("ok", "great", "sounds good", "thanks", "got it"). Respond naturally to all of that. Only when the conversation establishes genuine build intent — the user describing something they want built, changed, fixed, or investigated ("I want to build a CRM for my company", "I need an app where customers can book appointments", "add an authentication system to my project") — do you move toward structuring work, and even then not immediately: understand what they actually want first, asking only for genuinely missing, material information, before ever producing a proposal.

Choose turn_kind before doing anything else, and choose it fresh every turn from what THIS message actually is — never from momentum, and never from the fact that a proposal already exists:
- Use conversation for greetings, capability questions, brainstorming, ideation, tradeoff discussion, requirement gathering, clarification, incomplete or ambiguous intent, questions such as "what can you build for me?", a plain acknowledgment or filler reply ("ok", "great", "sounds good", "cool", "thanks", "got it", "I like it", "looks right"), a tangential or unrelated question asked mid-discovery ("what database would you recommend?", "explain how OAuth works" — answer it on its own merits, then let the user bring the work item back up), and a request to leave the work item entirely ("forget the project for now, explain X").
- Use proposal only when the user explicitly asks to build, change, implement, fix, investigate, or start/shape a concrete work item, AND there is enough information for a reviewable plan.
- A plain acknowledgment, agreement, or expression of satisfaction is NEVER by itself grounds for turn_kind proposal, even immediately after you described a work item in detail. "Ok", "sounds good", "great", "I like it", "looks right" are conversational replies, not confirmation to build and not approval — the user approves a shape only through the separate Approve Shape action outside this turn, never through anything said in conversation. If nothing in the message actually adds, changes, or removes a requirement, decision, or scope element, there is nothing to revise: reply conversationally and leave the existing outcome, plan, tasks, and roles exactly as they already are.
- Never interpret ordinary conversation as permission to create or schedule work.
- A conversation may give a detailed explanation of the work item you would create and ask whether the user wants you to shape it.
- For conversation, set outcome and plan_summary to empty strings and every array to []. Do not propose roles or tasks. This describes only what you return for THIS turn's JSON — it never erases the durable, already-stored proposal, plan, or roles, which stay exactly as they are until a turn that genuinely earns turn_kind proposal changes them.
- Do not ask a question merely to confirm an ordinary engineering default. Infer reversible, low-risk defaults from the repository and explain the assumption.
- Ask only when a missing product, security, data-loss, authorization, budget, or externally visible decision would materially change the result. Ask the smallest number of questions needed, and provide concrete options with a recommendation.
- Once you have enough information to confidently describe what should be built, stop asking further questions. Before the proposal itself, use assistant_message to explain back your understanding ("Based on our conversation, here is what I understand you want to build…") summarizing the product, functionality, workflows, requirements, and constraints, so the user can see exactly what is about to be proposed.
- If the request and repository already provide enough information, produce the complete proposal without asking the user to repeat requirements or approve intermediate reasoning.

For proposal, the arrays, proposal, plan tasks, and proposed roles must represent the complete current understanding, not only the latest delta. Propose 1-7 roles and a dependency-ordered plan of 1-30 tasks. Every task key must be unique. Dependencies must reference task keys and the graph must be acyclic. Assign every task to exactly one proposed role using its exact name, and give every proposed role at least one task. Every acceptance criterion must be covered by at least one task, using the exact criterion text. Tasks are autonomous handoffs: describe the deliverable and evidence, not a future conversation with the user. Set role kind to worker for implementation, architecture, research, and coordination roles; set it to reviewer only for an independent read-only verification role that produces findings. When the work changes code or configuration, the team ALWAYS includes: (1) a code reviewer role with one review task per implementation task; (2) a security reviewer role whose single task reviews the final implementation task (which builds on all the others) for security issues (secrets, injection, authorization, unsafe input, data exposure); (3) a QA role whose single task runs the project's test suite and any scripted checks against that same final implementation task and reports failures as findings. The plan policy given with an approved shaping turn states this structure exactly. Every implementation task's description must require the worker to write comprehensive automated tests for what it implements and to validate its own work (run its tests and checks) before reporting done — a task without tests is not complete. Reviewer and QA roles never write, edit, or patch files — they inspect a read-only candidate checkout. The QA role is the one exception allowed to use run_command, strictly to execute the project's existing test suite and scripted checks (e.g. cargo test, npm test, a lint or build command), never to change files. Every reviewer task must set review_target to exactly one worker task key and include that key in depends_on; non-reviewer tasks must set review_target to null. One review task never covers several implementation tasks. Reviewer roles must have an empty write_scope. Roles are reviewable drafts only: never claim they are running, never grant secrets, and use only project-relative write scopes without '..'. Select preferred_model_route from available_model_routes for every role. Use the least expensive suitable model: reserve stronger reasoning models for architecture, ambiguous cross-system work, security, and final review; prefer smaller models for bounded implementation driven by a precise specification; and use cheap models first for extraction, filtering, and summarization. Never invent a route. Explain the cost/capability decision in the role rationale. The user can override each selection before approval. repository_references must contain only project-relative files actually present in supplied repository context and actually used to shape the response; otherwise return an empty array. Do not include markdown fences or additional keys.

Execution-role tools are exact, case-sensitive identifiers, not tool families, capabilities, credentials, wildcards, or aliases. For each proposed role choose only the minimal tools needed from available_execution_tools for its kind; use [] when no tools are needed. Workers implementing code typically need read_file, write_file or edit_file, and run_command for checks. Reviewers must use only the reviewer identifiers, which exclude every file-mutating tool; run_command is included among them so the QA role can execute the test suite and scripted checks, but a code or security reviewer role should omit run_command unless its mission specifically requires running something. ask_user and todowrite belong to the Principal's own gathering loop, not workspace execution roles; never include them in proposed_roles.tools. Listing a tool does not grant command execution, write scope, or any other permission. Never expand family names into tools or silently broaden an approved contract: if legacy approved tools are unsupported, explain the required correction and require renewed approval rather than claiming the old approval covers it.

Revision discipline — this section applies only once you have already chosen turn_kind proposal for a genuine reason above; it is not itself a reason to choose proposal. When a structured shaping state already exists and this turn does warrant a revision, you are revising it, not starting over. Act like the senior engineer who owns this plan:
- Analyze impact first: decide which existing requirements, decisions, tasks, and roles the new message touches, and change only those. Extend the plan; never rebuild or replace it unless the user explicitly asks for a replacement or the new requirement fundamentally invalidates the previous plan.
- Preserve identity: reproduce every untouched role name, task key, requirement, criterion, and decision text byte-identically. A rename silently orphans approvals and history, so keep names and keys stable unless that item itself is what changes.
- Approved roles and resolved decisions are sacrosanct: keep them exactly as they are unless the new request directly contradicts them. If it does, reconcile with the smallest reasonable change and state what you changed and why in assistant_message.
- Propagate intelligently: when a change affects related work, update dependencies, review targets, acceptance criteria, test/verification tasks, and the plan summary to match — infer the obvious engineering implications yourself instead of asking the user to micromanage each edit.
- Resolve conflict by minimal disturbance: if the new instruction conflicts with an earlier decision, keep all unrelated work intact and alter only the conflicting portion.
- Ask only on genuine material ambiguity: never ask the user which existing sections to change or how — that is your judgment call. Ask only when the request admits meaningfully different outcomes you cannot responsibly choose between.

Work-item memory — every turn, record up to a few durable facts in memory_notes so future turns understand this work item without re-deriving everything. Each note is {"kind": one of decision, approval, preference, change, rejected, note, "text": one sentence}. Record: decisions reached (with the reason), user preferences stated ("prefers X", "never Y"), what this turn changed relative to the prior shape and why (kind change, only when something actually changed), and options you considered and rejected with cause. Skip trivia, tool play-by-plays, and anything already captured in requirements or acceptance criteria. On a first turn with no history, memory_notes may be []."#;

/// Parse the shape JSON, tolerating prose around the object (models often
/// preface the JSON with a sentence). Falls back to the first `{` … last `}`
/// span before failing.
pub(crate) fn parse_shape_json(json: &str) -> Result<PrincipalShapeResult, String> {
    if let Ok(result) = serde_json::from_str::<PrincipalShapeResult>(json) {
        return Ok(result);
    }
    if let (Some(start), Some(end)) = (json.find('{'), json.rfind('}')) {
        if end > start {
            if let Ok(result) = serde_json::from_str::<PrincipalShapeResult>(&json[start..=end]) {
                return Ok(result);
            }
        }
    }
    serde_json::from_str::<PrincipalShapeResult>(json)
        .map_err(|error| format!("Invalid Principal response: {error}"))
}

/// Best-effort message when the model omitted `assistant_message`: the shape
/// itself is still valid and reviewable, so the turn should succeed instead of
/// surfacing the global "could not produce a consistent work proposal" error.
pub(crate) fn synthesize_assistant_message(result: &PrincipalShapeResult) -> String {
    if result.turn_kind == PrincipalTurnKind::Proposal {
        "I've structured the work — review the outcome, plan, and proposed team below.".into()
    } else {
        "Got it — tell me anything else to fold into the shape and I'll keep refining it.".into()
    }
}

pub(crate) fn strip_code_fence(trimmed: &str) -> &str {
    if trimmed.starts_with("```") {
        let without_open = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```JSON"))
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        without_open
            .strip_suffix("```")
            .unwrap_or(without_open)
            .trim()
    } else {
        trimmed
    }
}

pub(crate) fn parse_response(content: &str) -> Result<PrincipalShapeResult, String> {
    let json = strip_code_fence(content.trim());
    let mut result = parse_shape_json(json)?;
    if result.turn_kind == PrincipalTurnKind::Proposal {
        validate_role_tools(&result.proposed_roles)?;
    }
    result.assistant_message = normalize_prose(result.assistant_message);
    if result.assistant_message.is_empty() {
        result.assistant_message = synthesize_assistant_message(&result);
    }
    result.outcome = normalize_text(result.outcome);
    result.plan_summary = normalize_text(result.plan_summary);
    result.requirements = normalize_items(result.requirements);
    result.open_decisions = normalize_items(result.open_decisions);
    result.acceptance_criteria = normalize_items(result.acceptance_criteria);
    result.proposed_roles = normalize_roles(result.proposed_roles);
    result.plan_tasks = normalize_plan_tasks(result.plan_tasks);
    result.repository_references = normalize_repository_references(result.repository_references)?;
    repair_plan_references(&mut result);
    // Memory survives both turn kinds: even a pure conversation can record a
    // user preference or a settled decision for the brain.
    result.memory_notes = normalize_memory_notes(result.memory_notes);
    if result.assistant_message.is_empty() {
        return Err("Principal response did not include a message".into());
    }
    if result.turn_kind == PrincipalTurnKind::Conversation {
        // Conversation must remain inert even if a provider includes stale or
        // speculative structured fields. The UI persists only the discussion.
        result.outcome.clear();
        result.requirements.clear();
        result.open_decisions.clear();
        result.acceptance_criteria.clear();
        result.plan_summary.clear();
        result.plan_tasks.clear();
        result.proposed_roles.clear();
        result.repository_references.clear();
        return Ok(result);
    }
    if result.outcome.is_empty() {
        return Err("Principal response did not include an outcome".into());
    }
    if result.plan_summary.is_empty() {
        return Err("Principal response did not include a plan summary".into());
    }
    if result.acceptance_criteria.is_empty() {
        return Err("Principal response did not include acceptance criteria".into());
    }
    if result.proposed_roles.is_empty() {
        return Err("Principal response did not include a proposed team".into());
    }
    if let Some(role) = result
        .proposed_roles
        .iter()
        .find(|role| role.kind == AgentRoleKind::Reviewer && !role.write_scope.is_empty())
    {
        return Err(format!(
            "Reviewer role '{}' must remain read-only",
            role.name
        ));
    }
    validate_plan_tasks(&result)?;
    Ok(result)
}

pub(crate) fn normalize_repository_references(
    references: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for reference in references.into_iter().take(MAX_REPOSITORY_REFERENCES) {
        let reference = reference.trim().replace('\\', "/");
        let reference = reference.strip_prefix("./").unwrap_or(&reference);
        if reference.is_empty() {
            continue;
        }
        let path = Path::new(reference);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(format!(
                "Principal response contains unsafe repository reference '{reference}'"
            ));
        }
        let reference = path.to_string_lossy().into_owned();
        if !normalized.contains(&reference) {
            normalized.push(reference);
        }
    }
    Ok(normalized)
}

pub(crate) fn validate_repository_references(
    result: &PrincipalShapeResult,
    repository_context: Option<&str>,
) -> Result<(), String> {
    validate_repository_references_with_observed(result, repository_context, &[])
}

pub(crate) fn validate_repository_references_with_observed(
    result: &PrincipalShapeResult,
    repository_context: Option<&str>,
    observed_files: &[String],
) -> Result<(), String> {
    let context = repository_context.unwrap_or_default();
    let observed: HashSet<&str> = observed_files.iter().map(String::as_str).collect();
    for reference in &result.repository_references {
        // Durable findings: files actually read via Principal research tools
        // count as grounded evidence alongside the pre-built snapshot context.
        if observed.contains(reference.as_str()) {
            continue;
        }
        if context.is_empty() || !context.contains(reference) {
            return Err(format!(
                "Principal response cites repository path '{reference}' that was not supplied in repository context"
            ));
        }
    }
    Ok(())
}

/// Memory-first routing for worker escalations: answer from prior work when
/// the question overlaps durable memory/records/evidence, otherwise do a
/// bounded findings turn first and only then escalate to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WorkerQuestionRoute {
    /// Reply directly with cited prior context.
    AnswerFromMemory,
    /// Run read-only findings before answering.
    NeedsFindings,
}

pub(crate) fn route_worker_question(
    question: &str,
    memory_text: &str,
    shaping_text: &str,
    evidence_text: &str,
) -> WorkerQuestionRoute {
    let keywords: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|w| {
            let w = w.to_ascii_lowercase();
            (w.len() >= 4).then_some(w)
        })
        .collect();
    if keywords.is_empty() {
        return WorkerQuestionRoute::NeedsFindings;
    }
    let combined = format!("{memory_text}\n{shaping_text}\n{evidence_text}").to_ascii_lowercase();
    let mut hits = 0usize;
    let mut seen: HashSet<&str> = HashSet::new();
    for word in &keywords {
        if seen.insert(word.as_str()) && combined.contains(word.as_str()) {
            hits += 1;
            if hits >= 2 {
                return WorkerQuestionRoute::AnswerFromMemory;
            }
        }
    }
    // Single long distinctive token (e.g. a file path or identifier) is enough.
    if keywords
        .iter()
        .any(|w| w.len() >= 8 && combined.contains(w.as_str()))
    {
        return WorkerQuestionRoute::AnswerFromMemory;
    }
    WorkerQuestionRoute::NeedsFindings
}
