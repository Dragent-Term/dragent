//! A tab's client follows its host across an update restart: the client
//! it was bound with stops working (new endpoint, new token), and
//! `reconnected` finds the running host, where the terminal lives on.

mod support;

use support::{Harness, wait};

/// A one-liner that greets, then echoes every further stdin line back out,
/// exiting at EOF — the `echo greeting; cat` idiom, for whichever shell
/// `TERMINAL_SHELL` below resolves to.
#[cfg(unix)]
const GREET_THEN_ECHO: &str = "echo before-update; cat";
#[cfg(not(unix))]
const GREET_THEN_ECHO: &str =
    "Write-Output before-update; while ($line = [Console]::ReadLine()) { Write-Output $line }";

#[cfg(unix)]
const TERMINAL_SHELL: Option<&str> = Some("/bin/sh");
#[cfg(not(unix))]
const TERMINAL_SHELL: Option<&str> = None;

#[test]
fn a_bound_client_finds_its_host_again_after_an_update_restart() {
    let mut harness = Harness::new();
    // `reconnected` starts hosts the way the app does; here, this binary.
    // SAFETY: this executable runs this one test.
    unsafe { std::env::set_var("DRAGENT_HOST_EXE", support::helper()) };
    let bound = harness.client.clone();
    // An old tab must never discover a different organisation's host just
    // because the app switched its storage context while it was reconnecting.
    let endpoint = std::env::var_os(dragent_host::ENDPOINT_FILE_ENV).unwrap();
    let foreign = harness.dir.join("other-organisation/endpoint.json");
    unsafe { std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, &foreign); }
    let refused = bound.reconnected();
    unsafe { std::env::set_var(dragent_host::ENDPOINT_FILE_ENV, endpoint); }
    assert!(refused.is_err());
    assert!(!foreign.exists(), "must not start another organisation's host");
    assert!(bound.ping());
    let root = harness.dir.to_str().unwrap();
    let session = bound
        .create_terminal_with_shell(
            &harness.workspace,
            &harness.home,
            Some(GREET_THEN_ECHO),
            Some(root),
            80,
            24,
            TERMINAL_SHELL,
        )
        .unwrap();
    let output = |client: &dragent_host::HostClient| {
        client
            .read_terminal(&harness.workspace, &session.id, None)
            .map(|read| read.0)
            .unwrap_or_default()
    };
    wait(|| output(&bound).contains("before-update").then_some(()));

    // The host stops for an update (its terminals live on in the PTY
    // service); the next request that needs a host starts the new one. The
    // bound client still knows only the old endpoint.
    let old = bound.endpoint().token.clone();
    dragent_host::discovery::restart_for_update(bound.endpoint()).unwrap();
    wait(|| (!bound.ping()).then_some(()));

    let fresh = bound.reconnected().unwrap();
    // Cleanup must address the successor, not the now-dead bound endpoint.
    harness.client = fresh.clone();
    assert_ne!(fresh.endpoint().token, old);
    assert!(fresh.ping());
    // The terminal survived, with its output, and takes input again.
    assert!(output(&fresh).contains("before-update"));
    fresh
        .send_terminal_input(&harness.workspace, &session.id, b"after-update\n")
        .unwrap();
    wait(|| (output(&fresh).matches("after-update").count() >= 2).then_some(()));
}
