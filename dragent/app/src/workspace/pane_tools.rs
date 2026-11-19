//! `dragent panes` and the pane tools of `dragent mcp`: the tabs and panes of
//! the caller's workspace, arranged the way the person would with the mouse.
//!
//! Words, as agents use them: a *workspace tab* is one of the window's top
//! tabs; it holds *tabs* (terminals, browsers, settings), some of which can be
//! shown side by side in its *split*. A terminal tab shows a *terminal
//! session* (a shell, which may be running an *agent session*).
//!
//! Rules every operation keeps:
//! - Layout changes act on what is on screen in the target's workspace tab:
//!   a pane that isn't shown is an `invalid_target`, never quietly replaced
//!   by the active one. (A split only exists while it's shown.)
//! - The keyboard stays where the person had it unless the request asks for
//!   focus. Opening a tab adds it to the sidebar without switching to it.
//! - Closing respects the person's confirmations, and nobody closes the
//!   pane they run in.
//! - Every answer names the affected panes and returns the updated layout.

use dragent_host::cli_requests::Response;
use dragent_host::outcome::{Failure, Status};
use serde_json::{json, Value};
use warpui::{ReadView, TypedActionView, UpdateView, ViewContext};

use super::pane::WorkspacePane;
use super::pane_layout::{PaneLayout, SplitSide, EDITOR_PANE_ID};
use super::view::{PaneTab, WorkspaceView};
use super::{DropSide, WorkspaceAction};

/// Where the keyboard was before a change.
#[derive(Clone, Copy)]
struct Focus {
    surface: usize,
    tab: usize,
}

type Answer = Result<Value, Failure>;

fn invalid(code: &str, message: impl Into<String>) -> Failure {
    Failure::new(Status::InvalidInput, code, message)
}

fn no_pane(id: usize) -> Failure {
    Failure::new(Status::InvalidTarget, "no_such_pane", format!("No pane {id}."))
        .next("`dragent panes list` shows the current panes and their ids.")
}

impl WorkspaceView {
    pub(crate) fn pane_tool_for_cli(
        &mut self,
        pane: Option<&str>,
        terminal: Option<&str>,
        tool: &str,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Response {
        let caller = self.cli_caller_tab(pane, terminal, ctx);
        let answer = match tool {
            "list" => Ok(self.describe_panes(caller, ctx)),
            "open" => self.open_pane_for_cli(caller, arguments, ctx),
            "move" => self.move_pane_for_cli(caller, arguments, ctx),
            "unsplit" => self.unsplit_pane_for_cli(caller, arguments, ctx),
            "swap" => self.swap_panes_for_cli(caller, arguments, ctx),
            "resize" => self.resize_pane_for_cli(caller, arguments, ctx),
            "equalize" => self.equalize_panes_for_cli(caller, ctx),
            "focus" => self.focus_pane_for_cli(caller, arguments, ctx),
            "close" => self.close_pane_for_cli(caller, arguments, ctx),
            other => Err(invalid(
                "unknown_panes_command",
                format!("Unknown panes command “{other}”."),
            )),
        };
        match answer {
            Ok(value) => Response::with_data(json!({
                "output": serde_json::to_string_pretty(&value).unwrap_or_default()
            })),
            Err(failure) => Response::failure(failure),
        }
    }

    // ── Reading ──────────────────────────────────────────────────────────

    /// Every workspace tab with its panes and layout. The caller's pane is
    /// marked `you`; `active` is where the keyboard is in that workspace tab.
    fn describe_panes(&self, caller: Option<(usize, usize)>, ctx: &ViewContext<Self>) -> Value {
        let workspace_tabs: Vec<Value> = self
            .surfaces()
            .map(|(surface, _)| self.describe_surface(surface, caller, ctx))
            .collect();
        json!({
            "workspace_tabs": workspace_tabs,
            "you": caller.map(|(_, tab)| tab),
            "shown_workspace_tab": self.active_workspace_tab_id,
        })
    }

    fn describe_surface(
        &self,
        surface: usize,
        caller: Option<(usize, usize)>,
        ctx: &ViewContext<Self>,
    ) -> Value {
        let shown = surface == self.active_workspace_tab_id;
        let parked = self.parked_workspace_tabs.iter().find(|tab| tab.id == surface);
        let (name, active, split, panes) = match parked {
            Some(tab) => (
                tab.name.clone(),
                tab.active_tab_id,
                tab.split_tree.as_ref(),
                tab.panes.as_slice(),
            ),
            None => (
                self.active_workspace_tab_name.clone(),
                self.active_tab_id,
                self.split_tree.as_ref(),
                self.tabs.as_slice(),
            ),
        };
        let visible: Vec<usize> = match split {
            Some(layout) => layout.panes().into_iter().filter(|id| *id != EDITOR_PANE_ID).collect(),
            None => vec![active],
        };
        let entries: Vec<Value> = panes
            .iter()
            .map(|tab| {
                let mut entry = json!({
                    "id": tab.id,
                    "kind": pane_kind(tab),
                    "title": tab.custom_label.clone().unwrap_or_else(|| tab.title.clone()),
                    "active": tab.id == active,
                    "visible": visible.contains(&tab.id),
                    "in_split": split.is_some_and(|layout| layout.contains(tab.id)),
                    "you": caller.is_some_and(|(_, id)| id == tab.id),
                });
                if matches!(tab.pane, WorkspacePane::Terminal(_)) {
                    entry["running"] = json!(tab.pane.is_running(ctx));
                }
                if let Some(root) = &tab.project_root {
                    entry["project"] = json!(root.display().to_string());
                }
                if let WorkspacePane::Browser(browser) = &tab.pane {
                    entry["url"] = json!(browser.read(ctx, |view, _| view.address().to_string()));
                }
                entry
            })
            .collect();
        json!({
            "id": surface,
            "name": name,
            "shown": shown,
            "panes": entries,
            "layout": split.map(PaneLayout::describe),
        })
    }

    // ── Targets and focus ────────────────────────────────────────────────

    /// The workspace tab a pane lives in.
    fn surface_of(&self, id: usize) -> Option<usize> {
        self.surfaces()
            .find(|(_, panes)| panes.iter().any(|tab| tab.id == id))
            .map(|(surface, _)| surface)
    }

    /// The pane named by `key`, or the caller's own pane when it's absent.
    fn pane_argument(
        &self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        key: &str,
    ) -> Result<usize, Failure> {
        match pane_id_in(arguments, key)? {
            Some(id) => self.surface_of(id).map(|_| id).ok_or_else(|| no_pane(id)),
            None => caller.map(|(_, tab)| tab).ok_or_else(|| {
                Failure::new(
                    Status::InvalidTarget,
                    "no_caller_pane",
                    format!("This command isn't running in a Dragent terminal, so name the pane with {key}."),
                )
                .next("`dragent panes list` shows the panes and their ids.")
            }),
        }
    }

    fn remember_focus(&self) -> Focus {
        Focus {
            surface: self.active_workspace_tab_id,
            tab: self.active_tab_id,
        }
    }

    /// Show the workspace tab holding `id` (layout changes happen there).
    fn bring_forward(&mut self, id: usize, ctx: &mut ViewContext<Self>) -> Result<(), Failure> {
        let surface = self.surface_of(id).ok_or_else(|| no_pane(id))?;
        if surface != self.active_workspace_tab_id {
            self.handle_action(
                &WorkspaceAction::SelectWorkspaceTab {
                    workspace_tab_id: surface,
                },
                ctx,
            );
        }
        Ok(())
    }

    /// Panes on screen in the shown workspace tab.
    fn visible_tabs(&self) -> Vec<usize> {
        match &self.split_tree {
            Some(layout) => layout.panes().into_iter().filter(|id| *id != EDITOR_PANE_ID).collect(),
            None if self.tabs.iter().any(|tab| tab.id == self.active_tab_id) => vec![self.active_tab_id],
            None => Vec::new(),
        }
    }

    fn require_visible(&self, id: usize) -> Result<(), Failure> {
        if self.visible_tabs().contains(&id) {
            return Ok(());
        }
        Err(Failure::new(
            Status::InvalidTarget,
            "pane_not_shown",
            format!("Pane {id} isn't on screen, so the layout can't change around it."),
        )
        .next("Pick a pane that is visible (\"visible\": true in `dragent panes list`), or show it first with `dragent panes focus ID`."))
    }

    /// Give the keyboard to `pane` (on screen) and show it.
    fn activate_visible(&mut self, pane: usize, take_focus: bool, ctx: &mut ViewContext<Self>) {
        if !self.tabs.iter().any(|tab| tab.id == pane) {
            return;
        }
        let switched = self.active_tab_id != pane;
        self.active_tab_id = pane;
        if let Some(layout) = self.split_tree.as_mut() {
            layout.focus(pane);
        }
        if switched {
            // As selecting it would: its project's work comes with it.
            self.switch_workspace_work_for_tab(pane, ctx);
        }
        self.update_active_pane_with_focus(take_focus, ctx);
    }

    /// After a change: the requested pane takes the keyboard, or the person
    /// keeps it where it was when that pane is still on screen.
    fn settle(
        &mut self,
        before: Focus,
        requested: Option<usize>,
        take_focus: bool,
        ctx: &mut ViewContext<Self>,
    ) -> Value {
        if let (true, Some(pane)) = (take_focus, requested) {
            if self.visible_tabs().contains(&pane) {
                self.activate_visible(pane, true, ctx);
            } else if self.tabs.iter().any(|tab| tab.id == pane) {
                self.handle_action(&WorkspaceAction::SelectTab { tab_id: pane }, ctx);
            }
        } else {
            if before.surface != self.active_workspace_tab_id
                && self.surfaces().any(|(surface, _)| surface == before.surface)
            {
                self.handle_action(
                    &WorkspaceAction::SelectWorkspaceTab {
                        workspace_tab_id: before.surface,
                    },
                    ctx,
                );
            }
            if before.surface == self.active_workspace_tab_id
                && self.visible_tabs().contains(&before.tab)
            {
                self.activate_visible(before.tab, false, ctx);
            }
        }
        ctx.notify();
        let kept = before.surface == self.active_workspace_tab_id && before.tab == self.active_tab_id;
        json!({
            "kept": kept,
            "workspace_tab": self.active_workspace_tab_id,
            "pane": self.active_tab_id,
        })
    }

    fn changed(&mut self, ctx: &mut ViewContext<Self>) {
        self.prune_split_handles();
        self.refresh_all_tab_titles(ctx);
        self.sync_left_panel(ctx);
        self.save_session(ctx);
        ctx.notify();
    }

    fn report(
        &self,
        action: &str,
        details: Value,
        focus: Value,
        surface: usize,
        caller: Option<(usize, usize)>,
        ctx: &ViewContext<Self>,
    ) -> Value {
        let mut report = json!({
            "status": "ok",
            "action": action,
            "focus": focus,
            "workspace_tab": self.describe_surface(surface, caller, ctx),
        });
        if let (Some(target), Some(details)) = (report.as_object_mut(), details.as_object()) {
            for (key, value) in details {
                target.insert(key.clone(), value.clone());
            }
        }
        report
    }

    // ── Changing ─────────────────────────────────────────────────────────

    /// A terminal (optionally running a command) or a browser: a new tab in
    /// the sidebar, or split on one side of a pane that's on screen.
    fn open_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let kind = text(arguments, "kind").unwrap_or_else(|| "terminal".into());
        let place = crate::cli::app_tools::pane_open_place(arguments).to_string();
        let command = text(arguments, "command");
        let label = text(arguments, "label");
        let take_focus = arguments["focus"].as_bool() == Some(true);
        let url = text(arguments, "url")
            .map(|address| dragent_workspace_model::BrowserUrl::from_address_or_search(&address))
            .transpose()
            .map_err(|error| invalid("invalid_url", error))?;
        if !matches!(kind.as_str(), "terminal" | "browser") {
            return Err(invalid("invalid_kind", format!("A pane is a terminal or a browser, not “{kind}”.")));
        }
        let side = match place.as_str() {
            "tab" => None,
            other => Some(split_side(other)?),
        };
        if kind == "browser" && command.is_some() {
            return Err(invalid("command_needs_terminal", "A command runs in a terminal pane; give a browser a url."));
        }

        let before = self.remember_focus();
        let beside = match (side, pane_id_in(arguments, "target")?, caller) {
            (_, Some(id), _) => Some(self.surface_of(id).map(|_| id).ok_or_else(|| no_pane(id))?),
            (_, None, Some((_, tab))) => Some(tab),
            (Some(_), None, None) => {
                return Err(Failure::new(
                    Status::InvalidTarget,
                    "no_caller_pane",
                    "This command isn't running in a Dragent terminal, so name the pane to split with target.",
                )
                .next("`dragent panes list` shows the panes and their ids."))
            }
            (None, None, None) => None,
        };
        if let Some(target) = beside {
            self.bring_forward(target, ctx)?;
        }
        let new_id = match side {
            None => {
                let (cwd, project) = self.place_of(beside, ctx);
                let mut tab = match kind.as_str() {
                    "browser" => self.create_browser_tab(url, project, ctx),
                    _ => self.create_tab_with_cwd_focus(ctx, label.clone(), None, cwd, project, false),
                };
                if label.is_some() {
                    tab.custom_label = label.clone();
                }
                let id = tab.id;
                self.tabs.push(tab);
                id
            }
            Some(side) => {
                let target = beside.expect("a split always has a target");
                self.require_visible(target)?;
                let id = match kind.as_str() {
                    "browser" => {
                        self.active_tab_id = target;
                        self.split_browser(side, url, false, ctx)
                            .ok_or_else(|| Failure::new(Status::Failed, "open_failed", "Dragent couldn't open a browser there."))?
                    }
                    _ => self.split_terminal_beside(target, side, label.clone(), ctx),
                };
                if let (Some(label), Some(tab)) = (&label, self.tabs.iter_mut().find(|tab| tab.id == id)) {
                    tab.custom_label = Some(label.clone());
                }
                id
            }
        };
        if let Some(command) = &command {
            if let Some(WorkspacePane::Terminal(terminal)) =
                self.tabs.iter().find(|tab| tab.id == new_id).map(|tab| tab.pane.clone())
            {
                terminal.update(ctx, |view, view_ctx| {
                    // settle() owns the single explicit focus decision after
                    // launch; submitting a command must not compete with it.
                    view.submit_shell_command_preserving_focus(command, view_ctx);
                });
            }
        }
        self.changed(ctx);
        let surface = self.active_workspace_tab_id;
        let focus = self.settle(before, Some(new_id), take_focus, ctx);
        Ok(self.report(
            "open",
            json!({ "pane": new_id, "kind": kind, "place": place, "beside": beside, "label": label }),
            focus,
            surface,
            caller,
            ctx,
        ))
    }

    /// The folder and project a new pane beside `beside` starts in.
    fn place_of(
        &self,
        beside: Option<usize>,
        ctx: &ViewContext<Self>,
    ) -> (std::path::PathBuf, Option<std::path::PathBuf>) {
        let tab = beside.and_then(|id| self.tabs.iter().find(|tab| tab.id == id));
        let cwd = tab
            .filter(|tab| !tab.pane.is_settings())
            .map(|tab| tab.pane.cwd(ctx))
            .filter(|cwd| cwd.is_dir())
            .or_else(|| self.document_owner_root.clone())
            .or_else(|| self.selected_project_root())
            .unwrap_or_else(crate::workspace::project_naming::default_pane_cwd);
        let project = tab
            .and_then(|tab| tab.project_root.clone())
            .or_else(|| self.document_owner_root.clone())
            .or_else(|| self.selected_project_root());
        (cwd, project)
    }

    /// A new terminal split on `side` of `target` (on screen), keeping the
    /// keyboard where it is.
    fn split_terminal_beside(
        &mut self,
        target: usize,
        side: SplitSide,
        label: Option<String>,
        ctx: &mut ViewContext<Self>,
    ) -> usize {
        let (cwd, project) = self.place_of(Some(target), ctx);
        let mut tab = self.create_tab_with_cwd_focus(ctx, label, None, cwd, project, false);
        // Split panes live in the layout, not the sidebar.
        tab.hidden_from_sidebar = true;
        let id = tab.id;
        self.tabs.push(tab);
        match &mut self.split_tree {
            Some(layout) => {
                layout.split_pane(target, id, side);
            }
            None => {
                let mut layout = PaneLayout::new(target);
                layout.split_pane(target, id, side);
                layout.focus(target);
                self.split_tree = Some(layout);
            }
        }
        id
    }

    /// Show an existing tab's session on `place` of a pane on screen; also
    /// how a pane moves within the split. Nothing restarts.
    fn move_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = pane_id_in(arguments, "id")?.ok_or_else(|| invalid("missing_id", "Name the pane to move with id."))?;
        let target = self.pane_argument(caller, arguments, "target")?;
        let side = split_side(&text(arguments, "place").unwrap_or_else(|| "right".into()))?;
        let take_focus = arguments["focus"].as_bool() == Some(true);
        if id == target {
            return Err(invalid("same_pane", "A pane can't move beside itself."));
        }
        let surface = self.surface_of(id).ok_or_else(|| no_pane(id))?;
        if self.surface_of(target) != Some(surface) {
            return Err(invalid(
                "different_workspace_tabs",
                format!("Panes {id} and {target} are in different workspace tabs; panes move within one."),
            ));
        }
        let before = self.remember_focus();
        self.bring_forward(target, ctx)?;
        self.require_visible(target)?;
        let drop = match side {
            SplitSide::Left => DropSide::Left,
            SplitSide::Right => DropSide::Right,
            SplitSide::Up => DropSide::Top,
            SplitSide::Down => DropSide::Bottom,
        };
        self.move_tab_into_split(id, target, drop, ctx);
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.hidden_from_sidebar = true;
        }
        self.changed(ctx);
        let focus = self.settle(before, Some(id), take_focus, ctx);
        Ok(self.report("move", json!({ "pane": id, "beside": target }), focus, surface, caller, ctx))
    }

    /// Take a pane out of the split into its own tab; its session keeps running.
    fn unsplit_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = self.pane_argument(caller, arguments, "id")?;
        let before = self.remember_focus();
        self.bring_forward(id, ctx)?;
        if !self.split_tree.as_ref().is_some_and(|layout| layout.contains(id)) {
            return Err(Failure::new(
                Status::InvalidTarget,
                "not_in_split",
                format!("Pane {id} isn't in the split that's on screen."),
            ));
        }
        let surface = self.active_workspace_tab_id;
        self.pop_tab_out_of_split(id, ctx);
        self.changed(ctx);
        let focus = self.settle(before, None, false, ctx);
        Ok(self.report("unsplit", json!({ "pane": id }), focus, surface, caller, ctx))
    }

    fn swap_panes_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = self.pane_argument(caller, arguments, "id")?;
        let other = pane_id_in(arguments, "with")?.ok_or_else(|| invalid("missing_with", "Name the pane to swap with using with."))?;
        let before = self.remember_focus();
        self.bring_forward(id, ctx)?;
        self.require_visible(id)?;
        self.require_visible(other)?;
        let swapped = self.split_tree.as_mut().is_some_and(|layout| layout.swap_panes(id, other));
        if !swapped {
            return Err(Failure::new(
                Status::InvalidTarget,
                "not_in_split",
                "Both panes must be in the split that's on screen.",
            ));
        }
        let surface = self.active_workspace_tab_id;
        self.changed(ctx);
        let focus = self.settle(before, None, false, ctx);
        Ok(self.report("swap", json!({ "panes": [id, other] }), focus, surface, caller, ctx))
    }

    fn resize_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = self.pane_argument(caller, arguments, "id")?;
        let share = arguments["share"]
            .as_f64()
            .or_else(|| arguments["share"].as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|share| (0.1..=0.9).contains(share))
            .ok_or_else(|| invalid("invalid_share", "share is the pane's part of its split, from 0.1 to 0.9."))?;
        let before = self.remember_focus();
        self.bring_forward(id, ctx)?;
        self.require_visible(id)?;
        let resized = self.split_tree.as_mut().is_some_and(|layout| layout.set_share(id, share as f32));
        if !resized {
            return Err(Failure::new(
                Status::InvalidTarget,
                "not_in_split",
                format!("Pane {id} isn't in a split, so it has nothing to share."),
            ));
        }
        let surface = self.active_workspace_tab_id;
        self.changed(ctx);
        let focus = self.settle(before, None, false, ctx);
        Ok(self.report("resize", json!({ "pane": id, "share": share }), focus, surface, caller, ctx))
    }

    fn equalize_panes_for_cli(&mut self, caller: Option<(usize, usize)>, ctx: &mut ViewContext<Self>) -> Answer {
        let before = self.remember_focus();
        if let Some((_, tab)) = caller {
            self.bring_forward(tab, ctx)?;
        }
        let surface = self.active_workspace_tab_id;
        let changed = match self.split_tree.as_mut() {
            Some(layout) => {
                layout.equalize_all();
                true
            }
            None => false,
        };
        if changed {
            self.changed(ctx);
        }
        let focus = self.settle(before, None, false, ctx);
        Ok(self.report("equalize", json!({ "changed": changed }), focus, surface, caller, ctx))
    }

    fn focus_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = pane_id_in(arguments, "id")?.ok_or_else(|| invalid("missing_id", "Name the pane to show with id."))?;
        let surface = self.surface_of(id).ok_or_else(|| no_pane(id))?;
        let before = self.remember_focus();
        self.bring_forward(id, ctx)?;
        let focus = self.settle(before, Some(id), true, ctx);
        Ok(self.report("focus", json!({ "pane": id }), focus, surface, caller, ctx))
    }

    fn close_pane_for_cli(
        &mut self,
        caller: Option<(usize, usize)>,
        arguments: &Value,
        ctx: &mut ViewContext<Self>,
    ) -> Answer {
        let id = pane_id_in(arguments, "id")?.ok_or_else(|| invalid("missing_id", "Name the pane to close with id."))?;
        if caller.is_some_and(|(_, tab)| tab == id) {
            return Err(invalid("own_pane", "That's the pane you run in; end your session instead of closing it."));
        }
        let surface = self.surface_of(id).ok_or_else(|| no_pane(id))?;
        let before = self.remember_focus();
        self.bring_forward(id, ctx)?;
        self.handle_action(&WorkspaceAction::CloseTab { tab_id: id }, ctx);
        if self.tabs.iter().any(|tab| tab.id == id) {
            // The person is asked first (something runs there, or their
            // confirmation setting is on). Never answered for them.
            return Err(Failure::new(
                Status::NeedsApproval,
                "close_confirmation",
                format!("The person was asked to confirm closing pane {id}."),
            )
            .next("Wait for them; `dragent panes list` shows whether it closed."));
        }
        let focus = self.settle(before, None, false, ctx);
        Ok(self.report("close", json!({ "pane": id }), focus, surface, caller, ctx))
    }
}

fn pane_kind(tab: &PaneTab) -> &'static str {
    match tab.pane {
        WorkspacePane::Terminal(_) => "terminal",
        WorkspacePane::Browser(_) => "browser",
        WorkspacePane::Settings(_) => "settings",
    }
}

fn text(arguments: &Value, key: &str) -> Option<String> {
    arguments[key]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// A pane id under `key`: absent is `None`, unreadable is an error.
fn pane_id_in(arguments: &Value, key: &str) -> Result<Option<usize>, Failure> {
    match &arguments[key] {
        Value::Null => Ok(None),
        value => value
            .as_u64()
            .or_else(|| value.as_str().and_then(|id| id.trim().parse().ok()))
            .map(|id| Some(id as usize))
            .ok_or_else(|| invalid("invalid_pane_id", format!("{key} is a pane id from `dragent panes list`."))),
    }
}

fn split_side(place: &str) -> Result<SplitSide, Failure> {
    match place {
        "right" => Ok(SplitSide::Right),
        "left" => Ok(SplitSide::Left),
        "down" | "below" => Ok(SplitSide::Down),
        "up" | "above" => Ok(SplitSide::Up),
        other => Err(invalid(
            "invalid_place",
            format!("Place a pane right, left, down or up (or in a tab), not “{other}”."),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_read_strictly() {
        assert_eq!(pane_id_in(&json!({}), "id").unwrap(), None);
        assert_eq!(pane_id_in(&json!({"id": 7}), "id").unwrap(), Some(7));
        assert_eq!(pane_id_in(&json!({"id": " 7 "}), "id").unwrap(), Some(7));
        let bad = pane_id_in(&json!({"id": "seven"}), "id").unwrap_err();
        assert_eq!((bad.status, bad.code.as_str()), (Status::InvalidInput, "invalid_pane_id"));
        assert!(matches!(split_side("up"), Ok(SplitSide::Up)));
        assert!(matches!(split_side("below"), Ok(SplitSide::Down)));
        assert_eq!(split_side("diagonal").unwrap_err().code, "invalid_place");
        assert_eq!(no_pane(3).status, Status::InvalidTarget);
        assert!(no_pane(3).retry_safe);
    }
}
