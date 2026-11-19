//! Machine-scoped coordination identities. Display aliases never leave the
//! desktop; requests always use the owning host's IDs and native paths.
use dragent_host::{
    HostClient,
    team::{Team, TeamRequest, TeamResponse, TeamSummary},
};
use serde_json::{Value, json};
use std::path::Path;

pub fn scoped(machine: &str, team: &str) -> String {
    format!("ssh/{machine}/{team}")
}
pub fn split(id: &str) -> (Option<&str>, &str) {
    match id.strip_prefix("ssh/") {
        Some(rest) => {
            let (machine, team) = rest.split_once('/').unwrap_or(("", rest));
            (Some(machine), team)
        }
        None => (None, id),
    }
}
pub fn client(id: &str) -> Result<HostClient, String> {
    match split(id).0 {
        Some(machine) => crate::remote_fs::client(machine),
        None => HostClient::connect_existing(),
    }
}
pub fn call(tool: &str, args: &Value, caller: &crate::cli::team::Caller) -> Result<Value, String> {
    let id = args["team"].as_str().unwrap_or_default();
    match split(id) {
        (Some(machine), raw) => {
            let mut args = args.clone();
            args["team"] = json!(raw);
            crate::remote_fs::client(machine)?.coordination(
                tool,
                args,
                serde_json::to_value(caller).map_err(|e| e.to_string())?,
            )
        }
        _ => crate::cli::team::call(tool, args, caller),
    }
}
pub fn list() -> Result<Vec<TeamSummary>, String> {
    let mut all = Vec::new();
    let mut sources = vec![(None, HostClient::connect_existing()?)];
    sources.extend(
        crate::remote_fs::connected_machines()
            .into_iter()
            .filter_map(|id| crate::remote_fs::client(&id).ok().map(|c| (Some(id), c))),
    );
    for (machine, client) in sources {
        match client.team(TeamRequest::List { project_root: None }) {
            Ok(TeamResponse::Teams(mut teams)) => {
                if let Some(machine) = machine {
                    for team in &mut teams {
                        team.id = scoped(&machine, &team.id);
                        team.project_root = crate::remote_fs::alias(&machine, &team.project_root)
                            .to_string_lossy()
                            .into_owned();
                    }
                }
                all.extend(teams);
            }
            Err(error) if machine.is_none() => return Err(error),
            _ => {}
        }
    }
    all.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    Ok(all)
}
pub fn display_team(mut team: Team, machine: &str) -> Team {
    team.id = scoped(machine, &team.id);
    team.project_root = crate::remote_fs::alias(machine, &team.project_root)
        .to_string_lossy()
        .into_owned();
    for task in &mut team.tasks {
        if let Some(path) = task.workspace.as_mut() {
            *path = crate::remote_fs::alias(machine, path)
                .to_string_lossy()
                .into_owned();
        }
    }
    team
}
pub fn native_team(team: &Team) -> Team {
    let mut native = team.clone();
    if let (Some(machine), id) = split(&team.id) {
        native.id = id.into();
        if let Some(root) = crate::remote_fs::resolve(Path::new(&team.project_root))
            .filter(|root| root.machine_id == machine)
        {
            native.project_root = root.path;
        }
        for task in &mut native.tasks {
            if let Some(path) = task.workspace.as_mut() {
                if let Some(remote) = crate::remote_fs::resolve(Path::new(path))
                    .filter(|remote| remote.machine_id == machine)
                {
                    *path = remote.path;
                }
            }
        }
    }
    native
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_team_ids_on_different_machines_are_distinct() {
        let a = scoped("build-a", "team-12345678");
        let b = scoped("build-b", "team-12345678");
        assert_ne!(a, b);
        assert_eq!(split(&a), (Some("build-a"), "team-12345678"));
        assert_eq!(split("team-12345678"), (None, "team-12345678"));
        assert!(client("ssh/disconnected-test-machine/team-12345678").is_err());
        assert_eq!(split("ssh/broken"), (Some(""), "broken"));
        assert!(client("ssh/broken").is_err());
    }
    #[test]
    fn display_aliases_round_trip_without_changing_host_paths() {
        let native: Team = serde_json::from_value(json!({
            "id":"team-12345678","title":"Build","project_root":"/home/dev/repo","status":"running",
            "created_at_ms":0,"updated_at_ms":0,
            "tasks":[{"id":"t1","title":"Task","prompt":"Build","kind":"work","status":"running",
                "workspace":"/home/dev/worktrees/task","created_at_ms":0,"updated_at_ms":0}]
        }))
        .unwrap();
        let display = display_team(native.clone(), "build-box");
        assert!(crate::remote_fs::is_remote(Path::new(
            &display.project_root
        )));
        assert!(display.id.starts_with("ssh/build-box/"));
        let restored = native_team(&display);
        assert_eq!(restored.id, native.id);
        assert_eq!(restored.project_root, native.project_root);
        assert_eq!(restored.tasks, native.tasks);
        let prompt = crate::cli::team::lead_prompt(&restored, "Build it");
        assert!(!prompt.contains("ssh/build-box"));
        assert!(
            !prompt.contains(
                &crate::remote_fs::mounts_root()
                    .to_string_lossy()
                    .to_string()
            )
        );
    }
}
