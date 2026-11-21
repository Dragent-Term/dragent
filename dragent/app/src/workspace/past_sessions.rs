//! "Past sessions": earlier agent conversations started in this workspace's
//! folder (Claude Code, Codex, Gemini, OpenCode, MiMo, Copilot, Cursor),
//! newest first, grouped by when they last changed; choosing one continues
//! it in a new tab, under the account it belongs to. It is the agent
//! popover's card, search field, captions and rows. Listing comes from
//! `dragent_host::agent_history`.

use std::cell::RefCell;
use std::collections::HashMap;

use dragent_host::agent_history::{self, PastSession};
use warpui::color::ColorU;
use warpui::elements::{Align, Container, CrossAxisAlignment, Element, Expanded, Flex, ParentElement, Text};
use warpui::fonts::FamilyId;
use warpui::{EntityId, UpdateView, ViewContext};
use warpui_core::elements::{DispatchEventResult, EventHandler, Hoverable, MouseStateHandle};
use warpui_core::platform::Cursor;
use warpui_core::scene::{Border, CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

use super::action::PromptAgent;
use super::view::WorkspaceView;
use super::WorkspaceAction;
use crate::icons::{render_icon, AppIcon};
use crate::theme::{text_size, Theme};

/// Conversations listed at most.
const LIMIT: usize = 40;
const PANEL_WIDTH: f32 = 460.0;
const PANEL_MAX_HEIGHT: f32 = 420.0;

impl WorkspaceView {
    /// The folders whose conversations are listed: the active tab's and the
    /// project's.
    fn past_session_folders(&self, ctx: &ViewContext<Self>) -> Vec<std::path::PathBuf> {
        let mut folders: Vec<std::path::PathBuf> = self
            .tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .filter(|tab| !tab.pane.is_settings())
            .map(|tab| tab.pane.cwd(ctx))
            .into_iter()
            .collect();
        folders.extend(self.active_project_root());
        folders.retain(|folder| folder.is_dir());
        folders
    }

    pub(crate) fn open_past_sessions(&mut self, ctx: &mut ViewContext<Self>) {
        self.new_tab_picker_open = false;
        self.past_sessions_open = true;
        self.past_sessions_loading = true;
        self.past_sessions_scroll = Default::default();
        // Opens ready to type into, with no search left from last time.
        ctx.update_view(&self.past_sessions_search_editor, |editor, editor_ctx| {
            editor.set_text("", editor_ctx);
            editor_ctx.focus_self();
        });
        let folders = self.past_session_folders(ctx);
        self.past_sessions_confirm = None;
        self.past_sessions_busy = None;
        self.past_sessions_note = None;
        ctx.spawn(async move { agent_history::list(&folders, LIMIT) }, |view, sessions, ctx| {
            view.past_sessions = sessions;
            view.past_sessions_loading = false;
            ctx.notify();
            view.measure_past_sessions(ctx);
        });
        ctx.notify();
    }

    /// Sizes of conversations kept in a database, and the space deleting
    /// left free inside those databases; they take a moment, so they arrive
    /// after the list.
    fn measure_past_sessions(&mut self, ctx: &mut ViewContext<Self>) {
        let mut sessions = self.past_sessions.clone();
        ctx.spawn(
            async move {
                agent_history::measure(&mut sessions);
                let reclaimable: Vec<(String, u64)> = ["opencode", "mimo"]
                    .into_iter()
                    .filter_map(|provider| agent_history::reclaimable(provider).map(|(free, _)| (provider.to_string(), free)))
                    .collect();
                (sessions, reclaimable)
            },
            |view, (measured, reclaimable), ctx| {
                for session in &mut view.past_sessions {
                    if let Some(found) = measured.iter().find(|m| m.provider == session.provider && m.id == session.id) {
                        session.size = found.size;
                    }
                }
                view.past_sessions_reclaimable = reclaimable;
                ctx.notify();
            },
        );
    }

    /// Delete a conversation for good (already confirmed in the list).
    pub(crate) fn delete_past_session(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(session) = self.past_sessions.get(index).cloned() else { return };
        self.past_sessions_confirm = None;
        self.past_sessions_note = None;
        self.past_sessions_busy = Some(format!("Deleting “{}”…", session.title));
        ctx.notify();
        ctx.spawn(async move { (agent_history::delete(&session), session) }, |view, (result, session), ctx| {
            view.past_sessions_busy = None;
            match result {
                Ok(deleted) => {
                    view.past_sessions.retain(|s| !(s.provider == session.provider && s.id == session.id));
                    let name = agent_of(&session.provider).map_or_else(|| session.provider.clone(), |a| a.label().to_string());
                    view.past_sessions_note = Some(if deleted.freed > 0 {
                        format!("Deleted. {} freed.", agent_history::bytes_label(deleted.freed))
                    } else if deleted.reusable > 0 {
                        format!(
                            "Deleted. {} is free inside {name}'s database; Free up space gives it back to your disk.",
                            agent_history::bytes_label(deleted.reusable)
                        )
                    } else {
                        "Deleted.".to_string()
                    });
                    view.measure_past_sessions(ctx);
                }
                Err(error) => view.past_sessions_note = Some(error),
            }
            ctx.notify();
        });
    }

    /// Compact OpenCode's or MiMo's database (asked for in the list).
    pub(crate) fn compact_agent_history(&mut self, provider: String, ctx: &mut ViewContext<Self>) {
        self.past_sessions_confirm = None;
        self.past_sessions_note = None;
        let name = agent_of(&provider).map_or_else(|| provider.clone(), |a| a.label().to_string());
        self.past_sessions_busy = Some(format!("Freeing up space in {name}'s database… this can take a few minutes."));
        ctx.notify();
        ctx.spawn(async move { (agent_history::compact(&provider), name) }, |view, (result, name), ctx| {
            view.past_sessions_busy = None;
            view.past_sessions_note = Some(match result {
                Ok(freed) => format!("{} given back to your disk from {name}.", agent_history::bytes_label(freed)),
                Err(error) => error,
            });
            view.measure_past_sessions(ctx);
            ctx.notify();
        });
    }

    /// Continue a listed conversation in a new tab in its own folder.
    pub(crate) fn resume_past_session(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(session) = self.past_sessions.get(index).cloned() else { return };
        self.past_sessions_open = false;
        if let Err(error) = self.resume_session(session, ctx) {
            self.show_error_notice("Past session", error, ctx);
        }
    }

    /// Continue `session` in a new tab in its own folder, under its account
    /// (also what a phone's "Continue" asks for).
    pub(crate) fn resume_session(&mut self, session: PastSession, ctx: &mut ViewContext<Self>) -> Result<(), String> {
        // A folder on a remote machine resumes with that machine's account.
        // The stored selection (profile id or "system") plus the machine
        // behind the folder is all the resume needs; the machine resolves
        // it again at resume time.
        if let Some(machine) = self.machine_agents(Some(&session.cwd), ctx) {
            return self.resume_remote_session(session, machine, ctx);
        }
        let settings = crate::setup::UiSettings::load();
        let definition = super::agent_definition::definition_by_cli(
            &session.provider,
            &settings.custom_agents,
            self.theme.is_dark,
        );
        // The conversation lives in its account's folder: run it there.
        let binding = if session.account == "system" {
            dragent_host::agent_accounts::Binding::system(&session.provider)
        } else {
            let selection = match dragent_host::agent_accounts::Selection::parse(&session.account) {
                Ok(selection) => selection,
                Err(error) => return Err(error),
            };
            match dragent_host::agent_accounts::Registry::load()
                .and_then(|registry| registry.resolve(&session.provider, None, &selection))
            {
                Ok(binding) => binding,
                Err(error) => return Err(error),
            }
        };
        if !session.cwd.is_dir() {
            return Err(format!("{} no longer exists.", session.cwd.display()));
        }
        let project_root = self.active_project_root();
        if self.begin_fresh_surface(ctx).is_none() {
            return Err("The desktop couldn't open a new tab here.".into());
        }
        let label = definition.as_ref().map_or_else(|| session.provider.clone(), |d| d.name.clone());
        let new_tab = self.create_tab_with_cwd(ctx, Some(label), None, session.cwd.clone(), project_root);
        if let Err(error) = dragent_host::agent_sessions::stage_account(new_tab.pane.pane_uuid(ctx), &binding) {
            return Err(error);
        }
        if let Some(definition) = &definition {
            new_tab.pane.set_agent_id(ctx, Some(definition.id.clone()));
        }
        let terminal = match &new_tab.pane {
            super::pane::WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
            _ => None,
        };
        let new_tab_id = new_tab.id;
        self.tabs.push(new_tab);
        self.active_tab_id = new_tab_id;
        if let Some(terminal) = terminal {
            let command = session.resume_command();
            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                terminal.submit_shell_command(&command, terminal_ctx);
            });
        }
        self.after_agent_surface_created(ctx);
        Ok(())
    }

    /// Continue a remote `session` where it ran: the resume command
    /// prefixed with that machine's `LaunchEnv` for the stored account.
    /// `System` needs no account lookup; any other selection is resolved on
    /// the machine off the UI thread. Remote accounts are carried by the env
    /// prefix, never by local staging. Failures show an error notice and
    /// open nothing.
    fn resume_remote_session(
        &mut self,
        session: PastSession,
        machine: super::remote_project::MachineAgents,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        use dragent_host::development::{
            AccountsRequest, AccountsResponse, DevelopmentRequest, DevelopmentResponse,
        };
        if !session.cwd.is_dir() {
            return Err(format!("{} no longer exists.", session.cwd.display()));
        }
        let selection = if session.account == "system" {
            dragent_host::agent_accounts::Selection::System
        } else {
            dragent_host::agent_accounts::Selection::parse(&session.account)?
        };
        let settings = crate::setup::UiSettings::load();
        let definition = super::agent_definition::definition_by_cli(
            &session.provider,
            &settings.custom_agents,
            self.theme.is_dark,
        );
        let project_root = self.active_project_root();
        let label = definition.as_ref().map_or_else(|| session.provider.clone(), |d| d.name.clone());
        let definition_id = definition.as_ref().map(|d| d.id.clone());
        let remote_resume = machine.launch_line(&session.resume_command());
        if matches!(selection, dragent_host::agent_accounts::Selection::System) {
            if self.begin_fresh_surface(ctx).is_none() {
                return Err("The desktop couldn't open a new tab here.".into());
            }
            let new_tab = self.create_tab_with_cwd(ctx, Some(label), None, session.cwd.clone(), project_root);
            if let Some(id) = definition_id {
                new_tab.pane.set_agent_id(ctx, Some(id));
            }
            let terminal = match &new_tab.pane {
                super::pane::WorkspacePane::Terminal(terminal) => Some(terminal.clone()),
                _ => None,
            };
            let new_tab_id = new_tab.id;
            self.tabs.push(new_tab);
            self.active_tab_id = new_tab_id;
            if let Some(terminal) = terminal {
                ctx.update_view(&terminal, |terminal, terminal_ctx| {
                    terminal.submit_shell_command(&remote_resume, terminal_ctx);
                });
            }
            self.after_agent_surface_created(ctx);
            return Ok(());
        }
        let client = crate::remote_fs::client(&machine.machine_id)?;
        let provider = session.provider.clone();
        let cwd = session.cwd.clone();
        ctx.spawn(
            async move {
                match client.development(DevelopmentRequest::Environment) {
                    Ok(DevelopmentResponse::Environment { os, .. }) => {
                        let lower = os.to_lowercase();
                        if lower.contains("windows") || lower == "win32" {
                            return Err(
                                "Saved accounts aren't supported on Windows machines yet".to_string(),
                            );
                        }
                    }
                    Err(error) => return Err(error),
                    _ => {}
                }
                match client.development(DevelopmentRequest::Accounts(
                    AccountsRequest::LaunchEnv { provider, selection },
                )) {
                    Ok(DevelopmentResponse::Accounts(AccountsResponse::LaunchEnv(env))) => Ok(env),
                    Ok(other) => Err(format!("unexpected development response: {other:?}")),
                    Err(error) => Err(error),
                }
            },
            move |view, result, ctx| {
                match result {
                    Ok(env) => {
                        let command = format!(
                            "{}{}",
                            dragent_host::agent_accounts::posix_prefix(&env),
                            remote_resume
                        );
                        if view.begin_fresh_surface(ctx).is_none() {
                            return;
                        }
                        let new_tab =
                            view.create_tab_with_cwd(ctx, Some(label), None, cwd, project_root);
                        if let Some(id) = definition_id {
                            new_tab.pane.set_agent_id(ctx, Some(id));
                        }
                        let terminal = match &new_tab.pane {
                            super::pane::WorkspacePane::Terminal(terminal) => {
                                Some(terminal.clone())
                            }
                            _ => None,
                        };
                        let new_tab_id = new_tab.id;
                        view.tabs.push(new_tab);
                        view.active_tab_id = new_tab_id;
                        if let Some(terminal) = terminal {
                            ctx.update_view(&terminal, |terminal, terminal_ctx| {
                                terminal.submit_shell_command(&command, terminal_ctx);
                            });
                        }
                        view.after_agent_surface_created(ctx);
                    }
                    Err(message) => view.show_error_notice("Past session", message, ctx),
                }
                ctx.notify();
            },
        );
        Ok(())
    }
}

/// The agent behind a listed conversation, for its artwork and name.
fn agent_of(provider: &str) -> Option<PromptAgent> {
    PromptAgent::ALL.into_iter().find(|agent| agent.cli() == Some(provider))
}

/// Which caption a conversation sits under, by the day it last changed.
fn period(updated_at: i64, now: chrono::DateTime<chrono::Local>) -> &'static str {
    use chrono::TimeZone;
    let Some(at) = chrono::Local.timestamp_opt(updated_at, 0).single() else { return "EARLIER" };
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        ..=0 => "TODAY",
        1 => "YESTERDAY",
        2..=6 => "THIS WEEK",
        7..=30 => "THIS MONTH",
        _ => "EARLIER",
    }
}

/// Typed words all appear in the title, the agent's name or the account.
fn matches(session: &PastSession, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let agent = agent_of(&session.provider).map_or_else(|| session.provider.clone(), |agent| agent.label().to_string());
    let haystack = format!("{} {} {}", session.title, agent, session.account_label).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

/// A row in the agent popover's style: artwork, title, then "agent · age ·
/// account"; on the right its size and a delete button. The text part
/// continues the conversation; the button asks before deleting.
fn session_row(
    index: usize,
    session: &PastSession,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
    busy: bool,
) -> Box<dyn Element> {
    let theme = *theme;
    let agent = agent_of(&session.provider);
    let mut detail = vec![
        agent.map_or_else(|| session.provider.clone(), |agent| agent.label().to_string()),
        agent_history::age(session.updated_at),
    ];
    if session.account != "system" {
        detail.push(session.account_label.clone());
    }
    let detail = detail.join(" · ");
    let title = session.title.clone();
    let size = session.size.map_or_else(|| "…".to_string(), agent_history::bytes_label);
    let main_state = mouse(states, format!("past-session-{index}"));
    let trash_state = mouse(states, format!("past-session-delete-{index}"));
    Hoverable::new(mouse(states, format!("past-session-row-{index}")), move |row| {
        let icon = match agent {
            Some(agent) => super::project_prompt::agent_image(agent, &theme, 16.0, 1.0),
            None => render_icon(AppIcon::Agent, theme.sub_text(), 16.0),
        };
        let words = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(1.0)
            .with_child(
                Text::new(title.clone(), ui, text_size(12.5))
                    .with_color(theme.main_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .with_child(
                Text::new(detail.clone(), ui, text_size(11.0))
                    .with_color(theme.sub_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
            )
            .finish();
        let open = Hoverable::new(main_state.clone(), move |_| {
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(icon)
                .with_child(Expanded::new(1.0, words).finish())
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, WorkspaceAction::ResumePastSession(index)))
        .finish();
        let trash = Hoverable::new(trash_state.clone(), move |state| {
            let color = if state.is_hovered() { theme.ui_red() } else if row.is_hovered() { theme.sub_text() } else { theme.disabled_text() };
            Container::new(render_icon(AppIcon::Trash, color, 13.0))
                .with_uniform_padding(5.0)
                .with_background_color(if state.is_hovered() { theme.surface_2() } else { ColorU::transparent_black() })
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.0)))
                .finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            if !busy {
                ctx.dispatch_typed_action_for(view_id, WorkspaceAction::AskDeletePastSession(index));
            }
        })
        .finish();
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(Expanded::new(1.0, open).finish())
                .with_child(
                    Text::new(size.clone(), ui, text_size(11.0))
                        .with_color(theme.disabled_text())
                        .finish(),
                )
                .with_child(trash)
                .finish(),
        )
        .with_background_color(if row.is_hovered() { theme.surface_3() } else { theme.surface_2() })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
        .with_padding_left(10.0)
        .with_padding_right(4.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish()
    })
    .finish()
}

/// A small button in the popover's language: `strong` is the red, final one.
fn small_button(
    label: &'static str,
    strong: bool,
    theme: &Theme,
    ui: FamilyId,
    state: MouseStateHandle,
    on_click: impl Fn(&mut warpui::EventContext) + 'static,
) -> Box<dyn Element> {
    let theme = *theme;
    Hoverable::new(state, move |hover| {
        let (background, color) = if strong {
            (if hover.is_hovered() { theme.ui_red() } else { faded(theme.ui_red(), 220) }, ColorU::white())
        } else {
            (if hover.is_hovered() { theme.surface_3() } else { theme.surface_2() }, theme.main_text())
        };
        Container::new(Text::new(label.to_string(), ui, text_size(12.0)).with_color(color).finish())
            .with_horizontal_padding(10.0)
            .with_vertical_padding(4.0)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, _| on_click(ctx))
    .finish()
}

/// In place of the row: "Delete …? This can't be undone." Cancel / Delete.
fn confirm_row(
    index: usize,
    session: &PastSession,
    theme: &Theme,
    ui: FamilyId,
    view_id: EntityId,
    states: &RefCell<HashMap<String, MouseStateHandle>>,
) -> Box<dyn Element> {
    let what = match session.size {
        Some(bytes) if bytes > 0 => format!("Frees {}. ", agent_history::bytes_label(bytes)),
        _ => String::new(),
    };
    let question = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(1.0)
        .with_child(
            Text::new(format!("Delete “{}”?", session.title), ui, text_size(12.5))
                .with_color(theme.main_text())
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
        )
        .with_child(
            Text::new(format!("{what}It can't be undone or continued again."), ui, text_size(11.0))
                .with_color(theme.sub_text())
                .soft_wrap(false)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
        )
        .finish();
    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(6.0)
            .with_child(render_icon(AppIcon::Trash, theme.ui_red(), 14.0))
            .with_child(Expanded::new(1.0, question).finish())
            .with_child(small_button("Cancel", false, theme, ui, mouse(states, format!("past-cancel-{index}")), move |ctx| {
                ctx.dispatch_typed_action_for(view_id, WorkspaceAction::CancelDeletePastSession)
            }))
            .with_child(small_button("Delete", true, theme, ui, mouse(states, format!("past-delete-{index}")), move |ctx| {
                ctx.dispatch_typed_action_for(view_id, WorkspaceAction::DeletePastSession(index))
            }))
            .finish(),
    )
    .with_background_color(theme.surface_3())
    .with_border(Border::all(1.0).with_border_color(faded(theme.ui_red(), 120)))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.0)))
    .with_horizontal_padding(10.0)
    .with_vertical_padding(7.0)
    .finish()
}

/// `color` with opacity `alpha` (0–255).
fn faded(color: ColorU, alpha: u8) -> ColorU {
    ColorU::new(color.r, color.g, color.b, alpha)
}

fn mouse(states: &RefCell<HashMap<String, MouseStateHandle>>, key: String) -> MouseStateHandle {
    states.borrow_mut().entry(key).or_default().clone()
}

/// Everything the Past sessions popover needs.
pub(crate) struct PastSessionsProps<'a> {
    pub sessions: &'a [PastSession],
    pub loading: bool,
    pub theme: &'a Theme,
    pub ui: FamilyId,
    pub view_id: EntityId,
    pub states: &'a RefCell<HashMap<String, MouseStateHandle>>,
    pub scroll: &'a warpui::elements::ClippedScrollStateHandle,
    pub search_editor: &'a warpui::ViewHandle<super::InputEditor>,
    pub search: &'a str,
    pub confirm: Option<usize>,
    pub busy: Option<&'a str>,
    pub note: Option<&'a str>,
    pub reclaimable: &'a [(String, u64)],
}

/// The agent popover's card over a lightly dimmed workspace: search, then
/// conversations under TODAY / YESTERDAY / THIS WEEK… captions. Clicking
/// outside closes it.
pub(crate) fn past_sessions_overlay(props: PastSessionsProps<'_>) -> Box<dyn Element> {
    use super::project_prompt::{agent_search_field, menu_caption, menu_card, menu_list};
    let PastSessionsProps { sessions, loading, theme, ui, view_id, states, scroll, search_editor, search, confirm, busy, note, reclaimable } = props;
    let query = search.trim().to_lowercase();
    let now = chrono::Local::now();
    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(2.0);
    let mut shown = 0;
    let mut caption = "";
    for (index, session) in sessions.iter().enumerate() {
        if !matches(session, &query) {
            continue;
        }
        let group = period(session.updated_at, now);
        if group != caption {
            caption = group;
            col = col.with_child(menu_caption(theme, ui, group));
        }
        col = col.with_child(if confirm == Some(index) {
            confirm_row(index, session, theme, ui, view_id, states)
        } else {
            session_row(index, session, theme, ui, view_id, states, busy.is_some())
        });
        shown += 1;
    }
    if shown == 0 {
        let note = if loading {
            "Looking for earlier conversations…"
        } else if !query.is_empty() {
            "No matching sessions"
        } else {
            "No earlier agent conversations in this folder"
        };
        col = col.with_child(
            Container::new(
                Text::new(note.to_string(), ui, text_size(12.0))
                    .with_color(theme.disabled_text())
                    .finish(),
            )
            .with_padding_left(10.0)
            .with_padding_right(10.0)
            .with_padding_top(10.0)
            .with_padding_bottom(10.0)
            .finish(),
        );
    }
    let clear: std::rc::Rc<dyn Fn(&mut warpui::EventContext)> =
        std::rc::Rc::new(move |ctx| ctx.dispatch_typed_action_for(view_id, WorkspaceAction::ClearPastSessionsSearch));
    let mut contents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(agent_search_field(theme, states, search_editor, search, clear))
        .with_child(menu_list(theme, scroll, col.finish(), PANEL_MAX_HEIGHT));
    // Under the list: what is happening or just happened, and space a
    // database can give back (only ever on request).
    let mut footer = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(6.0);
    let mut has_footer = false;
    if let Some(text) = busy.or(note) {
        has_footer = true;
        footer = footer.with_child(
            Text::new(text.to_string(), ui, text_size(11.5))
                .with_color(if busy.is_some() { theme.sub_text() } else { theme.main_text() })
                .finish(),
        );
    }
    for (provider, free) in reclaimable.iter().filter(|(_, free)| *free >= 50 * 1024 * 1024) {
        has_footer = true;
        let name = agent_of(provider).map_or_else(|| provider.clone(), |agent| agent.label().to_string());
        let provider = provider.clone();
        let line = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(
                Expanded::new(
                    1.0,
                    Text::new(
                        format!("{} free inside {name}'s database", agent_history::bytes_label(*free)),
                        ui,
                        text_size(11.5),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .finish(),
            )
            .with_child(if busy.is_some() {
                warpui::elements::Empty::new().finish()
            } else {
                small_button("Free up space", false, theme, ui, mouse(states, format!("past-compact-{provider}")), move |ctx| {
                    ctx.dispatch_typed_action_for(view_id, WorkspaceAction::CompactAgentHistory(provider.clone()))
                })
            })
            .finish();
        footer = footer.with_child(line);
    }
    if has_footer {
        contents = contents.with_child(
            Container::new(footer.finish())
                .with_padding_left(10.0)
                .with_padding_right(4.0)
                .with_padding_top(8.0)
                .with_padding_bottom(4.0)
                .with_border(Border::top(1.0).with_border_color(theme.outline()))
                .finish(),
        );
    }
    let contents = contents.finish();
    let panel = menu_card(theme, contents, PANEL_WIDTH);
    // The card keeps its own clicks; anything outside it closes the list.
    let panel = EventHandler::new(panel)
        .on_left_mouse_down(|_, _, _| DispatchEventResult::StopPropagation)
        .finish();
    EventHandler::new(
        Container::new(Align::new(panel).finish())
            .with_background_color(ColorU::new(0, 0, 0, 60))
            .finish(),
    )
    .on_left_mouse_down(move |ctx, _, _| {
        ctx.dispatch_typed_action_for(view_id, WorkspaceAction::ClosePastSessions);
        DispatchEventResult::StopPropagation
    })
    .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn conversations_sit_under_the_day_they_last_changed() {
        let now = chrono::Local.with_ymd_and_hms(2026, 11, 16, 15, 0, 0).unwrap();
        let at = |d: u32, h: u32| chrono::Local.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap().timestamp();
        assert_eq!(period(at(16, 1), now), "TODAY");
        assert_eq!(period(at(15, 23), now), "YESTERDAY");
        assert_eq!(period(at(11, 9), now), "THIS WEEK");
        assert_eq!(period(at(2, 9), now), "THIS MONTH");
        assert_eq!(period(chrono::Local.with_ymd_and_hms(2026, 8, 1, 9, 0, 0).unwrap().timestamp(), now), "EARLIER");
    }

    #[test]
    fn search_matches_every_word_in_title_agent_or_account() {
        let session = PastSession {
            provider: "opencode".into(), id: "x".into(), title: "Fix the login redirect".into(),
            cwd: Default::default(), updated_at: 0, account: "system".into(), account_label: "System login".into(),
            ..Default::default()
        };
        assert!(matches(&session, ""));
        assert!(matches(&session, "login fix"));
        assert!(matches(&session, "opencode"));
        assert!(!matches(&session, "login codex"));
    }
}
