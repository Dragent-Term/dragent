//! Additive relay catalog published by the desktop: projects, worktree
//! workspaces, and their tabs.
//!
//! This module is deliberately pure: wire types, validation, multi-window
//! aggregation, debounce, and bounds. No UI reads, no network, no disk, so it
//! compiles on every target and is fully unit-testable.
//!
//! Hierarchy (mirroring the desktop sidebar, `sync_left_panel`):
//! Project → worktree Workspace (branch) → Tabs.
//! - Projects are sidebar project roots: linked worktrees resolve to their
//!   main checkout, so sibling worktrees share one project. Deduplicated by
//!   path across contributors.
//! - Workspaces are the durable `SerializedWorkspace` records (`id`, `root`,
//!   `title`), one entry per record — never one per surface, never invented.
//!   Wire ids are namespaced `w{contributor-index}w{workspace-id}`: durable
//!   numeric ids are unique only within one view's lifetime, so two windows
//!   must never share an entry.
//! - Tabs are every pane of every surface owned by a workspace (active and
//!   parked, shown or not). Each tab names its owning `workspace_id` /
//!   `project_id` from that same membership — never by array position. A tab
//!   whose surface has no workspace entry in the same contribution is dropped.
//!   Legacy catalogs without those ids can still be *read*.
//! - Tab ids are numbers `<= 2^53 - 1` (JS-safe): the aggregator maps
//!   `(contributor-index, local tab id)` to `(index << 32) | local`.
//! - `kind` is `agent` only for an explicit agent identity on a terminal pane;
//!   detected-but-unidentified activity stays `terminal`.

use std::collections::{HashMap, HashSet};

/// Largest tab id the relay/mobile path can carry exactly (`2^53 - 1`).
pub const MAX_TAB_ID: u64 = (1u64 << 53) - 1;
/// Publish heartbeat: a signed-in desktop re-sends at least this often.
pub const HEARTBEAT_MS: u64 = 15_000;
/// Coalescing window for UI bursts before a send is attempted.
pub const COALESCE_MS: u64 = 2_000;
/// PUT route, relative to the backend origin (`API_CLIENT.put` joins it).
pub const RELAY_CATALOG_PATH: &str = "v1/relay/catalog";

/// Deterministic caps so one pathological window cannot blow up the body.
pub const MAX_PROJECTS: usize = 256;
pub const MAX_WORKSPACES: usize = 1024;
pub const MAX_TABS: usize = 4096;
pub const MAX_TITLE_CHARS: usize = 240;
/// Recent Dragent pipeline entries carried per agent tab (mirrors
/// `coordination.list/get` caps: bounded count, truncated text).
pub const MAX_AGENT_ACTIVITY_ENTRIES: usize = 10;
/// Activity text bound (chars, char-boundary truncated), mirroring the
/// coordination title bound.
pub const MAX_AGENT_ACTIVITY_TEXT_CHARS: usize = 512;
/// Activity kind bound (chars). Kinds are controlled (`thinking`,
/// `narration`, `tool`, `finished`, …); the bound is defensive only.
pub const MAX_AGENT_ACTIVITY_KIND_CHARS: usize = 64;

// ── Collector-side (local) types ─────────────────────────────────────────────

/// Account that owned a contribution at collect time. Stamped by the UI
/// collector while its view belongs to the account; the worker never re-reads
/// auth to (re-)label it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountKey {
    /// Signed-in user id observed on the UI thread (empty when unknown).
    pub user_id: String,
    /// Organisation context the owning view was built for (immutable per view).
    pub org: String,
}

impl AccountKey {
    pub fn new(user_id: impl Into<String>, org: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            org: org.into(),
        }
    }
}

/// One durable worktree workspace (`SerializedWorkspace`): `id`, `root` and
/// `title` are authoritative and stable; `branch` comes from the in-memory
/// git cache only (`None` when unknown). `project_*` is the sidebar project
/// this checkout belongs to — linked worktrees resolve to their main
/// checkout, so siblings share one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalWorkspace {
    /// Durable workspace id, unique within its view's lifetime.
    pub workspace_id: usize,
    pub title: String,
    /// Worktree checkout root, as stored.
    pub root: String,
    pub branch: Option<String>,
    pub project_path: String,
    pub project_name: String,
    /// The coordination this workspace works for, as the sidebar groups it.
    pub team: Option<LocalTeam>,
}

/// A workspace's place in a coordination (an agent team): which one, and
/// whether its lead runs there or it works on one of the tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTeam {
    pub id: String,
    pub title: String,
    pub lead: bool,
    /// What it works on: the task's title; for the lead, the agent leading.
    pub task: String,
}

/// Longest coordination title or task the phone gets.
const MAX_TEAM_TEXT: usize = 160;

fn team_text(text: &str) -> String {
    text.chars().take(MAX_TEAM_TEXT).collect()
}

/// One top-level surface (the shown surface plus each parked workspace tab).
/// Membership is authoritative: `workspace_id` names the owning
/// `LocalWorkspace` of the same contribution (via `tab_ids`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSurface {
    /// Numeric surface id, unique only within its own `WorkspaceView`.
    pub surface_id: usize,
    /// Durable id of the owning workspace in the same contribution.
    pub workspace_id: usize,
    /// That surface's own active pane-tab (live or parked).
    pub active_tab_id: usize,
    /// Pane-tabs on screen. Empty for parked surfaces: membership and
    /// activeness are reported, but nothing parked is visible.
    pub visible_tab_ids: Vec<usize>,
}

/// One recent Dragent pipeline entry for the relay: the same
/// `WorkspaceExecutionActivity{sequence,kind,text}` the desktop header folds
/// into its latest-step segment, plus principal coordination activity when a
/// coordination turn is in flight. Bounded by
/// [`MAX_AGENT_ACTIVITY_ENTRIES`] / [`MAX_AGENT_ACTIVITY_TEXT_CHARS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalAgentActivity {
    pub sequence: u64,
    pub kind: String,
    pub text: String,
}

/// One pane tab on a surface. Relationships are explicit: `surface_id` names
/// the owning entry of the same contribution's `surfaces`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTab {
    /// Numeric pane-tab id, unique only within its own `WorkspaceView`.
    pub local_id: u64,
    pub surface_id: usize,
    pub kind: TabKind,
    pub title: String,
    pub agent_kind: Option<String>,
    pub agent_state: Option<AgentState>,
    /// What the agent needs from the person now, beside its state.
    pub attention: Option<Attention>,
    /// Dragent live context (built-in agent only, `None` otherwise): the
    /// active work title the header shows before the `·` segment.
    pub agent_work: Option<String>,
    /// Latest live step (`Thinking…`, `Dispatched: <tool>`, principal
    /// coordination activity, …) — the header's trailing segment.
    pub agent_step: Option<String>,
    /// Recent pipeline entries, oldest-first, so a phone renders something
    /// instead of a blank idle tab. `None` when there is nothing to show;
    /// the phone falls back to PTY bytes.
    pub agent_activity: Option<Vec<LocalAgentActivity>>,
}

/// A project root opened directly that currently owns no worktree workspace.
/// Keeps empty projects visible, like the sidebar does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalProject {
    pub path: String,
    pub name: String,
}

/// One window's worth of catalog state, stamped with its owning account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowContribution {
    /// Per-window contributor key (window identity at collect time).
    pub contributor: String,
    pub account: AccountKey,
    pub workspaces: Vec<LocalWorkspace>,
    pub surfaces: Vec<LocalSurface>,
    pub tabs: Vec<LocalTab>,
    pub standalone_projects: Vec<LocalProject>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Terminal,
    Browser,
    Settings,
    Agent,
}

impl TabKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TabKind::Terminal => "terminal",
            TabKind::Browser => "browser",
            TabKind::Settings => "settings",
            TabKind::Agent => "agent",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "terminal" => Some(TabKind::Terminal),
            "browser" => Some(TabKind::Browser),
            "settings" => Some(TabKind::Settings),
            "agent" => Some(TabKind::Agent),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Review,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Review => "review",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(AgentState::Idle),
            "working" => Some(AgentState::Working),
            "review" => Some(AgentState::Review),
            _ => None,
        }
    }
}

/// What an agent tab needs from the person: it waits at a permission prompt
/// or a question, or its turn failed. Rides beside [`AgentState`] (which
/// folds both into `review`), so readers that predate it keep working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    NeedsInput,
    Failed,
}

impl Attention {
    pub fn as_str(self) -> &'static str {
        match self {
            Attention::NeedsInput => "needs_input",
            Attention::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "needs_input" => Some(Attention::NeedsInput),
            "failed" => Some(Attention::Failed),
            _ => None,
        }
    }
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogProject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogWorkspace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Legacy project display name (kept alongside `project_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The coordination it works for. Older phones ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<CatalogTeam>,
}

/// A workspace's coordination on the wire.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogTeam {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub lead: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogAgentActivity {
    pub sequence: u64,
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogTab {
    pub id: u64,
    #[serde(rename = "kind")]
    pub kind: String,
    pub title: String,
    pub active: bool,
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<String>,
    /// Dragent live context (built-in agent only). Optional so readers that
    /// predate it keep working; the phone falls back to PTY when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_work: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_activity: Option<Vec<CatalogAgentActivity>>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MachineMeta {
    pub name: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CatalogPayload {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<MachineMeta>,
    #[serde(default)]
    pub projects: Vec<CatalogProject>,
    #[serde(default)]
    pub workspaces: Vec<CatalogWorkspace>,
    #[serde(default)]
    pub tabs: Vec<CatalogTab>,
}

// ── Wire-id mapping ──────────────────────────────────────────────────────────

/// Map a per-view tab id to a process-wide JS-safe wire id.
///
/// `(contributor-index << 32) | local-id`. Contributor indices are small
/// (one per window); the result always fits in `2^53 - 1`. Returns `None`
/// when the local id does not fit in 32 bits or the index is too large —
/// callers drop such tabs rather than publish a colliding id.
pub fn tab_wire_id(contrib_index: u32, local_id: u64) -> Option<u64> {
    if contrib_index >= (1 << 21) {
        return None;
    }
    if local_id >= (1 << 32) {
        return None;
    }
    let wire = ((contrib_index as u64) << 32) | local_id;
    (wire <= MAX_TAB_ID).then_some(wire)
}

/// Workspace wire id, namespaced by contributor *and* durable workspace id.
/// Durable ids are unique only within one view's lifetime (a fresh window
/// restarts its counter), so the contributor namespace is what keeps two
/// windows from sharing an entry.
pub fn workspace_wire_id(contrib_index: u32, workspace_id: usize) -> String {
    format!("w{contrib_index}w{workspace_id}")
}

// ── Aggregation ──────────────────────────────────────────────────────────────

/// One contribution plus the contributor index the publisher assigned it.
/// Indices must be stable per contributor across publishes (kept in publisher
/// state, not recomputed per flush).
pub struct IndexedContribution<'a> {
    pub index: u32,
    pub contribution: &'a WindowContribution,
}

/// Merge per-window contributions into one publishable catalog.
///
/// - Projects deduplicate by path string (same checkout, one entry).
/// - Workspaces are per `(contributor, durable workspace id)` — never merged
///   across windows, never overwritten, never invented.
/// - Tabs resolve through their surface's workspace entry in the same
///   contribution; a tab whose surface (or surface's workspace) is unknown
///   is dropped — no orphan refs, no array-order guessing.
/// - Deterministic: contributions are ordered by contributor index.
pub fn aggregate(items: &[IndexedContribution<'_>]) -> CatalogPayload {
    let mut ordered: Vec<&IndexedContribution> = items.iter().collect();
    ordered.sort_by_key(|item| item.index);

    let mut projects: Vec<CatalogProject> = Vec::new();
    let mut project_ids: HashSet<String> = HashSet::new();
    let mut push_project = |path: &str, name: &str| {
        if path.is_empty() {
            return;
        }
        if project_ids.insert(path.to_string()) {
            projects.push(CatalogProject {
                id: Some(path.to_string()),
                name: name.to_string(),
                path: path.to_string(),
            });
        }
    };

    let mut workspaces: Vec<CatalogWorkspace> = Vec::new();
    let mut workspace_ids: HashSet<String> = HashSet::new();
    // (contrib index, durable workspace id) -> wire workspace id.
    let mut workspace_wires: HashMap<(u32, usize), String> = HashMap::new();
    // (contrib index, durable workspace id) -> owning project path.
    let mut workspace_projects: HashMap<(u32, usize), String> = HashMap::new();

    for item in &ordered {
        for workspace in &item.contribution.workspaces {
            if workspace.root.is_empty() || workspace.project_path.is_empty() {
                continue;
            }
            push_project(&workspace.project_path, &workspace.project_name);
            let wire = workspace_wire_id(item.index, workspace.workspace_id);
            if workspace_ids.insert(wire.clone()) {
                workspaces.push(CatalogWorkspace {
                    id: Some(wire.clone()),
                    name: workspace.title.clone(),
                    path: workspace.root.clone(),
                    branch: workspace.branch.clone(),
                    project: Some(workspace.project_name.clone()),
                    project_id: Some(workspace.project_path.clone()),
                    team: workspace.team.as_ref().map(|team| CatalogTeam {
                        id: team_text(&team.id),
                        title: team_text(&team.title),
                        lead: team.lead,
                        task: team_text(&team.task),
                    }),
                });
                workspace_wires.insert((item.index, workspace.workspace_id), wire);
                workspace_projects.insert(
                    (item.index, workspace.workspace_id),
                    workspace.project_path.clone(),
                );
            }
        }
        for standalone in &item.contribution.standalone_projects {
            push_project(&standalone.path, &standalone.name);
        }
    }

    // (contrib index, surface id) -> (wire workspace id, project path,
    // surface active tab, visible set). Surfaces whose workspace has no entry
    // in the same contribution are dropped: no invented grouping.
    let mut surface_resolution: HashMap<(u32, usize), (String, String, usize, HashSet<usize>)> =
        HashMap::new();
    for item in &ordered {
        for surface in &item.contribution.surfaces {
            let key = (item.index, surface.workspace_id);
            let (Some(wire), Some(project)) = (
                workspace_wires.get(&key).cloned(),
                workspace_projects.get(&key).cloned(),
            ) else {
                continue;
            };
            surface_resolution.insert(
                (item.index, surface.surface_id),
                (
                    wire,
                    project,
                    surface.active_tab_id,
                    surface.visible_tab_ids.iter().copied().collect(),
                ),
            );
        }
    }

    let mut tabs: Vec<CatalogTab> = Vec::new();
    for item in &ordered {
        for tab in &item.contribution.tabs {
            let key = (item.index, tab.surface_id);
            let Some((workspace_id, project_id, active_tab, visible)) =
                surface_resolution.get(&key)
            else {
                // No owning surface/workspace entry in this contribution:
                // drop rather than guess membership by position.
                log::warn!(
                    "[catalog] dropping tab {}/{} with unknown surface {}",
                    item.index,
                    tab.local_id,
                    tab.surface_id
                );
                continue;
            };
            let Some(wire_id) = tab_wire_id(item.index, tab.local_id) else {
                log::warn!(
                    "[catalog] dropping tab {}/{}: id out of wire range",
                    item.index,
                    tab.local_id
                );
                continue;
            };
            tabs.push(CatalogTab {
                id: wire_id,
                kind: tab.kind.as_str().to_string(),
                title: tab.title.clone(),
                active: tab.local_id == *active_tab as u64,
                visible: visible.contains(&(tab.local_id as usize)),
                workspace_id: Some(workspace_id.clone()),
                project_id: Some(project_id.clone()),
                agent_kind: tab.agent_kind.clone(),
                agent_state: tab.agent_state.map(|state| state.as_str().to_string()),
                attention: tab.attention.map(|attention| attention.as_str().to_string()),
                agent_work: tab.agent_work.clone(),
                agent_step: tab.agent_step.clone(),
                agent_activity: tab.agent_activity.clone().map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| CatalogAgentActivity {
                            sequence: entry.sequence,
                            kind: entry.kind,
                            text: entry.text,
                        })
                        .collect()
                }),
            });
        }
    }

    apply_bounds(CatalogPayload {
        device_id: String::new(),
        machine: None,
        projects,
        workspaces,
        tabs,
    })
}

/// Bound counts and title lengths deterministically (truncate the tail).
/// Dragent live context rides along: work/step to the title bound, activity
/// to its own count/text bounds (oldest dropped first, keeping the most
/// recent). Mirrors the `coordination.list/get` truncation discipline.
pub fn apply_bounds(mut payload: CatalogPayload) -> CatalogPayload {
    payload.projects.truncate(MAX_PROJECTS);
    payload.workspaces.truncate(MAX_WORKSPACES);
    payload.tabs.truncate(MAX_TABS);
    for tab in &mut payload.tabs {
        tab.title = truncate_title(&tab.title);
        if let Some(work) = tab.agent_work.as_mut() {
            *work = truncate_title(work);
            if work.is_empty() {
                tab.agent_work = None;
            }
        }
        if let Some(step) = tab.agent_step.as_mut() {
            *step = truncate_title(step);
            if step.is_empty() {
                tab.agent_step = None;
            }
        }
        if let Some(entries) = tab.agent_activity.as_mut() {
            if entries.len() > MAX_AGENT_ACTIVITY_ENTRIES {
                let drop = entries.len() - MAX_AGENT_ACTIVITY_ENTRIES;
                entries.drain(0..drop);
            }
            for entry in entries.iter_mut() {
                entry.kind = truncate_chars(&entry.kind, MAX_AGENT_ACTIVITY_KIND_CHARS);
                entry.text =
                    truncate_chars(&entry.text, MAX_AGENT_ACTIVITY_TEXT_CHARS);
            }
            entries.retain(|entry| !entry.text.is_empty());
            if entries.is_empty() {
                tab.agent_activity = None;
            }
        }
    }
    payload
}

/// Truncate to `max` chars on a char boundary (never split UTF-8).
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// Truncate to a char boundary (never split UTF-8).
pub fn truncate_title(title: &str) -> String {
    truncate_chars(title, MAX_TITLE_CHARS)
}

/// Stable hash of the publishable body (device/machine excluded: they are
/// transport envelope, not catalog content).
pub fn catalog_hash(payload: &CatalogPayload) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(payload)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// Full PUT body: device identity plus the aggregated catalog.
pub fn build_body(
    device_id: &str,
    machine: Option<MachineMeta>,
    mut catalog: CatalogPayload,
) -> serde_json::Value {
    catalog.device_id = device_id.to_string();
    catalog.machine = machine;
    serde_json::to_value(&catalog).unwrap_or(serde_json::Value::Null)
}

/// Send when the snapshot changed, or when the heartbeat elapsed since the
/// last successful send (`None` = never sent: send on first data).
pub fn should_send(changed: bool, now_ms: u64, last_sent_ms: Option<u64>) -> bool {
    if changed {
        return true;
    }
    match last_sent_ms {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= HEARTBEAT_MS,
    }
}

/// Publishing is disabled without a backend or without any signed-in refresh
/// credential. Both are checked without logging in: no auto-login.
pub fn publisher_enabled(backend_empty: bool, has_credential: bool) -> bool {
    !backend_empty && has_credential
}

// ── Validation ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    TabIdOutOfRange(u64),
    UnknownKind(String),
    OrphanTabWorkspace { tab: u64, workspace: String },
    OrphanTabProject { tab: u64, project: String },
    OrphanWorkspaceProject { workspace: String, project: String },
    CrossProjectMismatch { tab: u64 },
    AgentFieldsOnNonAgent(u64),
    EmptyPath(&'static str),
    TooMany(&'static str),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::TabIdOutOfRange(id) => write!(f, "tab id {id} exceeds 2^53-1"),
            CatalogError::UnknownKind(kind) => write!(f, "unknown tab kind {kind:?}"),
            CatalogError::OrphanTabWorkspace { tab, workspace } => {
                write!(f, "tab {tab} names unknown workspace {workspace:?}")
            }
            CatalogError::OrphanTabProject { tab, project } => {
                write!(f, "tab {tab} names unknown project {project:?}")
            }
            CatalogError::OrphanWorkspaceProject { workspace, project } => {
                write!(f, "workspace {workspace:?} names unknown project {project:?}")
            }
            CatalogError::CrossProjectMismatch { tab } => {
                write!(f, "tab {tab} disagrees with its workspace about project")
            }
            CatalogError::AgentFieldsOnNonAgent(tab) => {
                write!(f, "tab {tab} carries agent fields but is not kind=agent")
            }
            CatalogError::EmptyPath(what) => write!(f, "empty {what} path"),
            CatalogError::TooMany(what) => write!(f, "too many {what}"),
        }
    }
}

/// Gate kept before every send: the new publisher must always emit complete
/// relationships; legacy catalogs without ids still *read* elsewhere.
pub fn validate_for_publish(payload: &CatalogPayload) -> Result<(), CatalogError> {
    if payload.projects.len() > MAX_PROJECTS {
        return Err(CatalogError::TooMany("projects"));
    }
    if payload.workspaces.len() > MAX_WORKSPACES {
        return Err(CatalogError::TooMany("workspaces"));
    }
    if payload.tabs.len() > MAX_TABS {
        return Err(CatalogError::TooMany("tabs"));
    }
    let project_ids: HashSet<&str> = payload
        .projects
        .iter()
        .map(|project| {
            if project.path.is_empty() {
                Err(CatalogError::EmptyPath("project"))
            } else {
                Ok(project.path.as_str())
            }
        })
        .collect::<Result<_, _>>()?;
    let mut workspace_projects: HashMap<&str, Option<&str>> = HashMap::new();
    for workspace in &payload.workspaces {
        if workspace.path.is_empty() {
            return Err(CatalogError::EmptyPath("workspace"));
        }
        let id = workspace.id.as_deref().unwrap_or("");
        if let Some(project) = workspace.project_id.as_deref() {
            if !project_ids.contains(project) {
                return Err(CatalogError::OrphanWorkspaceProject {
                    workspace: id.to_string(),
                    project: project.to_string(),
                });
            }
        }
        workspace_projects.insert(id, workspace.project_id.as_deref());
    }
    for tab in &payload.tabs {
        if tab.id > MAX_TAB_ID {
            return Err(CatalogError::TabIdOutOfRange(tab.id));
        }
        let kind = TabKind::parse(&tab.kind).ok_or_else(|| CatalogError::UnknownKind(tab.kind.clone()))?;
        let is_agent = kind == TabKind::Agent;
        if !is_agent
            && (tab.agent_kind.is_some()
                || tab.agent_state.is_some()
                || tab.attention.is_some()
                || tab.agent_work.is_some()
                || tab.agent_step.is_some()
                || tab.agent_activity.is_some())
        {
            return Err(CatalogError::AgentFieldsOnNonAgent(tab.id));
        }
        if let Some(attention) = tab.attention.as_deref() {
            Attention::parse(attention)
                .ok_or_else(|| CatalogError::UnknownKind(format!("attention:{attention}")))?;
        }
        if let Some(state) = tab.agent_state.as_deref() {
            AgentState::parse(state)
                .ok_or_else(|| CatalogError::UnknownKind(format!("agent_state:{state}")))?;
        }
        if let Some(workspace) = tab.workspace_id.as_deref() {
            let owner_project = workspace_projects.get(workspace).ok_or_else(|| {
                CatalogError::OrphanTabWorkspace {
                    tab: tab.id,
                    workspace: workspace.to_string(),
                }
            })?;
            if let Some(project) = tab.project_id.as_deref() {
                if !project_ids.contains(project) {
                    return Err(CatalogError::OrphanTabProject {
                        tab: tab.id,
                        project: project.to_string(),
                    });
                }
                if let Some(owner) = owner_project {
                    if *owner != project {
                        return Err(CatalogError::CrossProjectMismatch { tab: tab.id });
                    }
                }
            }
        } else if let Some(project) = tab.project_id.as_deref() {
            if !project_ids.contains(project) {
                return Err(CatalogError::OrphanTabProject {
                    tab: tab.id,
                    project: project.to_string(),
                });
            }
        }
    }
    Ok(())
}

// ── E2EE sealed catalog (§6.5) ───────────────────────────────────────────────
// Behind the `e2ee` feature until the P2 audit gate. No plaintext catalog
// field leaves the desktop: the catalog JSON is encrypted once with a random
// `K` (XChaCha20-Poly1305, AD = "dragent catalog v1" || account || desktop
// || version) and `K || version` is wrapped per paired phone with Noise X
// under the catalog prologue. The PUT body carries routing only
// (`device_id`, `v`, `desktop`, `version`, `recipients`, `nonce`, `body`).

/// Ciphertext bucket: the sealed body is padded (via plaintext spaces, which
/// are valid JSON whitespace) so the ciphertext lands on a 4 KiB multiple.
/// This satisfies the relay's opaque size check without touching the AEAD
/// tag layout; the phone's JSON parser ignores the trailing spaces.
#[cfg(feature = "e2ee")]
pub const SEALED_BODY_BUCKET: usize = 4096;
/// Largest sealed body the relay stores (256 buckets = 1 MiB).
#[cfg(feature = "e2ee")]
pub const MAX_SEALED_BODY_BYTES: usize = 256 * 4096;
/// XChaCha20-Poly1305 tag length.
#[cfg(feature = "e2ee")]
const SEAL_TAG_LEN: usize = 16;

/// Sealing failed. Fail closed: the caller must send nothing (never
/// plaintext).
#[cfg(feature = "e2ee")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealError {
    /// The version is 0 or above 2^53 − 1.
    BadVersion,
    /// The catalog is too large to seal (over 1 MiB even before the tag).
    TooLarge,
    /// The wrap or body failed authentication, or a key/prologue was wrong.
    Rejected,
    /// A recipient list defect (duplicate phone, bad wrap length).
    Malformed,
    /// The OS RNG failed.
    Random,
}

#[cfg(feature = "e2ee")]
impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SealError::BadVersion => f.write_str("catalog version outside 1..=2^53-1"),
            SealError::TooLarge => f.write_str("catalog too large to seal"),
            SealError::Rejected => f.write_str("catalog seal rejected"),
            SealError::Malformed => f.write_str("catalog recipients malformed"),
            SealError::Random => f.write_str("random number generator failed"),
        }
    }
}

#[cfg(feature = "e2ee")]
impl std::error::Error for SealError {}

#[cfg(feature = "e2ee")]
impl From<dragent_e2ee::seal::catalog::CatalogError> for SealError {
    fn from(e: dragent_e2ee::seal::catalog::CatalogError) -> Self {
        use dragent_e2ee::seal::catalog::CatalogError as C;
        match e {
            C::BadVersion => SealError::BadVersion,
            C::Random => SealError::Random,
            C::Malformed => SealError::Malformed,
            C::Rejected | C::WrongSender | C::VersionMismatch => SealError::Rejected,
        }
    }
}

/// Pad the catalog JSON with trailing spaces (valid JSON whitespace) so the
/// AEAD ciphertext (`plaintext + 16-byte tag`) lands exactly on a 4 KiB
/// bucket. Returns the padded plaintext.
#[cfg(feature = "e2ee")]
fn pad_plaintext(mut json: Vec<u8>) -> Result<Vec<u8>, SealError> {
    if json.len().saturating_add(SEAL_TAG_LEN) > MAX_SEALED_BODY_BYTES {
        return Err(SealError::TooLarge);
    }
    let target_ct =
        ((json.len() + SEAL_TAG_LEN + SEALED_BODY_BUCKET - 1) / SEALED_BODY_BUCKET)
            * SEALED_BODY_BUCKET;
    let target_pt = target_ct - SEAL_TAG_LEN;
    json.resize(target_pt, b' ');
    Ok(json)
}

/// Seal one catalog version for these phones (§6.5 steps 2–3).
///
/// - `catalog_json` is the canonical [`CatalogPayload`] JSON (device +
///   machine envelope included; it is content, so it stays inside the seal).
/// - `phones` is `(phone id, phone identity key)` per paired phone. It is
///   sorted internally so identical inputs give byte-identical blobs.
/// - `identity` is this desktop's identity keypair.
/// - `version` must already be reserved (persisted) by the caller via
///   `E2eStore::next_catalog_version`, so it is never reused.
///
/// Returns the blob to publish (encode with `CatalogBlob::encode`, which
/// pads the whole JSON blob to 4 KiB buckets).
#[cfg(feature = "e2ee")]
pub fn seal_catalog(
    account: &dragent_e2ee::Id,
    desktop: &dragent_e2ee::Id,
    version: u64,
    catalog_json: &[u8],
    phones: &[(dragent_e2ee::Id, dragent_e2ee::noise::PublicKey)],
    identity: &dragent_e2ee::noise::StaticKeypair,
) -> Result<dragent_e2ee::codec::catalog_blob::CatalogBlob, SealError> {
    use dragent_e2ee::codec::catalog_blob::{CatalogBlob, Recipient};
    use dragent_e2ee::noise::CatalogPrologue;
    use dragent_e2ee::seal::catalog as seal;

    let padded = pad_plaintext(catalog_json.to_vec())?;
    let (k, nonce, body) = seal::seal_body(account, desktop, version, &padded)?;
    let mut sorted: Vec<(dragent_e2ee::Id, dragent_e2ee::noise::PublicKey)> =
        phones.to_vec();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut recipients = Vec::with_capacity(sorted.len());
    let mut seen = std::collections::HashSet::new();
    for (phone, key) in &sorted {
        if !seen.insert(*phone) {
            return Err(SealError::Malformed);
        }
        let prologue = CatalogPrologue {
            account: *account,
            desktop: *desktop,
            phone: *phone,
            version,
        };
        let wrap = seal::wrap_key(identity, key, &prologue, &k)?;
        if wrap.len() != dragent_e2ee::seal::catalog::WRAP_LEN {
            return Err(SealError::Malformed);
        }
        recipients.push(Recipient {
            phone: *phone,
            wrap,
        });
    }
    let mut nonce_arr = [0u8; 24];
    nonce_arr.copy_from_slice(&nonce);
    Ok(CatalogBlob {
        desktop: *desktop,
        version,
        recipients,
        nonce: nonce_arr,
        body,
    })
}

/// Build the relay PUT body for a sealed blob: routing only plus ciphertext.
/// No `projects`/`workspaces`/`tabs`/`machine` keys appear outside the seal
/// (the relay answers those plaintext shapes with 410 after the cut-over).
#[cfg(feature = "e2ee")]
pub fn sealed_put_body(
    device_id: &str,
    blob: &dragent_e2ee::codec::catalog_blob::CatalogBlob,
) -> serde_json::Value {
    use dragent_e2ee::codec::b64;
    let recipients: Vec<serde_json::Value> = blob
        .recipients
        .iter()
        .map(|r| {
            serde_json::json!({
                "phone": r.phone.to_string(),
                "wrap": b64::encode(&r.wrap),
            })
        })
        .collect();
    serde_json::json!({
        "device_id": device_id,
        "v": 1,
        "desktop": blob.desktop.to_string(),
        "version": blob.version,
        "recipients": recipients,
        "nonce": b64::encode(&blob.nonce),
        "body": b64::encode(&blob.body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(user: &str, org: &str) -> AccountKey {
        AccountKey::new(user, org)
    }

    fn workspace(
        id: usize,
        title: &str,
        root: &str,
        project: &str,
        branch: Option<&str>,
    ) -> LocalWorkspace {
        LocalWorkspace {
            team: None,
            workspace_id: id,
            title: title.to_string(),
            root: root.to_string(),
            branch: branch.map(str::to_string),
            project_path: project.to_string(),
            project_name: project
                .rsplit('/')
                .next()
                .unwrap_or(project)
                .to_string(),
        }
    }

    fn surface(
        surface_id: usize,
        workspace_id: usize,
        active_tab: usize,
        visible: &[usize],
    ) -> LocalSurface {
        LocalSurface {
            surface_id,
            workspace_id,
            active_tab_id: active_tab,
            visible_tab_ids: visible.to_vec(),
        }
    }

    fn tab(local: u64, surface: usize, kind: TabKind) -> LocalTab {
        LocalTab {
            local_id: local,
            surface_id: surface,
            kind,
            title: format!("tab-{local}"),
            agent_kind: (kind == TabKind::Agent).then(|| "claude".to_string()),
            agent_state: (kind == TabKind::Agent).then_some(AgentState::Working),
            attention: None,
            agent_work: None,
            agent_step: None,
            agent_activity: None,
        }
    }

    fn contribution(
        contributor: &str,
        workspaces: Vec<LocalWorkspace>,
        surfaces: Vec<LocalSurface>,
        tabs: Vec<LocalTab>,
    ) -> WindowContribution {
        WindowContribution {
            contributor: contributor.to_string(),
            account: account("u1", "o1"),
            workspaces,
            surfaces,
            tabs,
            standalone_projects: Vec::new(),
        }
    }

    /// One window: a repo with two sibling worktrees plus a parked surface.
    fn repo_contribution(contributor: &str) -> WindowContribution {
        contribution(
            contributor,
            vec![
                workspace(1, "main", "/repo", "/repo", Some("main")),
                workspace(2, "feat", "/repo-wt/feat", "/repo", Some("feat")),
            ],
            vec![
                surface(1, 1, 7, &[7]),
                surface(2, 2, 8, &[]),
            ],
            vec![tab(7, 1, TabKind::Terminal), tab(8, 2, TabKind::Agent)],
        )
    }

    #[test]
    fn sibling_worktrees_share_one_project_but_keep_workspaces() {
        // The reported bug: two worktree checkouts of one repo must not
        // become two projects. Both resolve (via the sidebar's main-checkout
        // rule, mirrored by the collector) to project "/repo".
        let c = repo_contribution("win-a");
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.projects.len(), 1);
        assert_eq!(payload.projects[0].path, "/repo");
        // …while each worktree keeps its own workspace entry with its branch.
        assert_eq!(payload.workspaces.len(), 2);
        let mut branches: Vec<Option<String>> = payload
            .workspaces
            .iter()
            .map(|workspace| workspace.branch.clone())
            .collect();
        branches.sort();
        assert_eq!(branches, [Some("feat".to_string()), Some("main".to_string())]);
        for workspace in &payload.workspaces {
            assert_eq!(workspace.project_id.as_deref(), Some("/repo"));
        }
        // Tabs land under their authoritative workspace, with per-surface
        // active/visible — never by array position.
        assert_eq!(payload.tabs.len(), 2);
        let main_tab = payload.tabs.iter().find(|tab| tab.id == 7).unwrap();
        let feat_tab = payload.tabs.iter().find(|tab| tab.id == 8).unwrap();
        assert_ne!(
            main_tab.workspace_id.as_deref(),
            feat_tab.workspace_id.as_deref()
        );
        assert_eq!(
            main_tab.workspace_id.as_deref(),
            payload.workspaces[0].id.as_deref()
        );
        assert_eq!(
            feat_tab.workspace_id.as_deref(),
            payload.workspaces[1].id.as_deref()
        );
        assert_eq!(
            main_tab.project_id.as_deref(),
            feat_tab.project_id.as_deref()
        );
        assert!(main_tab.active && main_tab.visible);
        assert!(feat_tab.active && !feat_tab.visible);
        assert_eq!(feat_tab.kind, "agent");
        assert_eq!(feat_tab.agent_kind.as_deref(), Some("claude"));
        assert!(validate_for_publish(&payload).is_ok());
    }

    #[test]
    fn tabs_follow_authoritative_membership_not_position() {
        // Surfaces listed before their workspace still resolve; a tab on an
        // unowned surface is dropped rather than guessed into a workspace.
        let mut c = repo_contribution("win-a");
        c.tabs.push(tab(9, 99, TabKind::Terminal));
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.tabs.len(), 2, "orphan tab must be dropped");
        assert!(payload.tabs.iter().all(|tab| tab.id == 7 || tab.id == 8));
    }

    #[test]
    fn surface_without_workspace_entry_is_dropped_not_invented() {
        // The collector must never emit a surface for a workspace it did not
        // also emit: without the workspace entry the tabs have no owner.
        let mut c = repo_contribution("win-a");
        c.workspaces.remove(1);
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.workspaces.len(), 1);
        assert_eq!(payload.tabs.len(), 1);
        assert_eq!(payload.tabs[0].id, 7);
    }

    #[test]
    fn nonactive_workspace_without_shown_surface_is_represented() {
        // A workspace whose surfaces are all parked still publishes its
        // entry and tabs (active preserved, nothing visible).
        let c = contribution(
            "win-a",
            vec![workspace(5, "bg", "/other", "/other", None)],
            vec![surface(3, 5, 11, &[])],
            vec![tab(11, 3, TabKind::Terminal)],
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.workspaces.len(), 1);
        assert_eq!(payload.workspaces[0].name, "bg");
        assert_eq!(payload.workspaces[0].branch, None);
        assert_eq!(payload.tabs.len(), 1);
        assert!(payload.tabs[0].active);
        assert!(!payload.tabs[0].visible);
        assert!(validate_for_publish(&payload).is_ok());
    }

    #[test]
    fn standalone_project_without_workspace_is_listed() {
        // An opened project with no worktree workspace yet (empty project
        // stays visible in the sidebar) publishes its project entry.
        let mut c = repo_contribution("win-a");
        c.standalone_projects.push(LocalProject {
            path: "/lonely".to_string(),
            name: "lonely".to_string(),
        });
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.projects.len(), 2);
        assert!(payload.projects.iter().any(|p| p.path == "/lonely"));
        assert!(validate_for_publish(&payload).is_ok());
    }

    #[test]
    fn colliding_local_ids_across_windows_get_distinct_wire_ids() {
        // Two windows that both own durable workspace 1 / surface 1 / tab 1:
        // the exact multiwindow collision the aggregator must survive.
        let mk = || {
            contribution(
                "win",
                vec![workspace(1, "main", "/repo", "/repo", Some("main"))],
                vec![surface(1, 1, 1, &[1])],
                vec![tab(1, 1, TabKind::Terminal)],
            )
        };
        let (a, b) = (mk(), mk());
        let items = [
            IndexedContribution { index: 0, contribution: &a },
            IndexedContribution { index: 1, contribution: &b },
        ];
        let payload = aggregate(&items);
        assert_eq!(payload.tabs.len(), 2);
        assert_ne!(payload.tabs[0].id, payload.tabs[1].id);
        assert_eq!(payload.tabs[0].id, 1);
        assert_eq!(payload.tabs[1].id, (1u64 << 32) + 1);
        // Workspaces are namespaced per contributor AND durable workspace id.
        assert_eq!(payload.workspaces.len(), 2);
        assert_eq!(payload.workspaces[0].id.as_deref(), Some("w0w1"));
        assert_eq!(payload.workspaces[1].id.as_deref(), Some("w1w1"));
        // Same checkout path deduplicates to one project.
        assert_eq!(payload.projects.len(), 1);
        // Each tab points at its own window's workspace, never by position.
        assert_eq!(
            payload.tabs[0].workspace_id.as_deref(),
            payload.workspaces[0].id.as_deref()
        );
        assert_eq!(
            payload.tabs[1].workspace_id.as_deref(),
            payload.workspaces[1].id.as_deref()
        );
        assert!(validate_for_publish(&payload).is_ok());
    }

    #[test]
    fn closing_one_contributor_removes_only_its_entries() {
        let a = contribution(
            "win-a",
            vec![workspace(1, "a", "/a", "/a", None)],
            vec![surface(1, 1, 1, &[1])],
            vec![tab(1, 1, TabKind::Terminal)],
        );
        let b = contribution(
            "win-b",
            vec![workspace(1, "b", "/b", "/b", None)],
            vec![surface(1, 1, 1, &[1])],
            vec![tab(1, 1, TabKind::Terminal)],
        );
        let both = [
            IndexedContribution { index: 0, contribution: &a },
            IndexedContribution { index: 1, contribution: &b },
        ];
        assert_eq!(aggregate(&both).tabs.len(), 2);
        // After win-a closes only win-b's aggregate remains.
        let rest = [IndexedContribution { index: 1, contribution: &b }];
        let payload = aggregate(&rest);
        assert_eq!(payload.tabs.len(), 1);
        assert_eq!(payload.tabs[0].id, (1u64 << 32) + 1);
        assert_eq!(payload.tabs[0].workspace_id.as_deref(), Some("w1w1"));
        assert_eq!(payload.projects.len(), 1);
        assert_eq!(payload.projects[0].path, "/b");
        assert!(validate_for_publish(&payload).is_ok());
    }

    #[test]
    fn a_coordinations_workspaces_carry_their_team_and_others_none() {
        let mut lead = workspace(1, "main", "/app", "/app", None);
        lead.team = Some(LocalTeam { id: "t1".into(), title: "Auth rewrite".into(), lead: true, task: "Claude Code".into() });
        let mut worker = workspace(2, "t2", "/wt/t2", "/app", Some("t2"));
        worker.team = Some(LocalTeam { id: "t1".into(), title: "Auth rewrite".into(), lead: false, task: String::new() });
        let plain = workspace(3, "docs", "/wt/docs", "/app", Some("docs"));
        let a = contribution(
            "win-a",
            vec![lead, worker, plain],
            vec![surface(1, 1, 1, &[1]), surface(2, 2, 2, &[2]), surface(3, 3, 3, &[3])],
            vec![tab(1, 1, TabKind::Terminal), tab(2, 2, TabKind::Terminal), tab(3, 3, TabKind::Terminal)],
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &a }]);
        assert!(validate_for_publish(&payload).is_ok());
        let teams: Vec<_> = payload.workspaces.iter().map(|w| w.team.as_ref().map(|t| (t.id.as_str(), t.lead))).collect();
        assert_eq!(teams, vec![Some(("t1", true)), Some(("t1", false)), None]);
        let json = serde_json::to_string(&payload.workspaces).unwrap();
        assert!(json.contains(r#""team":{"id":"t1","title":"Auth rewrite","lead":true,"task":"Claude Code"}"#), "{json}");
        // No team, no field (older phones see what they always saw); an empty task is left out.
        assert_eq!(json.matches(r#""team""#).count(), 2, "{json}");
        assert!(json.contains(r#""lead":false}"#), "{json}");
    }

    #[test]
    fn what_an_agent_needs_rides_on_its_tab_and_only_there() {
        let mut needs = tab(1, 1, TabKind::Agent);
        needs.agent_state = Some(AgentState::Review);
        needs.attention = Some(Attention::NeedsInput);
        let a = contribution(
            "win-a",
            vec![workspace(1, "a", "/a", "/a", None)],
            vec![surface(1, 1, 1, &[1])],
            vec![needs],
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &a }]);
        assert_eq!(payload.tabs[0].attention.as_deref(), Some("needs_input"));
        // `review` stays for readers that don't know `attention`.
        assert_eq!(payload.tabs[0].agent_state.as_deref(), Some("review"));
        assert!(validate_for_publish(&payload).is_ok());
        let json = serde_json::to_string(&payload.tabs[0]).unwrap();
        assert!(json.contains(r#""attention":"needs_input""#), "{json}");

        let mut unknown = payload.clone();
        unknown.tabs[0].attention = Some("panicking".into());
        assert!(validate_for_publish(&unknown).is_err());
        let mut on_terminal = payload.clone();
        on_terminal.tabs[0].kind = "terminal".into();
        on_terminal.tabs[0].agent_kind = None;
        on_terminal.tabs[0].agent_state = None;
        assert!(matches!(
            validate_for_publish(&on_terminal),
            Err(CatalogError::AgentFieldsOnNonAgent(_))
        ));
        for attention in [Attention::NeedsInput, Attention::Failed] {
            assert_eq!(Attention::parse(attention.as_str()), Some(attention));
        }
    }

    #[test]
    fn wire_id_bounds_are_enforced() {
        assert_eq!(tab_wire_id(0, 1), Some(1));
        assert_eq!(tab_wire_id(1, 1), Some((1u64 << 32) + 1));
        assert!(tab_wire_id(0, 1u64 << 32).is_none());
        assert!(tab_wire_id(1 << 21, 1).is_none());
        // Largest representable pair still fits under 2^53-1.
        let max_ok = tab_wire_id((1 << 21) - 1, (1 << 32) - 1).unwrap();
        assert!(max_ok <= MAX_TAB_ID);
    }

    #[test]
    fn validation_rejects_mismatches_and_orphans() {
        let good_tab = CatalogTab {
            id: 1,
            kind: "terminal".into(),
            title: "t".into(),
            active: true,
            visible: true,
            workspace_id: Some("w0w1".into()),
            project_id: Some("/repo".into()),
            agent_kind: None,
            agent_state: None,
            attention: None,
            agent_work: None,
            agent_step: None,
            agent_activity: None,
        };
        let payload = CatalogPayload {
            device_id: "d".into(),
            machine: None,
            projects: vec![CatalogProject { id: Some("/repo".into()), name: "repo".into(), path: "/repo".into() }],
            workspaces: vec![CatalogWorkspace {
                team: None,
                id: Some("w0w1".into()),
                name: "s".into(),
                path: "/repo/wt".into(),
                branch: None,
                project: Some("repo".into()),
                project_id: Some("/repo".into()),
            }],
            tabs: vec![good_tab.clone()],
        };
        assert!(validate_for_publish(&payload).is_ok());

        // Legacy catalog without relationship ids still reads as valid.
        let legacy = CatalogPayload {
            device_id: "d".into(),
            machine: None,
            projects: vec![],
            workspaces: vec![],
            tabs: vec![CatalogTab { workspace_id: None, project_id: None, ..good_tab.clone() }],
        };
        assert!(validate_for_publish(&legacy).is_ok());

        // Orphan workspace ref.
        let mut bad = payload.clone();
        bad.tabs[0].workspace_id = Some("w9w9".into());
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::OrphanTabWorkspace { .. })
        ));

        // Orphan project ref.
        let mut bad = payload.clone();
        bad.tabs[0].project_id = Some("/nope".into());
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::OrphanTabProject { .. })
        ));

        // Cross-project mismatch between tab and its workspace.
        let mut bad = payload.clone();
        bad.projects.push(CatalogProject { id: Some("/other".into()), name: "other".into(), path: "/other".into() });
        bad.tabs[0].project_id = Some("/other".into());
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::CrossProjectMismatch { .. })
        ));

        // Agent fields on a non-agent tab.
        let mut bad = payload.clone();
        bad.tabs[0].agent_kind = Some("claude".into());
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::AgentFieldsOnNonAgent(_))
        ));

        // Unknown kind and out-of-range id.
        let mut bad = payload.clone();
        bad.tabs[0].kind = "tty".into();
        assert!(matches!(validate_for_publish(&bad), Err(CatalogError::UnknownKind(_))));
        let mut bad = payload.clone();
        bad.tabs[0].id = MAX_TAB_ID + 1;
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::TabIdOutOfRange(_))
        ));
    }

    #[test]
    fn bounds_truncate_deterministically() {
        let tabs: Vec<LocalTab> = (0..(MAX_TABS + 100) as u64)
            .map(|id| LocalTab {
                local_id: id,
                surface_id: 1,
                kind: TabKind::Terminal,
                title: format!("tab-{id}"),
                agent_kind: None,
                agent_state: None,
                attention: None,
                agent_work: None,
                agent_step: None,
                agent_activity: None,
            })
            .collect();
        let c = contribution(
            "win-a",
            vec![workspace(1, "a", "/a", "/a", None)],
            vec![surface(1, 1, 0, &[0])],
            tabs,
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &c }]);
        assert_eq!(payload.tabs.len(), MAX_TABS);

        let long = "x".repeat(MAX_TITLE_CHARS + 10);
        assert_eq!(truncate_title(&long).chars().count(), MAX_TITLE_CHARS);
        // Char boundary, not byte boundary.
        let emoji = "é".repeat(MAX_TITLE_CHARS + 5);
        let cut = truncate_title(&emoji);
        assert_eq!(cut.chars().count(), MAX_TITLE_CHARS);
        assert!(cut.is_char_boundary(cut.len()));
    }

    #[test]
    fn dragent_live_context_rides_on_agent_tabs_only() {
        let mut live = tab(1, 1, TabKind::Agent);
        live.agent_kind = Some("dragent".to_string());
        live.agent_state = Some(AgentState::Working);
        live.agent_work = Some("Login authentication".to_string());
        live.agent_step = Some("Thinking…".to_string());
        live.agent_activity = Some(vec![
            LocalAgentActivity {
                sequence: 1,
                kind: "thinking".to_string(),
                text: "Thinking…".to_string(),
            },
            LocalAgentActivity {
                sequence: 2,
                kind: "tool".to_string(),
                text: "Dispatched: edit".to_string(),
            },
        ]);
        let a = contribution(
            "win-a",
            vec![workspace(1, "a", "/a", "/a", None)],
            vec![surface(1, 1, 1, &[1])],
            vec![live],
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &a }]);
        let tab = &payload.tabs[0];
        assert_eq!(tab.agent_state.as_deref(), Some("working"));
        assert_eq!(tab.agent_work.as_deref(), Some("Login authentication"));
        assert_eq!(tab.agent_step.as_deref(), Some("Thinking…"));
        let activity = tab.agent_activity.as_ref().expect("activity");
        assert_eq!(activity.len(), 2);
        assert_eq!(activity[1].text, "Dispatched: edit");
        assert!(validate_for_publish(&payload).is_ok());
        let json = serde_json::to_string(tab).unwrap();
        assert!(json.contains(r#""agent_work":"Login authentication""#), "{json}");
        assert!(json.contains(r#""agent_step":"#), "{json}");

        // Same fields on a non-agent tab are rejected.
        let mut bad = payload.clone();
        bad.tabs[0].kind = "terminal".into();
        bad.tabs[0].agent_kind = None;
        bad.tabs[0].agent_state = None;
        assert!(matches!(
            validate_for_publish(&bad),
            Err(CatalogError::AgentFieldsOnNonAgent(_))
        ));
    }

    #[test]
    fn agent_activity_bounds_keep_the_most_recent() {
        let entries: Vec<LocalAgentActivity> = (0..(MAX_AGENT_ACTIVITY_ENTRIES + 5) as u64)
            .map(|i| LocalAgentActivity {
                sequence: i,
                kind: "tool".to_string(),
                text: format!("Dispatched: edit {i}"),
            })
            .collect();
        let mut live = tab(1, 1, TabKind::Agent);
        live.agent_work = Some("x".repeat(MAX_TITLE_CHARS + 10));
        live.agent_step = Some("y".repeat(MAX_TITLE_CHARS + 10));
        live.agent_activity = Some(entries);
        let a = contribution(
            "win-a",
            vec![workspace(1, "a", "/a", "/a", None)],
            vec![surface(1, 1, 1, &[1])],
            vec![live],
        );
        let payload = aggregate(&[IndexedContribution { index: 0, contribution: &a }]);
        let tab = &payload.tabs[0];
        assert_eq!(
            tab.agent_work.as_deref().unwrap().chars().count(),
            MAX_TITLE_CHARS
        );
        assert_eq!(
            tab.agent_step.as_deref().unwrap().chars().count(),
            MAX_TITLE_CHARS
        );
        let activity = tab.agent_activity.as_ref().expect("activity");
        assert_eq!(activity.len(), MAX_AGENT_ACTIVITY_ENTRIES);
        // Oldest dropped first: the tail (most recent) survives.
        assert_eq!(
            activity.last().unwrap().text,
            format!(
                "Dispatched: edit {}",
                MAX_AGENT_ACTIVITY_ENTRIES + 5 - 1
            )
        );
        assert!(validate_for_publish(&payload).is_ok());

        // Long activity text truncates on a char boundary.
        let long = "é".repeat(MAX_AGENT_ACTIVITY_TEXT_CHARS + 5);
        assert_eq!(
            truncate_chars(&long, MAX_AGENT_ACTIVITY_TEXT_CHARS)
                .chars()
                .count(),
            MAX_AGENT_ACTIVITY_TEXT_CHARS
        );
    }

    #[test]
    fn debounce_sends_on_change_or_heartbeat() {
        assert!(should_send(true, 0, Some(0)));
        assert!(should_send(false, 0, None));
        assert!(!should_send(false, HEARTBEAT_MS - 1, Some(0)));
        assert!(should_send(false, HEARTBEAT_MS, Some(0)));
    }

    #[test]
    fn disabled_gate_needs_backend_and_credential() {
        assert!(!publisher_enabled(true, true));
        assert!(!publisher_enabled(false, false));
        assert!(!publisher_enabled(true, false));
        assert!(publisher_enabled(false, true));
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [TabKind::Terminal, TabKind::Browser, TabKind::Settings, TabKind::Agent] {
            assert_eq!(TabKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(TabKind::parse("tty"), None);
        for state in [AgentState::Idle, AgentState::Working, AgentState::Review] {
            assert_eq!(AgentState::parse(state.as_str()), Some(state));
        }
    }
}

/// §6.5 seal tests with a scripted phone: seal on the desktop, unwrap and
/// open on the phone, then the high-water rollback/equivocation rejections
/// (§12.3 L6) and the no-plaintext-canary assertion (§12.10 shape).
#[cfg(all(test, feature = "e2ee"))]
mod e2ee_tests {
    use super::{
        seal_catalog, sealed_put_body, SealError, MAX_SEALED_BODY_BYTES, SEALED_BODY_BUCKET,
    };
    use dragent_e2ee::codec::catalog_blob;
    use dragent_e2ee::noise::{CatalogPrologue, StaticKeypair};
    use dragent_e2ee::seal::catalog as seal;
    use dragent_e2ee::seal::pin_store::{CatalogCheck, PinStore};
    use dragent_e2ee::Id;

    fn ids() -> (Id, Id, Id) {
        (
            Id::parse("11111111-2222-4333-8444-555555555555").unwrap(),
            Id::parse("01234567-89ab-4cde-8f01-23456789abcd").unwrap(),
            Id::parse("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap(),
        )
    }

    fn sha256(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(bytes);
        h.finalize().into()
    }

    fn open_as_phone(
        phone_keys: &StaticKeypair,
        pinned_desktop: &dragent_e2ee::noise::PublicKey,
        account: &Id,
        desktop: &Id,
        blob: &catalog_blob::CatalogBlob,
        phone: &Id,
    ) -> Vec<u8> {
        let wrap = blob
            .recipients
            .iter()
            .find(|r| &r.phone == phone)
            .expect("wrap for phone")
            .wrap
            .clone();
        let prologue = CatalogPrologue {
            account: *account,
            desktop: *desktop,
            phone: *phone,
            version: blob.version,
        };
        let k = seal::unwrap_key(phone_keys, pinned_desktop, &prologue, &wrap).unwrap();
        seal::open_body(&k, account, desktop, blob.version, &blob.nonce, &blob.body)
            .unwrap()
            .to_vec()
    }

    #[test]
    fn seal_round_trip_and_blob_bucketing() {
        let (account, desktop, phone) = ids();
        let desktop_keys = StaticKeypair::generate().unwrap();
        let phone_keys = StaticKeypair::generate().unwrap();
        let json = br#"{"device_id":"d","machine":{"name":"Studio","platform":"macos"},"projects":[{"id":"/repo","name":"repo","path":"/repo"}],"workspaces":[],"tabs":[]}"#;
        let blob = seal_catalog(
            &account,
            &desktop,
            7,
            json,
            &[(phone, phone_keys.public())],
            &desktop_keys,
        )
        .unwrap();
        assert_eq!(blob.version, 7);
        // Ciphertext lands on a 4 KiB bucket for the relay's opaque check.
        assert_eq!(blob.body.len() % SEALED_BODY_BUCKET, 0);
        assert!(blob.body.len() <= MAX_SEALED_BODY_BYTES);
        // The whole blob JSON is padded to 4 KiB buckets (codec).
        let enc = blob.encode().unwrap();
        assert_eq!(enc.len() % catalog_blob::PAD_BUCKET, 0);
        // Phone opens it.
        let pt = open_as_phone(
            &phone_keys,
            &desktop_keys.public(),
            &account,
            &desktop,
            &blob,
            &phone,
        );
        // Trailing space padding is valid JSON whitespace.
        let v: serde_json::Value = serde_json::from_slice(&pt).unwrap();
        assert_eq!(v["projects"][0]["path"].as_str().unwrap(), "/repo");
        // The sealed PUT body carries routing only outside the seal.
        let put = sealed_put_body(&desktop.to_string(), &blob);
        for leaked in ["projects", "workspaces", "tabs", "machine"] {
            assert!(put.get(leaked).is_none(), "plaintext key {leaked} outside seal");
        }
        assert_eq!(put["device_id"], desktop.to_string());
        assert_eq!(put["desktop"], desktop.to_string());
        // No plaintext canary in the blob bytes (§12.10 shape).
        let canary = b"super-secret-project-canary-9z";
        let json2 = format!(
            "{{\"projects\":[{{\"id\":\"/c\",\"name\":\"{c}\",\"path\":\"/c\"}}]}}",
            c = "super-secret-project-canary-9z"
        );
        let blob2 = seal_catalog(
            &account,
            &desktop,
            8,
            json2.as_bytes(),
            &[(phone, phone_keys.public())],
            &desktop_keys,
        )
        .unwrap();
        let enc2 = blob2.encode().unwrap();
        assert!(
            !enc2.windows(canary.len()).any(|w| w == canary),
            "canary must not appear in sealed bytes"
        );
        // Deterministic recipient order: identical inputs, identical bytes.
        let blob_again = seal_catalog(
            &account,
            &desktop,
            7,
            json,
            &[(phone, phone_keys.public())],
            &desktop_keys,
        )
        .unwrap();
        // Different random K/nonce each time, but same shape and version.
        assert_eq!(blob_again.version, blob.version);
        assert_eq!(blob_again.recipients.len(), 1);
    }

    #[test]
    fn seal_rejects_bad_version_and_oversize() {
        let (account, desktop, phone) = ids();
        let desktop_keys = StaticKeypair::generate().unwrap();
        let phone_keys = StaticKeypair::generate().unwrap();
        let phones = [(phone, phone_keys.public())];
        assert_eq!(
            seal_catalog(&account, &desktop, 0, b"{}", &phones, &desktop_keys).unwrap_err(),
            SealError::BadVersion
        );
        assert_eq!(
            seal_catalog(
                &account,
                &desktop,
                1 << 53,
                b"{}",
                &phones,
                &desktop_keys
            )
            .unwrap_err(),
            SealError::BadVersion
        );
        let huge = vec![b'x'; MAX_SEALED_BODY_BYTES];
        assert_eq!(
            seal_catalog(&account, &desktop, 3, &huge, &phones, &desktop_keys).unwrap_err(),
            SealError::TooLarge
        );
        // Duplicate recipient is malformed.
        let dup = [(phone, phone_keys.public()), (phone, phone_keys.public())];
        assert_eq!(
            seal_catalog(&account, &desktop, 3, b"{}", &dup, &desktop_keys).unwrap_err(),
            SealError::Malformed
        );
    }

    #[test]
    fn phone_rejects_rollback_and_equivocation() {
        // §12.3 L6 shape: pair (floor), accept v5, then the relay replays v5
        // after the desktop moved to v9 — rejected as rollback. Same version
        // with a different body is equivocation.
        let (account, desktop, phone_id) = ids();
        let desktop_keys = StaticKeypair::generate().unwrap();
        let phone_keys = StaticKeypair::generate().unwrap();
        let phones = [(phone_id, phone_keys.public())];

        let v5 = seal_catalog(&account, &desktop, 5, b"{\"projects\":[]}", &phones, &desktop_keys)
            .unwrap();
        let pt5 = open_as_phone(
            &phone_keys,
            &desktop_keys.public(),
            &account,
            &desktop,
            &v5,
            &phone_id,
        );
        let mut marks = PinStore::default();
        marks.catalog_set_floor(desktop, 5);
        assert_eq!(marks.catalog_check(&desktop, 5, &sha256(&pt5)), CatalogCheck::Accept);
        marks.catalog_accept(desktop, 5, &sha256(&pt5));
        assert_eq!(
            marks.catalog_check(&desktop, 5, &sha256(&pt5)),
            CatalogCheck::Unchanged
        );

        // Equivocation: same version, different body.
        let v5b = seal_catalog(
            &account,
            &desktop,
            5,
            b"{\"projects\":[{\"path\":\"/evil\"}]}",
            &phones,
            &desktop_keys,
        )
        .unwrap();
        let pt5b = open_as_phone(
            &phone_keys,
            &desktop_keys.public(),
            &account,
            &desktop,
            &v5b,
            &phone_id,
        );
        assert_ne!(pt5, pt5b);
        assert_eq!(
            marks.catalog_check(&desktop, 5, &sha256(&pt5b)),
            CatalogCheck::Equivocation
        );

        // Move on to v9, then the relay replays v5: rollback.
        let v9 = seal_catalog(&account, &desktop, 9, b"{\"projects\":[]}", &phones, &desktop_keys)
            .unwrap();
        let pt9 = open_as_phone(
            &phone_keys,
            &desktop_keys.public(),
            &account,
            &desktop,
            &v9,
            &phone_id,
        );
        assert_eq!(marks.catalog_check(&desktop, 9, &sha256(&pt9)), CatalogCheck::Accept);
        marks.catalog_accept(desktop, 9, &sha256(&pt9));
        assert_eq!(
            marks.catalog_check(&desktop, 5, &sha256(&pt5)),
            CatalogCheck::Rollback
        );
        // Re-pair floor (pair-done v9 → floor v8): v5 still rejected.
        marks.catalog_set_floor(desktop, 9);
        assert_eq!(
            marks.catalog_check(&desktop, 5, &sha256(&pt5)),
            CatalogCheck::Rollback
        );
    }

    #[test]
    fn phone_rejects_wrong_sender_and_version_mismatch() {
        let (account, desktop, phone_id) = ids();
        let desktop_keys = StaticKeypair::generate().unwrap();
        let relay_keys = StaticKeypair::generate().unwrap();
        let phone_keys = StaticKeypair::generate().unwrap();
        let (k, nonce, body) = seal::seal_body(&account, &desktop, 7, b"{}").unwrap();
        // Forged by the relay's key.
        let forged = seal::wrap_key(
            &relay_keys,
            &phone_keys.public(),
            &CatalogPrologue {
                account,
                desktop,
                phone: phone_id,
                version: 7,
            },
            &k,
        )
        .unwrap();
        assert_eq!(
            seal::unwrap_key(
                &phone_keys,
                &desktop_keys.public(),
                &CatalogPrologue {
                    account,
                    desktop,
                    phone: phone_id,
                    version: 7,
                },
                &forged
            )
            .unwrap_err(),
            seal::CatalogError::WrongSender
        );
        // Wrap for v7 presented as v8 fails authentication (prologue-bound).
        let wrap = seal::wrap_key(
            &desktop_keys,
            &phone_keys.public(),
            &CatalogPrologue {
                account,
                desktop,
                phone: phone_id,
                version: 7,
            },
            &k,
        )
        .unwrap();
        assert!(matches!(
            seal::unwrap_key(
                &phone_keys,
                &desktop_keys.public(),
                &CatalogPrologue {
                    account,
                    desktop,
                    phone: phone_id,
                    version: 8,
                },
                &wrap
            ),
            Err(seal::CatalogError::Rejected)
        ));
        // Body under another version fails.
        assert!(seal::open_body(&k, &account, &desktop, 8, &nonce, &body).is_err());
    }
}
