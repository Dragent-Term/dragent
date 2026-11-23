// Desktop binary entry. All app logic lives in the `dragent` lib (so the WASM
// web viewer can run the same views); this just launches it on native.
//
// On Windows, build a GUI-subsystem executable so launching from Explorer or the
// taskbar doesn't open a console window alongside the app.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    // Started from a Dragent terminal, this process inherits that terminal's
    // daemon hand-off variables. Only the PTY daemon, which its predecessor
    // or supervisor starts with them, may act on them; anyone else would
    // adopt another daemon's listener (and hand it on to the services it
    // starts). Runs first: nothing else may run while the environment changes.
    #[cfg(not(target_family = "wasm"))]
    if !std::env::args().any(|a| a == dragent_host::protocol::PTY_SERVICE_ARG) {
        dragent_paths::strip_inherited_vars(&[&dragent_paths::PTY_HANDOFF_VARS]);
    }
    dragent_host::capture_build_identity();
    // Services spawned by the app name their organisation explicitly; the
    // desktop app itself selects one from the signed-in user's record.
    #[cfg(not(target_family = "wasm"))]
    dragent_paths::select_organisation_from_process_args();
    #[cfg(not(target_family = "wasm"))]
    if dragent_host::remote::run_askpass() {
        return Ok(());
    }
    // Computer use's helper: the app spawns its own executable with this flag
    // and talks to it over stdin/stdout. No window, no app state.
    #[cfg(not(target_family = "wasm"))]
    if std::env::args().any(|a| a == dragent_computer::SERVICE_ARG) {
        dragent_computer::serve();
        return Ok(());
    }
    #[cfg(not(target_family = "wasm"))]
    if let Some(code) = dragent_host::agent_sessions::run_if_requested() {
        std::process::exit(code);
    }
    // `dragent …` commands (the launchers in Dragent's terminals and
    // ~/.dragent/bin), never the app itself.
    #[cfg(not(target_family = "wasm"))]
    if let Some(code) = dragent::cli::run_if_requested() {
        std::process::exit(code);
    }
    // What remains is the app itself and its background services. The
    // terminal identity above was for the `dragent` command and agent
    // wrappers; carried on, it would tag everything the app or its services
    // start with a terminal of whichever app launched this one. Still before
    // any thread: the arms above that start threads never return here.
    #[cfg(not(target_family = "wasm"))]
    dragent_paths::strip_inherited_vars(&[&dragent_paths::HOST_TERMINAL_VARS]);
    runtime_metrics::init();
    // Dev edition: copy the person's preferences from the release folder once,
    // before settings are first read. Release builds run none of this. Legacy
    // migration below is already a no-op in Dev builds.
    #[cfg(not(target_family = "wasm"))]
    dragent::dev_settings_copy::maybe_copy_release_settings();
    #[cfg(not(target_family = "wasm"))]
    dragent_paths::migrate_legacy_storage()?;
    #[cfg(all(not(target_family = "wasm"), feature = "local-index"))]
    if dragent_index_service::run_if_requested()? {
        return Ok(());
    }
    #[cfg(not(target_family = "wasm"))]
    if std::env::args().any(|a| a == dragent_host::protocol::SERVICE_ARG) {
        let _ = env_logger::try_init();
        return dragent_host::server::run_service();
    }
    // Supervised PTY daemon child: the host supervisor spawns its own
    // executable with this flag. Without this arm the flag falls through
    // to the full UI boot — a phantom window that never writes the pty
    // manifest, so every `create_terminal` times out as "pty daemon
    // unavailable" while tabs silently downgrade to in-process shells.
    #[cfg(not(target_family = "wasm"))]
    if std::env::args().any(|a| a == dragent_host::protocol::PTY_SERVICE_ARG) {
        let _ = env_logger::try_init();
        return dragent_host::pty_service::run_service().map_err(|e| anyhow::anyhow!("{e:#}"));
    }
    dragent::run()
}
