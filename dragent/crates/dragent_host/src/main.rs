//! `dragent-host` service process entry point.

use dragent_host::{
    protocol::{PTY_SERVICE_ARG, SERVICE_ARG},
    server,
};

// Experimental allocator swap, off by default (`--features mimalloc`).
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // Inherited daemon hand-off variables are only for the PTY daemon its
    // predecessor or supervisor starts (see the app's `main`).
    if !std::env::args().any(|a| a == PTY_SERVICE_ARG) {
        dragent_paths::strip_inherited_vars(&[&dragent_paths::PTY_HANDOFF_VARS]);
    }
    dragent_host::capture_build_identity();
    dragent_paths::select_organisation_from_process_args();
    if dragent_host::remote::run_askpass() { return; }
    if std::env::args().any(|a| a == "--dragent-stop-host") {
        if let Ok(client) = dragent_host::HostClient::connect_existing() { let _ = client.shutdown(); }
        return;
    }
    if std::env::args().any(|a| a == "--dragent-print-host-endpoint") {
        match dragent_host::HostClient::ensure_host() {
            Ok(client) => { println!("{}", serde_json::to_string(client.endpoint()).unwrap()); return; }
            Err(error) => { eprintln!("{error}"); std::process::exit(1); }
        }
    }
    #[cfg(not(target_family = "wasm"))]
    if let Some(code) = dragent_host::agent_sessions::run_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = dragent_host::team_runtime::run_if_requested() { std::process::exit(code); }
    // Services from here on: a terminal's identity is for commands run in it.
    dragent_paths::strip_inherited_vars(&[&dragent_paths::HOST_TERMINAL_VARS]);
    let _ = env_logger::try_init();
    let args: Vec<String> = std::env::args().collect();
    // PTY daemon branch comes first: the supervised child runs the same
    // binary with its own flag.
    #[cfg(not(target_family = "wasm"))]
    if args.iter().any(|a| a == PTY_SERVICE_ARG) {
        if let Err(error) = dragent_host::pty_service::run_service() {
            eprintln!("dragent-pty failed: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    if !args.iter().any(|a| a == SERVICE_ARG) {
        eprintln!("usage: dragent-host {SERVICE_ARG}");
        eprintln!("(the UI spawns this automatically; see dragent_host::client)");
        std::process::exit(2);
    }
    if let Err(error) = dragent_paths::migrate_legacy_storage() {
        eprintln!("dragent-host storage migration failed: {error}");
        std::process::exit(1);
    }
    if let Err(error) = dragent_host::team_runtime::install_cli() { eprintln!("Coordination CLI setup failed: {error}"); }
    if let Err(error) = server::run_service() {
        eprintln!("dragent-host failed: {error:#}");
        std::process::exit(1);
    }
}
