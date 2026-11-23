//! The built-in agent's models in local mode (a Dev build with no server).
//!
//! Normally the model list comes from the account (the server's catalog).
//! Local mode has no account, so the pickers offer only models the person can
//! call with their own key: the first-party models below whose provider has a
//! key, plus their custom endpoints. Those calls go straight from the app to
//! the provider (`agent::pipeline::step_direct`); nothing needs the server.
//! There is no "Auto": it means Dragent's own models.

use super::account_store::ModelInfo;
use super::app_settings::AppSettings;

/// First-party models callable directly with a provider key, mirroring the
/// server catalog's user-facing entries: (id, name, provider, context window).
const BYOK_CATALOG: &[(&str, &str, &str, i64)] = &[
    ("claude-fable-5", "Claude Fable 5", "anthropic", 1_000_000),
    ("claude-sonnet-5", "Claude Sonnet 5", "anthropic", 1_000_000),
    ("claude-haiku-4-5", "Claude Haiku 4.5", "anthropic", 200_000),
    ("gpt-5.6-sol", "GPT-5.6 Sol", "openai", 400_000),
    ("gpt-5.6-terra", "GPT-5.6 Terra", "openai", 400_000),
    ("gpt-5.6-luna", "GPT-5.6 Luna", "openai", 400_000),
    ("gemini-3.1-pro", "Gemini 3.1 Pro", "google", 1_000_000),
    ("gemini-3.5-flash", "Gemini 3.5 Flash", "google", 1_000_000),
    ("grok-4.5", "Grok 4.5", "xai", 256_000),
    ("deepseek-chat", "DeepSeek Chat", "deepseek", 128_000),
];

/// Models the built-in agent can use in local mode: catalog models whose
/// provider has a key, then the custom endpoints.
pub fn local_mode_models(settings: &AppSettings) -> Vec<ModelInfo> {
    let catalog = BYOK_CATALOG
        .iter()
        .filter(|(id, ..)| settings.has_user_credential_for_model(id))
        .map(|&(id, name, provider, context_window)| ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            provider: provider.to_string(),
            context_window,
            min_tier: "free".to_string(),
            is_active: true,
        });
    let custom = settings
        .custom_models
        .iter()
        .filter(|m| settings.has_user_credential_for_model(&m.model_id))
        .map(|m| ModelInfo {
            id: m.model_id.clone(),
            name: m.name.clone(),
            provider: "custom".to_string(),
            context_window: 128_000,
            min_tier: "free".to_string(),
            is_active: true,
        });
    catalog.chain(custom).collect()
}

/// `(id, name)` pairs for pickers that list names only.
pub fn local_mode_choices(settings: &AppSettings) -> Vec<(String, String)> {
    local_mode_models(settings)
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_only_models_with_the_persons_own_key() {
        let mut settings = AppSettings::default();
        assert!(
            local_mode_choices(&settings).is_empty(),
            "no key, no models"
        );

        settings
            .provider_keys
            .set("anthropic", "sk-ant-test".into());
        let ids: Vec<_> = local_mode_choices(&settings)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(ids.contains(&"claude-sonnet-5".to_string()));
        assert!(ids.iter().all(|id| id.starts_with("claude-")), "{ids:?}");
        assert!(!ids.contains(&super::super::AUTO_MODEL_ID.to_string()));
    }

    /// Every catalog model with a key goes straight to its provider.
    #[test]
    fn every_catalog_model_routes_directly() {
        let mut settings = AppSettings::default();
        for provider in ["anthropic", "openai", "google", "xai", "deepseek"] {
            settings.provider_keys.set(provider, "key".into());
        }
        let models = local_mode_models(&settings);
        assert_eq!(models.len(), BYOK_CATALOG.len());
        for model in models {
            assert!(
                crate::agent::pipeline::step_direct::select_transport(&model.id, &settings)
                    .is_some(),
                "{} must route direct",
                model.id
            );
        }
    }
}
