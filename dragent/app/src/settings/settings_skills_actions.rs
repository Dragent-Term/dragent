use super::action::SettingsAction;
use super::settings_view::SettingsView;
use crate::settings::settings_view::SkillDraft;
use crate::settings::SettingsEvent;
use crate::workspace::InputEditor;
use crate::workspace::InputMode;
use warpui::UpdateView;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_skills_actions(
        &mut self,
        action: &SettingsAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            SettingsAction::ShowSkillCreator => {
                let ui = self.ui_font;
                let theme = self.theme();
                let input =
                    |placeholder: &'static str, key: &'static str, ctx: &mut ViewContext<Self>| {
                        ctx.add_typed_action_view(move |inner| {
                            InputEditor::new(
                                placeholder,
                                ui,
                                crate::theme::text_size(12.5),
                                theme.main_text(),
                                theme.disabled_text(),
                                theme.accent,
                                theme,
                                InputMode::Settings(key.into()),
                                inner.focus_handle(),
                            )
                        })
                    };
                let name_editor = input("my-skill", "skill_name", ctx);
                let description_editor =
                    input("When should this skill be used?", "skill_description", ctx);
                let body_editor = ctx.add_typed_action_view(move |inner| {
                    InputEditor::new_secure(
                        "Add skill instructions…",
                        ui,
                        crate::theme::text_size(12.5),
                        theme.main_text(),
                        theme.disabled_text(),
                        theme.accent,
                        theme,
                        InputMode::MultiLine {
                            key: "skill_body".into(),
                            max_height: 240,
                        },
                        inner.focus_handle(),
                        false,
                        "",
                    )
                });
                self.skill_draft = Some(SkillDraft {
                    name_editor,
                    description_editor,
                    body_editor,
                    project_scope: false,
                    disable_model_invocation: false,
                });
                self.status_message = None;
                ctx.notify();
            }
            SettingsAction::HideSkillCreator => {
                self.skill_draft = None;
                ctx.notify();
            }
            SettingsAction::ToggleSkillScope => {
                if let Some(draft) = &mut self.skill_draft {
                    draft.project_scope = !draft.project_scope;
                    ctx.notify();
                }
            }
            SettingsAction::ToggleSkillModelInvocation => {
                if let Some(draft) = &mut self.skill_draft {
                    draft.disable_model_invocation = !draft.disable_model_invocation;
                    ctx.notify();
                }
            }
            SettingsAction::SaveSkill => {
                if let Some(draft) = &self.skill_draft {
                    let name = draft
                        .name_editor
                        .read(ctx, |e, _| e.get_text().trim().to_string());
                    let description = draft
                        .description_editor
                        .read(ctx, |e, _| e.get_text().trim().to_string());
                    let body = draft.body_editor.read(ctx, |e, _| e.get_text().to_string());
                    let scope = draft.project_scope.then_some(self.project_root.as_path());
                    match crate::agent::skill_discovery::write_skill(
                        scope,
                        &name,
                        &description,
                        &body,
                        draft.disable_model_invocation,
                    ) {
                        Ok(path) => {
                            self.status_message = Some(format!(
                                "Installed {}. Agents already running elsewhere need a restart to see it.",
                                path.display()
                            ));
                            self.skill_draft = None;
                        }
                        Err(error) => self.status_message = Some(error),
                    }
                    ctx.notify();
                }
            }
            SettingsAction::CloseSkillMenu => { self.open_picker = None; ctx.notify(); }
            SettingsAction::ToggleSkillGroup(group) => {
                if !self.skill_groups_expanded.remove(group) { self.skill_groups_expanded.insert(group.clone()); }
                self.open_picker = None;
                ctx.notify();
            }
            SettingsAction::ToggleSkillDetails(path) => {
                self.open_picker = None;
                self.expanded_skill = if self.expanded_skill.as_ref() == Some(path) {
                    None
                } else {
                    Some(path.clone())
                };
                self.skill_detail_scroll = Default::default();
                ctx.notify();
            }
            SettingsAction::OpenSkill(path) => {
                self.open_picker = None;
                ctx.emit(SettingsEvent::OpenSkill(path.clone()));
            }
            SettingsAction::DeleteSkill(path) => {
                self.open_picker = None;
                self.pending_skill_delete = Some(path.clone());
                ctx.notify();
            }
            SettingsAction::CancelDeleteSkill => {
                self.pending_skill_delete = None;
                ctx.notify();
            }
            SettingsAction::ConfirmDeleteSkill => {
                if let Some(path) = self.pending_skill_delete.take() {
                    if let Some(directory) = path.parent() {
                        self.status_message = Some(
                            match crate::agent::skill_discovery::trash_skill_directory(directory) {
                                Ok(destination) => {
                                    format!("Moved skill to Trash: {}", destination.display())
                                }
                                Err(error) => format!("Could not delete skill: {error}"),
                            },
                        );
                    }
                }
                ctx.notify();
            }
            _ => {}
        }
    }
}
