use super::action::SettingsAction;
use super::settings_view::SettingsView;
use warpui::ViewContext;

impl SettingsView {
    pub(crate) fn handle_index(&mut self, action: &SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::ToggleIndexList => {
                self.index_list_expanded = !self.index_list_expanded;
                self.index_list_page = 0;
                ctx.notify();
            }
            SettingsAction::IndexListPage(page) => {
                self.index_list_page = *page;
                ctx.notify();
            }
            SettingsAction::ToggleIndexingEnabled => {
                let on = !self.app_settings.index.enabled;
                self.app_settings.index.enabled = on;
                self.app_settings.save();
                let manager = dragent_index::get_manager();
                manager.set_enabled(on);
                self.index_status_message = Some(if on {
                    "Indexing enabled — rebuilding watchers…".to_string()
                } else {
                    "Indexing disabled. Existing indexes are kept on disk.".to_string()
                });
                if on {
                    self.arm_index_poll(ctx);
                }
            }
            SettingsAction::ToggleAutoIndex => {
                let on = !self.app_settings.index.auto_index;
                self.app_settings.index.auto_index = on;
                self.app_settings.save();
                dragent_index::get_manager().set_auto_index(on);
            }
            SettingsAction::ToggleRespectGitignore => {
                let respect = !self.app_settings.index.respect_gitignore;
                self.app_settings.index.respect_gitignore = respect;
                self.app_settings.save();
                let manager = dragent_index::get_manager();
                manager.set_respect_gitignore(respect);
                manager.reindex_all();
                self.index_status_message =
                    Some("Re-indexing with the new ignore rules…".to_string());
            }
            SettingsAction::SelectEmbeddingsBackend(backend) => {
                // Cloud has no backend yet — the UI marks it disabled, and this
                // guard keeps a stray action from persisting an unusable state.
                if *backend == super::app_settings::EmbeddingsBackend::Local {
                    self.app_settings.index.embeddings = *backend;
                    self.app_settings.save();
                }
                self.open_picker = None;
            }
            SettingsAction::ReindexRepo(root) => {
                dragent_index::get_manager().reindex(root);
                self.index_status_message = Some(format!(
                    "Re-indexing {}…",
                    root.file_name().and_then(|n| n.to_str()).unwrap_or("repo")
                ));
                self.arm_index_poll(ctx);
            }
            SettingsAction::DeleteIndex(root) => {
                dragent_index::get_manager().remove_index(root);
                self.index_status_message = Some(format!(
                    "Deleted the index for {}. Re-open the folder in a terminal to rebuild it.",
                    root.file_name().and_then(|n| n.to_str()).unwrap_or("repo")
                ));
                self.index_list_page = 0;
                ctx.notify();
            }
            SettingsAction::SaveIgnorePatterns => {
                let text = self
                    .ignore_patterns_editor
                    .as_ref()
                    .map(|e| e.read(ctx, |ed, _| ed.get_text().to_string()))
                    .unwrap_or_default();
                match dragent_index::walk::global_ignore_path() {
                    Some(path) => {
                        let write = path
                            .parent()
                            .map(|p| std::fs::create_dir_all(p))
                            .unwrap_or(Ok(()))
                            .and_then(|_| std::fs::write(&path, text.as_bytes()));
                        match write {
                            Ok(()) => {
                                dragent_index::get_manager().reindex_all();
                                self.index_status_message =
                                    Some("Ignore patterns saved — re-indexing…".to_string());
                            }
                            Err(e) => {
                                self.index_status_message =
                                    Some(format!("Could not save ignore patterns: {e}"));
                            }
                        }
                    }
                    None => {
                        self.index_status_message =
                            Some("Could not locate the config directory.".to_string());
                    }
                }
            }
            _ => {}
        }
    }
}
