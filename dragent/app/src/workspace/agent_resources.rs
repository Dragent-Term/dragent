//! Agents that wait for memory, agents that are paused for holding too much,
//! and what each running agent holds: read from the host, shown over the
//! window (with the person's choices) and in the agents' tabs.
//!
//! The waiting itself happens in each agent's own terminal
//! (`dragent_host::start_wait`) and a pause is the host's, so nothing here
//! needs to survive the app.

use std::collections::HashMap;
use std::time::Duration;

use uuid::Uuid;
use warpui::elements::{
    ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Hoverable, MainAxisSize,
    ParentElement, Text,
};
use warpui::r#async::Timer;
use warpui::ViewContext;
use warpui_core::scene::{Border, CornerRadius, Radius};
use web_time::Instant;

use super::action::WorkspaceAction;
use super::agent_sleep::size_label;
use super::view::WorkspaceView;
use crate::theme::Theme;

const LOOK_EVERY: Duration = Duration::from_secs(3);
/// How long a row stays away after its button was pressed. An answer the host
/// gave before it acted would put it back; one still there later did not work.
const PRESSED_FOR: Duration = Duration::from_secs(10);
/// Rows shown at once; the rest are counted.
const MAX_ROWS: usize = 4;

/// What the host said last, as the window shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AgentResources {
    pub waiting: Vec<HeldAgent>,
    pub paused: Vec<HeldAgent>,
    /// What each running agent holds ("1.2 GB"), by pane.
    pub memory: HashMap<Uuid, String>,
    /// Rows whose button was pressed, and when.
    pressed: HashMap<String, Instant>,
}

/// An agent that waits or is paused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeldAgent {
    /// Its pane, else its terminal.
    pub id: String,
    /// Where a waiting agent waits.
    pub terminal: String,
    pub label: String,
    /// What a paused agent holds.
    pub holds: String,
}

impl AgentResources {
    /// A row's button was pressed: it goes away now.
    fn press(&mut self, id: &str, now: Instant) {
        self.pressed.insert(id.to_string(), now);
        self.hide_pressed(now);
    }

    /// What the host says now, without the rows just pressed.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    fn look(&self, mut found: Self, now: Instant) -> Self {
        found.pressed = self.pressed.clone();
        found.hide_pressed(now);
        found
    }

    fn hide_pressed(&mut self, now: Instant) {
        self.pressed
            .retain(|_, at| now.saturating_duration_since(*at) < PRESSED_FOR);
        let pressed = &self.pressed;
        self.waiting
            .retain(|agent| !pressed.contains_key(&agent.id));
        self.paused.retain(|agent| !pressed.contains_key(&agent.id));
    }

    /// The rows the person is asked about, paused ones (`true`) first. A
    /// team's workers aren't: they start in turn, and their lead is told
    /// when one is paused. Their tabs still say so.
    fn rows(&self, is_worker: impl Fn(&str) -> bool) -> Vec<(&HeldAgent, bool)> {
        let paused = self.paused.iter().map(|agent| (agent, true));
        let waiting = self.waiting.iter().map(|agent| (agent, false));
        paused
            .chain(waiting)
            .filter(|(agent, _)| !is_worker(&agent.id))
            .collect()
    }

    fn shows(&self, pane: Uuid) -> Option<String> {
        let id = pane.to_string();
        if self.waiting.iter().any(|agent| agent.id == id) {
            return Some("Waiting for memory".into());
        }
        if let Some(agent) = self.paused.iter().find(|agent| agent.id == id) {
            return Some(format!("Paused ({})", agent.holds));
        }
        self.memory.get(&pane).cloned()
    }
}

#[cfg(not(target_family = "wasm"))]
fn read() -> AgentResources {
    // Never starts a host: without one nothing waits or is paused by it.
    let Ok(found) =
        dragent_host::HostClient::connect_existing().and_then(|client| client.agent_resources())
    else {
        return AgentResources::default();
    };
    AgentResources {
        waiting: found
            .waiting
            .into_iter()
            .map(|start| HeldAgent {
                id: start.id,
                terminal: start.terminal_id,
                label: start.label,
                holds: String::new(),
            })
            .collect(),
        paused: found
            .paused
            .into_iter()
            .map(|agent| HeldAgent {
                id: agent.pane,
                terminal: String::new(),
                label: agent.label,
                holds: size_label(agent.bytes),
            })
            .collect(),
        memory: found
            .running
            .into_iter()
            .filter_map(|agent| Some((agent.pane.parse().ok()?, size_label(agent.bytes))))
            .collect(),
        pressed: HashMap::new(),
    }
}

/// What a row says, and what its buttons do.
fn row_text(agent: &HeldAgent, paused: bool) -> String {
    if paused {
        format!("{} is paused: it holds {}.", agent.label, agent.holds)
    } else {
        format!("{} waits for memory and starts on its own.", agent.label)
    }
}

impl WorkspaceView {
    /// Keep looking (once, when the window is built).
    #[cfg(not(target_family = "wasm"))]
    pub(super) fn start_agent_resources(&mut self, ctx: &mut ViewContext<Self>) {
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let _ = tx.send(read());
        });
        ctx.spawn(rx, |view, found, ctx| {
            let found = view
                .agent_resources
                .look(found.unwrap_or_default(), Instant::now());
            let arrived = found.waiting.len() > view.agent_resources.waiting.len();
            if view.agent_resources != found {
                view.agent_resources = found;
                ctx.notify();
            }
            // Idle agents could make room for one that waits: ask now, not
            // at the next look.
            if arrived {
                view.sweep_idle_agents(ctx);
            }
            ctx.spawn(Timer::after(LOOK_EVERY), |view, _, ctx| {
                view.start_agent_resources(ctx);
            });
        });
    }

    #[cfg(target_family = "wasm")]
    pub(super) fn start_agent_resources(&mut self, _ctx: &mut ViewContext<Self>) {}

    /// What a pane's tab says about its agent's memory.
    pub(crate) fn pane_resources(&self, pane: Uuid) -> Option<String> {
        self.agent_resources.shows(pane)
    }

    pub(crate) fn handle_agent_resources_action(
        &mut self,
        action: &WorkspaceAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            WorkspaceAction::StartAgentAnyway { id } => {
                self.agent_resources.press(id, Instant::now());
                tell_host(id.clone(), |client, id| client.start_anyway(id));
            }
            WorkspaceAction::ResumeAgent { id } => {
                self.agent_resources.press(id, Instant::now());
                tell_host(id.clone(), |client, id| client.resume_agent(id));
            }
            WorkspaceAction::StopHeldAgent { id } => {
                let closed = id.parse().is_ok_and(|pane| self.close_agent_tab(pane, ctx));
                // A waiting agent without a tab (a run the host started).
                let waits = self
                    .agent_resources
                    .waiting
                    .iter()
                    .find(|agent| agent.id == *id);
                if let Some(agent) = waits.filter(|_| !closed) {
                    tell_host(agent.terminal.clone(), close_terminal);
                    self.agent_resources.press(id, Instant::now());
                }
                if closed {
                    self.agent_resources.press(id, Instant::now());
                }
            }
            _ => {}
        }
        ctx.notify();
    }

    /// Who waits or is paused, over the window's bottom corner.
    pub(crate) fn overlay_agent_resources(
        &self,
        root: Box<dyn Element>,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let rows = self.agent_resources.rows(crate::teams::is_worker);
        if rows.is_empty() {
            return root;
        }
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(6.0);
        for (agent, paused) in rows.iter().take(MAX_ROWS) {
            list = list.with_child(self.agent_resources_row(agent, *paused, theme));
        }
        if rows.len() > MAX_ROWS {
            list = list.with_child(
                Text::new(
                    format!("and {} more", rows.len() - MAX_ROWS),
                    self.ui_font,
                    crate::theme::text_size(12.0),
                )
                .with_color(theme.sub_text())
                .finish(),
            );
        }
        let card = Container::new(list.finish())
            .with_padding_left(14.0)
            .with_padding_right(14.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .with_background_color(theme.surface_2())
            .with_border(Border::all(1.0).with_border_color(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .finish();
        let mut stack = warpui::elements::Stack::new().with_child(root);
        // Only the card's own size, so the rest of the window stays usable.
        stack.add_positioned_overlay_child(
            ConstrainedBox::new(card).with_max_width(560.0).finish(),
            warpui::elements::OffsetPositioning::offset_from_parent(
                warpui::geometry::vector::vec2f(-16.0, -40.0),
                warpui::elements::ParentOffsetBounds::WindowByPosition,
                warpui::elements::ParentAnchor::BottomRight,
                warpui::elements::ChildAnchor::BottomRight,
            ),
        );
        stack.finish()
    }

    fn agent_resources_row(
        &self,
        agent: &HeldAgent,
        paused: bool,
        theme: &Theme,
    ) -> Box<dyn Element> {
        let id = agent.id.clone();
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(8.0)
            .with_child(
                Text::new(
                    row_text(agent, paused),
                    self.ui_font,
                    crate::theme::text_size(12.5),
                )
                .with_color(theme.main_text())
                .finish(),
            );
        let (label, action) = if paused {
            ("Resume", WorkspaceAction::ResumeAgent { id: id.clone() })
        } else {
            (
                "Start anyway",
                WorkspaceAction::StartAgentAnyway { id: id.clone() },
            )
        };
        row = row
            .with_child(self.agent_resources_button(label, theme.accent, action, &agent.id))
            .with_child(self.agent_resources_button(
                "Stop",
                theme.ui_red(),
                WorkspaceAction::StopHeldAgent { id },
                &agent.id,
            ));
        row.finish()
    }

    fn agent_resources_button(
        &self,
        label: &'static str,
        color: warpui::color::ColorU,
        action: WorkspaceAction,
        key: &str,
    ) -> Box<dyn Element> {
        let (ui, view_id) = (self.ui_font, self.workspace_view_id);
        let hover = self
            .new_tab_picker_mouse_states
            .borrow_mut()
            .entry(format!("agent_resources_{label}_{key}"))
            .or_default()
            .clone();
        Box::new(
            Hoverable::new(hover, move |state| {
                let alpha = if state.is_hovered() { 28 } else { 0 };
                Container::new(
                    Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(color)
                        .finish(),
                )
                .with_padding_left(9.0)
                .with_padding_right(9.0)
                .with_padding_top(3.0)
                .with_padding_bottom(3.0)
                .with_background_color(warpui::color::ColorU { a: alpha, ..color })
                .with_border(Border::all(1.0).with_border_color(color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(7.0)))
                .finish()
            })
            .on_click(move |ectx, _, _| ectx.dispatch_typed_action_for(view_id, action.clone())),
        )
    }
}

fn tell_host(
    id: String,
    say: impl FnOnce(&dragent_host::HostClient, &str) -> Result<(), String> + Send + 'static,
) {
    #[cfg(not(target_family = "wasm"))]
    crate::bg::detach(move || {
        let said = dragent_host::HostClient::connect_existing().and_then(|c| say(&c, &id));
        if let Err(error) = said {
            log::warn!("[agent-resources] {id}: {error}");
        }
    });
    #[cfg(target_family = "wasm")]
    let _ = (id, say);
}

/// Close a terminal knowing only its id.
#[cfg(not(target_family = "wasm"))]
fn close_terminal(client: &dragent_host::HostClient, id: &str) -> Result<(), String> {
    let all = client.list_terminals("")?;
    let found = all.iter().find(|terminal| terminal.id == id);
    let found = found.ok_or("its terminal is already closed")?;
    client.close_terminal(&found.workspace_id, id).map(|_| ())
}

#[cfg(target_family = "wasm")]
fn close_terminal(_: &dragent_host::HostClient, _: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pressed_row_stays_away_until_the_host_had_time_to_act() {
        let row = HeldAgent {
            id: "one".into(),
            terminal: "t".into(),
            label: "Agent".into(),
            holds: String::new(),
        };
        let other = HeldAgent {
            id: "two".into(),
            ..row.clone()
        };
        let host = AgentResources {
            waiting: vec![row, other],
            ..Default::default()
        };
        let pressed_at = Instant::now();
        let mut shown = host.clone();
        shown.press("one", pressed_at);
        // The host's answer still names it; the other row is untouched.
        let soon = shown.look(host.clone(), pressed_at + Duration::from_secs(3));
        assert_eq!(soon.waiting.len(), 1);
        assert_eq!(soon.waiting[0].id, "two");
        // Still waiting much later: the press didn't work, so it's offered again.
        let later = soon.look(host, pressed_at + PRESSED_FOR);
        assert_eq!(later.waiting.len(), 2);
    }

    #[test]
    fn a_teams_workers_are_not_the_persons_to_click() {
        let row = |id: &str| HeldAgent {
            id: id.into(),
            terminal: "t".into(),
            label: "Agent".into(),
            holds: String::new(),
        };
        let resources = AgentResources {
            waiting: vec![row("worker"), row("mine")],
            paused: vec![row("paused-worker"), row("paused-mine")],
            ..Default::default()
        };
        let rows = resources.rows(|pane| pane.contains("worker"));
        let shown: Vec<_> = rows
            .iter()
            .map(|(a, paused)| (a.id.as_str(), *paused))
            .collect();
        assert_eq!(shown, [("paused-mine", true), ("mine", false)]);
    }

    #[test]
    fn a_tab_says_waiting_then_paused_then_what_it_holds() {
        let pane = Uuid::new_v4();
        let held = HeldAgent {
            id: pane.to_string(),
            terminal: String::new(),
            label: "claude".into(),
            holds: "5.0 GB".into(),
        };
        let mut resources = AgentResources::default();
        assert_eq!(resources.shows(pane), None);
        resources.memory.insert(pane, "1.2 GB".into());
        assert_eq!(resources.shows(pane).as_deref(), Some("1.2 GB"));
        resources.paused.push(held.clone());
        assert_eq!(resources.shows(pane).as_deref(), Some("Paused (5.0 GB)"));
        resources.waiting.push(held.clone());
        assert_eq!(resources.shows(pane).as_deref(), Some("Waiting for memory"));
        assert!(row_text(&held, true).contains("5.0 GB"));
        assert!(row_text(&held, false).contains("waits for memory"));
    }
}
