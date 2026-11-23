use super::onboarding_flow::{SLIDE_COUNT, TASK_COUNT};
use super::settings::UiSettings;
use crate::theme::{Theme, DEFAULT_THEME_ID};

/// Onboarding is shown on first launch (onboarding_completed == false) and
/// skipped on subsequent launches (onboarding_completed == true).
#[test]
fn onboarding_shown_once() {
    let fresh = UiSettings::default();
    assert!(
        !fresh.onboarding_completed,
        "fresh install: wizard must be shown"
    );

    let completed = UiSettings {
        onboarding_completed: true,
        github_cli_setup_completed: true,
        theme_id: "dark".to_string(),
        ..Default::default()
    };
    assert!(
        completed.onboarding_completed,
        "after wizard: workspace shown directly"
    );
}

/// A fresh local-mode start (Dev build, no server) opens the workspace: no
/// onboarding, no sign-in screen. Every other fresh start still shows it.
#[test]
fn local_mode_fresh_start_skips_onboarding() {
    use super::settings::onboarding_needed;
    let fresh = UiSettings::default();
    assert!(!onboarding_needed(fresh.onboarding_completed, true));
    assert!(onboarding_needed(fresh.onboarding_completed, false));
    assert!(!onboarding_needed(true, false));
    assert!(!onboarding_needed(true, true));
}

/// GitHub and GitLab are alternatives, not two mandatory provider connections.
#[test]
fn checklist_structure() {
    assert_eq!(SLIDE_COUNT, 2, "welcome + get set up");
    assert_eq!(TASK_COUNT, 2, "account and the selected Git provider");
}

#[test]
fn gitlab_only_onboarding_persists_without_marking_github_connected() {
    let settings = UiSettings {
        onboarding_completed: true,
        gitlab_cli_setup_completed: true,
        ..Default::default()
    };
    let saved = serde_json::to_string(&settings).unwrap();
    let restored: UiSettings = serde_json::from_str(&saved).unwrap();
    assert!(restored.onboarding_completed);
    assert!(restored.gitlab_cli_setup_completed);
    assert!(!restored.github_cli_setup_completed);
    let mut legacy = serde_json::to_value(&settings).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("gitlab_cli_setup_completed");
    assert!(
        !serde_json::from_value::<UiSettings>(legacy)
            .unwrap()
            .gitlab_cli_setup_completed
    );
}

#[test]
fn gitlab_login_and_verification_do_not_require_github_or_force_gitlab_com() {
    use super::git_provider::GitProvider;
    assert_eq!(GitProvider::Gitlab.cli(), "glab");
    assert_eq!(GitProvider::Gitlab.login_command(), "glab auth login");
    assert_eq!(GitProvider::Gitlab.status_args(), ["auth", "status"]);
    assert!(GitProvider::Github.login_command().contains("github.com"));
}

/// Theme chosen before onboarding must persist across serialization.
#[test]
fn theme_selection_persists() {
    let settings = UiSettings {
        onboarding_completed: true,
        github_cli_setup_completed: true,
        theme_id: "light".to_string(),
        ..Default::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    let restored: UiSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.theme_id, "light");
    assert!(restored.onboarding_completed);
}

/// Theme presets are available and `from_id` resolves each one.
#[test]
fn theme_from_id_round_trips() {
    for preset in Theme::presets() {
        let theme = Theme::from_id(preset.id);
        assert_eq!(theme.id, preset.id);
    }

    let unknown = Theme::from_id("neon");
    assert_eq!(unknown.id, DEFAULT_THEME_ID);
}
