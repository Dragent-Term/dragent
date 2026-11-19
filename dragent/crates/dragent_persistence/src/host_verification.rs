//! Host-only command receipts. No model-facing RecordEvidence command can
//! write these rows. A receipt proves execution, not semantic test coverage.
use anyhow::{Context, Result};
use diesel::{prelude::*, sql_types::Text, sqlite::SqliteConnection};
use dragent_workspace_model::{AcceptanceCriterionId, ExecutionAttemptId, WorkItemId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandReceipt {
    pub id: String,
    pub work_item_id: WorkItemId,
    pub attempt_id: ExecutionAttemptId,
    pub candidate_attempt_id: ExecutionAttemptId,
    pub criteria: Vec<AcceptanceCriterionId>,
    pub command: String,
    pub cwd: String,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub output: String,
    pub snapshot_before: String,
    pub snapshot_after: Option<String>,
}

pub fn start(conn: &mut SqliteConnection, receipt: &CommandReceipt) -> Result<()> {
    anyhow::ensure!(
        receipt.finished_at_unix_ms.is_none() && receipt.exit_code.is_none(),
        "new command must be pending"
    );
    diesel::sql_query("INSERT INTO host_command_verifications(id, work_item_id, candidate_attempt_id, receipt_json) VALUES (?1, ?2, ?3, ?4)")
        .bind::<Text, _>(&receipt.id)
        .bind::<Text, _>(receipt.work_item_id.0.to_string())
        .bind::<Text, _>(receipt.candidate_attempt_id.0.to_string())
        .bind::<Text, _>(serde_json::to_string(receipt)?)
        .execute(conn).context("persist pending verification")?;
    Ok(())
}

pub fn finish(conn: &mut SqliteConnection, receipt: &CommandReceipt) -> Result<()> {
    anyhow::ensure!(
        receipt.finished_at_unix_ms.is_some(),
        "command has not finished"
    );
    let updated =
        diesel::sql_query("UPDATE host_command_verifications SET receipt_json = ?2 WHERE id = ?1")
            .bind::<Text, _>(&receipt.id)
            .bind::<Text, _>(serde_json::to_string(receipt)?)
            .execute(conn)?;
    anyhow::ensure!(updated == 1, "pending command receipt is missing");
    Ok(())
}

pub fn load(
    conn: &mut SqliteConnection,
    work: WorkItemId,
    candidate: ExecutionAttemptId,
) -> Result<Vec<CommandReceipt>> {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Text)]
        receipt_json: String,
    }
    diesel::sql_query("SELECT receipt_json FROM host_command_verifications WHERE work_item_id = ?1 AND candidate_attempt_id = ?2 ORDER BY rowid")
        .bind::<Text, _>(work.0.to_string())
        .bind::<Text, _>(candidate.0.to_string())
        .load::<Row>(conn)?.into_iter().map(|r| Ok(serde_json::from_str(&r.receipt_json)?)).collect()
}

/// Every mapped command must finish successfully. A later pass cannot hide a
/// failure; retry verification in a fresh reviewer attempt after fixing it.
pub fn passing_receipts<'a>(
    receipts: &'a [CommandReceipt],
    reviewer: ExecutionAttemptId,
    criterion: AcceptanceCriterionId,
) -> Result<Vec<&'a CommandReceipt>> {
    let relevant: Vec<_> = receipts
        .iter()
        .filter(|r| r.attempt_id == reviewer && r.criteria.contains(&criterion))
        .collect();
    anyhow::ensure!(!relevant.is_empty(), "acceptance criterion {} has no host-recorded verification; reviewer must run its checks with acceptance_criteria or request a user waiver", criterion.0);
    anyhow::ensure!(relevant.iter().all(|r| r.finished_at_unix_ms.is_some() && r.exit_code == Some(0)
        && !r.snapshot_before.is_empty() && r.snapshot_after.as_deref() == Some(r.snapshot_before.as_str())),
        "acceptance criterion {} has failed, unfinished, or unknown-exit verification; request a fresh review after resolving it", criterion.0);
    Ok(relevant)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt() -> CommandReceipt {
        CommandReceipt {
            id: "host-call-1".into(),
            work_item_id: WorkItemId::new(),
            attempt_id: ExecutionAttemptId::new(),
            candidate_attempt_id: ExecutionAttemptId::new(),
            criteria: vec![AcceptanceCriterionId::new()],
            command: "cargo test".into(),
            cwd: "/isolated/review".into(),
            started_at_unix_ms: 10,
            finished_at_unix_ms: None,
            exit_code: None,
            output: String::new(),
            snapshot_before: "candidate-bytes".into(),
            snapshot_after: None,
        }
    }
    #[test]
    fn verification_requires_matching_finished_successful_receipts() {
        let mut r = receipt();
        let check = |r: &CommandReceipt| {
            passing_receipts(std::slice::from_ref(r), r.attempt_id, r.criteria[0]).is_ok()
        };
        assert!(!check(&r));
        r.finished_at_unix_ms = Some(20);
        assert!(!check(&r));
        r.exit_code = Some(1);
        assert!(!check(&r));
        r.exit_code = Some(0);
        assert!(!check(&r));
        r.snapshot_after = Some(r.snapshot_before.clone());
        assert!(check(&r));
        assert!(passing_receipts(&[r.clone()], ExecutionAttemptId::new(), r.criteria[0]).is_err());
        assert!(
            passing_receipts(&[r.clone()], r.attempt_id, AcceptanceCriterionId::new()).is_err()
        );
        let mut failed = r.clone();
        failed.id = "earlier-failure".into();
        failed.exit_code = Some(2);
        assert!(passing_receipts(&[failed, r.clone()], r.attempt_id, r.criteria[0]).is_err());
    }
    #[test]
    fn receipts_survive_reopen_and_remain_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("verification.sqlite");
        let mut r = receipt();
        {
            let mut conn = crate::setup_connection(&path).unwrap();
            start(&mut conn, &r).unwrap();
            assert!(start(&mut conn, &r).is_err());
        }
        let mut conn = crate::setup_connection(&path).unwrap();
        let pending = load(&mut conn, r.work_item_id, r.candidate_attempt_id).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(passing_receipts(&pending, r.attempt_id, r.criteria[0]).is_err());
        r.finished_at_unix_ms = Some(20);
        r.exit_code = Some(0);
        r.output = "2 passed".into();
        r.snapshot_after = Some(r.snapshot_before.clone());
        finish(&mut conn, &r).unwrap();
        drop(conn);
        let mut conn = crate::setup_connection(&path).unwrap();
        let completed = load(&mut conn, r.work_item_id, r.candidate_attempt_id).unwrap();
        assert_eq!(completed[0].output, "2 passed");
        assert!(passing_receipts(&completed, r.attempt_id, r.criteria[0]).is_ok());
        assert!(load(&mut conn, WorkItemId::new(), r.candidate_attempt_id)
            .unwrap()
            .is_empty());
        assert!(load(&mut conn, r.work_item_id, ExecutionAttemptId::new())
            .unwrap()
            .is_empty());
    }
}
