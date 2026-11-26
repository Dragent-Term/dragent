//! Idle agents can be put to sleep to free memory, and wake when opened.
//!
//! An agent CLI (opencode, Claude, Codex…) keeps its process — often near a
//! gigabyte — for as long as its tab exists, working or not. With 10–20 tabs
//! that exhausts a laptop's memory. So once more than [`ALWAYS_AWAKE`] agents
//! run, and some have finished, sit off screen and haven't been touched for a
//! while (Settings → Agents, 15 minutes by default), Dragent asks whether to
//! put those to sleep. Nothing sleeps without the person's word: from that
//! question, or "Sleep workspace". A sleeping agent's tab stays, with its
//! history; opening it (or opening it on the phone) resumes the same
//! conversation in a new process.
//!
//! Never put to sleep: an agent that is working or waiting for an answer, one
//! with something it started still running in the background (a build, tests,
//! a dev server), one whose conversation can't be resumed exactly, one
//! coordinating with a team, one in use on the phone, and the built-in
//! Dragent agent (it has no process).
//!
//! When the machine is short of memory the idle time drops to a couple of
//! minutes. Starting an agent never asks.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use uuid::Uuid;
use warpui::elements::{ConstrainedBox, Container, Element, ParentElement, Text};
use warpui::{r#async::Timer, UpdateModel, UpdateView, ViewContext};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::action::WorkspaceAction;
use super::view::WorkspaceView;
use crate::theme::Theme;

/// How often idle agents are looked for.
const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// Idle time before sleeping while the machine is short of memory.
const SHORT_OF_MEMORY_IDLE: Duration = Duration::from_secs(2 * 60);
/// How long the "put to sleep" notice stays.
const NOTICE_FOR: Duration = Duration::from_secs(7);
/// This many agents are left running however long they sit.
const ALWAYS_AWAKE: usize = 5;
/// After "Keep awake", how long until asking again; sooner while a new agent
/// waits for memory.
const ASK_AGAIN: Duration = Duration::from_secs(60 * 60);
const ASK_AGAIN_WAITING: Duration = Duration::from_secs(10 * 60);
/// How often wake requests (a team's courier needs a sleeping agent) are
/// looked for, while any agent sleeps.
const WAKE_EVERY: Duration = Duration::from_secs(2);

fn asleep() -> &'static Mutex<HashSet<Uuid>> {
    static ASLEEP: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();
    ASLEEP.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Whether this pane's agent is asleep (cheap; read while drawing).
pub(crate) fn is_pane_asleep(pane: Uuid) -> bool {
    asleep().lock().map(|set| set.contains(&pane)).unwrap_or(false)
}

/// Whether any agent is asleep: lets callers skip all work otherwise.
pub(crate) fn any_asleep() -> bool {
    asleep().lock().map(|set| !set.is_empty()).unwrap_or(false)
}

/// When each agent last woke: its idle time counts from then, so one woken
/// and then left isn't put straight back to sleep.
fn woken() -> &'static Mutex<std::collections::HashMap<Uuid, std::time::Instant>> {
    static WOKEN: OnceLock<Mutex<std::collections::HashMap<Uuid, std::time::Instant>>> = OnceLock::new();
    WOKEN.get_or_init(Default::default)
}

fn since_woken(pane: Uuid) -> Option<Duration> {
    woken().lock().ok()?.get(&pane).map(|at| at.elapsed())
}

fn note(pane: Uuid, sleeping: bool) {
    if let Ok(mut set) = asleep().lock() {
        if sleeping {
            set.insert(pane);
        } else if set.remove(&pane) {
            if let Ok(mut woken) = woken().lock() {
                woken.insert(pane, std::time::Instant::now());
            }
        }
    }
}

/// "1.2 GB" / "640 MB".
pub(crate) fn size_label(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", bytes >> 20)
    }
}

/// "Put 3 agents to sleep, freeing about 2.4 GB. …", and how many of the
/// `chosen` had started on something since they were offered.
fn sleep_notice(slept: usize, chosen: usize, freed: u64) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    if slept == 0 {
        return "No agent was put to sleep: they are in use again.".into();
    }
    let busy = chosen.saturating_sub(slept);
    let rest = match busy {
        0 => String::new(),
        n => format!(" {n} stayed awake: in use again."),
    };
    format!(
        "Put {slept} agent{} to sleep, freeing about {}. Open a tab to wake it.{rest}",
        plural(slept),
        size_label(freed)
    )
}

/// How many agents may be offered for sleep: those beyond the ones that
/// always stay awake.
fn may_offer(awake: usize) -> usize {
    awake.saturating_sub(ALWAYS_AWAKE)
}

/// Whether the person may be asked, given how long ago they said to keep
/// agents awake.
fn may_ask(since_declined: Option<Duration>, start_waits: bool) -> bool {
    let again = if start_waits { ASK_AGAIN_WAITING } else { ASK_AGAIN };
    since_declined.is_none_or(|since| since >= again)
}

fn declined() -> &'static Mutex<Option<std::time::Instant>> {
    static DECLINED: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    DECLINED.get_or_init(Default::default)
}

/// "22 min" / "1 h 5 min".
fn idle_label(idle: Duration) -> String {
    let minutes = idle.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{} min", m.max(1)),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The question "put these idle agents to sleep?".
#[derive(Clone, Debug, Default)]
pub(crate) struct SleepPrompt {
    agents: Vec<OfferedAgent>,
    /// The ones the person unticked.
    keep: HashSet<Uuid>,
    /// A new agent waits for the memory.
    start_waits: bool,
    short: bool,
}

#[derive(Clone, Debug)]
struct OfferedAgent {
    pane: Uuid,
    provider: String,
    title: String,
    place: String,
    idle: Duration,
    memory: u64,
}

impl SleepPrompt {
    fn chosen(&self) -> impl Iterator<Item = &OfferedAgent> {
        self.agents.iter().filter(|agent| !self.keep.contains(&agent.pane))
    }
}

/// Why agents are being looked at.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
enum Sweep {
    /// The regular look: finds what to ask about.
    Look,
    /// The person said yes to these.
    Chosen(HashSet<Uuid>),
    /// "Sleep workspace": its tabs, and how many agents they hold.
    Workspace(HashSet<usize>, usize),
}

/// What "Sleep workspace" did: how many of the workspace's `agents` slept,
/// and why the rest didn't.
pub(crate) fn stayed_awake_notice(slept: usize, freed: u64, agents: usize) -> String {
    let awake = agents.saturating_sub(slept);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let why = "working, running something in the background, waiting for you, on screen, coordinating a team, or can't be resumed";
    match (slept, awake) {
        (0, _) => format!("No agent was put to sleep: {awake} agent{} {} ({why}).", plural(awake), if awake == 1 { "is" } else { "are" }),
        (_, 0) => format!("Put {slept} agent{} to sleep, freeing about {}. Open a tab to wake it.", plural(slept), size_label(freed)),
        _ => format!(
            "Put {slept} agent{} to sleep, freeing about {}. {awake} stayed awake ({why}).",
            plural(slept),
            size_label(freed)
        ),
    }
}

/// The idle time required before an agent may sleep.
pub(crate) fn idle_before_sleep(minutes: u32, short_of_memory: bool, asked: bool) -> Option<Duration> {
    if asked {
        // The person chose "Sleep idle agents": no waiting.
        return Some(Duration::ZERO);
    }
    if minutes == 0 {
        return None;
    }
    let configured = Duration::from_secs(u64::from(minutes) * 60);
    Some(if short_of_memory { configured.min(SHORT_OF_MEMORY_IDLE) } else { configured })
}

/// [`idle_before_sleep`], and how long while a start waits for memory: two
/// minutes, whatever the setting, so an agent that just finished isn't put
/// to sleep the moment it stops.
pub(crate) fn required_idle(
    minutes: u32,
    short_of_memory: bool,
    asked: bool,
    start_waits: bool,
) -> Option<Duration> {
    let required = idle_before_sleep(minutes, short_of_memory, asked)?;
    if start_waits && short_of_memory && !asked {
        return Some(SHORT_OF_MEMORY_IDLE);
    }
    Some(required)
}

/// One agent the UI thread found eligible; the rest is checked off it.
struct Candidate {
    tab_id: usize,
    pane: Uuid,
    /// How long nothing has been typed into the pane.
    quiet: Duration,
}

/// An agent confirmed idle, with the memory its processes hold.
#[derive(Default)]
struct Sleeper {
    tab_id: usize,
    pane: Uuid,
    idle: Duration,
    memory: u64,
}

/// Drops the candidates a team needs awake: its lead and its running
/// workers (`team::panes_to_keep_awake`). Pure so the rule can be tested.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
fn spare_kept_awake(candidates: Vec<Candidate>, keep: &HashSet<Uuid>) -> Vec<Candidate> {
    candidates.into_iter().filter(|candidate| !keep.contains(&candidate.pane)).collect()
}

/// Splits wake requests by the window and tab that own each pane:
/// `(window, tab, pane, reason)` for owned panes, and the rest, owned by no
/// window (its tab was closed), to drop. Pure so the rule can be tested.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub(crate) fn route_wake_requests<W: Copy>(
    requests: Vec<(Uuid, String)>,
    owned: &std::collections::HashMap<Uuid, (W, usize)>,
) -> (Vec<(W, usize, Uuid, String)>, Vec<(Uuid, String)>) {
    let mut routed = Vec::new();
    let mut dropped = Vec::new();
    for (pane, reason) in requests {
        match owned.get(&pane) {
            Some((window, tab_id)) => routed.push((*window, *tab_id, pane, reason)),
            None => dropped.push((pane, reason)),
        }
    }
    (routed, dropped)
}

/// Whether a candidate may sleep, from what is known off the UI thread.
/// Pure so the rules can be tested.
pub(crate) fn may_sleep(
    resumable: bool,
    between_turns: bool,
    idle: Duration,
    required: Duration,
    found: bool,
    listening: Option<bool>,
    background: bool,
) -> bool {
    resumable && between_turns && idle >= required && found && listening == Some(false) && !background
}

impl WorkspaceView {
    /// Start looking for idle agents (once, when the window is built).
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn start_agent_sleep(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(SWEEP_EVERY), |view, _, ctx| {
            view.sweep_idle_agents(ctx);
            view.start_agent_sleep(ctx);
        });
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn start_agent_sleep(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Pane-tab ids on screen in this window.
    fn visible_tab_ids(&self) -> HashSet<usize> {
        let in_surface: HashSet<usize> = self.tabs.iter().map(|tab| tab.id).collect();
        super::desktop_catalog::visible_pane_ids(self.split_tree.as_ref(), self.active_tab_id)
            .into_iter()
            .filter(|id| in_surface.contains(id))
            .collect()
    }

    /// Look for agents idle long enough, and ask about them.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn sweep_idle_agents(&mut self, ctx: &mut ViewContext<Self>) {
        let start_waits = !self.agent_resources.waiting.is_empty();
        let since = declined().lock().ok().and_then(|at| at.map(|at| at.elapsed()));
        if !may_ask(since, start_waits) || self.sleep_prompt_open(ctx) {
            return;
        }
        self.sweep_agents(Sweep::Look, ctx);
    }

    /// Whether any window is asking already.
    #[cfg(not(target_family = "wasm"))]
    fn sleep_prompt_open(&self, ctx: &mut ViewContext<Self>) -> bool {
        let this_window = ctx.window_id();
        if self.agent_sleep_prompt.is_some() {
            return true;
        }
        let others: Vec<_> = ctx.window_ids().filter(|window| *window != this_window).collect();
        others.into_iter().any(|window| {
            let workspace = ctx
                .root_view::<crate::root_view::RootView>(window)
                .and_then(|root| root.read(ctx, |root, _| root.workspace()));
            workspace.is_some_and(|workspace| workspace.read(ctx, |other, _| other.agent_sleep_prompt.is_some()))
        })
    }

    /// "Sleep workspace": put the workspace's agents to sleep now, without
    /// waiting for them to be idle. Agents that are working or waiting for
    /// an answer, that are on screen, or whose conversation couldn't be
    /// resumed stay awake, and the notice says so.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn sleep_workspace(&mut self, workspace_id: usize, ctx: &mut ViewContext<Self>) {
        let Some(workspace) = self.workspaces.iter().find(|workspace| workspace.id == workspace_id) else {
            return;
        };
        let mut tabs = HashSet::new();
        for surface in &workspace.tab_ids {
            if *surface == self.active_workspace_tab_id {
                tabs.extend(self.tabs.iter().map(|tab| tab.id));
            } else if let Some(parked) = self.parked_workspace_tabs.iter().find(|tab| tab.id == *surface) {
                tabs.extend(parked.panes.iter().map(|tab| tab.id));
            }
        }
        // Agent tabs that could sleep at all (CLI agents, awake, local).
        let agents = self
            .host_owned_tabs
            .iter()
            .filter(|(tab_id, owned)| tabs.contains(tab_id) && owned.session_id.is_some() && owned.remote_host.is_none())
            .filter(|(_, owned)| {
                owned.pane.read(ctx, |view, _| {
                    view.agent_id.as_deref().is_some_and(|id| id != "dragent") && !is_pane_asleep(view.pane_uuid)
                })
            })
            .count();
        if agents == 0 {
            self.show_agent_sleep_notice("No running agents in this workspace to put to sleep.".into(), ctx);
            return;
        }
        self.sweep_agents(Sweep::Workspace(tabs, agents), ctx);
    }

    /// Find the agents that may sleep, then ask about them or, when the
    /// person already said so, put them to sleep.
    #[cfg(not(target_family = "wasm"))]
    fn sweep_agents(&mut self, sweep: Sweep, ctx: &mut ViewContext<Self>) {
        if self.agent_sleep_sweeping {
            // A yes is never lost to a look that happens to be under way.
            if let Sweep::Chosen(panes) = sweep {
                ctx.spawn(Timer::after(Duration::from_secs(1)), move |view, _, ctx| {
                    view.sweep_agents(Sweep::Chosen(panes), ctx);
                });
            }
            return;
        }
        let minutes = crate::settings::AppSettings::global().agent.idle_sleep_minutes;
        if minutes == 0 && matches!(sweep, Sweep::Look) {
            return;
        }
        let visible = self.visible_tab_ids();
        let candidates: Vec<Candidate> = self
            .host_owned_tabs
            .iter()
            .filter(|(tab_id, owned)| {
                owned.session_id.is_some()
                    && owned.remote_host.is_none()
                    && !visible.contains(tab_id)
                    && match &sweep {
                        Sweep::Workspace(tabs, _) => tabs.contains(tab_id),
                        _ => true,
                    }
            })
            .filter_map(|(tab_id, owned)| {
                let session = owned.session_id.clone()?;
                // In use on the phone.
                if matches!(
                    crate::realtime::phone_control::get(&session),
                    Some(crate::realtime::phone_control::Control::Phone { .. })
                ) {
                    return None;
                }
                owned.pane.read(ctx, |view, _| {
                    let cli_agent = view.agent_id.as_deref().is_some_and(|id| id != "dragent");
                    let typed = view.manager.writer().input_quiet_for()?;
                    let quiet = since_woken(view.pane_uuid).map_or(typed, |woke| woke.min(typed));
                    // Drawing output: working, whatever its reports say.
                    let drawing = crate::terminal::output_activity::busy(view.pane_uuid) == Some(true);
                    let wanted = match &sweep {
                        Sweep::Chosen(panes) => panes.contains(&view.pane_uuid),
                        _ => true,
                    };
                    (cli_agent && wanted && !drawing && !is_pane_asleep(view.pane_uuid)).then_some(Candidate {
                        tab_id: *tab_id,
                        pane: view.pane_uuid,
                        quiet,
                    })
                })
            })
            // A team's worker waits for its lead's messages: it stays awake.
            .filter(|candidate| {
                !self
                    .cli_workers
                    .values()
                    .any(|worker| worker.pane == candidate.pane && !worker.done)
            })
            .collect();
        if candidates.is_empty() {
            match sweep {
                Sweep::Workspace(_, agents) => {
                    self.show_agent_sleep_notice(stayed_awake_notice(0, 0, agents), ctx)
                }
                Sweep::Chosen(panes) => {
                    self.show_agent_sleep_notice(sleep_notice(0, panes.len(), 0), ctx)
                }
                Sweep::Look => {}
            }
            return;
        }
        let look = matches!(sweep, Sweep::Look);
        let chosen = matches!(sweep, Sweep::Chosen(_));
        let waiting: HashSet<String> =
            self.agent_resources.waiting.iter().map(|agent| agent.id.clone()).collect();
        self.agent_sleep_sweeping = true;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let memory = dragent_host::agent_sleep::memory();
            let short = memory.pressure >= dragent_host::agent_sleep::Pressure::Elevated;
            let required = if look {
                required_idle(minutes, short, false, !waiting.is_empty())
            } else if chosen {
                // Not one that finished a moment ago.
                Some(SHORT_OF_MEMORY_IDLE)
            } else {
                Some(Duration::ZERO)
            };
            // The person asked for a whole workspace: how many run elsewhere
            // doesn't matter. Otherwise some always stay.
            let most = if look || chosen {
                may_offer(dragent_host::agent_sleep::agents_awake())
            } else {
                usize::MAX
            };
            let (Some(required), true) = (required, most > 0) else {
                let _ = tx.send((Vec::new(), short));
                return;
            };
            // A team's lead and its running workers stay awake: it needs them.
            let candidates = spare_kept_awake(candidates, &dragent_host::team::panes_to_keep_awake());
            let panes: Vec<Uuid> = candidates.iter().map(|c| c.pane).collect();
            let processes = dragent_host::agent_sleep::agent_processes(&panes);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let mut sleepers: Vec<Sleeper> = candidates
                .into_iter()
                // One that hasn't started holds nothing.
                .filter(|candidate| !waiting.contains(&candidate.pane.to_string()))
                .filter_map(|candidate| {
                    use dragent_host::agent_sessions as sessions;
                    let record = sessions::load(candidate.pane).filter(|r| r.active)?;
                    let resumable = sessions::resume_command(&record, &record.cwd, "sh").is_some();
                    // Idle since the agent's last report and the last keystroke.
                    let reported = sessions::load_activity(candidate.pane)
                        .map(|activity| Duration::from_millis(now_ms.saturating_sub(activity.at_ms)))
                        .unwrap_or(candidate.quiet);
                    let idle = reported.min(candidate.quiet);
                    let found = processes.get(&candidate.pane).copied().unwrap_or_default();
                    may_sleep(
                        resumable,
                        sessions::is_resting(candidate.pane),
                        idle,
                        required,
                        found.found,
                        found.listening,
                        found.background,
                    )
                    .then_some(Sleeper {
                        tab_id: candidate.tab_id,
                        pane: candidate.pane,
                        idle,
                        memory: found.memory,
                    })
                })
                .collect();
            sleepers.sort_by(|a, b| b.idle.cmp(&a.idle));
            sleepers.truncate(most);
            let _ = tx.send((sleepers, short));
        });
        ctx.spawn(rx, move |view, result, ctx| {
            view.agent_sleep_sweeping = false;
            let (mut sleepers, short) = result.unwrap_or_default();
            // Opened since it was found idle: it stays awake.
            let visible = view.visible_tab_ids();
            sleepers.retain(|sleeper| !visible.contains(&sleeper.tab_id));
            if let Sweep::Look = sweep {
                view.ask_to_sleep(sleepers, short, ctx);
                return;
            }
            let mut count = 0usize;
            let mut freed = 0u64;
            for sleeper in sleepers {
                if view.put_agent_to_sleep(sleeper.tab_id, sleeper.pane, ctx) {
                    count += 1;
                    freed += sleeper.memory;
                }
            }
            if count > 0 {
                view.save_session(ctx);
                log::info!(
                    "[agent-sleep] put {count} idle agent(s) to sleep, freeing about {}",
                    size_label(freed)
                );
            }
            let notice = match sweep {
                Sweep::Workspace(_, agents) => stayed_awake_notice(count, freed, agents),
                Sweep::Chosen(panes) => sleep_notice(count, panes.len(), freed),
                Sweep::Look => return,
            };
            view.show_agent_sleep_notice(notice, ctx);
            ctx.notify();
        });
    }

    /// Ask whether these may sleep.
    #[cfg(not(target_family = "wasm"))]
    fn ask_to_sleep(&mut self, sleepers: Vec<Sleeper>, short: bool, ctx: &mut ViewContext<Self>) {
        if sleepers.is_empty() || self.sleep_prompt_open(ctx) {
            return;
        }
        let agents = sleepers
            .into_iter()
            .filter_map(|sleeper| {
                let owned = self.host_owned_tabs.get(&sleeper.tab_id)?;
                let provider = owned.pane.read(ctx, |view, _| view.agent_id.clone()).unwrap_or_default();
                let (title, place) = self.sleeper_place(sleeper.tab_id);
                Some(OfferedAgent {
                    pane: sleeper.pane,
                    provider,
                    title,
                    place,
                    idle: sleeper.idle,
                    memory: sleeper.memory,
                })
            })
            .collect();
        self.agent_sleep_prompt = Some(SleepPrompt {
            agents,
            keep: HashSet::new(),
            start_waits: !self.agent_resources.waiting.is_empty(),
            short,
        });
        ctx.notify();
    }

    /// A tab's name and the workspace it is in.
    #[cfg(not(target_family = "wasm"))]
    fn sleeper_place(&self, tab_id: usize) -> (String, String) {
        let here = self.tabs.iter().find(|tab| tab.id == tab_id).map(|tab| (tab, self.active_workspace_tab_id));
        let found = here.or_else(|| {
            self.parked_workspace_tabs.iter().find_map(|surface| {
                surface.panes.iter().find(|tab| tab.id == tab_id).map(|tab| (tab, surface.id))
            })
        });
        let Some((tab, surface)) = found else {
            return (String::new(), String::new());
        };
        let place = self
            .workspaces
            .iter()
            .find(|workspace| workspace.tab_ids.contains(&surface))
            .map(|workspace| workspace.title.clone())
            .unwrap_or_default();
        (tab.custom_label.clone().unwrap_or_else(|| tab.title.clone()), place)
    }

    pub(crate) fn handle_sleep_prompt_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match action {
            WorkspaceAction::ToggleSleepAgent { pane } => {
                if let Some(prompt) = &mut self.agent_sleep_prompt {
                    if !prompt.keep.remove(pane) {
                        prompt.keep.insert(*pane);
                    }
                }
            }
            WorkspaceAction::ConfirmSleepAgents => {
                let Some(prompt) = &self.agent_sleep_prompt else {
                    return;
                };
                let panes: HashSet<Uuid> = prompt.chosen().map(|agent| agent.pane).collect();
                if panes.is_empty() {
                    return;
                }
                self.agent_sleep_prompt = None;
                // Those kept aren't offered again straight away.
                if let Ok(mut at) = declined().lock() {
                    *at = Some(std::time::Instant::now());
                }
                #[cfg(not(target_family = "wasm"))]
                self.sweep_agents(Sweep::Chosen(panes), ctx);
            }
            WorkspaceAction::KeepAgentsAwake => {
                self.agent_sleep_prompt = None;
                if let Ok(mut at) = declined().lock() {
                    *at = Some(std::time::Instant::now());
                }
            }
            _ => {}
        }
        ctx.notify();
    }

    /// "Put these agents to sleep?", over the window. It takes no keys, so
    /// typing carries on and Enter never answers it.
    pub(crate) fn overlay_sleep_prompt(&self, root: Box<dyn Element>, theme: &Theme) -> Box<dyn Element> {
        use crate::machine_ui::{self as mui, Size, Variant};
        use warpui::elements::{CrossAxisAlignment, Expanded, Flex, Hoverable};
        let Some(prompt) = &self.agent_sleep_prompt else {
            return root;
        };
        let (ui, id) = (self.ui_font, self.workspace_view_id);
        let hover = |key: String| {
            self.new_tab_picker_mouse_states
                .borrow_mut()
                .entry(format!("sleep_prompt_{key}"))
                .or_default()
                .clone()
        };
        let why = if prompt.start_waits {
            "A new agent is waiting for memory. These have finished and aren't in use."
        } else if prompt.short {
            "Memory is running low. These have finished and aren't in use."
        } else {
            "These have finished and haven't been used for a while. Sleeping frees their memory."
        };
        let mut rows = Vec::new();
        for agent in &prompt.agents {
            let on = !prompt.keep.contains(&agent.pane);
            let known = super::action::PromptAgent::ALL
                .into_iter()
                .find(|known| known.command_key() == agent.provider);
            let name = known.map(|known| known.label().to_string()).unwrap_or_else(|| agent.provider.clone());
            let mut detail: Vec<String> = Vec::new();
            if !agent.place.is_empty() && agent.place != agent.title {
                detail.push(agent.place.clone());
            }
            detail.push(format!("idle {}", idle_label(agent.idle)));
            detail.push(size_label(agent.memory));
            let title = if agent.title.is_empty() { name } else { agent.title.clone() };
            let pane = agent.pane;
            let theme = *theme;
            let detail = detail.join(" · ");
            rows.push(
                Hoverable::new(hover(format!("row_{pane}")), move |state| {
                    let mut row = Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_spacing(12.0);
                    if let Some(known) = known {
                        row = row.with_child(super::agent_image(known, &theme, 18.0, 1.0));
                    }
                    let row = row
                        .with_child(
                            Expanded::new(
                                1.0,
                                Flex::column()
                                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                    .with_spacing(3.0)
                                    .with_child(mui::line(ui, title.clone(), 13.0, theme.main_text()))
                                    .with_child(mui::line(ui, detail.clone(), 11.5, theme.sub_text()))
                                    .finish(),
                            )
                            .finish(),
                        )
                        .with_child(if on {
                            crate::icons::render_icon(crate::icons::AppIcon::CircleCheck, theme.accent, 17.0)
                        } else {
                            ConstrainedBox::new(
                                Container::new(warpui::elements::Empty::new().finish())
                                    .with_border(Border::all(1.5).with_border_color(theme.disabled_text()))
                                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.5)))
                                    .finish(),
                            )
                            .with_width(15.0)
                            .with_height(15.0)
                            .finish()
                        })
                        .finish();
                    let mut cell = Container::new(row)
                        .with_padding_left(10.0)
                        .with_padding_right(10.0)
                        .with_padding_top(9.0)
                        .with_padding_bottom(9.0)
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)));
                    if state.is_hovered() {
                        cell = cell.with_background_color(theme.surface_2());
                    }
                    cell.finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action_for(id, WorkspaceAction::ToggleSleepAgent { pane })
                })
                .finish(),
            );
        }
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(2.0);
        for row in rows {
            list = list.with_child(row);
        }
        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(mui::text(ui, why, 12.5, theme.sub_text()))
            .with_child(list.finish())
            .with_child(mui::text(
                ui,
                "Open an agent's tab when you need it: it starts again on the same conversation.",
                12.0,
                theme.disabled_text(),
            ))
            .finish();
        let (count, memory) = prompt.chosen().fold((0usize, 0u64), |(n, m), agent| (n + 1, m + agent.memory));
        let free = (count > 0).then(|| mui::text(ui, format!("Frees about {}", size_label(memory)), 12.0, theme.sub_text()));
        let footer = mui::dialog_footer(
            free,
            vec![
                mui::button(theme, ui, id, hover("keep".into()), None, "Keep awake", Variant::Secondary, Size::Md, true, WorkspaceAction::KeepAgentsAwake),
                mui::button(
                    theme,
                    ui,
                    id,
                    hover("sleep".into()),
                    None,
                    &if count == prompt.agents.len() { "Put to sleep".to_string() } else { format!("Put {count} to sleep") },
                    Variant::Primary,
                    Size::Md,
                    count > 0,
                    WorkspaceAction::ConfirmSleepAgents,
                ),
            ],
        );
        let title = if prompt.agents.len() == 1 { "Put an idle agent to sleep?" } else { "Put idle agents to sleep?" };
        let dialog = mui::dialog::<WorkspaceAction>(
            theme,
            ui,
            id,
            hover("unused".into()),
            &self.agent_sleep_scroll,
            mui::dialog_title(theme, ui, crate::icons::AppIcon::Clock, title, None),
            body,
            Some(footer),
            500.0,
            None,
        );
        let mut stack = warpui::elements::Stack::new().with_child(root);
        stack.add_overlay_child(dialog);
        stack.finish()
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn sleep_workspace(&mut self, _workspace_id: usize, _ctx: &mut ViewContext<Self>) {}

    /// Stop a tab's agent process; the tab stays and wakes it when opened.
    #[cfg(not(target_family = "wasm"))]
    fn put_agent_to_sleep(&mut self, tab_id: usize, pane_uuid: Uuid, ctx: &mut ViewContext<Self>) -> bool {
        let Some(owned) = self.host_owned_tabs.get_mut(&tab_id) else {
            return false;
        };
        let Some(session_id) = owned.session_id.take() else {
            return false;
        };
        if let Some(cancel) = owned.stream.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        owned.slot.unbind();
        let workspace_id = owned.workspace_id.clone();
        let client = owned.client.clone();
        let pane = owned.pane.clone();
        note(pane_uuid, true);
        crate::bg::detach(move || {
            // Marked first: a restart in between leaves it asleep, not lost.
            if let Err(error) = dragent_host::agent_sessions::set_asleep(pane_uuid, true) {
                log::warn!("[agent-sleep] couldn't record sleep for {pane_uuid}: {error}");
            }
            match client
                .map(Ok)
                .unwrap_or_else(dragent_host::HostClient::ensure_host)
                .and_then(|client| client.close_terminal(&workspace_id, &session_id))
            {
                Ok(_) => {}
                Err(error) => log::warn!("[agent-sleep] stopping {pane_uuid} failed: {error}"),
            }
        });
        ctx.update_view(&pane, |terminal, terminal_ctx| {
            let theme = terminal.theme;
            terminal.model.update(terminal_ctx, |model, model_ctx| {
                model.end_running_command(&theme, model_ctx);
            });
            terminal.set_host_notice(Some(SLEEPING_NOTICE.to_string()), terminal_ctx);
        });
        // The caller saves the session once for the whole sweep: saving
        // serialises every tab on the main thread.
        true
    }

    /// A restored tab whose agent was asleep before the restart.
    pub(crate) fn mark_tab_asleep(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) {
        let Some(pane) = self.host_owned_tabs.get(&tab_id).map(|owned| owned.pane.clone()) else {
            return;
        };
        let pane_uuid = pane.read(ctx, |view, _| view.pane_uuid);
        note(pane_uuid, true);
        ctx.update_view(&pane, |terminal, terminal_ctx| {
            terminal.set_host_notice(Some(SLEEPING_NOTICE.to_string()), terminal_ctx);
        });
    }

    /// Wake every sleeping agent whose tab is on screen. Cheap when none is
    /// asleep; called after every workspace action.
    pub(crate) fn wake_visible_sleepers(&mut self, ctx: &mut ViewContext<Self>) {
        if !any_asleep() {
            return;
        }
        for tab_id in self.visible_tab_ids() {
            self.wake_agent(tab_id, ctx);
        }
    }

    /// Wake one tab's agent, if it is asleep: resume its conversation in a
    /// new process in the same pane.
    pub(crate) fn wake_agent(&mut self, tab_id: usize, ctx: &mut ViewContext<Self>) -> bool {
        let Some(pane) = self.host_owned_tabs.get(&tab_id).map(|owned| owned.pane.clone()) else {
            return false;
        };
        let pane_uuid = pane.read(ctx, |view, _| view.pane_uuid);
        if !is_pane_asleep(pane_uuid) {
            return false;
        }
        note(pane_uuid, false);
        let plan = self.host_recovery_plan(&pane, ctx);
        let used = dragent_host::agent_sessions::load(pane_uuid).is_some_and(|record| record.used);
        let command = if used { plan.resume.clone() } else { plan.fresh.clone() };
        let Some(command) = command else {
            log::warn!("[agent-sleep] cannot wake {}: no way to resume it", plan.name);
            ctx.update_view(&pane, |terminal, terminal_ctx| {
                terminal.set_host_notice(
                    Some(format!("{} couldn't be woken: its conversation can't be resumed.", plan.name)),
                    terminal_ctx,
                );
            });
            return false;
        };
        let notice = format!("Waking {}…", plan.name);
        ctx.update_view(&pane, |terminal, terminal_ctx| {
            terminal.set_host_notice(Some(notice), terminal_ctx);
        });
        #[cfg(not(target_family = "wasm"))]
        {
            let (tx, rx) = futures::channel::oneshot::channel();
            crate::bg::detach(move || {
                if let Err(error) = dragent_host::agent_sessions::set_asleep(pane_uuid, false) {
                    log::warn!("[agent-sleep] couldn't clear sleep for {pane_uuid}: {error}");
                }
                let _ = tx.send(());
            });
            ctx.spawn(rx, move |view, _, ctx| {
                view.replace_pane_session(tab_id, Some(command), ctx);
            });
        }
        true
    }

    /// Start looking for wake requests (once, when the window is built).
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn start_wake_requests(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(WAKE_EVERY), |view, _, ctx| {
            view.poll_wake_requests(ctx);
            view.start_wake_requests(ctx);
        });
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn start_wake_requests(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// Wake the sleeping agents the team courier asked for. The request
    /// files are read off the main thread, and only while an agent sleeps.
    /// One window (the lowest id with a workspace) takes the requests for
    /// the whole app and hands each to the window owning its pane; a request
    /// no window owns (its tab was closed) is dropped.
    #[cfg(not(target_family = "wasm"))]
    fn poll_wake_requests(&mut self, ctx: &mut ViewContext<Self>) {
        if !any_asleep() {
            return;
        }
        let this_window = ctx.window_id();
        let workspaces: Vec<(warpui::WindowId, warpui::ViewHandle<WorkspaceView>)> = ctx
            .window_ids()
            .filter(|window| *window != this_window)
            .filter_map(|window| {
                let root = ctx.root_view::<crate::root_view::RootView>(window)?;
                root.read(ctx, |root, _| root.workspace()).map(|workspace| (window, workspace))
            })
            .collect();
        if workspaces.iter().any(|(window, _)| *window < this_window) {
            return; // Another window takes the requests.
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let _ = tx.send(dragent_host::agent_sessions::take_wake_requests());
        });
        ctx.spawn(rx, move |view, requests, ctx| {
            let requests = requests.unwrap_or_default();
            if requests.is_empty() {
                return;
            }
            let mut owned: std::collections::HashMap<Uuid, (warpui::WindowId, usize)> = view
                .host_owned_tabs
                .iter()
                .map(|(tab_id, owned)| (owned.pane.read(ctx, |pane, _| pane.pane_uuid), (this_window, *tab_id)))
                .collect();
            for (window, workspace) in &workspaces {
                let tabs: Vec<_> = workspace.read(ctx, |other, _| {
                    other.host_owned_tabs.iter().map(|(tab_id, owned)| (*tab_id, owned.pane.clone())).collect()
                });
                for (tab_id, pane) in tabs {
                    owned.insert(pane.read(ctx, |pane, _| pane.pane_uuid), (*window, tab_id));
                }
            }
            let (routed, dropped) = route_wake_requests(requests, &owned);
            for (pane, reason) in dropped {
                log::info!("[agent-sleep] dropped wake request for {pane} ({reason}): no window has its tab");
            }
            for (window, tab_id, pane, reason) in routed {
                // Awake already (or woken by someone else): nothing to do.
                if !is_pane_asleep(pane) {
                    continue;
                }
                log::info!("[agent-sleep] waking {pane}: {reason}");
                let woken = if window == this_window {
                    view.wake_agent(tab_id, ctx)
                } else if let Some((_, workspace)) = workspaces.iter().find(|(w, _)| *w == window) {
                    ctx.update_view(workspace, |other, other_ctx| other.wake_agent(tab_id, other_ctx))
                } else {
                    false
                };
                if !woken {
                    log::warn!("[agent-sleep] couldn't wake {pane}: its conversation can't be resumed");
                    view.show_agent_sleep_notice(
                        "A team's agent couldn't be woken: its conversation can't be resumed.".into(),
                        ctx,
                    );
                }
            }
        });
    }

    /// Wake a tab's agent asked for by its window-local tab id (the phone).
    pub(crate) fn wake_agent_for_phone(&mut self, local_tab_id: u64, ctx: &mut ViewContext<Self>) -> bool {
        self.wake_agent(local_tab_id as usize, ctx)
    }

    fn show_agent_sleep_notice(&mut self, text: String, ctx: &mut ViewContext<Self>) {
        self.agent_sleep_notice = Some(text);
        self.agent_sleep_notice_epoch = self.agent_sleep_notice_epoch.wrapping_add(1);
        let epoch = self.agent_sleep_notice_epoch;
        ctx.spawn(Timer::after(NOTICE_FOR), move |view, _, ctx| {
            if view.agent_sleep_notice_epoch == epoch {
                view.agent_sleep_notice = None;
                ctx.notify();
            }
        });
        ctx.notify();
    }

    /// The sleep notice, over the window.
    pub(crate) fn overlay_agent_sleep(&self, root: Box<dyn Element>, theme: &Theme) -> Box<dyn Element> {
        let mut stack = warpui::elements::Stack::new().with_child(root);
        if let Some(text) = &self.agent_sleep_notice {
            let pill = Container::new(
                Text::new(text.clone(), self.ui_font, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(8.0)
            .with_padding_bottom(8.0)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish();
            // Only the notice's own size: a full-window overlay layer would
            // cover the whole app and swallow every click until it went away.
            stack.add_positioned_overlay_child(
                ConstrainedBox::new(pill).with_max_width(520.0).finish(),
                warpui::elements::OffsetPositioning::offset_from_parent(
                    warpui::geometry::vector::vec2f(0.0, -56.0),
                    warpui::elements::ParentOffsetBounds::WindowByPosition,
                    warpui::elements::ParentAnchor::BottomMiddle,
                    warpui::elements::ChildAnchor::BottomMiddle,
                ),
            );
        }
        stack.finish()
    }
}

/// What a sleeping agent's pane says.
const SLEEPING_NOTICE: &str = "Sleeping to save memory. It wakes on the same conversation when you open this tab.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleeping_a_workspace_says_what_happened() {
        let gb = 1_500_000_000;
        assert!(stayed_awake_notice(2, gb, 2).starts_with("Put 2 agents to sleep, freeing about"));
        let mixed = stayed_awake_notice(1, gb, 3);
        assert!(mixed.starts_with("Put 1 agent to sleep") && mixed.contains("2 stayed awake"), "{mixed}");
        assert!(stayed_awake_notice(0, 0, 1).starts_with("No agent was put to sleep: 1 agent is"));
    }

    #[test]
    fn the_idle_time_follows_the_setting_memory_and_the_persons_choice() {
        let min = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(idle_before_sleep(15, false, false), Some(min(15)));
        assert_eq!(idle_before_sleep(60, true, false), Some(min(2)), "short of memory: sooner");
        assert_eq!(idle_before_sleep(1, true, false), Some(min(1)), "never later than the setting");
        assert_eq!(idle_before_sleep(0, true, false), None, "Never means never on its own");
        assert_eq!(idle_before_sleep(0, false, true), Some(Duration::ZERO), "but the person can ask");
    }

    #[test]
    fn only_an_idle_resumable_agent_with_nothing_serving_sleeps() {
        let idle = Duration::from_secs(20 * 60);
        let required = Duration::from_secs(15 * 60);
        assert!(may_sleep(true, true, idle, required, true, Some(false), false));
        assert!(!may_sleep(false, true, idle, required, true, Some(false), false), "can't resume exactly");
        assert!(!may_sleep(true, false, idle, required, true, Some(false), false), "working or asking");
        assert!(!may_sleep(true, true, Duration::from_secs(60), required, true, Some(false), false), "not idle long enough");
        assert!(!may_sleep(true, true, idle, required, false, Some(false), false), "no process found");
        assert!(!may_sleep(true, true, idle, required, true, Some(true), false), "a server it started still runs");
        assert!(!may_sleep(true, true, idle, required, true, None, false), "can't tell what it serves");
        assert!(!may_sleep(true, true, idle, required, true, Some(false), true), "a build it started still runs");
    }

    #[test]
    fn a_waiting_start_makes_idle_agents_sleep_after_two_minutes() {
        let two_minutes = Duration::from_secs(2 * 60);
        // Sooner than the setting, never at once; only while memory is short.
        assert_eq!(required_idle(15, false, false, true), idle_before_sleep(15, false, false));
        assert_eq!(required_idle(15, true, false, true), Some(two_minutes));
        assert_eq!(required_idle(1, true, false, true), Some(two_minutes));
        // The person's explicit ask still means now; Never still means never.
        assert_eq!(required_idle(0, true, true, true), Some(Duration::ZERO));
        assert_eq!(required_idle(0, true, false, true), None);
        // Without a waiter, the setting (shortened when short) applies.
        assert_eq!(required_idle(15, true, false, false), idle_before_sleep(15, true, false));
        assert_eq!(required_idle(15, false, false, false), idle_before_sleep(15, false, false));
    }

    #[test]
    fn the_notice_reads_naturally() {
        assert_eq!(
            sleep_notice(1, 1, 900 << 20),
            "Put 1 agent to sleep, freeing about 900 MB. Open a tab to wake it."
        );
        assert_eq!(
            sleep_notice(2, 3, 3 << 30),
            "Put 2 agents to sleep, freeing about 3.0 GB. Open a tab to wake it. 1 stayed awake: in use again."
        );
        assert!(sleep_notice(0, 2, 0).starts_with("No agent was put to sleep"));
        assert_eq!(idle_label(Duration::from_secs(22 * 60)), "22 min");
        assert_eq!(idle_label(Duration::from_secs(65 * 60)), "1 h 5 min");
        assert_eq!(idle_label(Duration::from_secs(2 * 3600)), "2 h");
    }

    #[test]
    fn five_agents_are_always_left_running() {
        assert_eq!((may_offer(0), may_offer(5), may_offer(6), may_offer(9)), (0, 0, 1, 4));
    }

    #[test]
    fn keep_awake_is_not_asked_again_for_a_while() {
        let min = |m: u64| Some(Duration::from_secs(m * 60));
        assert!(may_ask(None, false), "never declined");
        assert!(!may_ask(min(30), false) && may_ask(min(60), false));
        assert!(!may_ask(min(5), true) && may_ask(min(10), true), "sooner while a start waits");
    }

    #[test]
    fn only_the_ticked_agents_are_chosen() {
        let agent = |pane| OfferedAgent {
            pane,
            provider: "claude".into(),
            title: String::new(),
            place: String::new(),
            idle: Duration::ZERO,
            memory: 1,
        };
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let prompt = SleepPrompt { agents: vec![agent(a), agent(b)], keep: HashSet::from([b]), ..Default::default() };
        assert_eq!(prompt.chosen().map(|agent| agent.pane).collect::<Vec<_>>(), [a]);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size_label(3 << 29), "1.5 GB");
        assert_eq!(size_label(640 << 20), "640 MB");
    }

    #[test]
    fn a_team_s_lead_and_workers_are_spared_from_sleep() {
        let (lead, worker, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let keep: HashSet<Uuid> = [lead, worker].into_iter().collect();
        let candidate = |tab_id, pane| Candidate { tab_id, pane, quiet: Duration::from_secs(3600) };
        let kept = spare_kept_awake(vec![candidate(1, lead), candidate(2, other), candidate(3, worker)], &keep);
        assert_eq!(kept.iter().map(|c| c.tab_id).collect::<Vec<_>>(), vec![2]);
        assert!(spare_kept_awake(vec![candidate(4, other)], &HashSet::new()).len() == 1);
    }

    #[test]
    fn a_wake_request_goes_to_the_window_and_tab_that_own_its_pane() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let owned = std::collections::HashMap::from([(a, (1u8, 7usize)), (b, (2u8, 9usize))]);
        let (routed, dropped) = route_wake_requests(
            vec![(a, "news".to_string()), (c, "closed tab".to_string()), (b, "more".to_string())],
            &owned,
        );
        assert_eq!(routed, vec![(1, 7, a, "news".to_string()), (2, 9, b, "more".to_string())]);
        assert_eq!(dropped, vec![(c, "closed tab".to_string())]);
    }

    #[test]
    fn a_wake_request_no_window_owns_is_dropped_not_requeued() {
        let orphan = Uuid::new_v4();
        let owned: std::collections::HashMap<Uuid, (u8, usize)> = std::collections::HashMap::new();
        let (routed, dropped) = route_wake_requests(vec![(orphan, "news".to_string())], &owned);
        assert!(routed.is_empty());
        assert_eq!(dropped, vec![(orphan, "news".to_string())]);
    }

    #[test]
    fn the_sleep_registry_tracks_panes() {
        let pane = Uuid::new_v4();
        assert!(!is_pane_asleep(pane));
        note(pane, true);
        assert!(is_pane_asleep(pane) && any_asleep());
        note(pane, false);
        assert!(!is_pane_asleep(pane));
    }
}
