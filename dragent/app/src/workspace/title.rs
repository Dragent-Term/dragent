pub const DEFAULT_SESSION_TITLE: &str = "New session";

/// Native window title for a tab's base title (project, folder or fallback).
/// Release keeps the base title unchanged. Dev suffixes the edition's app
/// name so Dock and Window-menu entries distinguish dev windows from the
/// installed app ("my-app — Dragent Dev").
pub fn window_title(base: &str) -> String {
    window_title_for(base, dragent_paths::app_name(), dragent_paths::is_dev())
}

/// [`window_title`] over explicit inputs, so both editions are unit-testable:
/// test builds are always the Dev edition (see dragent_paths), so the
/// Release arm can only be checked through this function.
pub fn window_title_for(base: &str, app_name: &str, is_dev: bool) -> String {
    if is_dev {
        format!("{base} — {app_name}")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_window_title_suffixes_the_edition_app_name() {
        // Test builds are always the Dev edition (see dragent_paths).
        assert!(dragent_paths::is_dev());
        // Release keeps "Dragent"; Dev must read "Dragent Dev".
        assert_eq!(dragent_paths::app_name(), "Dragent Dev");
        assert_eq!(window_title("acme"), "acme — Dragent Dev");
        assert_eq!(
            window_title(DEFAULT_SESSION_TITLE),
            format!("{DEFAULT_SESSION_TITLE} — Dragent Dev")
        );
        assert_eq!(
            window_title_for("acme", "Dragent Dev", true),
            "acme — Dragent Dev"
        );
    }

    #[test]
    fn release_window_title_is_unchanged() {
        assert_eq!(window_title_for("acme", "Dragent", false), "acme");
        assert_eq!(
            window_title_for(DEFAULT_SESSION_TITLE, "Dragent", false),
            DEFAULT_SESSION_TITLE
        );
    }
}
