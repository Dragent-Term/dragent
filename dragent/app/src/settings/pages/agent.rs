use warpui::color::ColorU;
use warpui::elements::{
    ChildView, Container, CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize,
    ParentElement, Shrinkable, Text,
};
use warpui::AppContext;

use crate::theme::Theme;
use warpui::{GetSingletonModelHandle, ReadModel, SingletonEntity};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::super::action::SettingsAction;
use super::super::app_settings::{ApprovalMode, ContextScope, McpTransport};
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    alert_banner, card_rows, choice_row, group_label, info_row, mcp_installed_row, page_header,
    primary_button, secondary_button, segmented_control, select_control, setting_group,
    setting_row, slider_control, toggle_row, SegmentOption, SelectItem,
};
use crate::workspace::PromptAgent;

pub fn render_agent_general(
    view: &SettingsView,
    theme: &Theme,
    _app: &AppContext,
) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let agent = &view.app_settings.agent;

    // ── Autonomy ──────────────────────────────────────────────────────────────
    let approval = agent.approval_mode();
    let approval_segmented = segmented_control(
        theme,
        ui_font,
        id,
        ApprovalMode::ALL
            .into_iter()
            .map(|mode| SegmentOption {
                label: mode.label().to_string(),
                selected: approval == mode,
                hover: view.hover(&format!("approval_{mode:?}")),
                action: SettingsAction::SelectApprovalMode(mode),
            })
            .collect(),
    );
    let approval_summary = format!(
        "{} Agents you start in terminals follow their own launch settings.",
        approval.summary()
    );

    // ── Idle agents sleep ─────────────────────────────────────────────────────
    let idle_sleep = agent.idle_sleep_minutes;
    let idle_sleep_segmented = segmented_control(
        theme,
        ui_font,
        id,
        dragent_agent::autonomy::IDLE_SLEEP_CHOICES
            .into_iter()
            .map(|minutes| SegmentOption {
                label: match minutes {
                    0 => "Never".to_string(),
                    60 => "1 hour".to_string(),
                    m => format!("{m} min"),
                },
                selected: idle_sleep == minutes,
                hover: view.hover(&format!("idle_sleep_{minutes}")),
                action: SettingsAction::SelectIdleSleepMinutes(minutes),
            })
            .collect(),
    );
    let idle_sleep_card = card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "Put idle agents to sleep after",
            Some(
                "An agent tab that has finished and sits unused off screen is stopped to free \
                 memory. Its tab and history stay; opening it resumes the same conversation. \
                 Agents that are working, waiting for you, serving something or open on your \
                 phone stay awake. When memory runs low, idle agents sleep after 2 minutes.",
            ),
            idle_sleep_segmented,
        )],
    );

    let autonomy_card = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui_font,
                "Approval mode",
                Some(&approval_summary),
                approval_segmented,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("auto_run_safe"),
                "Auto-run safe commands",
                Some("Let the agent run read-only commands (ls, cat, git status) without asking."),
                agent.auto_run_safe_commands,
                SettingsAction::ToggleAutoRunSafeCommands,
            ),
        ],
    );

    let experimental_card = card_rows(
        theme,
        vec![toggle_row(
            theme,
            ui_font,
            id,
            view.hover("agentic_workspace"),
            "Work items (earlier workflow)",
            Some("The Shape → Coordinate → Review work items that Agent Coordination replaces. Turn on to keep starting them from the + menu on this device; work items you already have open either way."),
            agent.agentic_workspace_enabled,
            SettingsAction::ToggleAgenticWorkspace,
        )],
    );

    // ── Self-healing ──────────────────────────────────────────────────────────
    let scope_select = select_control(
        theme,
        ui_font,
        id,
        view.hover("ctx_scope_btn"),
        agent.context_scope.label(),
        180.0,
        view.picker_open("ctx_scope"),
        SettingsAction::TogglePicker("ctx_scope".to_string()),
        [
            ContextScope::CurrentDir,
            ContextScope::Workspace,
            ContextScope::GitTracked,
        ]
        .into_iter()
        .map(|scope| SelectItem {
            label: scope.label().to_string(),
            selected: agent.context_scope == scope,
            enabled: true,
            hover: view.hover(&format!("ctx_scope_{scope:?}")),
            action: SettingsAction::SelectContextScope(scope),
        })
        .collect(),
    );

    let self_healing_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("proactive_scan"),
                "Proactive error scanning",
                Some("Analyze failed commands in the background and surface fixes before you ask."),
                agent.proactive_error_scanning,
                SettingsAction::ToggleProactiveErrorScanning,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("auto_fix"),
                "Auto-fix on failure",
                Some("Show the suggested fix card the moment an analysis finds one."),
                agent.auto_fix_on_failure,
                SettingsAction::ToggleAutoFixOnFailure,
            ),
            setting_row(
                theme,
                ui_font,
                "Context scope",
                Some("Which files the agent's automatic code context may draw from."),
                scope_select,
            ),
        ],
    );

    Flex::column()
        .with_cross_axis_alignment(warpui::elements::CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui_font,
            "General",
            "Control how the Dragent agent behaves when assisting you in the terminal.",
        ))
        .with_child(group_label(theme, ui_font, "Experimental"))
        .with_child(experimental_card)
        .with_child(group_label(theme, ui_font, "Autonomy"))
        .with_child(autonomy_card)
        .with_child(group_label(theme, ui_font, "Self-healing"))
        .with_child(self_healing_card)
        .with_child(group_label(theme, ui_font, "Agents"))
        .with_child(render_agent_picker_card(view, theme, ui_font, id))
        .with_child(group_label(theme, ui_font, "Memory"))
        .with_child(idle_sleep_card)
        .with_child(group_label(theme, ui_font, "Agents offered"))
        .with_child(render_disabled_agents_card(view, theme, ui_font, id))
        .with_child(group_label(theme, ui_font, "Workspace"))
        .with_child(render_workspace_trust_card(view, theme, ui_font, id))
        .with_child(group_label(theme, ui_font, "Codex"))
        .with_child(render_codex_launch_card(view, theme, ui_font, id))
        .finish()
}

/// The agent CLIs installed on this computer, as the last check found them.
fn installed_agents() -> Vec<PromptAgent> {
    #[cfg(not(target_family = "wasm"))]
    {
        crate::workspace::installed_agents()
    }
    #[cfg(target_family = "wasm")]
    {
        Vec::new()
    }
}

/// The agent a stored key (`claude`, `codex`, …) names.
fn agent_for_key(key: &str) -> Option<PromptAgent> {
    PromptAgent::ALL
        .into_iter()
        .find(|candidate| candidate.command_key() == key)
}

/// The name of the agent a stored key names, or the key itself.
fn agent_name(key: &str) -> String {
    agent_for_key(key).map_or_else(|| key.to_string(), |agent| agent.label().to_string())
}

/// An agent's icon, for the label of its row.
fn agent_icon(theme: &Theme, agent: PromptAgent) -> Box<dyn Element> {
    crate::workspace::prompt_agent_image(agent, theme, 16.0, 1.0)
}

/// Agents section: the default agent, picked with the prompt screen's own
/// chip and menu (installed agents only).
fn render_agent_picker_card(
    view: &SettingsView,
    theme: &Theme,
    ui_font: warpui::fonts::FamilyId,
    id: warpui::EntityId,
) -> Box<dyn Element> {
    let current = view
        .app_settings
        .agent
        .default_agent
        .as_deref()
        .and_then(agent_for_key);
    let toggle = move |ectx: &mut warpui::EventContext| {
        ectx.dispatch_typed_action_for(id, SettingsAction::TogglePicker("default_agent".to_string()))
    };
    let chip = match current {
        Some(agent) => crate::workspace::agent_chip(theme, view.ui_font, Some(agent), view.hover("default_agent_btn"), toggle),
        None => crate::workspace::labelled_agent_chip(
            theme,
            view.ui_font,
            crate::icons::render_icon(crate::icons::AppIcon::Agent, theme.sub_text(), 16.0),
            "System default".to_string(),
            view.hover("default_agent_btn"),
            toggle,
        ),
    };
    let mut control = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(10.0)
        .with_child(warpui::elements::SavePosition::new(chip, super::super::settings_view::DEFAULT_AGENT_CHIP_TAG).finish());
    if current.is_some() {
        control = control.with_child(super::super::widgets::link_button(
            theme,
            ui_font,
            id,
            view.hover("default_agent_reset"),
            "Use default",
            SettingsAction::SelectDefaultAgent(None),
        ));
    }
    card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "Default agent",
            Some("The agent new tabs and runs start with. Turned-off agents are skipped."),
            control.finish(),
        )],
    )
}

/// The default-agent menu, opened under its chip: the prompt screen's agent
/// menu, listing the installed agents only.
pub fn render_default_agent_menu(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let id = view.view_id;
    let installed: std::collections::HashSet<PromptAgent> = installed_agents().into_iter().collect();
    let selected = view
        .app_settings
        .agent
        .default_agent
        .as_deref()
        .and_then(agent_for_key);
    let menu = crate::workspace::render_agent_menu(crate::workspace::AgentMenuProps {
        theme,
        ui: view.ui_font,
        states: &view.hover_states,
        scroll: &view.agent_menu_scroll,
        search_editor: &view.agent_menu_search_editor,
        search: &view.agent_menu_search,
        selected,
        installed: &installed,
        checking: false,
        installed_only: true,
        with_dragent: true,
        pick: std::rc::Rc::new(move |ectx, agent, _| {
            ectx.dispatch_typed_action_for(
                id,
                SettingsAction::SelectDefaultAgent(Some(agent.command_key().to_string())),
            );
        }),
        clear_search: std::rc::Rc::new(move |ectx| {
            ectx.dispatch_typed_action_for(id, SettingsAction::ClearAgentMenuSearch);
        }),
    });
    warpui_core::elements::Dismiss::new(menu)
        .on_dismiss(move |ectx, _| {
            ectx.dispatch_typed_action_for(id, SettingsAction::TogglePicker("default_agent".to_string()))
        })
        .prevent_interaction_with_other_elements()
        .finish()
}

/// Which installed agents Dragent offers: a switch per agent (on means
/// offered).
fn render_disabled_agents_card(
    view: &SettingsView,
    theme: &Theme,
    ui_font: warpui::fonts::FamilyId,
    id: warpui::EntityId,
) -> Box<dyn Element> {
    let installed = installed_agents();
    if installed.is_empty() {
        return card_rows(
            theme,
            vec![info_row(
                theme,
                ui_font,
                "No coding agents found",
                "install one from the new-workspace screen's agent menu.",
            )],
        );
    }
    let rows = installed
        .into_iter()
        .map(|candidate| {
            let key = candidate.command_key().to_string();
            let offered = !view.app_settings.agent.is_agent_disabled(&key);
            super::super::widgets::toggle_row_with_icon(
                theme,
                ui_font,
                id,
                view.hover(&format!("disabled_agent_{key}")),
                Some(agent_icon(theme, candidate)),
                candidate.label(),
                Some(if offered {
                    "In agent menus and launches."
                } else {
                    "Kept out of agent menus, launches and the default-agent picker."
                }),
                offered,
                SettingsAction::ToggleDisabledAgent(key),
            )
        })
        .collect();
    card_rows(theme, rows)
}

/// Workspace trust gate.
fn render_workspace_trust_card(
    view: &SettingsView,
    theme: &Theme,
    ui_font: warpui::fonts::FamilyId,
    id: warpui::EntityId,
) -> Box<dyn Element> {
    card_rows(
        theme,
        vec![toggle_row(
            theme,
            ui_font,
            id,
            view.hover("workspace_trust"),
            "Workspace trust",
            Some("Ask before opening untrusted folders. Off opens everything as if trusted."),
            view.app_settings.agent.workspace_trust_enabled,
            SettingsAction::ToggleWorkspaceTrust,
        )],
    )
}

fn render_codex_launch_card(
    view: &SettingsView,
    theme: &Theme,
    ui_font: warpui::fonts::FamilyId,
    id: warpui::EntityId,
) -> Box<dyn Element> {
    let agent = &view.app_settings.agent;
    card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("server_isolation"),
                "Let Codex skip its sandbox and approvals",
                Some("On keeps bypass flags in Codex launch commands. Off uses Codex's sandbox and approval prompts."),
                agent.terminal_server_isolation,
                SettingsAction::ToggleServerIsolation,
            ),
        ],
    )
}

pub fn render_agent_mcp(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    let servers = &view.app_settings.mcp_servers;
    let q = view.mcp_search_query.to_lowercase();

    // ── Header ────────────────────────────────────────────────────────────────
    let header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(page_header(
            theme,
            ui,
            "MCP Servers",
            "Connect Model Context Protocol servers to give the agent new tools and data sources.",
        ))
        .with_child(primary_button(
            theme,
            ui,
            id,
            view.hover("mcp_add_custom"),
            "+ Add",
            SettingsAction::ShowMcpForm(None),
        ))
        .finish();

    // ── Search bar ────────────────────────────────────────────────────────────
    let search_bar = crate::shared_input::shared_input(
        theme,
        &view.mcp_search_editor,
        crate::shared_input::SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: crate::icons::AppIcon::Search,
                size: 14.0,
            }),
            width: crate::shared_input::SharedInputWidth::Fill,
            radius: 8.0,
            background: Some(theme.surface_1()),
            border: Some(theme.outline()),
            pad_left: 12.0,
            pad_right: 12.0,
            pad_top: 8.0,
            pad_bottom: 8.0,
            spacing: 8.0,
            ..Default::default()
        },
    );

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(16.0)
        .with_child(header)
        .with_child(search_bar);

    // A Dev build never writes into other apps' settings, so the servers
    // below stay inside this build.
    if dragent_paths::is_dev() {
        col = col.with_child(
            Text::new(
                "Dev builds don't change other apps' settings.".to_string(),
                ui,
                crate::theme::text_size(12.5),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    }

    // ── MY SERVERS ────────────────────────────────────────────────────────────
    let installed: Vec<_> = servers
        .iter()
        .enumerate()
        .filter(|(_, s)| q.is_empty() || s.name.to_lowercase().contains(&q))
        .collect();

    if !installed.is_empty() {
        col = col.with_child(group_label(theme, ui, "My servers"));
        let mut list = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min);

        for (i, (idx, entry)) in installed.iter().enumerate() {
            if i > 0 {
                list = list.with_child(
                    Container::new(warpui::elements::Empty::new().finish())
                        .with_border(Border::top(1.0).with_border_color(theme.outline()))
                        .finish(),
                );
            }
            let is_sse = entry.transport == McpTransport::Sse;
            let tool_count = crate::agent::mcp::tool_count_for(&entry.name);
            let prompt_count = crate::agent::mcp::prompt_count_for(&entry.name);
            let resource_count = crate::agent::mcp::resource_count_for(&entry.name);
            let tool_names = crate::agent::mcp::tools_for(&entry.name);
            let prompt_names = crate::agent::mcp::prompts_for(&entry.name);
            let resource_names = crate::agent::mcp::resources_for(&entry.name);

            // The counts live on the row's expand toggle, not in the address.
            let desc = if is_sse {
                format!("SSE · {}", entry.command)
            } else {
                entry.display_command()
            };
            let has_oauth = crate::settings::settings_view::mcp_token_exists(&entry.name);

            // SSE servers without a stored token → OAuth discovery flow.
            // Stdio servers with any empty env var (e.g. GITHUB_TOKEN) → open Edit
            // form so the user can fill in the required value.
            let needs_oauth = is_sse && !has_oauth;
            let needs_env_setup = !is_sse && entry.env.values().any(|v| v.is_empty());

            let (connect_hover, connect_action) = if needs_oauth {
                (
                    Some(view.hover(&format!("mcp_connect_{idx}"))),
                    Some(SettingsAction::TriggerMcpOAuth(*idx)),
                )
            } else if needs_env_setup {
                (
                    Some(view.hover(&format!("mcp_connect_{idx}"))),
                    Some(SettingsAction::ShowMcpForm(Some(*idx))),
                )
            } else {
                (None, None)
            };

            let disconnect_hover = if is_sse && has_oauth {
                Some(view.hover(&format!("mcp_disconnect_{idx}")))
            } else {
                None
            };
            list = list.with_child(mcp_installed_row(
                theme,
                ui,
                id,
                view.hover(&format!("mcp_edit_{idx}")),
                view.hover(&format!("mcp_toggle_{idx}")),
                view.hover(&format!("mcp_remove_{idx}")),
                connect_hover,
                connect_action,
                disconnect_hover,
                view.hover(&format!("mcp_capabilities_{idx}")),
                &entry.name,
                &desc,
                crate::agent::mcp_agent_sync::agent_labels(&entry.sync_agents),
                *idx,
                entry.enabled,
                has_oauth,
                view.mcp_expanded.contains(&entry.name),
                tool_count,
                tool_names,
                prompt_count,
                prompt_names,
                resource_count,
                resource_names,
            ));
        }

        col = col.with_child(
            Container::new(list.finish())
                .with_background_color(ColorU::new(0, 0, 0, 0))
                .with_border(Border::all(1.0).with_border_color(theme.outline()))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .with_padding_left(12.0)
                .with_padding_right(12.0)
                .finish(),
        );
    }

    // ── Status ────────────────────────────────────────────────────────────────
    if let Some(msg) = &view.mcp_status_message {
        col = col.with_child(alert_banner(theme, ui, msg));
    }

    col.with_child(super::skills::render_mcp_access(view, theme)).finish()
}
