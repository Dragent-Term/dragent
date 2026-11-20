//! Browser sign-in URL builder.

use super::deep_link::CUSTOM_SCHEME;

/// There is one login page, `/login`; it handles the desktop hand-off when a
/// `state` is present and finishes on the dashboard either way.
///
/// Older releases baked `/login/desktop`, so the portal keeps that path as a
/// redirect. Do not remove that redirect until no supported build emits it.
///
/// `challenge` is the PKCE challenge (base64url, URL-safe as it is).
pub fn browser_sign_in_url(state: &str, challenge: &str) -> String {
    let origin = crate::api::web_origin();
    format!("{origin}/login?scheme={CUSTOM_SCHEME}&state={state}&challenge={challenge}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_url_includes_scheme_and_state() {
        let url = browser_sign_in_url("csrf-test", "chal");
        assert!(url.contains("/login?"));
        assert!(
            !url.contains("/login/desktop"),
            "the desktop-only path is retired"
        );
        assert!(url.contains("scheme=dragent")); // CUSTOM_SCHEME
        assert!(url.contains("state=csrf-test"));
        assert!(url.contains("challenge=chal"));
    }
}
