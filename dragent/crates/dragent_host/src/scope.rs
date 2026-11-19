//! Which machine a command acts on. Results name it, so an agent never
//! mistakes work on a remote machine for work on this computer, or the other
//! way round.

use serde_json::{json, Value};

/// This computer's name, as its operating system gives it: on macOS the
/// name the person gave it ("MacBook Air"), not the network host name,
/// which is often a bare "Mac".
pub fn machine_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Some(name) = std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|name| !name.is_empty())
        {
            return name;
        }
        sysinfo::System::host_name()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "this computer".into())
    })
    .clone()
}

/// Whether this process is the host Dragent runs on a remote machine over
/// SSH (no window there: no browser panes, pane layout or desktop control).
pub fn is_remote_host() -> bool {
    std::env::var_os("DRAGENT_REMOTE_HOST_BINARY").is_some()
}

/// The machine this process runs on: `kind` is `desktop` where Dragent's
/// window runs, `ssh_host` on a machine Dragent reaches over SSH.
pub fn this_machine() -> Value {
    json!({
        "kind": if is_remote_host() { "ssh_host" } else { "desktop" },
        "name": machine_name(),
        "os": std::env::consts::OS,
    })
}

/// What a machine Dragent reaches over SSH provides, for `machines list`
/// and capabilities: work runs there; the window stays on the desktop.
pub fn ssh_machine_provides() -> Value {
    json!({
        "runs_there": ["terminals", "files", "git", "search", "agents", "team coordination"],
        "through_the_desktop": ["connected services"],
        "desktop_only": ["browser panes", "pane layout", "desktop control", "automations", "workspace setup"],
    })
}
