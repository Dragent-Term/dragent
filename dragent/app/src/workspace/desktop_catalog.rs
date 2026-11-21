//! UI-thread collector for the desktop relay catalog.
//!
//! `note_rendered` runs inside `WorkspaceView::render`, so it observes every state
//! change the app actually paints (sign-in, project open, tab open/switch/
//! close, organisation switch) without touching unrelated update paths. It
//! reads the sidebar's resolved metadata in memory — no filesystem reads,
//! network or subprocesses — and hands one snapshot per
//! window to the background publisher, which owns sending.
//!
//! Hierarchy (Project → worktree Workspace(branch) → Tabs), mirroring the
//! desktop sidebar (`sync_left_panel`, `sidebar_project_roots`):
//! - Projects are sidebar project roots: linked worktrees resolve through
//!   their `.git` pointer to the main checkout, so sibling worktrees share
//!   one project. Nothing is guessed by path prefix.
//! - Workspaces are the durable `SerializedWorkspace` records (`id`, `root`,
//!   `title`); project membership and branch come from the sidebar snapshot.
//!   No path resolution is repeated during collection. Trust-hidden projects
//!   are skipped, like their sidebar rows.
//! - Tabs are every pane of every surface owned by a workspace (active and
//!   parked, shown or not), resolved through the workspace's authoritative
//!   `tab_ids` membership. Surfaces with no owning workspace are skipped —
//!   never assigned by array position.
//! - `kind` is `agent` only for an explicit `agent_id` on a terminal pane;
//!   detected-but-unidentified activity stays `terminal`.

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::path::PathBuf;

use warpui::{AppContext, SingletonEntity};

use super::pane::WorkspacePane;
use super::pane_layout::PaneLayout;
use super::view::WorkspaceView;
use super::worktree_status::{pane_agent_status, WorktreeAgentStatus};

/// Per-window contributor key. `WindowId`s are globally unique and never
/// reused, so two windows (or a workspace rebuilt after an organisation
/// switch, which re-notes under the same window) never share a key, and one
/// window's state can never overwrite another's.
pub(crate) fn window_contributor(window: &warpui_core::WindowId) -> String {
    format!("win{window:?}")
}

pub(crate) fn contributor_key(view: &WorkspaceView) -> String {
    window_contributor(&view.window_id)
}

/// Retire a whole window's contribution (called from `RootView`'s drop when
/// its window closes). Other windows' state is untouched.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn note_window_closed(window: warpui_core::WindowId) {
    crate::realtime::desktop_publisher::note_removed(&window_contributor(&window));
}

/// On-screen pane-tabs for one surface: the split layout's panes, or just
/// the active tab when there is no split. Settings/editor chrome never
/// counts. Mirrors the sidebar's visible-set computation.
pub(crate) fn visible_pane_ids(split: Option<&PaneLayout>, active_tab_id: usize) -> HashSet<usize> {
    match split {
        Some(layout) => layout
            .panes()
            .into_iter()
            .filter(|id| *id != super::EDITOR_PANE_ID)
            .collect(),
        None => [active_tab_id].into_iter().collect(),
    }
}

/// Terminal kind from explicit identity only: `agent` requires a stable id,
/// either the agent Dragent started there or a known agent the terminal
/// recognised by its command. Anything unidentified stays `terminal`.
pub(crate) fn tab_kind_of(
    agent_id: Option<&str>,
) -> (crate::realtime::desktop_catalog::TabKind, Option<String>) {
    use crate::realtime::desktop_catalog::TabKind;
    match agent_id.filter(|id| !id.is_empty()) {
        Some(id) => (TabKind::Agent, Some(id.to_string())),
        None => (TabKind::Terminal, None),
    }
}

/// What the agent needs from the person, beside its folded state.
pub(crate) fn map_attention(
    status: &WorktreeAgentStatus,
) -> Option<crate::realtime::desktop_catalog::Attention> {
    use crate::realtime::desktop_catalog::Attention;
    match status {
        WorktreeAgentStatus::Permission => Some(Attention::NeedsInput),
        WorktreeAgentStatus::Failed => Some(Attention::Failed),
        _ => None,
    }
}

/// Dragent runtime fallback, mirroring `effective_pane_status` in
/// `workspace_tab_strip.rs`: the built-in agent runs in
/// `WorkspaceExecutionRuntime` + principal coordination, not in the terminal
/// PTY that `pane_agent_status` reads. Without this, a working Dragent pane
/// reports `idle` with an empty terminal. Terminal signals and CLI hook
/// status win; only an otherwise-idle Dragent pane folds up to working.
pub(crate) fn fold_dragent_status(
    base: WorktreeAgentStatus,
    is_dragent: bool,
    dragent_working: bool,
) -> WorktreeAgentStatus {
    if !matches!(base, WorktreeAgentStatus::Idle) {
        return base;
    }
    if is_dragent && dragent_working {
        return WorktreeAgentStatus::Working {
            label: "Dragent".to_string(),
        };
    }
    base
}

/// Fold the pane lifecycle into the three relay states. Anything needing the
/// user's eyes (finished, failed, or waiting on a decision) is `review`;
/// only a live agent is `working`.
pub(crate) fn map_agent_state(
    status: &WorktreeAgentStatus,
) -> crate::realtime::desktop_catalog::AgentState {
    use crate::realtime::desktop_catalog::AgentState;
    match status {
        WorktreeAgentStatus::Idle => AgentState::Idle,
        WorktreeAgentStatus::Working { .. } => AgentState::Working,
        WorktreeAgentStatus::Review
        | WorktreeAgentStatus::Permission
        | WorktreeAgentStatus::Failed => AgentState::Review,
    }
}

/// Collect this window's catalog snapshot and hand it to the background
/// publisher. Cheap reads only; safe to run every render.
///
/// Native only: the web viewer has no local workspaces to publish.
#[cfg(not(target_family = "wasm"))]
pub(crate) fn note_rendered(view: &WorkspaceView, app: &AppContext) {
    use crate::auth::{AuthManager, AuthStatus};
    use crate::realtime::desktop_catalog::{
        AccountKey, LocalProject, LocalSurface, LocalTab, LocalWorkspace, TabKind,
        WindowContribution,
    };

    let contributor = contributor_key(view);

    // A view that outlived an organisation switch retires itself: its
    // immutable field still names the old context, so its data must never be
    // relabeled under the new account.
    if view.organisation_id != dragent_paths::organisation_id() {
        crate::realtime::desktop_publisher::note_signed_out(&contributor);
        return;
    }
    let user_id = match AuthManager::as_ref(app).status() {
        AuthStatus::LoggedIn(user) => match user.user_id.clone().filter(|id| !id.is_empty()) {
            Some(id) => id,
            // Signed in, identity not known yet (a session refresh that
            // couldn't reach the server keeps the person signed in without
            // it). Noting an empty account here made the phone's owner flip
            // to "" and back, which ended every phone channel each time.
            // Keep what was published until the identity is known again.
            None => return,
        },
        _ => {
            crate::realtime::desktop_publisher::note_signed_out(&contributor);
            return;
        }
    };
    let account = AccountKey::new(user_id, view.organisation_id.clone());

    let mut workspaces = Vec::new();
    let mut surfaces = Vec::new();
    let mut tabs = Vec::new();
    // Reuse the sidebar's already-resolved projection. Resolving roots here
    // would repeat canonicalize/.git reads on every root render.
    let (sidebar_rows, sidebar_projects, sidebar_teams, shown_coordinations) = view.left_panel_view.read(app, |panel, _| {
        let rows: HashMap<usize, (String, Option<String>)> = panel
            .tabs
            .iter()
            .filter_map(|row| {
                Some((
                    row.workspace_id?,
                    (row.project_key.clone()?, row.branch.clone()),
                ))
            })
            .collect();
        let projects: Vec<LocalProject> = panel
            .projects
            .iter()
            .map(|project| LocalProject {
                path: project.key.clone(),
                name: project.label.clone(),
            })
            .collect();
        // Workspaces working for a coordination, as the sidebar groups them.
        let workspace_of: HashMap<usize, usize> = panel
            .tabs
            .iter()
            .filter_map(|row| Some((row.id, row.workspace_id?)))
            .collect();
        let teams: HashMap<usize, crate::realtime::desktop_catalog::LocalTeam> = panel
            .coordination
            .iter()
            .flat_map(|group| {
                group.members.iter().filter_map(|member| {
                    let workspace = *workspace_of.get(&member.tab_id)?;
                    Some((
                        workspace,
                        crate::realtime::desktop_catalog::LocalTeam {
                            id: group.team_id.clone(),
                            title: group.title.clone(),
                            lead: member.lead,
                            task: member.title.clone(),
                        },
                    ))
                })
            })
            .collect();
        // Every coordination the sidebar shows, with the project it sits under.
        let shown: Vec<(String, Option<String>)> = panel
            .coordination
            .iter()
            .map(|group| (group.team_id.clone(), group.project_key.clone()))
            .collect();
        (rows, projects, teams, shown)
    });
    let mut covered_projects: HashSet<String> = HashSet::new();
    // Built-in agent tabs: (window tab id, pane).
    let mut agent_panes: Vec<(u64, uuid::Uuid)> = Vec::new();
    // What the sidebar knows of each agent from its hook reports.
    let sidebar_status: std::collections::HashMap<uuid::Uuid, WorktreeAgentStatus> = view
        .left_panel_view
        .read(app, |panel, _| {
            panel
                .tabs
                .iter()
                .flat_map(|row| row.agents.iter())
                .filter(|agent| agent.activity_known)
                .map(|agent| (agent.pane_id, agent.status.clone()))
                .collect()
        });

    // Authoritative membership: every workspace owns its surfaces through
    // `tab_ids`. Nonactive workspaces and parked surfaces are included; a
    // surface with no owning workspace is skipped, never guessed in.
    for workspace in &view.workspaces {
        let Some((project_root, branch)) = sidebar_rows.get(&workspace.id) else {
            continue;
        };
        let Some(project) = sidebar_projects
            .iter()
            .find(|project| &project.path == project_root)
        else {
            continue;
        };
        covered_projects.insert(project_root.clone());
        workspaces.push(LocalWorkspace {
            team: sidebar_teams.get(&workspace.id).cloned(),
            workspace_id: workspace.id,
            title: workspace.title.clone(),
            root: workspace.root.to_string_lossy().into_owned(),
            branch: branch.clone(),
            project_path: project_root.clone(),
            project_name: project.name.clone(),
        });

        for surface_id in &workspace.tab_ids {
            let shown = *surface_id == view.active_workspace_tab_id;
            let (panes, active_tab, visible): (&[super::view::PaneTab], usize, HashSet<usize>) =
                if shown {
                    (
                        &view.tabs,
                        view.active_tab_id,
                        visible_pane_ids(view.split_tree.as_ref(), view.active_tab_id),
                    )
                } else if let Some(parked) = view
                    .parked_workspace_tabs
                    .iter()
                    .find(|tab| tab.id == *surface_id)
                {
                    // Parked surfaces keep their own active tab; nothing
                    // parked is on screen.
                    (
                        parked.panes.as_slice(),
                        parked.active_tab_id,
                        HashSet::new(),
                    )
                } else {
                    // Closed race: membership references a surface that is
                    // already gone. Skip it; reconciliation prunes it.
                    continue;
                };
            surfaces.push(LocalSurface {
                surface_id: *surface_id,
                workspace_id: workspace.id,
                active_tab_id: active_tab,
                visible_tab_ids: visible.into_iter().collect(),
            });

            // Global to the window (same source `effective_pane_status` uses
            // for the header glow): any active pipeline session or pending
            // principal coordination counts as Dragent working.
            let dragent_working = view.is_dragent_working();
            for tab in panes {
                let (kind, agent_kind) = match &tab.pane {
                    WorkspacePane::Browser(_) => (TabKind::Browser, None),
                    WorkspacePane::Settings(_) => (TabKind::Settings, None),
                    // An agent started by hand in a terminal tab counts too,
                    // once the terminal recognises it (as the sidebar does).
                    WorkspacePane::Terminal(_) => tab_kind_of(
                        tab.pane
                            .agent_id(app)
                            .or_else(|| tab.pane.active_ai_tool(app).map(|tool| tool.cli().to_string()))
                            .as_deref(),
                    ),
                };
                let status = (kind == TabKind::Agent).then(|| {
                    let pane_uuid = tab.pane.pane_uuid(app);
                    // Terminal signals first, CLI hook activity (the same
                    // source the sidebar rows use) second — mirroring
                    // `effective_pane_status` so the relay agrees with the
                    // header. A terminal agent's hooks know about its
                    // permission prompts; its pane alone doesn't.
                    let stop = view.agent_stop_state.get(&pane_uuid);
                    let base = pane_agent_status(&tab.pane, stop, app);
                    let folded = match sidebar_status.get(&pane_uuid).cloned() {
                        Some(cli) => WorktreeAgentStatus::pick_higher(base, cli),
                        None => base,
                    };
                    // Dragent runtime third: the built-in agent works in
                    // `WorkspaceExecutionRuntime` + principal coordination,
                    // not in the terminal PTY. Without this, a working
                    // Dragent pane reports `idle` with an empty terminal.
                    let is_dragent = view.is_dragent_pane(tab, app);
                    fold_dragent_status(folded, is_dragent, dragent_working)
                });
                let agent_state = status.as_ref().map(map_agent_state);
                let attention = status.as_ref().and_then(map_attention);
                // Built-in agent live context for the phone: work title +
                // latest step + recent pipeline entries. Read-only; the phone
                // falls back to PTY bytes when absent. PTY behavior unchanged.
                let (agent_work, agent_step, agent_activity) =
                    if kind == TabKind::Agent {
                        view.dragent_relay_context(tab, app)
                            .map(|ctx| {
                                let activity = if ctx.activity.is_empty() {
                                    None
                                } else {
                                    Some(
                                        ctx.activity
                                            .into_iter()
                                            .map(|entry| {
                                                crate::realtime::desktop_catalog::LocalAgentActivity {
                                                    sequence: entry.sequence,
                                                    kind: entry.kind,
                                                    text: entry.text,
                                                }
                                            })
                                            .collect(),
                                    )
                                };
                                (ctx.work_title, ctx.step, activity)
                            })
                            .unwrap_or((None, None, None))
                    } else {
                        (None, None, None)
                    };
                if kind == TabKind::Agent && view.is_dragent_pane(tab, app) {
                    agent_panes.push((tab.id as u64, tab.pane.pane_uuid(app)));
                }
                tabs.push(LocalTab {
                    local_id: tab.id as u64,
                    surface_id: *surface_id,
                    kind,
                    title: {
                        let title = tab
                            .custom_label
                            .clone()
                            .unwrap_or_else(|| tab.pane.display_title(app));
                        if super::agent_sleep::any_asleep()
                            && super::agent_sleep::is_pane_asleep(tab.pane.pane_uuid(app))
                        {
                            format!("{title} · Sleeping")
                        } else {
                            title
                        }
                    },
                    agent_kind,
                    agent_state,
                    attention,
                    agent_work,
                    agent_step,
                    agent_activity,
                });
            }
        }
    }

    // Opened projects that own no worktree workspace yet stay visible, like
    // their sidebar rows; trust-hidden ones stay hidden with their rows.
    // The coordination allowlist is the full sidebar set (already resolved
    // in memory): covered worktree projects plus standalone opens.
    let allowed_projects: Vec<String> = sidebar_projects
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let standalone_projects = sidebar_projects
        .into_iter()
        .filter(|project| !covered_projects.contains(&project.path))
        .collect();

    let workspace_ids: Vec<usize> = workspaces.iter().map(|w| w.workspace_id).collect();
    let workspace_roots: Vec<String> = workspaces.iter().map(|w| w.root.clone()).collect();
    let host_tab_ids: HashSet<u64> = tabs
        .iter()
        .filter(|tab| matches!(tab.kind, TabKind::Terminal | TabKind::Agent))
        .map(|tab| tab.local_id)
        .collect();
    crate::realtime::desktop_publisher::note_snapshot(WindowContribution {
        contributor: contributor.clone(),
        account: account.clone(),
        workspaces,
        surfaces,
        tabs,
        standalone_projects,
    });
    let routes = view
        .host_owned_tabs
        .iter()
        .filter_map(|(tab_id, tab)| {
            if !host_tab_ids.contains(&(*tab_id as u64)) {
                return None;
            }
            let wire_tab_id = crate::realtime::desktop_publisher::wire_tab_id_for(
                &account,
                &contributor,
                *tab_id as u64,
            )?;
            Some(crate::realtime::companion_host::HostRoute {
                wire_tab_id,
                client: tab.client.as_ref()?.clone(),
                workspace_id: tab.workspace_id.clone(),
                session_id: tab.session_id.as_ref()?.clone(),
                theme: view.theme,
            })
        })
        .collect();
    crate::realtime::companion_host::note_snapshot(contributor.clone(), account.clone(), routes);
    // Sleeping agents have no route: a phone opening one wakes it.
    let sleeping: HashSet<u64> = if super::agent_sleep::any_asleep() {
        view.host_owned_tabs
            .iter()
            .filter(|(tab_id, owned)| {
                host_tab_ids.contains(&(**tab_id as u64))
                    && super::agent_sleep::is_pane_asleep(owned.pane.read(app, |pane, _| pane.pane_uuid))
            })
            .filter_map(|(tab_id, _)| {
                crate::realtime::desktop_publisher::wire_tab_id_for(&account, &contributor, *tab_id as u64)
            })
            .collect()
    } else {
        HashSet::new()
    };
    crate::realtime::companion_host::note_sleeping(&contributor, &account, sleeping);
    // Files and changes a phone may read: under these checkouts.
    crate::realtime::companion_host::note_roots(&contributor, &account, workspace_roots);
    // The built-in agent's tabs: a phone gets their conversation, not a terminal.
    crate::realtime::companion_host::note_agents(&contributor, &account, agent_panes
        .into_iter()
        .filter_map(|(tab_id, pane)| {
            let wire = crate::realtime::desktop_publisher::wire_tab_id_for(&account, &contributor, tab_id)?;
            Some((wire, pane))
        })
        .collect());
    // Which workspaces a phone may open tabs in, by their catalog wire ids.
    if let Some(index) = crate::realtime::desktop_publisher::contributor_index(&account, &contributor) {
        let published = workspace_ids
            .into_iter()
            .map(|id| (crate::realtime::desktop_catalog::workspace_wire_id(index, id), id))
            .collect();
        crate::realtime::companion_host::note_workspaces(&contributor, &account, index, published);
    }
    crate::realtime::companion_coordination::note_projects(
        contributor.clone(),
        account.clone(),
        allowed_projects,
    );
    crate::realtime::companion_coordination::note_teams(&contributor, &account, shown_coordinations);
}

/// Web viewer: no local workspaces to publish.
#[cfg(target_family = "wasm")]
pub(crate) fn note_rendered(_view: &WorkspaceView, _app: &AppContext) {}

// Removal note: `WorkspaceView` already owns its `Drop` (tearing down
// watchers and host sessions in `view_render.rs`, which this worker must not
// touch), so window close retires through `RootView`'s drop
// (`note_window_closed`) instead. Contributions are otherwise retained until
// explicit close/signout: there is no render-based expiry, so an idle open
// desktop keeps publishing its hierarchy on the heartbeat.

#[cfg(test)]
mod tests {
    use super::super::pane_layout::SplitSide;
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn agent_kind_requires_explicit_identity() {
        use crate::realtime::desktop_catalog::TabKind;
        assert_eq!(
            tab_kind_of(Some("claude")),
            (TabKind::Agent, Some("claude".to_string()))
        );
        assert_eq!(tab_kind_of(Some("")), (TabKind::Terminal, None));
        assert_eq!(tab_kind_of(None), (TabKind::Terminal, None));
    }

    #[test]
    fn permission_prompts_and_failures_say_what_they_need() {
        use crate::realtime::desktop_catalog::Attention;
        assert_eq!(
            map_attention(&WorktreeAgentStatus::Permission),
            Some(Attention::NeedsInput)
        );
        assert_eq!(map_attention(&WorktreeAgentStatus::Failed), Some(Attention::Failed));
        assert_eq!(map_attention(&WorktreeAgentStatus::Review), None);
        assert_eq!(map_attention(&WorktreeAgentStatus::Idle), None);
        assert_eq!(
            map_attention(&WorktreeAgentStatus::Working { label: String::new() }),
            None
        );
    }

    #[test]
        fn agent_state_folds_to_review_working_idle() {
        use crate::realtime::desktop_catalog::AgentState;
        assert_eq!(
            map_agent_state(&WorktreeAgentStatus::Idle),
            AgentState::Idle
        );
        assert_eq!(
            map_agent_state(&WorktreeAgentStatus::Working {
                label: "Claude".into()
            }),
            AgentState::Working
        );
        assert_eq!(
            map_agent_state(&WorktreeAgentStatus::Review),
            AgentState::Review
        );
        assert_eq!(
            map_agent_state(&WorktreeAgentStatus::Permission),
            AgentState::Review
        );
        assert_eq!(
            map_agent_state(&WorktreeAgentStatus::Failed),
            AgentState::Review
        );
    }

    #[test]
    fn dragent_runtime_lifts_an_idle_dragent_pane_to_working() {
        use crate::realtime::desktop_catalog::AgentState;
        // Working Dragent pane reports working (the relay bug: it was idle
        // with an empty terminal because only PTY signals were folded).
        let working = fold_dragent_status(WorktreeAgentStatus::Idle, true, true);
        assert_eq!(map_agent_state(&working), AgentState::Working);
        // Idle Dragent without runtime work stays idle.
        assert_eq!(
            fold_dragent_status(WorktreeAgentStatus::Idle, true, false),
            WorktreeAgentStatus::Idle
        );
        // Non-Dragent panes never lift, even while Dragent works elsewhere.
        assert_eq!(
            fold_dragent_status(WorktreeAgentStatus::Idle, false, true),
            WorktreeAgentStatus::Idle
        );
        // Terminal signals win: a permission prompt is never shadowed by the
        // Dragent fallback.
        assert_eq!(
            fold_dragent_status(WorktreeAgentStatus::Permission, true, true),
            WorktreeAgentStatus::Permission
        );
    }

    #[test]
    fn visible_set_follows_split_and_filters_editor_chrome() {
        use super::super::pane_layout::EDITOR_PANE_ID;
        // No split: only the active tab is on screen.
        assert_eq!(
            visible_pane_ids(None, 7),
            HashSet::from([7]),
            "split-less surface shows its active tab"
        );
        // Split: every layout pane counts except the reserved editor pane.
        let mut layout = PaneLayout::new(17);
        assert!(layout.split_pane(17, EDITOR_PANE_ID, SplitSide::Right));
        assert!(layout.split_pane(17, 23, SplitSide::Down));
        let visible = visible_pane_ids(Some(&layout), 17);
        assert!(
            visible.contains(&17) && visible.contains(&23),
            "split panes stay visible: {visible:?}"
        );
        assert!(
            !visible.contains(&EDITOR_PANE_ID),
            "editor chrome is never a tab: {visible:?}"
        );
    }

    static WORKTREE_TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    /// Scratch `main` + two linked worktrees using only pointer files (no
    /// git binary): exercises the real sidebar grouping the collector uses.
    fn linked_worktree_fixture() -> (PathBuf, PathBuf, PathBuf) {
        let seq = WORKTREE_TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!(
            "dragent-catalog-worktree-{}-{seq}",
            std::process::id()
        ));
        let main = base.join("repo");
        let git = main.join(".git");
        std::fs::create_dir_all(git.join("worktrees/wt-a")).unwrap();
        std::fs::create_dir_all(git.join("worktrees/wt-b")).unwrap();
        // Pointer files carry the canonical git dir (temp dirs live under
        // symlinked `/tmp`; the sidebar canonicalizes its side, so a raw
        // `/tmp` spelling would compare unequal to the same checkout).
        let git_canon = git.canonicalize().unwrap();
        for wt in ["wt-a", "wt-b"] {
            let dir = base.join(wt);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(".git"),
                format!("gitdir: {}/worktrees/{wt}", git_canon.display()),
            )
            .unwrap();
        }
        (base.clone(), main, base.join("wt-a"))
    }

    #[test]
    fn sibling_linked_worktrees_share_one_sidebar_project() {
        use super::super::project_naming::{
            belongs_to_project, sidebar_project_label, sidebar_project_root,
        };
        let (base, main, wt_a) = linked_worktree_fixture();
        let wt_b = base.join("wt-b");
        let open = vec![main.clone()];
        // Both linked checkouts resolve through their `.git` pointers to the
        // main checkout: one repo, one sidebar project — the grouping the
        // collector must reproduce instead of prefix-guessing.
        let project_a = sidebar_project_root(&wt_a, &open);
        let project_b = sidebar_project_root(&wt_b, &open);
        assert_eq!(project_a, project_b);
        assert_eq!(project_a, main.canonicalize().unwrap());
        // Compare canonical paths: temp dirs live under symlinked `/tmp`,
        // while the sidebar project root is canonical.
        let wt_a = wt_a.canonicalize().unwrap();
        let wt_b = wt_b.canonicalize().unwrap();
        assert!(belongs_to_project(&wt_a, &project_a));
        assert!(belongs_to_project(&wt_b, &project_a));
        assert!(!belongs_to_project(&base.join("elsewhere"), &project_a));
        assert_eq!(sidebar_project_label(&project_a), "repo");
        std::fs::remove_dir_all(&base).ok();
    }
}
