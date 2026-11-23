use warpui::color::ColorU;
use warpui::elements::{
    ChildAnchor, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex,
    MainAxisAlignment, MainAxisSize, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Shrinkable, Stack, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, EntityId, ReadModel, SingletonEntity};
use warpui_core::elements::{Hoverable, MouseStateHandle};
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::settings::account_store::{AccountStore, ModelInfo};
use crate::settings::app_settings::AUTO_MODEL_ID;
use crate::theme::Theme;

use super::super::action::SettingsAction;
use super::super::settings_view::SettingsView;
use super::super::widgets::{
    card_rows, group_label, page_header, secondary_button, setting_row, toggle_row,
};

pub fn render_agent_models(
    view: &SettingsView,
    theme: &Theme,
    app: &AppContext,
) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let view_id = view.view_id;

    // Models from the server — only active ones.
    let account = AccountStore::handle(app);
    let mut db_models: Vec<ModelInfo> = app.read_model(&account, |store, _| {
        store
            .data
            .as_ref()
            .map(|d| d.models.iter().filter(|m| m.is_active).cloned().collect())
            .unwrap_or_default()
    });

    // Append custom models to the dropdown list.
    for cm in &view.app_settings.custom_models {
        db_models.push(ModelInfo {
            id: cm.model_id.clone(),
            name: cm.name.clone(),
            provider: "custom".to_string(),
            context_window: 128000,
            min_tier: "free".to_string(),
            is_active: true,
        });
    }

    let settings = &view.app_settings;
    let has_byok = settings.has_any_user_credential();
    if crate::api::is_local_mode() {
        // No server: only models the person's own keys can call.
        db_models = crate::settings::local_models::local_mode_models(settings);
    } else if has_byok {
        db_models.retain(|model| {
            model.id == AUTO_MODEL_ID || settings.has_user_credential_for_model(&model.id)
        });
    }
    let restricted_to_free = app.read_model(&account, |store, _| {
        store
            .data
            .as_ref()
            .map(|data| {
                data.usage.model_locked_to_free || data.user.tier.eq_ignore_ascii_case("free")
            })
            .unwrap_or(false)
    });
    let locked_to_free = restricted_to_free && !has_byok;
    let primary_select = model_select_widget(view, theme, &db_models, locked_to_free);

    let mut card_elements = vec![setting_row(
        theme,
        ui_font,
        "Primary model",
        Some("Used for fix suggestions and agent tasks."),
        primary_select,
    )];

    // List each added custom model with edit and remove controls.
    for (idx, cm) in view.app_settings.custom_models.iter().enumerate() {
        let edit_hover = view.hover(&format!("edit_custom_{}", idx));
        let remove_hover = view.hover(&format!("remove_custom_{}", idx));
        let actions = Flex::row()
            .with_spacing(8.0)
            .with_child(secondary_button(
                theme,
                ui_font,
                view_id,
                edit_hover,
                "Edit",
                SettingsAction::EditCustomModel(idx),
            ))
            .with_child(secondary_button(
                theme,
                ui_font,
                view_id,
                remove_hover,
                "Remove",
                SettingsAction::RemoveCustomModel(idx),
            ))
            .finish();
        let desc = view
            .keychain_errors
            .get(&crate::settings::credentials::account_for_custom(&cm.model_id))
            .cloned()
            .unwrap_or_else(|| format!("Custom OpenAI-compatible model: {}", cm.model_id));
        card_elements.push(setting_row(theme, ui_font, &cm.name, Some(&desc), actions));
    }

    // "Add custom model" action row
    let add_custom_hover = view.hover("add_custom_model_btn");
    let add_custom_btn = secondary_button(
        theme,
        ui_font,
        view_id,
        add_custom_hover,
        "Add custom model",
        SettingsAction::AddCustomModel,
    );
    card_elements.push(setting_row(
        theme,
        ui_font,
        "Add custom model",
        Some("Configure a local or hosted OpenAI-compatible model endpoint (e.g. Ollama, LM Studio)."),
        add_custom_btn,
    ));

    let unified_card = card_rows(theme, card_elements);

    // Core BYOK providers must remain configurable even when account/model
    // filtering omits all of their models. Additional active providers are
    // appended when the account advertises them.
    let active_providers = provider_ids_for_key_settings(&db_models);

    let mut col = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_child(page_header(
            theme,
            ui_font,
            "Models & Keys",
            "Select your model and optionally supply your own API keys.",
        ))
        .with_child(group_label(theme, ui_font, "Model routing"))
        .with_child(unified_card);

    let key_card = render_provider_key_card(view, theme, &active_providers);
    col = col
        .with_child(group_label(theme, ui_font, "Provider keys"))
        .with_child(key_card);

    // Fallback policy: when a user key is missing or failing, may Dragent's
    // platform keys serve the request? Off = never (policy-safe).
    col = col
        .with_child(group_label(theme, ui_font, "Fallback"))
        .with_child(card_rows(theme, vec![toggle_row(
            theme,
            ui_font,
            view_id,
            view.hover("dragent_fallback"),
            "Dragent provider fallback",
            Some("If your API key is missing or errors, fall back to Dragent's keys. When off, such requests fail instead of ever using our keys."),
            view.app_settings.allow_dragent_fallback,
            SettingsAction::ToggleDragentFallback,
        )]));

    col.finish()
}

fn provider_ids_for_key_settings(models: &[ModelInfo]) -> Vec<String> {
    const CORE_PROVIDERS: [&str; 4] = ["anthropic", "openai", "google", "xai"];
    let mut providers = CORE_PROVIDERS
        .iter()
        .map(|provider| (*provider).to_string())
        .collect::<Vec<_>>();
    let mut seen = CORE_PROVIDERS
        .into_iter()
        .collect::<std::collections::HashSet<_>>();

    for model in models {
        let provider = model.provider.as_str();
        if provider != "custom" && seen.insert(provider) {
            providers.push(model.provider.clone());
        }
    }
    providers
}

// ── Model select ──────────────────────────────────────────────────────────────

fn model_select_widget(
    view: &SettingsView,
    theme: &Theme,
    db_models: &[ModelInfo],
    locked_to_free: bool,
) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let view_id = view.view_id;
    let is_open = view.primary_picker_open;
    let current_id = if locked_to_free || view.app_settings.selected_model == AUTO_MODEL_ID {
        AUTO_MODEL_ID
    } else {
        view.app_settings.selected_model.as_str()
    };

    // Display label: prefer DB name, fall back to the model ID itself.
    let label = db_models
        .iter()
        .find(|m| m.id == current_id)
        .map(|m| m.name.as_str())
        .unwrap_or(current_id)
        .to_string();

    let hover = view.hover("primary_sel_btn");
    let button = select_button(
        theme,
        ui_font,
        view_id,
        hover,
        &label,
        is_open && !locked_to_free,
        if locked_to_free {
            None
        } else {
            Some(SettingsAction::TogglePrimaryPicker)
        },
        locked_to_free,
    );

    if locked_to_free || !is_open || db_models.is_empty() {
        return button;
    }

    // Inline picker — only shows models returned by the API.
    let mut picker = Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_main_axis_size(MainAxisSize::Min)
        .with_spacing(2.0);

    let auto_hover = view.hover("pick_auto");
    picker = picker.with_child(picker_item(
        theme,
        ui_font,
        view_id,
        auto_hover,
        "Auto",
        current_id == AUTO_MODEL_ID,
        SettingsAction::SelectModel(AUTO_MODEL_ID.to_string()),
    ));
    for model in db_models {
        let selected = current_id == model.id;
        let action = SettingsAction::SelectModel(model.id.clone());
        let item_hover = view.hover(&format!("pick_{}", model.id));
        picker = picker.with_child(picker_item(
            theme,
            ui_font,
            view_id,
            item_hover,
            &model.name,
            selected,
            action,
        ));
    }

    let dropdown = Container::new(picker.finish())
        .with_background_color(theme.surface_1())
        .with_border(Border::all(1.0).with_border_color(theme.outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.0)))
        .with_padding_left(6.0)
        .with_padding_right(6.0)
        .with_padding_top(6.0)
        .with_padding_bottom(6.0)
        .finish();

    let dropdown_constrained = ConstrainedBox::new(dropdown)
        .with_min_width(220.0)
        .with_max_width(220.0)
        .finish();

    let offset = OffsetPositioning::offset_from_parent(
        warpui::geometry::vector::vec2f(0.0, 4.0),
        ParentOffsetBounds::WindowByPosition,
        ParentAnchor::BottomLeft,
        ChildAnchor::TopLeft,
    );

    Box::new(
        Stack::new()
            .with_child(button)
            .with_positioned_overlay_child(dropdown_constrained, offset),
    )
}

fn select_button(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    is_open: bool,
    action: Option<SettingsAction>,
    locked: bool,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    let chevron = if locked {
        "🔒"
    } else if is_open {
        "▴"
    } else {
        "▾"
    };
    let chevron_owned = chevron.to_string();
    let border_color = if is_open {
        theme.accent
    } else {
        theme.outline()
    };
    let hoverable = Hoverable::new(hover, move |state| {
        let bg = if state.is_hovered() {
            theme.surface_2()
        } else {
            theme.surface_1()
        };
        let inner = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                        .with_color(theme.main_text())
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new(chevron_owned.clone(), ui_font, crate::theme::text_size(10.0))
                    .with_color(theme.sub_text())
                    .finish(),
            )
            .finish();
        ConstrainedBox::new(
            Container::new(inner)
                .with_background_color(bg)
                .with_border(Border::all(1.0).with_border_color(border_color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.0)))
                .with_padding_left(12.0)
                .with_padding_right(10.0)
                .with_padding_top(8.0)
                .with_padding_bottom(8.0)
                .finish(),
        )
        .with_min_width(220.0)
        .with_max_width(220.0)
        .finish()
    });
    if let Some(action) = action {
        Box::new(hoverable.on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }))
    } else {
        Box::new(hoverable)
    }
}

// ── Provider key section ──────────────────────────────────────────────────────

fn render_provider_key_card(
    view: &SettingsView,
    theme: &Theme,
    providers: &[String],
) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let rows: Vec<Box<dyn Element>> = providers
        .iter()
        .map(|pid| {
            let name = provider_display_name(pid);
            let right = provider_key_right(view, theme, pid);
            let error = view
                .keychain_errors
                .get(&crate::settings::credentials::account_for_slot(pid));
            setting_row(theme, ui_font, &name, error.map(String::as_str), right)
        })
        .collect();
    card_rows(theme, rows)
}

fn provider_key_right(view: &SettingsView, theme: &Theme, provider_id: &str) -> Box<dyn Element> {
    let ui_font = view.ui_font;
    let view_id = view.view_id;
    let has_key = view.app_settings.provider_keys.get(provider_id).is_some();

    let input_el: Box<dyn Element> =
        if let Some(editor) = view.provider_key_editors.get(provider_id) {
            crate::shared_input::shared_input(
                theme,
                editor,
                crate::shared_input::SharedInputOpts {
                    width: crate::shared_input::SharedInputWidth::MinMax(260.0, 320.0),
                    radius: 8.0,
                    pad_left: 10.0,
                    pad_right: 10.0,
                    pad_top: 7.0,
                    pad_bottom: 7.0,
                    ..Default::default()
                },
            )
        } else {
            warpui::elements::Empty::new().finish()
        };

    if has_key {
        let clear_hover = view.hover(&format!("clear_key_{provider_id}"));
        ConstrainedBox::new(
            Flex::row()
                .with_spacing(8.0)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(input_el)
                .with_child(secondary_button(
                    theme,
                    ui_font,
                    view_id,
                    clear_hover,
                    "Clear",
                    SettingsAction::ClearProviderKey(provider_id.to_string()),
                ))
                .finish(),
        )
        .with_max_width(420.0)
        .finish()
    } else {
        ConstrainedBox::new(input_el).with_max_width(320.0).finish()
    }
}

fn provider_display_name(provider_id: &str) -> String {
    match provider_id {
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "google" => "Google (Gemini)".into(),
        "xai" => "xAI (Grok)".into(),
        "groq" => "Groq".into(),
        "cerebras" => "Cerebras".into(),
        "openrouter" => "OpenRouter".into(),
        _ => provider_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str) -> ModelInfo {
        ModelInfo {
            id: format!("{provider}/model"),
            name: "Model".to_string(),
            provider: provider.to_string(),
            context_window: 128_000,
            min_tier: "free".to_string(),
            is_active: true,
        }
    }

    #[test]
    fn provider_key_settings_always_include_core_byok_providers() {
        assert_eq!(
            provider_ids_for_key_settings(&[]),
            ["anthropic", "openai", "google", "xai"]
        );
    }

    #[test]
    fn provider_key_settings_append_active_providers_without_duplicates() {
        let providers = provider_ids_for_key_settings(&[
            model("openai"),
            model("groq"),
            model("groq"),
            model("custom"),
        ]);
        assert_eq!(providers, ["anthropic", "openai", "google", "xai", "groq"]);
    }
}

// ── Picker item ───────────────────────────────────────────────────────────────

fn picker_item(
    theme: &Theme,
    ui_font: FamilyId,
    view_id: EntityId,
    hover: MouseStateHandle,
    label: &str,
    selected: bool,
    action: SettingsAction,
) -> Box<dyn Element> {
    let label_owned = label.to_string();
    Box::new(
        Hoverable::new(hover, move |state| {
            let bg = if selected {
                theme.accent_overlay()
            } else if state.is_hovered() {
                theme.surface_2()
            } else {
                ColorU::new(0, 0, 0, 0)
            };
            let color = if selected {
                theme.accent
            } else {
                theme.main_text()
            };
            let row = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.0,
                        Text::new(label_owned.clone(), ui_font, crate::theme::text_size(13.0))
                            .with_color(color)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(if selected {
                    crate::icons::render_icon(crate::icons::AppIcon::Check, theme.accent, 12.0)
                } else {
                    warpui::elements::Empty::new().finish()
                })
                .finish();
            Container::new(row)
                .with_background_color(bg)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.0)))
                .with_padding_left(8.0)
                .with_padding_right(8.0)
                .with_padding_top(7.0)
                .with_padding_bottom(7.0)
                .finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action_for(view_id, action.clone());
        }),
    )
}
