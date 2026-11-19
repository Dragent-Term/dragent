//! Host scheduler driver (Phase 4).
//!
//! Moves *when the scheduler runs and what feeds it* out of WorkspaceView
//! into the Host. The pure `evaluate_scheduler` algorithm is untouched.
//!
//! The loop runs headless: every tick (plus after every accepted workspace
//! command) each managed workspace with a structured plan is evaluated, and
//! a `SchedulerEvaluated` event is broadcast only when the decision set
//! changes. Effect execution (worker spawn) arrives in later phases — the
//! driver records decisions; it never forks execution authority.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::protocol::SchedulerStatus;
use dragent_workspace_model::{SchedulerProjection, WorkspaceState};

/// Concurrency capacity until the Phase-10 resource manager exists.
pub const DEFAULT_MAX_PARALLEL_TASKS: usize = 4;
/// Driver cadence. Evaluations are pure and cheap; the tick only re-runs
/// them, and broadcasts fire solely on decision change.
pub const DRIVER_TICK: Duration = Duration::from_secs(2);

/// True when a lease no longer authorizes its run: past its expiry, or
/// in a terminal status (released/expired/cancelled mean the holder gave
/// it up or the authority revoked it). Pure over fields — the watchdog
/// resolves the lease from canonical state, this only judges it.
pub fn lease_is_spent(lease: &dragent_workspace_model::ExecutionLease, now_unix_ms: i64) -> bool {
    !matches!(
        lease.status,
        dragent_workspace_model::ExecutionLeaseStatus::Active
    ) || lease.expires_at_unix_ms <= now_unix_ms
}

/// Evaluate the pure scheduler against one canonical state. Returns `None`
/// when there is no structured plan yet (nothing to decide).
pub fn evaluate_workspace(state: &WorkspaceState) -> Option<SchedulerProjection> {
    let plan = state.latest_structured_plan()?.clone();
    let contracts = state
        .work_contracts()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let criteria = state
        .shaping_records_projection()
        .map(|records| records.acceptance_criteria.clone())
        .unwrap_or_default();
    let evidence = state.evidence().into_iter().cloned().collect::<Vec<_>>();
    let attention = state
        .attention_items()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    Some(dragent_workspace_model::evaluate_scheduler(
        &plan,
        &contracts,
        &criteria,
        &evidence,
        &attention,
        DEFAULT_MAX_PARALLEL_TASKS,
    ))
}

/// Last broadcast decision per workspace, for change detection.
#[derive(Default)]
pub struct SchedulerDriver {
    last: Mutex<HashMap<String, SchedulerProjection>>,
}

impl SchedulerDriver {
    /// Evaluate one workspace; returns the projection when the decision set
    /// changed since the last broadcast (caller emits the event).
    pub fn evaluate(
        &self,
        workspace_id: &str,
        state: &WorkspaceState,
    ) -> Option<SchedulerProjection> {
        let projection = evaluate_workspace(state)?;
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let changed = last.get(workspace_id) != Some(&projection);
        if changed {
            last.insert(workspace_id.to_string(), projection.clone());
        }
        changed.then_some(projection)
    }

    pub fn last_projection(&self, workspace_id: &str) -> Option<SchedulerProjection> {
        self.last
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(workspace_id)
            .cloned()
    }

    pub fn forget(&self, workspace_id: &str) {
        self.last
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(workspace_id);
    }
}

/// Drive one evaluation pass over cloned snapshots. Pure function of states
/// (testable without threads): returns per-workspace projections that changed.
pub fn drive_once(
    driver: &SchedulerDriver,
    snapshots: &[(String, WorkspaceState)],
) -> Vec<(String, SchedulerProjection)> {
    snapshots
        .iter()
        .filter_map(|(id, state)| driver.evaluate(id, state).map(|p| (id.clone(), p)))
        .collect()
}

/// Spawn the headless driver loop. Stops when `shutdown` is set.
pub fn spawn_driver_loop(
    snapshot_fn: impl Fn() -> Vec<(String, WorkspaceState)> + Send + 'static,
    on_decision: impl Fn(String, SchedulerProjection) + Send + 'static,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        let driver = SchedulerDriver::default();
        while !shutdown.load(Ordering::SeqCst) {
            for (id, projection) in drive_once(&driver, &snapshot_fn()) {
                on_decision(id, projection);
            }
            std::thread::sleep(DRIVER_TICK);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dragent_workspace_model::{
        ActorId, AgentInstanceId, ChargingAuthorization, ChargingMode, ExecutionLease,
        ExecutionLeaseId, ExecutionLeaseStatus, WorkContractId, WorkItemId,
    };

    fn lease(status: ExecutionLeaseStatus, expires_at_unix_ms: i64) -> ExecutionLease {
        ExecutionLease {
            id: ExecutionLeaseId::new(),
            work_item_id: WorkItemId::new(),
            agent_instance_id: AgentInstanceId::new(),
            contract_id: WorkContractId::new(),
            status,
            charging: ChargingAuthorization {
                owner_id: ActorId::new(),
                mode: ChargingMode::LocalCompute,
                billing_operation_id: None,
            },
            acquired_at_unix_ms: 0,
            expires_at_unix_ms,
            updated_at_unix_ms: 0,
        }
    }

    #[test]
    fn active_lease_before_expiry_is_not_spent() {
        assert!(!lease_is_spent(
            &lease(ExecutionLeaseStatus::Active, 10_000),
            9_999
        ));
    }

    #[test]
    fn past_expiry_is_spent_even_when_active() {
        assert!(lease_is_spent(
            &lease(ExecutionLeaseStatus::Active, 10_000),
            10_000
        ));
        assert!(lease_is_spent(
            &lease(ExecutionLeaseStatus::Active, 10_000),
            10_001
        ));
    }

    #[test]
    fn terminal_statuses_are_spent_without_waiting() {
        for status in [
            ExecutionLeaseStatus::Released,
            ExecutionLeaseStatus::Expired,
            ExecutionLeaseStatus::Cancelled,
        ] {
            assert!(lease_is_spent(&lease(status, i64::MAX), 0));
        }
    }
}
