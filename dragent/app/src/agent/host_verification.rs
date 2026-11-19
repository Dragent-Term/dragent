//! Binds trusted workspace identity to local shell execution. Tool arguments
//! may name criteria, but cannot choose a work item, candidate, or exit code.
use dragent_workspace_model::{AcceptanceCriterionId, ExecutionAttemptId, WorkItemId};
use once_cell::sync::Lazy;
use std::{collections::HashMap, sync::Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct VerificationScope {
    pub work_item_id: WorkItemId,
    pub attempt_id: ExecutionAttemptId,
    pub candidate_attempt_id: ExecutionAttemptId,
    pub criteria: Vec<AcceptanceCriterionId>,
    pub root: std::path::PathBuf,
}
static SCOPES: Lazy<Mutex<HashMap<Uuid, VerificationScope>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
pub fn bind(scope: VerificationScope) {
    SCOPES.lock().unwrap().insert(scope.attempt_id.0, scope);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verification_scope_cannot_be_chosen_by_tool_arguments() {
        let pane = ExecutionAttemptId::new();
        let criterion = AcceptanceCriterionId::new();
        bind(VerificationScope {
            work_item_id: WorkItemId::new(),
            attempt_id: pane,
            candidate_attempt_id: ExecutionAttemptId::new(),
            criteria: vec![criterion],
            root: "/candidate".into(),
        });
        let args = serde_json::json!({"acceptance_criteria": [criterion]}).to_string();
        assert!(is_scoped_check(pane.0, &args));
        assert!(!is_scoped_check(Uuid::new_v4(), &args));
        assert!(!is_scoped_check(pane.0, "{}"));
        assert!(!is_scoped_check(pane.0, "{\"acceptance_criteria\": []}"));
        assert!(!is_scoped_check(
            pane.0,
            &serde_json::json!({"acceptance_criteria": [AcceptanceCriterionId::new()]}).to_string()
        ));
        SCOPES.lock().unwrap().remove(&pane.0);
    }
}
pub type Completion = Box<dyn FnOnce(Option<i32>, String) + Send>;

/// This only selects the contract authorization path; it grants no permission.
pub fn is_scoped_check(pane: Uuid, args: &str) -> bool {
    let Ok(args) = serde_json::from_str::<serde_json::Value>(args) else {
        return false;
    };
    let Some(scope) = SCOPES.lock().unwrap().get(&pane).cloned() else {
        return false;
    };
    let Some(raw) = args.get("acceptance_criteria") else {
        return false;
    };
    serde_json::from_value::<Vec<AcceptanceCriterionId>>(raw.clone())
        .is_ok_and(|ids| !ids.is_empty() && ids.iter().all(|id| scope.criteria.contains(id)))
}

#[cfg(not(target_family = "wasm"))]
pub fn validate_review(pane: Uuid, waived: &[AcceptanceCriterionId]) -> anyhow::Result<()> {
    let scope = SCOPES
        .lock()
        .unwrap()
        .get(&pane)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("review has no host verification scope"))?;
    let path = dragent_persistence::db_path_for_read(&scope.root)?;
    let mut conn = dragent_persistence::setup_connection(&path)?;
    let receipts = dragent_persistence::host_verification::load(
        &mut conn,
        scope.work_item_id,
        scope.candidate_attempt_id,
    )?;
    let fingerprint = crate::git_status::worktree::verification_fingerprint(&scope.root)
        .map_err(anyhow::Error::msg)?;
    for id in scope.criteria.iter().filter(|id| !waived.contains(id)) {
        for receipt in dragent_persistence::host_verification::passing_receipts(
            &receipts,
            scope.attempt_id,
            *id,
        )? {
            anyhow::ensure!(
                receipt.snapshot_after.as_deref() == Some(fingerprint.as_str()),
                "candidate changed after verification; run a new independent review"
            );
        }
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
pub fn begin(
    pane: Uuid,
    call_id: &str,
    args: &serde_json::Value,
    command: &str,
    cwd: &Option<std::path::PathBuf>,
) -> anyhow::Result<Option<Completion>> {
    use dragent_persistence::host_verification::{self, CommandReceipt};
    let Some(scope) = SCOPES.lock().unwrap().get(&pane).cloned() else {
        return Ok(None);
    };
    let Some(raw) = args.get("acceptance_criteria") else {
        return Ok(None);
    };
    let criteria: Vec<AcceptanceCriterionId> = serde_json::from_value(raw.clone())?;
    anyhow::ensure!(
        !criteria.is_empty() && criteria.iter().all(|id| scope.criteria.contains(id)),
        "acceptance_criteria must name criteria assigned to this review"
    );
    let root = scope.root.canonicalize()?;
    let cwd = cwd.as_ref().unwrap_or(&scope.root).canonicalize()?;
    anyhow::ensure!(
        cwd.starts_with(&root),
        "verification must run inside the candidate checkout"
    );
    let snapshot =
        crate::git_status::worktree::verification_fingerprint(&root).map_err(anyhow::Error::msg)?;
    let path = dragent_persistence::db_path_for_write(&scope.root)?;
    let mut receipt = CommandReceipt {
        id: format!("{}:{call_id}:{}", scope.attempt_id.0, Uuid::new_v4()),
        work_item_id: scope.work_item_id,
        attempt_id: scope.attempt_id,
        candidate_attempt_id: scope.candidate_attempt_id,
        criteria,
        command: command.to_string(),
        cwd: cwd.display().to_string(),
        started_at_unix_ms: chrono::Utc::now().timestamp_millis(),
        finished_at_unix_ms: None,
        exit_code: None,
        output: String::new(),
        snapshot_before: snapshot,
        snapshot_after: None,
    };
    host_verification::start(&mut dragent_persistence::setup_connection(&path)?, &receipt)?;
    Ok(Some(Box::new(move |exit_code, output| {
        receipt.exit_code = exit_code;
        receipt.finished_at_unix_ms = Some(chrono::Utc::now().timestamp_millis());
        receipt.output = output.chars().take(8_000).collect();
        receipt.snapshot_after = crate::git_status::worktree::verification_fingerprint(&root).ok();
        let result = dragent_persistence::setup_connection(&path)
            .and_then(|mut conn| host_verification::finish(&mut conn, &receipt));
        if let Err(error) = result {
            // Pending receipt remains a fail-closed completion gate.
            log::error!("[verification] could not persist command completion: {error}");
        }
    })))
}

#[cfg(target_family = "wasm")]
pub fn begin(
    _: Uuid,
    _: &str,
    _: &serde_json::Value,
    _: &str,
    _: &Option<std::path::PathBuf>,
) -> anyhow::Result<Option<Completion>> {
    Ok(None)
}
