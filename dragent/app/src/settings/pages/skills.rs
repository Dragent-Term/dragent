use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, link_button, page_header, secondary_button, setting_row, toggle_row,
};
use crate::agent::skill_discovery::{self, DiscoveredSkill, SkillSource, RESERVED_SLASH_COMMANDS};
use crate::icons::AppIcon;
use crate::machine_ui as mui;
use crate::settings::{SettingsAction, PLUGIN_CONSENT_VERSION};
use crate::theme::{text_size, Theme};
use std::collections::HashMap;
use warpui_core::scene::{CornerRadius, Radius};
use warpui_core::text_layout::ClipConfig;

pub(crate) const SKILL_MENU_TAG: &str = "settings-skill-menu";

/// Rows listed per resource section before the rest are summarised.
const RESOURCE_LIMIT: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillRowAction {
    Inspect,
    EditFile,
    Remove,
}

/// What a skill row shows and offers, separate from how it is drawn.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SkillRowModel {
    pub provenance: &'static str,
    pub invocation: &'static str,
    /// Why the skill is not usable (or not usable as written), if so.
    pub problems: Vec<String>,
    /// Neutral facts about which copy is in use.
    pub notes: Vec<String>,
    /// Other agents read this location, so they need a restart to see edits.
    pub read_by_other_agents: bool,
    pub actions: Vec<SkillRowAction>,
}

pub(crate) fn provenance_label(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::BuiltIn => "Built into Dragent",
        SkillSource::Global => "Shared (~/.agents/skills)",
        SkillSource::ProjectLocal { .. } => "This project",
    }
}

fn copy_label(source: &SkillSource) -> &'static str {
    match source {
        SkillSource::BuiltIn => "built-in",
        SkillSource::Global => "shared",
        SkillSource::ProjectLocal { .. } => "project",
    }
}

pub(crate) const RESTART_NOTE: &str =
    "Other agents read this folder when they start. Restart a running agent to see changes.";

/// `winner` is the copy Dragent actually uses for this skill's name, if any.
pub(crate) fn skill_row_model(skill: &DiscoveredSkill, winner: Option<&DiscoveredSkill>) -> SkillRowModel {
    let built_in = matches!(skill.source, SkillSource::BuiltIn);
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    match winner {
        Some(w) if w.source_path != skill.source_path => notes.push(format!(
            "Not in use here: the {} copy of this skill takes priority.",
            copy_label(&w.source)
        )),
        None if matches!(skill.source, SkillSource::ProjectLocal { .. }) => {
            problems.push("Not in use until this project is trusted.".to_string())
        }
        None => problems.push("Not in use.".to_string()),
        _ => {}
    }
    let has_file = !built_in && skill.source_path.is_file();
    if !built_in && !has_file {
        problems.push("SKILL.md is missing.".to_string());
    }
    if RESERVED_SLASH_COMMANDS.contains(&skill.name.as_str()) {
        problems.push(format!(
            "/{} is a Dragent command; type {} instead.",
            skill.name,
            skill.slash_name()
        ));
    }
    problems.extend(skill.load_warnings.iter().cloned());
    let mut actions = vec![SkillRowAction::Inspect];
    if has_file {
        actions.push(SkillRowAction::EditFile);
    }
    if has_file && cfg!(not(windows)) {
        actions.push(SkillRowAction::Remove);
    }
    SkillRowModel {
        provenance: provenance_label(&skill.source),
        invocation: if skill.disable_model_invocation {
            "Manual only"
        } else {
            "Agent can choose"
        },
        problems,
        notes,
        read_by_other_agents: !built_in,
        actions,
    }
}
use warpui::elements::{
    ClippedScrollable, ConstrainedBox, Container, CrossAxisAlignment, Element, Expanded, Flex,
    MainAxisAlignment, MainAxisSize, ParentElement, SavePosition, ScrollbarWidth, Shrinkable, Text,
};

pub fn render_skills(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let builtin = skill_discovery::built_in_skills();
    let shared = skill_discovery::skills_for_scope(None);
    let project = skill_discovery::skills_for_scope(Some(&view.project_root));
    let effective: HashMap<_, _> = skill_discovery::discover_skills(&view.project_root)
        .into_iter()
        .map(|skill| (skill.name.clone(), skill))
        .collect();
    let query = view.skill_search_query.trim().to_lowercase();
    let total = builtin.len() + shared.len() + project.len();
    let mut page = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min).with_spacing(16.0)
        .with_child(page_header(theme, ui, "Skills", "Reusable instructions for Dragent’s agent. Choose a skill yourself, or let the agent pick one that fits."))
        .with_child(Flex::row().with_main_axis_size(MainAxisSize::Max).with_main_axis_alignment(MainAxisAlignment::SpaceBetween).with_spacing(16.0).with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Shrinkable::new(1.0, Text::new(format!("{total} skills · built-in, shared and project"), ui, text_size(12.5)).with_color(theme.sub_text()).finish()).finish())
            .with_child(link_button(theme, ui, view.view_id, view.hover("skill-create"), "+ New", SettingsAction::ShowSkillCreator)).finish())
        .with_child(crate::shared_input::shared_input(theme, &view.skill_search_editor, crate::shared_input::SharedInputOpts {
            width: crate::shared_input::SharedInputWidth::Fill, radius: 8.0, pad_left: 12.0, pad_right: 12.0, pad_top: 8.0, pad_bottom: 8.0, ..Default::default()
        }));
    if let Some(message) = &view.status_message {
        page = page.with_child(
            Text::new(message.clone(), ui, text_size(12.0))
                .with_color(theme.sub_text())
                .finish(),
        );
    }
    #[cfg(not(target_family = "wasm"))]
    if query.is_empty() {
        page = page.with_child(other_agents_section(view, theme));
    }
    let mut shown = 0;
    for (key, title, description, skills) in [
        ("builtin", "Built into Dragent", "Included with the app and read-only. Updated with each Dragent version.", builtin),
        ("shared", "Shared (~/.agents/skills)", "Yours, and read by other agents on this computer too. Restart a running agent to see changes.", shared),
        ("project", "This project", "Stored in this project and used when it is trusted. Project copies win when names match.", project),
    ] {
        let count = skills.len();
        let filtered: Vec<_> = skills.into_iter().filter(|skill| query.is_empty() || format!("{} {}", skill.name, skill.description).to_lowercase().contains(&query)).collect();
        shown += filtered.len();
        if !query.is_empty() && filtered.is_empty() { continue; }
        page = page.with_child(Text::new(format!("{title}  {count}"), ui, text_size(14.0)).with_color(theme.main_text()).finish())
            .with_child(Text::new(description.to_string(), ui, text_size(12.0)).with_color(theme.sub_text()).finish())
            .with_child(skill_group(view, theme, key, filtered, &effective));
    }
    if !query.is_empty() && shown == 0 {
        page = page.with_child(
            Text::new(
                "No skills match your search.".to_string(),
                ui,
                text_size(13.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    }
    let mut issues = skill_discovery::skill_load_issues(None);
    issues.extend(skill_discovery::skill_load_issues(Some(&view.project_root)));
    if !issues.is_empty() {
        page = page
            .with_child(group_label(theme, ui, "Could not load"))
            .with_child(card_rows(
                theme,
                issues
                    .into_iter()
                    .map(|issue| {
                        setting_row(
                            theme,
                            ui,
                            &issue.path.display().to_string(),
                            Some(&issue.message),
                            Text::new("Check SKILL.md", ui, text_size(11.0))
                                .with_color(theme.ui_red())
                                .finish(),
                        )
                    })
                    .collect(),
            ));
    }
    page.with_child(
        Text::new(
            "Skills are instructions only. MCP servers, plugins and connected services are set up on their own pages."
                .to_string(),
            ui,
            text_size(11.5),
        )
        .with_color(theme.disabled_text())
        .finish(),
    )
    .finish()
}

fn skill_group(
    view: &SettingsView,
    theme: &Theme,
    group: &str,
    skills: Vec<DiscoveredSkill>,
    effective: &HashMap<String, DiscoveredSkill>,
) -> Box<dyn Element> {
    let ui = view.ui_font;
    if skills.is_empty() {
        return card_rows(
            theme,
            vec![setting_row(
                theme,
                ui,
                "No skills here yet",
                None,
                Text::new("", ui, text_size(1.0)).finish(),
            )],
        );
    }
    let total = skills.len();
    let all_shown =
        !view.skill_search_query.trim().is_empty() || view.skill_groups_expanded.contains(group);
    let limit = if all_shown { total } else { 8 };
    let rows = skills
        .into_iter()
        .take(limit)
        .map(|skill| {
            let path = skill.source_path.clone();
            let expanded = view.expanded_skill.as_ref() == Some(&path);
            let model = skill_row_model(&skill, effective.get(&skill.name));
            let key = format!("skill-menu:{}", path.display());
            let menu_open = view.open_picker.as_deref() == Some(key.as_str());
            let button = link_button(
                theme,
                ui,
                view.view_id,
                view.hover(&key),
                "···",
                SettingsAction::TogglePicker(key),
            );
            let button = if menu_open {
                SavePosition::new(button, SKILL_MENU_TAG).finish()
            } else {
                button
            };
            let mut labels = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(3.0)
                .with_child(
                    Text::new(skill.name.clone(), ui, text_size(14.0))
                        .with_color(theme.main_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .with_child(
                    Text::new(skill.description.clone(), ui, text_size(12.5))
                        .with_color(theme.sub_text())
                        .soft_wrap(false)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        format!("{} · {}", model.provenance, model.invocation),
                        ui,
                        text_size(11.5),
                    )
                    .with_color(theme.disabled_text())
                    .soft_wrap(false)
                    .with_clip(ClipConfig::ellipsis())
                    .finish(),
                );
            for note in &model.notes {
                labels = labels.with_child(
                    Text::new(note.clone(), ui, text_size(11.5))
                        .with_color(theme.sub_text())
                        .finish(),
                );
            }
            for problem in &model.problems {
                labels = labels.with_child(
                    Text::new(problem.clone(), ui, text_size(11.5))
                        .with_color(theme.ui_red())
                        .finish(),
                );
            }
            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(14.0)
                .with_child(mui::icon_tile(theme, AppIcon::Bolt, 34.0, None))
                .with_child(Expanded::new(1.0, labels.finish()).finish())
                .with_child(button)
                .finish();
            let mut body = Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(row);
            if expanded {
                let instructions = skill
                    .load_body()
                    .unwrap_or_else(|e| format!("Could not read this skill: {e}"));
                let mut details = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(10.0)
                    .with_child(
                        Text::new(
                            format!("Type {} in an agent message to use it.", skill.slash_name()),
                            ui,
                            text_size(12.0),
                        )
                        .with_color(theme.accent)
                        .finish(),
                    )
                    .with_child(
                        Text::new(
                            if matches!(skill.source, SkillSource::BuiltIn) {
                                "Included with Dragent; read-only.".to_string()
                            } else {
                                path.display().to_string()
                            },
                            ui,
                            text_size(11.5),
                        )
                        .with_color(theme.sub_text())
                        .finish(),
                    );
                if model.read_by_other_agents {
                    details = details.with_child(
                        Text::new(RESTART_NOTE.to_string(), ui, text_size(11.5))
                            .with_color(theme.sub_text())
                            .finish(),
                    );
                }
                details = details.with_child(
                    Text::new(instructions, ui, text_size(12.5))
                        .with_color(theme.main_text())
                        .finish(),
                );
                let resources = skill_discovery::skill_resources(skill.dir(), RESOURCE_LIMIT + 1);
                if !resources.is_empty() {
                    let openable = !matches!(skill.source, SkillSource::BuiltIn);
                    details = details.with_child(
                        Text::new("Supporting files", ui, text_size(12.0))
                            .with_color(theme.sub_text())
                            .finish(),
                    );
                    for relative in resources.iter().take(RESOURCE_LIMIT) {
                        let file = skill.dir().join(relative);
                        details = details.with_child(if openable {
                            link_button(
                                theme,
                                ui,
                                view.view_id,
                                view.hover(&format!("skill-resource:{}", file.display())),
                                relative,
                                SettingsAction::OpenSkill(file),
                            )
                        } else {
                            Text::new(relative.clone(), ui, text_size(12.0))
                                .with_color(theme.main_text())
                                .finish()
                        });
                    }
                    if resources.len() > RESOURCE_LIMIT {
                        details = details.with_child(
                            Text::new("and more in the skill folder", ui, text_size(11.5))
                                .with_color(theme.sub_text())
                                .finish(),
                        );
                    }
                }
                body = body.with_child(
                    ConstrainedBox::new(
                        ClippedScrollable::vertical(
                            view.skill_detail_scroll.clone(),
                            Container::new(details.finish())
                                .with_padding_left(12.0)
                                .with_padding_right(12.0)
                                .with_padding_top(12.0)
                                .with_padding_bottom(12.0)
                                .finish(),
                            ScrollbarWidth::Auto,
                            theme.disabled_text().into(),
                            theme.sub_text().into(),
                            theme.surface_1().into(),
                        )
                        .finish(),
                    )
                    .with_max_height(320.0)
                    .finish(),
                );
            }
            Container::new(body.finish())
                .with_padding_left(16.0)
                .with_padding_right(12.0)
                .with_padding_top(14.0)
                .with_padding_bottom(14.0)
                .finish()
        })
        .collect();
    let mut body = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(
            Container::new(card_rows(theme, rows))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(12.0)))
                .finish(),
        );
    if total > 8 {
        let label = if all_shown {
            "Show less  ⌃".to_string()
        } else {
            format!("Show {} more  ⌄", total - 8)
        };
        // Search results always stay visible; pagination only applies to the full inventory.
        if view.skill_search_query.trim().is_empty() {
            body = body.with_child(link_button(
                theme,
                ui,
                view.view_id,
                view.hover(&format!("skill-group-{group}")),
                &label,
                SettingsAction::ToggleSkillGroup(group.into()),
            ));
        }
    }
    body.finish()
}

pub(crate) fn render_skill_menu(view: &SettingsView, theme: &Theme) -> Option<Box<dyn Element>> {
    let path = view.open_picker.as_deref()?.strip_prefix("skill-menu:")?;
    let skill = skill_discovery::built_in_skills()
        .into_iter()
        .chain(skill_discovery::skills_for_scope(None))
        .chain(skill_discovery::skills_for_scope(Some(&view.project_root)))
        .find(|skill| skill.source_path.to_string_lossy() == path)?;
    let model = skill_row_model(&skill, None);
    let mut rows = Vec::new();
    for action in model.actions {
        rows.push(match action {
            SkillRowAction::Inspect => mui::menu_row(
                theme,
                view.ui_font,
                view.view_id,
                view.hover("skill-menu-view"),
                None,
                if view.expanded_skill.as_ref() == Some(&skill.source_path) { "Hide instructions" } else { "Inspect" },
                Some("Instructions and supporting files"),
                false,
                false,
                SettingsAction::ToggleSkillDetails(skill.source_path.clone()),
            ),
            SkillRowAction::EditFile => mui::menu_row(
                theme,
                view.ui_font,
                view.view_id,
                view.hover("skill-menu-edit"),
                None,
                "Edit file",
                None,
                false,
                false,
                SettingsAction::OpenSkill(skill.source_path.clone()),
            ),
            SkillRowAction::Remove => mui::menu_row(
                theme,
                view.ui_font,
                view.view_id,
                view.hover("skill-menu-remove"),
                None,
                "Remove…",
                None,
                false,
                true,
                SettingsAction::DeleteSkill(skill.source_path.clone()),
            ),
        });
    }
    let id = view.view_id;
    Some(
        warpui_core::elements::Dismiss::new(mui::menu_panel(theme, rows, 240.0))
            .on_dismiss(move |ctx, _| {
                ctx.dispatch_typed_action_for(id, SettingsAction::CloseSkillMenu)
            })
            .prevent_interaction_with_other_elements()
            .finish(),
    )
}

pub(super) fn render_mcp_access(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(12.0)
        .with_child(group_label(theme, view.ui_font, "Tool access in Dragent"))
        .with_child(render_plugin_system(view, theme))
        .with_child(render_plugin_inventory(view, theme))
        .finish()
}

/// Master switch row for the whole plugin system.
fn render_plugin_system(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    card_rows(
        theme,
        vec![toggle_row(
            theme,
            view.ui_font,
            view.view_id,
            view.hover("plugin-system"),
            "Enable MCP tools",
            Some("Allow Dragent’s agent to use connected MCP servers. Each server still needs approval."),
            view.app_settings.plugin.system_enabled,
            SettingsAction::TogglePluginSystem,
        )],
    )
}

/// One row per plugin. Without a separate registry, each MCP server is a
/// plugin entry, so these toggles take effect immediately at the MCP layer.
fn render_plugin_inventory(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    let ui = view.ui_font;
    let id = view.view_id;
    if view.app_settings.mcp_servers.is_empty() {
        return card_rows(
            theme,
            vec![setting_row(
                theme,
                ui,
                "No MCP servers added",
                Some("Add a server above to make its tools available."),
                Text::new("", ui, crate::theme::text_size(1.0)).finish(),
            )],
        );
    }
    let mut note = String::new();
    if !view.app_settings.plugin.system_enabled {
        note = "Plugin system is off — toggles below pre-arm the state for when it is enabled."
            .to_string();
    }
    let mut rows: Vec<Box<dyn Element>> = Vec::new();
    for server in &view.app_settings.mcp_servers {
        let name = server.name.clone();
        let enabled = view.app_settings.plugin.is_plugin_enabled(&name);
        let consent = view
            .app_settings
            .plugin
            .consents
            .get(&name)
            .cloned()
            .map(|v| format!("Approved ({v})"))
            .unwrap_or_else(|| "Needs approval".to_string());
        let desc = format!(
            "{} · {} · {consent}{}",
            server.transport.label(),
            server.display_command(),
            if note.is_empty() {
                String::new()
            } else {
                format!(" · {}", note)
            }
        );
        let mut actions = Flex::row().with_spacing(6.0);
        actions = actions.with_child(secondary_button(
            theme,
            ui,
            id,
            view.hover(&format!("plugin-toggle-{name}")),
            if enabled { "Disable" } else { "Enable" },
            SettingsAction::TogglePluginEnabled(name.clone()),
        ));
        if !view.app_settings.plugin.has_consent(&name) {
            actions = actions.with_child(secondary_button(
                theme,
                ui,
                id,
                view.hover(&format!("plugin-approve-{name}")),
                "Approve",
                SettingsAction::SetPluginConsent {
                    plugin: name.clone(),
                    version: PLUGIN_CONSENT_VERSION.to_string(),
                },
            ));
        }
        rows.push(setting_row(theme, ui, &name, Some(&desc), actions.finish()));
    }
    card_rows(theme, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn skill(name: &str, source: SkillSource, path: PathBuf) -> DiscoveredSkill {
        let text = format!("---\nname: {name}\ndescription: d\n---\nbody");
        let mut skill = skill_discovery::parse_skill_file(&text, &path).unwrap();
        skill.source = source;
        skill
    }

    fn project() -> SkillSource {
        SkillSource::ProjectLocal { project_root: PathBuf::from("/p") }
    }

    #[test]
    fn provenance_labels_are_distinct() {
        assert_eq!(provenance_label(&SkillSource::BuiltIn), "Built into Dragent");
        assert_eq!(provenance_label(&SkillSource::Global), "Shared (~/.agents/skills)");
        assert_eq!(provenance_label(&project()), "This project");
    }

    #[test]
    fn built_in_skill_is_inspect_only_and_not_for_other_agents() {
        let s = skill("alpha", SkillSource::BuiltIn, PathBuf::new());
        let m = skill_row_model(&s, Some(&s));
        assert_eq!(m.actions, vec![SkillRowAction::Inspect]);
        assert!(!m.read_by_other_agents);
        assert!(m.problems.is_empty());
        assert_eq!(m.invocation, "Agent can choose");
    }

    #[test]
    fn missing_file_and_overridden_copy_are_explained() {
        let shared = skill("beta", SkillSource::Global, PathBuf::from("/nope/beta/SKILL.md"));
        let proj = skill("beta", project(), PathBuf::from("/nope/p/beta/SKILL.md"));
        let m = skill_row_model(&shared, Some(&proj));
        assert!(m.read_by_other_agents);
        assert_eq!(m.actions, vec![SkillRowAction::Inspect]);
        assert!(m.notes.iter().any(|p| p.contains("project copy of this skill takes priority")));
        assert!(m.problems.iter().any(|p| p.contains("SKILL.md is missing")));
    }

    #[test]
    fn untrusted_project_skill_and_reserved_name_are_flagged() {
        let s = skill("plan", project(), PathBuf::from("/nope/help/SKILL.md"));
        let m = skill_row_model(&s, None);
        assert!(m.problems.iter().any(|p| p.contains("trusted")));
        assert!(m.problems.iter().any(|p| p.contains("Dragent command")));
    }

    #[test]
    fn existing_user_file_offers_edit_and_remove() {
        let dir = std::env::temp_dir().join(format!("dragent-skill-row-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("SKILL.md");
        std::fs::write(&path, "x").unwrap();
        let s = skill("gamma", SkillSource::Global, path);
        let m = skill_row_model(&s, Some(&s));
        std::fs::remove_dir_all(&dir).ok();
        assert!(m.actions.contains(&SkillRowAction::EditFile));
        assert_eq!(m.actions.contains(&SkillRowAction::Remove), cfg!(not(windows)));
        assert!(m.problems.is_empty());
    }
}

/// Dragent's skills for the other agents on this computer, each installed
/// on its own (with a preview first) and removable again.
#[cfg(not(target_family = "wasm"))]
fn other_agents_section(view: &SettingsView, theme: &Theme) -> Box<dyn Element> {
    use crate::cli::agent_skill::{self, Copy};
    let ui = view.ui_font;
    let rows = agent_skill::skill_rows();
    let first_sentence = |text: &str| -> String {
        match text.find(". ") {
            Some(end) => text[..=end].trim().to_string(),
            None => text.trim().to_string(),
        }
    };
    let list: Vec<Box<dyn Element>> = rows
        .iter()
        .map(|row| {
            let names = |copy: Copy| -> Vec<&str> {
                row.agents.iter().filter(|agent| agent.copy == copy).map(|agent| agent.agent.label()).collect()
            };
            let (installed, missing, theirs) = (names(Copy::Installed), names(Copy::NotInstalled), names(Copy::Theirs));
            let mut trailing = Vec::new();
            if !installed.is_empty() {
                let label = if missing.is_empty() {
                    "Installed".to_string()
                } else {
                    format!("Installed for {}", installed.join(", "))
                };
                trailing.push(mui::pill(theme, ui, &label, mui::Tone::Success));
            }
            if !theirs.is_empty() {
                trailing.push(mui::pill(theme, ui, &format!("Your own copy in {}", theirs.join(", ")), mui::Tone::Neutral));
            }
            if row.agents.is_empty() {
                trailing.push(mui::pill(theme, ui, "No other agents found", mui::Tone::Neutral));
            }
            if !installed.is_empty() {
                trailing.push(mui::button(
                    theme,
                    ui,
                    view.view_id,
                    view.hover(&format!("agent-skill-remove-{}", row.name)),
                    None,
                    "Remove",
                    mui::Variant::Ghost,
                    mui::Size::Xs,
                    true,
                    SettingsAction::RemoveAgentSkill(row.name.to_string()),
                ));
            }
            if !missing.is_empty() {
                let label = if missing.len() == 1 {
                    format!("Install for {}", missing[0])
                } else if installed.is_empty() {
                    format!("Install for {} agents", missing.len())
                } else {
                    format!("Install for {} more", missing.len())
                };
                trailing.push(mui::button(
                    theme,
                    ui,
                    view.view_id,
                    view.hover(&format!("agent-skill-install-{}", row.name)),
                    None,
                    &label,
                    mui::Variant::Secondary,
                    mui::Size::Xs,
                    true,
                    SettingsAction::InstallAgentSkill(row.name.to_string()),
                ));
            }
            mui::list_row(
                ui,
                theme,
                None,
                mui::medium(ui, row.name, 13.5, theme.main_text()),
                Some(first_sentence(&row.description)),
                trailing,
            )
        })
        .collect();
    let mut section = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(8.0)
        .with_child(Text::new(format!("Dragent skills for your other agents  {}", rows.len()), ui, text_size(14.0)).with_color(theme.main_text()).finish())
        .with_child(
            Text::new(
                "Agents you start from Dragent already have all of these. Install one to put it in your other agents' own skill folders, so they have it everywhere, also outside Dragent. You'll see exactly what is written first.".to_string(),
                ui,
                text_size(12.0),
            )
            .with_color(theme.sub_text())
            .finish(),
        );
    if let Some(note) = &view.agent_skill_note {
        section = section.with_child(Text::new(note.clone(), ui, text_size(12.0)).with_color(theme.sub_text()).finish());
    }
    if dragent_paths::is_dev() {
        section = section.with_child(
            Text::new("Dev builds don't change other apps' folders.".to_string(), ui, text_size(12.0))
                .with_color(theme.disabled_text())
                .finish(),
        );
    }
    section.with_child(mui::list_card(theme, list)).finish()
}
