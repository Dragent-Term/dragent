//! Workflow settings: general behavior, editor, agent commands, version
//! control, and file search.
//!
//! The Editor page carries real controls today (vim mode is live state);
//! the rest render the standard header plus an intentional planned-state
//! until their backends land.

use warpui::elements::{
    ChildView, ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, ScrollbarWidth, Shrinkable, Text, Wrap,
};

use crate::console::editor_settings::{
    AUTO_SAVE_DELAY_MAX_MS, AUTO_SAVE_DELAY_MIN_MS, TAB_SIZE_MAX, TAB_SIZE_MIN,
};
use crate::shared_input::{shared_input, SharedInputOpts, SharedInputWidth};
use crate::theme::Theme;
use crate::workspace::PromptAgent;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, danger_button, group_label, page_header, primary_button, secondary_button,
    row_control, row_field, segmented_control, select_control, setting_row, slider_control,
    status_badge, toggle_row, SegmentOption, SelectItem,
};

pub fn render_general(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::settings::git_settings::setup_guide_steps;
    use crate::setup::UiSettings;
    let ui = view.ui_font;
    let id = view.view_id;
    let general = &view.app_settings.general;
    let ui_settings = UiSettings::load();

    // ── Workspace ───────────────────────────────────────────────────────────
    let workspace_dir_control: Box<dyn warpui::elements::Element> = match &view.workspace_dir_editor {
        Some(editor) => row_field(theme, editor),
        None => secondary_button(
            theme, ui, id, view.hover("general_workspace_dir_edit"),
            "Set…", SettingsAction::TogglePicker("general_workspace_dir".to_string()),
        ),
    };
    let workspace_card = card_rows(
        theme,
        vec![
            setting_row(
                theme, ui, "Workspace directory",
                Some("Base folder for new worktrees. Empty = default (~/.dragent/worktrees)."),
                workspace_dir_control,
            ),
            toggle_row(
                theme, ui, id, view.hover("general_nest"),
                "Nest workspaces",
                Some("Group worktrees under ~/.dragent/worktrees/{project}/. Off puts them flat."),
                general.nest_workspaces,
                SettingsAction::ToggleNestWorkspaces,
            ),
            toggle_row(
                theme, ui, id, view.hover("general_refresh_base"),
                "Refresh base ref on create",
                Some("Fetch the base branch before creating a worktree so it starts current."),
                general.refresh_base_ref_on_create,
                SettingsAction::ToggleRefreshBaseRef,
            ),
            setting_row(
                theme, ui, "Reset workspace folder",
                Some("Clear the override and use the default location."),
                secondary_button(
                    theme, ui, id, view.hover("general_workspace_clear"),
                    "Use default", SettingsAction::ClearWorkspaceDir,
                ),
            ),
        ],
    );

    // ── Confirmations ───────────────────────────────────────────────────────
    let confirm_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme, ui, id, view.hover("general_confirm_close"),
                "Confirm closing terminals with processes",
                Some("Ask before closing a terminal tab that still runs a process."),
                general.confirm_close_terminal_with_process,
                SettingsAction::ToggleConfirmCloseTerminal,
            ),
            toggle_row(
                theme, ui, id, view.hover("general_confirm_idle"),
                "Confirm closing idle tabs",
                Some("Ask before closing a tab with no running process."),
                view.app_settings.agent.confirm_close_idle_tab,
                SettingsAction::ToggleConfirmCloseIdle,
            ),
            toggle_row(
                theme, ui, id, view.hover("general_confirm_delete"),
                "Confirm deleting worktrees",
                Some("Ask before deleting a worktree checkout from disk."),
                general.confirm_delete_worktree,
                SettingsAction::ToggleConfirmDeleteWorktree,
            ),
        ],
    );

    // ── Startup & links ─────────────────────────────────────────────────────
    let startup_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme, ui, id, view.hover("general_open_links"),
                "Open links in Dragent",
                Some("Web links open in Dragent's browser, where you can still send a page to your own browser. Off opens them in your system browser."),
                general.open_links_in_app,
                SettingsAction::ToggleOpenLinksInApp,
            ),
            toggle_row(
                theme, ui, id, view.hover("general_restore"),
                "Restore sessions on startup",
                Some("Reopen the previous session, tabs and workspaces on launch."),
                general.restore_sessions_on_startup,
                SettingsAction::ToggleRestoreSessions,
            ),
        ],
    );

    // ── Setup guide (onboarding checklist) ──────────────────────────────────
    let steps = setup_guide_steps();
    let mut guide_rows: Vec<Box<dyn warpui::elements::Element>> = Vec::new();
    for (step_id, title, desc) in &steps {
        let done = general.setup_guide_completed
            || general.setup_item_dismissed(step_id)
            || (*step_id == "account" && ui_settings.onboarding_completed);
        let toggle = if done {
            secondary_button(
                theme, ui, id, view.hover(&format!("guide_undismiss_{step_id}")),
                "Reopen", SettingsAction::UndismissSetupGuideItem(step_id.to_string()),
            )
        } else {
            secondary_button(
                theme, ui, id, view.hover(&format!("guide_dismiss_{step_id}")),
                "Dismiss", SettingsAction::DismissSetupGuideItem(step_id.to_string()),
            )
        };
        guide_rows.push(setting_row(
            theme, ui,
            &format!("{} {}", if done { "✓" } else { "○" }, title),
            Some(desc),
            toggle,
        ));
    }
    guide_rows.push(setting_row(
        theme, ui, "Setup guide",
        Some(if general.setup_guide_completed { "All steps done." } else { "Mark every step done, or reset to start over." }),
        if general.setup_guide_completed {
            secondary_button(theme, ui, id, view.hover("guide_reset"), "Reset", SettingsAction::ResetSetupGuide)
        } else {
            primary_button(theme, ui, id, view.hover("guide_complete"), "Mark complete", SettingsAction::CompleteSetupGuide)
        },
    ));
    let guide_card = card_rows(theme, guide_rows);

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(theme, ui, "General", "Startup behavior, session restore, confirmations, and link handling."))
        .with_child(group_label(theme, ui, "Workspace"))
        .with_child(workspace_card)
        .with_child(group_label(theme, ui, "Confirmations"))
        .with_child(confirm_card)
        .with_child(group_label(theme, ui, "Startup & links"))
        .with_child(startup_card)
        .with_child(group_label(theme, ui, "Session sharing"))
        .with_child(card_rows(theme, vec![toggle_row(theme,ui,id,view.hover("general-session-sharing"),"Allow session sharing",Some("Show sharing controls and allow sessions to be shared."),view.app_settings.sharing.artifact_sharing_enabled,SettingsAction::ToggleArtifactSharing)]))
        .with_child(group_label(theme, ui, "Setup guide"))
        .with_child(guide_card)
        .finish()
}

pub fn render_editor(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let id = view.view_id;
    let prefs = &view.editor_settings.editor;

    let editing_card = card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "Vim mode",
            Some("Modal editing for the command line and editors."),
            segmented_control(
                theme,
                ui_font,
                id,
                vec![
                    SegmentOption {
                        label: "Default".to_string(),
                        selected: !view.editor_settings.vim_mode_enabled,
                        hover: view.hover("editor_vim_default"),
                        action: SettingsAction::SetVimMode(false),
                    },
                    SegmentOption {
                        label: "Vim".to_string(),
                        selected: view.editor_settings.vim_mode_enabled,
                        hover: view.hover("editor_vim_mode"),
                        action: SettingsAction::SetVimMode(true),
                    },
                ],
            ),
        )],
    );

    let typography_card = card_rows(
        theme,
        vec![
            setting_row(
                theme,
                ui_font,
                "Tab size",
                Some("Tab stop width in spaces."),
                slider_control(
                    theme,
                    ui_font,
                    id,
                    view.slider("editor_tab_size"),
                    prefs.tab_size.clamp(TAB_SIZE_MIN, TAB_SIZE_MAX) as f32,
                    (TAB_SIZE_MIN as f32)..(TAB_SIZE_MAX as f32),
                    1.0,
                    format!(
                        "{} spaces",
                        prefs.tab_size.clamp(TAB_SIZE_MIN, TAB_SIZE_MAX)
                    ),
                    170.0,
                    |v| SettingsAction::SetTabSize(v.round() as u32),
                ),
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_insert_spaces"),
                "Insert spaces",
                Some("Tab key inserts spaces. Off inserts a tab character."),
                prefs.insert_spaces,
                SettingsAction::ToggleInsertSpaces,
            ),
        ],
    );

    let display_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_line_numbers"),
                "Line numbers",
                Some("Gutter with line numbers on source editors."),
                prefs.line_numbers,
                SettingsAction::ToggleLineNumbers,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_whitespace"),
                "Show whitespace",
                Some("Render spaces as visible dots."),
                prefs.show_whitespace,
                SettingsAction::ToggleShowWhitespace,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_indent_guides"),
                "Indent guides",
                Some("Indent rails on source editors."),
                prefs.indent_guides,
                SettingsAction::ToggleIndentGuides,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_minimap"),
                "Minimap",
                Some("Miniature overview rail on source editors."),
                prefs.minimap_enabled,
                SettingsAction::ToggleMinimap,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_word_wrap"),
                "Word wrap",
                Some("Soft-wrap long lines in markdown and wrapped surfaces."),
                prefs.word_wrap,
                SettingsAction::ToggleWordWrap,
            ),
        ],
    );

    // ── Autosave ────────────────────────────────────────────────────────────
    let autosave_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_autosave"),
                "Autosave",
                Some("Save dirty documents automatically after edits."),
                prefs.auto_save,
                SettingsAction::ToggleAutoSave,
            ),
            setting_row(
                theme,
                ui_font,
                "Autosave delay",
                Some("Debounce before an automatic save (100–10000 ms)."),
                slider_control(
                    theme,
                    ui_font,
                    id,
                    view.slider("editor_autosave_delay"),
                    prefs
                        .auto_save_delay_ms
                        .clamp(AUTO_SAVE_DELAY_MIN_MS, AUTO_SAVE_DELAY_MAX_MS)
                        as f32,
                    (AUTO_SAVE_DELAY_MIN_MS as f32)..(AUTO_SAVE_DELAY_MAX_MS as f32),
                    100.0,
                    format!(
                        "{}ms",
                        prefs
                            .auto_save_delay_ms
                            .clamp(AUTO_SAVE_DELAY_MIN_MS, AUTO_SAVE_DELAY_MAX_MS)
                    ),
                    170.0,
                    |v| SettingsAction::SetAutoSaveDelay(v.round() as u32),
                ),
            ),
        ],
    );

    // ── Editor font (None = follow terminal code font) ─────────────────────
    let current_editor_font = prefs.font_family.as_deref().unwrap_or("Follow terminal");
    let mut editor_font_items = vec![SelectItem {
        label: "Follow terminal (default)".to_string(),
        selected: prefs.font_family.is_none(),
        enabled: true,
        hover: view.hover("editor_font_default"),
        action: SettingsAction::SelectEditorFontFamily(None),
    }];
    for family in &view.available_mono_fonts {
        editor_font_items.push(SelectItem {
            label: family.clone(),
            selected: prefs.font_family.as_deref() == Some(family.as_str()),
            enabled: true,
            hover: view.hover(&format!("editor_font_{family}")),
            action: SettingsAction::SelectEditorFontFamily(Some(family.clone())),
        });
    }
    let editor_font_select = select_control(
        theme,
        ui_font,
        id,
        view.hover("editor_font_btn"),
        current_editor_font,
        200.0,
        view.picker_open("editor_font"),
        SettingsAction::TogglePicker("editor_font".to_string()),
        editor_font_items,
    );
    let font_card = card_rows(
        theme,
        vec![setting_row(
            theme,
            ui_font,
            "Editor font",
            Some("Source editor family. Default follows the terminal code font."),
            editor_font_select,
        )],
    );

    // ── Markdown & tabs ─────────────────────────────────────────────────────
    let markdown_card = card_rows(
        theme,
        vec![
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_spellcheck"),
                "Spellcheck",
                Some("Squiggles in markdown preview and rich surfaces."),
                prefs.spellcheck_enabled,
                SettingsAction::ToggleSpellcheck,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_review_tools"),
                "Review tools",
                Some("Review affordances on markdown surfaces."),
                prefs.markdown_review_tools,
                SettingsAction::ToggleMarkdownReview,
            ),
            toggle_row(
                theme,
                ui_font,
                id,
                view.hover("editor_preview_tabs"),
                "Preview tabs",
                Some("Single-click opens a reusable preview tab."),
                prefs.preview_tabs,
                SettingsAction::TogglePreviewTabs,
            ),
        ],
    );

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(
            theme,
            ui_font,
            "Editor",
            "Editing behavior, typography, indentation, and display.",
        ))
        .with_child(group_label(theme, ui_font, "Editing"))
        .with_child(editing_card)
        .with_child(group_label(theme, ui_font, "Autosave"))
        .with_child(autosave_card)
        .with_child(group_label(theme, ui_font, "Typography"))
        .with_child(typography_card)
        .with_child(group_label(theme, ui_font, "Font"))
        .with_child(font_card)
        .with_child(group_label(theme, ui_font, "Display"))
        .with_child(display_card)
        .with_child(group_label(theme, ui_font, "Markdown & tabs"))
        .with_child(markdown_card)
        .finish()
}

fn agent_image(agent: PromptAgent, theme: &Theme) -> Box<dyn Element> {
    use crate::icons::{render_icon, AppIcon};
    use warpui::elements::Image;
    use warpui_core::assets::asset_cache::AssetSource;
    use warpui_core::image_cache::CacheOption;

    let Some(path) = agent.image_path(theme.is_dark) else {
        return render_icon(AppIcon::Agent, theme.sub_text(), 16.0);
    };
    ConstrainedBox::new(
        Image::new(AssetSource::Bundled { path }, CacheOption::BySize)
            .contain()
            .finish(),
    )
    .with_width(16.0)
    .with_height(16.0)
    .finish()
}

pub fn render_agent_commands(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::icons::{render_icon, AppIcon};
    use warpui::elements::{Container, Expanded, Image, MainAxisAlignment};
    use warpui_core::assets::asset_cache::AssetSource;
    use warpui_core::elements::Hoverable;
    use warpui_core::image_cache::CacheOption;
    use warpui_core::scene::{Border, CornerRadius, Radius};

    let ui = view.ui_font;
    let id = view.view_id;
    let selected = view.command_agent;
    let config = &view.agent_configs[selected.command_key()];
    let text = |value: &str, size: f32, color| {
        Text::new(value.to_string(), ui, crate::theme::text_size(size))
            .with_color(color)
            .finish()
    };
    let query = view.command_agent_search.trim().to_lowercase();
    let visible: Vec<PromptAgent> = PromptAgent::ALL
        .into_iter()
        .filter(|agent| {
            query.is_empty()
                || agent.label().to_lowercase().contains(&query)
                || agent
                    .cli()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&query)
        })
        .collect();
    let mut agents = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(3.0);
    if visible.is_empty() {
        agents = agents.with_child(text("No matching agents", 12.0, theme.sub_text()));
    }
    for agent in visible {
        let selected = agent == view.command_agent;
        agents = agents.with_child(Box::new(
            Hoverable::new(
                view.hover(&format!("command-agent-{}", agent.command_key())),
                move |state| {
                    Container::new(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_spacing(8.0)
                            .with_child(agent_image(agent, theme))
                            .with_child(
                                Expanded::new(
                                    1.0,
                                    Flex::column()
                                        .with_main_axis_size(MainAxisSize::Min)
                                        .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                        .with_spacing(3.0)
                                        .with_child(
                                            Text::new(
                                                agent.label().to_string(),
                                                ui,
                                                crate::theme::text_size(13.0),
                                            )
                                            .with_color(if selected {
                                                theme.accent
                                            } else {
                                                theme.main_text()
                                            })
                                            .finish(),
                                        )
                                        .with_child(
                                            Text::new(
                                                agent.cli().unwrap_or("Built-in agent").to_string(),
                                                ui,
                                                crate::theme::text_size(11.0),
                                            )
                                            .with_color(theme.sub_text())
                                            .finish(),
                                        )
                                        .finish(),
                                )
                                .finish(),
                            )
                            .finish(),
                    )
                    .with_padding_left(12.0)
                    .with_padding_right(12.0)
                    .with_padding_top(9.0)
                    .with_padding_bottom(9.0)
                    .with_background_color(if selected {
                        theme.accent_bg()
                    } else if state.is_hovered() {
                        theme.surface_2()
                    } else {
                        theme.sidebar_background()
                    })
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                    .finish()
                },
            )
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::SelectCommandAgent(agent));
            }),
        ));
    }
    let trailing: Option<Box<dyn Element>> = if !view.command_agent_search.is_empty() {
        Some(Box::new(
            Hoverable::new(view.hover("command_agent_search_clear"), move |state| {
                let icon_color = if state.is_hovered() {
                    theme.main_text()
                } else {
                    theme.disabled_text()
                };
                Container::new(render_icon(AppIcon::XClose, icon_color, 11.0))
                    .with_padding_left(4.0)
                    .with_padding_right(2.0)
                    .finish()
            })
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::ClearCommandAgentSearch);
            }),
        ))
    } else {
        None
    };
    // Settings-sidebar style header: search icon plus the editor in one
    // centered box. `shared_input` keeps the caret and the text on a single
    // vertical midpoint, so the cursor sits directly in the middle.
    let search = shared_input(
        theme,
        &view.command_agent_search_editor,
        SharedInputOpts {
            leading_icon: Some(crate::shared_input::SharedInputIcon {
                icon: AppIcon::Search,
                size: 12.0,
            }),
            trailing,
            width: SharedInputWidth::Fill,
            ..Default::default()
        },
    );
    // The list bleeds to the right edge so its scrollbar sits directly on
    // the sidebar border, like the main settings sidebar. Title and search
    // keep their own right padding to stay optically aligned.
    let pad_right =
        |child: Box<dyn Element>| Container::new(child).with_padding_right(16.0).finish();
    let sidebar = Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(16.0)
            .with_child(pad_right(text("Agents", 16.0, theme.main_text())))
            .with_child(pad_right(search))
            .with_child(
                ConstrainedBox::new(
                    ClippedScrollable::vertical(
                        view.command_agents_scroll.clone(),
                        agents.finish(),
                        ScrollbarWidth::Auto,
                        theme.disabled_text().into(),
                        theme.sub_text().into(),
                        theme.sidebar_background().into(),
                    )
                    .finish(),
                )
                .with_height((view.pane_height() - 84.0).max(100.0))
                .finish(),
            )
            .finish(),
    )
    .with_padding_left(16.0)
    .with_padding_top(16.0)
    .with_padding_bottom(16.0)
    .with_background_color(theme.sidebar_background())
    .with_border(Border::right(1.0).with_border_color(theme.border()))
    .finish();
    let field =
        |label: &str, hint: &str, editor: &warpui::ViewHandle<crate::workspace::InputEditor>| {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(7.0)
                .with_child(text(label, 13.0, theme.main_text()))
                .with_child(shared_input(
                    theme,
                    editor,
                    SharedInputOpts {
                        height: Some(38.0),
                        ..Default::default()
                    },
                ))
                .with_child(text(hint, 11.0, theme.sub_text()))
                .finish()
        };
    let divider = || {
        Container::new(
            ConstrainedBox::new(warpui::elements::Empty::new().finish())
                .with_height(1.0)
                .finish(),
        )
        .with_background_color(theme.border())
        .finish()
    };
    let header = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(7.0)
        .with_child(text("AGENT CONFIGURATION", 10.0, theme.sub_text()))
        .with_child(text(selected.label(), 22.0, theme.main_text()))
        .with_child(text(
            "Customize how this agent opens in your workspace.",
            12.0,
            theme.sub_text(),
        ))
        .finish();
    let mut form = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(header)
        .with_child(divider());
    if selected == PromptAgent::Dragent {
        form = form.with_child(text(
            "Dragent runs in the app and does not use a terminal launch command.",
            12.0,
            theme.sub_text(),
        ));
    } else {
        form = form
            .with_child(field(
                "Launch command",
                "Used when opening the agent without an initial prompt.",
                &view.agent_base_editor,
            ))
            .with_child(field(
                "Command with prompt",
                "Receives your initial prompt. Leave empty to use the launch command.",
                &view.agent_command_editor,
            ))
            .with_child(field(
                "Prompt suffix · optional",
                "Flags or a follow-up shell command. Applied to prompt launches only.",
                &view.agent_suffix_editor,
            ))
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_spacing(16.0)
                    .with_child(
                        Shrinkable::new(
                            1.0,
                            Flex::column()
                                .with_main_axis_size(MainAxisSize::Min)
                                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                                .with_spacing(4.0)
                                .with_child(text("Prompt delivery", 13.0, theme.main_text()))
                                .with_child(text(
                                    "How the agent receives your prompt.",
                                    11.0,
                                    theme.sub_text(),
                                ))
                                .finish(),
                        )
                        .finish(),
                    )
                    .with_child(segmented_control(
                        theme,
                        ui,
                        id,
                        vec![
                            SegmentOption {
                                label: "Argument".into(),
                                selected: !config.prompt_stdin,
                                hover: view.hover("agent-argv"),
                                action: SettingsAction::SetAgentPromptStdin(false),
                            },
                            SegmentOption {
                                label: "Standard input".into(),
                                selected: config.prompt_stdin,
                                hover: view.hover("agent-stdin"),
                                action: SettingsAction::SetAgentPromptStdin(true),
                            },
                        ],
                    ))
                    .finish(),
            )
            .with_child(divider())
            .with_child(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_spacing(12.0)
                    .with_child(secondary_button(
                        theme,
                        ui,
                        id,
                        view.hover("agent-reset"),
                        "Reset to defaults",
                        SettingsAction::ResetAgentCommand,
                    ))
                    .with_child(primary_button(
                        theme,
                        ui,
                        id,
                        view.hover("agent-save"),
                        "Save changes",
                        SettingsAction::SaveAgentCommand,
                    ))
                    .finish(),
            );
    }
    if let Some(status) = &view.agent_command_status {
        form = form.with_child(text(status, 11.0, theme.sub_text()));
    }
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(ConstrainedBox::new(sidebar).with_width(228.0).finish())
        .with_child(
            Expanded::new(
                1.0,
                Container::new(
                    ConstrainedBox::new(form.finish())
                        .with_max_width(640.0)
                        .finish(),
                )
                .with_padding_left(32.0)
                .with_padding_right(32.0)
                .with_padding_top(24.0)
                .with_padding_bottom(24.0)
                .finish(),
            )
            .finish(),
        )
        .finish()
}

pub fn render_version_control(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::settings::{BranchPrefix, DiffView, JiraCasing, ScGroupOrder, ScViewMode};
    let ui = view.ui_font;
    let id = view.view_id;
    let git = &view.app_settings.git;

    // ── Branches ────────────────────────────────────────────────────────────
    let prefix_control: Box<dyn warpui::elements::Element> = if git.branch_prefix == BranchPrefix::Custom {
        match &view.git_prefix_custom_editor {
            Some(editor) => row_field(theme, editor),
            None => secondary_button(theme, ui, id, view.hover("git_prefix_edit"), "Set…",
                SettingsAction::TogglePicker("git_prefix_custom".to_string())),
        }
    } else {
        Text::new(git.prefix_string(), ui, crate::theme::text_size(12.5))
            .with_color(theme.sub_text()).finish()
    };
    let branch_card = card_rows(theme, vec![
        toggle_row(theme, ui, id, view.hover("git_auto_rename"),
            "Auto-rename branch from work",
            Some("Derive the worktree branch name from the task title."),
            git.auto_rename_branch, SettingsAction::ToggleAutoRenameBranch),
        setting_row(theme, ui, "Branch prefix",
            Some("Prefix for generated branches: your git username, a custom word, or none."),
            segmented_control(theme, ui, id, vec![
                SegmentOption { label: "Git username".into(), selected: git.branch_prefix == BranchPrefix::GitUsername, hover: view.hover("git_prefix_user"), action: SettingsAction::SelectBranchPrefix(BranchPrefix::GitUsername) },
                SegmentOption { label: "Custom".into(), selected: git.branch_prefix == BranchPrefix::Custom, hover: view.hover("git_prefix_custom"), action: SettingsAction::SelectBranchPrefix(BranchPrefix::Custom) },
                SegmentOption { label: "None".into(), selected: git.branch_prefix == BranchPrefix::None, hover: view.hover("git_prefix_none"), action: SettingsAction::SelectBranchPrefix(BranchPrefix::None) },
            ])),
        setting_row(theme, ui, "Custom prefix",
            Some("Used when Branch prefix is Custom (e.g. team → team/my-branch)."),
            prefix_control),
        setting_row(theme, ui, "Jira issue key casing",
            Some("How issue keys render in branch names and titles."),
            segmented_control(theme, ui, id, vec![
                SegmentOption { label: "UPPER".into(), selected: git.jira_casing == JiraCasing::Upper, hover: view.hover("git_jira_upper"), action: SettingsAction::SelectJiraCasing(JiraCasing::Upper) },
                SegmentOption { label: "lower".into(), selected: git.jira_casing == JiraCasing::Lower, hover: view.hover("git_jira_lower"), action: SettingsAction::SelectJiraCasing(JiraCasing::Lower) },
                SegmentOption { label: "As-is".into(), selected: git.jira_casing == JiraCasing::Preserve, hover: view.hover("git_jira_keep"), action: SettingsAction::SelectJiraCasing(JiraCasing::Preserve) },
            ])),
    ]);

    // ── Diffs ───────────────────────────────────────────────────────────────
    let diff_card = card_rows(theme, vec![
        setting_row(theme, ui, "Diff layout",
            Some("Default diff rendering: unified inline or side-by-side."),
            segmented_control(theme, ui, id, vec![
                SegmentOption { label: "Inline".into(), selected: git.diff_view == DiffView::Inline, hover: view.hover("git_diff_inline"), action: SettingsAction::SelectDiffView(DiffView::Inline) },
                SegmentOption { label: "Side-by-side".into(), selected: git.diff_view == DiffView::SideBySide, hover: view.hover("git_diff_side"), action: SettingsAction::SelectDiffView(DiffView::SideBySide) },
            ])),
        toggle_row(theme, ui, id, view.hover("git_diff_wrap"), "Word wrap diffs",
            Some("Soft-wrap long diff lines instead of horizontal scrolling."),
            git.diff_word_wrap, SettingsAction::ToggleDiffWordWrap),
        toggle_row(theme, ui, id, view.hover("git_diff_ws"), "Show whitespace in diffs",
            Some("Render spaces/tabs visibly inside diff hunks."),
            git.diff_show_whitespace, SettingsAction::ToggleDiffShowWhitespace),
        toggle_row(theme, ui, id, view.hover("git_diff_collapse"), "Collapse unchanged regions",
            Some("Fold large unchanged gaps between hunks."),
            git.diff_collapse_unchanged, SettingsAction::ToggleDiffCollapseUnchanged),
    ]);

    // ── Source-control view ─────────────────────────────────────────────────
    let group_items = [ScGroupOrder::Status, ScGroupOrder::Path, ScGroupOrder::FileType].into_iter().map(|order| {
        let label = order.label().to_string();
        let hover_key = format!("git_group_{label}");
        let selected = git.sc_group_order == order;
        crate::settings::widgets::SelectItem {
            label, selected, enabled: true,
            hover: view.hover(&hover_key),
            action: SettingsAction::SelectScGroupOrder(order),
        }
    }).collect::<Vec<_>>();
    let sc_card = card_rows(theme, vec![
        setting_row(theme, ui, "Source-control layout",
            Some("How changed files list in the Source Control panel."),
            segmented_control(theme, ui, id, vec![
                SegmentOption { label: "List".into(), selected: git.sc_view_mode == ScViewMode::List, hover: view.hover("git_sc_list"), action: SettingsAction::SelectScViewMode(ScViewMode::List) },
                SegmentOption { label: "Tree".into(), selected: git.sc_view_mode == ScViewMode::Tree, hover: view.hover("git_sc_tree"), action: SettingsAction::SelectScViewMode(ScViewMode::Tree) },
            ])),
        setting_row(theme, ui, "Group changes by",
            Some("Ordering of groups in the Source Control panel."),
            select_control(theme, ui, id, view.hover("git_group_btn"), git.sc_group_order.label(),
                170.0, view.picker_open("git_group"), SettingsAction::TogglePicker("git_group".to_string()), group_items)),
        toggle_row(theme, ui, id, view.hover("git_compare_upstream"), "Compare against upstream",
            Some("Show ahead/behind against the upstream branch in Source Control."),
            git.sc_compare_upstream, SettingsAction::ToggleScCompareUpstream),
    ]);

    // ── Git behavior ────────────────────────────────────────────────────────
    let commit_agent_control: Box<dyn warpui::elements::Element> = match &view.commit_ai_agent_editor {
        Some(editor) => crate::shared_input::shared_input(
            theme, editor, crate::shared_input::SharedInputOpts::default()),
        None => Text::new("auto".to_string(), ui, crate::theme::text_size(12.5)).with_color(theme.sub_text()).finish(),
    };
    let mut pr_rows: Vec<Box<dyn warpui::elements::Element>> = git.pr_bot_overrides.iter().enumerate().map(|(i, bot)| {
        setting_row(theme, ui, bot, Some("PR author treated as a bot (skips review requests)."),
            danger_button(theme, ui, id, view.hover(&format!("git_pr_remove_{i}")), "Remove",
                SettingsAction::RemovePrBotOverride(i)))
    }).collect();
    let pr_add: Box<dyn warpui::elements::Element> = match &view.pr_bot_draft_editor {
        Some(editor) => row_control(Flex::row().with_spacing(8.0)
            .with_child(Shrinkable::new(1.0, crate::shared_input::shared_input(theme, editor,
                crate::shared_input::SharedInputOpts::default())).finish())
            .with_child(secondary_button(theme, ui, id, view.hover("git_pr_add"), "Add",
                SettingsAction::AddPrBotOverride)).finish()),
        None => secondary_button(theme, ui, id, view.hover("git_pr_add"), "Add bot",
            SettingsAction::AddPrBotOverride),
    };
    let mut open_rows: Vec<Box<dyn warpui::elements::Element>> = git.open_in_apps.iter().enumerate().map(|(i, app)| {
        setting_row(theme, ui, app, Some("External app in the Open-in menu."),
            danger_button(theme, ui, id, view.hover(&format!("git_open_remove_{i}")), "Remove",
                SettingsAction::RemoveOpenInApp(i)))
    }).collect();
    let open_add: Box<dyn warpui::elements::Element> = match &view.open_in_draft_editor {
        Some(editor) => row_control(Flex::row().with_spacing(8.0)
            .with_child(Shrinkable::new(1.0, crate::shared_input::shared_input(theme, editor,
                crate::shared_input::SharedInputOpts::default())).finish())
            .with_child(secondary_button(theme, ui, id, view.hover("git_open_add"), "Add",
                SettingsAction::AddOpenInApp)).finish()),
        None => secondary_button(theme, ui, id, view.hover("git_open_add"), "Add app",
            SettingsAction::AddOpenInApp),
    };
    let mut behavior_rows = vec![
        toggle_row(theme, ui, id, view.hover("git_follow_symlinks"), "Follow symlinked directories",
            Some("Follow symlinks when scanning the repo (like dragent_index walk)."),
            git.follow_symlinks, SettingsAction::ToggleFollowSymlinks),
        toggle_row(theme, ui, id, view.hover("git_show_ignored"), "Show git-ignored files",
            Some("List ignored files in Source Control (dimmed). Backed by git ls-files --ignored."),
            git.show_ignored, SettingsAction::ToggleShowIgnored),
        toggle_row(theme, ui, id, view.hover("git_commit_ai"), "Commit-message AI",
            Some("Enable the Generate button for AI commit messages in Source Control."),
            git.commit_ai_enabled, SettingsAction::ToggleCommitAi),
        setting_row(theme, ui, "Commit message model",
            Some("Model that writes commit messages. Empty = the model you picked for the agent."),
            row_control(Flex::row().with_spacing(8.0)
                .with_child(Shrinkable::new(1.0, commit_agent_control).finish())
                .with_child(secondary_button(theme, ui, id, view.hover("git_commit_ai_clear"), "Auto",
                    SettingsAction::ClearCommitAiAgent)).finish())),
    ];
    behavior_rows.push(setting_row(theme, ui, "PR bot authors",
        Some("Authors treated as bots, e.g. renovate[bot], dependabot[bot]."), pr_add));
    behavior_rows.extend(pr_rows);
    behavior_rows.push(setting_row(theme, ui, "Open-in applications",
        Some("External editors in the Open-in menu. First app is the default."),
        open_add));
    behavior_rows.extend(open_rows);
    let behavior_card = card_rows(theme, behavior_rows);

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(20.0)
        .with_child(page_header(theme, ui, "Version Control", "Git defaults: branch prefixes, commit templates, and worktree locations."))
        .with_child(group_label(theme, ui, "Branches"))
        .with_child(branch_card)
        .with_child(group_label(theme, ui, "Diffs"))
        .with_child(diff_card)
        .with_child(group_label(theme, ui, "Source-control view"))
        .with_child(sc_card)
        .with_child(group_label(theme, ui, "Git behavior"))
        .with_child(behavior_card)
        .finish()
}

