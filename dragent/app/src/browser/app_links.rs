//! Links a page hands to another app (`mailto:`, `zoom:`, `slack:`…).
//! The pane never opens them by itself: it asks, and only a click on Open
//! passes the link to the system.

/// Schemes that are never handed to another app: the engine's own pages,
/// script, local files, other browsers' internal pages, and our own
/// deep-link schemes (both editions: a page must not fire the other
/// install's links at the OS).
const NEVER: &[&str] = &[
    "http", "https", "about", "data", "blob", "javascript", "vbscript", "file", "filesystem",
    "view-source", "chrome", "chrome-extension", "edge", "brave", "opera", "safari", "webkit",
    "moz-extension", "resource", "ws", "wss", "dragent", "dragent-dev",
];

/// The scheme of a link meant for another app, lowercased, or `None` when
/// the link is not one.
pub fn scheme(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once(':')?;
    let valid = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        && scheme.len() <= 40;
    let scheme = scheme.to_ascii_lowercase();
    (valid && !rest.is_empty() && !NEVER.contains(&scheme.as_str())).then_some(scheme)
}

/// "Open this mailto: link in another app?"-style wording for the prompt.
pub fn prompt(scheme: &str) -> String {
    match scheme {
        "mailto" => "This page wants to open your email app.".into(),
        "tel" | "facetime" | "facetime-audio" => "This page wants to start a call in another app.".into(),
        "sms" => "This page wants to open your messages app.".into(),
        _ => format!("This page wants to open a {scheme}: link in another app."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_links_are_recognised_and_browser_internals_never_are() {
        assert_eq!(scheme("mailto:a@example.com").as_deref(), Some("mailto"));
        assert_eq!(scheme("ZOOMUS://zoom.us/join?confno=1").as_deref(), Some("zoomus"));
        assert_eq!(scheme("vscode://file/x").as_deref(), Some("vscode"));
        for never in [
            "https://example.com", "javascript:alert(1)", "file:///etc/passwd",
            "chrome://settings", "data:text/html,x", "about:blank", "mailto:", "relative/path",
            "1bad:scheme", "",
            // Our own deep links (both editions) never leave the app this way.
            "dragent://join?session=a&secret=b",
            "dragent-dev://join?session=a&secret=b",
            "dragent://auth/callback?refresh_token=rt&state=s",
            "dragent-dev://auth/callback?refresh_token=rt&state=s",
        ] {
            assert_eq!(scheme(never), None, "{never}");
        }
    }

    #[test]
    fn prompts_name_what_will_open() {
        assert_eq!(prompt("mailto"), "This page wants to open your email app.");
        assert_eq!(prompt("slack"), "This page wants to open a slack: link in another app.");
    }
}
