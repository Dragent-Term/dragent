//! Tasks + SSH host settings: actions, state, persistence wiring,
//! consumer helpers and settings-page cards.
//!
//! Persistence lives in [`crate::settings::task_settings`] embedded in
//! [`AppSettings`](crate::settings::AppSettings); this module owns the
//! [`TaskHostAction`] dispatch, the draft editors, and the functional hooks
//! the rest of the app reads through.

use std::collections::HashMap;

use warpui::fonts::FamilyId;
use warpui::{ReadView, UpdateView, View, ViewContext, ViewHandle};

use crate::settings::task_settings::{
    available_wsl_distros, LocalRuntime, TASK_PROVIDER_KEYS, TASK_SOURCES, TASK_VIEW_PRESETS,
};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputEditorEvent, InputMode};

use super::action::SettingsAction;
use super::settings_view::{SettingsEvent, SettingsView};

// ── Actions ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum TaskHostAction {
    SetDefaultView(String),
    SetDefaultSource(String),
    ToggleVisibleProvider(String),
    AddDefaultRepo,
    RemoveDefaultRepo(String),
    AddDefaultTeam,
    RemoveDefaultTeam(String),
    SetActiveRuntime(Option<String>),
    SetHostDisplayLabel(String),
    SetHostWorktreeLocation(String),
    SetLocalRuntime(LocalRuntime),
    SetWslDistro(Option<String>),
    ToggleSshRouting,
    AddRoutingDisabled,
    RemoveRoutingDisabled(String),
    ToggleRoutingDisabledTarget(String),
    ToggleProbeSkippedTarget(String),
    /// Save the cap on agents and the memory limit; empty means none.
    SaveResourceLimits,
    /// Remove both.
    ClearResourceLimits,
}

fn task_act(action: TaskHostAction) -> SettingsAction {
    SettingsAction::TaskHost(action)
}

// ── State (draft editors) ─────────────────────────────────────────────────

pub struct TaskHostState {
    pub repo_add_editor: ViewHandle<InputEditor>,
    pub team_add_editor: ViewHandle<InputEditor>,
    pub display_label_editor: ViewHandle<InputEditor>,
    pub worktree_location_editor: ViewHandle<InputEditor>,
    pub routing_disabled_editor: ViewHandle<InputEditor>,
    pub max_agents_editor: ViewHandle<InputEditor>,
    pub memory_limit_editor: ViewHandle<InputEditor>,
    pub repo_draft: String,
    pub team_draft: String,
    pub display_label_draft: String,
    pub worktree_location_draft: String,
    pub routing_disabled_draft: String,
    pub max_agents_draft: String,
    pub memory_limit_draft: String,
    /// Why the limits couldn't be saved, or that they were.
    pub limits_note: Option<String>,
}

/// [`make_editor`] holding `text`.
fn limit_editor(
    ctx: &mut ViewContext<SettingsView>,
    ui: FamilyId,
    theme: Theme,
    placeholder: &str,
    key: &str,
    text: &str,
    update: fn(&mut TaskHostState, String),
) -> ViewHandle<InputEditor> {
    let editor = make_editor(ctx, ui, theme, placeholder, key, update);
    ctx.update_view(&editor, |editor, ctx| editor.set_text(text, ctx));
    editor
}

const GB: f64 = (1u64 << 30) as f64;

/// The limits as typed: agents at once, and bytes an agent may hold. Empty
/// is none.
fn limits_from(agents: &str, memory: &str) -> Result<(Option<usize>, Option<u64>), String> {
    let (agents, memory) = (agents.trim(), memory.trim());
    let agents = match agents {
        "" => None,
        _ => match agents.parse::<usize>() {
            Ok(n) if n > 0 => Some(n),
            _ => return Err("How many agents at once? Give a whole number, or leave it empty.".into()),
        },
    };
    let memory = match memory {
        "" => None,
        _ => match memory.replace(',', ".").parse::<f64>() {
            Ok(gb) if (0.25..=4096.0).contains(&gb) => Some((gb * GB) as u64),
            _ => return Err("How much memory per agent? Give GB (0.25 or more), or leave it empty.".into()),
        },
    };
    Ok((agents, memory))
}

#[cfg(not(target_family = "wasm"))]
fn save_limits((max_agents, agent_memory_limit_bytes): (Option<usize>, Option<u64>)) -> Result<(), String> {
    dragent_host::resources::ResourceLimits { max_agents, agent_memory_limit_bytes }
        .save(&dragent_paths::organisation_dir())
}

#[cfg(target_family = "wasm")]
fn save_limits(_limits: (Option<usize>, Option<u64>)) -> Result<(), String> {
    Ok(())
}

/// The limits that are set, as their fields show them.
#[cfg(not(target_family = "wasm"))]
fn saved_limits() -> (String, String) {
    let limits = dragent_host::resources::ResourceLimits::load(&dragent_paths::organisation_dir());
    (
        limits.max_agents().map(|n| n.to_string()).unwrap_or_default(),
        limits
            .agent_memory_limit()
            .map(|bytes| format!("{}", (bytes as f64 / GB * 100.0).round() / 100.0))
            .unwrap_or_default(),
    )
}

#[cfg(target_family = "wasm")]
fn saved_limits() -> (String, String) {
    Default::default()
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn limits_are_read_as_typed_and_nonsense_is_said_rather_than_dropped() {
        assert_eq!(limits_from("", " "), Ok((None, None)));
        assert_eq!(limits_from("6", "2.5"), Ok((Some(6), Some((2.5 * GB) as u64))));
        assert_eq!(limits_from("", "1,5"), Ok((None, Some((1.5 * GB) as u64))));
        for (agents, memory) in [("0", ""), ("many", ""), ("-1", ""), ("", "0"), ("", "x"), ("", "NaN")] {
            assert!(limits_from(agents, memory).is_err(), "{agents} {memory}");
        }
    }
}

fn make_editor(
    ctx: &mut ViewContext<SettingsView>,
    ui: FamilyId,
    theme: Theme,
    placeholder: &str,
    key: &str,
    update: fn(&mut TaskHostState, String),
) -> ViewHandle<InputEditor> {
    let placeholder = placeholder.to_string();
    let key = format!("taskhost_{key}");
    let handle = ctx.add_typed_action_view(move |inner| {
        InputEditor::new(
            &placeholder,
            ui,
            crate::theme::text_size(13.0),
            theme.main_text(),
            theme.disabled_text(),
            theme.accent,
            theme,
            InputMode::Plain { key },
            inner.focus_handle(),
        )
    });
    ctx.subscribe_to_view(&handle, move |view, _, event, ctx| {
        if let InputEditorEvent::Changed { text, .. } = event {
            update(&mut view.task_hosts, text.clone());
            ctx.notify();
        }
    });
    handle
}

impl TaskHostState {
    pub fn new(ctx: &mut ViewContext<SettingsView>, ui: FamilyId, theme: Theme) -> Self {
        let (max_agents, memory) = saved_limits();
        Self {
            repo_add_editor: make_editor(ctx, ui, theme, "owner/repo", "repo_add", |s, t| {
                s.repo_draft = t
            }),
            team_add_editor: make_editor(ctx, ui, theme, "ENG", "team_add", |s, t| {
                s.team_draft = t
            }),
            display_label_editor: make_editor(
                ctx,
                ui,
                theme,
                "Display label (defaults to machine name)",
                "display_label",
                |s, t| s.display_label_draft = t,
            ),
            worktree_location_editor: make_editor(
                ctx,
                ui,
                theme,
                "/home/user/work  (defaults to machine home)",
                "worktree_location",
                |s, t| s.worktree_location_draft = t,
            ),
            routing_disabled_editor: make_editor(
                ctx,
                ui,
                theme,
                "host or machine id",
                "routing_disabled",
                |s, t| s.routing_disabled_draft = t,
            ),
            max_agents_editor: limit_editor(ctx, ui, theme, "No cap", "max_agents", &max_agents, |s, t| {
                s.max_agents_draft = t
            }),
            memory_limit_editor: limit_editor(ctx, ui, theme, "GB per agent", "memory_limit", &memory, |s, t| {
                s.memory_limit_draft = t
            }),
            repo_draft: String::new(),
            team_draft: String::new(),
            display_label_draft: String::new(),
            worktree_location_draft: String::new(),
            routing_disabled_draft: String::new(),
            max_agents_draft: max_agents,
            memory_limit_draft: memory,
            limits_note: None,
        }
    }

    pub fn sync_inputs(&mut self, theme: Theme, ui: FamilyId, ctx: &mut ViewContext<SettingsView>) {
        let size = crate::theme::scaled(13.0);
        for editor in [
            &self.repo_add_editor,
            &self.team_add_editor,
            &self.display_label_editor,
            &self.worktree_location_editor,
            &self.routing_disabled_editor,
            &self.max_agents_editor,
            &self.memory_limit_editor,
        ] {
            let editor = editor.clone();
            ctx.update_view(&editor, |e, c| {
                e.set_theme(theme, c);
                e.set_font(ui, size, c);
                e.set_menu_font(ui, c);
            });
        }
    }
}

// ── Consumer helpers (functional wiring) ──────────────────────────────────
//
// All readers go through the global snapshot so a save in Settings is
// visible on the next read without disk I/O.

/// Provider keys the task UI shows, default source first.
pub fn ordered_visible_providers() -> Vec<String> {
    crate::settings::AppSettings::global()
        .tasks
        .ordered_providers()
}

pub fn is_provider_visible(provider_key: &str) -> bool {
    crate::settings::AppSettings::global()
        .tasks
        .is_visible(provider_key)
}

pub fn default_source_key() -> String {
    crate::settings::AppSettings::global()
        .tasks
        .default_source
        .trim()
        .to_ascii_lowercase()
}

/// Display name for a machine: host override wins, else the saved name.
pub fn display_name_for(machine_id: &str, fallback: &str) -> String {
    crate::settings::AppSettings::global()
        .hosts
        .display_name_owned(machine_id, fallback)
}

/// Remote base dir for new checkouts on a machine.
pub fn worktree_base_for(machine_id: &str, home_fallback: Option<&str>) -> String {
    crate::settings::AppSettings::global()
        .hosts
        .worktree_base(machine_id, home_fallback)
}

/// Default machine for a new project, given the ids currently available.
pub fn default_machine_id(available: &[String]) -> Option<String> {
    crate::settings::AppSettings::global()
        .hosts
        .default_machine(available)
        .cloned()
}

/// Whether `target` (host display or machine id) may use SSH workspace routing.
pub fn routing_enabled_for(target: &str) -> bool {
    crate::settings::AppSettings::global()
        .hosts
        .routing_enabled_for(target)
}

/// Whether a probe should even try this target.
pub fn probe_allowed_for(target: &str) -> bool {
    let hosts = &crate::settings::AppSettings::global().hosts;
    !hosts
        .routing_probe_skipped
        .iter()
        .any(|t| t == target.trim())
}

/// Backend label for local agent runs ("This machine" / "WSL: <distro>").
pub fn local_execution_label() -> String {
    crate::settings::AppSettings::global()
        .hosts
        .execution_backend_label()
}

/// Wrap a local shell command for the selected backend.
pub fn wrap_local_command(command: &str) -> String {
    crate::settings::AppSettings::global()
        .hosts
        .wrap_local_command(command)
}

// ── Handler ───────────────────────────────────────────────────────────────

impl SettingsView {
    pub(crate) fn handle_task_host_action(
        &mut self,
        action: &TaskHostAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            TaskHostAction::SetDefaultView(preset) => {
                if TASK_VIEW_PRESETS.contains(&preset.as_str()) {
                    self.app_settings.tasks.default_view_preset = preset.clone();
                    self.app_settings.save();
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            TaskHostAction::SetDefaultSource(source) => {
                if TASK_SOURCES.contains(&source.as_str()) {
                    self.app_settings.tasks.default_source = source.clone();
                    self.app_settings.save();
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            TaskHostAction::ToggleVisibleProvider(key) => {
                let visible = !self.app_settings.tasks.is_visible(key);
                // Keep at least one provider visible.
                if !visible && self.app_settings.tasks.visible_set().len() <= 1 {
                    return;
                }
                self.app_settings.tasks.set_visible(key, visible);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::AddDefaultRepo => {
                let draft = self.task_hosts.repo_draft.trim().to_string();
                if self.app_settings.tasks.add_repo(&draft) {
                    self.app_settings.save();
                    self.task_hosts.repo_draft.clear();
                    let editor = self.task_hosts.repo_add_editor.clone();
                    ctx.update_view(&editor, |e, c| e.set_text("", c));
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            TaskHostAction::RemoveDefaultRepo(repo) => {
                self.app_settings.tasks.default_repos.retain(|r| r != repo);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::AddDefaultTeam => {
                let draft = self.task_hosts.team_draft.trim().to_string();
                if self.app_settings.tasks.add_team(&draft) {
                    self.app_settings.save();
                    self.task_hosts.team_draft.clear();
                    let editor = self.task_hosts.team_add_editor.clone();
                    ctx.update_view(&editor, |e, c| e.set_text("", c));
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            TaskHostAction::RemoveDefaultTeam(team) => {
                self.app_settings
                    .tasks
                    .default_linear_teams
                    .retain(|t| t != team);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::SetActiveRuntime(id) => {
                self.app_settings.hosts.active_runtime_id = id.clone();
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::SetHostDisplayLabel(id) => {
                let draft = self.task_hosts.display_label_draft.clone();
                self.app_settings.hosts.set_display_label(id, &draft);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
                ctx.emit(SettingsEvent::MachinesChanged);
            }
            TaskHostAction::SetHostWorktreeLocation(id) => {
                let draft = self.task_hosts.worktree_location_draft.clone();
                self.app_settings.hosts.set_worktree_location(id, &draft);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::SetLocalRuntime(runtime) => {
                self.app_settings.hosts.local_runtime = *runtime;
                // Dropping back to host/auto-without-distro clears a stale pick.
                if matches!(runtime, LocalRuntime::Host) {
                    self.app_settings.hosts.wsl_distro = None;
                }
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::SetWslDistro(distro) => {
                self.app_settings.hosts.wsl_distro = distro.clone();
                if distro.is_some() {
                    self.app_settings.hosts.local_runtime = LocalRuntime::Wsl;
                }
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::ToggleSshRouting => {
                self.app_settings.hosts.ssh_routing_enabled =
                    !self.app_settings.hosts.ssh_routing_enabled;
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::AddRoutingDisabled => {
                let draft = self.task_hosts.routing_disabled_draft.trim().to_string();
                if !draft.is_empty()
                    && !self
                        .app_settings
                        .hosts
                        .routing_disabled_targets
                        .iter()
                        .any(|t| t == &draft)
                {
                    self.app_settings.hosts.routing_disabled_targets.push(draft);
                    self.app_settings.save();
                    self.task_hosts.routing_disabled_draft.clear();
                    let editor = self.task_hosts.routing_disabled_editor.clone();
                    ctx.update_view(&editor, |e, c| e.set_text("", c));
                    ctx.emit(SettingsEvent::AppSettingsChanged);
                }
            }
            TaskHostAction::RemoveRoutingDisabled(target) => {
                self.app_settings
                    .hosts
                    .routing_disabled_targets
                    .retain(|t| t != target);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::ToggleRoutingDisabledTarget(target) => {
                self.app_settings.hosts.toggle_disabled_target(target);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::ToggleProbeSkippedTarget(target) => {
                self.app_settings.hosts.toggle_probe_skipped(target);
                self.app_settings.save();
                ctx.emit(SettingsEvent::AppSettingsChanged);
            }
            TaskHostAction::SaveResourceLimits => {
                let saved = limits_from(
                    &self.task_hosts.max_agents_draft,
                    &self.task_hosts.memory_limit_draft,
                )
                .and_then(save_limits);
                self.task_hosts.limits_note = Some(match saved {
                    Ok(()) => "Saved.".into(),
                    Err(error) => error,
                });
            }
            TaskHostAction::ClearResourceLimits => {
                self.task_hosts.limits_note = save_limits((None, None)).err();
                self.task_hosts.max_agents_draft.clear();
                self.task_hosts.memory_limit_draft.clear();
                for editor in [
                    self.task_hosts.max_agents_editor.clone(),
                    self.task_hosts.memory_limit_editor.clone(),
                ] {
                    ctx.update_view(&editor, |editor, ctx| editor.set_text("", ctx));
                }
            }
        }
        ctx.notify();
    }

    /// Prefill the per-host editors when a machine page opens.
    pub(crate) fn sync_host_override_editors(&mut self, machine_id: &str, ctx: &mut ViewContext<Self>) {
        let hosts = &self.app_settings.hosts;
        let label = hosts
            .overrides
            .get(machine_id)
            .and_then(|o| o.display_label.as_deref())
            .unwrap_or_default();
        let location = hosts
            .overrides
            .get(machine_id)
            .and_then(|o| o.default_worktree_location.as_deref())
            .unwrap_or_default();
        self.task_hosts.display_label_draft = label.to_string();
        self.task_hosts.worktree_location_draft = location.to_string();
        let label_editor = self.task_hosts.display_label_editor.clone();
        let location_editor = self.task_hosts.worktree_location_editor.clone();
        ctx.update_view(&label_editor, |e, c| e.set_text(label, c));
        ctx.update_view(&location_editor, |e, c| e.set_text(location, c));
    }
}

// ── Settings-page cards ───────────────────────────────────────────────────

use crate::machine_ui::{self as mui, Size, Variant};
use crate::shared_input::{shared_input, SharedInputOpts};
use warpui::elements::{
    Container, CrossAxisAlignment, Element, Expanded, Flex, MainAxisSize, ParentElement,
};

fn small_add_button(
    view: &SettingsView,
    theme: &Theme,
    key: &str,
    label: &str,
    action: TaskHostAction,
) -> Box<dyn Element> {
    mui::button(
        theme,
        view.ui_font,
        view.view_id,
        view.hover(&format!("taskhost:{key}")),
        None,
        label,
        Variant::Secondary,
        Size::Xs,
        true,
        task_act(action),
    )
}

fn remove_button(
    view: &SettingsView,
    theme: &Theme,
    key: &str,
    action: TaskHostAction,
) -> Box<dyn Element> {
    mui::button(
        theme,
        view.ui_font,
        view.view_id,
        view.hover(&format!("taskhost:rm:{key}")),
        None,
        "Remove",
        Variant::Ghost,
        Size::Xs,
        true,
        task_act(action),
    )
}

fn text_input(
    view: &SettingsView,
    theme: &Theme,
    editor: &ViewHandle<InputEditor>,
) -> Box<dyn Element> {
    shared_input(
        theme,
        editor,
        SharedInputOpts {
            height: Some(mui::CONTROL_HEIGHT),
            radius: mui::RADIUS_CONTROL,
            pad_left: 11.0,
            pad_right: 11.0,
            ..Default::default()
        },
    )
}

fn provider_human(key: &str) -> &str {
    match key {
        "github" => "GitHub",
        "linear" => "Linear",
        "jira" => "Jira",
        "gitlab" => "GitLab",
        "plane" => "Plane",
        "youtrack" => "YouTrack",
        "asana" => "Asana",
        "trello" => "Trello",
        "monday" => "Monday",
        _ => "Provider",
    }
}

/// Tasks section: defaults + visible-provider multi-toggle + repo/team lists.
/// Rendered at the bottom of Settings → Integrations.
pub fn render_tasks_card(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use super::widgets::{card_rows, setting_row};
    use crate::integrations::{marks, Provider};
    use crate::settings::widgets_controls::{segmented_control, SegmentOption};

    let ui = view.ui_font;
    let tasks = &view.app_settings.tasks;
    let group = |title: &str, caption: Option<&str>, body: Box<dyn Element>| -> Box<dyn Element> {
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(warpui::elements::MainAxisSize::Min)
            .with_spacing(10.0)
            .with_child(mui::eyebrow(ui, theme, title));
        if let Some(caption) = caption {
            column = column.with_child(mui::text(ui, caption, 12.0, theme.sub_text()));
        }
        column.with_child(body).finish()
    };
    // A field with its button, as the last row of a list.
    let adder = |editor: &ViewHandle<InputEditor>, key: &str, action: TaskHostAction| {
        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(8.0)
                .with_child(Expanded::new(1.0, text_input(view, theme, editor)).finish())
                .with_child(small_add_button(view, theme, key, "Add", action))
                .finish(),
        )
        .with_padding_left(mui::ROW_PAD_X)
        .with_padding_right(mui::ROW_PAD_X - 4.0)
        .with_padding_top(mui::ROW_PAD_Y)
        .with_padding_bottom(mui::ROW_PAD_Y)
        .finish()
    };

    let source_options = TASK_SOURCES
        .iter()
        .map(|source| SegmentOption {
            label: provider_human(source).to_string(),
            selected: tasks.default_source == *source,
            hover: view.hover(&format!("taskhost:source:{source}")),
            action: task_act(TaskHostAction::SetDefaultSource(source.to_string())),
        })
        .collect();
    let defaults = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui,
                "Default source",
                Some("Listed first when you link work to a new workspace."),
                segmented_control(theme, ui, view.view_id, source_options),
            ),
        ],
    );

    // Which services the task pickers offer, each with its logo.
    let pickers = TASK_PROVIDER_KEYS
        .iter()
        .map(|key| {
            let visible = tasks.is_visible(key);
            let logo = Provider::ALL
                .into_iter()
                .find(|provider| provider.key() == *key)
                .map(|provider| marks::tile(theme, ui, provider, 28.0));
            mui::list_row(
                ui,
                theme,
                logo,
                mui::medium(ui, provider_human(key), 13.5, theme.main_text()),
                Some(if visible { "Offered when linking work" } else { "Hidden from task pickers" }.into()),
                vec![mui::switch(
                    theme,
                    view.view_id,
                    view.hover(&format!("taskhost:vis:{key}")),
                    visible,
                    task_act(TaskHostAction::ToggleVisibleProvider(key.to_string())),
                )],
            )
        })
        .collect();

    let mut repo_rows: Vec<Box<dyn Element>> = tasks
        .default_repos
        .iter()
        .map(|repo| {
            mui::list_row(
                ui,
                theme,
                None,
                mui::medium(ui, repo.clone(), 13.0, theme.main_text()),
                None,
                vec![remove_button(
                    view,
                    theme,
                    &format!("repo:{repo}"),
                    TaskHostAction::RemoveDefaultRepo(repo.clone()),
                )],
            )
        })
        .collect();
    repo_rows.push(adder(&view.task_hosts.repo_add_editor, "repo-add", TaskHostAction::AddDefaultRepo));

    let mut team_rows: Vec<Box<dyn Element>> = tasks
        .default_linear_teams
        .iter()
        .map(|team| {
            mui::list_row(
                ui,
                theme,
                None,
                mui::medium(ui, team.clone(), 13.0, theme.main_text()),
                None,
                vec![remove_button(
                    view,
                    theme,
                    &format!("team:{team}"),
                    TaskHostAction::RemoveDefaultTeam(team.clone()),
                )],
            )
        })
        .collect();
    team_rows.push(adder(&view.task_hosts.team_add_editor, "team-add", TaskHostAction::AddDefaultTeam));

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(warpui::elements::MainAxisSize::Min)
        .with_spacing(28.0)
        .with_child(defaults)
        .with_child(group("Task pickers", None, mui::list_card(theme, pickers)))
        .with_child(group(
            "Default repository",
            Some("Where agents file a new issue when they don't name one (owner/repo). The first is used."),
            mui::list_card(theme, repo_rows),
        ))
        .with_child(group(
            "Default Linear team",
            Some("Where agents file a new Linear issue when they don't name a team. The first is used."),
            mui::list_card(theme, team_rows),
        ))
        .finish()
}

/// Host defaults card for Settings → Machines list: default host, local
/// runtime + WSL distro, SSH routing + disabled targets. The machine rows
/// themselves are the remote-servers list (add/remove reused).
pub fn render_host_settings_card(
    view: &SettingsView,
    theme: &Theme,
    machines: &[(String, String)],
) -> Box<dyn Element> {
    use super::widgets::{card_rows, setting_group, toggle_row};
    use crate::settings::widgets_controls::{select_control, SelectItem, SegmentOption, segmented_control};

    let ui = view.ui_font;
    let hosts = &view.app_settings.hosts;
    // `mui::settings_row` has no padding of its own; match `setting_row`.
    let padded = |row: Box<dyn Element>| {
        Container::new(row)
            .with_padding_left(16.0)
            .with_padding_right(16.0)
            .with_padding_top(13.0)
            .with_padding_bottom(13.0)
            .finish()
    };

    // Default host for new projects.
    let mut host_rows: Vec<Box<dyn Element>> = Vec::new();
    {
        let mut items = vec![SelectItem {
            label: "First available".into(),
            selected: hosts.active_runtime_id.is_none(),
            enabled: true,
            hover: view.hover("taskhost:defhost:none"),
            action: task_act(TaskHostAction::SetActiveRuntime(None)),
        }];
        for (id, name) in machines {
            let selected = hosts.active_runtime_id.as_deref() == Some(id.as_str());
            items.push(SelectItem {
                label: name.clone(),
                selected,
                enabled: true,
                hover: view.hover(&format!("taskhost:defhost:{id}")),
                action: task_act(TaskHostAction::SetActiveRuntime(Some(id.clone()))),
            });
        }
        let current = hosts
            .active_runtime_id
            .as_deref()
            .and_then(|id| machines.iter().find(|(mid, _)| mid == id))
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "First available".to_string());
        host_rows.push(padded(mui::settings_row(
            ui,
            theme,
            "Default host for new projects",
            vec![],
            Some(mui::text(
                ui,
                "Add-project dialogs open on this machine when it exists.",
                12.0,
                theme.sub_text(),
            )),
            Some(select_control(
                theme,
                ui,
                view.view_id,
                view.hover("taskhost:defhost"),
                &current,
                240.0,
                view.picker_open("taskhost:defhost"),
                SettingsAction::TogglePicker("taskhost:defhost".into()),
                items,
            )),
        )));
    }

    // Local runtime and WSL distro only mean something where WSL exists.
    if cfg!(windows) {
        // Local runtime: auto / host / wsl.
        let runtime_options = LocalRuntime::all()
            .into_iter()
            .map(|runtime| SegmentOption {
                label: runtime.label().to_string(),
                selected: hosts.local_runtime == runtime,
                hover: view.hover(&format!("taskhost:runtime:{:?}", runtime)),
                action: task_act(TaskHostAction::SetLocalRuntime(runtime)),
            })
            .collect();
        host_rows.push(padded(mui::settings_row(
            ui,
            theme,
            "Local runtime",
            vec![],
            Some(mui::text(
                ui,
                format!("Local agent runs use {}.", hosts.execution_backend_label()),
                12.0,
                theme.sub_text(),
            )),
            Some(segmented_control(theme, ui, view.view_id, runtime_options)),
        )));

        // WSL distro picker (options from this machine; manual entry stays via save).
        {
            let mut distros = available_wsl_distros();
            if let Some(current) = hosts.wsl_distro.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if !distros.iter().any(|d| d == current) {
                    distros.push(current.to_string());
                }
            }
            distros.sort();
            distros.dedup();
            let mut items = vec![SelectItem {
                label: "None".into(),
                selected: hosts.wsl_distro.is_none(),
                enabled: true,
                hover: view.hover("taskhost:wsl:none"),
                action: task_act(TaskHostAction::SetWslDistro(None)),
            }];
            for distro in &distros {
                let selected = hosts.wsl_distro.as_deref() == Some(distro.as_str());
                items.push(SelectItem {
                    label: distro.clone(),
                    selected,
                    enabled: true,
                    hover: view.hover(&format!("taskhost:wsl:{distro}")),
                    action: task_act(TaskHostAction::SetWslDistro(Some(distro.clone()))),
                });
            }
            let current = hosts.wsl_distro.clone().unwrap_or_else(|| "None".to_string());
            host_rows.push(padded(mui::settings_row(
                ui,
                theme,
                "WSL distro",
                vec![],
                Some(mui::text(
                    ui,
                    "Used when the local runtime is WSL. Empty on machines without WSL.",
                    12.0,
                    theme.sub_text(),
                )),
                Some(select_control(
                    theme,
                    ui,
                    view.view_id,
                    view.hover("taskhost:wsl"),
                    &current,
                    200.0,
                    view.picker_open("taskhost:wsl"),
                    SettingsAction::TogglePicker("taskhost:wsl".into()),
                    items,
                )),
            )));
        }
    }

    let mut column = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(mui::eyebrow(ui, theme, "Hosts"))
        .with_child(card_rows(theme, host_rows));

    // Native only: the web viewer runs no agents.
    #[cfg(not(target_family = "wasm"))]
    {
        let caption = view.task_hosts.limits_note.clone().unwrap_or_else(|| {
            "Left empty, any number of agents start; new ones wait only while memory is critically low. \
             An agent over the memory limit is paused until you resume it; with no limit set, only one \
             that takes over a quarter of the memory while it is critically low."
                .into()
        });
        let save = small_add_button(view, theme, "resources-save", "Save", TaskHostAction::SaveResourceLimits);
        let clear = small_add_button(view, theme, "resources-clear", "Clear", TaskHostAction::ClearResourceLimits);
        let editors = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, text_input(view, theme, &view.task_hosts.max_agents_editor)).finish())
            .with_child(Expanded::new(1.0, text_input(view, theme, &view.task_hosts.memory_limit_editor)).finish())
            .with_child(save)
            .with_child(clear)
            .finish();
        let mut resource_rows = vec![padded(mui::settings_row(
            ui,
            theme,
            "Agents at once · memory limit per agent (GB)",
            vec![],
            Some(mui::text(ui, caption, 12.0, theme.sub_text())),
            None,
        ))];
        resource_rows.push(editors);
        column = column.with_child(setting_group(theme, ui, "Resources", vec![mui::list_card(theme, resource_rows)]));
    }

    // SSH workspace routing.
    let mut routing_rows = vec![toggle_row(
        theme,
        ui,
        view.view_id,
        view.hover("taskhost:routing"),
        "SSH workspace routing",
        Some("Route remote work through Dragent's workspace runtime instead of plain SSH."),
        hosts.ssh_routing_enabled,
        task_act(TaskHostAction::ToggleSshRouting),
    )];
    let disabled: Vec<Box<dyn Element>> = hosts
        .routing_disabled_targets
        .iter()
        .map(|target| {
            mui::list_row(
                ui,
                theme,
                None,
                mui::medium(ui, target.clone(), 13.0, theme.main_text()),
                Some("Plain SSH (routing skipped)".into()),
                vec![
                    remove_button(view, theme, &format!("routedis:{target}"), TaskHostAction::RemoveRoutingDisabled(target.clone())),
                ],
            )
        })
        .collect();
    let mut disabled_card_rows = disabled;
    disabled_card_rows.push(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.0)
            .with_child(Expanded::new(1.0, text_input(view, theme, &view.task_hosts.routing_disabled_editor)).finish())
            .with_child(small_add_button(view, theme, "routing-add", "Skip", TaskHostAction::AddRoutingDisabled))
            .finish(),
    );
    routing_rows.push(padded(mui::settings_row(
        ui,
        theme,
        "Targets without routing",
        vec![],
        Some(mui::text(
            ui,
            "These hosts and machines always fall back to plain SSH, and probes skip anything in the probe-skipped list.",
            12.0,
            theme.sub_text(),
        )),
        None,
    )));
    column = column
        .with_child(setting_group(theme, ui, "SSH workspace routing", vec![card_rows(theme, routing_rows)]))
        .with_child(setting_group(
            theme,
            ui,
            "Routing-skipped targets",
            vec![mui::list_card(theme, disabled_card_rows)],
        ));

    // Probe-skipped targets (per-target toggles for known machines).
    if !machines.is_empty() {
        let mut probe_rows: Vec<Box<dyn Element>> = machines
            .iter()
            .map(|(id, name)| {
                let skipped = hosts.routing_probe_skipped.iter().any(|t| t == id);
                toggle_row(
                    theme,
                    ui,
                    view.view_id,
                    view.hover(&format!("taskhost:probe:{id}")),
                    name,
                    Some(if skipped { "Probe skipped." } else { "Probed on connect." }),
                    !skipped,
                    task_act(TaskHostAction::ToggleProbeSkippedTarget(id.clone())),
                )
            })
            .collect();
        // Custom entries already skipped but not a known machine.
        for target in &hosts.routing_probe_skipped {
            if machines.iter().any(|(id, _)| id == target) {
                continue;
            }
            probe_rows.push(toggle_row(
                theme,
                ui,
                view.view_id,
                view.hover(&format!("taskhost:probe:{target}")),
                target,
                Some("Probe skipped."),
                false,
                task_act(TaskHostAction::ToggleProbeSkippedTarget(target.clone())),
            ));
        }
        column = column.with_child(setting_group(theme, ui, "Probe-skipped targets", vec![card_rows(theme, probe_rows)]));
    }

    Container::new(column.finish())
        .with_padding_top(8.0)
        .finish()
}

/// Per-host overrides on the machine detail page: display label + default
/// worktree location, saved to [`HostSettings`](crate::settings::task_settings::HostSettings).
pub fn render_host_override_card(view: &SettingsView, theme: &Theme, machine_id: &str) -> Box<dyn Element> {
    use super::widgets::setting_group;

    let ui = view.ui_font;
    let save_label = small_add_button(view, theme, &format!("label-save:{machine_id}"), "Save", TaskHostAction::SetHostDisplayLabel(machine_id.to_string()));
    let save_location = small_add_button(
        view,
        theme,
        &format!("loc-save:{machine_id}"),
        "Save",
        TaskHostAction::SetHostWorktreeLocation(machine_id.to_string()),
    );
    let label_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(Expanded::new(1.0, text_input(view, theme, &view.task_hosts.display_label_editor)).finish())
        .with_child(save_label)
        .finish();
    let location_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.0)
        .with_child(Expanded::new(1.0, text_input(view, theme, &view.task_hosts.worktree_location_editor)).finish())
        .with_child(save_location)
        .finish();
    setting_group(
        theme,
        ui,
        "Host display & worktrees",
        vec![
            mui::field(ui, theme, "Display label", label_row, Some("Shown in lists and the new-project screen. Empty restores the machine name.")),
            mui::field(
                ui,
                theme,
                "Default worktree location",
                location_row,
                Some("New checkouts on this host start here. Empty restores the machine home."),
            ),
        ],
    )
}

/// All known hosts as (id, effective display name) for pickers.
pub fn host_picker_entries(
    registry_machines: &[(String, String, String)],
) -> Vec<(String, String)> {
    let hosts = &crate::settings::AppSettings::global().hosts;
    registry_machines
        .iter()
        .map(|(id, _raw_host, fallback)| (id.clone(), hosts.display_name_owned(id, fallback)))
        .collect()
}

#[allow(dead_code)]
pub fn _keep_hashmap_import(_: &HashMap<String, String>) {}
