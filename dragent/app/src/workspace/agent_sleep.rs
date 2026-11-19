//! Idle agents are put to sleep to free memory, and wake when opened.
//!
//! An agent CLI (opencode, Claude, Codex…) keeps its process — often near a
//! gigabyte — for as long as its tab exists, working or not. With 10–20 tabs
//! that exhausts a laptop's memory. So an agent whose turn is finished, that
//! no one has touched for a while (Settings → Agents, 15 minutes by default)
//! and that isn't on screen, has its process stopped. Its tab stays, with its
//! history; opening it (or opening it on the phone) resumes the same
//! conversation in a new process.
//!
//! Never put to sleep: an agent that is working or waiting for an answer, one
//! whose conversation can't be resumed exactly, one that left something
//! serving (a dev server it started), one coordinating with a team, one in
//! use on the phone, and the built-in Dragent agent (it has no process).
//!
//! When the machine is short of memory the idle time drops to a couple of
//! minutes. Starting an agent never asks; the sweep makes the room.

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
fn size_label(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", bytes >> 20)
    }
}

/// "Memory was low: put 3 idle agents to sleep, freeing 2.4 GB. …"
fn sleep_notice(count: usize, freed: u64, asked: bool) -> String {
    let reason = if asked { "Put" } else { "Memory was low: put" };
    let (what, them) = if count == 1 {
        ("an idle agent".to_string(), "it")
    } else {
        (format!("{count} idle agents"), "one")
    };
    format!(
        "{reason} {what} to sleep, freeing {}. Open its tab to wake {them}.",
        size_label(freed)
    )
}

/// What "Sleep workspace" did: how many of the workspace's `agents` slept,
/// and why the rest didn't.
pub(crate) fn stayed_awake_notice(slept: usize, freed: u64, agents: usize) -> String {
    let awake = agents.saturating_sub(slept);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let why = "working, waiting for you, on screen, coordinating a team, or can't be resumed";
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

/// One agent the UI thread found eligible; the rest is checked off it.
struct Candidate {
    tab_id: usize,
    pane: Uuid,
    /// How long nothing has been typed into the pane.
    quiet: Duration,
}

/// An agent confirmed idle, with the memory its processes hold.
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
) -> bool {
    resumable && between_turns && idle >= required && found && listening == Some(false)
}

impl WorkspaceView {
    /// Start looking for idle agents (once, when the window is built).
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn start_agent_sleep(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.spawn(Timer::after(SWEEP_EVERY), |view, _, ctx| {
            view.sweep_idle_agents(false, None, ctx);
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

    /// Put to sleep the agents idle long enough. `asked`: the person chose
    /// "Sleep idle agents" (no idle time needed); `then` runs afterwards.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn sweep_idle_agents(
        &mut self,
        asked: bool,
        then: Option<WorkspaceAction>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.sweep_agents(asked, then, None, ctx);
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
        self.sweep_agents(true, None, Some((tabs, agents)), ctx);
    }

    /// Put to sleep the agents idle long enough, or, with `only`, the
    /// agents among those tabs (and how many agents they hold, for saying
    /// how many stayed awake).
    #[cfg(not(target_family = "wasm"))]
    fn sweep_agents(
        &mut self,
        asked: bool,
        then: Option<WorkspaceAction>,
        only: Option<(HashSet<usize>, usize)>,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.agent_sleep_sweeping {
            if let Some(action) = then {
                self.handle_low_memory_resume(action, ctx);
            }
            return;
        }
        let minutes = crate::settings::AppSettings::global().agent.idle_sleep_minutes;
        if minutes == 0 && !asked {
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
                    && only.as_ref().is_none_or(|(tabs, _)| tabs.contains(tab_id))
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
                    (cli_agent && !drawing && !is_pane_asleep(view.pane_uuid)).then_some(Candidate {
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
            if let Some((_, agents)) = &only {
                self.show_agent_sleep_notice(stayed_awake_notice(0, 0, *agents), ctx);
            }
            if let Some(action) = then {
                self.handle_low_memory_resume(action, ctx);
            }
            return;
        }
        let manual = only.map(|(_, agents)| agents);
        self.agent_sleep_sweeping = true;
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let memory = dragent_host::agent_sleep::memory();
            let short = memory.pressure >= dragent_host::agent_sleep::Pressure::Elevated;
            let Some(required) = idle_before_sleep(minutes, short, asked) else {
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
            let _ = tx.send((sleepers, short));
        });
        ctx.spawn(rx, move |view, result, ctx| {
            view.agent_sleep_sweeping = false;
            let (sleepers, short) = result.unwrap_or_default();
            // Opened since it was found idle: it stays awake.
            let visible = view.visible_tab_ids();
            let mut count = 0usize;
            let mut freed = 0u64;
            for sleeper in sleepers {
                if visible.contains(&sleeper.tab_id) {
                    continue;
                }
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
            if let Some(agents) = manual {
                view.show_agent_sleep_notice(stayed_awake_notice(count, freed, agents), ctx);
            } else if count > 0 && (short || asked) {
                view.show_agent_sleep_notice(sleep_notice(count, freed, asked), ctx);
            }
            if let Some(action) = then {
                view.handle_low_memory_resume(action, ctx);
            }
            ctx.notify();
        });
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn sleep_workspace(&mut self, _workspace_id: usize, _ctx: &mut ViewContext<Self>) {}

    #[cfg(target_family = "wasm")]
    pub(crate) fn sweep_idle_agents(
        &mut self,
        _asked: bool,
        _then: Option<WorkspaceAction>,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

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

    /// Run the action that was waiting for idle agents to sleep.
    fn handle_low_memory_resume(&mut self, action: WorkspaceAction, ctx: &mut ViewContext<Self>) {
        use warpui::TypedActionView;
        self.handle_action(&action, ctx);
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
        assert!(may_sleep(true, true, idle, required, true, Some(false)));
        assert!(!may_sleep(false, true, idle, required, true, Some(false)), "can't resume exactly");
        assert!(!may_sleep(true, false, idle, required, true, Some(false)), "working or asking");
        assert!(!may_sleep(true, true, Duration::from_secs(60), required, true, Some(false)), "not idle long enough");
        assert!(!may_sleep(true, true, idle, required, false, Some(false)), "no process found");
        assert!(!may_sleep(true, true, idle, required, true, Some(true)), "a server it started still runs");
        assert!(!may_sleep(true, true, idle, required, true, None), "can't tell what it serves");
    }

    #[test]
    fn the_notice_reads_naturally() {
        assert_eq!(
            sleep_notice(1, 900 << 20, false),
            "Memory was low: put an idle agent to sleep, freeing 900 MB. Open its tab to wake it."
        );
        assert_eq!(
            sleep_notice(3, 3 << 30, true),
            "Put 3 idle agents to sleep, freeing 3.0 GB. Open its tab to wake one."
        );
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
