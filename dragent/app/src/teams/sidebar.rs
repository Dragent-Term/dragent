//! What the sidebar needs to group a coordination's workspaces: its lead's
//! session and, for each task, the checkout and session it works in. Read
//! off the main thread with the team overview.

use std::path::PathBuf;

use dragent_host::team::{Team, TaskKind, TaskStatus, TeamRequest, TeamResponse, TeamStatus, TeamSummary};

/// Coordinations looked at per poll: the most recently changed ones.
const MOST: usize = 40;

/// One coordination, as the sidebar groups it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinationGroup {
    pub id: String,
    pub title: String,
    pub status: TeamStatus,
    /// Messages for the person not read yet.
    pub needs_you: usize,
    /// The lead's session (a pane id).
    pub lead_pane: Option<String>,
    pub tasks: Vec<GroupTask>,
}

/// One task of a coordination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupTask {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
    pub review: bool,
    /// The checkout it works in (canonical where it exists here).
    pub workspace: Option<PathBuf>,
    /// Its worker's session (a pane id).
    pub worker_pane: Option<String>,
}

/// The coordinations behind `summaries`, in the same order.
pub fn groups(summaries: &[TeamSummary]) -> Vec<CoordinationGroup> {
    summaries
        .iter()
        .take(MOST)
        .filter_map(|summary| {
            let (machine, raw) = crate::team_source::split(&summary.id);
            let client = crate::team_source::client(&summary.id).ok()?;
            let TeamResponse::Team(team) = client
                .team(TeamRequest::Get { team: raw.to_string() })
                .ok()?
            else {
                return None;
            };
            let team = *team;
            let team = match machine {
                Some(machine) => crate::team_source::display_team(team, machine),
                None => team,
            };
            Some(group_of(&team, summary.needs_user))
        })
        .collect()
}

fn group_of(team: &Team, needs_you: usize) -> CoordinationGroup {
    CoordinationGroup {
        id: team.id.clone(),
        title: team.title.clone(),
        status: team.status,
        needs_you,
        lead_pane: team.lead.as_ref().and_then(|lead| lead.pane.clone()),
        tasks: team
            .tasks
            .iter()
            .map(|task| GroupTask {
                id: task.id.clone(),
                title: task.title.clone(),
                status: task.status,
                review: task.kind == TaskKind::Review,
                workspace: task.workspace.as_deref().map(|path| {
                    let path = PathBuf::from(path);
                    if crate::remote_fs::is_remote(&path) {
                        path
                    } else {
                        path.canonicalize().unwrap_or(path)
                    }
                }),
                worker_pane: task.worker.as_ref().and_then(|worker| worker.pane.clone()),
            })
            .collect(),
    }
}
