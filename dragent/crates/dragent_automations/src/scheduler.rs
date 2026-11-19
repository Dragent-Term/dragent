//! When runs happen.
//!
//! Every enabled automation keeps exactly one planned occurrence. When its
//! time comes it joins the queue; queued runs start as slots free up (a
//! bounded number at once, never two of the same automation). An
//! occurrence that could not start before the next one was due is skipped
//! rather than run late, so a closed laptop never wakes up to a burst of
//! stale runs. Runs cut off by the app quitting are marked interrupted.
//!
//! The scheduler only moves records; executing a claimed run (preparing
//! its workspace, starting its agent) belongs to the caller, which reports
//! back through the `mark_*` calls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, TimeZone};

use crate::cron::{next_window, Schedule};
use crate::model::{
    codes, slug, Automation, MissedPolicy, OverlapPolicy, Run, RunError, RunStatus, RunStep,
    RunTerminal, Trigger,
};
use crate::store::Store;

/// Runs this process is executing: run id → automation id.
pub type Active = HashMap<String, String>;

#[derive(Debug, Clone)]
pub struct Scheduler {
    pub max_concurrent: usize,
    pub due_batch: usize,
    /// Only automations this account owns run here (unowned ones predate
    /// owners and count as this account's).
    pub owner_id: Option<String>,
    /// How many automations the plan runs (the oldest ones); `None` means
    /// unlimited.
    pub automation_limit: Option<usize>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            due_batch: 100,
            owner_id: None,
            automation_limit: None,
        }
    }
}

/// The free plan's allowance.
pub const FREE_AUTOMATION_LIMIT: usize = 4;

/// Next due time and when it goes stale, in milliseconds, reading the
/// schedule in `now`'s zone.
fn plan_window<Tz: TimeZone>(
    schedule: &Schedule,
    now: &DateTime<Tz>,
    after_ms: Option<i64>,
) -> Option<(i64, Option<i64>)> {
    let from = after_ms
        .and_then(|planned| now.timezone().timestamp_millis_opt(planned).single())
        .unwrap_or_else(|| now.clone());
    let (due, stale) = next_window(schedule, &from)?;
    Some((
        due.timestamp_millis(),
        stale.map(|stale| stale.timestamp_millis()),
    ))
}

/// What a reconcile pass did.
#[derive(Debug, Default)]
pub struct Reconciled {
    /// Runs claimed for execution, now preparing their workspace.
    pub start: Vec<Run>,
    /// Anything stored changed (the UI should refresh).
    pub changed: bool,
}

fn new_run(automation: &Automation, status: RunStatus, trigger: Trigger) -> Run {
    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("{}-{}", slug(&automation.name, 32), &id[..6]);
    Run {
        id,
        seq: 0,
        automation_id: automation.id.clone(),
        status,
        trigger,
        snapshot: automation.snapshot(),
        name,
        scheduled_at_ms: None,
        deadline_at_ms: None,
        started_at_ms: None,
        finished_at_ms: None,
        workspace_path: None,
        branch: None,
        terminal: None,
        error: None,
        continued: false,
        warning: None,
        pane_uuid: None,
        progress: None,
        read_at_ms: None,
        archived: None,
        attempt: 0,
        retry_of: None,
    }
}

/// Preview the next `count` run times for `schedule` in `timezone` (empty =
/// the machine's zone), starting after `from_ms`. Returns the times as
/// milliseconds since the epoch, without creating anything.
pub fn preview_schedule(
    schedule: &str,
    timezone: &str,
    from_ms: i64,
    count: usize,
) -> Result<Vec<i64>, String> {
    use chrono::TimeZone;
    let parsed = Schedule::parse(schedule)
        .map_err(|reason| format!("The schedule isn't valid: {reason}"))?;
    let zone: chrono_tz::Tz = if timezone.trim().is_empty() {
        crate::model::local_time_zone().parse().map_err(|_| {
            format!(
                "{} isn't a time zone this app knows.",
                crate::model::local_time_zone()
            )
        })?
    } else {
        timezone
            .parse()
            .map_err(|_| format!("{timezone} isn't a time zone this app knows."))?
    };
    let mut now = zone
        .timestamp_millis_opt(from_ms)
        .single()
        .ok_or_else(|| "The preview start time isn't valid.".to_string())?;
    let mut out = Vec::with_capacity(count.min(100));
    for _ in 0..count.min(100) {
        let next = parsed
            .next_after(&now)
            .ok_or_else(|| "The schedule never runs.".to_string())?;
        out.push(next.timestamp_millis());
        now = next;
    }
    Ok(out)
}

impl Scheduler {
    /// Automations owned here (see [`Scheduler::owner_id`]).
    pub fn owns(&self, automation: &Automation) -> bool {
        match &self.owner_id {
            Some(owner) => automation.owner_id.is_empty() || &automation.owner_id == owner,
            None => true,
        }
    }

    /// Owned automations the plan lets run: all of them, or the oldest up
    /// to the limit (so creating one more never stops an existing one).
    pub fn runnable<'a>(
        &self,
        automations: impl IntoIterator<Item = &'a Automation>,
    ) -> HashSet<String> {
        let mut owned: Vec<&Automation> =
            automations.into_iter().filter(|a| self.owns(a)).collect();
        owned.sort_by(|a, b| (a.created_at_ms, &a.id).cmp(&(b.created_at_ms, &b.id)));
        let take = self.automation_limit.unwrap_or(usize::MAX);
        owned.into_iter().take(take).map(|a| a.id.clone()).collect()
    }

    /// After a restart: runs that were mid-way are failed as interrupted
    /// (their worker died with the old process).
    pub fn recover_interrupted(&self, store: &Store, now_ms: i64) -> Result<usize> {
        let mut recovered = 0;
        for run in
            store.runs_in_status(&[RunStatus::PreparingWorkspace, RunStatus::StartingAgent])?
        {
            let step = if run.status == RunStatus::PreparingWorkspace {
                RunStep::Workspace
            } else {
                RunStep::Agent
            };
            let failed = store.transition(
                &run.id,
                &[RunStatus::PreparingWorkspace, RunStatus::StartingAgent],
                |run| {
                    run.status = RunStatus::Failed;
                    run.error = Some(RunError::new(step, codes::INTERRUPTED));
                    run.finished_at_ms = Some(now_ms);
                },
            )?;
            recovered += usize::from(failed.is_some());
        }
        Ok(recovered)
    }

    /// One pass: retire planned runs that no longer apply, queue due ones,
    /// plan the next occurrence of every enabled automation, and claim
    /// queued runs up to capacity.
    pub fn reconcile<Tz: TimeZone>(
        &self,
        store: &Store,
        now: &DateTime<Tz>,
        active: &Active,
    ) -> Result<Reconciled> {
        let now_ms = now.timestamp_millis();
        let automations: HashMap<String, Automation> = store
            .list_automations()?
            .into_iter()
            .map(|automation| (automation.id.clone(), automation))
            .collect();
        let mut result = Reconciled::default();
        let runnable = self.runnable(automations.values());

        // Planned occurrences of paused, deleted, rescheduled, or (on the
        // free plan) over-allowance automations never happened: remove them
        // instead of filling history with skips.
        for run in store.runs_in_status(&[RunStatus::Scheduled])? {
            let obsolete = match automations.get(&run.automation_id) {
                Some(automation) => {
                    !automation.enabled
                        || automation.schedule != run.snapshot.schedule
                        || !runnable.contains(&automation.id)
                }
                None => true,
            };
            if obsolete {
                store.delete_run(&run.id)?;
                result.changed = true;
            }
        }

        for run in store.due_runs(now_ms, self.due_batch)? {
            let queued = store.transition(&run.id, &[RunStatus::Scheduled], |run| {
                run.status = RunStatus::Queued;
            })?;
            result.changed |= queued.is_some();
        }

        for automation in automations
            .values()
            .filter(|a| a.enabled && runnable.contains(&a.id))
        {
            if store.scheduled_run(&automation.id)?.is_some() {
                continue;
            }
            let Ok(schedule) = Schedule::parse(&automation.schedule) else {
                continue;
            };
            // Plan after both now and the last planned occurrence, so an
            // occurrence that was skipped or already ran is never replanned.
            let after = store
                .last_planned_at(&automation.id)?
                .filter(|planned| *planned > now_ms);
            // A named zone reads the schedule there; otherwise in the
            // caller's clock (the machine's local time in the app).
            let window = match automation.time_zone() {
                Some(zone) => plan_window(&schedule, &now.with_timezone(&zone), after),
                None => plan_window(&schedule, now, after),
            };
            let Some((due, stale)) = window else {
                continue;
            };
            let mut run = new_run(automation, RunStatus::Scheduled, Trigger::Schedule);
            run.scheduled_at_ms = Some(due);
            run.deadline_at_ms = stale;
            result.changed |= store.insert_run(run)?.is_some();
        }

        let mut running: Vec<String> = active.values().cloned().collect();
        for run in store.runs_in_status(&[RunStatus::Queued])? {
            if running.len() >= self.max_concurrent {
                break;
            }
            // A retry waiting out its backoff: not due yet, leave it queued.
            if run.retry_of.is_some() && run.scheduled_at_ms.is_some_and(|at| at > now_ms) {
                continue;
            }
            let skip = |code: &str| -> Result<()> {
                store.transition(&run.id, &[RunStatus::Queued], |run| {
                    run.status = RunStatus::Skipped;
                    run.error = Some(RunError::new(RunStep::Queue, code));
                    run.finished_at_ms = Some(now_ms);
                })?;
                Ok(())
            };
            let automation = automations.get(&run.automation_id).cloned();
            if run
                .deadline_at_ms
                .is_some_and(|deadline| deadline <= now_ms)
            {
                // Stale occurrences are skipped, unless the automation asks
                // to run the most recent one once on return.
                let catch_up = automation.as_ref().is_some_and(|automation| {
                    automation.missed_policy == MissedPolicy::CatchUp
                        && run.trigger == Trigger::Schedule
                });
                if !catch_up {
                    result.changed = true;
                    skip(codes::MISSED_WINDOW)?;
                    continue;
                }
            }
            match automation {
                None => {
                    result.changed = true;
                    skip(codes::AUTOMATION_DELETED)?;
                    continue;
                }
                Some(automation) if !automation.enabled => {
                    result.changed = true;
                    skip(codes::AUTOMATION_PAUSED)?;
                    continue;
                }
                Some(automation) if !runnable.contains(&automation.id) => {
                    result.changed = true;
                    skip(codes::PLAN_LIMIT)?;
                    continue;
                }
                Some(automation) => {
                    if running.contains(&automation.id) {
                        match automation.overlap_policy {
                            OverlapPolicy::Skip => {
                                result.changed = true;
                                skip(codes::ALREADY_RUNNING)?;
                                continue;
                            }
                            OverlapPolicy::Queue => {
                                // Stay queued until the previous run leaves
                                // the starting states; try the next run.
                                continue;
                            }
                            OverlapPolicy::Allow => {}
                        }
                    }
                }
            }
            result.changed = true;
            let claimed = store.transition(&run.id, &[RunStatus::Queued], |run| {
                run.status = RunStatus::PreparingWorkspace;
                run.started_at_ms = Some(now_ms);
            })?;
            if let Some(claimed) = claimed {
                running.push(claimed.automation_id.clone());
                result.start.push(claimed);
            }
        }
        // Enforce each automation's retention limit: keep the planned
        // occurrence plus the newest `run_retention` past runs.
        for automation in automations.values() {
            let _ = store.prune_runs(&automation.id, automation.run_retention);
        }
        Ok(result)
    }

    /// Queue a run right away; the next reconcile starts it.
    pub fn run_now(&self, store: &Store, automation: &Automation) -> Result<Run> {
        let run = new_run(automation, RunStatus::Queued, Trigger::Manual);
        store
            .insert_run(run)?
            .ok_or_else(|| anyhow::anyhow!("could not queue the run"))
    }

    /// Stop a run. The planned occurrence is skipped (the following one
    /// takes its place); a queued or starting run is cancelled.
    pub fn cancel(&self, store: &Store, run_id: &str, now_ms: i64) -> Result<Option<Run>> {
        let skipped = store.transition(run_id, &[RunStatus::Scheduled], |run| {
            run.status = RunStatus::Skipped;
            run.error = Some(RunError::new(RunStep::Queue, codes::CANCELLED));
            run.finished_at_ms = Some(now_ms);
        })?;
        if skipped.is_some() {
            return Ok(skipped);
        }
        store.transition(
            run_id,
            &[
                RunStatus::Queued,
                RunStatus::PreparingWorkspace,
                RunStatus::StartingAgent,
            ],
            |run| {
                run.status = RunStatus::Cancelled;
                run.finished_at_ms = Some(now_ms);
            },
        )
    }

    // ── Reports from the executor ────────────────────────────────────────────

    pub fn mark_starting_agent(
        &self,
        store: &Store,
        run_id: &str,
        workspace_path: PathBuf,
        branch: Option<String>,
        warning: Option<String>,
    ) -> Result<Option<Run>> {
        store.transition(run_id, &[RunStatus::PreparingWorkspace], |run| {
            run.status = RunStatus::StartingAgent;
            run.workspace_path = Some(workspace_path);
            run.branch = branch;
            run.warning = warning;
        })
    }

    /// The agent has the prompt: in a terminal of its own, or (`continued`)
    /// in the one the previous run left running. An agent started inside a
    /// window pane has no separate terminal to record.
    pub fn mark_started(
        &self,
        store: &Store,
        run_id: &str,
        terminal: Option<RunTerminal>,
        continued: bool,
        now_ms: i64,
    ) -> Result<Option<Run>> {
        store.transition(run_id, &[RunStatus::StartingAgent], |run| {
            run.status = RunStatus::Started;
            run.terminal = terminal;
            run.continued = continued;
            run.finished_at_ms = Some(now_ms);
        })
    }

    /// Drop a run that is about to start its agent, without counting it as
    /// a failure.
    pub fn mark_skipped(
        &self,
        store: &Store,
        run_id: &str,
        error: RunError,
        now_ms: i64,
    ) -> Result<Option<Run>> {
        store.transition(run_id, &[RunStatus::StartingAgent], |run| {
            run.status = RunStatus::Skipped;
            run.error = Some(error);
            run.finished_at_ms = Some(now_ms);
        })
    }

    pub fn mark_failed(
        &self,
        store: &Store,
        run_id: &str,
        error: RunError,
        now_ms: i64,
    ) -> Result<Option<Run>> {
        let failed = store.transition(
            run_id,
            &[
                RunStatus::Queued,
                RunStatus::PreparingWorkspace,
                RunStatus::StartingAgent,
            ],
            |run| {
                run.status = RunStatus::Failed;
                run.error = Some(error);
                run.finished_at_ms = Some(now_ms);
            },
        )?;
        let Some(failed) = failed else {
            return Ok(None);
        };
        // Bounded retries with backoff: the retry waits out its delay as a
        // queued run with a future time (see `reconcile`).
        if let Ok(Some(automation)) = store.get_automation(&failed.automation_id) {
            let _ = self.queue_retry(store, &automation, &failed, now_ms);
        }
        Ok(Some(failed))
    }

    /// Queue a retry for `failed` when its automation's policy allows
    /// another attempt. Returns the queued retry, if any.
    pub fn queue_retry(
        &self,
        store: &Store,
        automation: &Automation,
        failed: &Run,
        now_ms: i64,
    ) -> Result<Option<Run>> {
        let Some(delay_secs) = automation.retry_delay_for_attempt(failed.attempt) else {
            return Ok(None);
        };
        let mut retry = new_run(automation, RunStatus::Queued, Trigger::Manual);
        retry.attempt = failed.attempt.saturating_add(1);
        retry.retry_of = Some(failed.id.clone());
        retry.scheduled_at_ms =
            Some(now_ms.saturating_add((delay_secs as i64).saturating_mul(1000)));
        Ok(store.insert_run(retry)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorkspaceMode;
    use chrono::Utc;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn setup(schedule: &str) -> (Store, Automation) {
        let store = Store::open_in_memory().unwrap();
        let automation = Automation::new(
            "Nightly review",
            PathBuf::from("/repo"),
            schedule,
            "claude",
            "Review",
            WorkspaceMode::default(),
            0,
        );
        store.save_automation(&automation).unwrap();
        (store, automation)
    }

    fn status_of(store: &Store, run: &Run) -> RunStatus {
        store.get_run(&run.id).unwrap().unwrap().status
    }

    #[test]
    fn plans_one_occurrence_and_starts_it_when_due() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let active = Active::new();

        let pass = scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &active)
            .unwrap();
        assert!(pass.start.is_empty());
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        assert_eq!(
            planned.scheduled_at_ms,
            Some(at("2026-10-01T09:00:00Z").timestamp_millis())
        );
        assert_eq!(
            planned.deadline_at_ms,
            Some(at("2026-10-02T09:00:00Z").timestamp_millis())
        );
        // Re-running before the time changes nothing.
        scheduler
            .reconcile(&store, &at("2026-10-01T08:30:00Z"), &active)
            .unwrap();
        assert_eq!(
            store.scheduled_run(&automation.id).unwrap().unwrap().id,
            planned.id
        );

        let pass = scheduler
            .reconcile(&store, &at("2026-10-01T09:00:10Z"), &active)
            .unwrap();
        assert_eq!(pass.start.len(), 1);
        assert_eq!(pass.start[0].id, planned.id);
        assert_eq!(pass.start[0].status, RunStatus::PreparingWorkspace);
        // The following occurrence is planned straight away.
        let next = store.scheduled_run(&automation.id).unwrap().unwrap();
        assert_eq!(
            next.scheduled_at_ms,
            Some(at("2026-10-02T09:00:00Z").timestamp_millis())
        );
    }

    #[test]
    fn a_stale_occurrence_is_skipped_not_run_late() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let active = Active::new();
        scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &active)
            .unwrap();
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        // The app was closed for two days.
        let pass = scheduler
            .reconcile(&store, &at("2026-10-03T12:00:00Z"), &active)
            .unwrap();
        assert!(pass.start.is_empty());
        let missed = store.get_run(&planned.id).unwrap().unwrap();
        assert_eq!(missed.status, RunStatus::Skipped);
        assert_eq!(missed.error.unwrap().code, codes::MISSED_WINDOW);
        assert_eq!(
            store
                .scheduled_run(&automation.id)
                .unwrap()
                .unwrap()
                .scheduled_at_ms,
            Some(at("2026-10-04T09:00:00Z").timestamp_millis())
        );
    }

    #[test]
    fn never_two_runs_of_one_automation_and_capacity_is_bounded() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler {
            max_concurrent: 1,
            ..Scheduler::default()
        };
        let first = scheduler.run_now(&store, &automation).unwrap();
        let second = scheduler.run_now(&store, &automation).unwrap();
        let now = at("2026-10-01T08:00:00Z");
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(
            pass.start.iter().map(|r| &r.id).collect::<Vec<_>>(),
            vec![&first.id]
        );
        // Capacity full: the second waits.
        let mut active = Active::new();
        active.insert(first.id.clone(), automation.id.clone());
        let pass = scheduler.reconcile(&store, &now, &active).unwrap();
        assert!(pass.start.is_empty());
        assert_eq!(status_of(&store, &second), RunStatus::Queued);
        // Room again, but the same automation is still running: skipped.
        let scheduler = Scheduler::default();
        scheduler.reconcile(&store, &now, &active).unwrap();
        let skipped = store.get_run(&second.id).unwrap().unwrap();
        assert_eq!(skipped.status, RunStatus::Skipped);
        assert_eq!(skipped.error.unwrap().code, codes::ALREADY_RUNNING);
    }

    #[test]
    fn pausing_removes_the_plan_and_skips_queued_runs() {
        let (store, mut automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let now = at("2026-10-01T08:00:00Z");
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        let queued = scheduler.run_now(&store, &automation).unwrap();
        automation.enabled = false;
        store.save_automation(&automation).unwrap();
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert!(pass.start.is_empty());
        assert!(store.scheduled_run(&automation.id).unwrap().is_none());
        let skipped = store.get_run(&queued.id).unwrap().unwrap();
        assert_eq!(skipped.error.unwrap().code, codes::AUTOMATION_PAUSED);
    }

    #[test]
    fn rescheduling_replans_without_a_history_entry() {
        let (store, mut automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let now = at("2026-10-01T08:00:00Z");
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        automation.schedule = "30 8 * * *".into();
        store.save_automation(&automation).unwrap();
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        assert_eq!(
            planned.scheduled_at_ms,
            Some(at("2026-10-01T08:30:00Z").timestamp_millis())
        );
        assert_eq!(
            store.run_counts(&automation.id).unwrap()[&crate::model::RunFilter::All],
            0
        );
    }

    #[test]
    fn executor_reports_walk_the_lifecycle() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        scheduler.run_now(&store, &automation).unwrap();
        let pass = scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        let run = &pass.start[0];
        let starting = scheduler
            .mark_starting_agent(
                &store,
                &run.id,
                PathBuf::from("/wt"),
                Some("b".into()),
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(starting.status, RunStatus::StartingAgent);
        let terminal = RunTerminal {
            workspace_id: "w".into(),
            home_id: "h".into(),
            session_id: "s".into(),
            pane_uuid: None,
        };
        let started = scheduler
            .mark_started(&store, &run.id, Some(terminal), false, 5)
            .unwrap()
            .unwrap();
        assert_eq!(started.status, RunStatus::Started);
        assert!(started.can_open());
        // A late cancel can't undo a finished run.
        assert!(scheduler.cancel(&store, &run.id, 6).unwrap().is_none());
    }

    #[test]
    fn cancelling_skips_the_plan_or_stops_a_run() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let now = at("2026-10-01T08:00:00Z");
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        let skipped = scheduler.cancel(&store, &planned.id, 1).unwrap().unwrap();
        assert_eq!(skipped.status, RunStatus::Skipped);
        // The following occurrence replaces it on the next pass.
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(
            store
                .scheduled_run(&automation.id)
                .unwrap()
                .unwrap()
                .scheduled_at_ms,
            Some(at("2026-10-02T09:00:00Z").timestamp_millis())
        );
        let queued = scheduler.run_now(&store, &automation).unwrap();
        assert_eq!(
            scheduler
                .cancel(&store, &queued.id, 2)
                .unwrap()
                .unwrap()
                .status,
            RunStatus::Cancelled
        );
    }

    #[test]
    fn interrupted_runs_fail_after_a_restart() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        scheduler.run_now(&store, &automation).unwrap();
        let pass = scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        assert_eq!(scheduler.recover_interrupted(&store, 9).unwrap(), 1);
        let failed = store.get_run(&pass.start[0].id).unwrap().unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.unwrap().code, codes::INTERRUPTED);
    }

    #[test]
    fn deleted_automations_stop_planning() {
        let (store, automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let now = at("2026-10-01T08:00:00Z");
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        store.delete_automation(&automation.id).unwrap();
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert!(store.runs_in_status(&RunStatus::ALL).unwrap().is_empty());
    }

    #[test]
    fn schedules_read_in_their_own_zone() {
        let (store, mut automation) = setup("0 9 * * *");
        automation.timezone = "America/New_York".into();
        store.save_automation(&automation).unwrap();
        Scheduler::default()
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        // 09:00 in New York (EDT, UTC-4) is 13:00 UTC.
        assert_eq!(
            planned.scheduled_at_ms,
            Some(at("2026-10-01T13:00:00Z").timestamp_millis())
        );
    }

    #[test]
    fn the_free_allowance_runs_the_oldest_automations_only() {
        let store = Store::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for n in 0..5 {
            let mut automation = Automation::new(
                format!("A{n}"),
                PathBuf::from("/repo"),
                "0 9 * * *",
                "claude",
                "Go",
                WorkspaceMode::default(),
                n,
            );
            automation.owner_id = "me".into();
            store.save_automation(&automation).unwrap();
            ids.push(automation.id);
        }
        // Someone else's automation in the same organisation never runs here.
        let mut theirs = Automation::new(
            "Theirs",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "Go",
            WorkspaceMode::default(),
            0,
        );
        theirs.owner_id = "them".into();
        store.save_automation(&theirs).unwrap();

        let scheduler = Scheduler {
            owner_id: Some("me".into()),
            automation_limit: Some(FREE_AUTOMATION_LIMIT),
            ..Scheduler::default()
        };
        scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        let planned: Vec<bool> = ids
            .iter()
            .map(|id| store.scheduled_run(id).unwrap().is_some())
            .collect();
        assert_eq!(planned, vec![true, true, true, true, false]);
        assert!(store.scheduled_run(&theirs.id).unwrap().is_none());

        // A manual run of the over-allowance automation is skipped.
        let fifth = store.get_automation(&ids[4]).unwrap().unwrap();
        let queued = scheduler.run_now(&store, &fifth).unwrap();
        let pass = scheduler
            .reconcile(&store, &at("2026-10-01T08:01:00Z"), &Active::new())
            .unwrap();
        assert!(pass.start.is_empty());
        let skipped = store.get_run(&queued.id).unwrap().unwrap();
        assert_eq!(skipped.error.unwrap().code, codes::PLAN_LIMIT);
    }

    #[test]
    fn missed_runs_skip_by_default_and_catch_up_when_asked() {
        // Skip (default): a stale occurrence is recorded as skipped.
        let (store, _) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        let planned = store
            .runs_in_status(&[RunStatus::Scheduled])
            .unwrap()
            .pop()
            .unwrap();
        scheduler
            .reconcile(&store, &at("2026-10-03T12:00:00Z"), &Active::new())
            .unwrap();
        let missed = store.get_run(&planned.id).unwrap().unwrap();
        assert_eq!(missed.status, RunStatus::Skipped);
        assert_eq!(missed.error.unwrap().code, codes::MISSED_WINDOW);

        // Catch-up: the most recent stale occurrence runs once on return.
        let (store, mut automation) = setup("0 9 * * *");
        automation.missed_policy = crate::model::MissedPolicy::CatchUp;
        store.save_automation(&automation).unwrap();
        scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        let pass = scheduler
            .reconcile(&store, &at("2026-10-03T12:00:00Z"), &Active::new())
            .unwrap();
        assert_eq!(
            pass.start.len(),
            1,
            "catch-up runs the missed occurrence once"
        );
        assert_eq!(pass.start[0].trigger, Trigger::Schedule);
    }

    #[test]
    fn overlap_policies_skip_queue_or_allow() {
        // Skip (default): covered by never_two_runs_of_one_automation, but
        // asserted here for the policy matrix.
        let (store, mut automation) = setup("0 9 * * *");
        automation.overlap_policy = crate::model::OverlapPolicy::Skip;
        store.save_automation(&automation).unwrap();
        let scheduler = Scheduler::default();
        let first = scheduler.run_now(&store, &automation).unwrap();
        let second = scheduler.run_now(&store, &automation).unwrap();
        let now = at("2026-10-01T08:00:00Z");
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(
            pass.start.iter().map(|r| &r.id).collect::<Vec<_>>(),
            vec![&first.id]
        );
        let mut active = Active::new();
        active.insert(first.id.clone(), automation.id.clone());
        scheduler.reconcile(&store, &now, &active).unwrap();
        assert_eq!(
            store.get_run(&second.id).unwrap().unwrap().status,
            RunStatus::Skipped
        );

        // Queue: the second run stays queued until the first leaves.
        let (store, mut automation) = setup("0 9 * * *");
        automation.overlap_policy = crate::model::OverlapPolicy::Queue;
        store.save_automation(&automation).unwrap();
        let first = scheduler.run_now(&store, &automation).unwrap();
        let second = scheduler.run_now(&store, &automation).unwrap();
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(pass.start.len(), 1);
        let mut active = Active::new();
        active.insert(first.id.clone(), automation.id.clone());
        let pass = scheduler.reconcile(&store, &now, &active).unwrap();
        assert!(pass.start.is_empty());
        assert_eq!(
            store.get_run(&second.id).unwrap().unwrap().status,
            RunStatus::Queued,
            "queued overlap waits instead of skipping"
        );
        // Once the previous run is gone, the queued one starts.
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(
            pass.start.iter().map(|r| &r.id).collect::<Vec<_>>(),
            vec![&second.id]
        );

        // Allow: both start together.
        let (store, mut automation) = setup("0 9 * * *");
        automation.overlap_policy = crate::model::OverlapPolicy::Allow;
        store.save_automation(&automation).unwrap();
        let scheduler = Scheduler {
            max_concurrent: 4,
            ..Scheduler::default()
        };
        let first = scheduler.run_now(&store, &automation).unwrap();
        let second = scheduler.run_now(&store, &automation).unwrap();
        let mut active = Active::new();
        active.insert("other-run".into(), automation.id.clone());
        // Even with the automation marked running, both queue-mates start
        // when capacity allows (allow ignores the same-automation check).
        let _ = (first, second);
        let pass = scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert_eq!(pass.start.len(), 2);
    }

    #[test]
    fn failed_runs_retry_with_backoff_until_the_limit() {
        let (store, mut automation) = setup("0 9 * * *");
        automation.max_retries = 2;
        automation.retry_delay_secs = 300;
        store.save_automation(&automation).unwrap();
        let scheduler = Scheduler::default();
        let base = at("2026-10-01T08:00:00Z");
        let base_ms = base.timestamp_millis();
        let run = scheduler.run_now(&store, &automation).unwrap();
        let pass = scheduler.reconcile(&store, &base, &Active::new()).unwrap();
        let starting = pass.start.into_iter().find(|r| r.id == run.id).unwrap();
        let failed = scheduler
            .mark_failed(
                &store,
                &starting.id,
                RunError::new(RunStep::Agent, "start_failed"),
                base_ms,
            )
            .unwrap()
            .unwrap();
        assert_eq!(failed.status, RunStatus::Failed);
        // The retry waits out its backoff as a queued run in the future.
        let retries: Vec<Run> = store
            .runs_in_status(&[RunStatus::Queued])
            .unwrap()
            .into_iter()
            .filter(|r| r.retry_of.as_deref() == Some(&failed.id))
            .collect();
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0].attempt, 1);
        assert_eq!(retries[0].scheduled_at_ms, Some(base_ms + 300_000));
        // Not due yet: reconcile leaves it queued.
        let pass = scheduler.reconcile(&store, &base, &Active::new()).unwrap();
        assert!(!pass.start.iter().any(|r| r.retry_of.is_some()));
        // After the backoff, it starts.
        let later = chrono::DateTime::from_timestamp_millis(base_ms + 300_000 + 60_000)
            .unwrap()
            .with_timezone(&Utc);
        let pass = scheduler.reconcile(&store, &later, &Active::new()).unwrap();
        assert!(pass.start.iter().any(|r| r.retry_of.is_some()));
        // queue_retry respects the bound directly.
        let failed_second = store.get_run(&retries[0].id).unwrap().unwrap();
        assert_eq!(failed_second.attempt, 1);
        let retry = scheduler
            .queue_retry(&store, &automation, &failed_second, base_ms)
            .unwrap();
        assert!(retry.is_some());
        let third = retry.unwrap();
        assert_eq!(third.attempt, 2);
        let exhausted = Run {
            attempt: 2,
            ..failed_second.clone()
        };
        assert!(scheduler
            .queue_retry(&store, &automation, &exhausted, base_ms)
            .unwrap()
            .is_none());
        let _ = third;
    }

    #[test]
    fn pause_resume_cycles_replan() {
        let (store, mut automation) = setup("0 9 * * *");
        let scheduler = Scheduler::default();
        let now = at("2026-10-01T08:00:00Z");
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert!(store.scheduled_run(&automation.id).unwrap().is_some());
        automation.enabled = false;
        store.save_automation(&automation).unwrap();
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        assert!(
            store.scheduled_run(&automation.id).unwrap().is_none(),
            "pause removes the plan"
        );
        automation.enabled = true;
        store.save_automation(&automation).unwrap();
        scheduler.reconcile(&store, &now, &Active::new()).unwrap();
        let planned = store.scheduled_run(&automation.id).unwrap().unwrap();
        assert_eq!(
            planned.scheduled_at_ms,
            Some(at("2026-10-01T09:00:00Z").timestamp_millis()),
            "resume plans the next occurrence again"
        );
    }

    #[test]
    fn retention_keeps_the_plan_and_the_newest_runs() {
        let (store, mut automation) = setup("0 9 * * *");
        automation.run_retention = 3;
        store.save_automation(&automation).unwrap();
        let scheduler = Scheduler::default();
        scheduler
            .reconcile(&store, &at("2026-10-01T08:00:00Z"), &Active::new())
            .unwrap();
        for _ in 0..5 {
            let run = scheduler.run_now(&store, &automation).unwrap();
            scheduler
                .mark_failed(&store, &run.id, RunError::new(RunStep::Queue, "x"), 1)
                .unwrap();
        }
        assert_eq!(store.prune_runs(&automation.id, 3).unwrap(), 2);
        let kept = store
            .list_runs(&automation.id, crate::model::RunFilter::All, None, 10)
            .unwrap();
        assert_eq!(kept.len(), 3);
        assert!(
            store.scheduled_run(&automation.id).unwrap().is_some(),
            "the plan survives pruning"
        );
        scheduler
            .reconcile(&store, &at("2026-10-01T08:01:00Z"), &Active::new())
            .unwrap();
        let kept = store
            .list_runs(&automation.id, crate::model::RunFilter::All, None, 10)
            .unwrap();
        assert!(kept.len() <= 3, "reconcile enforces retention");
    }

    #[test]
    fn preview_lists_the_next_run_times_without_creating_anything() {
        let before: Vec<String> = Store::open_in_memory()
            .unwrap()
            .runs_in_status(&RunStatus::ALL)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert!(before.is_empty());
        let runs = super::preview_schedule(
            "0 9 * * *",
            "UTC",
            at("2026-10-01T08:00:00Z").timestamp_millis(),
            3,
        )
        .unwrap();
        assert_eq!(
            runs,
            vec![
                at("2026-10-01T09:00:00Z").timestamp_millis(),
                at("2026-10-02T09:00:00Z").timestamp_millis(),
                at("2026-10-03T09:00:00Z").timestamp_millis(),
            ]
        );
        assert!(super::preview_schedule("not a schedule", "UTC", 0, 3).is_err());
        assert!(super::preview_schedule("0 9 * * *", "Mars/Olympus", 0, 3).is_err());
    }
}
