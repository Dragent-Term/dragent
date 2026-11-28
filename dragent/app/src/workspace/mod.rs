mod action;
mod action_agent;
mod action_agents;
mod action_editor;
mod action_explorer;
mod action_menus;
mod action_projects;
mod action_review_git;
mod action_settings;
mod action_shaping;
mod action_sharing;
mod action_tabs;
mod action_terminal;
mod action_workspace_tabs;
mod agent_definition;
mod agent_rail;
pub(crate) mod agent_sleep;
mod agents_panel;
mod auth_modals;
#[cfg(not(target_family = "wasm"))]
mod automation_runs;
mod bindings;
mod browser_design;
mod browser_tabs;
mod build_finish;
mod chrome_inset;
mod composer_input;
#[cfg(test)]
mod composer_input_tests;
mod agent_resources;
mod content_header;
mod coordination_principal;
mod docked_terminal;
pub(crate) mod documents_entries;
mod documents_ignore;
mod documents_imports;
mod documents_io;
mod editor_palette;
mod editor_render;
mod feedback;
#[cfg(not(target_family = "wasm"))]
mod linked_issue;
pub(crate) use editor_render::highlighted_source;
#[cfg(not(target_family = "wasm"))]
mod cli_requests;
#[cfg(not(target_family = "wasm"))]
mod computer_tools;
#[cfg(not(target_family = "wasm"))]
mod companion_starts;
mod companion_tabs;
pub(crate) mod desktop_catalog;
mod diff_comments;
mod editor_tabs;
mod error_notices;
mod exec_view_attention;
mod exec_view_cards;
mod exec_view_main;
mod exec_view_tasks;
mod execution_delivery;
mod execution_workspace;
pub mod git_graph;
pub mod git_status_bar;
mod header;
mod header_menus;
#[cfg(not(target_family = "wasm"))]
mod header_usage;
#[cfg(not(target_family = "wasm"))]
mod past_sessions;
pub(crate) use header_menus::{attention_menu_overlay, render_attention_bell, AutomationNotice};
pub(crate) use header::render_dev_badge;
pub(crate) mod host_mirror;
mod host_tabs;
mod ide_chrome;
mod input_editor;
mod language_nav;
pub(crate) mod language_services_tray;
mod left_panel;
pub mod multi_diff_editor;
mod org_menu;
pub(crate) use org_menu::render_demo_org_footer;
mod overlays_sharing;
mod pane;
mod pane_layout;
pub(crate) mod panel_coordination;
pub(crate) use panel_coordination::preview as panel_coordination_preview;
pub(crate) use panel_tabs::hover_card_preview;
pub(crate) use header_usage::usage_popover_preview;
mod panel_nav;
mod panel_tabs;
pub mod persistence;
mod ports_bar;
pub mod prewarm;
mod principal_normalize;
mod principal_stream;
mod principal_text;
mod principal_tools;
mod principal_types;
mod principal_validate;
mod project_context;
mod project_data;
pub(crate) mod project_icons;
mod project_documents;
mod project_links;
pub(crate) mod project_naming;
mod settings_projects;
mod worktree_recovery;
#[cfg(not(target_family = "wasm"))]
pub(crate) use computer_tools::render_computer_preview;
#[cfg(not(target_family = "wasm"))]
mod cli_workers;
#[cfg(not(target_family = "wasm"))]
mod pane_tools;
#[cfg(test)]
mod pane_focus_tests;
#[cfg(all(test, unix))]
mod popup_terminal_tests;
#[cfg(test)]
mod import_worktree_tests;
#[cfg(not(target_family = "wasm"))]
pub(crate) use cli_workers::{installed_agents, remember_installed_agents};
mod project_prompt;
pub(crate) use project_prompt::render_embedded_terminal_modal;
#[cfg(not(target_family = "wasm"))]
pub(crate) use project_prompt::render_review_setup;
pub mod project_sidebar;
mod project_start;
pub(crate) mod project_workspace_creation;
mod render_editor;
mod render_layout;
mod render_overlays;
mod render_picker;
mod team_prompt;
#[cfg(not(target_family = "wasm"))]
mod teams_page;
#[cfg(not(target_family = "wasm"))]
mod tasks_host;
pub(crate) use tasks_host::origin_url;
pub(crate) use render_picker::render_header_new_tab_menu;
pub(crate) mod add_project;
mod documents_remote;
pub(crate) mod remote_project;
pub(crate) mod review_pr_service;
mod review_workspace;
mod search_panel;
mod shape_build;
mod shape_parse;
mod shape_principal;
mod shape_view_composer;
mod shape_view_edit;
mod shape_view_nav;
mod shape_view_plan;
mod shape_view_roles;
mod shape_workspace;
mod shaping_edit;
mod shaping_turn;
mod shaping_voice;
pub(crate) mod share_sheet;
mod sidebar;
pub mod smart_indent;
mod split_tab_header;
pub(crate) mod ssh;
mod surface_changes;
mod surface_composer;
mod surface_header;
mod surface_plan;
mod surface_text;
mod tabs_git;
mod tabs_panes;
pub(crate) mod task_detail;
mod title;
pub(crate) use title::{window_title, window_title_for};
mod trust_prompt;
pub(crate) mod trust_store;
mod view;
mod view_actions;
mod view_attention;
mod view_build;
mod view_documents;
mod view_events;
mod view_exec_control;
mod view_exec_launch;
mod view_explorer;
mod view_language;
mod view_overlays;
mod view_persistence;
mod view_principal;
mod view_projects;
mod view_render;
mod view_session;
mod view_shaping;
mod view_state;
mod view_tabs;
mod view_tool_exec;
mod viewer_bar;
mod work_canvas;
mod work_item_surface;
mod work_journey;
mod worker_tools;
mod workspace_tab;
mod workspace_tab_strip;
pub(crate) mod workspace_setup;
pub(crate) mod worktree_status;
pub(crate) mod workspace_activity;

pub use sidebar::SidebarFocus;

#[cfg(not(target_family = "wasm"))]
pub(crate) use action::PromptMenu;
pub use action::{
    DropSide, LanguageNavigationKind, PaneDropZone, ProjectFolderIntent, PromptAgent,
    ShapePlanTaskMove, ShapeRecordEditTarget, WorkItemView, WorkspaceAction,
};
pub(crate) use action_projects::{
    agent_command_config, default_agent_command_config, detect_prompt_agents,
    prompt_agent_interactive_command, prompt_agent_launch_command,
};
#[cfg(not(target_family = "wasm"))]
pub(crate) use agent_definition::stage_custom_recovery;
pub(crate) use agent_definition::{
    all_definitions, builtin_image_for_cli, builtin_image_for_id, custom_id, definition_by_cli,
    definition_by_id, detect_custom_agents, is_custom_id, AgentDefinition, AgentImage,
    CustomAgentDefinition,
};
pub use chrome_inset::sync_native_window_chrome;
pub use input_editor::*;
pub(crate) use pane_layout::{
    PaneId, PaneLayout, SplitDirection, SplitId, SplitSide, EDITOR_PANE_ID, MIN_PANE_HEIGHT,
    MIN_PANE_WIDTH,
};
#[cfg(not(target_family = "wasm"))]
pub(crate) use project_links::{LinkKind, PromptLink};
pub(crate) use project_prompt::{
    agent_chip, agent_image, labelled_agent_chip, render_agent_menu, render_branch_menu, AgentMenuProps,
    BranchMenuProps,
};
#[cfg(not(target_family = "wasm"))]
pub(crate) use project_prompt::{
    agent_image as prompt_agent_image, menu_caption as prompt_menu_caption,
    menu_list as prompt_menu_list, prompt_box, render as render_demo_project_prompt,
    render_prompt_menu, ProjectPromptProps,
};
pub(crate) use project_prompt::{
    agent_search_field as picker_search, menu_caption as picker_caption, menu_card as picker_card,
    menu_list as picker_list, menu_row as picker_row,
};
pub(crate) use project_prompt::{
    agent_target_chip, agent_target_image, default_agent_target, remember_agent_target,
    render_agent_target_menu, visible_agent_targets, AgentTarget, AgentTargetMenuProps,
};
pub use title::DEFAULT_SESSION_TITLE;
pub use view::{PaneTab, WorkspaceView};
pub(crate) use workspace_tab::WorkspaceTab;

pub fn register_bindings(ctx: &mut warpui::AppContext) {
    bindings::register(ctx);
}

/// Re-apply shortcut overrides to the live keymap (rebind/reset without a
/// restart). Safe to call on every `AppSettingsChanged`.
pub fn sync_shortcut_bindings(ctx: &mut warpui::AppContext) {
    bindings::apply_shortcut_overrides(ctx);
}

#[cfg(not(target_family = "wasm"))]
mod pull_requests_page;
