//! Single source of truth for the backend base URL.
//!
//! Resolution order:
//!   1. `DRAGENT_BACKEND_URL` (runtime env)   — dev/ops override, e.g. point a
//!      build at localhost:8080 without a rebuild.
//!   2. `DRAGENT_BACKEND_URL_DEFAULT` (compile time) — baked into release
//!      builds by CI (production URL for main releases, staging for beta).
//!   3. The staging/beta backend — so a plain `cargo run` and beta builds work
//!      against the live environment out of the box.
//!
//! Every server call must go through this — a second hardcoded URL is how a
//! build ends up with auth on one host and streaming on another.
//!
//! A resolved URL that doesn't parse is DISCARDED in favour of the fallback
//! constant, loudly. Reason: a release once shipped with CI having baked
//! `https://gqip22kh3m…awsapprunner.com` — a Unicode ellipsis where
//! `.us-east-1.` belonged, from a URL copied while visually elided. Every
//! request died inside ureq at URL parse ("IdnaError"), which surfaces as a
//! *network* error, so the app read it as "offline" and kept a signed-in-looking
//! UI backed by nothing: sign-in appeared to work, no account ever loaded.
//! An unparseable URL is a permanent, total outage, never a transient blip —
//! so prefer a working default over an address we can't even dial.
//! `build.rs` rejects this at compile time too; this is the belt to that braces,
//! and also covers a typo'd runtime override.

use std::sync::OnceLock;

/// Reject anything we could never successfully dial. Deliberately narrow: an
/// http(s) scheme and an ASCII host with no spaces. Non-ASCII in a host is the
/// signature of a mangled paste (ellipsis, smart quotes, zero-width space).
pub(super) fn is_dialable(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty()
        && host.is_ascii()
        && !host
            .chars()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
}

/// Resolve `runtime_env` → `baked`, skipping any value we can't dial. The URL is
/// always supplied by configuration (env at runtime, or baked at build), so
/// there is no localhost fallback: an unset/mangled value returns empty — an
/// obvious misconfiguration rather than a silent, plausible-but-wrong default.
fn resolve(runtime_env: &str, baked: Option<&str>) -> String {
    let candidates = [
        (runtime_env, std::env::var(runtime_env).ok()),
        (
            "the value baked in at build time",
            baked.map(str::to_string),
        ),
    ];

    let mut saw_undialable = false;
    for (source, candidate) in candidates {
        let Some(candidate) = candidate else { continue };
        let candidate = candidate.trim().trim_end_matches('/');
        if candidate.is_empty() {
            continue;
        }
        if !is_dialable(candidate) {
            log::error!(
                "[config] ignoring {source}: {candidate:?} is not a dialable http(s) URL \
                 (mangled paste?)."
            );
            saw_undialable = true;
            continue;
        }
        return candidate.to_string();
    }

    // A candidate was supplied but unusable (mangled paste, stray ellipsis, …) —
    // that's a permanent outage, never a transient blip, so ship a URL we can
    // at least dial rather than a guaranteed-dead one.
    if saw_undialable {
        log::error!("[config] falling back to http://localhost:8080 for {runtime_env}");
        return "http://localhost:8080".to_string();
    }

    // A missing URL is a real misconfiguration on native. On wasm the web viewer
    // uses the page-injected gateway (backend_url isn't its source of truth), so
    // an empty value is expected — don't raise it as a `console.error` (which
    // trips Next.js's dev error overlay and blanks the page).
    #[cfg(not(target_family = "wasm"))]
    log::error!("[config] no dialable URL configured for {runtime_env}");
    #[cfg(target_family = "wasm")]
    log::debug!("[config] no {runtime_env} set (web viewer uses the injected gateway)");
    String::new()
}

/// Base URL of the Dragent backend, without a trailing slash.
///
/// A Dev edition build ignores the release variables: it uses the server named
/// by `DRAGENT_DEV_SERVER`, or none at all (local mode: empty, never a
/// localhost fallback), so it can't reach a server by inheriting the installed
/// app's environment.
pub fn backend_url() -> String {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if let Some(dev) = dev_choice() {
                return dev.map(|s| s.backend.clone()).unwrap_or_default();
            }
            resolve(
                "DRAGENT_BACKEND_URL",
                option_env!("DRAGENT_BACKEND_URL_DEFAULT"),
            )
        })
        .clone()
}

/// Base URL of the web frontend (browser sign-in, auth landing pages), without
/// a trailing slash. Same resolution pattern as [`backend_url`]:
///   1. `DRAGENT_WEB_ORIGIN` (runtime env — dev/ops override)
///   2. `DRAGENT_WEB_ORIGIN_DEFAULT` (compile time — baked by release CI)
///   3. localhost:3000 (local auth-portal dev server)
pub fn web_origin() -> String {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if let Some(dev) = dev_choice() {
                return dev.map(|s| s.web.clone()).unwrap_or_default();
            }
            resolve(
                "DRAGENT_WEB_ORIGIN",
                option_env!("DRAGENT_WEB_ORIGIN_DEFAULT"),
            )
        })
        .clone()
}

/// The Dragent service this build was compiled for, as `(backend, web
/// origin)`: the addresses release CI bakes in (`DRAGENT_BACKEND_URL_DEFAULT`,
/// `DRAGENT_WEB_ORIGIN_DEFAULT`). `None` for a build without them (any local
/// build). Runtime variables never change it: `dragent service-address`
/// prints it so a Dev build can sign in to the same service as the installed
/// Dragent without the address being written in the source.
pub fn compiled_service_address() -> Option<(String, String)> {
    compiled_service_address_from(
        option_env!("DRAGENT_BACKEND_URL_DEFAULT"),
        option_env!("DRAGENT_WEB_ORIGIN_DEFAULT"),
    )
}

/// [`compiled_service_address`] with the baked values passed in, for tests.
pub(crate) fn compiled_service_address_from(
    backend: Option<&str>,
    web: Option<&str>,
) -> Option<(String, String)> {
    let clean = |v: Option<&str>| {
        v.map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| is_dialable(v))
    };
    Some((clean(backend)?, clean(web)?))
}

/// `Some(server or none)` for a native Dev edition build, `None` when the
/// release resolution applies (Release edition, or the web viewer).
fn dev_choice() -> Option<Option<&'static super::local_mode::DevServer>> {
    if cfg!(target_family = "wasm") || !dragent_paths::is_dev() {
        return None;
    }
    Some(super::local_mode::dev_server())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_service_address_needs_both_dialable_values() {
        assert_eq!(compiled_service_address_from(None, None), None);
        assert_eq!(
            compiled_service_address_from(Some("https://api.example.test"), None),
            None
        );
        assert_eq!(
            compiled_service_address_from(Some("api.example.test"), Some("https://example.test")),
            None
        );
        assert_eq!(
            compiled_service_address_from(
                Some("https://api.example.test/"),
                Some(" https://example.test ")
            ),
            Some((
                "https://api.example.test".to_string(),
                "https://example.test".to_string()
            ))
        );
    }

    #[test]
    fn accepts_normal_urls() {
        // The fallback constant itself must always be dialable — resolve()
        // trusts it as the last resort.
        assert!(is_dialable("http://localhost:8080"));
        assert!(is_dialable("https://api.example.test/v1"));
        // Scheme-less hosts stay rejected — that's the whole guard.
        assert!(!is_dialable("localhost:8080"));
    }

    /// The exact string CI baked into the broken release.
    #[test]
    fn rejects_the_ellipsis_url_that_shipped() {
        assert!(!is_dialable("https://gqip22kh3m\u{2026}awsapprunner.com"));
    }

    #[test]
    fn rejects_mangled_and_schemeless_hosts() {
        assert!(!is_dialable("https://exam ple.com")); // space
        assert!(!is_dialable("https://exa\u{200b}mple.com")); // zero-width space
        assert!(!is_dialable("gqip22kh3m.us-east-1.awsapprunner.com")); // no scheme
        assert!(!is_dialable("https://")); // no host
        assert!(!is_dialable("ftp://example.com")); // wrong scheme
    }

    #[test]
    fn unusable_baked_value_falls_back_instead_of_shipping_a_dead_url() {
        let resolved = resolve(
            "DRAGENT_BACKEND_URL_UNSET_IN_TESTS",
            Some("https://gqip22kh3m\u{2026}awsapprunner.com"),
        );
        assert_eq!(resolved, "http://localhost:8080");
    }

    #[test]
    fn good_baked_value_wins_and_loses_its_trailing_slash() {
        let resolved = resolve(
            "DRAGENT_BACKEND_URL_UNSET_IN_TESTS",
            Some("https://api.example.com/"),
        );
        assert_eq!(resolved, "https://api.example.com");
    }
}
