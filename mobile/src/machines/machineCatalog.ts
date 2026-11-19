import type {
  RelayCatalog,
  RelayProject,
  RelayTab,
  RelayWorkspace,
} from "../transport/relayCatalog";

/**
 * Pure hierarchy selectors over a live relay catalog.
 *
 * Membership is never invented by array order: a workspace belongs to a
 * project only when its explicit relationship key matches, and a tab belongs
 * to a workspace only when its explicit workspace key matches. Stable
 * repository/worktree paths are allowed IDs, so the key is `id ?? path`.
 * Legacy entries without relationship IDs are readable but never guessed
 * into a group — they surface as orphan/unplaced so the UI stays honest.
 */

export function projectKey(project: RelayProject): string {
  return project.id ?? project.path;
}

export function projectLabelForPath(catalog: RelayCatalog | null, path: string): string {
  return catalog?.projects.find((project) => project.path === path)?.name || path;
}

export function workspaceKey(workspace: RelayWorkspace): string {
  return workspace.id ?? workspace.path;
}

/** The project a workspace claims, or null when it claims none (legacy/orphan). */
export function workspaceProjectKey(workspace: RelayWorkspace): string | null {
  return workspace.project_id ?? workspace.project ?? null;
}

/** The workspace a tab claims, or null when it claims none (legacy). */
export function tabWorkspaceKey(tab: RelayTab): string | null {
  return tab.workspace_id;
}

/** The project a tab claims, or null when it claims none (legacy). */
export function tabProjectKey(tab: RelayTab): string | null {
  return tab.project_id;
}

export function workspaceBranchLabel(workspace: RelayWorkspace): string {
  return workspace.branch ?? "No branch";
}

/** True when the catalog carries no relationship IDs at all (legacy publisher). */
export function isLegacyCatalog(catalog: RelayCatalog): boolean {
  return (
    catalog.projects.every((project) => project.id === null) &&
    catalog.workspaces.every(
      (workspace) => workspace.id === null && workspace.project_id === null,
    ) &&
    catalog.tabs.every(
      (tab) => tab.workspace_id === null && tab.project_id === null,
    )
  );
}

/** Workspaces explicitly owned by this project; never guessed by order. */
export function workspacesForProject(
  catalog: RelayCatalog,
  project: RelayProject,
): readonly RelayWorkspace[] {
  const key = projectKey(project);
  return catalog.workspaces.filter(
    (workspace) => workspaceProjectKey(workspace) === key,
  );
}

/**
 * Workspaces matching a name-or-branch query (case-insensitive, trimmed).
 * A null branch matches as empty so branchless checkouts never crash.
 * An empty query returns every workspace.
 */
export function filterWorkspaces(
  workspaces: readonly RelayWorkspace[],
  query: string,
): readonly RelayWorkspace[] {
  const q = query.trim().toLowerCase();
  if (!q) return workspaces;
  return workspaces.filter(
    (workspace) =>
      workspace.name.toLowerCase().includes(q) ||
      (workspace.branch ?? "").toLowerCase().includes(q),
  );
}

/** Workspaces claiming no known project (legacy or foreign ids). */
export function orphanWorkspaces(
  catalog: RelayCatalog,
  knownProjectKeys: ReadonlySet<string>,
): readonly RelayWorkspace[] {
  return catalog.workspaces.filter((workspace) => {
    const key = workspaceProjectKey(workspace);
    return key === null || !knownProjectKeys.has(key);
  });
}

/** Tabs explicitly owned by this workspace; legacy tabs are never placed. */
export function tabsForWorkspace(
  catalog: RelayCatalog,
  workspace: RelayWorkspace,
): readonly RelayTab[] {
  const key = workspaceKey(workspace);
  return catalog.tabs.filter((tab) => tabWorkspaceKey(tab) === key);
}

/**
 * Tabs owned by this project: tabs in its workspaces plus orphan tabs that
 * explicitly claim the project. Legacy tabs (no ids) are never placed.
 */
export function tabsForProject(
  catalog: RelayCatalog,
  project: RelayProject,
): readonly RelayTab[] {
  const key = projectKey(project);
  const workspaceKeys = new Set(
    workspacesForProject(catalog, project).map(workspaceKey),
  );
  return catalog.tabs.filter((tab) => {
    const workspaceId = tabWorkspaceKey(tab);
    if (workspaceId !== null && workspaceKeys.has(workspaceId)) return true;
    if (workspaceId !== null) return false;
    return tabProjectKey(tab) === key;
  });
}

/** Tabs claiming no known workspace: legacy (no ids) or foreign ids. */
export function unplacedTabs(
  catalog: RelayCatalog,
  knownWorkspaceKeys: ReadonlySet<string>,
): readonly RelayTab[] {
  return catalog.tabs.filter((tab) => {
    const key = tabWorkspaceKey(tab);
    return key === null || !knownWorkspaceKeys.has(key);
  });
}

export function knownProjectKeys(
  catalog: RelayCatalog,
): ReadonlySet<string> {
  return new Set(catalog.projects.map(projectKey));
}

export function knownWorkspaceKeys(
  catalog: RelayCatalog,
): ReadonlySet<string> {
  return new Set(catalog.workspaces.map(workspaceKey));
}

/**
 * §6.3: after the cut-over the relay knows no names, so a discovery entry
 * without one is a same-account desktop this phone has no pin for. It
 * renders as "Pair this desktop to open it" and never opens a catalog.
 */
export function isUnpairedMachineEntry(machine: { name: string }): boolean {
  return machine.name.trim() === "";
}

/**
 * Display name for a discovery entry. Post-cut-over the relay sends no
 * names, so a paired desktop's name comes from the pin store (set at
 * pairing); the relay name is only a pre-cut-over fallback. Empty means
 * no pin: a "Pair this desktop" row.
 */
export function displayMachineName(entry: { name: string }, pairedName: string | null): string {
  const pinned = (pairedName ?? "").trim();
  if (pinned) return pinned;
  return entry.name;
}
