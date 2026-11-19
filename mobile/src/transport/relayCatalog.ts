/**
 * Relay catalog transport: read-only access to the host's whole-machine
 * catalog (projects, workspaces with branches, tabs with kinds) through the
 * cloud relay's authorized catalog endpoint.
 *
 * The gateway URL and token plumbing mirror the relay socket: the same
 * `https://…` gateway serves `https://…/v1/relay/catalog`, and every call
 * resolves a fresh device access token the way each socket dial does —
 * never a cached one. The endpoint itself is same-pairing scoped
 * server-side (server/internal/relay), so a device id from another pairing
 * simply reads as unavailable.
 */

export type RelayTabKind = "terminal" | "browser" | "settings" | "agent";

export type RelayAgentState = "idle" | "working" | "review";

/**
 * What an agent tab needs from the person beside its state: it waits at a
 * permission prompt or a question, or its turn failed. Older desktops never
 * send it; an unknown value reads as nothing rather than dropping the tab.
 */
export type RelayAttention = "needs_input" | "failed";

/**
 * One recent Dragent pipeline entry carried on an agent tab: the same
 * `WorkspaceExecutionActivity{sequence,kind,text}` the desktop header folds
 * into its latest-step segment (plus principal coordination activity while a
 * coordination turn is in flight). Oldest-first; the phone renders it as a
 * readable feed instead of a blank idle terminal.
 */
export interface RelayAgentActivity {
  /** Pipeline sequence; orders the feed oldest-first. */
  readonly sequence: number;
  /** Pipeline kind (`thinking`, `narration`, `tool`, `finished`, …). Unknown kinds render as plain updates. */
  readonly kind: string;
  /** Bounded plain text, already truncated server-side; never empty. */
  readonly text: string;
}

export interface RelayProject {
  /** Stable owning id when the publisher supplies one; null for legacy catalogs. */
  readonly id: string | null;
  readonly name: string;
  readonly path: string;
}

export interface RelayWorkspace {
  /** Stable owning id when the publisher supplies one; null for legacy catalogs. */
  readonly id: string | null;
  readonly name: string;
  readonly path: string;
  readonly branch: string | null;
  readonly project: string | null;
  /** Owning project id when the publisher supplies one; null for legacy catalogs. */
  readonly project_id: string | null;
  /**
   * The coordination (agent team) this workspace works for, when it does:
   * its lead runs here, or a worker works on one of its tasks. Older
   * desktops never send it.
   */
  readonly team?: RelayWorkspaceTeam | null | undefined;
}

export interface RelayWorkspaceTeam {
  readonly id: string;
  readonly title: string;
  /** The lead runs here (else a worker on a task). */
  readonly lead: boolean;
  /** What it works on: the task, or for the lead, the agent leading. */
  readonly task: string;
}

export interface RelayTab {
  readonly id: number;
  readonly kind: RelayTabKind;
  readonly title: string;
  readonly active: boolean;
  readonly visible: boolean;
  /** Owning workspace id when the publisher supplies one; null for legacy catalogs. */
  readonly workspace_id: string | null;
  /** Owning project id when the publisher supplies one; null for legacy catalogs. */
  readonly project_id: string | null;
  /** Explicit agent identity on a terminal/agent pane; null unless the host states it. */
  readonly agent_kind: string | null;
  readonly agent_state: RelayAgentState | null;
  readonly attention: RelayAttention | null;
  /**
   * Dragent live context (built-in agent only, null otherwise): the active
   * work title the desktop header shows before the `·` segment. Absent on
   * older desktops; the phone falls back to PTY bytes then.
   */
  readonly agent_work: string | null;
  /** Latest live step (`Thinking…`, `Dispatched: <tool>`, …): the header's trailing segment. */
  readonly agent_step: string | null;
  /** Recent pipeline entries, oldest-first. Null when there is nothing to show. */
  readonly agent_activity: readonly RelayAgentActivity[] | null;
}

export interface RelayMachine {
  readonly name: string;
  readonly platform: string;
  readonly version: string | null;
}

export interface RelayCatalog {
  /** Host metadata; null when the publisher omits it (legacy catalogs). */
  readonly machine: RelayMachine | null;
  readonly projects: readonly RelayProject[];
  readonly workspaces: readonly RelayWorkspace[];
  readonly tabs: readonly RelayTab[];
}

/** Relay-side machine discovery entry (machines.list wire shape). */
export interface MachineSummary {
  readonly device_id: string;
  readonly name: string;
  readonly platform: string;
  readonly version: string | null;
  /** Unix milliseconds assigned by the relay. */
  readonly last_seen_at: number;
  /** Computed relay-side: last seen within ~30s. */
  readonly online: boolean;
}

export type RelayCatalogFetch = (url: string, init?: { headers?: Record<string, string> }) => Promise<{
  readonly status: number;
  json(): Promise<unknown>;
}>;

export interface RelayCatalogOptions {
  /** The relay gateway endpoint, e.g. `https://example.test`. */
  gateway: string;
  /** The host device whose catalog to read (same pairing only). */
  deviceId: string;
  /** Static device access token; prefer resolveAccessToken so every call is fresh. */
  accessToken?: string;
  /** Refresh device auth before every call; null permits an anonymous probe. */
  resolveAccessToken?: () => Promise<string | null>;
  fetchFn?: RelayCatalogFetch;
}

export class RelayCatalogError extends Error {
  readonly status: number | null;
  /** True when pairing (or re-pairing) can resolve the failure. */
  readonly loginRequired: boolean;
  constructor(message: string, status: number | null = null, loginRequired = false) {
    super(message);
    this.name = "RelayCatalogError";
    this.status = status;
    this.loginRequired = loginRequired;
  }
}

const RELAY_TAB_KINDS: readonly RelayTabKind[] = Object.freeze(["terminal", "browser", "settings", "agent"]);

export function isRelayTabKind(value: unknown): value is RelayTabKind {
  return typeof value === "string" && (RELAY_TAB_KINDS as readonly string[]).includes(value);
}

const RELAY_AGENT_STATES: readonly RelayAgentState[] = Object.freeze(["idle", "working", "review"]);

export function isRelayAgentState(value: unknown): value is RelayAgentState {
  return typeof value === "string" && (RELAY_AGENT_STATES as readonly string[]).includes(value);
}

/**
 * Bounds mirror server/internal/relay/catalog.go: a catalog is a small
 * listing, not storage. Malformed input returns null; callers map that to a
 * user-safe "unreadable" message without leaking payload details.
 */
export const RELAY_CATALOG_LIMITS = Object.freeze({
  projects: 128,
  workspaces: 256,
  tabs: 1024,
  machines: 256,
  name: 256,
  path: 1024,
  id: 1024,
  title: 1024,
  branch: 256,
  platform: 64,
  version: 64,
  agentKind: 64,
  deviceId: 128,
  /** Numeric tab ids must survive JSON decoding in mobile JavaScript exactly. */
  maxTabId: 9007199254740991,
  /**
   * Dragent live context caps. The desktop already bounds these
   * (`MAX_TITLE_CHARS` for work/step, `MAX_AGENT_ACTIVITY_ENTRIES` with
   * char-boundary-truncated text for activity); the phone caps are
   * deliberately roomier and degrade by dropping/truncating, never by
   * rejecting the catalog.
   */
  agentActivity: 32,
  agentActivityKind: 64,
  agentActivityText: 2048,
});

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function cleanBoundedText(value: unknown, max: number): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text === "" || text.length > max || value.length > max + 64) return null;
  return text;
}

function optionalBoundedText(value: unknown, max: number): string | null {
  if (value === undefined || value === null) return null;
  return cleanBoundedText(value, max);
}

/** Optional stable id: absent/null for legacy entries; otherwise a bounded non-empty string. */
function optionalId(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  return cleanBoundedText(value, RELAY_CATALOG_LIMITS.id);
}

/** Truncate to `max` chars on a char boundary (iterating code points, never splitting UTF-16). */
function truncateChars(text: string, max: number): string {
  if (text.length <= max) return text;
  return Array.from(text).slice(0, max).join("");
}

/**
 * Derive the catalog REST URL from the realtime gateway endpoint. The gateway
 * is an endpoint, never a credential carrier: like the realtime client,
 * credentials, query strings and fragments are rejected rather than forwarded.
 */
export function resolveRelayCatalogURL(gateway: string): string | null {
  let url: URL;
  try {
    url = new URL(gateway);
  } catch {
    return null;
  }
  if (url.username || url.password || url.search || url.hash) return null;
  if (url.protocol === "wss:") url.protocol = "https:";
  else if (url.protocol === "ws:") url.protocol = "http:";
  else if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  return `${url.origin}/v1/relay/catalog`;
}

function parseProject(entry: unknown): RelayProject | null {
  if (!isRecord(entry)) return null;
  const name = cleanBoundedText(entry.name, RELAY_CATALOG_LIMITS.name);
  const path = cleanBoundedText(entry.path, RELAY_CATALOG_LIMITS.path);
  if (!name || !path) return null;
  const id = optionalId(entry.id);
  if (entry.id !== undefined && entry.id !== null && id === null) return null;
  return Object.freeze({ id, name, path });
}

function parseWorkspace(entry: unknown): RelayWorkspace | null {
  if (!isRecord(entry)) return null;
  const name = cleanBoundedText(entry.name, RELAY_CATALOG_LIMITS.name);
  const path = cleanBoundedText(entry.path, RELAY_CATALOG_LIMITS.path);
  if (!name || !path) return null;
  const branch = optionalBoundedText(entry.branch, RELAY_CATALOG_LIMITS.branch);
  const project = optionalBoundedText(entry.project, RELAY_CATALOG_LIMITS.path);
  const projectId = optionalId(entry.project_id ?? entry.projectId);
  if (entry.branch !== undefined && entry.branch !== null && branch === null) return null;
  if (entry.project !== undefined && entry.project !== null && project === null) return null;
  if ((entry.project_id !== undefined && entry.project_id !== null) ||
      (entry.projectId !== undefined && entry.projectId !== null)) {
    if (projectId === null) return null;
  }
  const id = optionalId(entry.id);
  if (entry.id !== undefined && entry.id !== null && id === null) return null;
  return Object.freeze({ id, name, path, branch, project, project_id: projectId, team: parseWorkspaceTeam(entry.team) });
}

/** A workspace's coordination: a malformed one reads as none rather than dropping the workspace. */
function parseWorkspaceTeam(value: unknown): RelayWorkspaceTeam | null {
  if (!isRecord(value)) return null;
  const id = cleanBoundedText(value.id, RELAY_CATALOG_LIMITS.name);
  const title = cleanBoundedText(value.title, RELAY_CATALOG_LIMITS.title);
  if (!id || !title) return null;
  const task = optionalBoundedText(value.task, RELAY_CATALOG_LIMITS.title) ?? "";
  return Object.freeze({ id, title, lead: value.lead === true, task });
}

/**
 * Dragent live context degrades to null rather than rejecting the tab: newer
 * desktops may widen these display fields, and the phone must never drop a
 * whole catalog over a status string. Missing fields (older desktops) read
 * the same as empty ones.
 */
function parseAgentWork(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text === "" || text.length > RELAY_CATALOG_LIMITS.title) return null;
  return text;
}

function parseAgentStep(value: unknown): string | null {
  return parseAgentWork(value);
}

function parseAgentActivityEntry(entry: unknown): RelayAgentActivity | null {
  if (!isRecord(entry)) return null;
  const sequence = entry.sequence;
  if (typeof sequence !== "number" || !Number.isSafeInteger(sequence) || sequence < 0) return null;
  const kind = cleanBoundedText(entry.kind, RELAY_CATALOG_LIMITS.agentActivityKind);
  if (kind === null) return null;
  if (typeof entry.text !== "string") return null;
  // Defensive client-side re-truncation on a char boundary; the desktop
  // already caps text, so this only bites on host skew.
  const text = truncateChars(entry.text.trim(), RELAY_CATALOG_LIMITS.agentActivityText);
  if (text === "") return null;
  return Object.freeze({ sequence, kind, text });
}

function parseAgentActivityList(value: unknown): readonly RelayAgentActivity[] | null {
  if (value === undefined || value === null) return null;
  if (!Array.isArray(value)) return null;
  const entries: RelayAgentActivity[] = [];
  for (const item of value) {
    const parsed = parseAgentActivityEntry(item);
    if (parsed) entries.push(parsed);
  }
  if (entries.length === 0) return null;
  entries.sort((a, b) => a.sequence - b.sequence);
  // Keep the most recent on host skew; the desktop already bounds the list.
  if (entries.length > RELAY_CATALOG_LIMITS.agentActivity) {
    return Object.freeze(entries.slice(entries.length - RELAY_CATALOG_LIMITS.agentActivity));
  }
  return Object.freeze(entries);
}

function parseTab(entry: unknown): RelayTab | null {
  if (!isRecord(entry) || !isRelayTabKind(entry.kind)) return null;
  if (typeof entry.id !== "number" || !Number.isSafeInteger(entry.id) || entry.id < 0 ||
      entry.id > RELAY_CATALOG_LIMITS.maxTabId) return null;
  let title = "";
  if (entry.title !== undefined && entry.title !== null) {
    if (typeof entry.title !== "string" || entry.title.length > RELAY_CATALOG_LIMITS.title) return null;
    title = entry.title;
  }
  const workspaceId = optionalId(entry.workspace_id ?? entry.workspaceId);
  if ((entry.workspace_id !== undefined && entry.workspace_id !== null) ||
      (entry.workspaceId !== undefined && entry.workspaceId !== null)) {
    if (workspaceId === null) return null;
  }
  const projectId = optionalId(entry.project_id ?? entry.projectId);
  if ((entry.project_id !== undefined && entry.project_id !== null) ||
      (entry.projectId !== undefined && entry.projectId !== null)) {
    if (projectId === null) return null;
  }
  // Agent identity is explicit only: a terminal pane without an explicit
  // agent_kind stays terminal; unknown agent detection is never inferred.
  const agentKind = optionalBoundedText(entry.agent_kind ?? entry.agentKind, RELAY_CATALOG_LIMITS.agentKind);
  if ((entry.agent_kind !== undefined && entry.agent_kind !== null) ||
      (entry.agentKind !== undefined && entry.agentKind !== null)) {
    if (agentKind === null) return null;
  }
  let agentState: RelayAgentState | null = null;
  const rawState = entry.agent_state ?? entry.agentState;
  if (rawState !== undefined && rawState !== null) {
    if (!isRelayAgentState(rawState)) return null;
    agentState = rawState;
  }
  const rawAttention = entry.attention;
  const attention: RelayAttention | null =
    agentState !== null && (rawAttention === "needs_input" || rawAttention === "failed")
      ? rawAttention
      : null;
  // Dragent live context rides agent tabs only (the desktop rejects it on
  // other kinds at publish). A host sending it elsewhere is skew: ignore it
  // rather than dropping the tab.
  const isAgent = entry.kind === "agent";
  const agentWork = isAgent ? parseAgentWork(entry.agent_work ?? entry.agentWork) : null;
  const agentStep = isAgent ? parseAgentStep(entry.agent_step ?? entry.agentStep) : null;
  const agentActivity = isAgent ? parseAgentActivityList(entry.agent_activity ?? entry.agentActivity) : null;
  return Object.freeze({
    id: entry.id,
    kind: entry.kind,
    title,
    active: entry.active === true,
    visible: entry.visible === true,
    workspace_id: workspaceId,
    project_id: projectId,
    agent_kind: agentKind,
    agent_state: agentState,
    attention,
    agent_work: agentWork,
    agent_step: agentStep,
    agent_activity: agentActivity,
  });
}

/**
 * True when the tab is the built-in Dragent agent: an agent tab whose
 * explicit identity is `dragent`. Terminal CLIs stay on the PTY even when
 * they work; only Dragent renders the activity feed.
 */
export function isDragentTab(tab: RelayTab): boolean {
  return (
    tab.kind === "agent" &&
    tab.agent_kind !== null &&
    tab.agent_kind.trim().toLowerCase() === "dragent"
  );
}

/**
 * Phone rendering of the desktop header chip: `Dragent · <work> · <step>`,
 * omitting missing segments. Null for non-Dragent tabs. Missing context
 * reads as bare `Dragent` (older desktops send no context at all).
 */
export function dragentStatusLine(tab: RelayTab): string | null {
  if (!isDragentTab(tab)) return null;
  const segments = ["Dragent"];
  if (tab.agent_work) segments.push(tab.agent_work);
  if (tab.agent_step) segments.push(tab.agent_step);
  return segments.join(" · ");
}

/**
 * Recent Dragent pipeline entries for the feed, oldest-first. Empty when
 * there is nothing to show (older desktops, idle agents, non-Dragent tabs):
 * callers fall back to the PTY view then.
 */
export function dragentActivity(tab: RelayTab): readonly RelayAgentActivity[] {
  if (!isDragentTab(tab)) return EMPTY_AGENT_ACTIVITY;
  return tab.agent_activity ?? EMPTY_AGENT_ACTIVITY;
}

const EMPTY_AGENT_ACTIVITY: readonly RelayAgentActivity[] = Object.freeze([]);

function parseMachine(entry: unknown): RelayMachine | null {
  if (entry === undefined || entry === null) return null;
  if (!isRecord(entry)) return null;
  // After the E2EE cut-over the relay knows no names or platforms (§2.2):
  // they come from pairing and the sealed catalog. Absent reads as "".
  const name = entry.name === undefined ? "" : cleanBoundedText(entry.name, RELAY_CATALOG_LIMITS.name);
  const platform = entry.platform === undefined ? "" : cleanBoundedText(entry.platform, RELAY_CATALOG_LIMITS.platform);
  if (name === null || platform === null) return null;
  const version = optionalBoundedText(entry.version, RELAY_CATALOG_LIMITS.version);
  if (entry.version !== undefined && entry.version !== null && version === null) return null;
  return Object.freeze({ name, platform, version });
}

/** Validate an unknown payload as a relay catalog; null when it is not one. */
export function parseRelayCatalog(raw: unknown): RelayCatalog | null {
  if (!isRecord(raw)) return null;
  if (!Array.isArray(raw.projects) || !Array.isArray(raw.workspaces) || !Array.isArray(raw.tabs)) return null;
  if (raw.projects.length > RELAY_CATALOG_LIMITS.projects ||
      raw.workspaces.length > RELAY_CATALOG_LIMITS.workspaces ||
      raw.tabs.length > RELAY_CATALOG_LIMITS.tabs) return null;
  const projects: RelayProject[] = [];
  for (const entry of raw.projects) {
    const project = parseProject(entry);
    if (!project) return null;
    projects.push(project);
  }
  const workspaces: RelayWorkspace[] = [];
  for (const entry of raw.workspaces) {
    const workspace = parseWorkspace(entry);
    if (!workspace) return null;
    workspaces.push(workspace);
  }
  const tabs: RelayTab[] = [];
  for (const entry of raw.tabs) {
    const tab = parseTab(entry);
    if (!tab) return null;
    tabs.push(tab);
  }
  let machine: RelayMachine | null = null;
  if (raw.machine !== undefined && raw.machine !== null) {
    machine = parseMachine(raw.machine);
    if (!machine) return null;
  }
  return Object.freeze({
    machine,
    projects: Object.freeze(projects),
    workspaces: Object.freeze(workspaces),
    tabs: Object.freeze(tabs),
  });
}

/** Validate one machines.list entry; null when it is not one. */
export function parseMachineSummary(entry: unknown): MachineSummary | null {
  if (!isRecord(entry)) return null;
  const deviceId = cleanBoundedText(entry.device_id ?? entry.deviceId, RELAY_CATALOG_LIMITS.deviceId);
  if (!deviceId) return null;
  // After the E2EE cut-over the relay knows no names or platforms (§2.2):
  // they come from pairing and the sealed catalog. Absent reads as "".
  const name = entry.name === undefined ? "" : cleanBoundedText(entry.name, RELAY_CATALOG_LIMITS.name);
  const platform = entry.platform === undefined ? "" : cleanBoundedText(entry.platform, RELAY_CATALOG_LIMITS.platform);
  if (name === null || platform === null) return null;
  const version = optionalBoundedText(entry.version, RELAY_CATALOG_LIMITS.version);
  if (entry.version !== undefined && entry.version !== null && version === null) return null;
  const seen = entry.last_seen_at ?? entry.lastSeenAt;
  if (typeof seen !== "number" || !Number.isSafeInteger(seen) || seen < 0) return null;
  if (typeof entry.online !== "boolean") return null;
  return Object.freeze({
    device_id: deviceId,
    name,
    platform,
    version,
    last_seen_at: seen,
    online: entry.online,
  });
}

/** Validate a machines.list result payload; null when it is not one. */
export function parseMachinesListResult(raw: unknown): readonly MachineSummary[] | null {
  if (!isRecord(raw) || !Array.isArray(raw.machines)) return null;
  if (raw.machines.length > RELAY_CATALOG_LIMITS.machines) return null;
  const out: MachineSummary[] = [];
  for (const entry of raw.machines) {
    const machine = parseMachineSummary(entry);
    if (!machine) return null;
    out.push(machine);
  }
  return Object.freeze(out);
}

export interface RelayCatalogSnapshot {
  readonly catalog: RelayCatalog;
  readonly machine: MachineSummary;
}

/**
 * Validate a catalog stream payload: seq1 and every replacement carry the
 * full {catalog, machine} pair. Null when it is not one.
 */
export function parseRelayCatalogSnapshotPayload(raw: unknown): RelayCatalogSnapshot | null {
  if (!isRecord(raw)) return null;
  const catalog = parseRelayCatalog(raw.catalog);
  const machine = parseMachineSummary(raw.machine);
  if (!catalog || !machine) return null;
  return Object.freeze({ catalog, machine });
}

/**
 * Fetch the paired phone's catalog for one of its desktops. Each call
 * resolves a fresh device access token (mirroring the relay socket's
 * per-dial auth) and never puts tokens or device secrets into thrown
 * messages.
 */
export async function fetchRelayCatalog(options: RelayCatalogOptions): Promise<RelayCatalog> {
  const base = resolveRelayCatalogURL(options.gateway);
  const deviceId = options.deviceId?.trim();
  if (!base || !deviceId) {
    throw new RelayCatalogError("The relay catalog address is invalid.", null, false);
  }
  const resolve = options.resolveAccessToken;
  const accessToken = resolve ? await resolve().catch(() => null) : (options.accessToken ?? null);
  const headers: Record<string, string> = {};
  if (typeof accessToken === "string" && accessToken !== "") {
    headers.Authorization = `Bearer ${accessToken}`;
  }
  const fetchFn: RelayCatalogFetch = options.fetchFn ?? ((url, init) => {
    const impl = (globalThis as { fetch?: unknown }).fetch;
    if (typeof impl !== "function") throw new RelayCatalogError("Catalog fetch is unavailable.", null, false);
    const request: RequestInit = {};
    if (init?.headers !== undefined) request.headers = init.headers;
    return (impl as typeof fetch)(url, request) as unknown as Promise<{ status: number; json(): Promise<unknown> }>;
  });
  let response: { status: number; json(): Promise<unknown> };
  try {
    response = await fetchFn(`${base}?device_id=${encodeURIComponent(deviceId)}`, { headers });
  } catch {
    // Fetch failures can carry URLs or headers. Keep them out of UI state.
    throw new RelayCatalogError("Could not reach the relay catalog.", null, false);
  }
  if (response.status === 401) throw new RelayCatalogError("Pair this phone to view this desktop.", 401, true);
  if (response.status === 404) throw new RelayCatalogError("This machine has not shared its catalog.", 404, false);
  if (response.status < 200 || response.status >= 300) {
    throw new RelayCatalogError("The relay catalog request failed.", response.status, false);
  }
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    throw new RelayCatalogError("The relay catalog response was unreadable.", response.status, false);
  }
  const catalog = parseRelayCatalog(payload);
  if (!catalog) throw new RelayCatalogError("The relay catalog response was unreadable.", response.status, false);
  return catalog;
}
