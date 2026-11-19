//! Automations and their runs.
//!
//! An automation starts an agent on a schedule: at each due time a run
//! prepares a workspace (a fresh worktree, a worktree on an existing branch,
//! one pinned workspace reused every time, or a project-less session
//! folder), then launches the chosen agent there with the automation's
//! prompt. A run is finished once its agent has started; the conversation
//! then lives on in its terminal and can be opened like any other
//! workspace.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where a run's agent works.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// A new branch and linked worktree for every run, started from
    /// `base_branch` (the project's current branch when unset), optionally
    /// published to the remote as soon as it exists.
    NewWorktree {
        #[serde(default)]
        base_branch: Option<String>,
        #[serde(default)]
        push_branch: bool,
    },
    /// A worktree checked out on an existing branch (the one that already
    /// has it checked out, if any).
    ExistingBranch { branch: String },
    /// The same workspace every run: that worktree (or session folder), or
    /// when `path` is unset, the project's own checkout. A session
    /// automation without a pinned folder gets a fresh folder per run.
    #[serde(alias = "project_checkout")]
    Pinned {
        #[serde(default)]
        path: Option<PathBuf>,
    },
}

impl Default for WorkspaceMode {
    fn default() -> Self {
        Self::NewWorktree {
            base_branch: None,
            push_branch: false,
        }
    }
}

impl WorkspaceMode {
    pub fn is_pinned(&self) -> bool {
        matches!(self, Self::Pinned { .. })
    }
}

/// Every automation files its runs into a sidebar folder unless the user
/// clears its tags.
pub const DEFAULT_TAG: &str = "automation";

fn default_tags() -> Vec<String> {
    vec![DEFAULT_TAG.to_string()]
}

/// Tags are compared and stored trimmed and lowercased, without
/// duplicates, in a stable order.
pub fn normalize_tags(tags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty() && tag.chars().count() <= 64)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Gitignored files copied from the project into each new worktree, so a
/// run starts with the same local configuration (secrets, overrides).
pub const DEFAULT_COPY_PATTERNS: [&str; 6] = [
    ".env",
    ".env.keys",
    ".env.local",
    ".env.*.local",
    ".envrc",
    "docker-compose.override.yml",
];

/// The machine's IANA time zone, used for new automations.
pub fn local_time_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_default()
}

/// What starts an automation's runs besides its schedule: something new
/// showing up in a search of a connected service. The schedule then says
/// how often to look, and a run starts only when the search finds items it
/// hadn't found before (all of them in that one run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    /// The service's key (`linear`, `sentry`, `github`, …).
    pub service: String,
    /// What to search for there, as the service's search box takes it.
    /// Empty: its recent, open or assigned items.
    #[serde(default)]
    pub query: String,
}

/// What happens to an occurrence whose time passed while the app was
/// closed or the machine asleep.
///
/// - `Skip` (default): the stale occurrence is recorded as skipped and the
///   next one is planned. A closed laptop never wakes up to a burst.
/// - `CatchUp`: the most recent stale occurrence runs once on return,
///   instead of being skipped. Only one catch-up run happens no matter how
///   long the app was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedPolicy {
    #[default]
    Skip,
    #[serde(
        alias = "catch-up",
        alias = "catchup",
        alias = "run_once",
        alias = "run-once"
    )]
    CatchUp,
}

impl MissedPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::CatchUp => "catch_up",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "skip" => Some(Self::Skip),
            "catch_up" | "catchup" | "run_once" | "runonce" => Some(Self::CatchUp),
            _ => None,
        }
    }
}

/// What happens when an occurrence is due while the previous run of the
/// same automation is still starting.
///
/// - `Skip` (default): the new occurrence is recorded as skipped.
/// - `Queue`: it stays queued and starts when the previous run leaves the
///   starting states.
/// - `Allow`: it starts alongside the previous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    #[default]
    Skip,
    Queue,
    Allow,
}

impl OverlapPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Queue => "queue",
            Self::Allow => "allow",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "skip" => Some(Self::Skip),
            "queue" => Some(Self::Queue),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }
}

fn default_retry_delay_secs() -> u64 {
    300
}

fn default_run_retention() -> usize {
    100
}

/// Upper bound for configured retries, so a failing automation cannot loop
/// forever.
pub const MAX_RETRIES_LIMIT: u32 = 10;
/// Upper bound for stored run history per automation.
pub const RUN_RETENTION_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    /// Empty for a session automation: each run works in a project-less
    /// folder of its own.
    pub project_root: PathBuf,
    /// Five-field cron expression, evaluated in `timezone`.
    pub schedule: String,
    /// IANA zone the schedule is read in; empty means the machine's own.
    #[serde(default)]
    pub timezone: String,
    /// Agent registry key (`claude`, `codex`, `dragent`, …).
    pub agent: String,
    pub prompt: String,
    #[serde(default)]
    pub workspace: WorkspaceMode,
    /// Sidebar folders each run's workspace is filed into.
    #[serde(default = "default_tags")]
    pub tags: Vec<String>,
    /// With a pinned workspace: deliver each prompt to the agent the
    /// previous run left running there instead of starting another.
    #[serde(default)]
    pub continue_session: bool,
    /// Its agent may comment, change statuses and create items in connected
    /// services (Linear, Jira, …). Off: runs only read from them.
    #[serde(default)]
    pub service_writes: bool,
    /// Runs start only when this search finds something new.
    #[serde(default)]
    pub watch: Option<Watch>,
    /// What a missed occurrence does when the app returns. See
    /// [`MissedPolicy`].
    #[serde(default)]
    pub missed_policy: MissedPolicy,
    /// What a due occurrence does while the previous run is still
    /// starting. See [`OverlapPolicy`].
    #[serde(default)]
    pub overlap_policy: OverlapPolicy,
    /// How many times a failed run is retried (0 = no retries).
    #[serde(default)]
    pub max_retries: u32,
    /// Base delay before the first retry, in seconds. Each further retry
    /// waits twice as long (exponential backoff).
    #[serde(default = "default_retry_delay_secs")]
    pub retry_delay_secs: u64,
    /// How many past runs are kept per automation (the planned occurrence
    /// is always kept). Older runs are pruned.
    #[serde(default = "default_run_retention")]
    pub run_retention: usize,
    /// The account that created it; only its owner's app runs it.
    #[serde(default)]
    pub owner_id: String,
    pub enabled: bool,
    /// Bumped on every saved change.
    pub revision: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Automation {
    pub fn new(
        name: impl Into<String>,
        project_root: PathBuf,
        schedule: impl Into<String>,
        agent: impl Into<String>,
        prompt: impl Into<String>,
        workspace: WorkspaceMode,
        now_ms: i64,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            project_root,
            schedule: schedule.into(),
            agent: agent.into(),
            prompt: prompt.into(),
            workspace,
            tags: default_tags(),
            continue_session: false,
            service_writes: false,
            watch: None,
            missed_policy: MissedPolicy::default(),
            overlap_policy: OverlapPolicy::default(),
            max_retries: 0,
            retry_delay_secs: default_retry_delay_secs(),
            run_retention: default_run_retention(),
            owner_id: String::new(),
            timezone: String::new(),
            enabled: true,
            revision: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    /// Runs in a project-less session folder instead of a project.
    pub fn is_session(&self) -> bool {
        self.project_root.as_os_str().is_empty()
    }

    /// The zone the schedule is read in, when one is set.
    pub fn time_zone(&self) -> Option<chrono_tz::Tz> {
        let name = self.timezone.trim();
        (!name.is_empty()).then(|| name.parse().ok()).flatten()
    }

    /// The schedule's next time after `now_ms`, read in the automation's
    /// zone (this computer's when it names none), as the app will plan it.
    /// Wall-clock times are read in that zone: `0 9 * * *` with
    /// `America/New_York` runs at 9:00 in New York, whatever that is in UTC.
    pub fn next_occurrence_ms(&self, now_ms: i64) -> Option<i64> {
        use chrono::TimeZone;
        let schedule = crate::cron::Schedule::parse(&self.schedule).ok()?;
        let zone: chrono_tz::Tz = self
            .time_zone()
            .or_else(|| local_time_zone().parse().ok())?;
        let now = zone.timestamp_millis_opt(now_ms).single()?;
        schedule.next_after(&now).map(|at| at.timestamp_millis())
    }

    /// Delay before retry number `attempt` (0 = the first retry after the
    /// initial failure), or `None` when no more retries are allowed.
    /// Exponential backoff: base, base*2, base*4, … capped at a day.
    pub fn retry_delay_for_attempt(&self, attempt: u32) -> Option<u64> {
        if attempt >= self.max_retries {
            return None;
        }
        let shift = attempt.min(10);
        let delay = self.retry_delay_secs.saturating_mul(1u64 << shift);
        Some(delay.min(24 * 3600))
    }

    /// The configuration a run executes, frozen when the run is created.
    pub fn snapshot(&self) -> RunSnapshot {
        RunSnapshot {
            name: self.name.clone(),
            project_root: self.project_root.clone(),
            schedule: self.schedule.clone(),
            agent: self.agent.clone(),
            prompt: self.prompt.clone(),
            workspace: self.workspace.clone(),
            tags: self.tags.clone(),
            continue_session: self.continue_session && self.workspace.is_pinned(),
            service_writes: self.service_writes,
            watch: self.watch.clone(),
        }
    }

    /// Problems that keep this automation from being saved.
    pub fn validate(&self) -> Result<(), DefinitionError> {
        if self.name.trim().is_empty() {
            return Err(DefinitionError::NameRequired);
        }
        if self.prompt.trim().is_empty() {
            return Err(DefinitionError::PromptRequired);
        }
        if self.agent.trim().is_empty() {
            return Err(DefinitionError::AgentRequired);
        }
        if self
            .watch
            .as_ref()
            .is_some_and(|watch| watch.service.trim().is_empty())
        {
            return Err(DefinitionError::ServiceRequired);
        }
        match &self.workspace {
            WorkspaceMode::ExistingBranch { branch } if branch.trim().is_empty() => {
                return Err(DefinitionError::BranchRequired);
            }
            WorkspaceMode::NewWorktree { .. } | WorkspaceMode::ExistingBranch { .. }
                if self.is_session() =>
            {
                return Err(DefinitionError::ProjectRequired);
            }
            _ => {}
        }
        if !self.timezone.trim().is_empty() && self.time_zone().is_none() {
            return Err(DefinitionError::TimeZone(self.timezone.clone()));
        }
        crate::cron::Schedule::parse(&self.schedule).map_err(DefinitionError::Schedule)?;
        if self.max_retries > MAX_RETRIES_LIMIT {
            return Err(DefinitionError::Retries(format!(
                "at most {MAX_RETRIES_LIMIT} retries"
            )));
        }
        if self.max_retries > 0 && (self.retry_delay_secs < 10 || self.retry_delay_secs > 24 * 3600)
        {
            return Err(DefinitionError::RetryDelay(
                "the retry delay is 10 seconds to 24 hours".into(),
            ));
        }
        if self.run_retention == 0 || self.run_retention > RUN_RETENTION_LIMIT {
            return Err(DefinitionError::Retention(format!(
                "keep 1 to {RUN_RETENTION_LIMIT} runs"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    NameRequired,
    PromptRequired,
    AgentRequired,
    ServiceRequired,
    ProjectRequired,
    BranchRequired,
    TimeZone(String),
    Schedule(String),
    Retries(String),
    RetryDelay(String),
    Retention(String),
    MissedPolicy(String),
    OverlapPolicy(String),
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameRequired => write!(f, "Give the automation a name."),
            Self::PromptRequired => write!(f, "Write the prompt the agent should start with."),
            Self::AgentRequired => write!(f, "Choose an agent."),
            Self::ServiceRequired => write!(f, "Choose the service to watch."),
            Self::ProjectRequired => {
                write!(f, "Choose a project, or run it as a session.")
            }
            Self::BranchRequired => write!(f, "Choose the branch runs should use."),
            Self::TimeZone(zone) => write!(f, "{zone} isn't a time zone this app knows."),
            Self::Schedule(reason) => write!(f, "The schedule isn't valid: {reason}"),
            Self::Retries(reason) => write!(f, "The retry policy isn't valid: {reason}."),
            Self::RetryDelay(reason) => write!(f, "The retry delay isn't valid: {reason}."),
            Self::Retention(reason) => write!(f, "The retention limit isn't valid: {reason}."),
            Self::MissedPolicy(reason) => write!(f, "The missed-run policy isn't valid: {reason}."),
            Self::OverlapPolicy(reason) => write!(f, "The overlap policy isn't valid: {reason}."),
        }
    }
}

impl std::error::Error for DefinitionError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub name: String,
    pub project_root: PathBuf,
    pub schedule: String,
    pub agent: String,
    pub prompt: String,
    pub workspace: WorkspaceMode,
    #[serde(default = "default_tags")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub continue_session: bool,
    #[serde(default)]
    pub service_writes: bool,
    #[serde(default)]
    pub watch: Option<Watch>,
}

impl RunSnapshot {
    pub fn is_session(&self) -> bool {
        self.project_root.as_os_str().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The next due time; at most one per automation.
    Scheduled,
    /// Due, waiting for a free slot.
    Queued,
    PreparingWorkspace,
    StartingAgent,
    /// The agent is running with the prompt: the run's job is done.
    Started,
    Failed,
    /// Never executed (missed its window, automation paused, …).
    Skipped,
    Cancelled,
}

impl RunStatus {
    pub const ALL: [RunStatus; 8] = [
        Self::Scheduled,
        Self::Queued,
        Self::PreparingWorkspace,
        Self::StartingAgent,
        Self::Started,
        Self::Failed,
        Self::Skipped,
        Self::Cancelled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Queued => "queued",
            Self::PreparingWorkspace => "preparing_workspace",
            Self::StartingAgent => "starting_agent",
            Self::Started => "started",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// Doing work right now.
    pub fn is_in_flight(self) -> bool {
        matches!(self, Self::PreparingWorkspace | Self::StartingAgent)
    }

    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Started | Self::Failed | Self::Skipped | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Schedule,
    Manual,
}

/// The step a run failed or was skipped at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStep {
    Queue,
    Workspace,
    Agent,
    Run,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    pub step: RunStep,
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
}

impl RunError {
    pub fn new(step: RunStep, code: impl Into<String>) -> Self {
        Self {
            step,
            code: code.into(),
            message: None,
        }
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

/// Reasons a run is skipped or fails without executing.
pub mod codes {
    /// It waited past the next occurrence (the app was closed, or every
    /// slot stayed busy), so the occurrence is dropped rather than doubled.
    pub const MISSED_WINDOW: &str = "missed_window";
    pub const AUTOMATION_PAUSED: &str = "automation_paused";
    pub const AUTOMATION_DELETED: &str = "automation_deleted";
    /// The previous run of the same automation was still starting.
    pub const ALREADY_RUNNING: &str = "already_running";
    /// The schedule changed after this occurrence was planned.
    pub const SCHEDULE_CHANGED: &str = "schedule_changed";
    pub const CANCELLED: &str = "cancelled";
    /// The app quit while the run was preparing or starting.
    pub const INTERRUPTED: &str = "interrupted";
    /// The session it would continue is waiting for a person: typing the
    /// prompt there could answer that agent's approval prompt.
    pub const PREVIOUS_WAITING: &str = "previous_waiting";
    /// Over the free plan's automation allowance.
    pub const PLAN_LIMIT: &str = "plan_limit";
    /// A watched search found nothing it hadn't found before.
    pub const NOTHING_NEW: &str = "nothing_new";
    /// A watching automation used up the day's runs; what's new waits.
    pub const WATCH_LIMIT: &str = "watch_limit";
    /// The watched service couldn't be searched.
    pub const WATCH_FAILED: &str = "watch_failed";
}

/// The terminal a run's agent was started in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTerminal {
    pub workspace_id: String,
    pub home_id: String,
    pub session_id: String,
    /// The pane identity the agent's conversation is recorded under, so a
    /// tab opened for the run resumes it after a reboot.
    #[serde(default)]
    pub pane_uuid: Option<String>,
}

/// What a started run's agent is doing, as far as its hooks tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    /// Waiting for a person: a permission or a question.
    NeedsInput,
    /// Finished its turn; waiting for a follow-up.
    Done,
    Failed,
    /// Its session closed before it reported how the turn went.
    Ended,
    /// Waited for a person too long and was stopped.
    TimedOut,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::NeedsInput => "needs_input",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Ended => "ended",
            Self::TimedOut => "timed_out",
        }
    }
}

/// How a started run's agent got on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    pub state: AgentState,
    /// When `state` was reached.
    pub at_ms: i64,
    /// The agent's last reply, when its provider reports one.
    #[serde(default)]
    pub reply: Option<String>,
    /// No longer followed: the agent's session ended, or a later run took
    /// the same session over.
    #[serde(default)]
    pub closed: bool,
    /// The run's own turn is over: later turns in its session are someone's
    /// follow-ups, not the run waiting on its own.
    #[serde(default)]
    pub settled: bool,
}

impl RunProgress {
    /// Whether `state` ends the run's own turn.
    pub fn settles(state: AgentState) -> bool {
        matches!(
            state,
            AgentState::Done | AgentState::Failed | AgentState::Ended | AgentState::TimedOut
        )
    }
}

/// A workspace a run put in the sidebar. The header marks it with the
/// automation that started it (the latest run, for a pinned workspace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunWorkspace {
    pub path: PathBuf,
    pub run_id: String,
    pub automation_id: String,
    /// The automation's name when the run started, kept for display once
    /// the automation is renamed or deleted.
    pub automation_name: String,
    /// When the run started (or was due).
    pub run_at_ms: Option<i64>,
}

/// Who wrote a version of an automation's prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    /// Edited in the app.
    Human,
    /// Written by an agent through the command line.
    Agent,
    /// An earlier version brought back.
    Restore,
}

impl PromptSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Restore => "restore",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "agent" => Self::Agent,
            "restore" => Self::Restore,
            _ => Self::Human,
        }
    }
}

/// One saved prompt, newest first in history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptVersion {
    pub id: i64,
    pub automation_id: String,
    pub prompt: String,
    pub source: PromptSource,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    /// Creation order, unique across all runs.
    pub seq: i64,
    pub automation_id: String,
    pub status: RunStatus,
    pub trigger: Trigger,
    pub snapshot: RunSnapshot,
    /// Short label for the run (and its branch).
    pub name: String,
    pub scheduled_at_ms: Option<i64>,
    /// After this the occurrence is stale and is skipped instead of run.
    pub deadline_at_ms: Option<i64>,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub workspace_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub terminal: Option<RunTerminal>,
    pub error: Option<RunError>,
    /// The prompt went to the agent the previous run left running.
    #[serde(default)]
    pub continued: bool,
    /// Something went wrong that didn't stop the run (e.g. the branch
    /// couldn't be published).
    #[serde(default)]
    pub warning: Option<String>,
    /// The pane the agent works in when it has no terminal of its own
    /// (Dragent's agent runs in a window pane).
    #[serde(default)]
    pub pane_uuid: Option<String>,
    /// How the agent got on after it took the prompt.
    #[serde(default)]
    pub progress: Option<RunProgress>,
    /// When someone last opened the run's result (the inbox's unread mark).
    #[serde(default)]
    pub read_at_ms: Option<i64>,
    /// Filed away from the inbox. `None` until someone decides: a reply
    /// saying there's nothing to report files it on its own.
    #[serde(default)]
    pub archived: Option<bool>,
    /// Which attempt this run is: 0 for the first try, 1 for the first
    /// retry, and so on. Old runs load as 0.
    #[serde(default)]
    pub attempt: u32,
    /// The failed run this run retries, if it is a retry.
    #[serde(default)]
    pub retry_of: Option<String>,
}

/// How an agent opens its final reply when a run found nothing that needs
/// anyone; such runs go straight to the inbox's archive.
pub const NOTHING_TO_REPORT: &str = "Nothing to report";

/// Whether `reply` says there's nothing to report.
pub fn says_nothing_to_report(reply: &str) -> bool {
    let start = reply.trim_start_matches(|c: char| c.is_whitespace() || "#>*_-`\"'“".contains(c));
    start
        .get(..NOTHING_TO_REPORT.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(NOTHING_TO_REPORT))
}

impl Run {
    /// When the run's result came in: what its agent last did, or when it
    /// failed to start. `None` while there's no result yet.
    pub fn result_at_ms(&self) -> Option<i64> {
        match self.status {
            RunStatus::Failed => self.finished_at_ms,
            RunStatus::Started => self
                .progress
                .as_ref()
                .filter(|progress| progress.state != AgentState::Working)
                .map(|progress| progress.at_ms),
            _ => None,
        }
    }

    /// In the inbox and not opened since its result came in.
    pub fn is_unread(&self) -> bool {
        self.result_at_ms()
            .is_some_and(|at| self.read_at_ms.map_or(true, |read| read < at))
    }

    /// Filed away: by someone, or because it had nothing to report.
    pub fn is_archived(&self) -> bool {
        self.archived.unwrap_or_else(|| {
            self.progress
                .as_ref()
                .and_then(|progress| progress.reply.as_deref())
                .is_some_and(says_nothing_to_report)
        })
    }

    /// When the run happened, or will.
    pub fn display_time_ms(&self) -> Option<i64> {
        self.started_at_ms
            .or(self.finished_at_ms)
            .or(self.scheduled_at_ms)
    }

    /// The pane the run's agent works in: its terminal's, or the window
    /// pane Dragent's agent took the prompt in.
    pub fn agent_pane(&self) -> Option<&str> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.pane_uuid.as_deref())
            .or(self.pane_uuid.as_deref())
    }

    /// Over for good: it failed, was skipped or cancelled, or started and
    /// its agent is no longer being followed (or never reported).
    pub fn is_settled(&self) -> bool {
        match self.status {
            RunStatus::Failed | RunStatus::Skipped | RunStatus::Cancelled => true,
            RunStatus::Started => self
                .progress
                .as_ref()
                .map_or(true, |progress| progress.settled || progress.closed),
            _ => false,
        }
    }

    /// Ready to be opened as a workspace.
    pub fn can_open(&self) -> bool {
        self.workspace_path.is_some()
            && matches!(
                self.status,
                RunStatus::Started | RunStatus::Failed | RunStatus::Cancelled
            )
    }
}

/// Run history filters, matching the counts shown beside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunFilter {
    All,
    Started,
    Failed,
    Skipped,
    Cancelled,
}

impl RunFilter {
    pub const ALL: [RunFilter; 5] = [
        Self::All,
        Self::Started,
        Self::Failed,
        Self::Skipped,
        Self::Cancelled,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Started => "Started",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
            Self::Cancelled => "Cancelled",
        }
    }

    /// History never lists the planned next occurrence; it has its own banner.
    pub fn matches(self, status: RunStatus) -> bool {
        match self {
            Self::All => status != RunStatus::Scheduled,
            Self::Started => status == RunStatus::Started,
            Self::Failed => status == RunStatus::Failed,
            Self::Skipped => status == RunStatus::Skipped,
            Self::Cancelled => status == RunStatus::Cancelled,
        }
    }
}

/// A lowercase, dash-separated form of `name` for branches and labels.
pub fn slug(name: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= max_len {
            // Stop between words: a name shouldn't end halfway through one.
            if ch.is_ascii_alphanumeric() {
                if let Some(end) = out.rfind('-') {
                    out.truncate(end);
                }
            }
            break;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "automation".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_occurrence_reads_the_automation_zone() {
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let mut automation = Automation::new(
            "Sweep",
            PathBuf::new(),
            "0 9 * * 1-5",
            "claude",
            "go",
            WorkspaceMode::default(),
            0,
        );
        automation.timezone = "UTC".into();
        // Friday after 9:00 → Monday 9:00.
        assert_eq!(
            automation.next_occurrence_ms(at("2026-10-02T10:00:00Z")),
            Some(at("2026-10-05T09:00:00Z"))
        );
        // 9:00 in Lagos is 8:00 UTC.
        automation.timezone = "Africa/Lagos".into();
        assert_eq!(
            automation.next_occurrence_ms(at("2026-10-05T07:00:00Z")),
            Some(at("2026-10-05T08:00:00Z"))
        );
        automation.schedule = "not a schedule".into();
        assert_eq!(automation.next_occurrence_ms(0), None);
    }

    #[test]
    fn statuses_round_trip_and_classify() {
        for status in RunStatus::ALL {
            assert_eq!(RunStatus::parse(status.as_str()), Some(status));
        }
        assert!(RunStatus::PreparingWorkspace.is_in_flight());
        assert!(!RunStatus::Queued.is_in_flight());
        assert!(RunStatus::Skipped.is_finished());
        assert!(!RunFilter::All.matches(RunStatus::Scheduled));
        assert!(RunFilter::All.matches(RunStatus::Queued));
    }

    #[test]
    fn validation_names_the_missing_piece() {
        let mut automation = Automation::new(
            "Nightly review",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "Review yesterday's changes",
            WorkspaceMode::default(),
            0,
        );
        assert!(automation.validate().is_ok());
        automation.schedule = "61 * * * *".into();
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::Schedule(_))
        ));
        automation.schedule = "0 9 * * *".into();
        automation.prompt = "  ".into();
        assert_eq!(automation.validate(), Err(DefinitionError::PromptRequired));
    }

    #[test]
    fn older_records_still_load() {
        // Saved before tags, time zones, and pinned workspaces existed.
        let record = r#"{"id":"a","name":"Nightly","project_root":"/repo","schedule":"0 9 * * *",
            "agent":"claude","prompt":"Go","workspace":{"kind":"project_checkout"},"enabled":true,
            "revision":1,"created_at_ms":0,"updated_at_ms":0}"#;
        let automation: Automation = serde_json::from_str(record).unwrap();
        assert_eq!(automation.workspace, WorkspaceMode::Pinned { path: None });
        assert_eq!(automation.tags, vec![DEFAULT_TAG.to_string()]);
        assert!(automation.timezone.is_empty() && automation.time_zone().is_none());
        let record = r#"{"kind":"new_worktree","base_branch":"main"}"#;
        assert_eq!(
            serde_json::from_str::<WorkspaceMode>(record).unwrap(),
            WorkspaceMode::NewWorktree {
                base_branch: Some("main".into()),
                push_branch: false
            }
        );
    }

    #[test]
    fn sessions_time_zones_and_branches_are_validated() {
        let mut automation = Automation::new(
            "Inbox digest",
            PathBuf::new(),
            "0 9 * * *",
            "claude",
            "Summarise",
            WorkspaceMode::default(),
            0,
        );
        // A session has no project to branch from.
        assert_eq!(automation.validate(), Err(DefinitionError::ProjectRequired));
        automation.workspace = WorkspaceMode::Pinned { path: None };
        assert!(automation.validate().is_ok());
        automation.timezone = "Africa/Lagos".into();
        assert_eq!(automation.time_zone(), Some(chrono_tz::Africa::Lagos));
        assert!(automation.validate().is_ok());
        automation.timezone = "Mars/Olympus".into();
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::TimeZone(_))
        ));
        automation.timezone.clear();
        automation.project_root = PathBuf::from("/repo");
        automation.workspace = WorkspaceMode::ExistingBranch { branch: " ".into() };
        assert_eq!(automation.validate(), Err(DefinitionError::BranchRequired));
        // Continuing a session only applies to a pinned workspace.
        automation.workspace = WorkspaceMode::default();
        automation.continue_session = true;
        assert!(!automation.snapshot().continue_session);
    }

    #[test]
    fn tags_normalize() {
        assert_eq!(
            normalize_tags([
                "Nightly ".into(),
                "nightly".into(),
                " ".into(),
                "Bugs".into()
            ]),
            vec!["bugs".to_string(), "nightly".to_string()]
        );
    }

    #[test]
    fn slugs_are_branch_safe() {
        assert_eq!(slug("  Nightly Review: API!  ", 40), "nightly-review-api");
        assert_eq!(slug("!!!", 40), "automation");
        assert_eq!(slug("abcdefghij", 4), "abcd");
    }

    #[test]
    fn replies_that_have_nothing_to_report() {
        assert!(says_nothing_to_report("Nothing to report."));
        assert!(says_nothing_to_report("## nothing to report\n\nAll green."));
        assert!(says_nothing_to_report("> **Nothing to report**"));
        assert!(!says_nothing_to_report(
            "Found 2 issues. Nothing to report otherwise."
        ));
        assert!(!says_nothing_to_report("Nothing"));
    }

    #[test]
    fn timezones_read_wall_clock_times() {
        let at = |text: &str| {
            chrono::DateTime::parse_from_rfc3339(text)
                .unwrap()
                .timestamp_millis()
        };
        let mut automation = Automation::new(
            "Sweep",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "go",
            WorkspaceMode::default(),
            0,
        );
        // 09:00 in New York (EDT, UTC-4) is 13:00 UTC.
        automation.timezone = "America/New_York".into();
        assert_eq!(
            automation.next_occurrence_ms(at("2026-10-01T08:00:00Z")),
            Some(at("2026-10-01T13:00:00Z"))
        );
        // 09:00 in Tokyo (JST, UTC+9) is midnight UTC: Tokyo has passed
        // 09:00 on the 1st already, so the next is the 2nd.
        automation.timezone = "Asia/Tokyo".into();
        assert_eq!(
            automation.next_occurrence_ms(at("2026-10-01T08:00:00Z")),
            Some(at("2026-10-02T00:00:00Z"))
        );
        // Empty means the machine's zone; an unknown zone fails validation.
        automation.timezone.clear();
        assert!(automation.validate().is_ok());
        automation.timezone = "Mars/Olympus".into();
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::TimeZone(_))
        ));
    }

    #[test]
    fn retry_backoff_doubles_and_stops_at_the_limit() {
        let mut automation = Automation::new(
            "Sweep",
            PathBuf::from("/repo"),
            "0 9 * * *",
            "claude",
            "go",
            WorkspaceMode::default(),
            0,
        );
        assert_eq!(
            automation.retry_delay_for_attempt(0),
            None,
            "no retries by default"
        );
        automation.max_retries = 3;
        automation.retry_delay_secs = 300;
        assert_eq!(automation.retry_delay_for_attempt(0), Some(300));
        assert_eq!(automation.retry_delay_for_attempt(1), Some(600));
        assert_eq!(automation.retry_delay_for_attempt(2), Some(1200));
        assert_eq!(automation.retry_delay_for_attempt(3), None, "bounded");
        automation.max_retries = MAX_RETRIES_LIMIT + 1;
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::Retries(_))
        ));
        automation.max_retries = 1;
        automation.retry_delay_secs = 5;
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::RetryDelay(_))
        ));
        automation.retry_delay_secs = 300;
        automation.run_retention = 0;
        assert!(matches!(
            automation.validate(),
            Err(DefinitionError::Retention(_))
        ));
    }

    #[test]
    fn policies_parse_and_round_trip() {
        assert_eq!(MissedPolicy::parse("skip"), Some(MissedPolicy::Skip));
        assert_eq!(MissedPolicy::parse("catch-up"), Some(MissedPolicy::CatchUp));
        assert_eq!(MissedPolicy::parse("run_once"), Some(MissedPolicy::CatchUp));
        assert_eq!(MissedPolicy::parse("bogus"), None);
        assert_eq!(OverlapPolicy::parse("queue"), Some(OverlapPolicy::Queue));
        assert_eq!(OverlapPolicy::parse("allow"), Some(OverlapPolicy::Allow));
        assert_eq!(OverlapPolicy::parse("bogus"), None);
        assert_eq!(MissedPolicy::Skip.as_str(), "skip");
        assert_eq!(OverlapPolicy::Allow.as_str(), "allow");
    }

    #[test]
    fn stored_automations_without_policies_load_with_defaults() {
        // Saved before missed/overlap/retry/retention policies existed: the
        // JSON defaults are the migration, keeping old behaviour (skip,
        // skip, no retries, keep 100 runs).
        let record = r#"{"id":"a","name":"Nightly","project_root":"/repo","schedule":"0 9 * * *",
            "agent":"claude","prompt":"Go","workspace":{"kind":"project_checkout"},"enabled":true,
            "revision":1,"created_at_ms":0,"updated_at_ms":0}"#;
        let automation: Automation = serde_json::from_str(record).unwrap();
        assert_eq!(automation.missed_policy, MissedPolicy::Skip);
        assert_eq!(automation.overlap_policy, OverlapPolicy::Skip);
        assert_eq!(automation.max_retries, 0);
        assert_eq!(automation.retry_delay_secs, 300);
        assert_eq!(automation.run_retention, 100);
        assert!(automation.validate().is_ok());
        // Old runs load with attempt 0 and no retry link.
        let run_record = r#"{"id":"r","seq":1,"automation_id":"a","status":"failed",
            "trigger":"manual","snapshot":{"name":"N","project_root":"/repo","schedule":"0 9 * * *",
            "agent":"claude","prompt":"Go","workspace":{"kind":"project_checkout"}},
            "name":"run","scheduled_at_ms":null,"deadline_at_ms":null,
            "started_at_ms":null,"finished_at_ms":1,"workspace_path":null,"branch":null,
            "terminal":null,"error":null}"#;
        let run: Run = serde_json::from_str(run_record).unwrap();
        assert_eq!(run.attempt, 0);
        assert_eq!(run.retry_of, None);
    }
}
