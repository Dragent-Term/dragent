//! Principal normalization: files, plans, roles, text.
//!
//! Moved verbatim from shape_principal; same behavior.

//! Principal shaping turn for the Shape workspace.
//!
//! The model proposes structured work and may research the project with
//! read-only tools (same terminal-agent rhythm: Working → Reading/Searching →
//! Streaming); it still cannot execute commands, edit files, create runtime
//! agents, or grant authority. Parsing and normalization happen off the UI
//! thread before the result reaches workspace state.

use super::principal_stream::*;
use super::principal_text::*;
use super::principal_types::*;
use super::principal_validate::*;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};

use serde::{Deserialize, Deserializer, Serialize};

use dragent_workspace_model::{AgentRoleKind, Plan, ShapingProposal};

pub(crate) fn observed_files_from_tool_args(args_json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args_json) else {
        return out;
    };
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            serde_json::Value::String(s) => {
                let trimmed = s.trim().replace('\\', "/");
                let trimmed = trimmed.strip_prefix("./").unwrap_or(&trimmed).to_string();
                if trimmed.is_empty()
                    || trimmed.contains("..")
                    || Path::new(&trimmed).is_absolute()
                    || !(trimmed.contains('/') || trimmed.contains('.'))
                {
                    continue;
                }
                if trimmed.len() > 256 {
                    continue;
                }
                if !out.contains(&trimmed) {
                    out.push(trimmed);
                }
            }
            serde_json::Value::Array(items) => stack.extend(items),
            serde_json::Value::Object(map) => stack.extend(map.into_values()),
            _ => {}
        }
    }
    out
}

pub(crate) fn normalize_plan_tasks(
    tasks: Vec<PrincipalPlanTaskDraft>,
) -> Vec<PrincipalPlanTaskDraft> {
    tasks
        .into_iter()
        .take(MAX_PLAN_TASKS)
        .map(|task| PrincipalPlanTaskDraft {
            key: normalize_plan_key(&task.key),
            title: normalize_text(task.title),
            description: normalize_text(task.description),
            depends_on: task
                .depends_on
                .into_iter()
                .map(|key| normalize_plan_key(&key))
                .filter(|key| !key.is_empty())
                .collect(),
            review_target: task
                .review_target
                .map(|key| normalize_plan_key(&key))
                .filter(|key| !key.is_empty()),
            assigned_role: normalize_text(task.assigned_role),
            acceptance_criteria: normalize_items(task.acceptance_criteria),
        })
        .collect()
}

pub(crate) fn normalize_plan_key(key: &str) -> String {
    key.trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Fix a proposal's mechanical slips in place instead of rejecting it. A
/// rejected proposal costs a whole extra model pass, which the person sees
/// as the Principal repeating the same summary. Only unambiguous fixes:
/// - a criterion cited by number, key or near-identical wording becomes the
///   one top-level criterion it plainly means;
/// - a reviewer task without a review target reviews its last worker
///   dependency, and a review target missing from `depends_on` joins it;
/// - a non-reviewer task's stray review target is dropped;
/// - criteria no task covers go to the final review task (else the last
///   task).
/// Everything else is left for [`validate_plan_tasks`] to reject.
pub(crate) fn repair_plan_references(result: &mut PrincipalShapeResult) {
    if result.turn_kind != PrincipalTurnKind::Proposal || result.plan_tasks.is_empty() {
        return;
    }
    let criteria = result.acceptance_criteria.clone();
    for task in &mut result.plan_tasks {
        let mut resolved: Vec<String> = Vec::new();
        for cited in &task.acceptance_criteria {
            let criterion = resolve_criterion(cited, &criteria).unwrap_or_else(|| cited.clone());
            if !resolved.contains(&criterion) {
                resolved.push(criterion);
            }
        }
        task.acceptance_criteria = resolved;
    }
    let reviewer_roles: HashSet<String> = result
        .proposed_roles
        .iter()
        .filter(|role| role.kind == AgentRoleKind::Reviewer)
        .map(|role| role.name.clone())
        .collect();
    let order: HashMap<String, usize> = result
        .plan_tasks
        .iter()
        .enumerate()
        .map(|(index, task)| (task.key.clone(), index))
        .collect();
    let workers: HashSet<String> = result
        .plan_tasks
        .iter()
        .filter(|task| !reviewer_roles.contains(&task.assigned_role))
        .map(|task| task.key.clone())
        .collect();
    for task in &mut result.plan_tasks {
        if !reviewer_roles.contains(&task.assigned_role) {
            task.review_target = None;
            continue;
        }
        match task.review_target.clone() {
            Some(target) if order.contains_key(&target) && target != task.key => {
                if !task.depends_on.contains(&target) {
                    task.depends_on.push(target);
                }
            }
            _ => {
                task.review_target = task
                    .depends_on
                    .iter()
                    .filter(|dependency| workers.contains(*dependency))
                    .max_by_key(|dependency| order.get(*dependency).copied().unwrap_or(0))
                    .cloned();
            }
        }
    }
    let covered: HashSet<String> = result
        .plan_tasks
        .iter()
        .flat_map(|task| task.acceptance_criteria.iter().cloned())
        .collect();
    let uncovered: Vec<String> = criteria
        .iter()
        .filter(|criterion| !covered.contains(*criterion))
        .cloned()
        .collect();
    if !uncovered.is_empty() {
        let last_review = result
            .plan_tasks
            .iter()
            .rposition(|task| reviewer_roles.contains(&task.assigned_role));
        let index = last_review.unwrap_or(result.plan_tasks.len() - 1);
        result.plan_tasks[index].acceptance_criteria.extend(uncovered);
    }
}

/// The top-level criterion `cited` means, when that's unambiguous: the same
/// text up to case and punctuation, a position (`2`, `ac2`, `criterion-2`),
/// or the one criterion containing (or contained in) it.
fn resolve_criterion(cited: &str, criteria: &[String]) -> Option<String> {
    if criteria.iter().any(|criterion| criterion == cited) {
        return Some(cited.to_string());
    }
    let words = |text: &str| -> String {
        text.split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let cited_words = words(cited);
    if cited_words.is_empty() {
        return None;
    }
    let only = |matches: Vec<&String>| (matches.len() == 1).then(|| matches[0].clone());
    if let Some(same) = only(criteria.iter().filter(|c| words(c) == cited_words).collect()) {
        return Some(same);
    }
    let key = normalize_plan_key(cited);
    let position = key
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .parse::<usize>()
        .ok()
        .filter(|_| {
            let prefix = key.trim_end_matches(|character: char| character.is_ascii_digit());
            matches!(
                prefix.trim_end_matches('-'),
                "" | "ac" | "c" | "criterion" | "criteria" | "acceptance" | "acceptance-criterion"
                    | "acceptance-criteria"
            )
        });
    if let Some(position) = position {
        return position.checked_sub(1).and_then(|index| criteria.get(index)).cloned();
    }
    if cited_words.len() < 8 {
        return None;
    }
    only(
        criteria
            .iter()
            .filter(|criterion| {
                let criterion = words(criterion);
                criterion.contains(&cited_words) || cited_words.contains(&criterion)
            })
            .collect(),
    )
}

pub(crate) fn validate_plan_tasks(result: &PrincipalShapeResult) -> Result<(), String> {
    if result.plan_tasks.is_empty() {
        return Err("Principal response did not include plan tasks".into());
    }
    let role_names = result
        .proposed_roles
        .iter()
        .map(|role| role.name.as_str())
        .collect::<HashSet<_>>();
    let role_kinds = result
        .proposed_roles
        .iter()
        .map(|role| (role.name.as_str(), role.kind))
        .collect::<HashMap<_, _>>();
    let criterion_names = result
        .acceptance_criteria
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut task_indices = HashMap::new();
    let mut task_titles = HashSet::new();
    let mut assigned_roles = HashSet::new();
    let mut covered_criteria = HashSet::new();
    for (index, task) in result.plan_tasks.iter().enumerate() {
        if task.key.is_empty() || task.title.is_empty() || task.description.is_empty() {
            return Err("Principal plan tasks require a key, title, and description".into());
        }
        if task_indices.insert(task.key.as_str(), index).is_some() {
            return Err(format!(
                "Principal plan contains duplicate task key '{}'",
                task.key
            ));
        }
        if !task_titles.insert(task.title.to_lowercase()) {
            return Err(format!(
                "Principal plan contains duplicate task title '{}'",
                task.title
            ));
        }
        if !role_names.contains(task.assigned_role.as_str()) {
            return Err(format!(
                "Principal plan task '{}' references unknown role '{}'",
                task.key, task.assigned_role
            ));
        }
        if task.acceptance_criteria.is_empty() {
            return Err(format!(
                "Principal plan task '{}' has no acceptance criteria",
                task.key
            ));
        }
        assigned_roles.insert(task.assigned_role.as_str());
        for criterion in &task.acceptance_criteria {
            if !criterion_names.contains(criterion.as_str()) {
                return Err(format!(
                    "Principal plan task '{}' references unknown acceptance criterion",
                    task.key
                ));
            }
            covered_criteria.insert(criterion.as_str());
        }
    }
    for task in &result.plan_tasks {
        for dependency in &task.depends_on {
            if dependency == &task.key || !task_indices.contains_key(dependency.as_str()) {
                return Err(format!(
                    "Principal plan task '{}' has an invalid dependency '{}'",
                    task.key, dependency
                ));
            }
        }
        let reviewer = role_kinds.get(task.assigned_role.as_str())
            == Some(&dragent_workspace_model::AgentRoleKind::Reviewer);
        match (reviewer, task.review_target.as_deref()) {
            (true, Some(target))
                if task
                    .depends_on
                    .iter()
                    .any(|dependency| dependency == target) => {}
            (true, _) => {
                return Err(format!(
                    "Reviewer task '{}' must target one of its dependencies",
                    task.key
                ));
            }
            (false, None) => {}
            (false, Some(_)) => {
                return Err(format!(
                    "Non-reviewer task '{}' cannot declare a review target",
                    task.key
                ));
            }
        }
    }
    if assigned_roles.len() != role_names.len() {
        return Err("Every proposed role must own at least one plan task".into());
    }
    if covered_criteria.len() != criterion_names.len() {
        return Err("Every acceptance criterion must be covered by the plan".into());
    }
    Ok(())
}

pub(crate) fn normalize_roles(roles: Vec<PrincipalRoleDraft>) -> Vec<PrincipalRoleDraft> {
    let mut normalized = Vec::new();
    for role in roles.into_iter().take(MAX_PROPOSED_ROLES) {
        let role = PrincipalRoleDraft {
            kind: role.kind,
            name: normalize_text(role.name),
            mission: normalize_text(role.mission),
            rationale: normalize_text(role.rationale),
            capabilities: normalize_items(role.capabilities),
            tools: normalize_items(role.tools),
            write_scope: normalize_items(role.write_scope),
            preferred_model_route: role.preferred_model_route.and_then(|route| {
                let route = route.trim();
                (!route.is_empty() && !route.eq_ignore_ascii_case("auto"))
                    .then(|| route.to_string())
            }),
        };
        if !role.name.is_empty()
            && !role.mission.is_empty()
            && !role.rationale.is_empty()
            && !normalized
                .iter()
                .any(|existing: &PrincipalRoleDraft| existing.name == role.name)
        {
            normalized.push(role);
        }
    }
    normalized
}

pub(crate) fn normalize_items(items: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for item in items.into_iter().take(MAX_ITEMS_PER_SECTION) {
        let item = normalize_text(item);
        if !item.is_empty() && !normalized.contains(&item) {
            normalized.push(item);
        }
    }
    normalized
}

pub(crate) fn normalize_text(text: String) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_ITEM_CHARS {
        text
    } else {
        text.chars().take(MAX_ITEM_CHARS).collect()
    }
}

/// Whitespace normalization WITHOUT the length cap — for the agent's own
/// prose (assistant_message), which must return completely like the terminal
/// agent: no silent mid-word truncation, no streaming/token ceiling on our
/// side. Bounded caps stay on short schema fields and list items only.
pub(crate) fn normalize_prose(text: String) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bounds for the per-work turn memory digest (opencode-style: bounded
/// entries, truncated lines — evidence stays available without bloating
/// every turn's prompt).
pub(crate) const TURN_MEMORY_ENTRIES: usize = 20;
pub(crate) const TURN_MEMORY_CHARS: usize = 300;

/// Append one digest line to the per-work turn memory, kept bounded like
/// opencode's compacted history: newest last, oldest dropped past the cap.
pub(crate) fn push_turn_memory(memory: &mut Vec<String>, line: String) {
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return;
    }
    let line: String = if line.chars().count() > TURN_MEMORY_CHARS {
        let mut short: String = line.chars().take(TURN_MEMORY_CHARS).collect();
        short.push('…');
        short
    } else {
        line
    };
    memory.push(line);
    while memory.len() > TURN_MEMORY_ENTRIES {
        memory.remove(0);
    }
}
