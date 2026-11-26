//! Teams: one lead agent coordinating worker agents on a piece of work.
//!
//! The lead (any agent CLI, Dragent's own included) plans the work as tasks
//! with dependencies, gives each to an agent and model, and talks to its
//! workers through a mailbox; workers report back the same way. Everything
//! lives here in the host, saved after every change, so a team keeps going
//! while the desktop app is closed and survives it crashing. The app's Team
//! tab, the `dragent team`/`tasks`/`msg` commands and `dragent mcp` are all
//! views of this one store.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
#[cfg(not(target_family = "wasm"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(not(target_family = "wasm"))]
use std::sync::{Condvar, Mutex, OnceLock};
#[cfg(not(target_family = "wasm"))]
use std::time::{Duration, Instant};

/// Tasks in one team, at most.
pub const MAX_TASKS: usize = 60;
/// Messages kept per team; the oldest read ones go first.
pub const MAX_MESSAGES: usize = 2_000;
/// Changes `wait` and the Team tab can catch up on.
const MAX_EVENTS: usize = 1_000;
/// The longest a `wait` blocks.
pub const MAX_WAIT_MS: u64 = 10 * 60 * 1_000;
/// Times a task goes back to its worker for a review's changes before the
/// lead (or the person) has to decide what's next.
pub const MAX_REVIEW_ROUNDS: u32 = 3;
/// Messages two workers may send each other in a row before the lead has
/// to weigh in: two agents never argue forever.
pub const MAX_PAIR_RUN: usize = 8;
/// Workers of one agent running at once, unless the team says otherwise:
/// several sessions on one subscription hit its rate limits.
pub const DEFAULT_MAX_PARALLEL: u32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cost {
    /// The cheapest model that can do each task.
    Cheapest,
    #[default]
    Balanced,
    /// The strongest models, cost second.
    Best,
}

/// An agent the team may use, and the models it may run (any when empty).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterAgent {
    /// `claude`, `codex`, `opencode`, `dragent`, …
    pub agent: String,
    #[serde(default)]
    pub models: Vec<String>,
}

/// A terminal an agent of the team works in: its session (`pane`) and its
/// host terminal. Commands find their team and role by it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub pane: Option<String>,
    #[serde(default)]
    pub terminal: Option<String>,
    /// The agent in it (`claude`, …).
    #[serde(default)]
    pub agent: Option<String>,
    /// The agent's own conversation id, which stays the same when the
    /// conversation is resumed in another pane: the seat follows it there.
    /// Learned from the session's record once the agent reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// When it took this seat, in Unix milliseconds: a move planned before
    /// then is stale and doesn't undo it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seated_ms: Option<i64>,
}

impl Member {
    fn is(&self, pane: Option<&str>, terminal: Option<&str>) -> bool {
        (pane.is_some() && self.pane.as_deref() == pane)
            || (terminal.is_some() && self.terminal.as_deref() == terminal)
    }

    /// The agent conversation `identity` sits in this seat: the same
    /// conversation of the same agent. Unknown on either side never matches.
    pub fn holds(&self, identity: &SessionIdentity) -> bool {
        self.conversation.as_deref() == Some(identity.conversation.as_str())
            && self.agent.as_deref() == Some(identity.agent.as_str())
    }
}

/// Which agent conversation runs in a session: what follows a member when
/// it's resumed in another pane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionIdentity {
    /// The agent's own conversation id.
    pub conversation: String,
    /// The agent's key (`claude`, `codex`, …).
    pub agent: String,
}

/// How a member came to a new pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReseatReason {
    /// Its conversation, resumed elsewhere, ran a team command.
    Contact,
    /// Dragent resumed its conversation in another pane.
    Resume,
    /// `team lead --take`.
    Taken,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Work,
    /// An independent review of another task's result.
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Planned; starts once its dependencies are done.
    Pending,
    /// Its worker is being set up (checkout, agent).
    Starting,
    Running,
    /// Its worker needs a decision, access or another task first.
    Blocked,
    Done,
    /// It could not be finished: its worker didn't start, stopped or gave up.
    Failed,
    /// It stopped for a decision rather than an error: its review still asks
    /// for changes after the rounds it gets. The work is kept; the lead (or
    /// the person) decides whether it goes back, the plan changes, or the
    /// person is asked.
    NeedsDecision,
    Cancelled,
}

impl Task {
    /// A QA pass of everything integrated into the person's checkout,
    /// rather than of one task's branch.
    pub fn tests_integrated(&self) -> bool {
        self.qa && self.review_of.is_none()
    }

    /// Finished work whose latest changes are in the person's checkout.
    pub fn integrated_up_to_date(&self) -> bool {
        self.kind == TaskKind::Work
            && self.status == TaskStatus::Done
            && self.integrated.as_ref().is_some_and(|integration| {
                self.finished_at_ms.is_none_or(|finished| integration.at_ms >= finished)
            })
    }

    /// Files its result lists that lie outside the area it owns.
    pub fn outside_area(&self) -> Vec<String> {
        if self.owns.is_empty() {
            return Vec::new();
        }
        self.result
            .iter()
            .flat_map(|result| &result.files)
            .filter(|file| !self.owns.iter().any(|area| in_area(area, file)))
            .cloned()
            .collect()
    }

    /// Where it stands, finer than its status for finished work:
    /// implemented, then approved by its review, passed QA, integrated.
    pub fn stage(&self, team: &Team) -> &'static str {
        if self.kind != TaskKind::Work || self.status != TaskStatus::Done {
            return match (self.status, &self.result) {
                (TaskStatus::Done, Some(result)) if result.verdict == Some(Verdict::Changes) => {
                    "changes_asked"
                }
                (TaskStatus::Pending, Some(result)) if result.verdict == Some(Verdict::Changes) => {
                    "changes_asked"
                }
                (status, _) => status.key(),
            };
        }
        if self.integrated_up_to_date() {
            return "integrated";
        }
        let passed = |review: &Task| {
            review.status == TaskStatus::Done
                && review.review_of.as_deref() == Some(self.id.as_str())
                && review
                    .result
                    .as_ref()
                    .is_some_and(|result| result.verdict == Some(Verdict::Approve))
        };
        if team.tasks.iter().any(|review| review.qa && passed(review)) {
            "qa_passed"
        } else if team.tasks.iter().any(|review| !review.qa && passed(review)) {
            "review_approved"
        } else {
            "implemented"
        }
    }

    /// A worker is on it, or about to be.
    pub fn under_way(&self) -> bool {
        matches!(
            self.status,
            TaskStatus::Starting | TaskStatus::Running | TaskStatus::Blocked
        )
    }

    /// Tasks after this one may start: it's done (and, for a review,
    /// approved), or it was taken out of the plan.
    pub fn lets_dependents_start(&self) -> bool {
        match self.status {
            TaskStatus::Done => self
                .result
                .as_ref()
                .is_none_or(|result| result.verdict != Some(Verdict::Changes)),
            TaskStatus::Cancelled => true,
            _ => false,
        }
    }
}

impl TaskStatus {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::NeedsDecision | Self::Cancelled
        )
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::NeedsDecision => "needs_decision",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Whether `file` (a path from the project's root) lies in `area`: that
/// file, a folder holding it, or a pattern where `*` stands for any run of
/// characters.
pub fn in_area(area: &str, file: &str) -> bool {
    let area = area.trim().trim_start_matches("./");
    if area.is_empty() {
        return false;
    }
    if area.contains('*') {
        return wildcard(area.as_bytes(), file.as_bytes());
    }
    let area = area.trim_end_matches('/');
    file == area || file.strip_prefix(area).is_some_and(|rest| rest.starts_with('/'))
}

fn wildcard(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| wildcard(rest, &text[skip..])),
        Some((first, rest)) => text
            .split_first()
            .is_some_and(|(head, tail)| head == first && wildcard(rest, tail)),
    }
}

/// What a worker reported when it finished.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskResult {
    /// One or two plain sentences for someone who hasn't read the code: what
    /// now works.
    pub summary: String,
    /// Technical notes for the next developer: versions, commits, decisions.
    #[serde(default)]
    pub details: Option<String>,
    /// Files it changed.
    #[serde(default)]
    pub files: Vec<String>,
    /// The checks it ran and how they went.
    #[serde(default)]
    pub checks: Vec<String>,
    /// A review's conclusion (reviews only).
    #[serde(default)]
    pub verdict: Option<Verdict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Ready as it is.
    Approve,
    /// It goes back to its worker with the review's findings.
    Changes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// `t1`, `t2`, … in the order they were planned.
    pub id: String,
    pub title: String,
    /// What the worker is asked to do.
    pub prompt: String,
    pub kind: TaskKind,
    /// For a review: the task it reviews (also in `depends_on`).
    #[serde(default)]
    pub review_of: Option<String>,
    /// A review that's the QA pass: it tests the whole feature the
    /// reviewed task holds, as a tester would, not just its code.
    #[serde(default)]
    pub qa: bool,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// The agent and model chosen for it; the lead's choice by default.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_locked: bool,
    /// User's choice for the next attempt. None with model_locked means agent default.
    #[serde(default)]
    pub user_model: Option<String>,
    /// Model used when the current/last worker was launched.
    #[serde(default)]
    pub attempt_model: Option<String>,
    pub status: TaskStatus,
    /// Its checkout, once started.
    #[serde(default)]
    pub workspace: Option<String>,
    /// The branch it works on there.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worker: Option<Member>,
    #[serde(default)]
    pub result: Option<TaskResult>,
    /// Why it's blocked or failed.
    #[serde(default)]
    pub reason: Option<String>,
    /// Times it was started.
    #[serde(default)]
    pub attempts: u32,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    #[serde(default)]
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub finished_at_ms: Option<i64>,
    /// The files and folders this work may change (`app/src/teams/`,
    /// `docs/*.md`); anything when empty. Changes elsewhere are flagged
    /// when it finishes.
    #[serde(default)]
    pub owns: Vec<String>,
    /// The commit its branch started from: a snapshot of the person's
    /// checkout (uncommitted changes included), or the tip of the work it
    /// builds on. Its own changes are everything since.
    #[serde(default)]
    pub base: Option<String>,
    /// When its changes were last brought into the person's checkout.
    #[serde(default)]
    pub integrated: Option<Integration>,
}

/// Work brought into the person's checkout as uncommitted changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integration {
    /// The commit of the task's branch that was integrated; the next
    /// integration brings what came after it.
    pub commit: String,
    pub at_ms: i64,
    /// Files it changed in the person's checkout, over every integration.
    #[serde(default)]
    pub files: Vec<String>,
    /// What the integrator ran to check it, and how that went.
    #[serde(default)]
    pub checks: Vec<String>,
}

/// Who a message is from or for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Party {
    Lead,
    /// The person the team works for.
    User,
    /// The worker on a task.
    Task(String),
    /// Dragent itself: notes the team's courier writes (a worker waiting
    /// for an approval, one that went quiet). Nobody writes to it.
    Dragent,
}

impl Party {
    /// `lead`, `user` or a task id, as the commands write it.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "lead" => Some(Self::Lead),
            "user" | "me" | "you" => Some(Self::User),
            "dragent" => Some(Self::Dragent),
            "" => None,
            id => Some(Self::Task(id.to_string())),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Lead => "lead".into(),
            Self::User => "user".into(),
            Self::Task(id) => id.clone(),
            Self::Dragent => "dragent".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    #[default]
    Note,
    Question,
    Answer,
    /// Work or context handed to another task.
    Handoff,
    /// Something that affects the plan.
    Finding,
    /// The lead needs the person: a decision, access, an approval.
    NeedsUser,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub from: Party,
    pub to: Party,
    pub kind: MessageKind,
    pub body: String,
    #[serde(default)]
    pub reply_to: Option<u64>,
    pub at_ms: i64,
    /// Read by its recipient (`inbox` marks it). The person's messages to
    /// the lead count as read only once they went into the lead's session:
    /// a `wait` can hand them to a script that never shows them to the lead.
    #[serde(default)]
    pub read: bool,
    /// Answers the person can pick with one click (a question to them):
    /// the one picked goes back as the answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

impl Message {
    /// The person writing to the lead: it goes straight into the lead's
    /// session, and only that makes it read.
    pub fn goes_in_directly(&self) -> bool {
        self.from == Party::User && self.to == Party::Lead
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    /// The lead is planning; nothing has started.
    #[default]
    Planning,
    Running,
    Done,
    Stopped,
}

/// One change, for `wait` and the Team tab to catch up on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamEvent {
    pub revision: u64,
    pub at_ms: i64,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Team {
        status: TeamStatus,
    },
    TaskAdded {
        task: String,
    },
    Task {
        task: String,
        status: TaskStatus,
    },
    TaskEdited {
        task: String,
    },
    Message {
        id: u64,
        to: Party,
        /// Who wrote it (absent in records from before this was kept).
        #[serde(default)]
        from: Option<Party>,
    },
    /// Review `review` asked for changes to `task`, which went back to its
    /// worker (or failed, after too many rounds).
    ChangesAsked {
        review: String,
        task: String,
    },
    /// Every task finished, but nothing tested the whole feature yet: the
    /// work isn't done until a QA pass has.
    QaNeeded,
    /// `task`'s changes were brought into the person's checkout.
    Integrated {
        task: String,
    },
    /// The lead answered a permission prompt in `task`'s terminal: what it
    /// asked (the end of the screen) and the keys pressed.
    PromptAnswered {
        task: String,
        keys: Vec<String>,
        prompt: String,
    },
    /// These agents joined the team while it ran.
    AgentsAdded {
        agents: Vec<String>,
    },
    /// `task` moved from one agent to another, keeping its checkout.
    Reassigned {
        task: String,
        #[serde(default)]
        from: Option<String>,
        to: String,
    },
    /// `party`'s agent moved to another pane (its conversation resumed
    /// there, or it took its seat back): the team reaches it there now.
    Reseated {
        party: Party,
        #[serde(default)]
        pane: Option<String>,
        reason: ReseatReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    /// `team-` and eight hex digits.
    pub id: String,
    pub title: String,
    pub project_root: String,
    pub status: TeamStatus,
    #[serde(default)]
    pub lead: Option<Member>,
    /// The model the lead runs on, as the person chose it (`None`: its
    /// agent's own choice).
    #[serde(default)]
    pub lead_model: Option<String>,
    /// The lead's account of the run for the person: what now works, what's
    /// left, and what to do next.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Agents the team may use; any installed one when empty.
    #[serde(default)]
    pub roster: Vec<RosterAgent>,
    #[serde(default)]
    pub cost: Cost,
    #[serde(default = "default_max_parallel")]
    pub max_parallel_per_agent: u32,
    /// Once the lead has set the plan going, a task starts on its own when
    /// what it waited for is done (the person may turn this off).
    #[serde(default = "starts_on_its_own")]
    pub auto_start: bool,
    /// The lead may answer workers' permission prompts, case by case
    /// (the person may turn this off and answer them all themselves).
    #[serde(default = "lead_answers")]
    pub lead_answers_prompts: bool,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub events: Vec<TeamEvent>,
    /// Bumped by every change.
    #[serde(default)]
    pub revision: u64,
    /// The revision each party's last `wait` saw, by `Party::label`, so a
    /// wait without `since` picks up where the previous one stopped.
    #[serde(default)]
    pub cursors: HashMap<String, u64>,
    /// What the courier already typed into each party's session, by
    /// `Party::label` (see `team_courier`).
    #[serde(default)]
    pub courier: HashMap<String, CourierMark>,
    #[serde(default)]
    pub next_task: u32,
    #[serde(default)]
    pub next_message: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What the courier did for one party, so it neither repeats itself nor
/// stays quiet after a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourierMark {
    /// News up to this revision was typed into the party's session.
    #[serde(default)]
    pub typed_revision: u64,
    /// Reminders typed into a worker that went quiet unfinished.
    #[serde(default)]
    pub nudges: u32,
    #[serde(default)]
    pub last_typed_ms: i64,
    /// The approval prompt (by its activity time) already reported.
    #[serde(default)]
    pub permission_reported: u64,
    /// The lead was told this worker stopped without finishing.
    #[serde(default)]
    pub stall_reported: bool,
    /// What this worker held when the lead was last told it was paused.
    #[serde(default)]
    pub pause_reported: u64,
    /// When a wake was last requested for this party's sleeping agent.
    #[serde(default)]
    pub wake_requested_ms: i64,
    /// When news was last typed into the lead, reminders included.
    #[serde(default)]
    pub news_typed_ms: i64,
    /// Reminders typed since news was last typed in full.
    #[serde(default)]
    pub reminders: u32,
    /// The person was told the lead isn't picking up team news.
    #[serde(default)]
    pub person_told: bool,
    /// When the lead last read its news through its inbox or by taking it
    /// over (a `tasks wait` moves its cursor instead).
    #[serde(default)]
    pub heard_ms: i64,
    /// When the first wake still unanswered was requested (0: none). A
    /// wake unanswered too long means the agent's tab is gone.
    #[serde(default)]
    pub wake_pending_ms: i64,
    /// The wake the agent last answered: nothing is typed into it until it
    /// shows a sign of life after that (it may still be starting).
    #[serde(default)]
    pub woke_ms: i64,
    /// The person was told the lead couldn't be woken.
    #[serde(default)]
    pub wake_told: bool,
}

/// The agent key of Dragent's own agent, which leads from inside the app.
pub const DRAGENT_AGENT: &str = "dragent";

fn starts_on_its_own() -> bool {
    true
}

fn lead_answers() -> bool {
    true
}

fn default_max_parallel() -> u32 {
    DEFAULT_MAX_PARALLEL
}

impl Team {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    /// Pending tasks that wait for nothing.
    pub fn ready_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Pending && self.waiting_on(task).is_empty())
            .collect()
    }

    /// What `task` waits for before it can start: the tasks it depends on
    /// that haven't let it go yet, and reviews still open on work it
    /// builds on. It starts from that work, so it waits for the verdict:
    /// a fix asked for later would land under it.
    pub fn waiting_on(&self, task: &Task) -> Vec<String> {
        let mut waiting: Vec<String> = task
            .depends_on
            .iter()
            .filter(|dependency| {
                self.task(dependency)
                    .is_none_or(|dependency| !dependency.lets_dependents_start())
            })
            .cloned()
            .collect();
        for review in &self.tasks {
            let on_its_base = review.kind == TaskKind::Review
                && review.id != task.id
                && review.review_of.as_ref().is_some_and(|target| {
                    task.depends_on.contains(target) && task.review_of.as_ref() != Some(target)
                });
            if on_its_base
                && !matches!(review.status, TaskStatus::Done | TaskStatus::Cancelled)
                // A review planned after this task can't hold it back.
                && !self.leads_to(review, &task.id)
                && !waiting.contains(&review.id)
            {
                waiting.push(review.id.clone());
            }
        }
        let to_integrate = |work: &Task| match work.status {
            TaskStatus::Done => format!("{} to be integrated", work.id),
            _ => work.id.clone(),
        };
        // Work on several finished tasks starts from the person's checkout:
        // they have to be in it.
        let built_on: Vec<&Task> = task
            .depends_on
            .iter()
            .filter_map(|id| self.task(id))
            .filter(|work| work.kind == TaskKind::Work && work.status != TaskStatus::Cancelled)
            .collect();
        if task.kind == TaskKind::Work && task.workspace.is_none() && built_on.len() > 1 {
            for work in built_on {
                let entry = to_integrate(work);
                if work.status == TaskStatus::Done
                    && !work.integrated_up_to_date()
                    && !waiting.contains(&entry)
                {
                    waiting.push(entry);
                }
            }
        }
        if task.tests_integrated() {
            // It tests the person's checkout: every piece of work has to be
            // in it, as finished.
            for work in &self.tasks {
                let entry = to_integrate(work);
                if work.kind == TaskKind::Work
                    && work.status != TaskStatus::Cancelled
                    && !work.integrated_up_to_date()
                    && !waiting.contains(&entry)
                    && !waiting.contains(&work.id)
                {
                    waiting.push(entry);
                }
            }
            // After it found problems, it tests again once a fix is in.
            let found_problems = task
                .result
                .as_ref()
                .is_some_and(|result| result.verdict == Some(Verdict::Changes));
            let fixed_since = self.tasks.iter().any(|work| {
                work.integrated.as_ref().is_some_and(|integration| {
                    task.started_at_ms.is_none_or(|started| integration.at_ms > started)
                })
            });
            if found_problems && !fixed_since && waiting.is_empty() {
                waiting.push("an integrated fix".into());
            }
        }
        waiting
    }

    /// How many messages workers `a` and `b` have sent each other since the
    /// lead last wrote to either of them.
    fn pair_run(&self, a: &str, b: &str) -> usize {
        let (a, b) = (Party::Task(a.into()), Party::Task(b.into()));
        self.messages
            .iter()
            .rev()
            .take_while(|message| {
                !(message.from == Party::Lead && (message.to == a || message.to == b))
            })
            .filter(|message| {
                (message.from == a && message.to == b) || (message.from == b && message.to == a)
            })
            .count()
    }

    /// Whether `task` depends on task `id`, directly or through others.
    fn leads_to(&self, task: &Task, id: &str) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut next: Vec<&str> = task.depends_on.iter().map(String::as_str).collect();
        while let Some(current) = next.pop() {
            if current == id {
                return true;
            }
            if let Some(found) = self.task(current).filter(|_| seen.insert(current)) {
                next.extend(found.depends_on.iter().map(String::as_str));
            }
        }
        false
    }

    /// Unread messages for `party`.
    pub fn unread(&self, party: &Party) -> usize {
        self.messages
            .iter()
            .filter(|message| &message.to == party && !message.read)
            .count()
    }

    pub fn summary(&self) -> TeamSummary {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for task in &self.tasks {
            *counts.entry(task.status.key()).or_default() += 1;
        }
        TeamSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            project_root: self.project_root.clone(),
            status: self.status,
            tasks: self.tasks.len(),
            done: counts.get("done").copied().unwrap_or(0),
            running: counts.get("running").copied().unwrap_or(0)
                + counts.get("starting").copied().unwrap_or(0),
            blocked: counts.get("blocked").copied().unwrap_or(0),
            needs_user: self.unread(&Party::User),
            updated_at_ms: self.updated_at_ms,
            worker_panes: if matches!(self.status, TeamStatus::Planning | TeamStatus::Running) {
                self.tasks
                    .iter()
                    .filter_map(|task| task.worker.as_ref()?.pane.clone())
                    .collect()
            } else {
                Vec::new()
            },
            dragent_lead: self
                .lead
                .as_ref()
                .filter(|lead| lead.agent.as_deref() == Some(DRAGENT_AGENT))
                .filter(|_| matches!(self.status, TeamStatus::Planning | TeamStatus::Running))
                .and_then(|lead| lead.pane.clone()),
            lead_model: self.lead_model.clone(),
        }
    }

    fn change(&mut self, change: Change) {
        let now = now_ms();
        self.revision += 1;
        self.updated_at_ms = now;
        self.events.push(TeamEvent {
            revision: self.revision,
            at_ms: now,
            change,
        });
        if self.events.len() > MAX_EVENTS {
            let excess = self.events.len() - MAX_EVENTS;
            self.events.drain(..excess);
        }
    }

    fn task_mut(&mut self, id: &str) -> Result<&mut Task, String> {
        self.tasks
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| format!("This team has no task “{id}”."))
    }

    fn check_agent(&self, agent: Option<&str>, model: Option<&str>) -> Result<(), String> {
        let Some(agent) = agent else {
            return match model {
                Some(_) => Err("Say which agent the model is for (--agent).".into()),
                None => Ok(()),
            };
        };
        if self.roster.is_empty() {
            return Ok(());
        }
        let Some(entry) = self.roster.iter().find(|entry| entry.agent == agent) else {
            let allowed = self
                .roster
                .iter()
                .map(|entry| entry.agent.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "“{agent}” isn't one of this team's agents ({allowed}). If the person asked for it, add it: \
                 `dragent team agents add {agent}`."
            ));
        };
        match model {
            Some(model) if !entry.models.is_empty() && !entry.models.iter().any(|m| m == model) => {
                Err(format!(
                    "{agent} can run {} here, not “{model}”.",
                    entry.models.join(", ")
                ))
            }
            _ => Ok(()),
        }
    }

    /// Dependencies that exist, aren't the task itself, and add no cycle.
    fn check_dependencies(&self, id: &str, depends_on: &[String]) -> Result<(), String> {
        for dependency in depends_on {
            if dependency == id {
                return Err(format!("Task {id} can't depend on itself."));
            }
            if self.task(dependency).is_none() {
                return Err(format!("There's no task “{dependency}” to depend on."));
            }
        }
        // A cycle through `id` means `id` is reachable from its dependencies.
        let mut seen = HashSet::new();
        let mut stack: Vec<&str> = depends_on.iter().map(String::as_str).collect();
        while let Some(current) = stack.pop() {
            if current == id {
                return Err(format!("That would make task {id} wait on itself."));
            }
            if seen.insert(current)
                && let Some(task) = self.task(current)
            {
                stack.extend(task.depends_on.iter().map(String::as_str));
            }
        }
        Ok(())
    }

    /// Work was finished but no QA pass has tested it (one the lead
    /// cancelled counts: that was their call).
    fn needs_qa(&self) -> bool {
        self.tasks
            .iter()
            .any(|task| task.kind == TaskKind::Work && task.status == TaskStatus::Done)
            && !self.tasks.iter().any(|task| {
                task.qa && matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled)
            })
    }

    /// The lead was told a QA pass is missing since the plan last moved.
    fn qa_asked(&self) -> bool {
        self.events
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event.change,
                    Change::QaNeeded | Change::Task { .. } | Change::TaskAdded { .. }
                )
            })
            .is_some_and(|event| event.change == Change::QaNeeded)
    }

    pub fn running_with(&self, agent: &str) -> usize {
        self.tasks
            .iter()
            .filter(|task| {
                matches!(task.status, TaskStatus::Starting | TaskStatus::Running)
                    && task.agent.as_deref() == Some(agent)
            })
            .count()
    }

    fn settle_status(&mut self) {
        let finished =
            !self.tasks.is_empty() && self.tasks.iter().all(|task| task.status.is_finished());
        let next = if self.status == TeamStatus::Stopped {
            return;
        } else if finished && self.needs_qa() {
            // Built, but not tested as a whole: say so once.
            if !self.qa_asked() {
                self.change(Change::QaNeeded);
            }
            TeamStatus::Running
        } else if finished {
            TeamStatus::Done
        } else if self
            .tasks
            .iter()
            .any(|task| task.status != TaskStatus::Pending || task.attempts > 0)
        {
            TeamStatus::Running
        } else {
            TeamStatus::Planning
        };
        if next != self.status {
            self.status = next;
            self.change(Change::Team { status: next });
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSummary {
    pub id: String,
    pub title: String,
    pub project_root: String,
    pub status: TeamStatus,
    pub tasks: usize,
    pub done: usize,
    pub running: usize,
    pub blocked: usize,
    /// Messages for the person they haven't read.
    pub needs_user: usize,
    pub updated_at_ms: i64,
    /// Sessions of its workers while the team is at work (the app keeps
    /// their routine alerts quiet: the lead reports on them).
    #[serde(default)]
    pub worker_panes: Vec<String>,
    /// The session of its lead when that's Dragent's own agent, while the
    /// team is at work: the app brings it its news.
    #[serde(default)]
    pub dragent_lead: Option<String>,
    /// The model its lead runs on, as the person chose it.
    #[serde(default)]
    pub lead_model: Option<String>,
}

/// Fields a task can change before it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEdit {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    /// `Some(None)` clears it.
    #[serde(default)]
    pub agent: Option<Option<String>>,
    #[serde(default)]
    pub model: Option<Option<String>>,
    #[serde(default)]
    pub owns: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TeamRequest {
    Create {
        title: String,
        project_root: String,
        #[serde(default)]
        roster: Vec<RosterAgent>,
        #[serde(default)]
        cost: Cost,
        #[serde(default)]
        max_parallel_per_agent: Option<u32>,
        #[serde(default)]
        lead: Option<Member>,
    },
    List {
        #[serde(default)]
        project_root: Option<String>,
    },
    Get {
        team: String,
    },
    /// Which team and role the terminal `pane`/`terminal` belongs to.
    /// When nothing knows that terminal but the conversation running there
    /// holds a seat (it was resumed in a new pane), the seat moves to it.
    Resolve {
        #[serde(default)]
        pane: Option<String>,
        #[serde(default)]
        terminal: Option<String>,
        /// The conversation in that terminal. The host fills it in from
        /// the session's record (whatever a client sends is replaced).
        #[serde(default)]
        identity: Option<SessionIdentity>,
    },
    /// Dragent resumed `identity` in `pane`: every seat its conversation
    /// holds moves there, unless one was taken after `at_ms`.
    Reseat {
        pane: String,
        #[serde(default)]
        terminal: Option<String>,
        identity: SessionIdentity,
        /// When it was resumed (now when absent).
        #[serde(default)]
        at_ms: Option<i64>,
    },
    /// The session `lead` takes the lead's seat back (`team lead --take`);
    /// the caller has been checked. The lead's model is kept.
    TakeLead {
        team: String,
        lead: Member,
    },
    SetLead {
        team: String,
        lead: Member,
        /// The model it runs on, as the person chose it.
        #[serde(default)]
        model: Option<String>,
    },
    /// The person moved the lead to another model.
    SetLeadModel {
        team: String,
        #[serde(default)]
        model: Option<String>,
    },
    Stop {
        team: String,
    },
    /// Take a finished or stopped team off the list (its record goes).
    Forget {
        team: String,
    },
    /// The person's choices for a team: how to weigh cost, and how many
    /// workers of one agent run at once.
    Configure {
        team: String,
        #[serde(default)]
        cost: Option<Cost>,
        #[serde(default)]
        max_parallel_per_agent: Option<u32>,
        /// Whether ready tasks start on their own.
        #[serde(default)]
        auto_start: Option<bool>,
        #[serde(default)]
        lead_answers_prompts: Option<bool>,
    },
    /// More agents the team may use, while it runs.
    AddAgents {
        team: String,
        agents: Vec<RosterAgent>,
    },
    /// Hand a task that's under way to another agent, in the same checkout.
    ReassignTask {
        team: String,
        task: String,
        agent: String,
        #[serde(default)]
        model: Option<String>,
        /// What the next worker must know.
        note: String,
    },
    AddTask {
        team: String,
        title: String,
        prompt: String,
        #[serde(default)]
        depends_on: Vec<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        model: Option<String>,
        /// Makes it a review of that task.
        #[serde(default)]
        review_of: Option<String>,
        /// With `review_of`: the QA pass, testing the whole feature that
        /// task holds.
        #[serde(default)]
        qa: bool,
        #[serde(default)]
        owns: Vec<String>,
    },
    SetTaskModel {
        team: String,
        task: String,
        model: Option<String>,
    },
    EditTask {
        team: String,
        task: String,
        edit: TaskEdit,
    },
    /// Reserve a ready task for starting: its dependencies are done and
    /// its agent has room. The caller then sets up the checkout and agent
    /// and reports `TaskStarted` (or `FailTask`).
    BeginTask {
        team: String,
        task: String,
    },
    TaskStarted {
        team: String,
        task: String,
        workspace: String,
        #[serde(default)]
        branch: Option<String>,
        worker: Member,
        /// The commit its branch starts from (kept from its first start).
        #[serde(default)]
        base: Option<String>,
    },
    /// The lead answered a permission prompt in `task`'s terminal.
    PromptAnswered {
        team: String,
        task: String,
        keys: Vec<String>,
        prompt: String,
    },
    /// The task's changes, up to `integration.commit`, are now in the
    /// person's checkout.
    Integrated {
        team: String,
        task: String,
        integration: Integration,
    },
    FinishTask {
        team: String,
        task: String,
        result: TaskResult,
    },
    /// A short name for the run, in place of the request it started from.
    SetTitle {
        team: String,
        title: String,
    },
    /// The lead's account of the whole run, shown at the top of its page.
    SetOutcome {
        team: String,
        outcome: String,
    },
    BlockTask {
        team: String,
        task: String,
        reason: String,
    },
    /// A blocked task's worker carries on.
    UnblockTask {
        team: String,
        task: String,
    },
    FailTask {
        team: String,
        task: String,
        reason: String,
    },
    CancelTask {
        team: String,
        task: String,
    },
    /// Back to pending with feedback, for another attempt (after a review).
    ReopenTask {
        team: String,
        task: String,
        feedback: String,
    },
    Send {
        team: String,
        from: Party,
        to: Party,
        #[serde(default)]
        kind: MessageKind,
        body: String,
        #[serde(default)]
        reply_to: Option<u64>,
        /// One-click answers for the person (a question to them).
        #[serde(default)]
        choices: Vec<String>,
    },
    /// `party`'s messages, unread ones only unless `all`; marks them read
    /// unless `peek`.
    Inbox {
        team: String,
        party: Party,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        peek: bool,
    },
    /// Block until something after `since` concerns the caller: one of
    /// `tasks` (any task when empty) finishing, failing or blocking, or a
    /// message for `party`. Returns at once when that already happened.
    /// Without `since`, from where `party`'s last wait stopped.
    Wait {
        team: String,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        tasks: Vec<String>,
        #[serde(default)]
        party: Option<Party>,
        timeout_ms: u64,
    },
    /// Every change after `since`, for the Team tab.
    Changes {
        team: String,
        since: u64,
    },
    /// The news a lead the app serves (Dragent's own agent) hasn't had
    /// yet, as one message; handing it over counts as delivered.
    TakeLeadNews {
        /// It's working: only the person's messages go in now, and it queues
        /// them; the rest waits for the end of its turn.
        #[serde(default)]
        busy: bool,
        team: String,
    },
    /// A worker's first prompt carried these messages to its task (they're
    /// read now, so the courier doesn't type them a second time).
    PromptDelivered {
        team: String,
        task: String,
        messages: Vec<u64>,
    },
}

/// A message to `task`, which can't read it now: it stays for the task (it
/// reads it once it's running again, or once the lead reopens it), the
/// sender is told where it went, and the lead gets a copy when the task is
/// finished, so it can reopen the task if that's needed.
fn route_undelivered(team: &mut Team, message: &Message, task: &str, status: TaskStatus) {
    let label = match status {
        TaskStatus::Pending | TaskStatus::Starting => {
            post_message(
                team,
                Party::Dragent,
                message.from.clone(),
                MessageKind::Note,
                format!("{task} hasn't started yet, so your message will be delivered when it starts."),
            );
            return;
        }
        TaskStatus::Done => "finished",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::NeedsDecision => "waiting for a decision",
        _ => return,
    };
    if message.from == Party::Lead {
        let reopen = if status == TaskStatus::Cancelled {
            String::new()
        } else {
            format!(" Reopen it (`dragent tasks reopen {task} --feedback …`) if it should see this.")
        };
        post_message(
            team,
            Party::Dragent,
            Party::Lead,
            MessageKind::Note,
            format!("{task} is {label}, so it won't see this.{reopen}"),
        );
        return;
    }
    post_message(
        team,
        Party::Dragent,
        message.from.clone(),
        MessageKind::Note,
        format!("{task} is {label}, so it won't see this; your message went to the lead."),
    );
    post_message(
        team,
        message.from.clone(),
        Party::Lead,
        message.kind,
        format!(
            "To {task} ({label}, so it won't see this): {}",
            message.body.trim()
        ),
    );
}

/// Most one-click answers a question to the person offers, and their length.
pub const MAX_CHOICES: usize = 4;
pub const MAX_CHOICE_CHARS: usize = 40;

/// Record a message Dragent or a party writes itself (no checks, no reply).
fn post_message(team: &mut Team, from: Party, to: Party, kind: MessageKind, body: String) {
    team.next_message += 1;
    let id = team.next_message;
    team.change(Change::Message {
        id,
        to: to.clone(),
        from: Some(from.clone()),
    });
    team.messages.push(Message {
        id,
        from,
        to,
        kind,
        body,
        reply_to: None,
        at_ms: now_ms(),
        read: false,
        choices: Vec::new(),
    });
}

impl TeamRequest {
    /// The team a request changes (empty for a new one), `None` for reads.
    pub fn changes(&self) -> Option<String> {
        match self {
            Self::Create { .. } => Some(String::new()),
            Self::List { .. }
            | Self::Get { .. }
            | Self::Resolve { .. }
            // Seats it moves are announced with `take_reseated`.
            | Self::Reseat { .. }
            | Self::Wait { .. }
            | Self::Changes { .. } => None,
            // Marking messages read changes nothing anyone watches, and
            // nor does handing a lead its news.
            Self::Inbox { .. } | Self::TakeLeadNews { .. } => None,
            Self::SetLead { team, .. }
            | Self::TakeLead { team, .. }
            | Self::SetLeadModel { team, .. }
            | Self::Stop { team }
            | Self::Forget { team }
            | Self::Configure { team, .. }
            | Self::AddAgents { team, .. }
            | Self::ReassignTask { team, .. }
            | Self::AddTask { team, .. }
            | Self::SetTaskModel { team, .. }
            | Self::EditTask { team, .. }
            | Self::BeginTask { team, .. }
            | Self::TaskStarted { team, .. }
            | Self::FinishTask { team, .. }
            | Self::SetOutcome { team, .. }
            | Self::SetTitle { team, .. }
            | Self::BlockTask { team, .. }
            | Self::UnblockTask { team, .. }
            | Self::FailTask { team, .. }
            | Self::CancelTask { team, .. }
            | Self::ReopenTask { team, .. }
            | Self::Integrated { team, .. }
            | Self::PromptAnswered { team, .. }
            | Self::PromptDelivered { team, .. }
            | Self::Send { team, .. } => Some(team.clone()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TeamResponse {
    Team(Box<Team>),
    Teams(Vec<TeamSummary>),
    Task(Box<Task>),
    Message(Box<Message>),
    Messages(Vec<Message>),
    Resolved(Option<(String, Party)>),
    /// Seats a `Reseat` moved: each team and whose seat.
    Reseated(Vec<(String, Party)>),
    /// A lead's news, when there is some.
    News(Option<String>),
    /// What happened after `since`, and the revision to wait from next.
    Changes {
        revision: u64,
        events: Vec<TeamEvent>,
        timed_out: bool,
    },
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The longest each kind of text may be, in characters: a team's record
/// stays small, and long material belongs in a file a task points to.
fn limit(what: &str) -> usize {
    match what {
        "team's goal" | "task's title" => 300,
        "project folder" => 4_096,
        "task's instructions" | "message" => 20_000,
        _ => 10_000,
    }
}

/// A task's area as given: paths from the project's root, each once.
fn areas(owns: Vec<String>) -> Result<Vec<String>, String> {
    let mut areas: Vec<String> = Vec::new();
    for area in owns {
        let area = area.trim().trim_start_matches("./").to_string();
        if area.is_empty() || areas.contains(&area) {
            continue;
        }
        if area.starts_with('/') || area.split('/').any(|part| part == "..") {
            return Err(format!(
                "“{area}” isn't a path inside the project: name areas from its root, like app/src/teams/."
            ));
        }
        areas.push(area);
    }
    if areas.len() > 50 {
        return Err("A task owns at most 50 paths: name folders instead.".into());
    }
    Ok(areas)
}

fn required(text: &str, what: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("Give the {what}."));
    }
    let (length, max) = (text.chars().count(), limit(what));
    if length > max {
        return Err(format!(
            "The {what} is too long: {length} characters, at most {max}. Put long material in a file in the project and point to it."
        ));
    }
    Ok(text.to_string())
}

/// All teams of this organisation, saved one file each.
/// Teams saved before `NeedsDecision` existed marked work whose review ran
/// out of rounds as failed; show those as waiting for a decision instead.
fn relabel_review_limits(team: &mut Team) {
    for task in &mut team.tasks {
        if task.status == TaskStatus::Failed
            && task.kind == TaskKind::Work
            && task.reason.as_deref().is_some_and(|reason| {
                reason.starts_with("Review ") && reason.contains(" still asks for changes after ")
            })
        {
            task.status = TaskStatus::NeedsDecision;
        }
    }
}

/// The member in `party`'s seat: the lead, or a task's worker.
fn seat_of<'a>(team: &'a Team, party: &Party) -> Option<&'a Member> {
    match party {
        Party::Lead => team.lead.as_ref(),
        Party::Task(id) => team.task(id)?.worker.as_ref(),
        Party::User | Party::Dragent => None,
    }
}

fn seat_of_mut<'a>(team: &'a mut Team, party: &Party) -> Option<&'a mut Member> {
    match party {
        Party::Lead => team.lead.as_mut(),
        Party::Task(id) => team
            .tasks
            .iter_mut()
            .find(|task| task.id == *id)?
            .worker
            .as_mut(),
        Party::User | Party::Dragent => None,
    }
}

pub struct TeamStore {
    pub(crate) dir: PathBuf,
    pub(crate) teams: HashMap<String, Team>,
    /// Teams whose seats moved by a request that changes nothing else
    /// (`Resolve`, `Reseat`), for the host to announce.
    pub(crate) reseated: Vec<String>,
}

impl TeamStore {
    /// Load every team saved in `dir`.
    pub fn open(dir: PathBuf) -> Self {
        let mut teams = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match std::fs::read(&path)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        serde_json::from_slice::<Team>(&bytes).map_err(|error| error.to_string())
                    }) {
                    Ok(mut team) => {
                        relabel_review_limits(&mut team);
                        teams.insert(team.id.clone(), team);
                    }
                    Err(error) => log::warn!("[team] couldn't load {}: {error}", path.display()),
                }
            }
        }
        Self {
            dir,
            teams,
            reseated: Vec::new(),
        }
    }

    fn save(&self, team: &Team) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec_pretty(team).map_err(|error| error.to_string())?;
        let partial = self.dir.join(format!(
            "{}.json.{}",
            team.id,
            uuid::Uuid::new_v4().simple()
        ));
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&partial)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&partial, self.dir.join(format!("{}.json", team.id)))
        };
        write().map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            format!("Couldn't save the team: {error}")
        })
    }

    pub fn team(&self, id: &str) -> Result<&Team, String> {
        self.teams
            .get(id)
            .ok_or_else(|| format!("There's no team “{id}”."))
    }

    /// Change team `id` and save it; nothing is kept when `change` fails
    /// or the save does.
    pub(crate) fn update<T>(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Team) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut team = self.team(id)?.clone();
        let output = change(&mut team)?;
        team.settle_status();
        self.save(&team)?;
        self.teams.insert(team.id.clone(), team);
        Ok(output)
    }

    /// The team and role of the terminal `pane`/`terminal`: the unfinished
    /// team that knows it, else the most recently active one.
    fn resolve(&self, pane: Option<&str>, terminal: Option<&str>) -> Option<(String, Party)> {
        let mut teams: Vec<&Team> = self.teams.values().collect();
        teams.sort_by_key(|team| {
            (
                matches!(team.status, TeamStatus::Done | TeamStatus::Stopped),
                std::cmp::Reverse(team.updated_at_ms),
            )
        });
        teams.into_iter().find_map(|team| {
            if team
                .lead
                .as_ref()
                .is_some_and(|lead| lead.is(pane, terminal))
            {
                return Some((team.id.clone(), Party::Lead));
            }
            // A worker stays one after its task is done (to answer
            // follow-ups), but its unfinished task comes first.
            let mut worked: Vec<&Task> = team
                .tasks
                .iter()
                .filter(|task| {
                    task.worker
                        .as_ref()
                        .is_some_and(|worker| worker.is(pane, terminal))
                })
                .collect();
            worked.sort_by_key(|task| {
                (
                    task.status.is_finished(),
                    std::cmp::Reverse(task.updated_at_ms),
                )
            });
            worked
                .first()
                .map(|task| (team.id.clone(), Party::Task(task.id.clone())))
        })
    }

    /// `party`'s seat in `team` learns the conversation running in it
    /// (`identity`, read from the session in `pane`), when it doesn't know
    /// one yet. A seat's conversation is never replaced: the conversation
    /// it was given to is what may follow it elsewhere.
    pub(crate) fn learn_conversation(
        &mut self,
        team: &str,
        party: &Party,
        pane: Option<&str>,
        identity: &SessionIdentity,
    ) -> bool {
        let fits = |member: &Member| {
            member.conversation.is_none()
                && pane.is_none_or(|pane| member.pane.as_deref() == Some(pane))
                && member
                    .agent
                    .as_deref()
                    .is_none_or(|agent| agent == identity.agent)
        };
        let Ok(current) = self.team(team) else {
            return false;
        };
        if !seat_of(current, party).is_some_and(fits) {
            return false;
        }
        self.update(team, |team| {
            let member = seat_of_mut(team, party).ok_or("That seat is gone.")?;
            if !fits(member) {
                return Ok(false);
            }
            member.conversation = Some(identity.conversation.clone());
            member.agent.get_or_insert_with(|| identity.agent.clone());
            Ok(true)
        })
        .unwrap_or(false)
    }

    /// Move every seat `identity`'s conversation holds to `pane`/`terminal`
    /// (where it runs now), each recorded as a change on its team. A seat
    /// taken after `at_ms` stays: this move is older than it. Returns the
    /// seats moved.
    pub(crate) fn reseat(
        &mut self,
        pane: Option<&str>,
        terminal: Option<&str>,
        identity: &SessionIdentity,
        at_ms: i64,
        reason: ReseatReason,
    ) -> Result<Vec<(String, Party)>, String> {
        if pane.is_none() && terminal.is_none() {
            return Ok(Vec::new());
        }
        let due = |member: &Member| {
            member.holds(identity)
                && (member.pane.as_deref() != pane || member.terminal.as_deref() != terminal)
                && member.seated_ms.unwrap_or(0) <= at_ms
        };
        let mut ids: Vec<String> = self
            .teams
            .values()
            .filter(|team| {
                team.lead.as_ref().is_some_and(due)
                    || team
                        .tasks
                        .iter()
                        .any(|task| task.worker.as_ref().is_some_and(due))
            })
            .map(|team| team.id.clone())
            .collect();
        ids.sort();
        let mut moved = Vec::new();
        for id in ids {
            let seats = self.update(&id, |team| {
                let mut parties: Vec<Party> = Vec::new();
                if team.lead.as_ref().is_some_and(due) {
                    parties.push(Party::Lead);
                }
                parties.extend(
                    team.tasks
                        .iter()
                        .filter(|task| task.worker.as_ref().is_some_and(due))
                        .map(|task| Party::Task(task.id.clone())),
                );
                for party in &parties {
                    let member = seat_of_mut(team, party).expect("found above");
                    let new_pane = member.pane.as_deref() != pane;
                    member.pane = pane.map(str::to_owned);
                    member.terminal = terminal.map(str::to_owned);
                    member.seated_ms = Some(at_ms);
                    // The same pane in a new terminal (a restart) isn't news.
                    if new_pane {
                        team.change(Change::Reseated {
                            party: party.clone(),
                            pane: pane.map(str::to_owned),
                            reason,
                        });
                    }
                }
                Ok(parties)
            })?;
            if !seats.is_empty() {
                log::info!("[team] {id}: {} moved to pane {pane:?}", seats.iter().map(Party::label).collect::<Vec<_>>().join(", "));
                self.reseated.push(id.clone());
            }
            moved.extend(seats.into_iter().map(|party| (id.clone(), party)));
        }
        Ok(moved)
    }

    pub fn execute(&mut self, request: TeamRequest) -> Result<TeamResponse, String> {
        match request {
            TeamRequest::Create {
                title,
                project_root,
                roster,
                cost,
                max_parallel_per_agent,
                lead,
            } => {
                let now = now_ms();
                let mut team = Team {
                    id: format!("team-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
                    title: required(&title, "team's goal")?,
                    project_root: required(&project_root, "project folder")?,
                    status: TeamStatus::Planning,
                    lead,
                    lead_model: None,
                    outcome: None,
                    auto_start: true,
                    lead_answers_prompts: true,
                    roster,
                    cost,
                    max_parallel_per_agent: max_parallel_per_agent
                        .unwrap_or(DEFAULT_MAX_PARALLEL)
                        .max(1),
                    tasks: Vec::new(),
                    messages: Vec::new(),
                    events: Vec::new(),
                    revision: 0,
                    cursors: HashMap::new(),
                    courier: HashMap::new(),
                    next_task: 0,
                    next_message: 0,
                    created_at_ms: now,
                    updated_at_ms: now,
                };
                team.change(Change::Team {
                    status: TeamStatus::Planning,
                });
                self.save(&team)?;
                self.teams.insert(team.id.clone(), team.clone());
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::List { project_root } => {
                let mut teams: Vec<TeamSummary> = self
                    .teams
                    .values()
                    .filter(|team| {
                        project_root
                            .as_deref()
                            .is_none_or(|root| team.project_root == root)
                    })
                    .map(Team::summary)
                    .collect();
                teams.sort_by_key(|team| std::cmp::Reverse(team.updated_at_ms));
                Ok(TeamResponse::Teams(teams))
            }
            TeamRequest::Get { team } => {
                Ok(TeamResponse::Team(Box::new(self.team(&team)?.clone())))
            }
            TeamRequest::Resolve {
                pane,
                terminal,
                identity,
            } => {
                if pane.is_none() && terminal.is_none() {
                    return Ok(TeamResponse::Resolved(None));
                }
                let found = self.resolve(pane.as_deref(), terminal.as_deref());
                let Some(identity) = identity else {
                    return Ok(TeamResponse::Resolved(found));
                };
                if let Some((team, party)) = &found {
                    // Known by its terminal: learn its conversation, if the
                    // seat doesn't know one yet.
                    self.learn_conversation(team, party, pane.as_deref(), &identity);
                    return Ok(TeamResponse::Resolved(found));
                }
                // Its conversation was resumed here: the seat follows it.
                let moved = self.reseat(
                    pane.as_deref(),
                    terminal.as_deref(),
                    &identity,
                    now_ms(),
                    ReseatReason::Contact,
                )?;
                if moved.is_empty() {
                    return Ok(TeamResponse::Resolved(None));
                }
                Ok(TeamResponse::Resolved(
                    self.resolve(pane.as_deref(), terminal.as_deref()),
                ))
            }
            TeamRequest::Reseat {
                pane,
                terminal,
                identity,
                at_ms,
            } => {
                let moved = self.reseat(
                    Some(&pane),
                    terminal.as_deref(),
                    &identity,
                    at_ms.unwrap_or_else(now_ms),
                    ReseatReason::Resume,
                )?;
                Ok(TeamResponse::Reseated(moved))
            }
            TeamRequest::TakeLead { team, lead } => {
                let team = self.update(&team, |team| {
                    let pane = lead.pane.clone();
                    let mut lead = lead;
                    lead.seated_ms = Some(now_ms());
                    team.lead = Some(lead);
                    team.change(Change::Reseated {
                        party: Party::Lead,
                        pane,
                        reason: ReseatReason::Taken,
                    });
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::SetLead { team, lead, model } => {
                let team = self.update(&team, |team| {
                    let mut lead = lead;
                    lead.seated_ms.get_or_insert_with(now_ms);
                    team.lead = Some(lead);
                    team.lead_model = model;
                    team.change(Change::Team {
                        status: team.status,
                    });
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::SetLeadModel { team, model } => {
                let team = self.update(&team, |team| {
                    team.lead_model = model.filter(|model| !model.trim().is_empty());
                    team.change(Change::Team {
                        status: team.status,
                    });
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::Stop { team } => {
                let team = self.update(&team, |team| {
                    team.status = TeamStatus::Stopped;
                    let unfinished: Vec<String> = team
                        .tasks
                        .iter()
                        .filter(|task| !task.status.is_finished())
                        .map(|task| task.id.clone())
                        .collect();
                    for id in unfinished {
                        let task = team.task_mut(&id)?;
                        task.status = TaskStatus::Cancelled;
                        task.updated_at_ms = now_ms();
                        task.finished_at_ms = Some(now_ms());
                        team.change(Change::Task {
                            task: id,
                            status: TaskStatus::Cancelled,
                        });
                    }
                    team.change(Change::Team {
                        status: TeamStatus::Stopped,
                    });
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::Forget { team } => {
                let current = self.team(&team)?;
                if !matches!(current.status, TeamStatus::Done | TeamStatus::Stopped) {
                    return Err(
                        "Only a finished or stopped team can be removed: stop it first.".into(),
                    );
                }
                match std::fs::remove_file(self.dir.join(format!("{team}.json"))) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(format!("Couldn't remove the team: {error}"));
                    }
                    _ => {}
                }
                let removed = self
                    .teams
                    .remove(&team)
                    .ok_or("That team is gone already.")?;
                Ok(TeamResponse::Team(Box::new(removed)))
            }
            TeamRequest::Configure {
                team,
                cost,
                max_parallel_per_agent,
                auto_start,
                lead_answers_prompts,
            } => {
                let team = self.update(&team, |team| {
                    if let Some(cost) = cost {
                        team.cost = cost;
                    }
                    if let Some(on) = auto_start {
                        team.auto_start = on;
                    }
                    if let Some(on) = lead_answers_prompts {
                        team.lead_answers_prompts = on;
                    }
                    if let Some(limit) = max_parallel_per_agent {
                        team.max_parallel_per_agent = limit.clamp(1, 10);
                    }
                    team.change(Change::Team {
                        status: team.status,
                    });
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::AddAgents { team, agents } => {
                let team = self.update(&team, |team| {
                    if team.status == TeamStatus::Stopped {
                        return Err("This team was stopped.".into());
                    }
                    let mut added = Vec::new();
                    for agent in agents {
                        match team
                            .roster
                            .iter_mut()
                            .find(|entry| entry.agent == agent.agent)
                        {
                            // Models widen an entry that names some; one
                            // with none already allows every model.
                            Some(entry) if !entry.models.is_empty() => {
                                if agent.models.is_empty() {
                                    entry.models.clear();
                                }
                                for model in agent.models {
                                    if !entry.models.contains(&model) {
                                        entry.models.push(model);
                                    }
                                }
                            }
                            Some(_) => {}
                            None => {
                                added.push(agent.agent.clone());
                                team.roster.push(agent);
                            }
                        }
                    }
                    if !added.is_empty() {
                        team.change(Change::AgentsAdded { agents: added });
                    }
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::ReassignTask {
                team,
                task,
                agent,
                model,
                note,
            } => {
                let note = required(&note, "note for the next worker")?;
                let moved = self.update(&team, |team| {
                    let current = team
                        .task(&task)
                        .ok_or_else(|| format!("This team has no task “{task}”."))?
                        .clone();
                    if team.status == TeamStatus::Stopped {
                        return Err("This team was stopped.".into());
                    }
                    match current.status {
                        TaskStatus::Running | TaskStatus::Blocked => {}
                        TaskStatus::Pending => {
                            return Err(format!(
                                "Task {task} hasn't started: change its agent with `dragent tasks edit {task} --agent {agent}`."
                            ));
                        }
                        TaskStatus::Starting => {
                            return Err(format!(
                                "Task {task}'s worker is still starting. Try again in a moment."
                            ));
                        }
                        status => {
                            return Err(format!(
                                "Task {task} is {}, so no worker is on it to replace.",
                                status.key()
                            ));
                        }
                    }
                    if current.model_locked
                        && (Some(&agent) != current.agent.as_ref()
                            || model != current.user_model)
                    {
                        return Err(
                            "The person chose this task's model. Ask them to change it.".into()
                        );
                    }
                    team.check_agent(Some(&agent), model.as_deref())?;
                    let now = now_ms();
                    let moved = team.task_mut(&task)?;
                    moved.agent = Some(agent.clone());
                    moved.model = model;
                    moved.status = TaskStatus::Pending;
                    moved.reason = Some(note);
                    // Its old worker is out of the team from here on.
                    moved.worker = None;
                    moved.updated_at_ms = now;
                    let moved = moved.clone();
                    team.courier.remove(&task);
                    team.change(Change::Task {
                        task: task.clone(),
                        status: TaskStatus::Pending,
                    });
                    team.change(Change::Reassigned {
                        task: task.clone(),
                        from: current.agent,
                        to: agent,
                    });
                    Ok(moved)
                })?;
                Ok(TeamResponse::Task(Box::new(moved)))
            }
            TeamRequest::AddTask {
                team,
                title,
                prompt,
                mut depends_on,
                agent,
                model,
                review_of,
                qa,
                owns,
            } => {
                let task = self.update(&team, |team| {
                    if team.status == TeamStatus::Stopped {
                        return Err("This team was stopped.".into());
                    }
                    if team.tasks.len() >= MAX_TASKS {
                        return Err(format!("A team holds at most {MAX_TASKS} tasks."));
                    }
                    let title = required(&title, "task's title")?;
                    if team
                        .tasks
                        .iter()
                        .any(|task| task.title.eq_ignore_ascii_case(&title))
                    {
                        return Err(format!("There's already a task called “{title}”."));
                    }
                    let prompt = required(&prompt, "task's instructions")?;
                    let id = format!("t{}", team.next_task + 1);
                    let kind = match &review_of {
                        Some(target) => {
                            let target_task = team
                                .task(target)
                                .ok_or_else(|| format!("There's no task “{target}” to review."))?;
                            if target_task.kind == TaskKind::Review {
                                return Err(format!(
                                    "{target} is itself a review; review the work it reviews."
                                ));
                            }
                            if !depends_on.contains(target) {
                                depends_on.push(target.clone());
                            }
                            TaskKind::Review
                        }
                        // A QA pass of the integrated result.
                        None if qa => TaskKind::Review,
                        None => TaskKind::Work,
                    };
                    depends_on.dedup();
                    team.check_dependencies(&id, &depends_on)?;
                    team.check_agent(agent.as_deref(), model.as_deref())?;
                    let now = now_ms();
                    let task = Task {
                        id: id.clone(),
                        title,
                        prompt,
                        kind,
                        review_of,
                        qa,
                        depends_on,
                        agent,
                        model,
                        model_locked: false,
                        user_model: None,
                        attempt_model: None,
                        status: TaskStatus::Pending,
                        workspace: None,
                        branch: None,
                        worker: None,
                        result: None,
                        reason: None,
                        attempts: 0,
                        created_at_ms: now,
                        updated_at_ms: now,
                        started_at_ms: None,
                        finished_at_ms: None,
                        owns: areas(owns)?,
                        base: None,
                        integrated: None,
                    };
                    team.next_task += 1;
                    team.tasks.push(task.clone());
                    team.change(Change::TaskAdded { task: id });
                    Ok(task)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::SetTaskModel { team, task, model } => {
                let task = self.update(&team, |team| {
                    let current = team.task(&task).ok_or_else(|| format!("No task {task}"))?;
                    team.check_agent(current.agent.as_deref(), model.as_deref())?;
                    let task_record = team.task_mut(&task)?;
                    task_record.user_model = model.clone();
                    task_record.model_locked = true;
                    if task_record.status == TaskStatus::Pending {
                        task_record.model = model;
                    }
                    task_record.updated_at_ms = now_ms();
                    let changed = task_record.clone();
                    team.change(Change::TaskEdited { task });
                    Ok(changed)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::EditTask { team, task, edit } => {
                let task = self.update(&team, |team| {
                    let current = team
                        .task(&task)
                        .ok_or_else(|| format!("This team has no task “{task}”."))?
                        .clone();
                    if current.status != TaskStatus::Pending {
                        return Err(format!(
                            "Task {task} has already started, so only planned tasks change this way. To move it \
                             to another agent, keeping its work: `dragent tasks reassign {task} --agent KEY --note \"…\"`."
                        ));
                    }
                    let agent = edit.agent.clone().unwrap_or(current.agent.clone());
                    let model = edit.model.clone().unwrap_or(current.model.clone());
                    if current.model_locked
                        && (agent != current.agent || model != current.user_model)
                    {
                        return Err(
                            "The person chose this task's model. Ask them to change it.".into()
                        );
                    }
                    team.check_agent(agent.as_deref(), model.as_deref())?;
                    if let Some(depends_on) = &edit.depends_on {
                        let mut depends_on = depends_on.clone();
                        if let Some(target) = &current.review_of
                            && !depends_on.contains(target)
                        {
                            depends_on.push(target.clone());
                        }
                        team.check_dependencies(&task, &depends_on)?;
                    }
                    let edited = team.task_mut(&task)?;
                    if let Some(title) = &edit.title {
                        edited.title = required(title, "task's title")?;
                    }
                    if let Some(prompt) = &edit.prompt {
                        edited.prompt = required(prompt, "task's instructions")?;
                    }
                    if let Some(depends_on) = edit.depends_on {
                        edited.depends_on = depends_on;
                        if let Some(target) = edited.review_of.clone()
                            && !edited.depends_on.contains(&target)
                        {
                            edited.depends_on.push(target);
                        }
                    }
                    if let Some(owns) = edit.owns {
                        edited.owns = areas(owns)?;
                    }
                    edited.agent = agent;
                    edited.model = model;
                    edited.updated_at_ms = now_ms();
                    let edited = edited.clone();
                    team.change(Change::TaskEdited { task: task.clone() });
                    Ok(edited)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::BeginTask { team, task } => {
                let task = self.update(&team, |team| {
                    let pending = team.task_mut(&task)?;
                    if pending.status == TaskStatus::Pending && pending.model_locked {
                        pending.model = pending.user_model.clone();
                    }
                    if team.status == TeamStatus::Stopped {
                        return Err("This team was stopped.".into());
                    }
                    let current = team
                        .task(&task)
                        .ok_or_else(|| format!("This team has no task “{task}”."))?
                        .clone();
                    if current.status != TaskStatus::Pending {
                        return Err(format!("Task {task} is {}, not waiting to start.", current.status.key()));
                    }
                    let waiting = team.waiting_on(&current);
                    if !waiting.is_empty() {
                        return Err(format!(
                            "Task {task} waits for {} to finish first.",
                            waiting.join(", ")
                        ));
                    }
                    let agent = current
                        .agent
                        .clone()
                        .ok_or_else(|| format!("Choose an agent for task {task} first."))?;
                    if team.running_with(&agent) >= team.max_parallel_per_agent as usize {
                        return Err(format!(
                            "{} {agent} workers are already running, this team's limit for one agent. Start it when one finishes.",
                            team.max_parallel_per_agent
                        ));
                    }
                    let started = team.task_mut(&task)?;
                    started.status = TaskStatus::Starting;
                    started.attempts += 1;
                    started.attempt_model = started.model.clone();
                    started.reason = None;
                    started.updated_at_ms = now_ms();
                    let started = started.clone();
                    team.change(Change::Task {
                        task: task.clone(),
                        status: TaskStatus::Starting,
                    });
                    Ok(started)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::TaskStarted {
                team,
                task,
                workspace,
                branch,
                worker,
                base,
            } => {
                let started = self.transition(
                    &team,
                    &task,
                    &[TaskStatus::Starting],
                    TaskStatus::Running,
                    |task| {
                        task.workspace = Some(workspace);
                        task.branch = branch.or(task.branch.take());
                        task.worker = Some(worker);
                        task.started_at_ms = Some(now_ms());
                        if task.base.is_none() {
                            task.base = base;
                        }
                    },
                )?;
                // A new attempt: the courier starts over with its worker.
                self.update(&team, |team| {
                    team.courier.remove(&task);
                    Ok(())
                })?;
                Ok(TeamResponse::Task(Box::new(started)))
            }
            TeamRequest::PromptAnswered {
                team,
                task,
                keys,
                prompt,
            } => {
                let task = self.update(&team, |team| {
                    let answered = team.task(&task).ok_or_else(|| format!("No task {task}"))?.clone();
                    let mut prompt = prompt.trim().to_string();
                    if prompt.chars().count() > 1_500 {
                        prompt = prompt.chars().rev().take(1_500).collect::<Vec<_>>().into_iter().rev().collect();
                    }
                    team.change(Change::PromptAnswered {
                        task: task.clone(),
                        keys,
                        prompt,
                    });
                    Ok(answered)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::Integrated {
                team,
                task,
                integration,
            } => {
                let task = self.update(&team, |team| {
                    let integrated = team.task_mut(&task)?;
                    if integrated.kind != TaskKind::Work || integrated.status != TaskStatus::Done {
                        return Err(format!(
                            "Only finished work is integrated; {task} is {}.",
                            integrated.status.key()
                        ));
                    }
                    let mut integration = integration;
                    if let Some(earlier) = integrated.integrated.take() {
                        for file in earlier.files {
                            if !integration.files.contains(&file) {
                                integration.files.push(file);
                            }
                        }
                    }
                    integration.files.sort();
                    integrated.integrated = Some(integration);
                    integrated.updated_at_ms = now_ms();
                    let integrated = integrated.clone();
                    team.change(Change::Integrated { task: task.clone() });
                    Ok(integrated)
                })?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::SetTitle { team, title } => {
                let title = required(&title, "team's goal")?;
                let team = self.update(&team, |team| {
                    team.title = title;
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::SetOutcome { team, outcome } => {
                let outcome = required(&outcome, "summary")?;
                let team = self.update(&team, |team| {
                    team.outcome = Some(outcome);
                    Ok(team.clone())
                })?;
                Ok(TeamResponse::Team(Box::new(team)))
            }
            TeamRequest::FinishTask {
                team,
                task,
                mut result,
            } => {
                if result.summary.trim().is_empty() {
                    return Err("Say what was done and how it was checked.".into());
                }
                result.summary = required(&result.summary, "summary")?;
                if result.files.len() > 500 || result.checks.len() > 100 {
                    return Err("List at most 500 files and 100 checks; summarise the rest.".into());
                }
                let kind = self
                    .team(&team)?
                    .task(&task)
                    .ok_or_else(|| format!("This team has no task “{task}”."))?
                    .kind;
                match (kind, result.verdict) {
                    (TaskKind::Review, None) => {
                        return Err("A review ends with a verdict: approve it, or ask for changes and say what must change.".into());
                    }
                    (TaskKind::Work, Some(_)) => {
                        return Err(format!("Only reviews have a verdict; {task} is work."));
                    }
                    (TaskKind::Review, Some(Verdict::Changes)) => {
                        let review = self.changes_asked(&team, &task, result)?;
                        return Ok(TeamResponse::Task(Box::new(review)));
                    }
                    _ => {}
                }
                let task = self.transition(
                    &team,
                    &task,
                    &[TaskStatus::Running, TaskStatus::Blocked],
                    TaskStatus::Done,
                    |task| {
                        task.result = Some(result);
                        task.reason = None;
                        task.finished_at_ms = Some(now_ms());
                    },
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::BlockTask { team, task, reason } => {
                let reason = required(&reason, "reason it's blocked")?;
                let task = self.transition(
                    &team,
                    &task,
                    &[TaskStatus::Running],
                    TaskStatus::Blocked,
                    |task| {
                        task.reason = Some(reason);
                    },
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::UnblockTask { team, task } => {
                let task = self.transition(
                    &team,
                    &task,
                    &[TaskStatus::Blocked],
                    TaskStatus::Running,
                    |task| {
                        task.reason = None;
                    },
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::FailTask { team, task, reason } => {
                let reason = required(&reason, "reason it failed")?;
                let task = self.transition(
                    &team,
                    &task,
                    &[
                        TaskStatus::Starting,
                        TaskStatus::Running,
                        TaskStatus::Blocked,
                    ],
                    TaskStatus::Failed,
                    |task| {
                        task.reason = Some(reason);
                        task.finished_at_ms = Some(now_ms());
                    },
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::CancelTask { team, task } => {
                let task = self.transition(
                    &team,
                    &task,
                    &[
                        TaskStatus::Pending,
                        TaskStatus::Starting,
                        TaskStatus::Running,
                        TaskStatus::Blocked,
                    ],
                    TaskStatus::Cancelled,
                    |task| task.finished_at_ms = Some(now_ms()),
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::ReopenTask {
                team,
                task,
                feedback,
            } => {
                let feedback = required(&feedback, "feedback for the next attempt")?;
                let task = self.transition(
                    &team,
                    &task,
                    &[
                        TaskStatus::Done,
                        TaskStatus::Failed,
                        TaskStatus::NeedsDecision,
                        TaskStatus::Blocked,
                    ],
                    TaskStatus::Pending,
                    |task| {
                        if task.model_locked {
                            task.model = task.user_model.clone();
                        }
                        task.reason = Some(feedback);
                        task.result = None;
                        task.finished_at_ms = None;
                    },
                )?;
                Ok(TeamResponse::Task(Box::new(task)))
            }
            TeamRequest::Send {
                team,
                from,
                to,
                kind,
                body,
                reply_to,
                choices,
            } => {
                let body = required(&body, "message")?;
                // One-click answers are for the person, a few short words each.
                let choices: Vec<String> = if to == Party::User {
                    choices
                        .iter()
                        .map(|choice| choice.trim().chars().take(MAX_CHOICE_CHARS).collect::<String>())
                        .filter(|choice| !choice.is_empty())
                        .take(MAX_CHOICES)
                        .collect()
                } else {
                    Vec::new()
                };
                let message = self.update(&team, |team| {
                    for party in [&from, &to] {
                        if let Party::Task(id) = party
                            && team.task(id).is_none()
                        {
                            return Err(format!("This team has no task “{id}”."));
                        }
                    }
                    if from == to {
                        return Err("A message needs someone else to go to.".into());
                    }
                    if to == Party::Dragent {
                        return Err(
                            "Dragent doesn't read messages: write to lead, user or a task.".into(),
                        );
                    }
                    if let Some(reply_to) = reply_to
                        && !team.messages.iter().any(|message| message.id == reply_to)
                    {
                        return Err(format!("There's no message {reply_to} to reply to."));
                    }
                    if let (Party::Task(a), Party::Task(b)) = (&from, &to)
                        && team.pair_run(a, b) >= MAX_PAIR_RUN
                    {
                        return Err(format!(
                            "{a} and {b} have written to each other {MAX_PAIR_RUN} times in a row. \
                             Ask the lead to settle it: send it to lead, as a question."
                        ));
                    }
                    team.next_message += 1;
                    let message = Message {
                        id: team.next_message,
                        from,
                        to: to.clone(),
                        kind,
                        body,
                        reply_to,
                        at_ms: now_ms(),
                        read: false,
                        choices,
                    };
                    // The lead or the person answering a blocked worker
                    // lets it carry on.
                    if let (Party::Lead | Party::User, Party::Task(id)) = (&message.from, &to) {
                        let task = team.task_mut(id)?;
                        if task.status == TaskStatus::Blocked {
                            task.status = TaskStatus::Running;
                            task.reason = None;
                            task.updated_at_ms = now_ms();
                            let id = id.clone();
                            team.change(Change::Task {
                                task: id,
                                status: TaskStatus::Running,
                            });
                        }
                    }
                    team.messages.push(message.clone());
                    if team.messages.len() > MAX_MESSAGES {
                        if let Some(oldest_read) =
                            team.messages.iter().position(|message| message.read)
                        {
                            team.messages.remove(oldest_read);
                        } else {
                            team.messages.remove(0);
                        }
                    }
                    team.change(Change::Message {
                        id: message.id,
                        to: to.clone(),
                        from: Some(message.from.clone()),
                    });
                    if let Party::Task(task) = &to
                        && let Some(status) = team.task(task).map(|task| task.status)
                    {
                        route_undelivered(team, &message, task, status);
                    }
                    Ok(message)
                })?;
                Ok(TeamResponse::Message(Box::new(message)))
            }
            TeamRequest::PromptDelivered {
                team,
                task,
                messages,
            } => {
                let owner = Party::Task(task.clone());
                let task = self.update(&team, |team| {
                    for message in team
                        .messages
                        .iter_mut()
                        .filter(|message| message.to == owner && messages.contains(&message.id))
                    {
                        message.read = true;
                    }
                    Ok(team.task(&task).cloned())
                })?;
                match task {
                    Some(task) => Ok(TeamResponse::Task(Box::new(task))),
                    None => Err(format!("This team has no task “{}”.", owner.label())),
                }
            }
            TeamRequest::Inbox {
                team,
                party,
                all,
                peek,
            } => {
                let pick = |team: &Team| -> Vec<Message> {
                    team.messages
                        .iter()
                        .filter(|message| message.to == party && (all || !message.read))
                        .cloned()
                        .collect()
                };
                let current = self.team(&team)?;
                let messages = pick(current);
                if party == Party::Lead && !peek {
                    self.update(&team, |team| {
                        team.courier.entry(Party::Lead.label()).or_default().heard_ms = now_ms();
                        Ok(())
                    })?;
                }
                if peek || messages.iter().all(|message| message.read) {
                    return Ok(TeamResponse::Messages(messages));
                }
                self.update(&team, |team| {
                    for message in team
                        .messages
                        .iter_mut()
                        .filter(|message| message.to == party)
                        .filter(|message| !message.goes_in_directly())
                    {
                        message.read = true;
                    }
                    Ok(())
                })?;
                Ok(TeamResponse::Messages(messages))
            }
            TeamRequest::Wait {
                team,
                since,
                tasks,
                party,
                ..
            } => {
                // Blocking happens in `execute_in`, which releases the
                // lock between looks; this looks once.
                let since = since.unwrap_or_else(|| self.cursor(&team, party.as_ref()));
                Ok(match self.look(&team, since, &tasks, party.as_ref())? {
                    Ok((revision, events)) => TeamResponse::Changes {
                        revision,
                        events,
                        timed_out: false,
                    },
                    Err((revision, events)) => TeamResponse::Changes {
                        revision,
                        events,
                        timed_out: true,
                    },
                })
            }
            TeamRequest::Changes { team, since } => {
                let current = self.team(&team)?;
                Ok(TeamResponse::Changes {
                    revision: current.revision,
                    events: current
                        .events
                        .iter()
                        .filter(|event| event.revision > since)
                        .cloned()
                        .collect(),
                    timed_out: false,
                })
            }
            TeamRequest::TakeLeadNews { team, busy } => {
                let current = self.team(&team)?;
                let person = crate::team_courier::person_messages(current);
                let news = if busy {
                    Vec::new()
                } else {
                    crate::team_courier::lead_news_events(current)
                };
                if (person.is_empty() && news.is_empty())
                    || current.status == TeamStatus::Stopped
                {
                    return Ok(TeamResponse::News(None));
                }
                let voice = crate::team_courier::Voice::Tools;
                let mut parts = Vec::new();
                if !person.is_empty() {
                    parts.push(crate::team_courier::person_text(current, &person, voice));
                }
                if !news.is_empty() {
                    parts.push(crate::team_courier::lead_news(current, &news, voice));
                }
                let read: Vec<u64> = person.iter().map(|message| message.id).collect();
                let revision = (!news.is_empty()).then_some(current.revision);
                self.update(&team, |team| {
                    for message in team
                        .messages
                        .iter_mut()
                        .filter(|message| read.contains(&message.id))
                    {
                        message.read = true;
                    }
                    let mark = team.courier.entry(Party::Lead.label()).or_default();
                    if let Some(revision) = revision {
                        mark.typed_revision = mark.typed_revision.max(revision);
                    }
                    mark.last_typed_ms = now_ms();
                    mark.heard_ms = mark.last_typed_ms;
                    Ok(())
                })?;
                Ok(TeamResponse::News(Some(parts.join("\n\n"))))
            }
        }
    }

    /// Changes after `since`: `Ok` when one concerns the waiter (and its
    /// cursor moves past them), `Err` when none does yet.
    #[allow(clippy::type_complexity)]
    fn look(
        &mut self,
        team: &str,
        since: u64,
        tasks: &[String],
        party: Option<&Party>,
    ) -> Result<Result<(u64, Vec<TeamEvent>), (u64, Vec<TeamEvent>)>, String> {
        let current = self.team(team)?;
        let revision = current.revision;
        let events: Vec<TeamEvent> = current
            .events
            .iter()
            .filter(|event| event.revision > since)
            .cloned()
            .collect();
        if !events.iter().any(|event| concerns(event, tasks, party)) {
            return Ok(Err((revision, events)));
        }
        if let Some(party) = party {
            self.advance_cursor(team, party, revision)?;
        }
        Ok(Ok((revision, events)))
    }

    /// Review `review` of team `team` asks for changes: the task it reviews
    /// goes back to its worker with the findings (unless it has had its
    /// rounds, then it fails for the lead to decide), and the review waits
    /// to look at the next attempt.
    fn changes_asked(
        &mut self,
        team: &str,
        review: &str,
        result: TaskResult,
    ) -> Result<Task, String> {
        self.update(team, |team| {
            let current = team
                .task(review)
                .ok_or_else(|| format!("This team has no task “{review}”."))?
                .clone();
            if !matches!(current.status, TaskStatus::Running | TaskStatus::Blocked) {
                return Err(format!(
                    "Task {review} is {}, so it can't become done.",
                    current.status.key()
                ));
            }
            let now = now_ms();
            let Some(target) = current.review_of.clone() else {
                // QA of the integrated result: no one worker gets it back.
                // It waits for a fix to be integrated, then tests again,
                // until it has had its rounds.
                let looked = team.task_mut(review)?;
                looked.status = if looked.attempts >= MAX_REVIEW_ROUNDS {
                    TaskStatus::Done
                } else {
                    TaskStatus::Pending
                };
                looked.result = Some(result);
                looked.reason = (looked.status == TaskStatus::Done).then(|| {
                    format!(
                        "It still finds problems after {} rounds. Decide what's next: send work back once more \
                         (`dragent tasks reopen`), change the plan, or ask the person.",
                        looked.attempts
                    )
                });
                looked.finished_at_ms = (looked.status == TaskStatus::Done).then_some(now);
                looked.updated_at_ms = now;
                let looked = looked.clone();
                team.change(Change::Task {
                    task: review.to_string(),
                    status: looked.status,
                });
                team.change(Change::ChangesAsked {
                    review: review.to_string(),
                    task: String::new(),
                });
                return Ok(looked);
            };
            let findings = result.summary.trim().to_string();
            let reviewed = team.task_mut(&target)?;
            let sent_back = reviewed.status == TaskStatus::Done;
            if sent_back {
                if reviewed.attempts >= MAX_REVIEW_ROUNDS {
                    // Not an error: the work is kept and waits for someone
                    // to choose what happens next.
                    reviewed.status = TaskStatus::NeedsDecision;
                    reviewed.reason = Some(format!(
                        "Review {review} still asks for changes after {} attempts: {findings}\nDecide what's next: send it back once more with `dragent tasks reopen {target}`, change the plan, or ask the person.",
                        reviewed.attempts
                    ));
                    reviewed.finished_at_ms = Some(now);
                } else {
                    reviewed.status = TaskStatus::Pending;
                    reviewed.reason = Some(format!("Review {review} asked for changes: {findings}"));
                    reviewed.result = None;
                    reviewed.finished_at_ms = None;
                }
                reviewed.updated_at_ms = now;
            }
            let reviewed_status = reviewed.status;
            // The review keeps what it found, and looks again once the
            // work is done again.
            let looked = team.task_mut(review)?;
            looked.result = Some(result);
            looked.reason = None;
            looked.status = if sent_back && reviewed_status == TaskStatus::Pending {
                TaskStatus::Pending
            } else {
                TaskStatus::Done
            };
            looked.finished_at_ms = (looked.status == TaskStatus::Done).then_some(now);
            looked.updated_at_ms = now;
            let looked = looked.clone();
            if sent_back {
                team.change(Change::Task {
                    task: target.clone(),
                    status: reviewed_status,
                });
            }
            team.change(Change::Task {
                task: review.to_string(),
                status: looked.status,
            });
            team.change(Change::ChangesAsked {
                review: review.to_string(),
                task: target,
            });
            Ok(looked)
        })
    }

    /// Where `party`'s last wait stopped (0: from the start).
    fn cursor(&self, team: &str, party: Option<&Party>) -> u64 {
        party
            .and_then(|party| self.teams.get(team)?.cursors.get(&party.label()).copied())
            .unwrap_or(0)
    }

    /// Remember that `party` has seen up to `revision`.
    fn advance_cursor(&mut self, team: &str, party: &Party, revision: u64) -> Result<(), String> {
        if self.cursor(team, Some(party)) >= revision {
            return Ok(());
        }
        self.update(team, |team| {
            team.cursors.insert(party.label(), revision);
            Ok(())
        })
    }

    /// Move a task from one of `from` to `to`, recording it.
    fn transition(
        &mut self,
        team: &str,
        task: &str,
        from: &[TaskStatus],
        to: TaskStatus,
        apply: impl FnOnce(&mut Task),
    ) -> Result<Task, String> {
        self.update(team, |team| {
            let current = team.task_mut(task)?;
            if !from.contains(&current.status) {
                return Err(format!(
                    "Task {task} is {}, so it can't become {}.",
                    current.status.key(),
                    to.key()
                ));
            }
            current.status = to;
            current.updated_at_ms = now_ms();
            apply(current);
            let changed = current.clone();
            team.change(Change::Task {
                task: task.to_string(),
                status: to,
            });
            Ok(changed)
        })
    }
}

/// Whether `event` concerns a waiter on `tasks` (any task when empty) and
/// messages for `party`.
pub(crate) fn concerns(event: &TeamEvent, tasks: &[String], party: Option<&Party>) -> bool {
    match &event.change {
        Change::Task { task, status } => {
            matches!(
                status,
                TaskStatus::Done
                    | TaskStatus::Failed
                    | TaskStatus::NeedsDecision
                    | TaskStatus::Blocked
                    | TaskStatus::Cancelled
            ) && (tasks.is_empty() || tasks.contains(task))
        }
        // The lead also hears what workers tell each other, so it can weigh in.
        Change::Message { to, from, .. } => party.is_some_and(|party| {
            party == to
                || (*party == Party::Lead
                    && matches!(to, Party::Task(_))
                    && matches!(from, Some(Party::Task(_))))
        }),
        Change::Team { status } => matches!(status, TeamStatus::Done | TeamStatus::Stopped),
        Change::ChangesAsked { review, task } => {
            tasks.is_empty() || tasks.contains(review) || tasks.contains(task)
        }
        Change::TaskAdded { .. }
        | Change::TaskEdited { .. }
        | Change::Integrated { .. }
        | Change::PromptAnswered { .. }
        | Change::AgentsAdded { .. }
        | Change::Reassigned { .. }
        | Change::Reseated { .. } => false,
        Change::QaNeeded => matches!(party, None | Some(Party::Lead)),
    }
}

// ── The host's store ────────────────────────────────────────────────────────

#[cfg(not(target_family = "wasm"))]
pub(crate) struct Shared {
    pub(crate) store: Mutex<Option<TeamStore>>,
    pub(crate) changed: Condvar,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        store: Mutex::new(None),
        changed: Condvar::new(),
    })
}

/// Run `request` against this organisation's teams. `Wait` blocks (up to
/// its timeout) without holding the store.
#[cfg(not(target_family = "wasm"))]
pub fn execute(request: TeamRequest) -> Result<TeamResponse, String> {
    let request = match request {
        // Who the caller is comes from its session's record, never from
        // the caller's word.
        TeamRequest::Resolve { pane, terminal, .. } => TeamRequest::Resolve {
            identity: session_identity(pane.as_deref(), terminal.as_deref()),
            pane,
            terminal,
        },
        request => request,
    };
    execute_in(shared(), &store_dir(), request)
}

/// The agent conversation running in the session `pane` (else the one in
/// host terminal `terminal`), as its record knows it.
#[cfg(not(target_family = "wasm"))]
pub fn session_identity(pane: Option<&str>, terminal: Option<&str>) -> Option<SessionIdentity> {
    let pane = match pane {
        Some(pane) => uuid::Uuid::parse_str(pane).ok()?,
        None => crate::agent_sessions::pane_for_terminal(terminal?)?,
    };
    let record = crate::agent_sessions::load(pane)?;
    Some(SessionIdentity {
        conversation: record.conversation_id.filter(|id| !id.is_empty())?,
        agent: crate::team_command_support::agent_key(&record.provider).ok()?,
    })
}

/// Teams whose seats moved since the last call, for the host to announce.
#[cfg(not(target_family = "wasm"))]
pub fn take_reseated() -> Vec<String> {
    let mut guard = shared()
        .store
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let mut teams = guard
        .as_mut()
        .map(|store| std::mem::take(&mut store.reseated))
        .unwrap_or_default();
    teams.sort();
    teams.dedup();
    teams
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn store_dir() -> PathBuf {
    dragent_paths::organisation_dir().join("teams")
}

/// Whether any team is still at work (not done or stopped). Its courier runs
/// in the host, so an idle host stays up while one is. Loads the store fresh
/// from disk; meant for an about-once-a-minute check.
#[cfg(not(target_family = "wasm"))]
pub fn any_active_team() -> bool {
    active_team_in(&store_dir())
}

/// A store or team file that can't be read right now counts as active: not
/// being able to look is no reason to stop a team's host.
#[cfg(not(target_family = "wasm"))]
fn active_team_in(dir: &std::path::Path) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return error.kind() != std::io::ErrorKind::NotFound,
    };
    for entry in entries {
        let Ok(entry) = entry else { return true };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let active = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Team>(&bytes) {
                Ok(team) => !matches!(team.status, TeamStatus::Done | TeamStatus::Stopped),
                // Never readable by this build: its courier can't run here.
                Err(_) => false,
            },
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        };
        if active {
            return true;
        }
    }
    false
}

/// Sessions the app's sleep sweep must not put to sleep: every team's lead
/// while the team isn't done/stopped, and the worker of every task whose
/// status is Running or Blocked. Pane strings that aren't UUIDs are skipped.
/// Loads the store fresh from disk; cheap enough to call about once a minute.
pub fn panes_to_keep_awake() -> std::collections::HashSet<uuid::Uuid> {
    let store = TeamStore::open(dragent_paths::organisation_dir().join("teams"));
    panes_to_keep_awake_in(&store)
}

pub(crate) fn panes_to_keep_awake_in(
    store: &TeamStore,
) -> std::collections::HashSet<uuid::Uuid> {
    let mut panes = std::collections::HashSet::new();
    for team in store.teams.values() {
        if matches!(team.status, TeamStatus::Done | TeamStatus::Stopped) {
            continue;
        }
        if let Some(pane) = team.lead.as_ref().and_then(|lead| lead.pane.as_deref()) {
            if let Ok(pane) = uuid::Uuid::parse_str(pane) {
                panes.insert(pane);
            }
        }
        for task in &team.tasks {
            if !matches!(task.status, TaskStatus::Running | TaskStatus::Blocked) {
                continue;
            }
            if let Some(pane) = task.worker.as_ref().and_then(|worker| worker.pane.as_deref()) {
                if let Ok(pane) = uuid::Uuid::parse_str(pane) {
                    panes.insert(pane);
                }
            }
        }
    }
    panes
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn execute_in(
    shared: &Shared,
    dir: &Path,
    request: TeamRequest,
) -> Result<TeamResponse, String> {
    // Loaded on first use, and again if the organisation (its folder) changed.
    let open = |guard: &mut Option<TeamStore>| {
        if guard.as_ref().is_none_or(|store| store.dir != dir) {
            *guard = Some(TeamStore::open(dir.to_path_buf()));
        }
    };
    let mut guard = shared
        .store
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    open(&mut guard);
    let TeamRequest::Wait {
        team,
        since,
        tasks,
        party,
        timeout_ms,
    } = request
    else {
        let response = guard.as_mut().expect("opened").execute(request);
        if response.is_ok() {
            shared.changed.notify_all();
        }
        return response;
    };
    let since = since.unwrap_or_else(|| {
        guard
            .as_ref()
            .expect("opened")
            .cursor(&team, party.as_ref())
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(MAX_WAIT_MS));
    loop {
        open(&mut guard);
        let looked = guard
            .as_mut()
            .expect("opened")
            .look(&team, since, &tasks, party.as_ref())?;
        let now = Instant::now();
        match looked {
            Ok((revision, events)) => {
                return Ok(TeamResponse::Changes {
                    revision,
                    events,
                    timed_out: false,
                });
            }
            Err((revision, events)) if now >= deadline => {
                return Ok(TeamResponse::Changes {
                    revision,
                    events,
                    timed_out: true,
                });
            }
            Err(_) => {}
        }
        guard = shared
            .changed
            .wait_timeout(guard, deadline - now)
            .unwrap_or_else(|poison| poison.into_inner())
            .0;
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn team_records_from_before_seats_knew_their_conversation_still_load() {
        let old = r#"{"id":"team-0a1b2c3d","title":"Old","project_root":"/w","status":"running",
            "lead":{"pane":"p","terminal":"t","agent":"claude"},
            "tasks":[{"id":"t1","title":"A","prompt":"B","kind":"work","status":"running",
              "worker":{"pane":"w","terminal":null,"agent":"codex"},
              "created_at_ms":1,"updated_at_ms":1}],
            "created_at_ms":1,"updated_at_ms":1}"#;
        let team: Team = serde_json::from_str(old).unwrap();
        let lead = team.lead.clone().unwrap();
        assert_eq!(lead.pane.as_deref(), Some("p"));
        assert_eq!((lead.conversation.clone(), lead.seated_ms), (None, None));
        assert!(team.tasks[0].worker.as_ref().unwrap().conversation.is_none());
        // Written back, a seat without a conversation looks as it did.
        let written = serde_json::to_value(&team).unwrap();
        assert!(written["lead"].get("conversation").is_none());
        // A seat with no known conversation is never taken by one.
        assert!(!lead.holds(&SessionIdentity {
            conversation: String::new(),
            agent: "claude".into(),
        }));
        // And the store loads the file.
        let dir = dir();
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("team-0a1b2c3d.json"), old).unwrap();
        let store = TeamStore::open(dir.0.clone());
        assert!(store.team("team-0a1b2c3d").is_ok());
    }

    #[test]
    fn areas_name_files_folders_and_patterns() {
        assert!(in_area("app/src/teams/", "app/src/teams/page.rs"));
        assert!(in_area("app/src/teams", "app/src/teams/page.rs"));
        assert!(in_area("./README.md", "README.md"));
        assert!(!in_area("app/src/teams", "app/src/teamsx/page.rs"));
        assert!(in_area("docs/*.md", "docs/guide.md"));
        assert!(in_area("*.lock", "Cargo.lock"));
        assert!(!in_area("docs/*.md", "docs/guide.txt"));
        assert!(!in_area("", "anything"));
        assert!(areas(vec!["../outside".into()]).is_err());
        assert!(areas(vec!["/etc".into()]).is_err());
        assert_eq!(
            areas(vec![" src/ ".into(), "src/".into(), "".into()]).unwrap(),
            vec!["src/".to_string()]
        );
    }
    use super::*;

    /// A private folder, removed when the test ends.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir() -> Scratch {
        Scratch(
            std::env::temp_dir().join(format!("dragent-teams-{}", uuid::Uuid::new_v4().simple())),
        )
    }

    fn create(store: &mut TeamStore, roster: Vec<RosterAgent>) -> Team {
        match store
            .execute(TeamRequest::Create {
                title: "Google sign-in and invites".into(),
                project_root: "/work/app".into(),
                roster,
                cost: Cost::Balanced,
                max_parallel_per_agent: Some(2),
                lead: Some(Member {
                    pane: Some("lead-pane".into()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                }),
            })
            .unwrap()
        {
            TeamResponse::Team(team) => *team,
            other => panic!("{other:?}"),
        }
    }

    fn add(
        store: &mut TeamStore,
        team: &str,
        title: &str,
        depends_on: &[&str],
        review_of: Option<&str>,
    ) -> Result<Task, String> {
        store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.into(),
                title: title.into(),
                prompt: format!("Do {title}"),
                depends_on: depends_on.iter().map(|d| d.to_string()).collect(),
                agent: Some("claude".into()),
                model: Some("sonnet".into()),
                review_of: review_of.map(str::to_string),
                qa: false,
            })
            .map(|response| match response {
                TeamResponse::Task(task) => *task,
                other => panic!("{other:?}"),
            })
    }

    fn status(store: &TeamStore, team: &str, task: &str) -> TaskStatus {
        store.team(team).unwrap().task(task).unwrap().status
    }

    #[test]
    fn saved_review_limits_load_as_needing_a_decision_and_real_failures_stay_failed() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let id = create(&mut store, vec![]).id;
        add(&mut store, &id, "Design", &[], None).unwrap();
        add(&mut store, &id, "Build", &[], None).unwrap();
        let mut team = store.team(&id).unwrap().clone();
        team.tasks[0].status = TaskStatus::Failed;
        team.tasks[0].reason = Some(
            "Review t5 still asks for changes after 3 attempts: HIGH H7R-A\nDecide what's next.".into(),
        );
        team.tasks[1].status = TaskStatus::Failed;
        team.tasks[1].reason = Some("Its agent exited before the task was finished.".into());
        store.save(&team).unwrap();
        let store = TeamStore::open(scratch.0.clone());
        let loaded = store.team(&id).unwrap();
        assert_eq!(loaded.tasks[0].status, TaskStatus::NeedsDecision);
        assert_eq!(loaded.tasks[0].stage(loaded), "needs_decision");
        assert_eq!(loaded.tasks[1].status, TaskStatus::Failed);
    }

    #[test]
    fn user_models_survive_restart_and_apply_only_to_the_next_attempt() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        let task = add(&mut store, &team, "Implement", &[], None).unwrap().id;
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some("lead-pane".into()),
                    terminal: None,
                    agent: Some("dragent".into()),
                    ..Default::default()
                },
                model: Some("chosen-model".into()),
            })
            .unwrap();
        store
            .execute(TeamRequest::SetTaskModel {
                team: team.clone(),
                task: task.clone(),
                model: Some("opus".into()),
            })
            .unwrap();
        assert!(
            store
                .execute(TeamRequest::EditTask {
                    team: team.clone(),
                    task: task.clone(),
                    edit: TaskEdit {
                        model: Some(Some("haiku".into())),
                        ..Default::default()
                    }
                })
                .is_err()
        );
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: task.clone(),
            })
            .unwrap();
        store
            .execute(TeamRequest::SetTaskModel {
                team: team.clone(),
                task: task.clone(),
                model: None,
            })
            .unwrap();
        assert_eq!(
            store
                .team(&team)
                .unwrap()
                .task(&task)
                .unwrap()
                .model
                .as_deref(),
            Some("opus")
        );
        drop(store);
        let mut store = TeamStore::open(scratch.0.clone());
        let saved = store.team(&team).unwrap();
        assert_eq!(saved.lead_model.as_deref(), Some("chosen-model"));
        assert!(saved.task(&task).unwrap().model_locked);
        assert_eq!(saved.task(&task).unwrap().user_model, None);
        store
            .execute(TeamRequest::FailTask {
                team: team.clone(),
                task: task.clone(),
                reason: "retry".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::ReopenTask {
                team: team.clone(),
                task: task.clone(),
                feedback: "try again".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: task.clone(),
            })
            .unwrap();
        assert_eq!(store.team(&team).unwrap().task(&task).unwrap().model, None);
    }

    #[test]
    fn a_plan_runs_in_dependency_order_and_survives_a_restart() {
        let scratch = dir();
        let dir = scratch.0.clone();
        let mut store = TeamStore::open(dir.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "OAuth", &[], None).unwrap();
        add(&mut store, &team, "Invites", &[], None).unwrap();
        add(&mut store, &team, "UI", &["t1", "t2"], None).unwrap();
        let review = add(&mut store, &team, "Review UI", &[], Some("t3")).unwrap();
        assert_eq!(
            (review.kind, review.depends_on.clone()),
            (TaskKind::Review, vec!["t3".to_string()])
        );

        // Only tasks whose dependencies are done can start.
        let ready: Vec<String> = store
            .team(&team)
            .unwrap()
            .ready_tasks()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert_eq!(ready, vec!["t1", "t2"]);
        let error = store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t3".into(),
            })
            .unwrap_err();
        assert!(error.contains("waits for t1, t2"), "{error}");

        for id in ["t1", "t2"] {
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: id.into(),
                })
                .unwrap();
            store
                .execute(TeamRequest::TaskStarted {
                    base: None,
                    team: team.clone(),
                    task: id.into(),
                    workspace: format!("/work/app-{id}"),
                    branch: None,
                    worker: Member {
                        pane: Some(format!("{id}-pane")),
                        terminal: None,
                        agent: Some("claude".into()),
                        ..Default::default()
                    },
                })
                .unwrap();
        }
        assert_eq!(store.team(&team).unwrap().status, TeamStatus::Running);
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "OAuth works; tests pass".into(),
                    ..Default::default()
                },
            })
            .unwrap();

        // Saved after every change: a new host reads the same plan.
        let reopened = TeamStore::open(dir.clone());
        let saved = reopened.team(&team).unwrap();
        assert_eq!(saved.task("t1").unwrap().status, TaskStatus::Done);
        assert_eq!(saved.task("t2").unwrap().status, TaskStatus::Running);
        assert_eq!(
            saved.task("t1").unwrap().result.as_ref().unwrap().summary,
            "OAuth works; tests pass"
        );
    }

    #[test]
    fn bad_plans_are_refused_with_a_reason() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(
            &mut store,
            vec![RosterAgent {
                agent: "claude".into(),
                models: vec!["opus".into(), "sonnet".into()],
            }],
        )
        .id;
        add(&mut store, &team, "A", &[], None).unwrap();
        assert!(
            add(&mut store, &team, "B", &["t9"], None)
                .unwrap_err()
                .contains("no task “t9”")
        );
        assert!(
            add(&mut store, &team, "a", &[], None)
                .unwrap_err()
                .contains("already a task")
        );
        add(&mut store, &team, "B", &["t1"], None).unwrap();
        // t1 can't come to wait on t2, which waits on t1.
        let error = store
            .execute(TeamRequest::EditTask {
                team: team.clone(),
                task: "t1".into(),
                edit: TaskEdit {
                    depends_on: Some(vec!["t2".into()]),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(error.contains("wait on itself"), "{error}");
        // Only the agents and models the person chose.
        let codex = store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.clone(),
                title: "C".into(),
                prompt: "Do C".into(),
                depends_on: vec![],
                agent: Some("codex".into()),
                model: None,
                review_of: None,
                qa: false,
            })
            .unwrap_err();
        assert!(codex.contains("isn't one of this team's agents"), "{codex}");
        let haiku = store
            .execute(TeamRequest::EditTask {
                team: team.clone(),
                task: "t1".into(),
                edit: TaskEdit {
                    model: Some(Some("haiku".into())),
                    ..Default::default()
                },
            })
            .unwrap_err();
        assert!(haiku.contains("opus, sonnet"), "{haiku}");
        // A review can't review a review.
        add(&mut store, &team, "Review A", &[], Some("t1")).unwrap();
        assert!(
            add(&mut store, &team, "Review review", &[], Some("t3"))
                .unwrap_err()
                .contains("itself a review")
        );
    }

    #[test]
    fn one_agent_runs_at_most_its_limit_at_once() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id; // limit 2
        for title in ["A", "B", "C"] {
            add(&mut store, &team, title, &[], None).unwrap();
        }
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t2".into(),
            })
            .unwrap();
        let error = store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t3".into(),
            })
            .unwrap_err();
        assert!(error.contains("already running"), "{error}");
        store
            .execute(TeamRequest::FailTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "checkout failed".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t3".into(),
            })
            .unwrap();
        assert_eq!(status(&store, &team, "t3"), TaskStatus::Starting);
    }

    #[test]
    fn terminals_find_their_team_and_role() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "A", &[], None).unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t1".into(),
                workspace: "/w".into(),
                branch: None,
                worker: Member {
                    pane: None,
                    terminal: Some("term-7".into()),
                    agent: None,
                    ..Default::default()
                },
            })
            .unwrap();
        let resolve =
            |store: &mut TeamStore, pane: Option<&str>, terminal: Option<&str>| match store
                .execute(TeamRequest::Resolve {
                    pane: pane.map(str::to_string),
                    terminal: terminal.map(str::to_string),
                    identity: None,
                })
                .unwrap()
            {
                TeamResponse::Resolved(found) => found,
                other => panic!("{other:?}"),
            };
        assert_eq!(
            resolve(&mut store, Some("lead-pane"), None),
            Some((team.clone(), Party::Lead))
        );
        assert_eq!(
            resolve(&mut store, None, Some("term-7")),
            Some((team.clone(), Party::Task("t1".into())))
        );
        assert_eq!(resolve(&mut store, Some("other"), None), None);
        assert_eq!(resolve(&mut store, None, None), None);
    }

    #[test]
    fn a_question_to_the_person_carries_its_answers_and_only_it() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        let send = |store: &mut TeamStore, to: Party, choices: Vec<&str>| {
            match store
                .execute(TeamRequest::Send {
                    team: team.clone(),
                    from: Party::Lead,
                    to,
                    kind: MessageKind::NeedsUser,
                    body: "The plan is ready. Start it?".into(),
                    reply_to: None,
                    choices: choices.into_iter().map(String::from).collect(),
                })
                .unwrap()
            {
                TeamResponse::Message(message) => message.choices,
                other => panic!("{other:?}"),
            }
        };
        let long = "x".repeat(MAX_CHOICE_CHARS + 10);
        let kept = send(&mut store, Party::User, vec![" Start ", "", "Change the plan", "A", "B", &long]);
        assert_eq!(kept, vec!["Start", "Change the plan", "A", "B"], "trimmed, no blanks, at most {MAX_CHOICES}");
        let kept = send(&mut store, Party::User, vec![&long]);
        assert_eq!(kept[0].chars().count(), MAX_CHOICE_CHARS);
    }

    #[test]
    fn the_mailbox_delivers_reads_and_replies() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "Invites", &[], None).unwrap();
        let send =
            |store: &mut TeamStore, from: Party, to: Party, body: &str, reply_to: Option<u64>| {
                store.execute(TeamRequest::Send {
                    team: team.clone(),
                    from,
                    to,
                    kind: MessageKind::Question,
                    body: body.into(),
                    reply_to,
                    choices: Vec::new(),
                })
            };
        let question = match send(
            &mut store,
            Party::Task("t1".into()),
            Party::Lead,
            "Resend or SES?",
            None,
        )
        .unwrap()
        {
            TeamResponse::Message(message) => *message,
            other => panic!("{other:?}"),
        };
        send(
            &mut store,
            Party::Lead,
            Party::Task("t1".into()),
            "Resend",
            Some(question.id),
        )
        .unwrap();
        assert!(
            send(&mut store, Party::Lead, Party::Task("t9".into()), "x", None)
                .unwrap_err()
                .contains("no task")
        );
        assert!(
            send(&mut store, Party::Lead, Party::User, "x", Some(99))
                .unwrap_err()
                .contains("no message 99")
        );

        let inbox = |store: &mut TeamStore, party: Party, all: bool| match store
            .execute(TeamRequest::Inbox {
                team: team.clone(),
                party,
                all,
                peek: false,
            })
            .unwrap()
        {
            TeamResponse::Messages(messages) => messages,
            other => panic!("{other:?}"),
        };
        let lead = inbox(&mut store, Party::Lead, false);
        assert_eq!(lead.len(), 2);
        assert_eq!(lead[0].body, "Resend or SES?");
        // t1 hasn't started, so the lead is told its answer waits for it.
        assert_eq!(lead[1].from, Party::Dragent);
        assert!(lead[1].body.contains("delivered when it starts"), "{}", lead[1].body);
        // Read once: gone from unread, still in the full history.
        assert!(inbox(&mut store, Party::Lead, false).is_empty());
        assert_eq!(inbox(&mut store, Party::Lead, true).len(), 2);
        let worker = inbox(&mut store, Party::Task("t1".into()), false);
        assert_eq!(
            (worker[0].body.as_str(), worker[0].reply_to),
            ("Resend", Some(question.id))
        );

        // A worker that blocked on a question carries on once the lead answers.
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t1".into(),
                workspace: "/w".into(),
                branch: None,
                worker: Member::default(),
            })
            .unwrap();
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t1".into(),
                reason: "Which mail provider?".into(),
            })
            .unwrap();
        send(
            &mut store,
            Party::Task("t1".into()),
            Party::Lead,
            "still blocked",
            None,
        )
        .unwrap();
        assert_eq!(
            status(&store, &team, "t1"),
            TaskStatus::Blocked,
            "a worker's own message doesn't unblock it"
        );
        send(
            &mut store,
            Party::Lead,
            Party::Task("t1".into()),
            "Resend, as decided",
            None,
        )
        .unwrap();
        let task = store.team(&team).unwrap().task("t1").unwrap().clone();
        assert_eq!((task.status, task.reason), (TaskStatus::Running, None));
    }

    #[test]
    fn wait_returns_when_a_task_finishes_or_a_message_arrives() {
        let scratch = dir();
        let dir = scratch.0.clone();
        let shared: &'static Shared = Box::leak(Box::new(Shared {
            store: Mutex::new(None),
            changed: Condvar::new(),
        }));
        let run = |request| execute_in(shared, &dir, request);
        let team = match run(TeamRequest::Create {
            title: "T".into(),
            project_root: "/w".into(),
            roster: vec![],
            cost: Cost::Balanced,
            max_parallel_per_agent: None,
            lead: None,
        })
        .unwrap()
        {
            TeamResponse::Team(team) => team.id,
            other => panic!("{other:?}"),
        };
        run(TeamRequest::AddTask {
            owns: Vec::new(),
            team: team.clone(),
            title: "A".into(),
            prompt: "Do A".into(),
            depends_on: vec![],
            agent: Some("claude".into()),
            model: None,
            review_of: None,
            qa: false,
        })
        .unwrap();
        let since = match run(TeamRequest::Changes {
            team: team.clone(),
            since: 0,
        })
        .unwrap()
        {
            TeamResponse::Changes { revision, .. } => revision,
            other => panic!("{other:?}"),
        };
        // Nothing yet: times out.
        match run(TeamRequest::Wait {
            team: team.clone(),
            since: Some(since),
            tasks: vec![],
            party: Some(Party::Lead),
            timeout_ms: 50,
        })
        .unwrap()
        {
            TeamResponse::Changes { timed_out, .. } => assert!(timed_out),
            other => panic!("{other:?}"),
        }
        // A worker finishing on another thread wakes the waiting lead.
        let dir_for_worker = dir.clone();
        let worker_team = team.clone();
        let worker = std::thread::spawn(move || {
            let run = |request| execute_in(shared, &dir_for_worker, request);
            std::thread::sleep(Duration::from_millis(100));
            run(TeamRequest::BeginTask {
                team: worker_team.clone(),
                task: "t1".into(),
            })
            .unwrap();
            run(TeamRequest::TaskStarted {
                base: None,
                team: worker_team.clone(),
                task: "t1".into(),
                workspace: "/w1".into(),
                branch: None,
                worker: Member::default(),
            })
            .unwrap();
            run(TeamRequest::FinishTask {
                team: worker_team,
                task: "t1".into(),
                result: TaskResult {
                    summary: "done".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        });
        let started = Instant::now();
        match run(TeamRequest::Wait {
            team: team.clone(),
            since: Some(since),
            tasks: vec!["t1".into()],
            party: None,
            timeout_ms: 5_000,
        })
        .unwrap()
        {
            TeamResponse::Changes {
                timed_out, events, ..
            } => {
                assert!(!timed_out);
                assert!(events.iter().any(|event| event.change
                    == Change::Task {
                        task: "t1".into(),
                        status: TaskStatus::Done
                    }));
            }
            other => panic!("{other:?}"),
        }
        assert!(started.elapsed() < Duration::from_secs(4));
        worker.join().unwrap();
        // A message for the lead also ends a wait. Without `since`, a wait
        // starts where the lead's last one stopped: the first sees the
        // finished task, the next only what came after it.
        let lead_wait = |timeout_ms| match run(TeamRequest::Wait {
            team: team.clone(),
            since: None,
            tasks: vec![],
            party: Some(Party::Lead),
            timeout_ms,
        })
        .unwrap()
        {
            TeamResponse::Changes {
                timed_out, events, ..
            } => (timed_out, events),
            other => panic!("{other:?}"),
        };
        let (timed_out, events) = lead_wait(1_000);
        assert!(
            !timed_out
                && events.iter().any(|event| matches!(
                    event.change,
                    Change::Task {
                        status: TaskStatus::Done,
                        ..
                    }
                ))
        );
        assert!(lead_wait(50).0, "nothing new since the last wait");
        run(TeamRequest::Send {
            team: team.clone(),
            from: Party::User,
            to: Party::Lead,
            kind: MessageKind::Answer,
            body: "Use the staging key".into(),
            reply_to: None,
            choices: Vec::new(),
        })
        .unwrap();
        let (timed_out, events) = lead_wait(1_000);
        assert!(!timed_out);
        assert!(
            events
                .iter()
                .all(|event| !matches!(event.change, Change::Task { .. })),
            "{events:?}"
        );
        assert!(events.iter().any(|event| matches!(
            event.change,
            Change::Message {
                to: Party::Lead,
                ..
            }
        )));
        // The cursor is saved with the team.
        assert!(TeamStore::open(dir.clone()).cursor(&team, Some(&Party::Lead)) > 0);
    }

    #[test]
    fn reviews_hold_back_what_comes_after_until_they_approve() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "API", &[], None).unwrap();
        add(&mut store, &team, "Review API", &[], Some("t1")).unwrap();
        add(&mut store, &team, "Integrate", &["t2"], None).unwrap();
        add(&mut store, &team, "Docs", &[], None).unwrap();
        let run = |store: &mut TeamStore, id: &str| {
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: id.into(),
                })
                .unwrap();
            store
                .execute(TeamRequest::TaskStarted {
                    base: None,
                    team: team.clone(),
                    task: id.into(),
                    workspace: "/w".into(),
                    branch: None,
                    worker: Member::default(),
                })
                .unwrap();
        };
        let finish = |store: &mut TeamStore, id: &str, verdict: Option<Verdict>| {
            store.execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: id.into(),
                result: TaskResult {
                    summary: "s".into(),
                    verdict,
                    ..Default::default()
                },
            })
        };
        let ready = |store: &TeamStore| -> Vec<String> {
            store
                .team(&team)
                .unwrap()
                .ready_tasks()
                .iter()
                .map(|task| task.id.clone())
                .collect()
        };
        assert!(
            finish(&mut store, "t4", Some(Verdict::Approve)).is_err(),
            "not running"
        );
        run(&mut store, "t4");
        assert!(
            finish(&mut store, "t4", Some(Verdict::Approve))
                .unwrap_err()
                .contains("Only reviews")
        );
        finish(&mut store, "t4", None).unwrap();
        for round in 1..=MAX_REVIEW_ROUNDS {
            run(&mut store, "t1");
            finish(&mut store, "t1", None).unwrap();
            run(&mut store, "t2");
            assert!(
                finish(&mut store, "t2", None)
                    .unwrap_err()
                    .contains("verdict")
            );
            let since = store.team(&team).unwrap().revision;
            finish(&mut store, "t2", Some(Verdict::Changes)).unwrap();
            let current = store.team(&team).unwrap();
            assert!(current.events.iter().any(|event| event.revision > since
                && event.change
                    == Change::ChangesAsked {
                        review: "t2".into(),
                        task: "t1".into()
                    }));
            if round < MAX_REVIEW_ROUNDS {
                // Back to its worker; the review waits for it, integration for the review.
                assert_eq!(
                    (status(&store, &team, "t1"), status(&store, &team, "t2")),
                    (TaskStatus::Pending, TaskStatus::Pending)
                );
                assert_eq!(ready(&store), vec!["t1"]);
            }
        }
        // Out of rounds: t1 waits for the lead to decide (not a failure);
        // nothing after it starts.
        let t1 = store.team(&team).unwrap().task("t1").unwrap().clone();
        assert_eq!(t1.status, TaskStatus::NeedsDecision);
        assert_eq!(t1.stage(store.team(&team).unwrap()), "needs_decision");
        assert!(
            t1.reason
                .unwrap()
                .contains("still asks for changes after 3 attempts")
        );
        assert_eq!(status(&store, &team, "t2"), TaskStatus::Done);
        assert!(ready(&store).is_empty());
        // Sent back by the lead and approved at last: integration may start.
        store
            .execute(TeamRequest::ReopenTask {
                team: team.clone(),
                task: "t1".into(),
                feedback: "Once more".into(),
            })
            .unwrap();
        run(&mut store, "t1");
        finish(&mut store, "t1", None).unwrap();
        store
            .execute(TeamRequest::ReopenTask {
                team: team.clone(),
                task: "t2".into(),
                feedback: "Look again".into(),
            })
            .unwrap();
        run(&mut store, "t2");
        finish(&mut store, "t2", Some(Verdict::Approve)).unwrap();
        assert_eq!(ready(&store), vec!["t3"]);
    }

    #[test]
    fn work_that_builds_on_reviewed_work_waits_for_the_verdict() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "API", &[], None).unwrap();
        add(&mut store, &team, "Review API", &[], Some("t1")).unwrap();
        add(&mut store, &team, "Page", &["t1"], None).unwrap();
        // Planned to look after the page: it can't hold the page back.
        add(&mut store, &team, "Review again", &["t3"], Some("t1")).unwrap();
        let run = |store: &mut TeamStore, id: &str| {
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: id.into(),
                })
                .map(|_| {
                    store
                        .execute(TeamRequest::TaskStarted {
                            base: None,
                            team: team.clone(),
                            task: id.into(),
                            workspace: "/w".into(),
                            branch: None,
                            worker: Member::default(),
                        })
                        .unwrap();
                })
        };
        let finish = |store: &mut TeamStore, id: &str, verdict: Option<Verdict>| {
            store
                .execute(TeamRequest::FinishTask {
                    team: team.clone(),
                    task: id.into(),
                    result: TaskResult {
                        summary: "s".into(),
                        verdict,
                        ..Default::default()
                    },
                })
                .unwrap();
        };
        let ready = |store: &TeamStore| -> Vec<String> {
            let team = store.team(&team).unwrap();
            team.ready_tasks()
                .iter()
                .map(|task| task.id.clone())
                .collect()
        };
        run(&mut store, "t1").unwrap();
        finish(&mut store, "t1", None);
        assert_eq!(ready(&store), vec!["t2"]);
        let current = store.team(&team).unwrap();
        assert_eq!(current.waiting_on(current.task("t3").unwrap()), vec!["t2"]);
        let refused = run(&mut store, "t3").unwrap_err();
        assert!(refused.contains("waits for t2"), "{refused}");
        run(&mut store, "t2").unwrap();
        finish(&mut store, "t2", Some(Verdict::Approve));
        assert_eq!(ready(&store), vec!["t3"]);
        run(&mut store, "t3").unwrap();
        finish(&mut store, "t3", None);
        assert_eq!(ready(&store), vec!["t4"]);
    }

    #[test]
    fn built_work_is_done_only_once_a_qa_pass_tested_it() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "API", &[], None).unwrap();
        let run = |store: &mut TeamStore, id: &str| {
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: id.into(),
                })
                .unwrap();
            store
                .execute(TeamRequest::TaskStarted {
                    base: None,
                    team: team.clone(),
                    task: id.into(),
                    workspace: "/w".into(),
                    branch: None,
                    worker: Member::default(),
                })
                .unwrap();
        };
        let finish = |store: &mut TeamStore, id: &str, verdict: Option<Verdict>| {
            store
                .execute(TeamRequest::FinishTask {
                    team: team.clone(),
                    task: id.into(),
                    result: TaskResult {
                        summary: "s".into(),
                        verdict,
                        ..Default::default()
                    },
                })
                .unwrap();
        };
        let asked = |store: &TeamStore| {
            let team = store.team(&team).unwrap();
            (
                team.status,
                team.events
                    .iter()
                    .filter(|event| event.change == Change::QaNeeded)
                    .count(),
            )
        };
        let qa = |store: &mut TeamStore, title: &str, review_of: Option<&str>| {
            store.execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.clone(),
                title: title.into(),
                prompt: "Test it all".into(),
                depends_on: Vec::new(),
                agent: Some("claude".into()),
                model: None,
                review_of: review_of.map(str::to_string),
                qa: true,
            })
        };
        run(&mut store, "t1");
        finish(&mut store, "t1", None);
        // Built but untested: not done, and the lead hears it once.
        assert_eq!(asked(&store), (TeamStatus::Running, 1));
        store
            .execute(TeamRequest::Send {
                team: team.clone(),
                from: Party::User,
                to: Party::Lead,
                kind: MessageKind::Note,
                body: "How's it going?".into(),
                reply_to: None,
                choices: Vec::new(),
            })
            .unwrap();
        assert_eq!(asked(&store), (TeamStatus::Running, 1));
        let current = store.team(&team).unwrap();
        let event = current
            .events
            .iter()
            .find(|event| event.change == Change::QaNeeded)
            .unwrap();
        assert!(concerns(event, &[], Some(&Party::Lead)), "wakes the lead");
        assert!(!concerns(event, &[], Some(&Party::Task("t1".into()))));

        // (Without a task to test, a QA pass tests the integrated work:
        // see the coordination commands' tests.)
        qa(&mut store, "QA", Some("t1")).unwrap();
        assert!(store.team(&team).unwrap().task("t2").unwrap().qa);
        run(&mut store, "t2");
        // Problems found: the work goes back, like any review's.
        finish(&mut store, "t2", Some(Verdict::Changes));
        assert_eq!(status(&store, &team, "t1"), TaskStatus::Pending);
        run(&mut store, "t1");
        finish(&mut store, "t1", None);
        run(&mut store, "t2");
        finish(&mut store, "t2", Some(Verdict::Approve));
        assert_eq!(asked(&store), (TeamStatus::Done, 1));

        // A QA pass the lead cancelled is their call.
        let other = create(&mut store, vec![]).id;
        add(&mut store, &other, "Docs", &[], None).unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: other.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: other.clone(),
                task: "t1".into(),
                workspace: "/w".into(),
                branch: None,
                worker: Member::default(),
            })
            .unwrap();
        store
            .execute(TeamRequest::FinishTask {
                team: other.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "s".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        assert_eq!(store.team(&other).unwrap().status, TeamStatus::Running);
        store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: other.clone(),
                title: "QA".into(),
                prompt: "Test it".into(),
                depends_on: Vec::new(),
                agent: Some("claude".into()),
                model: None,
                review_of: Some("t1".into()),
                qa: true,
            })
            .unwrap();
        store
            .execute(TeamRequest::CancelTask {
                team: other.clone(),
                task: "t2".into(),
            })
            .unwrap();
        assert_eq!(store.team(&other).unwrap().status, TeamStatus::Done);
    }

    #[test]
    fn records_stay_small_and_finished_teams_can_go() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        let long = "x".repeat(20_001);
        let refused = store
            .execute(TeamRequest::AddTask {
                owns: Vec::new(),
                team: team.clone(),
                title: "Big".into(),
                prompt: long.clone(),
                depends_on: vec![],
                agent: None,
                model: None,
                review_of: None,
                qa: false,
            })
            .unwrap_err();
        assert!(
            refused.contains("too long: 20001 characters, at most 20000"),
            "{refused}"
        );
        assert!(
            store
                .execute(TeamRequest::Send {
                    team: team.clone(),
                    from: Party::User,
                    to: Party::Lead,
                    kind: MessageKind::Note,
                    body: long,
                    reply_to: None,
                    choices: Vec::new(),
                })
                .is_err()
        );
        // Only once it's over does a team leave the list, file and all.
        assert!(
            store
                .execute(TeamRequest::Forget { team: team.clone() })
                .unwrap_err()
                .contains("stop it first")
        );
        store
            .execute(TeamRequest::Stop { team: team.clone() })
            .unwrap();
        store
            .execute(TeamRequest::Forget { team: team.clone() })
            .unwrap();
        assert!(store.team(&team).is_err());
        assert!(!scratch.0.join(format!("{team}.json")).exists());
        assert!(TeamStore::open(scratch.0.clone()).team(&team).is_err());
    }

    #[test]
    fn a_cancelled_task_no_longer_holds_up_what_came_after_it() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "Spike", &[], None).unwrap();
        add(&mut store, &team, "Build", &["t1"], None).unwrap();
        assert!(
            store
                .team(&team)
                .unwrap()
                .ready_tasks()
                .iter()
                .all(|task| task.id != "t2")
        );
        store
            .execute(TeamRequest::CancelTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        assert!(
            store
                .team(&team)
                .unwrap()
                .ready_tasks()
                .iter()
                .any(|task| task.id == "t2")
        );
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t2".into(),
            })
            .unwrap();
    }

    #[test]
    fn reviews_send_work_back_and_stopping_cancels_the_rest() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let team = create(&mut store, vec![]).id;
        add(&mut store, &team, "Invites", &[], None).unwrap();
        add(&mut store, &team, "Review invites", &[], Some("t1")).unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t1".into(),
                workspace: "/w".into(),
                branch: None,
                worker: Member::default(),
            })
            .unwrap();
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "done".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        store
            .execute(TeamRequest::ReopenTask {
                team: team.clone(),
                task: "t1".into(),
                feedback: "Hash the invite tokens".into(),
            })
            .unwrap();
        let reopened = store.team(&team).unwrap().task("t1").unwrap().clone();
        assert_eq!(
            (
                reopened.status,
                reopened.reason.as_deref(),
                reopened.attempts
            ),
            (TaskStatus::Pending, Some("Hash the invite tokens"), 1)
        );
        store
            .execute(TeamRequest::Stop { team: team.clone() })
            .unwrap();
        let stopped = store.team(&team).unwrap();
        assert_eq!(stopped.status, TeamStatus::Stopped);
        assert!(
            stopped
                .tasks
                .iter()
                .all(|task| task.status == TaskStatus::Cancelled)
        );
        assert!(
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: "t1".into()
                })
                .unwrap_err()
                .contains("stopped")
        );
    }

    #[test]
    fn agents_join_a_team_at_work_and_tasks_move_to_them() {
        let dir = std::env::temp_dir().join(format!("dragent-roster-{}", uuid::Uuid::new_v4().simple()));
        let mut store = TeamStore::open(dir.clone());
        let TeamResponse::Team(team) = store
            .execute(TeamRequest::Create {
                title: "Invites".into(),
                project_root: "/w".into(),
                roster: vec![RosterAgent {
                    agent: "opencode".into(),
                    models: vec![],
                }],
                cost: Cost::default(),
                max_parallel_per_agent: None,
                lead: None,
            })
            .unwrap()
        else {
            panic!()
        };
        let id = team.id;
        for title in ["API", "Page"] {
            store
                .execute(TeamRequest::AddTask {
                    owns: Vec::new(),
                    team: id.clone(),
                    title: title.into(),
                    prompt: "Build it".into(),
                    depends_on: vec![],
                    agent: Some("opencode".into()),
                    model: None,
                    review_of: None,
                    qa: false,
                })
                .unwrap();
        }
        store
            .execute(TeamRequest::BeginTask {
                team: id.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: id.clone(),
                task: "t1".into(),
                workspace: "/w/t1".into(),
                branch: Some("api-t1".into()),
                worker: Member {
                    pane: Some("old-worker".into()),
                    terminal: None,
                    agent: Some("opencode".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        let edit_page = |agent: &str| TeamRequest::EditTask {
            team: id.clone(),
            task: "t2".into(),
            edit: TaskEdit {
                agent: Some(Some(agent.to_string())),
                ..Default::default()
            },
        };
        let move_api = |task: &str| TeamRequest::ReassignTask {
            team: id.clone(),
            task: task.to_string(),
            agent: "claude".into(),
            model: None,
            note: "Carry on".into(),
        };

        // Not on the team yet: refused, and the answer says how.
        let refused = store.execute(edit_page("claude")).unwrap_err();
        assert!(refused.contains("team agents add claude"), "{refused}");
        assert!(store.execute(move_api("t1")).is_err());
        assert_eq!(store.team(&id).unwrap().task("t1").unwrap().status, TaskStatus::Running);

        let TeamResponse::Team(team) = store
            .execute(TeamRequest::AddAgents {
                team: id.clone(),
                agents: vec![RosterAgent {
                    agent: "claude".into(),
                    models: vec![],
                }],
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(team.roster.len(), 2);
        assert_eq!(team.status, TeamStatus::Running, "the team carries on");
        assert!(team.events.iter().any(|event| matches!(
            &event.change,
            Change::AgentsAdded { agents } if agents == &["claude"]
        )));

        // A planned task changes in place; one under way moves.
        store.execute(edit_page("claude")).unwrap();
        let edited = store.execute(edit_page("opencode"));
        assert!(edited.is_ok());
        store.execute(edit_page("claude")).unwrap();
        let refused = store.execute(move_api("t2")).unwrap_err();
        assert!(refused.contains("tasks edit t2"), "{refused}");

        let TeamResponse::Task(moved) = store.execute(move_api("t1")).unwrap() else {
            panic!()
        };
        assert_eq!(moved.status, TaskStatus::Pending);
        assert_eq!(moved.agent.as_deref(), Some("claude"));
        assert_eq!(moved.worker, None, "its old worker is out of the team");
        assert_eq!(moved.workspace.as_deref(), Some("/w/t1"), "it keeps its checkout");
        assert_eq!(moved.branch.as_deref(), Some("api-t1"));
        assert_eq!(moved.attempts, 1);
        let current = store.team(&id).unwrap();
        assert_eq!(current.task("t2").unwrap().status, TaskStatus::Pending, "nothing else moved");
        assert!(current.events.iter().any(|event| matches!(
            &event.change,
            Change::Reassigned { task, from, to }
                if task == "t1" && from.as_deref() == Some("opencode") && to == "claude"
        )));
        assert!(matches!(
            store.execute(TeamRequest::Resolve {
                pane: Some("old-worker".into()),
                terminal: None,
                identity: None,
            }).unwrap(),
            TeamResponse::Resolved(None)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keep_awake_covers_leads_and_running_or_blocked_workers() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let lead = uuid::Uuid::new_v4();
        let running = uuid::Uuid::new_v4();
        let blocked = uuid::Uuid::new_v4();
        let team = create(&mut store, vec![]).id;
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some(lead.to_string()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        add(&mut store, &team, "Running work", &[], None).unwrap();
        add(&mut store, &team, "Blocked work", &[], None).unwrap();
        add(&mut store, &team, "Pending work", &[], None).unwrap();
        for (id, pane) in [("t1", running), ("t2", blocked)] {
            store
                .execute(TeamRequest::BeginTask {
                    team: team.clone(),
                    task: id.into(),
                })
                .unwrap();
            store
                .execute(TeamRequest::TaskStarted {
                    base: None,
                    team: team.clone(),
                    task: id.into(),
                    workspace: format!("/w/{id}"),
                    branch: None,
                    worker: Member {
                        pane: Some(pane.to_string()),
                        terminal: None,
                        agent: Some("claude".into()),
                        ..Default::default()
                    },
                })
                .unwrap();
        }
        store
            .execute(TeamRequest::BlockTask {
                team: team.clone(),
                task: "t2".into(),
                reason: "need a decision".into(),
            })
            .unwrap();

        let panes = panes_to_keep_awake_in(&store);
        assert!(panes.contains(&lead));
        assert!(panes.contains(&running));
        assert!(panes.contains(&blocked));
        assert_eq!(panes.len(), 3, "pending work has no worker yet: {panes:?}");
    }

    #[test]
    fn keep_awake_skips_finished_teams_done_tasks_and_bad_panes() {
        let scratch = dir();
        let mut store = TeamStore::open(scratch.0.clone());
        let lead = uuid::Uuid::new_v4();
        let worker = uuid::Uuid::new_v4();
        let team = create(&mut store, vec![]).id;
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some(lead.to_string()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        add(&mut store, &team, "Work", &[], None).unwrap();
        store
            .execute(TeamRequest::BeginTask {
                team: team.clone(),
                task: "t1".into(),
            })
            .unwrap();
        store
            .execute(TeamRequest::TaskStarted {
                base: None,
                team: team.clone(),
                task: "t1".into(),
                workspace: "/w/t1".into(),
                branch: None,
                worker: Member {
                    pane: Some(worker.to_string()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(panes_to_keep_awake_in(&store).contains(&worker));

        // Done work no longer keeps its worker awake.
        store
            .execute(TeamRequest::FinishTask {
                team: team.clone(),
                task: "t1".into(),
                result: TaskResult {
                    summary: "done; tests pass".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let panes = panes_to_keep_awake_in(&store);
        assert!(!panes.contains(&worker), "done tasks sleep: {panes:?}");

        // Non-UUID panes are skipped, never fail the sweep.
        store
            .execute(TeamRequest::SetLead {
                team: team.clone(),
                lead: Member {
                    pane: Some("lead-pane".into()),
                    terminal: None,
                    agent: Some("claude".into()),
                    ..Default::default()
                },
                model: None,
            })
            .unwrap();
        assert!(!panes_to_keep_awake_in(&store).contains(&lead));

        // A stopped team keeps nothing awake.
        store.execute(TeamRequest::Stop { team: team.clone() }).unwrap();
        assert!(
            panes_to_keep_awake_in(&store).is_empty(),
            "stopped teams sleep"
        );

        // The public entry point loads the same store from disk.
        let _ = panes_to_keep_awake();
    }
}
