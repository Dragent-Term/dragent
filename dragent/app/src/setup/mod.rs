pub(crate) mod agent_command;
pub mod browser_auth_view;
pub mod checklist_slide;
pub mod chrome;
pub mod github_auth_view;
pub mod git_provider;
pub mod intro_slide;
pub mod onboarding_flow;
pub mod settings;

#[cfg(test)]
mod tests;

pub use onboarding_flow::{OnboardingEvent, OnboardingFlow};
pub use settings::{reset_for_onboarding, UiSettings};
