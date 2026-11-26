//! Settings skills, models, and MCP forms.
//!
//! Skill creation, custom models, and MCP drafts.
//! Moved verbatim; same behavior.

use super::account_store::{AccountStore, AccountStoreEvent};
use super::action::{SettingsAction, SettingsSection};
use super::app_settings::{AppSettings, CustomModel, McpServerEntry, McpTransport};
use super::nav;
use super::pages;
use super::widgets::{
    link_button, primary_button, secondary_button, CONTENT_MAX_WIDTH, CONTENT_PADDING,
};
use crate::auth::{AuthManager, AuthManagerEvent};
#[cfg(target_family = "wasm")]
use crate::channel_ext::SendBlockingCompat;
use crate::console::editor_settings::EditorSettings;
use crate::setup::{reset_for_onboarding, UiSettings};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputMode};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use warpui::color::ColorU;
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CrossAxisAlignment, Element, Flex, MainAxisAlignment, MainAxisSize, ParentElement, Point,
    ScrollStateHandle, ScrollbarWidth, Shrinkable, Stack, Text, UniformListState,
};
use warpui::event::DispatchedEvent;
use warpui::fonts::FamilyId;
use warpui::geometry::rect::RectF;
use warpui::geometry::vector::Vector2F;
use warpui::{
    AfterLayoutContext, AppContext, Entity, EntityId, EventContext, GetSingletonModelHandle,
    LayoutContext, PaintContext, ReadModel, SingletonEntity, SizeConstraint, TypedActionView,
    UpdateModel, UpdateView, View, ViewContext, ViewHandle,
};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use super::settings_view::{CustomModelDraft, McpServerDraft, SkillDraft};
use super::settings_view::{SettingsEvent, SettingsView};

impl SettingsView {
    pub(crate) fn render_skill_delete_confirm(&self, theme: &Theme) -> Box<dyn Element> {
        let path = self.pending_skill_delete.as_ref().unwrap();
        let dialog = Container::new(Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(12.0)
            .with_child(Text::new("Delete this skill?", self.ui_font, crate::theme::text_size(17.0)).with_color(theme.main_text()).finish())
            .with_child(Text::new(format!("{} will be removed. This shared Agent Skills location may also be used by other tools.", path.parent().unwrap_or(path).display()), self.ui_font, crate::theme::text_size(12.0)).with_color(theme.sub_text()).finish())
            .with_child(Flex::row().with_main_axis_alignment(MainAxisAlignment::End).with_spacing(8.0)
                .with_child(secondary_button(theme, self.ui_font, self.view_id, self.hover("skill-delete-cancel"), "Cancel", SettingsAction::CancelDeleteSkill))
                .with_child(super::widgets::danger_button(theme, self.ui_font, self.view_id, self.hover("skill-delete-confirm"), "Delete", SettingsAction::ConfirmDeleteSkill)).finish()).finish())
            .with_background_color(theme.surface_1()).with_border(Border::all(1.0).with_border_color(theme.border())).with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0))).with_uniform_padding(22.0).finish();
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(520.0).finish()).finish(),
        )
        .with_background_color(ColorU::new(0, 0, 0, 160))
        .finish()
    }
    /// The preview before Dragent writes into another agent's own setup:
    /// every folder or file it would change, and a yes or no.
    pub(crate) fn render_agent_change_confirm(&self, theme: &Theme) -> Box<dyn Element> {
        use super::settings_view::AgentChange;
        let Some(change) = self.pending_agent_change.as_ref() else {
            return Flex::column().finish();
        };
        let shown = |path: &std::path::Path| match dirs::home_dir()
            .and_then(|home| path.strip_prefix(home).ok().map(|rest| rest.to_path_buf()))
        {
            Some(rest) => format!("~/{}", rest.display()),
            None => path.display().to_string(),
        };
        let (title, intro, paths, after) = match change {
            AgentChange::InstallSkill { name, agents, writes, kept } => {
                let names = agents.iter().map(|agent| agent.label()).collect::<Vec<_>>().join(", ");
                (
                    format!("Install {name} for {names}?"),
                    "Dragent will write the skill into these folders. Those agents then have it in every session, including ones you start outside Dragent:".to_string(),
                    writes.iter().map(|path| shown(path)).collect::<Vec<_>>(),
                    if kept.is_empty() {
                        "Remove takes it out again, except any file you edited.".to_string()
                    } else {
                        format!(
                            "Your own skill at {} stays as it is. Remove takes Dragent's out again, except any file you edited.",
                            kept.iter().map(|path| shown(path)).collect::<Vec<_>>().join(", ")
                        )
                    },
                )
            }
        };
        let small = |text: String, color| {
            Text::new(text, self.ui_font, crate::theme::text_size(12.0)).with_color(color).finish()
        };
        let mut list = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch).with_spacing(3.0);
        for path in paths {
            list = list.with_child(
                Text::new(path, self.ui_font, crate::theme::text_size(11.5))
                    .with_color(theme.main_text())
                    .finish(),
            );
        }
        let list = Container::new(list.finish())
            .with_background_color(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
            .with_uniform_padding(10.0)
            .finish();
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(
                    Text::new(title, self.ui_font, crate::theme::text_size(17.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(small(intro, theme.sub_text()))
                .with_child(list)
                .with_child(small(after, theme.sub_text()))
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_spacing(8.0)
                        .with_child(secondary_button(theme, self.ui_font, self.view_id, self.hover("agent-change-cancel"), "Cancel", SettingsAction::CancelAgentChange))
                        .with_child(primary_button(theme, self.ui_font, self.view_id, self.hover("agent-change-confirm"), "Install", SettingsAction::ConfirmAgentChange))
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(22.0)
        .finish();
        Container::new(Align::new(ConstrainedBox::new(dialog).with_max_width(560.0).finish()).finish())
            .with_background_color(ColorU::new(0, 0, 0, 160))
            .finish()
    }
    pub(crate) fn render_skill_modal(&self, theme: &Theme, draft: &SkillDraft) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;
        let field = |label: &str, editor: &ViewHandle<InputEditor>, multiline: bool| {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(5.0)
                .with_child(
                    Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(crate::shared_input::shared_input(
                    theme,
                    editor,
                    crate::shared_input::SharedInputOpts {
                        // Single-line fields share the fixed-height centering
                        // used in Agent Commands. Instructions retain their
                        // multiline editor and intrinsic, scrollable height.
                        height: if multiline { None } else { Some(36.0) },
                        ..Default::default()
                    },
                ))
                .finish()
        };
        let scope = if draft.project_scope {
            "Scope: this project"
        } else {
            "Scope: shared on this computer"
        };
        let invocation = if draft.disable_model_invocation {
            "Use: manual only"
        } else {
            "Use: agent can choose"
        };
        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(12.0)
                .with_child(
                    Text::new("Create a skill", ui, crate::theme::text_size(17.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(if let Some(message) = &self.status_message {
                    Text::new(message.clone(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.ui_red())
                        .finish()
                } else {
                    warpui::elements::Empty::new().finish()
                })
                .with_child(field("Name", &draft.name_editor, false))
                .with_child(field("Description", &draft.description_editor, false))
                .with_child(field("Instructions", &draft.body_editor, true))
                .with_child(
                    Flex::row()
                        .with_spacing(8.0)
                        .with_child(secondary_button(
                            theme,
                            ui,
                            id,
                            self.hover("skill-scope"),
                            scope,
                            SettingsAction::ToggleSkillScope,
                        ))
                        .with_child(secondary_button(
                            theme,
                            ui,
                            id,
                            self.hover("skill-invocation"),
                            invocation,
                            SettingsAction::ToggleSkillModelInvocation,
                        ))
                        .finish(),
                )
                .with_child(
                    Flex::row()
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_spacing(8.0)
                        .with_child(secondary_button(
                            theme,
                            ui,
                            id,
                            self.hover("skill-cancel"),
                            "Cancel",
                            SettingsAction::HideSkillCreator,
                        ))
                        .with_child(primary_button(
                            theme,
                            ui,
                            id,
                            self.hover("skill-save"),
                            "Save skill",
                            SettingsAction::SaveSkill,
                        ))
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(22.0)
        .finish();
        Container::new(
            Align::new(ConstrainedBox::new(dialog).with_max_width(620.0).finish()).finish(),
        )
        .with_background_color(ColorU::new(0, 0, 0, 160))
        .finish()
    }
    /// The "Log out?" confirmation, warning about any live shared session.
    pub(crate) fn render_logout_confirm_modal(
        &self,
        theme: &Theme,
        app: &AppContext,
    ) -> Box<dyn Element> {
        super::logout_confirm::logout_confirm_modal(
            theme,
            self.ui_font,
            self.view_id,
            app,
            self.logout_dont_ask,
            |key| self.hover(key),
            super::logout_confirm::LogoutConfirmActions {
                toggle_dont_ask: SettingsAction::ToggleLogoutDontAsk,
                cancel: SettingsAction::CancelSignOut,
                confirm: SettingsAction::ConfirmSignOut,
            },
        )
    }
}

impl SettingsView {
    pub(crate) fn render_custom_model_modal(
        &self,
        theme: &Theme,
        draft: &CustomModelDraft,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;

        let field = |label: &str, editor: &ViewHandle<InputEditor>| -> Box<dyn Element> {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.0)
                .with_child(
                    Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(crate::shared_input::shared_input(
                    theme,
                    editor,
                    crate::shared_input::SharedInputOpts {
                        width: crate::shared_input::SharedInputWidth::Fill,
                        radius: 6.0,
                        pad_left: 10.0,
                        pad_right: 10.0,
                        pad_top: 7.0,
                        pad_bottom: 7.0,
                        ..Default::default()
                    },
                ))
                .finish()
        };

        let is_editing = draft.editing_index.is_some();
        let confirm_hover = self.hover("modal_confirm");
        let cancel_hover = self.hover("modal_cancel");

        let dialog = Container::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(14.0)
                .with_child(
                    Text::new(
                        if is_editing {
                            "Edit custom model"
                        } else {
                            "Add custom model"
                        }
                        .to_string(),
                        ui,
                        crate::theme::text_size(16.0),
                    )
                        .with_color(theme.main_text())
                        .finish(),
                )
                .with_child(
                    Text::new(
                        "Configure a custom endpoint. Note: The endpoint must support the OpenAI-compatible Chat Completions API format (e.g. Ollama, LM Studio, or custom proxies).".to_string(),
                        ui,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.sub_text())
                    .finish(),
                )
                .with_child(field("Model name", &draft.name_editor))
                .with_child(field("Base URL (OpenAI-compatible endpoint)", &draft.base_url_editor))
                .with_child(field("Model ID", &draft.model_id_editor))
                .with_child(field("API Key", &draft.api_key_editor))
                .with_child(
                    Flex::row()
                        .with_spacing(8.0)
                        .with_main_axis_alignment(MainAxisAlignment::End)
                        .with_child(secondary_button(theme, ui, id, cancel_hover, "Cancel", SettingsAction::CloseCustomModelModal))
                        .with_child(primary_button(
                            theme,
                            ui,
                            id,
                            confirm_hover,
                            if is_editing { "Save changes" } else { "Save model" },
                            SettingsAction::ConfirmCustomModal,
                        ))
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.border()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_uniform_padding(24.0)
        .finish();

        let centered_dialog = ConstrainedBox::new(dialog).with_max_width(480.0).finish();

        Container::new(Align::new(centered_dialog).finish())
            .with_background_color(warpui::color::ColorU::new(0, 0, 0, 160))
            .finish()
    }
}

impl SettingsView {
    pub(crate) fn open_custom_model_modal(
        &mut self,
        editing_index: Option<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let ui = self.ui_font;
        let theme = self.theme();
        let (name, base_url, model_id, api_key, endpoint_style) = editing_index
            .and_then(|index| self.app_settings.custom_models.get(index))
            .map(|model| {
                (
                    model.name.clone(),
                    model.base_url.clone(),
                    model.model_id.clone(),
                    // M2: prefill from the keychain (native) or the legacy
                    // field, so editing a migrated record shows its key.
                    super::credentials::custom_api_key(model).unwrap_or_default(),
                    model.endpoint_style,
                )
            })
            .unwrap_or_default();

        let make_editor = |ctx: &mut ViewContext<Self>,
                           placeholder: &'static str,
                           mode: &'static str,
                           initial: String,
                           secure: bool| {
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    placeholder,
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings(mode.to_string()),
                    inner.focus_handle(),
                    secure,
                    &initial,
                )
            })
        };

        let name_editor = make_editor(ctx, "Model name", "modal_name", name.clone(), false);
        let base_url_editor = make_editor(
            ctx,
            "http://localhost:8000/v1",
            "modal_base_url",
            base_url.clone(),
            false,
        );
        let model_id_editor =
            make_editor(ctx, "my-model", "modal_model_id", model_id.clone(), false);
        let api_key_editor = make_editor(ctx, "Optional", "modal_api_key", api_key.clone(), true);

        self.custom_model_modal = Some(CustomModelDraft {
            name_editor,
            base_url_editor,
            model_id_editor,
            api_key_editor,
            name,
            base_url,
            model_id,
            api_key,
            endpoint_style,
            editing_index,
        });
    }
    pub(crate) fn open_mcp_form(
        &mut self,
        editing_index: Option<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let ui = self.ui_font;
        let theme = self.theme();
        let init_agents = editing_index
            .and_then(|i| self.app_settings.mcp_servers.get(i))
            .map(|s| s.sync_agents.clone())
            .unwrap_or_default();

        let (init_name, init_cmd, init_args, init_env, init_bearer, init_transport) = editing_index
            .and_then(|i| self.app_settings.mcp_servers.get(i))
            .map(|s| {
                let args_raw = s.args.join("\n");
                let env_raw = s
                    .env
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let bearer = s.bearer_token.clone().unwrap_or_default();
                (
                    s.name.clone(),
                    s.command.clone(),
                    args_raw,
                    env_raw,
                    bearer,
                    s.transport,
                )
            })
            .unwrap_or_default();

        let name_ed = {
            let t = init_name.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "e.g. GitHub",
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("mcp_name".to_string()),
                    inner.focus_handle(),
                    false,
                    &t,
                )
            })
        };
        let cmd_ed = {
            let t = init_cmd.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "command or https://...",
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("mcp_command".to_string()),
                    inner.focus_handle(),
                    false,
                    &t,
                )
            })
        };
        let args_ed = {
            let t = init_args.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "one argument per line",
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("mcp_args".to_string()),
                    inner.focus_handle(),
                    false,
                    &t,
                )
            })
        };
        let env_ed = {
            let t = init_env.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "KEY=value (one per line)",
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("mcp_env".to_string()),
                    inner.focus_handle(),
                    false,
                    &t,
                )
            })
        };
        let bearer_ed = {
            let t = init_bearer.clone();
            ctx.add_typed_action_view(move |inner| {
                InputEditor::new_secure(
                    "Bearer token (optional)",
                    ui,
                    crate::theme::text_size(13.0),
                    theme.main_text(),
                    theme.disabled_text(),
                    theme.accent,
                    theme,
                    InputMode::Settings("mcp_bearer_token".to_string()),
                    inner.focus_handle(),
                    true,
                    &t,
                )
            })
        };

        self.mcp_form = Some(McpServerDraft {
            name_editor: name_ed,
            command_editor: cmd_ed,
            args_editor: args_ed,
            env_editor: env_ed,
            bearer_token_editor: bearer_ed,
            name: init_name,
            command: init_cmd,
            args_raw: init_args,
            env_raw: init_env,
            bearer_token: init_bearer,
            transport: init_transport,
            editing_index,
            sync_agents: init_agents,
        });
        self.section = super::action::SettingsSection::AgentMcp;
    }
    pub(crate) fn save_mcp_draft(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(draft) = self.mcp_form.take() else {
            return;
        };
        let name = draft.name.trim().to_string();
        let command_raw = draft.command.trim().to_string();
        if name.is_empty() || command_raw.is_empty() {
            self.mcp_status_message = Some("Name and command are required.".to_string());
            self.mcp_form = Some(draft);
            return;
        }

        // If the command field contains spaces, treat it as a full shell command
        // (e.g. "npx -y @modelcontextprotocol/server-filesystem /Users/me") and
        // split it: first token → binary, remaining tokens → prepended to args.
        let (command, extra_args) = if command_raw.contains(' ') {
            let mut tokens = command_raw.split_whitespace();
            let bin = tokens.next().unwrap().to_string();
            let rest: Vec<String> = tokens.map(|s| s.to_string()).collect();
            (bin, rest)
        } else {
            (command_raw, vec![])
        };

        let args: Vec<String> = extra_args
            .into_iter()
            .chain(
                draft
                    .args_raw
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            )
            .collect();

        let env: std::collections::HashMap<String, String> = draft
            .env_raw
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let eq = line.find('=')?;
                Some((
                    line[..eq].trim().to_string(),
                    line[eq + 1..].trim().to_string(),
                ))
            })
            .collect();

        let entry = McpServerEntry {
            name: name.clone(),
            command,
            transport: draft.transport,
            args,
            env,
            enabled: true,
            bearer_token: {
                let b = draft.bearer_token.trim().to_string();
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            },
            auth_url: None,
            token_url: None,
            oauth_scope: None,
            sync_agents: draft.sync_agents.clone(),
        };

        let is_sse = draft.transport == McpTransport::Sse;
        let has_empty_env = entry.env.values().any(|v| v.is_empty());

        // Decide whether to auto-connect: always reconnect when editing an
        // existing server (user just provided credentials), and for new SSE
        // servers only if they have a bearer token (no OAuth trigger without
        // an explicit Connect click).
        let should_reconnect = match draft.editing_index {
            Some(_) => true,
            None => !is_sse || entry.bearer_token.is_some(),
        };
        // But skip stdio servers that still have empty env vars — connecting
        // would just fail again until the user fills them in.
        let should_reconnect = should_reconnect && (!has_empty_env || is_sse);

        match draft.editing_index {
            Some(i) if i < self.app_settings.mcp_servers.len() => {
                self.app_settings.mcp_servers[i] = entry;
                self.mcp_status_message = Some(format!("Updated \"{name}\". Reconnecting…"));
            }
            _ => {
                if self.app_settings.mcp_servers.iter().any(|s| s.name == name) {
                    self.mcp_status_message = Some(format!("\"{name}\" already exists."));
                    return;
                }
                self.app_settings.mcp_servers.push(entry);
                self.mcp_status_message = Some(if should_reconnect {
                    format!("Added \"{name}\". Connecting…")
                } else {
                    format!("Added \"{name}\". Click Connect to authenticate.")
                });
            }
        }

        // Adding a server here is the user's approval of it.
        self.app_settings
            .plugin
            .set_consent(&name, crate::settings::PLUGIN_CONSENT_VERSION);
        self.app_settings.save();
        self.persist_mcp_servers(ctx);
        ctx.emit(SettingsEvent::AppSettingsChanged);

        if should_reconnect {
            let tx = self.mcp_reconnect_tx.clone();
            let name_for_cb = name.clone();
            crate::agent::mcp::reinit_server(
                &name,
                Some(Box::new(move || {
                    let _ = tx.send_blocking(name_for_cb);
                })),
            );
        }
    }
}
impl SettingsView {
    pub(crate) fn render_mcp_form_modal(
        &self,
        theme: &Theme,
        draft: &McpServerDraft,
    ) -> Box<dyn Element> {
        let ui = self.ui_font;
        let id = self.view_id;

        let field = |label: &str, editor: &ViewHandle<InputEditor>| -> Box<dyn Element> {
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_spacing(4.0)
                .with_child(
                    Text::new(label.to_string(), ui, crate::theme::text_size(12.0))
                        .with_color(theme.sub_text())
                        .finish(),
                )
                .with_child(crate::shared_input::shared_input(
                    theme,
                    editor,
                    crate::shared_input::SharedInputOpts {
                        width: crate::shared_input::SharedInputWidth::Fill,
                        radius: 6.0,
                        pad_left: 10.0,
                        pad_right: 10.0,
                        pad_top: 7.0,
                        pad_bottom: 7.0,
                        ..Default::default()
                    },
                ))
                .finish()
        };

        let is_stdio = draft.transport == McpTransport::Stdio;

        let transport_row = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.0)
            .with_child(
                Text::new("Transport".to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .with_child(
                Flex::row()
                    .with_spacing(8.0)
                    .with_child(if is_stdio {
                        primary_button(
                            theme,
                            ui,
                            id,
                            self.hover("mcp_t_stdio"),
                            "stdio (process)",
                            SettingsAction::UpdateMcpDraftField(
                                "mcp_transport_stdio".to_string(),
                                String::new(),
                            ),
                        )
                    } else {
                        secondary_button(
                            theme,
                            ui,
                            id,
                            self.hover("mcp_t_stdio"),
                            "stdio (process)",
                            SettingsAction::UpdateMcpDraftField(
                                "mcp_transport_stdio".to_string(),
                                String::new(),
                            ),
                        )
                    })
                    .with_child(if !is_stdio {
                        primary_button(
                            theme,
                            ui,
                            id,
                            self.hover("mcp_t_sse"),
                            "SSE (HTTP)",
                            SettingsAction::UpdateMcpDraftField(
                                "mcp_transport_sse".to_string(),
                                String::new(),
                            ),
                        )
                    } else {
                        secondary_button(
                            theme,
                            ui,
                            id,
                            self.hover("mcp_t_sse"),
                            "SSE (HTTP)",
                            SettingsAction::UpdateMcpDraftField(
                                "mcp_transport_sse".to_string(),
                                String::new(),
                            ),
                        )
                    })
                    .finish(),
            )
            .finish();

        let cmd_label = if is_stdio {
            "Command (executable)"
        } else {
            "Server URL"
        };
        let title = if draft.editing_index.is_some() {
            "Edit MCP Server"
        } else {
            "Add MCP Server"
        };

        let buttons = Flex::row()
            .with_spacing(8.0)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_child(secondary_button(
                theme,
                ui,
                id,
                self.hover("mcp_form_cancel"),
                "Cancel",
                SettingsAction::HideMcpForm,
            ))
            .with_child(primary_button(
                theme,
                ui,
                id,
                self.hover("mcp_form_save"),
                "Save server",
                SettingsAction::SaveMcpServer,
            ))
            .finish();

        let mut form = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(14.0)
            .with_child(
                Text::new(title.to_string(), ui, crate::theme::text_size(16.0))
                    .with_color(theme.main_text())
                    .finish(),
            )
            .with_child(field("Server name", &draft.name_editor))
            .with_child(transport_row)
            .with_child(field(cmd_label, &draft.command_editor));

        if is_stdio {
            form = form
                .with_child(field("Arguments (one per line)", &draft.args_editor))
                .with_child(field(
                    "Environment variables (KEY=value, one per line)",
                    &draft.env_editor,
                ));
        } else {
            form = form.with_child(field(
                "Bearer token (optional — leave blank to use OAuth)",
                &draft.bearer_token_editor,
            ));
        }

        form = form.with_child(self.render_mcp_agent_picker(theme, draft));
        form = form.with_child(buttons);

        let dialog = Container::new(form.finish())
            .with_background_color(theme.surface_1())
            .with_border(Border::all(1.0).with_border_color(theme.border()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
            .with_uniform_padding(24.0)
            .finish();

        let centered = ConstrainedBox::new(dialog).with_max_width(520.0).finish();

        Container::new(Align::new(centered).finish())
            .with_background_color(warpui::color::ColorU::new(0, 0, 0, 160))
            .finish()
    }
}

/// An agent to tick: its icon and name, accented while chosen.
fn agent_choice(
    theme: &Theme,
    ui: FamilyId,
    view_id: warpui::EntityId,
    hover: warpui_core::elements::MouseStateHandle,
    agent: crate::agent::mcp_agent_sync::SyncAgent,
    chosen: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    use crate::icons::{render_icon, AppIcon};
    let theme = *theme;
    warpui_core::elements::Hoverable::new(hover, move |state| {
        let hovered = state.is_hovered();
        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(7.0)
            .with_child(crate::workspace::prompt_agent_image(
                crate::agent::mcp_agent_sync::prompt_agent(agent),
                &theme,
                15.0,
                1.0,
            ))
            .with_child(
                Text::new(agent.label().to_string(), ui, crate::theme::text_size(12.5))
                    .with_color(theme.main_text())
                    .finish(),
            );
        if chosen {
            row = row.with_child(render_icon(AppIcon::Check, theme.accent, 12.0));
        }
        Container::new(row.finish())
            .with_background_color(if chosen {
                crate::machine_ui::alpha(theme.accent, 0x1f)
            } else if hovered {
                theme.surface_1()
            } else {
                ColorU::transparent_black()
            })
            .with_border(warpui_core::scene::Border::all(1.0).with_border_color(if chosen {
                theme.accent
            } else if hovered {
                theme.border()
            } else {
                theme.outline()
            }))
            .with_corner_radius(warpui_core::scene::CornerRadius::with_all(
                warpui_core::scene::Radius::Pixels(7.0),
            ))
            .with_padding_left(9.0)
            .with_padding_right(10.0)
            .with_padding_top(5.0)
            .with_padding_bottom(5.0)
            .finish()
    })
    .with_cursor(warpui_core::platform::Cursor::PointingHand)
    .on_click(move |ctx, _, _| ctx.dispatch_typed_action_for(view_id, action.clone()))
    .finish()
}

impl SettingsView {
    /// "Also use in": the other agents this server is written into. Lists the
    /// agents found on this machine, plus any already chosen.
    fn render_mcp_agent_picker(&self, theme: &Theme, draft: &McpServerDraft) -> Box<dyn Element> {
        use crate::agent::mcp_agent_sync::SyncAgent;
        let ui = self.ui_font;
        let id = self.view_id;
        let home = dirs::home_dir().unwrap_or_default();
        let agents: Vec<SyncAgent> = SyncAgent::ALL
            .into_iter()
            .filter(|agent| {
                agent.is_present(&home) || draft.sync_agents.iter().any(|k| k == agent.key())
            })
            .collect();
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.0)
            .with_child(
                Text::new("Also use in".to_string(), ui, crate::theme::text_size(12.0))
                    .with_color(theme.sub_text())
                    .finish(),
            );
        if agents.is_empty() {
            return column
                .with_child(
                    Text::new(
                        "No other coding agents found on this Mac.".to_string(),
                        ui,
                        crate::theme::text_size(12.0),
                    )
                    .with_color(theme.disabled_text())
                    .finish(),
                )
                .finish();
        }
        let mut choices = warpui::elements::Wrap::row().with_spacing(8.0).with_run_spacing(8.0);
        for agent in &agents {
            let chosen = draft.sync_agents.iter().any(|k| k == agent.key());
            let action = SettingsAction::ToggleMcpDraftAgent(agent.key().to_string());
            let hover = self.hover(&format!("mcp_agent_{}", agent.key()));
            choices = choices.with_child(agent_choice(theme, ui, id, hover, *agent, chosen, action));
        }
        if agents.len() > 1 {
            choices = choices.with_child(link_button(
                theme,
                ui,
                id,
                self.hover("mcp_agent_all"),
                "All",
                SettingsAction::SelectAllMcpDraftAgents,
            ));
        }
        column = column.with_child(choices.finish()).with_child(
            Text::new(
                "Dragent adds this server to each chosen agent's own MCP settings and removes it again if you untick the agent or delete the server. Servers you sign into in the browser ask each agent to sign in once.".to_string(),
                ui,
                crate::theme::text_size(11.0),
            )
            .with_color(theme.disabled_text())
            .finish(),
        );
        column.finish()
    }
}
