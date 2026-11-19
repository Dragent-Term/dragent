//! Principal message text: replies, approvals, continuity, errors.
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
use super::principal_types::*;
use super::principal_validate::*;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

/// Handle social openers before asking a model for a complete engineering
/// proposal. A greeting is part of the Principal conversation, not evidence of
/// an executable outcome, and therefore must not create tasks or agents.
pub(crate) fn conversational_reply(message: &str) -> Option<&'static str> {
    let normalized = message
        .trim()
        .trim_matches(|character: char| character.is_ascii_punctuation())
        .split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if matches!(
        normalized.as_str(),
        "hi" | "hello"
            | "hey"
            | "hello there"
            | "hey there"
            | "howdy"
            | "howdy there"
            | "hiya"
            | "yo"
            | "greetings"
            | "good day"
            | "good morning"
            | "good afternoon"
            | "good evening"
            | "how are you"
            | "how are you doing"
    ) {
        return Some(
            "Hello! Tell me what you want to build, change, investigate, or decide. I’ll help you shape the outcome before any agents are scheduled.",
        );
    }
    if matches!(
        normalized.as_str(),
        "thanks" | "thank you" | "thanks a lot" | "thx" | "thank you very much"
    ) {
        return Some("You're welcome! Let me know what you'd like to shape next.");
    }
    if matches!(
        normalized.as_str(),
        "bye" | "goodbye" | "good night" | "goodnight" | "see you later" | "later"
    ) {
        return Some("Goodbye! I'll be here when you're ready to continue shaping.");
    }
    None
}

/// Deterministic shaping-approval detector for explicit user consent typed
/// in chat ("approve shape", "start shaping", …). Conservative by design:
/// the phrase must name shaping, must not be a question, and must not carry
/// a negation — so "go ahead" (continue research), "how will you shape it?"
/// or "don't shape yet" never unlock shaping. Approval via text is
/// equivalent to the Approve Shape button: both express explicit user
/// consent, which is what the Work Item Flow gate requires (it guards
/// against *agent-initiated* shaping, never against the user's own words).
pub(crate) fn is_shape_approval_text(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.ends_with('?') {
        return false;
    }
    for negation in [
        "don't", "do not", "dont", "never", "not ", "stop", "cancel", "wait",
    ] {
        if normalized.contains(negation) {
            return false;
        }
    }
    // NOTE: keep in sync with the hint shown under the Approve Shape button.
    [
        "approve shape",
        "start shaping",
        "create the shape",
        "shape it",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
}

/// Explicit user-requested removal or replacement: a removal verb plus a
/// mention of the item (full normalized name, or any distinctive 5+ char
/// word of it). Conservative on the verb side, lenient on the mention side —
/// natural language rarely repeats full titles.
pub(crate) fn explicit_removal(user_message: &str, item_name: &str) -> bool {
    const VERBS: &[&str] = &[
        "remov",
        "delet",
        "drop",
        "replac",
        "instead",
        "no longer",
        "don't need",
        "do not need",
        "start over",
        "from scratch",
        "restructur",
        "get rid of",
    ];
    let message = user_message.to_ascii_lowercase();
    if !VERBS.iter().any(|verb| message.contains(verb)) {
        return false;
    }
    let name = item_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if name.is_empty() {
        return false;
    }
    message.contains(&name)
        || name
            .split_whitespace()
            .any(|word| word.len() >= 5 && message.contains(word))
}

/// Declared full restructuring: the model says so and says why. Lets a
/// legitimate rebuild pass the continuity guard — the guard only catches
/// SILENT drops.
pub(crate) fn declares_restructure(assistant_message: &str) -> bool {
    let message = assistant_message.to_ascii_lowercase();
    [
        "restructur",
        "start over",
        "from scratch",
        "rebuilt",
        "replacing the plan",
    ]
    .iter()
    .any(|marker| message.contains(marker))
        && message.contains("because")
}

/// Intent-preserving revision check for proposal turns that revise an
/// existing shape: approved roles and resolved decisions must survive unless
/// the user explicitly removed them or the model declared a restructuring
/// with reasons. Returns a repairable error consumed by the existing repair
/// loop — the model restores the dropped item instead of the turn failing.
pub(crate) fn check_revision_continuity(
    result: &PrincipalShapeResult,
    user_message: &str,
    has_current_proposal: bool,
    approved_roles: &[String],
    resolved_questions: &[String],
) -> Result<(), String> {
    if !has_current_proposal || result.turn_kind != PrincipalTurnKind::Proposal {
        return Ok(());
    }
    if declares_restructure(&result.assistant_message) {
        return Ok(());
    }
    let normalize = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    };
    let new_roles = result
        .proposed_roles
        .iter()
        .map(|role| normalize(&role.name))
        .collect::<std::collections::HashSet<_>>();
    for role in approved_roles {
        if !new_roles.contains(normalize(role).as_str()) && !explicit_removal(user_message, role) {
            return Err(format!(
                "Principal revision dropped the approved role '{role}': restore it with identical terms, or explain the replacement and why in assistant_message"
            ));
        }
    }
    let new_questions = result
        .open_decisions
        .iter()
        .map(|question| normalize(question))
        .collect::<std::collections::HashSet<_>>();
    for question in resolved_questions {
        if !new_questions.contains(normalize(question).as_str())
            && !explicit_removal(user_message, question)
        {
            return Err(format!(
                "Principal revision dropped the resolved decision '{question}': reproduce it verbatim in open_decisions"
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_repairable_proposal_error(error: &str) -> bool {
    [
        "Invalid Principal response:",
        "Principal response did not include",
        "Principal plan",
        "Principal proposal role",
        "Principal revision",
        "Reviewer role",
        "Every proposed role",
        "Every acceptance criterion",
    ]
    .iter()
    .any(|prefix| error.starts_with(prefix))
}

pub(crate) fn user_facing_principal_error(error: &str) -> String {
    if is_repairable_proposal_error(error) {
        "The Principal could not produce a consistent work proposal, so nothing was scheduled. You can continue discussing the idea or ask the Principal to shape it again."
            .into()
    } else {
        error.to_string()
    }
}

/// Which stage of the Work Item Flow this turn serves. The shaping gate is
/// deterministic, never model-discretionary: Gather turns may research,
/// converse, and ask — but never emit proposal JSON. Only Shape turns may
/// create or revise the shape, and only after the user confirmed requirements
/// AND clicked Approve Shape (or when refining an already-shaped item).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrincipalTurnMode {
    Gather,
    Shape,
}

pub(crate) fn turn_mode_block(mode: PrincipalTurnMode) -> &'static str {
    match mode {
        PrincipalTurnMode::Gather => {
            r#"THIS TURN IS REQUIREMENTS GATHERING ONLY. Research the project with the read-only tools, converse with the user, and call ask_user for anything ambiguous, missing, conflicting, or blocking. For research spanning 3+ distinct steps, track it with todowrite; otherwise skip the task list entirely. When you believe you hold everything, end with a confirmation summary covering: what the user wants to achieve, the requirements collected, the proposed scope, what the system will do, and all assumptions, limitations, and unresolved details — then ask the user to confirm completeness and click Approve Shape. Include "requirements_complete": true in that response JSON so the Approve Shape button unlocks. Do NOT emit proposal, plan, task, or role JSON in this turn: turn_kind must stay conversation. If the user adds or changes requirements, fold them in and confirm again; never shape on your own judgement."#
        }
        PrincipalTurnMode::Shape => SHAPE_POLICY,
    }
}

/// The plan-construction policy for an approved shaping turn. It states, as
/// one procedure, exactly what `validate_plan_tasks` and the other proposal
/// checks enforce, so a compliant answer is accepted on its first pass: a
/// rejected proposal costs the person a whole repeated model pass.
pub(crate) const SHAPE_POLICY: &str = r#"REQUIREMENTS ARE CONFIRMED AND THE USER APPROVED SHAPING. Create (or revise) the shape now from the confirmed requirements and scope: return the complete proposal JSON this turn, grounded in the tool evidence gathered. Do not re-ask settled requirements; only genuinely new ambiguities justify ask_user. When revising, the revision discipline in the system prompt governs: minimal, intent-preserving evolution of the existing shape — never a rebuild.

PLAN POLICY. Dragent validates the proposal mechanically and rejects it whole if any rule below is broken. Follow it exactly; it overrides any looser wording elsewhere.

1. Answer. Exactly one JSON object, turn_kind "proposal", no markdown fences, no text before or after it. assistant_message is a short summary of the shape for the user.
2. Required content. outcome, plan_summary, at least one acceptance criterion, at least one role and at least one plan task are all non-empty.
3. Acceptance criteria. Write each top-level criterion once as an observable statement. Wherever a task lists a criterion, copy the full top-level text character for character — never a number, key, slug, summary or paraphrase. Every top-level criterion must be listed by at least one task.
4. Task keys and titles. Every key is unique lowercase kebab-case (letters, digits, hyphens). Every title is unique. depends_on lists only keys of other tasks in this plan, never the task itself, and the dependency graph has no cycles.
5. Roles. 1-7 roles with unique names. Every task's assigned_role is the exact name of one proposed role, and every role owns at least one task. kind is "worker" or "reviewer". Reviewer roles have an empty write_scope and no file-changing tools.
6. Tools. Each role lists only exact identifiers from available_execution_tools for its kind (or []). Never ask_user, todowrite, a family name, an alias or a wildcard.
7. Model routes. Every role sets preferred_model_route to an exact route from available_model_routes — never null, never invented. Routes beginning with "agent:" run an agent CLI (Claude Code, Codex, …) in its own terminal: use them only for worker roles, and only when the user asked for that agent. Reviewer roles always use a model route.
8. Reviews — the structure to use whenever the work changes code or configuration:
   a. Implementation tasks are assigned to worker roles, set review_target to null, and require the worker to write automated tests for what it builds and run them before reporting done.
   b. Code review: for EACH implementation task X, one task with key "review-X", assigned to the code reviewer role, depends_on ["X"], review_target "X". One review task covers exactly one implementation task.
   c. The final implementation task is the one that depends — directly or through other implementation tasks — on every other implementation task. If the implementation tasks are independent of each other, add a last worker task with key "integration" that depends on all of them, connects their work, runs the complete test suite and fixes what breaks; it is then the final implementation task.
   d. Security review: one task "security-review", assigned to the security reviewer role, review_target = the final implementation task's key, depends_on = every implementation task key.
   e. QA: one task "qa-validation", assigned to the QA role (kind reviewer), review_target = the final implementation task's key, depends_on = every implementation task key; its description runs the project's existing test suite and scripted checks; it lists every top-level acceptance criterion.
   f. review_target is set on reviewer-role tasks only, always to a worker task's key that is also in that task's own depends_on. Worker tasks always set review_target to null.
9. Size. At most 30 tasks: with code review for every implementation task, keep implementation tasks to 12 or fewer by grouping closely related work.

Before returning, check every rule above against your JSON and correct it. A plan that follows this policy is accepted on its first pass; any violation discards the whole shape and costs the user another wait."#;

/// Fail-closed enforcement for Gather turns: a proposal-shaped response is
/// downgraded to inert conversation, mirroring the stale-field stripping the
/// parser already applies to conversation turns.
pub(crate) fn downgrade_to_conversation(result: &mut PrincipalShapeResult) {
    result.turn_kind = PrincipalTurnKind::Conversation;
    result.outcome.clear();
    result.requirements.clear();
    result.open_decisions.clear();
    result.acceptance_criteria.clear();
    result.plan_summary.clear();
    result.plan_tasks.clear();
    result.proposed_roles.clear();
    result.repository_references.clear();
}

/// One provider pass the host runs through its `AgentController` — the
/// Principal's stream, tools, question cards and persistence are the
/// terminal agent's own; this crate only decides WHAT each pass asks.
pub(crate) struct PrincipalPassRequest {
    pub model: String,
    pub system: String,
    pub prompt: String,
    /// Provider-visible tools for this pass (`AgentController::set_allowed_tools`).
    pub allowed_tools: Vec<String>,
    /// Status text for a follow-up pass ("Continuing the interrupted response…").
    /// `None` on the pass that carries the user's own message.
    pub note: Option<String>,
}

/// What the host hands back once that pass reached `Done`.
pub(crate) struct PrincipalPassOutcome {
    /// Raw assistant text (the structured JSON, before any display filter).
    pub content: String,
    /// Every tool call of the pass — used for observed-file validation and
    /// the turn's findings digest.
    pub tool_calls: Vec<crate::agent::controller::TurnToolCall>,
}

/// Provider-visible tool offer for a shaping turn: the read-only research
/// family plus the two interactive tools the controller intercepts locally
/// (`ask_user`, `todowrite`). Leaving those out would silently reduce the
/// Principal to plain chat — the model can only call what the offer contains.
pub(crate) fn shaping_allowed_tools() -> Vec<String> {
    let mut allowed_tools: Vec<String> = PRINCIPAL_READ_TOOLS
        .iter()
        .map(|name| name.to_string())
        .collect();
    allowed_tools.push("ask_user".to_string());
    allowed_tools.push("todowrite".to_string());
    allowed_tools
}

/// Executed research calls per pass before the model is told to synthesize.
pub(crate) const MAX_PRINCIPAL_FINDINGS: u32 = 12;

/// Error text a pass returns once the host stopped the turn.
pub(crate) const PRINCIPAL_TURN_STOPPED: &str = "Principal turn was stopped";
