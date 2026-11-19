/**
 * A desktop's workspaces as one list, grouped under their projects: what the
 * desktop screen shows, so the person reaches a workspace in one tap instead
 * of going through its project first. Searchable by workspace, branch,
 * project or tab title. Pure: built from the catalog alone.
 */
import type { RelayCatalog, RelayTab, RelayWorkspace, RelayWorkspaceTeam } from "../transport/relayCatalog";
import { countNeeds, mostUrgent, tabNeed, type Need, type NeedCounts } from "./attention";
import {
  knownProjectKeys,
  orphanWorkspaces,
  projectKey,
  tabsForWorkspace,
  workspaceBranchLabel,
  workspaceKey,
  workspacesForProject,
} from "./machineCatalog";

export interface DirectoryWorkspace {
  readonly key: string;
  readonly name: string;
  readonly branch: string;
  readonly projectName: string | null;
  readonly tabs: readonly RelayTab[];
  readonly needs: NeedCounts;
  /** The tab opening this workspace lands on; null when it has none. */
  readonly openTabId: number | null;
  /** Its folder on the desktop. */
  readonly path: string;
  /**
   * The project's own checkout rather than a worktree: deleting it only
   * closes it in Dragent, its files stay.
   */
  readonly mainCheckout: boolean;
  /** The coordination it works for, when it does. */
  readonly team: RelayWorkspaceTeam | null;
}

export interface DirectorySection {
  /** Project key, or `""` for workspaces without a known project. */
  readonly key: string;
  readonly title: string;
  readonly data: readonly DirectoryWorkspace[];
  readonly needs: NeedCounts;
}

const NEED_ORDER: readonly Need[] = ["needs_input", "failed", "review"];

/**
 * The tab a workspace opens on: the one waiting for the person (input first,
 * then a failure, then a finished turn), else the one in front on the
 * desktop, else an agent at work, else the first.
 */
export function openingTab(tabs: readonly RelayTab[]): RelayTab | null {
  for (const need of NEED_ORDER) {
    const tab = tabs.find((candidate) => tabNeed(candidate) === need);
    if (tab) return tab;
  }
  return (
    tabs.find((tab) => tab.active) ??
    tabs.find((tab) => tab.agent_state === "working") ??
    tabs[0] ??
    null
  );
}

function urgency(needs: NeedCounts): number {
  const need = mostUrgent(needs);
  if (need === "needs_input") return 0;
  if (need === "failed") return 1;
  if (need === "review") return 2;
  return needs.working > 0 ? 3 : 4;
}

function matches(query: string, workspace: DirectoryWorkspace): boolean {
  if (query === "") return true;
  const haystack = [
    workspace.name,
    workspace.branch,
    workspace.projectName ?? "",
    ...workspace.tabs.map((tab) => tab.title),
  ]
    .join("\n")
    .toLowerCase();
  return query
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

function entry(catalog: RelayCatalog, workspace: RelayWorkspace, projectName: string | null): DirectoryWorkspace {
  const tabs = tabsForWorkspace(catalog, workspace);
  return {
    key: workspaceKey(workspace),
    name: workspace.name,
    branch: workspaceBranchLabel(workspace),
    projectName,
    tabs,
    needs: countNeeds(tabs),
    openTabId: openingTab(tabs)?.id ?? null,
    path: workspace.path,
    mainCheckout: catalog.projects.some((project) => project.path === workspace.path),
    team: workspace.team ?? null,
  };
}

/**
 * Sections in the desktop's project order; within one, workspaces that need
 * the person come first (then failures, reviews, work in progress), keeping
 * the desktop's order otherwise. Empty projects are left out while searching.
 */
export type StatusFilter = "all" | "needs_you" | "working" | "review" | "failed";
export type DirectorySort = "attention" | "name" | "active";

export interface DirectoryFilters {
  readonly status?: StatusFilter;
  /** An agent kind (`claude`, `codex`, …): only workspaces where it runs. */
  readonly agent?: string | null;
  readonly sort?: DirectorySort;
  /**
   * Pinned workspace keys (this phone's, per desktop). They leave their
   * project for a "Pinned" section at the top, kept in the same order rules.
   */
  readonly pinned?: readonly string[];
}

/** The key of the "Pinned" section (project keys are never empty-prefixed like this). */
export const PINNED_SECTION = "\u0000pinned";

export const STATUS_FILTERS: readonly { id: StatusFilter; label: string }[] = [
  { id: "all", label: "All workspaces" },
  { id: "needs_you", label: "Needs you" },
  { id: "working", label: "Working" },
  { id: "review", label: "To review" },
  { id: "failed", label: "Failed" },
];

export const DIRECTORY_SORTS: readonly { id: DirectorySort; label: string }[] = [
  { id: "attention", label: "Needs you first" },
  { id: "active", label: "Most active" },
  { id: "name", label: "Name" },
];

function hasStatus(workspace: DirectoryWorkspace, status: StatusFilter): boolean {
  const needs = workspace.needs;
  // Blocked on the person: a question or permission, or a failure.
  if (status === "needs_you") return needs.needsInput > 0 || needs.failed > 0;
  if (status === "working") return needs.working > 0;
  if (status === "review") return needs.review > 0;
  if (status === "failed") return needs.failed > 0;
  return true;
}

/** The agent kinds running in a workspace's tabs. */
export function workspaceAgents(workspace: DirectoryWorkspace): string[] {
  return [...new Set(workspace.tabs.map((tab) => tab.agent_kind ?? "").filter(Boolean))];
}

/** Every agent kind running on the desktop, for the agent filter. */
export function directoryAgents(catalog: RelayCatalog): string[] {
  return [...new Set(catalog.tabs.map((tab) => tab.agent_kind ?? "").filter(Boolean))].sort();
}

/** How busy a workspace is: tabs waiting on you, then agents at work. */
function activity(needs: NeedCounts): number {
  return needs.needsInput * 100 + needs.failed * 10 + needs.working * 5 + needs.review;
}

export function workspaceDirectory(catalog: RelayCatalog, query = "", filters: DirectoryFilters = {}): readonly DirectorySection[] {
  const q = query.trim().toLowerCase();
  const status = filters.status ?? "all";
  const agent = filters.agent ?? null;
  const sort = filters.sort ?? "attention";
  const filtering = q !== "" || status !== "all" || agent !== null;
  const keep = (workspace: DirectoryWorkspace) =>
    matches(q, workspace) && hasStatus(workspace, status) && (agent === null || workspaceAgents(workspace).includes(agent));
  const byUrgency = (list: DirectoryWorkspace[]) =>
    list
      .map((workspace, index) => ({ workspace, index }))
      .sort((a, b) => {
        if (sort === "name") return a.workspace.name.localeCompare(b.workspace.name, undefined, { sensitivity: "base" }) || a.index - b.index;
        if (sort === "active") return activity(b.workspace.needs) - activity(a.workspace.needs) || a.index - b.index;
        return urgency(a.workspace.needs) - urgency(b.workspace.needs) || a.index - b.index;
      })
      .map(({ workspace }) => workspace);
  const pinnedKeys = new Set(filters.pinned ?? []);
  const pinned: DirectoryWorkspace[] = [];
  const unpinned = (list: DirectoryWorkspace[]) => {
    const rest: DirectoryWorkspace[] = [];
    for (const workspace of list) {
      if (pinnedKeys.has(workspace.key)) pinned.push(workspace);
      // Coordination work is listed once per coordination (see
      // `directoryTeams`), unless the person is searching or filtering.
      else if (workspace.team && !filtering) continue;
      else rest.push(workspace);
    }
    return rest;
  };
  const sections: DirectorySection[] = [];
  for (const project of catalog.projects) {
    const all = workspacesForProject(catalog, project).map((workspace) => entry(catalog, workspace, project.name));
    const data = byUrgency(unpinned(all.filter(keep)));
    if (data.length === 0 && filtering) continue;
    sections.push({
      key: projectKey(project),
      title: project.name,
      data,
      needs: countNeeds(all.flatMap((workspace) => workspace.tabs)),
    });
  }
  const orphans = unpinned(
    orphanWorkspaces(catalog, knownProjectKeys(catalog))
      .map((workspace) => entry(catalog, workspace, null))
      .filter(keep),
  );
  if (orphans.length > 0) {
    sections.push({
      key: "",
      title: "Other workspaces",
      data: byUrgency(orphans),
      needs: countNeeds(orphans.flatMap((workspace) => workspace.tabs)),
    });
  }
  if (pinned.length > 0) {
    sections.unshift({
      key: PINNED_SECTION,
      title: "Pinned",
      data: byUrgency(pinned),
      needs: countNeeds(pinned.flatMap((workspace) => workspace.tabs)),
    });
  }
  return sections;
}

/** Every tab on the desktop that waits for the person, most urgent first. */
export function tabsNeedingYou(catalog: RelayCatalog, limit = 5): readonly RelayTab[] {
  const out: RelayTab[] = [];
  for (const need of NEED_ORDER) {
    for (const tab of catalog.tabs) {
      if (tabNeed(tab) === need) out.push(tab);
    }
  }
  return out.slice(0, limit);
}

/**
 * What home shows under a desktop, kept short however many agents finish:
 * the few tabs that are blocked on the person (input, then failures) as
 * rows, how many more are blocked, and one count of finished turns to
 * review rather than a row each.
 */
export interface HomeAttention {
  readonly urgent: readonly RelayTab[];
  readonly moreUrgent: number;
  readonly reviews: number;
}

export function homeAttention(catalog: RelayCatalog, rows = 2): HomeAttention {
  const blocked: RelayTab[] = [];
  let reviews = 0;
  for (const need of NEED_ORDER) {
    for (const tab of catalog.tabs) {
      if (tabNeed(tab) !== need) continue;
      if (need === "review") reviews += 1;
      else blocked.push(tab);
    }
  }
  return { urgent: blocked.slice(0, rows), moreUrgent: Math.max(0, blocked.length - rows), reviews };
}

/** A `show` route param, as the workspace list's status filter. */
export function parseStatusFilter(value: unknown): StatusFilter {
  const raw = Array.isArray(value) ? value[0] : value;
  return STATUS_FILTERS.find((entry) => entry.id === raw)?.id ?? "all";
}

/** One coordination (agent team) on the desktop, standing for all its workspaces. */
export interface DirectoryTeam {
  readonly id: string;
  readonly title: string;
  /** The project its lead works in (a section key), so it lists there. */
  readonly projectKey: string;
  readonly workspaces: readonly DirectoryWorkspace[];
  readonly needs: NeedCounts;
  /** Agent kinds at work in it, lead's first. */
  readonly agents: readonly string[];
}

/**
 * The desktop's coordinations, each with the workspaces working for it (the
 * lead's first), most urgent first. Workspaces the person pinned stay in
 * Pinned and are left out here.
 */
export function directoryTeams(catalog: RelayCatalog, pinned: readonly string[] = []): readonly DirectoryTeam[] {
  const keep = new Set(pinned);
  const byTeam = new Map<string, { title: string; workspaces: { entry: DirectoryWorkspace; project: string }[] }>();
  const known = knownProjectKeys(catalog);
  const projectOf = (workspace: RelayWorkspace) => {
    const key = workspace.project_id ?? workspace.project ?? "";
    return known.has(key) ? key : "";
  };
  const projectName = (key: string) => catalog.projects.find((project) => projectKey(project) === key)?.name ?? null;
  for (const workspace of catalog.workspaces) {
    const team = workspace.team;
    if (!team || keep.has(workspaceKey(workspace))) continue;
    const group = byTeam.get(team.id) ?? { title: team.title, workspaces: [] };
    const project = projectOf(workspace);
    group.workspaces.push({ entry: entry(catalog, workspace, projectName(project)), project });
    byTeam.set(team.id, group);
  }
  const teams: DirectoryTeam[] = [...byTeam.entries()].map(([id, group]) => {
    const sorted = [...group.workspaces].sort((a, b) => Number(b.entry.team?.lead) - Number(a.entry.team?.lead));
    const workspaces = sorted.map((item) => item.entry);
    const tabs = workspaces.flatMap((workspace) => workspace.tabs);
    return {
      id,
      title: group.title,
      projectKey: sorted[0]?.project ?? "",
      workspaces,
      needs: countNeeds(tabs),
      agents: [...new Set(tabs.map((tab) => (tab.agent_kind ?? "").toLowerCase()).filter(Boolean))],
    };
  });
  return teams
    .map((team, index) => ({ team, index }))
    .sort((a, b) => urgency(a.team.needs) - urgency(b.team.needs) || a.index - b.index)
    .map(({ team }) => team);
}
