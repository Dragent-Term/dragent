//! Whether something running here may change things in connected
//! services: comment, move items between statuses, file new ones.
//!
//! A person's agent may (the agent's own approval rules ask them first).
//! An automation run's agent only reads, unless its automation allows
//! changes: nobody is watching what it posts under their name. Runs are
//! recognised by their terminal: the session helper that starts every
//! terminal agent Dragent launches is an ancestor process holding the
//! run's pane, and the terminal's id is in its environment (an agent may
//! start its tools with a cleared one, so both are tried).

use dragent_automations::{Run, Store};

/// `Err` with a sentence for the agent when this process belongs to an
/// automation run that may only read.
pub fn check_writes() -> Result<(), String> {
    match automation_run() {
        Some(run) if !run.snapshot.service_writes => Err(read_only_message(&run.snapshot.name)),
        _ => Ok(()),
    }
}

pub(crate) fn read_only_message(automation: &str) -> String {
    format!(
        "Not done: this is a run of the automation “{automation}”, which can only read from \
         connected services. Say in your final reply what you would have changed. (The \
         automation's owner can allow changes under “Connected services” in its settings.)"
    )
}

/// The started automation run this process works for, if any.
fn automation_run() -> Option<Run> {
    let path = dragent_paths::organisation_dir().join("automations.sqlite");
    if !path.is_file() {
        return None;
    }
    let store = Store::open(&path).ok()?;
    if let Some(terminal) = std::env::var("DRAGENT_HOST_TERMINAL_ID")
        .ok()
        .filter(|id| !id.is_empty())
    {
        if let Ok(Some(run)) = store.run_in_terminal(&terminal) {
            return Some(run);
        }
    }
    let pane = helper_pane()?;
    store.run_in_pane(&pane).ok().flatten()
}

/// The pane of the nearest ancestor that is Dragent's session helper.
#[cfg(unix)]
pub(crate) fn helper_pane() -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-ww", "-o", "pid=,ppid=,args="])
        .output()
        .ok()?;
    let table = String::from_utf8_lossy(&output.stdout);
    pane_of_ancestor(&table, std::process::id())
}

#[cfg(not(unix))]
pub(crate) fn helper_pane() -> Option<String> {
    None
}

/// Walk up from `pid` through `ps` rows (`pid ppid args…`) to a process
/// started as `… --dragent-agent-run <pane> …`.
fn pane_of_ancestor(table: &str, pid: u32) -> Option<String> {
    let rows: Vec<(u32, u32, &str)> = table
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, rest) = line.split_once(char::is_whitespace)?;
            let rest = rest.trim_start();
            let (ppid, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            Some((pid.parse().ok()?, ppid.parse().ok()?, args))
        })
        .collect();
    let parent_of = |pid: u32| rows.iter().find(|row| row.0 == pid).map(|row| row.1);
    let mut current = parent_of(pid)?;
    // Bounded: a malformed table must not loop.
    for _ in 0..64 {
        if current <= 1 {
            return None;
        }
        let (_, ppid, args) = rows.iter().find(|row| row.0 == current)?;
        let mut words = args.split_whitespace();
        if words.any(|word| word == dragent_host::agent_sessions::RUN_ARG) {
            if let Some(pane) = words
                .next()
                .filter(|pane| uuid::Uuid::parse_str(pane).is_ok())
            {
                return Some(pane.to_string());
            }
        }
        current = *ppid;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_helper_is_found_up_the_process_tree() {
        let pane = "6f1c2d3e-0000-4000-8000-000000000001";
        let table = format!(
            "    1     0 /sbin/launchd\n  400     1 /bin/zsh -l\n  500   400 /Applications/Dragent.app/Contents/MacOS/dragent --dragent-agent-run {pane} --dragent-organisation=acme -- claude --dangerously-skip-permissions\n  510   500 node /usr/local/bin/claude\n  520   510 dragent --dragent-cli mcp\n  600   400 /bin/zsh\n"
        );
        assert_eq!(pane_of_ancestor(&table, 520).as_deref(), Some(pane));
        assert_eq!(pane_of_ancestor(&table, 600), None);
        assert_eq!(pane_of_ancestor(&table, 999), None);
    }

    #[test]
    fn the_refusal_tells_the_agent_what_to_do_instead() {
        let message = read_only_message("Nightly triage");
        assert!(message.contains("“Nightly triage”") && message.contains("final reply"));
    }
}
