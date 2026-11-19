//! Automations runtime: runs the selected organisation's automations while
//! the app is open.
//!
//! Each organisation keeps its automations in `automations.sqlite` inside
//! its own data folder. A tick (and every change made in the Automations
//! page or by the `dragent automations` command line) reconciles the
//! scheduler; claimed runs execute on worker threads and report back here
//! so the queue drains as slots free up. Occurrences that fall due while
//! the app is closed start on the next launch if they are still within
//! their window, and are skipped otherwise.
//!
//! A started run's workspace goes into a window's sidebar (filed into its
//! automation's tag folders). Every owned automation is mirrored to the
//! organisation's registry on the server, which lists teammates'
//! automations and counts the free plan's allowance.

pub mod cli;
pub(crate) mod executor;
mod page;
mod panel;
pub(crate) mod progress;
pub(crate) mod team;
mod templates;
pub(crate) mod view;
mod watch;
pub(crate) mod widgets;

pub use view::{AutomationsEvent, AutomationsView};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dragent_automations::{
    codes, Active, AgentState, Automation, PromptSource, PromptVersion, Run, RunError, RunFilter,
    RunProgress, RunStatus, RunStep, RunWorkspace, Scheduler, Store, FREE_AUTOMATION_LIMIT,
};
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity};

const TICK: Duration = Duration::from_secs(30);
/// How often changes made outside this process (the command line) are
/// noticed.
const WATCH: Duration = Duration::from_secs(3);
/// How long a run that needs a window waits for one.
const WINDOW_WAIT: Duration = Duration::from_secs(60);
/// The Team tab refreshes at most this often on its own.
const TEAM_REFRESH: Duration = Duration::from_secs(60);

/// Shown when the free plan's allowance is used up.
pub const LIMIT_MESSAGE: &str =
    "The free plan includes 4 automations. Upgrade to Pro for unlimited automations.";

/// Runs happen on this computer; a remote project here is only a local
/// stand-in for a folder on another machine.
pub const REMOTE_PROJECT_MESSAGE: &str =
    "This project is on a remote machine. Automations only run on this computer for now.";

/// Meta key: when the user last looked at failures (the sidebar badge).
const FAILURES_SEEN: &str = "failures_seen_at";
/// Meta key the command line reads for the plan's allowance.
pub(crate) const PLAN_LIMIT_KEY: &str = "plan.automation_limit";
/// Meta key: the owning account, for the command line.
pub(crate) const OWNER_KEY: &str = "plan.owner_id";

#[derive(Clone, Debug, PartialEq)]
pub enum AutomationEvent {
    /// Automations or runs changed; views re-read what they show.
    Changed,
    /// A run started its agent; a window puts its workspace in the sidebar.
    RunStarted(Run),
    /// A run is waiting for a window to start Dragent's agent.
    StartInWindow(Run),
    /// There are alerts to show; the first window to ask takes them.
    Alert,
}

/// A desktop alert about a run: it finished, failed, or needs someone.
#[derive(Clone, Debug, PartialEq)]
pub struct RunAlert {
    pub run_id: String,
    /// The pane the run's agent works in, so a window showing it can skip
    /// the alert.
    pub pane: Option<String>,
    pub title: String,
    pub body: String,
    /// Waiting for someone: worth a sound.
    pub urgent: bool,
}

impl RunAlert {
    /// What a change in a run's progress is worth telling someone, if
    /// anything. A run's tab alerts only this way (the sidebar's own agent
    /// alerts skip it), follow-up turns included.
    pub(crate) fn for_progress(run: &Run, before: Option<&RunProgress>, after: &RunProgress) -> Option<Self> {
        if before.is_some_and(|before| before.state == after.state) {
            return None;
        }
        let (body, urgent) = match after.state {
            AgentState::NeedsInput => (
                "Waiting for you: it asked for an approval or an answer.".to_string(),
                true,
            ),
            // Nothing to report: archived on its own, not worth an alert.
            AgentState::Done
                if after
                    .reply
                    .as_deref()
                    .is_some_and(dragent_automations::says_nothing_to_report) =>
            {
                return None
            }
            AgentState::Done => (
                match after.reply.as_deref().map(progress::reply_preview) {
                    Some(reply) if !reply.is_empty() => reply.replace('\n', " "),
                    _ => "Finished.".to_string(),
                },
                false,
            ),
            AgentState::Failed => ("The agent reported an error.".to_string(), true),
            AgentState::TimedOut => (
                format!(
                    "Stopped: it waited {} minutes for you.",
                    progress::WAIT_LIMIT_MS / 60_000
                ),
                true,
            ),
            AgentState::Working | AgentState::Ended => return None,
        };
        Some(Self::new(run, body, urgent))
    }

    /// A run that couldn't start its agent.
    pub(crate) fn for_failure(run: &Run) -> Option<Self> {
        // A watching automation that used up its day's runs says so once.
        if run.status == RunStatus::Skipped
            && run
                .error
                .as_ref()
                .is_some_and(|error| error.code == dragent_automations::codes::WATCH_LIMIT)
        {
            let body = run.error.as_ref().map(executor::describe_error).unwrap_or_default();
            return Some(Self::new(run, body, false));
        }
        if run.status != RunStatus::Failed {
            return None;
        }
        let body = run
            .error
            .as_ref()
            .map(executor::describe_error)
            .unwrap_or_else(|| "Couldn't start.".to_string());
        Some(Self::new(run, body, true))
    }

    fn new(run: &Run, body: String, urgent: bool) -> Self {
        Self {
            run_id: run.id.clone(),
            pane: run.agent_pane().map(str::to_string),
            title: run.snapshot.name.clone(),
            body,
            urgent,
        }
    }

    /// Data a clicked alert carries back (see `run_id_from_alert`).
    pub fn data(&self) -> String {
        format!("{ALERT_DATA_PREFIX}{}", self.run_id)
    }
}

const ALERT_DATA_PREFIX: &str = "dragent-automation-run:";

/// The run a clicked alert is about.
pub fn run_id_from_alert(data: &str) -> Option<&str> {
    data.strip_prefix(ALERT_DATA_PREFIX).filter(|id| !id.is_empty())
}

/// Who is signed in and what their plan allows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub user_id: Option<String>,
    /// A paid plan: no automation limit.
    pub pro: bool,
}

/// The Automations page's summary cards.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub active: usize,
    /// Runs that started their agent in the last 7 days.
    pub created_7d: usize,
    pub failed_7d: usize,
}

/// The Team tab's data from the server.
#[derive(Clone, Debug, Default)]
pub struct TeamState {
    pub automations: Vec<team::TeamAutomation>,
    pub allowance: Option<team::Allowance>,
    pub loading: bool,
    pub error: Option<String>,
    fetched_at: Option<Instant>,
    org: String,
}

/// One organisation's store plus the runs this process is executing for it.
struct OrgRuntime {
    store: Arc<Store>,
    active: Arc<Mutex<Active>>,
    cancel_flags: HashMap<String, Arc<AtomicBool>>,
    /// Workspaces runs put in the sidebar, read on every header render.
    run_workspaces: Vec<RunWorkspace>,
    /// Runs waiting for a window to start Dragent's agent.
    window_runs: HashMap<String, (Run, Instant)>,
    /// Started runs a window has already placed.
    placed: HashSet<String>,
    data_version: i64,
}

impl OrgRuntime {
    fn reload_run_workspaces(&mut self) {
        match self.store.run_workspaces() {
            Ok(workspaces) => self.run_workspaces = workspaces,
            Err(error) => log::warn!("[automations] reading run workspaces: {error:#}"),
        }
    }
}

/// A worker's report: (organisation, run id, a run that needs a window).
type Finished = (String, String, Option<Run>);

pub struct AutomationRuntime {
    scheduler: Scheduler,
    account: Account,
    /// By organisation context id; opened on first use, kept so a worker
    /// still running for a previous selection reports into its own store.
    orgs: HashMap<String, OrgRuntime>,
    finished_tx: async_channel::Sender<Finished>,
    team: TeamState,
    syncing: bool,
    sync_again: bool,
    /// Reading run agents' progress on a worker.
    following: bool,
    /// Alerts no window has taken yet.
    alerts: Vec<RunAlert>,
    /// Failures the user hasn't seen, recounted on every change (the
    /// sidebar reads it on every sync).
    unseen_failures: usize,
    /// Inbox results nobody opened yet, recounted on every change.
    unread_results: usize,
    error: Option<String>,
    revision: u64,
}

impl Entity for AutomationRuntime {
    type Event = AutomationEvent;
}

impl SingletonEntity for AutomationRuntime {}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn local_format(ms: i64, format: &str) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|time| time.format(format).to_string())
        .unwrap_or_default()
}

/// `Oct 2` in local time.
pub(crate) fn format_day(ms: i64) -> String {
    local_format(ms, "%b %-d")
}

/// `Oct 2, 2026, 9:00 AM` in local time.
pub(crate) fn format_date_time(ms: i64) -> String {
    local_format(ms, "%b %-d, %Y, %-I:%M %p")
}

/// `just now`, `5m ago`, `3h ago`, `2d ago`, then the date.
pub(crate) fn compact_ago(ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - ms).max(0) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes}m ago"),
        60..=1439 => format!("{}h ago", minutes / 60),
        1440..=10079 => format!("{}d ago", minutes / 1440),
        _ => format_day(ms),
    }
}

/// `soon`, `in 5m`, `in 3h`, `in 2d`, then the date.
pub(crate) fn compact_until(ms: i64, now_ms: i64) -> String {
    let minutes = (ms - now_ms).max(0) / 60_000;
    match minutes {
        0 => "soon".into(),
        1..=59 => format!("in {minutes}m"),
        60..=1439 => format!("in {}h", minutes / 60),
        1440..=10079 => format!("in {}d", minutes / 1440),
        _ => format_day(ms),
    }
}

impl AutomationRuntime {
    /// A non-running scheduler and an in-memory sample registry for video captures.
    pub(crate) fn preview(_ctx: &mut ModelContext<Self>) -> Self {
        let (finished_tx, _) = async_channel::unbounded::<Finished>();
        let store = Arc::new(Store::open_in_memory().expect("demo automation store"));
        for (name, schedule, agent, prompt) in [
            ("Daily change review", "0 9 * * 1-5", "claude", "Review changes, run tests, and summarize findings. Do not push."),
            ("Weekly dependency check", "0 10 * * 1", "codex", "Check dependencies and propose a small, reviewed update."),
            ("Triage new issues", "*/30 * * * *", "opencode", "Summarize new issues and suggest the next action."),
        ] {
            let automation = dragent_automations::Automation::new(name, PathBuf::from("/demo/acme-app"),
                schedule, agent, prompt, dragent_automations::WorkspaceMode::default(), now_ms());
            store.save_automation(&automation).expect("sample automation");
        }
        let org = OrgRuntime { store, active: Default::default(), cancel_flags: Default::default(),
            run_workspaces: Vec::new(), window_runs: Default::default(), placed: Default::default(), data_version: 0 };
        Self { scheduler: Scheduler::default(), account: Account { user_id: Some("demo".into()), pro: true },
            orgs: [(dragent_paths::organisation_id(), org)].into(), finished_tx, team: TeamState::default(),
            syncing: false, sync_again: false, following: false, alerts: Vec::new(), unseen_failures: 0,
            unread_results: 0, error: None, revision: 0 }
    }

    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let (finished_tx, finished_rx) = async_channel::unbounded::<Finished>();
        ctx.spawn_stream_local(
            finished_rx,
            |this, (org, run_id, window), ctx| this.on_run_finished(&org, &run_id, window, ctx),
            |_, _| {},
        );
        let context = crate::organisation_context::OrganisationContext::handle(ctx);
        ctx.subscribe_to_model(&context, |this, event, ctx| {
            if matches!(
                event,
                crate::organisation_context::OrganisationContextEvent::Switched
            ) {
                this.team = TeamState::default();
                this.reconcile(ctx);
                this.changed(ctx);
                this.schedule_sync(ctx);
            }
        });
        ctx.spawn(Timer::after(Duration::from_secs(2)), |this, _, ctx| this.tick(ctx));
        ctx.spawn(Timer::after(WATCH), |this, _, ctx| this.watch(ctx));
        Self {
            scheduler: Scheduler::default(),
            account: Account::default(),
            orgs: HashMap::new(),
            finished_tx,
            team: TeamState::default(),
            syncing: false,
            sync_again: false,
            following: false,
            alerts: Vec::new(),
            unread_results: 0,
            unseen_failures: 0,
            error: None,
            revision: 0,
        }
    }

    /// The desktop app's runtime: follows the signed-in account and its
    /// plan from the account store.
    pub fn with_account(ctx: &mut ModelContext<Self>) -> Self {
        let this = Self::new(ctx);
        let store = crate::settings::AccountStore::handle(ctx);
        ctx.subscribe_to_model(&store, |this, _, ctx| this.refresh_account(ctx));
        let orgs = crate::settings::OrgStore::handle(ctx);
        ctx.subscribe_to_model(&orgs, |this, _, ctx| this.refresh_account(ctx));
        ctx.spawn(Timer::after(Duration::from_millis(50)), |this, _, ctx| {
            this.refresh_account(ctx)
        });
        this
    }

    fn refresh_account(&mut self, ctx: &mut ModelContext<Self>) {
        let data = crate::settings::AccountStore::handle(ctx)
            .read(ctx, |store, _| store.data.as_ref().map(|data| data.user.clone()));
        let org_paid = crate::settings::OrgStore::handle(ctx).read(ctx, |store, _| store.is_paid());
        let account = match data {
            Some(user) => Account {
                user_id: Some(user.id).filter(|id| !id.is_empty()),
                pro: org_paid || matches!(user.tier.as_str(), "pro" | "max" | "ultra" | "mega"),
            },
            None => Account::default(),
        };
        self.set_account(account, ctx);
    }

    /// Apply the signed-in account: whose automations run here and how many
    /// the plan allows.
    pub fn set_account(&mut self, account: Account, ctx: &mut ModelContext<Self>) {
        if self.account == account && self.scheduler.owner_id == account.user_id {
            return;
        }
        self.account = account;
        self.apply_plan();
        self.reconcile(ctx);
        self.changed(ctx);
        self.schedule_sync(ctx);
    }

    fn apply_plan(&mut self) {
        // The server's answer wins once it has one (it sees every machine).
        let pro = match &self.team.allowance {
            Some(allowance) if self.team.org == dragent_paths::organisation_id() => {
                allowance.limit.is_none()
            }
            _ => self.account.pro,
        };
        self.scheduler.owner_id = self.account.user_id.clone();
        self.scheduler.automation_limit = (!pro).then_some(FREE_AUTOMATION_LIMIT);
        let limit = self
            .scheduler
            .automation_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "unlimited".into());
        let owner = self.account.user_id.clone().unwrap_or_default();
        if let Some(store) = self.current_store() {
            let _ = store.set_meta(PLAN_LIMIT_KEY, &limit);
            let _ = store.set_meta(OWNER_KEY, &owner);
        }
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Bumped on every change; views compare it to know when to re-read.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Why automations can't load, if they can't.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn org(&mut self) -> Option<&mut OrgRuntime> {
        let key = dragent_paths::organisation_id();
        if !self.orgs.contains_key(&key) {
            let path = dragent_paths::organisation_dir().join("automations.sqlite");
            match Store::open(&path) {
                Ok(store) => {
                    // Nothing of this organisation runs in this process yet:
                    // anything mid-way belongs to a previous launch.
                    if let Err(error) = self.scheduler.recover_interrupted(&store, now_ms()) {
                        log::warn!("[automations] recovering interrupted runs: {error:#}");
                    }
                    self.error = None;
                    let mut org = OrgRuntime {
                        data_version: store.data_version(),
                        store: Arc::new(store),
                        active: Arc::default(),
                        cancel_flags: HashMap::new(),
                        run_workspaces: Vec::new(),
                        window_runs: HashMap::new(),
                        placed: HashSet::new(),
                    };
                    org.reload_run_workspaces();
                    self.orgs.insert(key.clone(), org);
                    self.apply_plan();
                    self.unseen_failures = self.count_unseen_failures();
                }
                Err(error) => {
                    self.error = Some(format!("Automations couldn't load: {error:#}"));
                    return None;
                }
            }
        }
        self.orgs.get_mut(&key)
    }

    fn store(&mut self) -> Option<Arc<Store>> {
        self.org().map(|org| org.store.clone())
    }

    fn changed(&mut self, ctx: &mut ModelContext<Self>) {
        self.unseen_failures = self.count_unseen_failures();
        self.unread_results = self.inbox(false).iter().filter(|run| run.is_unread()).count();
        self.revision = self.revision.wrapping_add(1);
        ctx.emit(AutomationEvent::Changed);
        ctx.notify();
    }

    fn tick(&mut self, ctx: &mut ModelContext<Self>) {
        self.reconcile(ctx);
        self.expire_window_runs(ctx);
        self.schedule_sync(ctx);
        ctx.spawn(Timer::after(TICK), |this, _, ctx| this.tick(ctx));
    }

    /// Pick up changes another process (the command line) committed.
    fn watch(&mut self, ctx: &mut ModelContext<Self>) {
        let changed = self.org().is_some_and(|org| {
            let version = org.store.data_version();
            let changed = version != org.data_version;
            org.data_version = version;
            changed
        });
        if changed {
            self.reconcile(ctx);
            self.changed(ctx);
            self.schedule_sync(ctx);
        }
        self.follow_runs(ctx);
        ctx.spawn(Timer::after(WATCH), |this, _, ctx| this.watch(ctx));
    }

    /// Catch up on what started runs' agents are doing (off the main thread).
    fn follow_runs(&mut self, ctx: &mut ModelContext<Self>) {
        if self.following {
            return;
        }
        let Some(store) = self.current().map(|org| org.store.clone()) else {
            return;
        };
        self.following = true;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let _ = tx.send(progress::follow(&store, now_ms()));
        });
        ctx.spawn(rx, |this, changes, ctx| {
            this.following = false;
            let Ok(changes) = changes else {
                return;
            };
            if changes.is_empty() {
                return;
            }
            for change in &changes {
                if let Some(alert) =
                    RunAlert::for_progress(&change.run, change.run.progress.as_ref(), &change.progress)
                {
                    this.alert(alert, ctx);
                }
            }
            this.changed(ctx);
        });
    }

    fn alert(&mut self, alert: RunAlert, ctx: &mut ModelContext<Self>) {
        // With no window open nothing takes them; only the latest matter.
        const KEEP: usize = 10;
        self.alerts.push(alert);
        if self.alerts.len() > KEEP {
            self.alerts.drain(..self.alerts.len() - KEEP);
        }
        ctx.emit(AutomationEvent::Alert);
    }

    /// The alerts to show (the first window to ask gets them).
    pub fn take_alerts(&mut self) -> Vec<RunAlert> {
        std::mem::take(&mut self.alerts)
    }

    /// Whether `pane` is where an automation run's agent works.
    pub fn is_run_pane(&self, pane: &str) -> bool {
        self.current_store()
            .and_then(|store| store.run_in_pane(pane).ok().flatten())
            .is_some()
    }

    /// Dragent's own agent in `pane` reached `state` (with its reply when
    /// it finished a turn): record it on the run that agent is working on.
    pub fn note_pane_progress(
        &mut self,
        pane: &str,
        state: AgentState,
        reply: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(store) = self.store() else {
            return;
        };
        let Ok(Some(run)) = store.run_in_pane(pane) else {
            return;
        };
        if run.progress.as_ref().is_some_and(|progress| progress.closed) {
            return;
        }
        let reply = reply.or_else(|| run.progress.as_ref().and_then(|progress| progress.reply.clone()));
        let settled = run.progress.as_ref().is_some_and(|progress| progress.settled);
        let progress = RunProgress {
            state,
            at_ms: now_ms(),
            reply,
            closed: false,
            settled: settled || RunProgress::settles(state),
        };
        let alert = RunAlert::for_progress(&run, run.progress.as_ref(), &progress);
        match store.set_progress(&run.id, progress) {
            Ok(Some(_)) => {
                if let Some(alert) = alert {
                    self.alert(alert, ctx);
                }
                self.changed(ctx)
            }
            Ok(None) => {}
            Err(error) => log::warn!("[automations] recording run progress: {error:#}"),
        }
    }

    /// Plan, queue, and start whatever is due for the selected organisation.
    pub fn reconcile(&mut self, ctx: &mut ModelContext<Self>) {
        let key = dragent_paths::organisation_id();
        let org_dir = dragent_paths::organisation_dir();
        let scheduler = self.scheduler.clone();
        let finished_tx = self.finished_tx.clone();
        let Some(org) = self.org() else {
            crate::settings::shortcut_settings::set_agents_running(false);
            ctx.notify();
            return;
        };
        let active = org.active.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let pass = match scheduler.reconcile(&org.store, &chrono::Local::now(), &active) {
            Ok(pass) => pass,
            Err(error) => {
                log::error!("[automations] reconcile failed: {error:#}");
                return;
            }
        };
        for run in pass.start.iter().cloned() {
            let cancelled = Arc::new(AtomicBool::new(false));
            org.cancel_flags.insert(run.id.clone(), cancelled.clone());
            org.active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(run.id.clone(), run.automation_id.clone());
            let store = org.store.clone();
            let scheduler = scheduler.clone();
            let finished_tx = finished_tx.clone();
            let key = key.clone();
            let org_dir = org_dir.clone();
            std::thread::Builder::new()
                .name("dragent-automation-run".into())
                .spawn(move || {
                    let run_id = run.id.clone();
                    let window = match executor::execute(run, store, scheduler, cancelled, org_dir)
                    {
                        executor::Outcome::Done => None,
                        executor::Outcome::NeedsWindow(run) => Some(run),
                    };
                    let _ = finished_tx.send_blocking((key, run_id, window));
                })
                .ok();
        }
        if let Some(org) = self.orgs.get_mut(&key) {
            org.data_version = org.store.data_version();
        }
        // Feed the keep-awake decision: active automation runs count as
        // agents running for Settings → Developer permissions → Auto mode.
        let any_running = self
            .org()
            .is_some_and(|org| !org.active.lock().unwrap_or_else(|p| p.into_inner()).is_empty())
            || !pass.start.is_empty();
        crate::settings::shortcut_settings::set_agents_running(any_running);
        crate::settings::shortcut_settings::sync_wake_lock(
            &crate::settings::AppSettings::global().dev_permissions,
        );
        if pass.changed || !pass.start.is_empty() {
            self.changed(ctx);
        }
    }

    fn on_run_finished(
        &mut self,
        org_key: &str,
        run_id: &str,
        window: Option<Run>,
        ctx: &mut ModelContext<Self>,
    ) {
        let mut started = None;
        let mut failed = None;
        if let Some(runtime) = self.orgs.get_mut(org_key) {
            runtime
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(run_id);
            runtime.cancel_flags.remove(run_id);
            if let Some(run) = window {
                runtime
                    .window_runs
                    .insert(run.id.clone(), (run.clone(), Instant::now()));
                if org_key == dragent_paths::organisation_id() {
                    ctx.emit(AutomationEvent::StartInWindow(run));
                }
            } else if let Ok(Some(run)) = runtime.store.get_run(run_id) {
                if run.status == RunStatus::Started {
                    started = Some(run);
                } else if let Some(alert) =
                    RunAlert::for_failure(&run).filter(|_| org_key == dragent_paths::organisation_id())
                {
                    failed = Some(alert);
                }
            }
        }
        if let Some(alert) = failed {
            self.alert(alert, ctx);
        }
        if let Some(run) = started.filter(|_| org_key == dragent_paths::organisation_id()) {
            ctx.emit(AutomationEvent::RunStarted(run));
        }
        self.changed(ctx);
        self.schedule_sync(ctx);
        // A slot freed up: start whatever is waiting.
        self.reconcile(ctx);
    }

    // ── Window hand-offs ─────────────────────────────────────────────────────

    /// The first window to ask places a started run's workspace.
    pub fn claim_placement(&mut self, run_id: &str) -> bool {
        self.org().is_some_and(|org| org.placed.insert(run_id.to_string()))
    }

    /// Runs started in this session whose workspace no window placed yet
    /// (e.g. no window was open when they started).
    pub fn unplaced_runs(&self) -> Vec<Run> {
        let Some(org) = self.current() else {
            return Vec::new();
        };
        let since = now_ms() - 24 * 60 * 60 * 1000;
        org.store
            .finished_since(since)
            .unwrap_or_default()
            .into_iter()
            .filter(|run| run.status == RunStatus::Started && !org.placed.contains(&run.id))
            .filter(|run| {
                // Placed in an earlier session: if the user closed it since,
                // it stays closed.
                run.workspace_path.as_ref().is_some_and(|path| {
                    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                    path.is_dir()
                        && !org
                            .run_workspaces
                            .iter()
                            .any(|placed| placed.path == canonical || placed.run_id == run.id)
                })
            })
            .collect()
    }

    /// Take a run waiting to start Dragent's agent (first window wins).
    pub fn claim_window_run(&mut self, run_id: &str) -> Option<Run> {
        self.org()?.window_runs.remove(run_id).map(|(run, _)| run)
    }

    /// Runs waiting for a window right now.
    pub fn waiting_window_runs(&self) -> Vec<Run> {
        self.current()
            .map(|org| org.window_runs.values().map(|(run, _)| run.clone()).collect())
            .unwrap_or_default()
    }

    /// A window started (or failed to start) a Dragent-agent run in `pane`.
    /// `continued`: it went to the conversation an earlier run left.
    pub fn finish_window_run(
        &mut self,
        run_id: &str,
        result: Result<(bool, uuid::Uuid), String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let scheduler = self.scheduler.clone();
        let Some(org) = self.org() else {
            return;
        };
        let recorded = match result {
            Ok((continued, pane)) => scheduler
                .mark_started(&org.store, run_id, None, continued, now_ms())
                .and_then(|_| org.store.set_pane(run_id, &pane.to_string()))
                .and_then(|_| {
                    let working = RunProgress {
                        state: AgentState::Working,
                        at_ms: now_ms(),
                        reply: None,
                        closed: false,
                        settled: false,
                    };
                    org.store.set_progress(run_id, working)
                }),
            Err(message) => scheduler.mark_failed(
                &org.store,
                run_id,
                RunError::new(RunStep::Agent, "start_failed").with_message(message),
                now_ms(),
            ),
        };
        let failure = match recorded {
            Err(error) => {
                log::error!("[automations] recording window run {run_id}: {error:#}");
                None
            }
            Ok(run) => run.as_ref().and_then(RunAlert::for_failure),
        };
        org.placed.insert(run_id.to_string());
        if let Some(alert) = failure {
            self.alert(alert, ctx);
        }
        self.changed(ctx);
        self.schedule_sync(ctx);
    }

    fn expire_window_runs(&mut self, ctx: &mut ModelContext<Self>) {
        let scheduler = self.scheduler.clone();
        let Some(org) = self.org() else {
            return;
        };
        let expired: Vec<String> = org
            .window_runs
            .iter()
            .filter(|(_, (_, since))| since.elapsed() >= WINDOW_WAIT)
            .map(|(id, _)| id.clone())
            .collect();
        let mut failures = Vec::new();
        for id in &expired {
            org.window_runs.remove(id);
            let failed = scheduler.mark_failed(
                &org.store,
                id,
                RunError::new(RunStep::Agent, "no_window"),
                now_ms(),
            );
            failures.extend(failed.ok().flatten().as_ref().and_then(RunAlert::for_failure));
        }
        for alert in failures {
            self.alert(alert, ctx);
        }
        if !expired.is_empty() {
            self.changed(ctx);
        }
    }

    /// Remember the workspace a run put in the sidebar (for the header).
    pub fn record_run_workspace(&mut self, run: &Run, path: &Path, ctx: &mut ModelContext<Self>) {
        let Some(org) = self.org() else {
            return;
        };
        let record = RunWorkspace {
            path: path.to_path_buf(),
            run_id: run.id.clone(),
            automation_id: run.automation_id.clone(),
            automation_name: run.snapshot.name.clone(),
            run_at_ms: run.display_time_ms(),
        };
        if let Err(error) = org.store.record_run_workspace(&record) {
            log::warn!("[automations] recording run workspace: {error:#}");
            return;
        }
        org.reload_run_workspaces();
        ctx.notify();
    }

    // ── Reads ────────────────────────────────────────────────────────────────
    // Reads use the store already opened for the selected organisation (the
    // first tick or an organisation switch opens it); they never block on
    // opening one, so views can render from `&self`.

    fn current(&self) -> Option<&OrgRuntime> {
        self.orgs.get(&dragent_paths::organisation_id())
    }

    fn current_store(&self) -> Option<&Store> {
        self.current().map(|org| org.store.as_ref())
    }

    /// The selected organisation's automations are loaded.
    pub fn is_ready(&self) -> bool {
        self.current_store().is_some()
    }

    /// This account's automations (the Mine tab).
    pub fn automations(&self) -> Vec<Automation> {
        self.current_store()
            .and_then(|store| store.list_automations().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|automation| self.scheduler.owns(automation))
            .collect()
    }

    pub fn automation(&self, id: &str) -> Option<Automation> {
        self.current_store()?.get_automation(id).ok().flatten()
    }

    pub fn latest_run(&self, automation_id: &str) -> Option<Run> {
        self.current_store()?.latest_run(automation_id).ok().flatten()
    }

    /// Each automation's latest run that actually happened.
    pub fn latest_runs(&self) -> HashMap<String, Run> {
        self.current_store()
            .and_then(|store| store.latest_runs().ok())
            .unwrap_or_default()
    }

    pub fn scheduled_run(&self, automation_id: &str) -> Option<Run> {
        self.current_store()?.scheduled_run(automation_id).ok().flatten()
    }

    pub fn run(&self, run_id: &str) -> Option<Run> {
        self.current_store()?.get_run(run_id).ok().flatten()
    }

    pub fn runs(&self, automation_id: &str, filter: RunFilter, limit: usize) -> Vec<Run> {
        self.current_store()
            .and_then(|store| store.list_runs(automation_id, filter, None, limit).ok())
            .unwrap_or_default()
    }

    pub fn run_counts(&self, automation_id: &str) -> HashMap<RunFilter, usize> {
        self.current_store()
            .and_then(|store| store.run_counts(automation_id).ok())
            .unwrap_or_default()
    }

    /// Runs being prepared or started right now for an automation.
    pub fn is_active(&self, automation_id: &str) -> bool {
        self.current().is_some_and(|org| {
            org.active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .values()
                .any(|id| id == automation_id)
                || org
                    .window_runs
                    .values()
                    .any(|(run, _)| run.automation_id == automation_id)
        })
    }

    /// Automations whose latest run failed (the "Needs attention" group).
    pub fn failing(&self) -> HashSet<String> {
        let owned: HashSet<String> = self.automations().into_iter().map(|a| a.id).collect();
        self.latest_runs()
            .into_values()
            .filter(|run| run.status == RunStatus::Failed && owned.contains(&run.automation_id))
            .map(|run| run.automation_id)
            .collect()
    }

    // ── Inbox ────────────────────────────────────────────────────────────────

    /// What runs came back with in the last 30 days, newest first: the ones
    /// filed away when `archived`, else the rest.
    pub fn inbox(&self, archived: bool) -> Vec<Run> {
        const DAYS: i64 = 30;
        const LIMIT: usize = 200;
        let Some(store) = self.current_store() else {
            return Vec::new();
        };
        store
            .inbox_runs(now_ms() - DAYS * 24 * 60 * 60 * 1000, LIMIT)
            .unwrap_or_default()
            .into_iter()
            .filter(|run| run.is_archived() == archived)
            .collect()
    }

    pub fn unread_results(&self) -> usize {
        self.unread_results
    }

    /// Someone opened a run's result. Returns the run.
    pub fn mark_read(&mut self, run_id: &str, ctx: &mut ModelContext<Self>) -> Option<Run> {
        let store = self.store()?;
        let run = store.get_run(run_id).ok().flatten()?;
        if !run.is_unread() {
            return Some(run);
        }
        match store.mark_run_read(run_id, now_ms()) {
            Ok(Some(run)) => {
                self.changed(ctx);
                Some(run)
            }
            Ok(None) => Some(run),
            Err(error) => {
                log::warn!("[automations] marking a run read: {error:#}");
                Some(run)
            }
        }
    }

    pub fn set_archived(&mut self, run_id: &str, archived: bool, ctx: &mut ModelContext<Self>) {
        let Some(store) = self.store() else {
            return;
        };
        match store.set_run_archived(run_id, archived) {
            Ok(Some(_)) => self.changed(ctx),
            Ok(None) => {}
            Err(error) => log::warn!("[automations] filing a run: {error:#}"),
        }
    }

    pub fn mark_all_read(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(store) = self.store() else {
            return;
        };
        let now = now_ms();
        for run in self.inbox(false).into_iter().filter(Run::is_unread) {
            if let Err(error) = store.mark_run_read(&run.id, now) {
                log::warn!("[automations] marking a run read: {error:#}");
            }
        }
        self.changed(ctx);
    }

    /// Failures since the user last opened the page (the sidebar badge).
    pub fn unseen_failures(&self) -> usize {
        self.unseen_failures
    }

    fn count_unseen_failures(&self) -> usize {
        let Some(store) = self.current_store() else {
            return 0;
        };
        let seen: i64 = store
            .meta(FAILURES_SEEN)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let failing = self.failing();
        self.latest_runs()
            .into_values()
            .filter(|run| failing.contains(&run.automation_id))
            .filter(|run| run.finished_at_ms.unwrap_or(0) > seen)
            .count()
    }

    pub fn mark_failures_seen(&mut self, ctx: &mut ModelContext<Self>) {
        if self.unseen_failures == 0 {
            return;
        }
        if let Some(store) = self.store() {
            let _ = store.set_meta(FAILURES_SEEN, &now_ms().to_string());
        }
        self.unseen_failures = 0;
        ctx.notify();
    }

    pub fn stats(&self) -> Stats {
        let automations = self.automations();
        let owned: HashSet<&str> = automations.iter().map(|a| a.id.as_str()).collect();
        let since = now_ms() - 7 * 24 * 60 * 60 * 1000;
        let recent = self
            .current_store()
            .and_then(|store| store.finished_since(since).ok())
            .unwrap_or_default();
        let mine = recent
            .iter()
            .filter(|run| owned.contains(run.automation_id.as_str()));
        let (mut created_7d, mut failed_7d) = (0, 0);
        for run in mine {
            match run.status {
                RunStatus::Started => created_7d += 1,
                RunStatus::Failed => failed_7d += 1,
                _ => {}
            }
        }
        Stats {
            active: automations.iter().filter(|a| a.enabled).count(),
            created_7d,
            failed_7d,
        }
    }

    /// The plan's limit (`None` = unlimited) and how many are owned.
    pub fn allowance(&self) -> (Option<usize>, usize) {
        let local = self.automations().len();
        let owned = match &self.team.allowance {
            Some(allowance) if self.team.org == dragent_paths::organisation_id() => {
                allowance.owned.max(local)
            }
            _ => local,
        };
        (self.scheduler.automation_limit, owned)
    }

    /// Over the free plan's allowance: kept, but not run.
    pub fn is_over_allowance(&self, automation_id: &str) -> bool {
        self.scheduler.automation_limit.is_some()
            && !self
                .scheduler
                .runnable(self.automations().iter())
                .contains(automation_id)
    }

    /// Creating another automation would exceed the plan.
    pub fn at_limit(&self) -> bool {
        let (limit, owned) = self.allowance();
        limit.is_some_and(|limit| owned >= limit)
    }

    pub fn prompt_versions(&self, automation_id: &str) -> Vec<PromptVersion> {
        self.current_store()
            .and_then(|store| store.prompt_versions(automation_id).ok())
            .unwrap_or_default()
    }

    pub fn copy_patterns(&self, project_root: &Path) -> Vec<String> {
        let main = crate::workspace::project_naming::worktree_main_checkout(project_root)
            .unwrap_or_else(|| project_root.to_path_buf());
        self.current_store()
            .and_then(|store| store.copy_patterns(&main).ok())
            .unwrap_or_default()
    }

    /// The workspace at `path` that a run put in the sidebar.
    pub fn run_workspace(&self, path: &Path) -> Option<RunWorkspace> {
        self.current()?
            .run_workspaces
            .iter()
            .find(|workspace| workspace.path == path)
            .cloned()
    }

    /// A small value kept with the organisation's automations.
    pub fn meta(&self, key: &str) -> Option<String> {
        self.current_store()?.meta(key).ok().flatten()
    }

    pub fn set_meta(&mut self, key: &str, value: &str) {
        if let Some(store) = self.store() {
            let _ = store.set_meta(key, value);
        }
    }

    pub fn team(&self) -> &TeamState {
        &self.team
    }

    /// Folder project-less session runs work in.
    pub fn sessions_root(&self) -> PathBuf {
        executor::sessions_root(&dragent_paths::organisation_dir())
    }

    // ── Changes ──────────────────────────────────────────────────────────────

    /// Save a new or edited automation (validated first).
    pub fn save(
        &mut self,
        automation: Automation,
        ctx: &mut ModelContext<Self>,
    ) -> Result<Automation, String> {
        self.save_from(automation, PromptSource::Human, ctx)
    }

    fn save_from(
        &mut self,
        mut automation: Automation,
        source: PromptSource,
        ctx: &mut ModelContext<Self>,
    ) -> Result<Automation, String> {
        automation.tags = dragent_automations::normalize_tags(automation.tags);
        automation.validate().map_err(|error| error.to_string())?;
        let store = self.store().ok_or_else(|| {
            self.error
                .clone()
                .unwrap_or_else(|| "Automations are unavailable.".into())
        })?;
        match store.get_automation(&automation.id) {
            Ok(Some(existing)) => {
                if existing == automation {
                    return Ok(automation);
                }
                automation.revision = existing.revision.saturating_add(1);
                automation.created_at_ms = existing.created_at_ms;
                automation.owner_id = existing.owner_id;
            }
            _ => {
                if self.at_limit() {
                    return Err(LIMIT_MESSAGE.into());
                }
                if automation.owner_id.is_empty() {
                    automation.owner_id = self.account.user_id.clone().unwrap_or_default();
                }
            }
        }
        automation.updated_at_ms = now_ms();
        store
            .save_automation(&automation)
            .map_err(|error| format!("Couldn't save the automation: {error:#}"))?;
        let _ = store.add_prompt_version(&automation.id, &automation.prompt, source, now_ms());
        self.changed(ctx);
        self.reconcile(ctx);
        self.schedule_sync(ctx);
        Ok(automation)
    }

    pub fn set_enabled(
        &mut self,
        id: &str,
        enabled: bool,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), String> {
        let Some(mut automation) = self.automation(id) else {
            return Err("This automation no longer exists.".into());
        };
        if automation.enabled == enabled {
            return Ok(());
        }
        if enabled && self.is_over_allowance(id) {
            return Err(LIMIT_MESSAGE.into());
        }
        automation.enabled = enabled;
        self.save(automation, ctx).map(|_| ())
    }

    /// Bring back an earlier prompt as the current one.
    pub fn restore_prompt(
        &mut self,
        automation_id: &str,
        version_id: i64,
        ctx: &mut ModelContext<Self>,
    ) -> Result<(), String> {
        let store = self.store().ok_or_else(|| "Automations are unavailable.".to_string())?;
        let version = store
            .prompt_version(version_id)
            .ok()
            .flatten()
            .filter(|version| version.automation_id == automation_id)
            .ok_or_else(|| "That version no longer exists.".to_string())?;
        let mut automation = self
            .automation(automation_id)
            .ok_or_else(|| "This automation no longer exists.".to_string())?;
        automation.prompt = version.prompt;
        self.save_from(automation, PromptSource::Restore, ctx).map(|_| ())
    }

    pub fn set_copy_patterns(
        &mut self,
        project_root: &Path,
        patterns: Vec<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let main = crate::workspace::project_naming::worktree_main_checkout(project_root)
            .unwrap_or_else(|| project_root.to_path_buf());
        let patterns: Vec<String> = patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        if let Some(store) = self.store() {
            if let Err(error) = store.set_copy_patterns(&main, &patterns) {
                log::warn!("[automations] saving copy patterns: {error:#}");
            }
        }
        self.changed(ctx);
    }

    /// Delete an automation and its run history. Runs in flight are
    /// cancelled; agents that already started keep their terminals.
    pub fn delete(&mut self, id: &str, ctx: &mut ModelContext<Self>) {
        let scheduler = self.scheduler.clone();
        let Some(org) = self.org() else {
            return;
        };
        for (run_id, automation_id) in org.active.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            if automation_id == id {
                if let Some(flag) = org.cancel_flags.get(run_id) {
                    flag.store(true, Ordering::Release);
                }
                let _ = scheduler.cancel(&org.store, run_id, now_ms());
            }
        }
        org.window_runs.retain(|_, (run, _)| run.automation_id != id);
        if let Err(error) = org.store.delete_automation(id) {
            log::error!("[automations] deleting {id}: {error:#}");
        }
        team::queue_delete(&org.store, id);
        self.changed(ctx);
        self.schedule_sync(ctx);
    }

    pub fn run_now(&mut self, id: &str, ctx: &mut ModelContext<Self>) -> Result<(), String> {
        let automation = self
            .automation(id)
            .ok_or_else(|| "This automation no longer exists.".to_string())?;
        if !automation.enabled {
            return Err("Turn the automation on before running it.".into());
        }
        if self.is_over_allowance(id) {
            return Err(LIMIT_MESSAGE.into());
        }
        let store = self.store().ok_or_else(|| "Automations are unavailable.".to_string())?;
        self.scheduler
            .run_now(&store, &automation)
            .map_err(|error| format!("Couldn't queue the run: {error:#}"))?;
        self.changed(ctx);
        self.reconcile(ctx);
        Ok(())
    }

    /// Run every owned automation whose latest run failed. Returns how many
    /// were queued.
    pub fn retry_failed(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let failing: Vec<String> = self.failing().into_iter().collect();
        let mut queued = 0;
        for id in failing {
            if self.run_now(&id, ctx).is_ok() {
                queued += 1;
            }
        }
        queued
    }

    pub fn cancel_run(&mut self, run_id: &str, ctx: &mut ModelContext<Self>) {
        let scheduler = self.scheduler.clone();
        let Some(org) = self.org() else {
            return;
        };
        if let Some(flag) = org.cancel_flags.get(run_id) {
            flag.store(true, Ordering::Release);
        }
        if org.window_runs.remove(run_id).is_some() {
            let _ = scheduler.mark_failed(
                &org.store,
                run_id,
                RunError::new(RunStep::Queue, codes::CANCELLED),
                now_ms(),
            );
        }
        let _ = scheduler.cancel(&org.store, run_id, now_ms());
        self.changed(ctx);
        self.reconcile(ctx);
    }

    // ── Organisation registry ────────────────────────────────────────────────

    /// The server id of the selected organisation, when signed in. (The
    /// local context id differs for a personal organisation.)
    fn signed_in_org(&self, ctx: &ModelContext<Self>) -> Option<String> {
        self.account.user_id.as_ref()?;
        crate::organisation_context::OrganisationContext::handle(ctx)
            .read(ctx, |context, _| {
                context
                    .active()
                    .filter(|active| active.context == dragent_paths::organisation_id())
                    .map(|active| active.organisation_id.clone())
            })
            .filter(|id| !id.is_empty())
    }

    /// Send changed automations to the registry (one pass at a time).
    fn schedule_sync(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(org_id) = self.signed_in_org(ctx) else {
            return;
        };
        if self.syncing {
            self.sync_again = true;
            return;
        }
        let Some(store) = self.store() else {
            return;
        };
        let owned = self.automations();
        self.syncing = true;
        self.sync_again = false;
        let (tx, rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("dragent-automation-sync".into())
            .spawn(move || {
                let report = team::sync(&org_id, &store, &owned);
                let _ = tx.send_blocking(report);
            })
            .ok();
        ctx.spawn(async move { rx.recv().await.ok() }, |this, report, ctx| {
            this.syncing = false;
            if let Some(report) = report {
                if let Some(error) = &report.error {
                    log::debug!("[automations] registry sync paused: {error}");
                }
                if report.sent > 0 || !report.over_limit.is_empty() {
                    // The registry changed: the Team tab and allowance follow.
                    this.refresh_team(true, ctx);
                }
            }
            if this.sync_again {
                this.schedule_sync(ctx);
            }
        });
    }

    /// Fetch the organisation's registry (the Team tab and the allowance).
    pub fn refresh_team(&mut self, force: bool, ctx: &mut ModelContext<Self>) {
        let Some(org_id) = self.signed_in_org(ctx) else {
            self.team = TeamState::default();
            return;
        };
        let context = dragent_paths::organisation_id();
        if self.team.loading && self.team.org == context {
            return;
        }
        let fresh = self.team.org == context
            && self
                .team
                .fetched_at
                .is_some_and(|at| at.elapsed() < TEAM_REFRESH);
        if fresh && !force {
            return;
        }
        if self.team.org != context {
            self.team = TeamState {
                org: context.clone(),
                ..TeamState::default()
            };
        }
        self.team.loading = true;
        let (tx, rx) = async_channel::bounded(1);
        let fetch_org = org_id;
        std::thread::Builder::new()
            .name("dragent-automation-team".into())
            .spawn(move || {
                let _ = tx.send_blocking(team::fetch(&fetch_org));
            })
            .ok();
        ctx.spawn(async move { rx.recv().await.ok() }, move |this, result, ctx| {
            if this.team.org != context {
                return;
            }
            this.team.loading = false;
            this.team.fetched_at = Some(Instant::now());
            match result {
                Some(Ok((automations, allowance))) => {
                    this.team.automations = automations;
                    this.team.allowance = Some(allowance);
                    this.team.error = None;
                    this.apply_plan();
                    this.reconcile(ctx);
                }
                Some(Err(error)) => this.team.error = Some(error),
                None => {}
            }
            this.changed(ctx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_read_compactly() {
        let now = 1_000_000_000_000;
        assert_eq!(compact_ago(now - 20_000, now), "just now");
        assert_eq!(compact_ago(now - 5 * 60_000, now), "5m ago");
        assert_eq!(compact_ago(now - 3 * 3_600_000, now), "3h ago");
        assert_eq!(compact_until(now + 20_000, now), "soon");
        assert_eq!(compact_until(now + 2 * 86_400_000, now), "in 2d");
        // Clock skew never reads as negative.
        assert_eq!(compact_ago(now + 60_000, now), "just now");
    }

    fn started_run() -> Run {
        let store = Store::open_in_memory().unwrap();
        let automation = Automation::new(
            "Nightly review",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "Review",
            dragent_automations::WorkspaceMode::default(),
            0,
        );
        store.save_automation(&automation).unwrap();
        let mut run = Scheduler::default().run_now(&store, &automation).unwrap();
        run.status = RunStatus::Started;
        run
    }

    fn progress(state: AgentState, reply: Option<&str>) -> RunProgress {
        RunProgress {
            state,
            at_ms: 1,
            reply: reply.map(str::to_owned),
            closed: false,
            settled: RunProgress::settles(state),
        }
    }

    #[test]
    fn runs_alert_when_their_agent_finishes_fails_or_waits() {
        let run = started_run();
        let working = progress(AgentState::Working, None);
        let done = progress(AgentState::Done, Some("# Findings\n\nAll tests pass."));
        let alert = RunAlert::for_progress(&run, Some(&working), &done).unwrap();
        assert_eq!(alert.title, "Nightly review");
        assert_eq!(alert.body, "Findings All tests pass.");
        assert!(!alert.urgent);
        assert_eq!(run_id_from_alert(&alert.data()), Some(run.id.as_str()));

        let waiting = progress(AgentState::NeedsInput, None);
        assert!(RunAlert::for_progress(&run, Some(&working), &waiting).unwrap().urgent);
        assert!(RunAlert::for_progress(&run, Some(&waiting), &progress(AgentState::TimedOut, None)).is_some());
        assert!(RunAlert::for_progress(&run, Some(&working), &progress(AgentState::Failed, None)).is_some());

        // Nothing to report files itself away quietly.
        let quiet = progress(AgentState::Done, Some("Nothing to report. All checks pass."));
        assert!(RunAlert::for_progress(&run, Some(&working), &quiet).is_none());

        // Starting, the same state again, or a closed session: nothing to say.
        assert!(RunAlert::for_progress(&run, None, &working).is_none());
        assert!(RunAlert::for_progress(&run, Some(&done), &done).is_none());
        assert!(RunAlert::for_progress(&run, Some(&working), &progress(AgentState::Ended, None)).is_none());
    }

    #[test]
    fn runs_that_fail_to_start_alert_with_the_reason() {
        let mut run = started_run();
        assert!(RunAlert::for_failure(&run).is_none());
        run.status = RunStatus::Failed;
        run.error = Some(RunError::new(RunStep::Agent, "no_window"));
        let alert = RunAlert::for_failure(&run).unwrap();
        assert!(alert.body.contains("needs an open Dragent window"));
        assert_eq!(run_id_from_alert("something else"), None);
    }
}
