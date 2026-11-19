/**
 * Typed relay RPC methods over the R0 transport core (`relayRpcClient.ts`).
 *
 * Split of responsibilities, mirroring the fleet briefs: the core owns the
 * wire (correlated requests, stream registry, resubscribe, epoch resets);
 * this module owns the method table, strict hand-written readers (same
 * style as `relayCatalog.ts`: malformed input returns null, frozen
 * outputs, user-safe errors only), and per-method calls. It never dials,
 * never retries, and never touches UI.
 */

import {
  RelayRpcClient,
  RelayRpcError,
  type RelaySubscription,
} from "./relayRpcClient";
import {
  parseMachinesListResult,
  parseMachineSummary,
  parseRelayCatalog,
  parseRelayCatalogSnapshotPayload,
  type MachineSummary,
  type RelayCatalog,
  type RelayCatalogSnapshot,
} from "./relayCatalog";
import { isValidRequestId, utf8ByteLength } from "./frames";
import { CATALOG_REJECTED_MESSAGE, type SealedCatalogResult } from "./e2ee/sealedCatalog";
import { STORE_LOST_MESSAGE } from "./e2ee/security";

export type RelayRpcMethod =
  | "machines.list"
  | "catalog.read"
  | "tabs.list"
  | "tabs.createTerminal"
  | "tabs.create"
  | "tabs.attach"
  | "tabs.read"
  | "tabs.screen"
  | "tabs.input"
  | "tabs.resize"
  | "tabs.detach"
  | "tabs.seen"
  | "agent.send"
  | "tabs.send"
  | "agent.transcript"
  | "files.image"
  | "agent.act"
  | "worktree.list"
  | "worktree.create"
  | "workspace.delete"
  | "files.list"
  | "files.search"
  | "files.open"
  | "git.status"
  | "git.diff"
  | "git.stage"
  | "git.unstage"
  | "git.discard"
  | "git.commit"
  | "git.push"
  | "git.log"
  | "git.branches"
  | "git.switch"
  | "git.pr_create"
  | "browser.act"
  | "github.listWorkItems"
  | "gitlab.listWorkItems";

const RELAY_RPC_METHODS: readonly RelayRpcMethod[] = Object.freeze([
  "machines.list",
  "catalog.read",
  "tabs.list",
  "tabs.createTerminal",
  "tabs.create",
  "tabs.attach",
  "tabs.read",
  "tabs.screen",
  "tabs.input",
  "tabs.resize",
  "tabs.detach",
  "agent.send",
  "tabs.send",
  "agent.transcript",
  "files.image",
  "agent.act",
  "worktree.list",
  "worktree.create",
  "workspace.delete",
  "files.list",
  "files.search",
  "files.open",
  "git.status",
  "git.diff",
  "git.stage",
  "git.unstage",
  "git.discard",
  "git.commit",
  "git.push",
  "git.log",
  "git.branches",
  "git.switch",
  "git.pr_create",
  "browser.act",
  "github.listWorkItems",
  "gitlab.listWorkItems",
]);

export function isRelayRpcMethod(value: unknown): value is RelayRpcMethod {
  return typeof value === "string" && (RELAY_RPC_METHODS as readonly string[]).includes(value);
}

export type RelayRpcTabKind = "terminal" | "browser" | "settings";

const RELAY_RPC_TAB_KINDS: readonly RelayRpcTabKind[] = Object.freeze(["terminal", "browser", "settings"]);

export function isRelayRpcTabKind(value: unknown): value is RelayRpcTabKind {
  return typeof value === "string" && (RELAY_RPC_TAB_KINDS as readonly string[]).includes(value);
}

export interface RelayRpcTab {
  readonly id: number;
  readonly kind: RelayRpcTabKind;
  readonly title: string;
  readonly active: boolean;
  readonly visible: boolean;
}

export interface RelayRpcTerminalRef {
  readonly id: number;
}

export interface RelayRpcWorktree {
  readonly name: string;
  readonly path: string;
  readonly branch: string | null;
  readonly project: string | null;
}

export interface RelayRpcCreateWorktreeInput {
  readonly path?: string;
  readonly name?: string;
  readonly branch?: string;
}

export type RelayRpcFileKind = "file" | "directory";

export interface RelayRpcFileEntry {
  readonly name: string;
  readonly kind: RelayRpcFileKind;
  /** Absolute path; only search results carry it. */
  readonly path?: string;
}

export interface RelayRpcFileContent {
  readonly content: string;
  readonly truncated: boolean;
}

export interface RelayRpcWorkItem {
  readonly id: string;
  readonly number: number;
  readonly title: string;
  readonly url: string;
  readonly state: string;
  readonly repo: string;
}

export interface RelayRpcListWorkItemsInput {
  readonly query?: string;
  readonly repo?: string;
  readonly project?: string;
}

/** Attention kinds mirror `server/internal/push` (closed set). */
export type RelayRpcAttentionKind = "join_approved" | "control_granted" | "agent_completed";

const RELAY_RPC_ATTENTION_KINDS: readonly RelayRpcAttentionKind[] = Object.freeze([
  "join_approved",
  "control_granted",
  "agent_completed",
]);

export interface RelayRpcAttentionEvent {
  readonly kind: RelayRpcAttentionKind;
  readonly title: string;
  readonly body: string;
  readonly sessionId: string | null;
  readonly occurredAt: string | null;
  readonly data: Readonly<Record<string, string>>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function cleanText(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  return text === "" ? null : text;
}

function optionalText(value: unknown): string | null {
  if (value === undefined || value === null) return null;
  return cleanText(value);
}

function unsignedId(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function boundedList(value: unknown, limit: number): unknown[] | null {
  return Array.isArray(value) && value.length <= limit ? value : null;
}

function parseRpcTab(entry: unknown): RelayRpcTab | null {
  if (!isRecord(entry) || !isRelayRpcTabKind(entry.kind)) return null;
  if (unsignedId(entry.id) === null) return null;
  const title = typeof entry.title === "string" ? entry.title : null;
  if (title === null) return null;
  return Object.freeze({
    id: entry.id as number,
    kind: entry.kind,
    title,
    active: entry.active === true,
    visible: entry.visible === true,
  });
}

function parseRpcWorktree(entry: unknown): RelayRpcWorktree | null {
  if (!isRecord(entry)) return null;
  const name = cleanText(entry.name);
  const path = cleanText(entry.path);
  if (name === null || path === null) return null;
  const branch = optionalText(entry.branch);
  const project = optionalText(entry.project);
  if (entry.branch !== undefined && entry.branch !== null && branch === null) return null;
  if (entry.project !== undefined && entry.project !== null && project === null) return null;
  return Object.freeze({ name, path, branch, project });
}

function parseRpcFileEntry(entry: unknown): RelayRpcFileEntry | null {
  if (!isRecord(entry)) return null;
  const name = cleanText(entry.name);
  if (name === null || name.length > 1024) return null;
  // The host does not follow symlinks; kind distinguishes files from
  // directories. Tolerate the common "dir" alias, reject anything else.
  const rawKind = entry.kind === "dir" ? "directory" : entry.kind;
  if (rawKind !== "file" && rawKind !== "directory") return null;
  const path = typeof entry.path === "string" && entry.path.startsWith("/") && entry.path.length <= 4096 ? entry.path : null;
  return Object.freeze(path === null ? { name, kind: rawKind } : { name, kind: rawKind, path });
}

function parseRpcWorkItem(entry: unknown): RelayRpcWorkItem | null {
  if (!isRecord(entry)) return null;
  const id = cleanText(entry.id);
  const url = cleanText(entry.url);
  const state = cleanText(entry.state);
  const repo = cleanText(entry.repo);
  if (id === null || id.length > 256) return null;
  if (url === null || !/^https?:\/\/\S+$/u.test(url)) return null;
  if (state === null || state.length > 64) return null;
  if (repo === null || repo.length > 256) return null;
  if (unsignedId(entry.number) === null) return null;
  if (typeof entry.title !== "string") return null;
  return Object.freeze({
    id,
    number: entry.number as number,
    title: entry.title,
    url,
    state,
    repo,
  });
}

function parseRpcAttentionEvent(entry: unknown): RelayRpcAttentionEvent | null {
  if (!isRecord(entry)) return null;
  if (typeof entry.kind !== "string" || !(RELAY_RPC_ATTENTION_KINDS as readonly string[]).includes(entry.kind)) {
    return null;
  }
  if (typeof entry.title !== "string" || typeof entry.body !== "string") return null;
  const sessionId = optionalText(entry.sessionId ?? entry.session_id);
  const occurredAt = optionalText(entry.occurredAt ?? entry.occurred_at);
  let data: Record<string, string> = {};
  if (entry.data !== undefined) {
    if (!isRecord(entry.data)) return null;
    for (const [key, val] of Object.entries(entry.data)) {
      if (typeof val !== "string") return null;
      data[key] = val;
    }
    data = Object.freeze(data);
  }
  return Object.freeze({
    kind: entry.kind as RelayRpcAttentionKind,
    title: entry.title,
    body: entry.body,
    sessionId,
    occurredAt,
    data: Object.freeze({ ...data }),
  });
}

/** Validate an unknown payload as a `tabs.list` result; null when not one. */
export function parseRelayRpcTabsResult(raw: unknown): readonly RelayRpcTab[] | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.tabs, 1024);
  if (!list) return null;
  const tabs: RelayRpcTab[] = [];
  for (const entry of list) {
    const tab = parseRpcTab(entry);
    if (!tab) return null;
    tabs.push(tab);
  }
  return Object.freeze(tabs);
}

/** Validate an unknown payload as a `tabs.createTerminal` result. */
export function parseRelayRpcTerminalRef(raw: unknown): RelayRpcTerminalRef | null {
  if (!isRecord(raw) || unsignedId(raw.id) === null) return null;
  return Object.freeze({ id: raw.id as number });
}

/** Validate an unknown payload as a `worktree.list` result. */
export function parseRelayRpcWorktreeListResult(raw: unknown): readonly RelayRpcWorktree[] | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.worktrees, 256);
  if (!list) return null;
  const worktrees: RelayRpcWorktree[] = [];
  for (const entry of list) {
    const worktree = parseRpcWorktree(entry);
    if (!worktree) return null;
    worktrees.push(worktree);
  }
  return Object.freeze(worktrees);
}

/**
 * Validate a `worktree.create` result. The spec returns the created entry
 * bare; a `{ worktree: entry }` wrapper is tolerated for host skew.
 */
export function parseRelayRpcWorktreeEntry(raw: unknown): RelayRpcWorktree | null {
  if (isRecord(raw) && raw.worktree !== undefined) return parseRpcWorktree(raw.worktree);
  return parseRpcWorktree(raw);
}

/** Validate a `files.list` / `files.search` result. */
export function parseRelayRpcFilesResult(raw: unknown): readonly RelayRpcFileEntry[] | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.entries, 2048);
  if (!list) return null;
  const entries: RelayRpcFileEntry[] = [];
  for (const entry of list) {
    const parsed = parseRpcFileEntry(entry);
    if (!parsed) return null;
    entries.push(parsed);
  }
  return Object.freeze(entries);
}

/** Validate a `files.open` result (text only, server-capped). */
export function parseRelayRpcFileContentResult(raw: unknown): RelayRpcFileContent | null {
  if (!isRecord(raw)) return null;
  if (typeof raw.content !== "string" || raw.content.length > 4 * 1024 * 1024) return null;
  if (typeof raw.truncated !== "boolean") return null;
  return Object.freeze({ content: raw.content, truncated: raw.truncated });
}

/** Validate a `github/gitlab.listWorkItems` result. */
export function parseRelayRpcWorkItemsResult(raw: unknown): readonly RelayRpcWorkItem[] | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.items, 500);
  if (!list) return null;
  const items: RelayRpcWorkItem[] = [];
  for (const entry of list) {
    const item = parseRpcWorkItem(entry);
    if (!item) return null;
    items.push(item);
  }
  return Object.freeze(items);
}

/** Validate one attention event payload (live stream frames). */
export function parseRelayRpcAttentionEvent(raw: unknown): RelayRpcAttentionEvent | null {
  return parseRpcAttentionEvent(raw);
}

/** Validate a runtime-events snapshot payload (first frame per subscribe). */
export function parseRelayRpcEventsSnapshot(raw: unknown): readonly RelayRpcAttentionEvent[] | null {
  if (!isRecord(raw)) return null;
  const list = boundedList(raw.events, 256);
  if (!list) return null;
  const events: RelayRpcAttentionEvent[] = [];
  for (const entry of list) {
    const event = parseRpcAttentionEvent(entry);
    if (!event) return null;
    events.push(event);
  }
  return Object.freeze(events);
}

function workItemParams(input: RelayRpcListWorkItemsInput, scopedKey: "repo" | "project"): Record<string, unknown> | null {
  if (!isRecord(input)) return null;
  const params: Record<string, unknown> = {};
  const query = input.query === undefined ? undefined : cleanText(input.query);
  if (input.query !== undefined && query === null) return null;
  if (query !== undefined) params.query = query;
  const scope = input[scopedKey] === undefined ? undefined : cleanText(input[scopedKey]);
  if (input[scopedKey] !== undefined && scope === null) return null;
  if (scope !== undefined) params[scopedKey] = scope;
  for (const key of ["query", "repo", "project"] as const) {
    if (key !== scopedKey && key !== "query" && input[key] !== undefined) return null;
  }
  return params;
}

/** Params cap from the R0 spec: params JSON must fit in 64KB. */
export const RELAY_RPC_MAX_PARAMS_CHARS = 64 * 1024;

/** User-safe failure when a result does not parse. */
export const RELAY_RPC_UNREADABLE_MESSAGE = "The relay response was unreadable.";

function checkedParams(params: Record<string, unknown>): Record<string, unknown> | null {
  let encoded: string;
  try {
    encoded = JSON.stringify(params);
  } catch {
    return null;
  }
  return encoded.length > RELAY_RPC_MAX_PARAMS_CHARS || utf8ByteLength(encoded) > RELAY_RPC_MAX_PARAMS_CHARS ? null : params;
}

async function call<T>(
  client: RelayRpcClient,
  method: string,
  params: Record<string, unknown>,
  parse: (raw: unknown) => T | null,
  opts?: { timeoutMs?: number },
): Promise<T> {
  const body = checkedParams(params);
  if (!body) throw new RelayRpcError("The relay request is invalid.");
  let raw: unknown;
  try {
    raw = await client.request(method, body, opts);
  } catch (error) {
    if (error instanceof RelayRpcError) throw error;
    throw new RelayRpcError("The request failed.");
  }
  const parsed = parse(raw);
  if (parsed === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return parsed;
}

/** `catalog.read` — the authenticated desktop hierarchy. */
export function readRelayCatalog(client: RelayRpcClient, opts?: { timeoutMs?: number }): Promise<RelayCatalog> {
  return call(client, "catalog.read", {}, parseRelayCatalog, opts);
}

/**
 * `machines.list` — account-scoped discovery. No device is required and none
 * is sent: the account socket carries no target.
 */
export function listMachines(client: RelayRpcClient, opts?: { timeoutMs?: number }): Promise<readonly MachineSummary[]> {
  return call(client, "machines.list", {}, parseMachinesListResult, opts);
}

export type { MachineSummary, RelayCatalogSnapshot };

export interface MachineCatalogListener {
  onSnapshot(snapshot: RelayCatalogSnapshot): void;
  onUpdate(snapshot: RelayCatalogSnapshot): void;
  /**
   * §9: a blob from the server didn't check out (rollback, equivocation,
   * wrong sender). The last good copy, if any, stays on screen.
   */
  onRejected?: ((message: string) => void) | undefined;
  /** Initial or midstream subscription failure; surfaces unavailable at once. */
  onError?: ((error: RelayRpcError) => void) | undefined;
}

/**
 * `catalog` subscription — seq1 carries {catalog, machine}, then full
 * replacement payloads on changes. Each payload replaces the previous one;
 * no deltas are lost because every frame is a complete snapshot.
 * Malformed payloads are dropped without breaking the sequence. Subscription
 * failures report through onError so callers leave loading (and never keep
 * stale live state) without waiting on a timeout.
 */
/** Opens a sealed `catalog` stream payload (docs/design/e2ee.md §6.5). */
export type CatalogOpener = (payload: unknown) => SealedCatalogResult;

export const CATALOG_REJECTED_NO_COPY_MESSAGE = "This desktop's data from the server didn't check out, and there's no earlier copy to show.";

/** Development-build diagnostics only; release builds and tests stay silent. */
function devWarn(...args: unknown[]): void {
  if ((globalThis as { __DEV__?: boolean }).__DEV__ === true) console.warn(...args);
}

/** Which part of a decrypted catalog failed to parse — field names and counts only, never content. */
function describeCatalogShape(json: unknown, machine: unknown): Record<string, unknown> {
  const rec = (v: unknown) => (typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null);
  const c = rec(json);
  // Index and field names of the first entry that fails on its own.
  const firstBad = (list: unknown, wrap: (entry: unknown) => unknown) => {
    if (!Array.isArray(list)) return null;
    const i = list.findIndex((entry) => parseRelayCatalog(wrap(entry)) === null);
    return i < 0 ? null : { index: i, keys: rec(list[i]) ? Object.keys(rec(list[i]) as object) : typeof list[i] };
  };
  return {
    catalogKeys: c ? Object.keys(c) : typeof json,
    projects: Array.isArray(c?.projects) ? c?.projects.length : typeof c?.projects,
    workspaces: Array.isArray(c?.workspaces) ? c?.workspaces.length : typeof c?.workspaces,
    tabs: Array.isArray(c?.tabs) ? c?.tabs.length : typeof c?.tabs,
    tabKinds: Array.isArray(c?.tabs) ? [...new Set(((c ? c.tabs : []) as unknown[]).map((t) => rec(t)?.kind))] : [],
    machineSummaryKeys: rec(machine) ? Object.keys(rec(machine) as object) : typeof machine,
    machineSummaryValid: parseMachineSummary(machine) !== null,
    catalogValid: parseRelayCatalog(json) !== null,
    machineValid: c?.machine === undefined || c?.machine === null || parseRelayCatalog({ projects: [], workspaces: [], tabs: [], machine: c ? c.machine : null }) !== null,
    firstBadProject: firstBad(c?.projects, (e) => ({ projects: [e], workspaces: [], tabs: [] })),
    firstBadWorkspace: firstBad(c?.workspaces, (e) => ({ projects: [], workspaces: [e], tabs: [] })),
    firstBadTab: firstBad(c?.tabs, (e) => ({ projects: [], workspaces: [], tabs: [e] })),
  };
}

export const CATALOG_UNVERIFIABLE_MESSAGE = "This desktop's data couldn't be verified on this device.";

/**
 * The relay stores only sealed catalogs (§6.5). Every payload is opened by
 * `open` (the pin store checks the pinned key and the high-water mark); a
 * plaintext or unverifiable payload is never shown. Without `open` nothing
 * can be verified, so the stream reports an error instead.
 */
export function subscribeMachineCatalog(
  client: RelayRpcClient,
  listener: MachineCatalogListener,
  open: CatalogOpener | undefined,
): RelaySubscription {
  let failed = false;
  const fail = (message: string, loginRequired = false) => {
    if (failed) return;
    failed = true;
    listener.onError?.(new RelayRpcError(message, loginRequired));
  };
  let delivered = false;
  const deliver = (snapshot: RelayCatalogSnapshot) => {
    if (!delivered) {
      delivered = true;
      listener.onSnapshot(snapshot);
    } else listener.onUpdate(snapshot);
  };
  return client.subscribe(
    "catalog",
    {},
    (payload, info) => {
      if (info.snapshot) delivered = false;
      if (!open) {
        fail(CATALOG_UNVERIFIABLE_MESSAGE);
        return;
      }
      let result: SealedCatalogResult;
      try {
        result = open(payload);
      } catch {
        fail(CATALOG_UNVERIFIABLE_MESSAGE);
        return;
      }
      switch (result.kind) {
        case "catalog": {
          const snapshot = parseRelayCatalogSnapshotPayload({ catalog: result.json, machine: result.machine });
          if (snapshot) deliver(snapshot);
          else devWarn("[catalog] opened, but the catalog's shape didn't parse", describeCatalogShape(result.json, result.machine));
          return;
        }
        case "rejected": {
          devWarn(`[catalog] rejected (${result.reason})`);
          listener.onRejected?.(CATALOG_REJECTED_MESSAGE);
          const last = result.lastGood === null ? null : parseRelayCatalogSnapshotPayload({ catalog: result.lastGood, machine: result.machine });
          if (last) deliver(last);
          // Nothing good to show yet: say so now instead of waiting out the
          // loading deadline as "unreachable". Not terminal — a later blob
          // that checks out still replaces this.
          else if (!delivered) listener.onError?.(new RelayRpcError(CATALOG_REJECTED_NO_COPY_MESSAGE, false));
          return;
        }
        case "unpaired":
          fail("Pair this desktop to open it.", true);
          return;
        case "lost":
          fail(STORE_LOST_MESSAGE, true);
          return;
        case "unreadable":
          devWarn("[catalog] payload unreadable (no e2ee security, no desktop id, or not a sealed blob)");
          return;
      }
    },
    listener.onError,
  );
}

/** `tabs.list` — all open tabs on the machine. */
export function listTabs(client: RelayRpcClient, opts?: { timeoutMs?: number }): Promise<readonly RelayRpcTab[]> {
  return call(client, "tabs.list", {}, parseRelayRpcTabsResult, opts);
}

/** `tabs.createTerminal` — open a terminal tab, returning its ref. */
export function createTerminal(client: RelayRpcClient, opts?: { timeoutMs?: number }): Promise<RelayRpcTerminalRef> {
  return call(client, "tabs.createTerminal", {}, parseRelayRpcTerminalRef, opts);
}

/** `worktree.list` — the host's workspaces. */
/** What the desktop's new-tab menu can open from the phone. */
export type RelayNewTabKind = "terminal" | "browser" | "dragent_cli";

const RELAY_NEW_TAB_KINDS: readonly RelayNewTabKind[] = Object.freeze(["terminal", "browser", "dragent_cli"]);

export function isRelayNewTabKind(value: unknown): value is RelayNewTabKind {
  return typeof value === "string" && (RELAY_NEW_TAB_KINDS as readonly string[]).includes(value);
}

/** Longest catalog workspace wire id the relay accepts (they look like "w0w3"). */
export const RELAY_NEW_TAB_MAX_WORKSPACE_ID = 64;

/**
 * Params for `tabs.create`, or null when they can't be sent: the workspace
 * must be a published catalog wire id (legacy path-keyed workspaces have
 * none) and the kind one of the menu's three.
 */
export function newTabParams(
  workspaceId: unknown,
  kind: unknown,
): { readonly workspace_id: string; readonly kind: RelayNewTabKind } | null {
  if (
    typeof workspaceId !== "string" ||
    workspaceId.length === 0 ||
    workspaceId.length > RELAY_NEW_TAB_MAX_WORKSPACE_ID ||
    !/^[A-Za-z0-9]+$/.test(workspaceId) ||
    !isRelayNewTabKind(kind)
  ) {
    return null;
  }
  return Object.freeze({ workspace_id: workspaceId, kind });
}

/** Validate an unknown payload as a `tabs.create` result: the new tab's catalog id. */
export function parseRelayRpcNewTabResult(raw: unknown): { readonly id: number } | null {
  if (!isRecord(raw) || Object.keys(raw).length !== 1) return null;
  const id = unsignedId(raw.id);
  return id === null ? null : Object.freeze({ id });
}

/** Opening a tab waits on the desktop's UI; give it longer than a read. */
export const RELAY_NEW_TAB_TIMEOUT_MS = 20000;

/**
 * `tabs.create {workspace_id, kind}` — the desktop opens that kind of tab in
 * the workspace (in the background) and answers its catalog wire id.
 */
export function createWorkspaceTab(
  client: RelayRpcClient,
  workspaceId: string,
  kind: RelayNewTabKind,
  opts?: { timeoutMs?: number },
): Promise<{ readonly id: number }> {
  const params = newTabParams(workspaceId, kind);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "tabs.create", { ...params }, parseRelayRpcNewTabResult, {
    timeoutMs: opts?.timeoutMs ?? RELAY_NEW_TAB_TIMEOUT_MS,
  });
}

export function listWorktrees(client: RelayRpcClient, opts?: { timeoutMs?: number }): Promise<readonly RelayRpcWorktree[]> {
  return call(client, "worktree.list", {}, parseRelayRpcWorktreeListResult, opts);
}

/** `worktree.create` — create a workspace; blank fields are dropped. */
export function createWorktree(
  client: RelayRpcClient,
  input: RelayRpcCreateWorktreeInput,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcWorktree> {
  const params: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(input)) {
    if (value === undefined) continue;
    if (typeof value !== "string" || value.trim() === "") {
      return Promise.reject(new RelayRpcError("The relay request is invalid."));
    }
    params[key] = value;
  }
  return call(client, "worktree.create", params, parseRelayRpcWorktreeEntry, opts);
}

/**
 * What deleting a workspace did. Without `force` the desktop refuses while
 * agents still work in it or (for a worktree) files are uncommitted, and
 * says which, so the phone can ask before deleting anyway.
 */
export type WorkspaceDeleteResult =
  | { readonly deleted: true; /** The project's own checkout: closed in Dragent, files kept. */ readonly keptFiles: boolean }
  | { readonly deleted: false; readonly agents: readonly string[]; readonly changes: number };

export function parseWorkspaceDeleteResult(raw: unknown): WorkspaceDeleteResult | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const value = raw as Record<string, unknown>;
  if (value.deleted === true) {
    return Object.freeze({ deleted: true as const, keptFiles: value.kept_files === true });
  }
  if (value.deleted !== false) return null;
  const agents = Array.isArray(value.agents)
    ? value.agents.filter((agent): agent is string => typeof agent === "string" && agent !== "").slice(0, 20)
    : [];
  const changes = typeof value.changes === "number" && Number.isSafeInteger(value.changes) && value.changes >= 0 ? value.changes : 0;
  if (agents.length === 0 && changes === 0) return null;
  return Object.freeze({ deleted: false as const, agents: Object.freeze(agents), changes });
}

/**
 * `workspace.delete {workspace_id, force}`: what deleting it on the desktop
 * does (a worktree's folder is removed; the project's own checkout is only
 * closed). Slow git work can take a while, hence the long deadline.
 */
export function deleteWorkspace(
  client: RelayRpcClient,
  workspaceId: string,
  force: boolean,
): Promise<WorkspaceDeleteResult> {
  if (!/^[A-Za-z0-9]{1,64}$/.test(workspaceId)) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "workspace.delete", { workspace_id: workspaceId, force }, parseWorkspaceDeleteResult, {
    timeoutMs: 45_000,
  });
}

function cleanPath(path: string): Record<string, unknown> | null {
  if (typeof path !== "string" || path.trim() === "") return null;
  return { path };
}

/** `files.list` — entries of one directory; symlinks are never followed. */
export function listFiles(
  client: RelayRpcClient,
  path: string,
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcFileEntry[]> {
  const params = cleanPath(path);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "files.list", params, parseRelayRpcFilesResult, opts);
}

/** `files.search` — bounded filename search. */
export function searchFiles(
  client: RelayRpcClient,
  query: string,
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcFileEntry[]> {
  if (typeof query !== "string" || query.trim() === "") {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "files.search", { query }, parseRelayRpcFilesResult, opts);
}

/** `files.open` — text content with a server-enforced cap. */
export function openFile(
  client: RelayRpcClient,
  path: string,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcFileContent> {
  const params = cleanPath(path);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "files.open", params, parseRelayRpcFileContentResult, opts);
}

/** `github.listWorkItems` — provider search stays server-side; only search parameters cross. */
export function listGithubWorkItems(
  client: RelayRpcClient,
  input: RelayRpcListWorkItemsInput = {},
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcWorkItem[]> {
  const params = workItemParams(input, "repo");
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "github.listWorkItems", params, parseRelayRpcWorkItemsResult, opts);
}

/** `gitlab.listWorkItems` — same contract as the GitHub variant. */
export function listGitlabWorkItems(
  client: RelayRpcClient,
  input: RelayRpcListWorkItemsInput = {},
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcWorkItem[]> {
  const params = workItemParams(input, "project");
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "gitlab.listWorkItems", params, parseRelayRpcWorkItemsResult, opts);
}

/**
 * Companion terminal bridge (contract `terminal-bridge-contract.md` + screen
 * extension). Strict readers and bounded calls for the per-tab host PTY:
 * attach/read/screen/input/resize/detach over the existing companion socket.
 * Detach releases transport only and never kills the host PTY. No retries or
 * replay live here; the hook owns sequencing, masking and cleanup.
 */
export const COMPANION_TERMINAL_LIMITS = Object.freeze({
  maxTabId: 9007199254740991,
  minGrid: 1,
  maxRows: 240,
  maxCols: 512,
  maxCells: 65_536,
  readDecodedBytes: 64 * 1024,
  inputUtf8Bytes: 16 * 1024,
  screenLines: 240,
  spansPerLine: 128,
  lineCharacters: 8192,
});

export interface RelayRpcTabsAttachResult {
  readonly alternateScreen?: boolean;
  readonly mouseReporting?: boolean;
  readonly sgrMouse?: boolean;
  readonly bracketedPaste?: boolean;
  readonly handle: string;
  readonly cols: number;
  readonly rows: number;
}

export interface RelayRpcTabsReadResult {
  readonly base: number;
  readonly total: number;
  readonly data: string;
  readonly alive: boolean;
  readonly exitCode: number | null;
}

export interface RelayRpcTabsScreenDelta {
  readonly kind: "alt_screen";
  readonly rows: number;
  readonly cols: number;
  readonly cursor_row: number;
  readonly cursor_col: number;
  readonly cursor_visible: boolean;
  readonly lines: readonly unknown[];
}

export interface RelayRpcTabsScreenResult {
  readonly complete: boolean;
  readonly total: number;
  readonly delta: RelayRpcTabsScreenDelta | null;
  readonly alternateScreen?: boolean;
  readonly mouseReporting?: boolean;
  readonly sgrMouse?: boolean;
  readonly bracketedPaste?: boolean;
}

function companionTabId(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 0 &&
    value <= COMPANION_TERMINAL_LIMITS.maxTabId
    ? value
    : null;
}

function companionOffset(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function companionGrid(rows: unknown, cols: unknown): { rows: number; cols: number } | null {
  if (
    typeof rows !== "number" ||
    typeof cols !== "number" ||
    !Number.isSafeInteger(rows) ||
    !Number.isSafeInteger(cols) ||
    rows < COMPANION_TERMINAL_LIMITS.minGrid ||
    cols < COMPANION_TERMINAL_LIMITS.minGrid ||
    rows > COMPANION_TERMINAL_LIMITS.maxRows ||
    cols > COMPANION_TERMINAL_LIMITS.maxCols ||
    rows * cols > COMPANION_TERMINAL_LIMITS.maxCells
  ) {
    return null;
  }
  return { rows, cols };
}

/**
 * Terminal handles are minted by the desktop: a u32 inside the end-to-end
 * channel (docs/design/e2ee.md §4.4 TERM, §4.6). The phone keeps them as
 * opaque decimal strings, so screens are unchanged; the wire carries the
 * number. A legacy UUID handle still reads (it never reaches a desktop).
 */
function companionHandle(value: unknown): string | null {
  if (typeof value === "number") {
    return Number.isSafeInteger(value) && value > 0 && value <= 0xffffffff ? String(value) : null;
  }
  if (typeof value !== "string") return null;
  if (/^[1-9][0-9]{0,9}$/u.test(value) && Number(value) <= 0xffffffff) return value;
  return isValidRequestId(value) ? value : null;
}

/** The handle as the desktop expects it: the u32 for a desktop-minted handle. */
function wireHandle(handle: string): number | string {
  return /^[1-9][0-9]{0,9}$/u.test(handle) ? Number(handle) : handle;
}

// A flat character class: the grouped form recursed per quantum and blew the
// stack on multi-megabyte seeds (§4.5 allows 16 MiB).
const COMPANION_BASE64_RE = /^[A-Za-z0-9+/]*={0,2}$/u;

function companionBase64ByteLength(data: string): number | null {
  if (data === "") return 0;
  if (data.length % 4 !== 0 || !COMPANION_BASE64_RE.test(data)) return null;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return (data.length / 4) * 3 - padding;
}

function companionColor(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.length === 4 &&
    value.every((channel) => typeof channel === "number" && Number.isSafeInteger(channel) && channel >= 0 && channel <= 255)
  );
}

function companionSpan(value: unknown): boolean {
  if (!isRecord(value) || typeof value.text !== "string") return false;
  if (value.text.length > COMPANION_TERMINAL_LIMITS.lineCharacters) return false;
  if (value.fg !== undefined && !companionColor(value.fg)) return false;
  if (value.bg !== undefined && !companionColor(value.bg)) return false;
  if (value.flags !== undefined && !(typeof value.flags === "number" && Number.isSafeInteger(value.flags) && value.flags >= 0)) {
    return false;
  }
  return true;
}

function companionScreenLine(value: unknown): boolean {
  if (!isRecord(value) || !Array.isArray(value.spans)) return false;
  if (value.spans.length > COMPANION_TERMINAL_LIMITS.spansPerLine) return false;
  return value.spans.every(companionSpan);
}

/** Strict `alt_screen` grid: authoritative desktop projection, no line index. */
function parseCompanionScreenDelta(raw: unknown): RelayRpcTabsScreenDelta | null {
  if (!isRecord(raw) || raw.kind !== "alt_screen") return null;
  const grid = companionGrid(raw.rows, raw.cols);
  if (!grid) return null;
  if (companionOffset(raw.cursor_row) === null || companionOffset(raw.cursor_col) === null) return null;
  if (typeof raw.cursor_visible !== "boolean") return null;
  if (!Array.isArray(raw.lines) || raw.lines.length > COMPANION_TERMINAL_LIMITS.screenLines) return null;
  if (raw.lines.length > grid.rows) return null;
  for (const entry of raw.lines) {
    if (!companionScreenLine(entry)) return null;
  }
  return Object.freeze({
    kind: "alt_screen" as const,
    rows: grid.rows,
    cols: grid.cols,
    cursor_row: raw.cursor_row as number,
    cursor_col: raw.cursor_col as number,
    cursor_visible: raw.cursor_visible as boolean,
    lines: Object.freeze([...(raw.lines as readonly unknown[])]),
  });
}

/** Optional screen modes: absent means false, wrong type is unreadable. */
function companionOptionalMode(value: unknown): boolean | undefined | null {
  if (value === undefined) return undefined;
  if (typeof value === "boolean") return value;
  return null;
}

type CompanionModePatch = Pick<
  RelayRpcTabsAttachResult,
  "alternateScreen" | "mouseReporting" | "sgrMouse" | "bracketedPaste"
>;

function companionScreenModes(raw: Record<string, unknown>): CompanionModePatch | null {
  const alternateScreen = companionOptionalMode(raw.alternate_screen);
  const mouseReporting = companionOptionalMode(raw.mouse_reporting);
  const sgrMouse = companionOptionalMode(raw.sgr_mouse);
  const bracketedPaste = companionOptionalMode(raw.bracketed_paste);
  if (alternateScreen === null || mouseReporting === null || sgrMouse === null || bracketedPaste === null) {
    return null;
  }
  return Object.freeze({
    ...(alternateScreen !== undefined ? { alternateScreen } : {}),
    ...(mouseReporting !== undefined ? { mouseReporting } : {}),
    ...(sgrMouse !== undefined ? { sgrMouse } : {}),
    ...(bracketedPaste !== undefined ? { bracketedPaste } : {}),
  });
}

/** Validate a `tabs.attach` result: opaque handle plus initial grid. */
export function parseRelayRpcTabsAttachResult(raw: unknown): RelayRpcTabsAttachResult | null {
  if (!isRecord(raw)) return null;
  const handle = companionHandle(raw.handle);
  if (!handle) return null;
  const grid = companionGrid(raw.rows, raw.cols);
  if (!grid) return null;
  const modes = companionScreenModes(raw);
  if (!modes) return null;
  return Object.freeze({ handle, cols: grid.cols, rows: grid.rows, ...modes });
}

/** Validate a `tabs.read` result: absolute offsets, capped base64, liveness. */
export function parseRelayRpcTabsReadResult(raw: unknown): RelayRpcTabsReadResult | null {
  if (!isRecord(raw)) return null;
  const base = companionOffset(raw.base);
  const total = companionOffset(raw.total);
  if (base === null || total === null || total < base) return null;
  if (typeof raw.data !== "string") return null;
  const decoded = companionBase64ByteLength(raw.data);
  if (decoded === null || decoded > COMPANION_TERMINAL_LIMITS.readDecodedBytes) return null;
  if (base + decoded > total) return null;
  if (typeof raw.alive !== "boolean") return null;
  const exitRaw = raw.exit_code ?? raw.exitCode;
  let exitCode: number | null = null;
  if (exitRaw !== undefined && exitRaw !== null) {
    if (typeof exitRaw !== "number" || !Number.isSafeInteger(exitRaw)) return null;
    exitCode = exitRaw;
  }
  return Object.freeze({ base, total, data: raw.data, alive: raw.alive, exitCode });
}

/**
 * Validate a `tabs.screen` result. `complete:false` carries `delta:null` and
 * means explicit resync; `complete:true` carries the authoritative grid.
 * Modes ride both branches: absent means false, wrong type is unreadable.
 */
export function parseRelayRpcTabsScreenResult(raw: unknown): RelayRpcTabsScreenResult | null {
  if (!isRecord(raw) || typeof raw.complete !== "boolean") return null;
  const total = companionOffset(raw.total);
  if (total === null) return null;
  const modes = companionScreenModes(raw);
  if (!modes) return null;
  if (raw.complete === false) {
    if (raw.delta !== null) return null;
    return Object.freeze({ complete: false as const, total, delta: null, ...modes });
  }
  const delta = parseCompanionScreenDelta(raw.delta);
  if (!delta) return null;
  return Object.freeze({ complete: true as const, total, delta, ...modes });
}

/** Validate a `tabs.input` acknowledgement. */
export function parseRelayRpcTabsInputResult(raw: unknown): { readonly sent: true } | null {
  if (!isRecord(raw) || raw.sent !== true) return null;
  return Object.freeze({ sent: true as const });
}

/** Validate a `tabs.resize` acknowledgement. */
export function parseRelayRpcTabsResizeResult(raw: unknown): { readonly resized: true } | null {
  if (!isRecord(raw) || raw.resized !== true) return null;
  return Object.freeze({ resized: true as const });
}

/** Validate a `tabs.detach` acknowledgement (transport release only). */
export function parseRelayRpcTabsDetachResult(raw: unknown): { readonly detached: true } | null {
  if (!isRecord(raw) || raw.detached !== true) return null;
  return Object.freeze({ detached: true as const });
}

/** `tabs.attach {tab_id}` — bind the catalog wire tab to an opaque handle. */
export function attachCompanionTab(
  client: RelayRpcClient,
  tabId: number,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcTabsAttachResult> {
  if (companionTabId(tabId) === null) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.attach", { tab_id: tabId }, parseRelayRpcTabsAttachResult, opts);
}

/** `tabs.read {handle,offset}` — sequential raw bytes from an absolute offset. */
export function readCompanionTab(
  client: RelayRpcClient,
  handle: string,
  offset: number,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcTabsReadResult> {
  if (companionHandle(handle) === null || companionOffset(offset) === null) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.read", { handle: wireHandle(handle), offset }, parseRelayRpcTabsReadResult, opts);
}

/** `tabs.screen {handle}` — authoritative grid fallback, never partial VT. */
export function screenCompanionTab(
  client: RelayRpcClient,
  handle: string,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcTabsScreenResult> {
  if (companionHandle(handle) === null) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.screen", { handle: wireHandle(handle) }, parseRelayRpcTabsScreenResult, opts);
}

/** `tabs.input {handle,text}` — one-shot write, never retried or replayed. */
export function inputCompanionTab(
  client: RelayRpcClient,
  handle: string,
  text: string,
  opts?: { timeoutMs?: number },
): Promise<{ readonly sent: true }> {
  if (companionHandle(handle) === null || typeof text !== "string" || text.length === 0) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  if (utf8ByteLength(text) > COMPANION_TERMINAL_LIMITS.inputUtf8Bytes) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.input", { handle: wireHandle(handle), text }, parseRelayRpcTabsInputResult, opts);
}

/** `tabs.resize {handle,rows,cols}` — bounded grid resize while attached. */
export function resizeCompanionTab(
  client: RelayRpcClient,
  handle: string,
  rows: number,
  cols: number,
  opts?: { timeoutMs?: number },
): Promise<{ readonly resized: true }> {
  if (companionHandle(handle) === null || !companionGrid(rows, cols)) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.resize", { handle: wireHandle(handle), rows, cols }, parseRelayRpcTabsResizeResult, opts);
}

/** `tabs.detach {handle}` — release transport only, never kills the host PTY. */
export function detachCompanionTab(
  client: RelayRpcClient,
  handle: string,
  opts?: { timeoutMs?: number },
): Promise<{ readonly detached: true }> {
  if (companionHandle(handle) === null) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.detach", { handle: wireHandle(handle) }, parseRelayRpcTabsDetachResult, opts);
}

export interface RuntimeEventsListener {
  onSnapshot(events: readonly RelayRpcAttentionEvent[]): void;
  onEvent(event: RelayRpcAttentionEvent): void;
}

/**
 * `runtime.events.subscribe` — attention events starting from a snapshot.
 * Malformed payloads are dropped without breaking the sequence.
 */
export function subscribeRuntimeEvents(
  client: RelayRpcClient,
  listener: RuntimeEventsListener,
): RelaySubscription {
  return client.subscribe("runtime.events", {}, (payload, info) => {
    if (info.snapshot) {
      const events = parseRelayRpcEventsSnapshot(payload);
      if (events) listener.onSnapshot(events);
      return;
    }
    const event = parseRelayRpcAttentionEvent(payload);
    if (event) listener.onEvent(event);
  });
}

/**
 * Terminal output stream (`terminal.<handle>`). Raw frames carry PTY bytes
 * from absolute offset `base`; a `reset` frame carries a VT seed that redraws
 * the current screen into a blank `rows` x `cols` terminal, continuing at
 * `base`. `alive:false` is the last frame.
 */
export const COMPANION_TERMINAL_STREAM_LIMITS = Object.freeze({
  rawDecodedBytes: 64 * 1024,
  /** §4.5: a seed may arrive through the part protocol, up to 16 MiB. */
  seedDecodedBytes: 16 * 1024 * 1024,
});

export interface RelayRpcTerminalStreamPayload {
  readonly base: number;
  readonly total: number;
  readonly data: string;
  readonly decoded: number;
  readonly alive: boolean;
  readonly exitCode: number | null;
  readonly reset: boolean;
  /** Present only on `reset` frames. */
  readonly rows: number | null;
  readonly cols: number | null;
  /**
   * §4.5: a seed over its cap keeps the screen plus the most recent
   * scrollback; this many earlier lines weren't sent (the viewer says so).
   */
  readonly scrollbackOmittedLines?: number;
}

/** Strict reader: anything off-contract is unreadable, which ends the stream. */
export function parseRelayRpcTerminalStreamPayload(raw: unknown): RelayRpcTerminalStreamPayload | null {
  if (!isRecord(raw)) return null;
  const base = companionOffset(raw.base);
  const total = companionOffset(raw.total);
  if (base === null || total === null || base > total) return null;
  if (typeof raw.data !== "string" || typeof raw.alive !== "boolean" || typeof raw.reset !== "boolean") return null;
  const decoded = companionBase64ByteLength(raw.data);
  if (decoded === null) return null;
  let exitCode: number | null = null;
  if (raw.exit_code !== undefined && raw.exit_code !== null) {
    // An exit code belongs only to the last frame of an exited terminal.
    if (raw.alive || typeof raw.exit_code !== "number" || !Number.isSafeInteger(raw.exit_code)) return null;
    exitCode = raw.exit_code;
  }
  if (raw.reset) {
    if (decoded > COMPANION_TERMINAL_STREAM_LIMITS.seedDecodedBytes) return null;
    const grid = companionGrid(raw.rows, raw.cols);
    if (!grid) return null;
    const omitted = raw.scrollback_omitted_lines;
    if (omitted !== undefined && !(typeof omitted === "number" && Number.isSafeInteger(omitted) && omitted > 0)) return null;
    return Object.freeze({
      base, total, data: raw.data, decoded, alive: raw.alive, exitCode, reset: true, rows: grid.rows, cols: grid.cols,
      ...(typeof omitted === "number" ? { scrollbackOmittedLines: omitted } : {}),
    });
  }
  if (decoded > COMPANION_TERMINAL_STREAM_LIMITS.rawDecodedBytes || base + decoded > total) return null;
  if (raw.rows !== undefined || raw.cols !== undefined) return null;
  return Object.freeze({
    base, total, data: raw.data, decoded, alive: raw.alive, exitCode, reset: false, rows: null, cols: null,
  });
}

export interface CompanionTerminalStreamListener {
  onFrame(payload: RelayRpcTerminalStreamPayload, info: { readonly first: boolean }): void;
  /** Server error, transport loss or an unreadable frame. Called at most once. */
  onError(error: RelayRpcError): void;
}

export interface CompanionTerminalStream {
  /** Stops delivery at once; the unsubscribe frame goes out best-effort. */
  close(): void;
}

/**
 * Subscribe to `terminal.<handle>`. `offset` omitted asks for a seed first.
 * Nothing is delivered after `close()` or after `onError`. A subscribe that
 * cannot even be sent reports through `onError` asynchronously, so callers see
 * one failure path.
 */
export function subscribeCompanionTerminal(
  client: RelayRpcClient,
  handle: string,
  offset: number | undefined,
  listener: CompanionTerminalStreamListener,
): CompanionTerminalStream {
  let done = false;
  let first = true;
  let subscription: RelaySubscription | null = null;
  const fail = (error: RelayRpcError) => {
    if (done) return;
    done = true;
    void subscription?.unsubscribe().catch(() => {});
    listener.onError(error);
  };
  const close = () => {
    if (done) return;
    done = true;
    void subscription?.unsubscribe().catch(() => {});
  };
  if (companionHandle(handle) === null || (offset !== undefined && companionOffset(offset) === null)) {
    queueMicrotask(() => fail(new RelayRpcError("The relay request is invalid.")));
    return { close };
  }
  const params: Record<string, unknown> = { handle: wireHandle(handle) };
  if (offset !== undefined) params.offset = offset;
  try {
    subscription = client.subscribe(
      `terminal.${handle}`,
      params,
      (payload) => {
        if (done) return;
        const parsed = parseRelayRpcTerminalStreamPayload(payload);
        if (!parsed) {
          fail(new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE));
          return;
        }
        const info = { first };
        first = false;
        listener.onFrame(parsed, info);
        if (!parsed.alive) close();
      },
      (error) => {
        // The client already retired the stream; nothing to unsubscribe.
        if (done) return;
        done = true;
        listener.onError(error);
      },
    );
  } catch (error) {
    const reason = error instanceof RelayRpcError ? error : new RelayRpcError("The request failed.");
    queueMicrotask(() => fail(reason));
  }
  return { close };
}

export { DESKTOP_IN_CONTROL_CODE } from "./relayRpcClient";

/** Longest prompt the desktop accepts (`MAX_PROMPT_BYTES`). */
export const AGENT_PROMPT_MAX_BYTES = 32 * 1024;

/** What a phone can do to a card or turn of the built-in agent's conversation. */
export type AgentAction =
  | "stop"
  | "approve"
  | "approve_always"
  | "reject"
  | "answer"
  | "confirm_answer"
  | "answer_text"
  | "approve_plan"
  | "refine_plan"
  | "cancel_queued";

function parseAgentSendResult(raw: unknown): { readonly sent: true } | null {
  if (!isRecord(raw) || raw.sent !== true) return null;
  return Object.freeze({ sent: true as const });
}

function parseAgentActResult(raw: unknown): { readonly done: true } | null {
  if (!isRecord(raw) || raw.done !== true) return null;
  return Object.freeze({ done: true as const });
}

/**
 * `agent.send {tab_id,text}` — a prompt for the tab's built-in agent, exactly
 * as if typed on the desktop (it queues while a turn runs). One-shot: never
 * retried or replayed.
 */
export function sendAgentPrompt(
  client: RelayRpcClient,
  tabId: number,
  text: string,
  opts?: { timeoutMs?: number },
): Promise<{ readonly sent: true }> {
  if (companionTabId(tabId) === null || typeof text !== "string" || text.trim() === "" || utf8ByteLength(text) > AGENT_PROMPT_MAX_BYTES) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "agent.send", { tab_id: tabId, text }, parseAgentSendResult, opts);
}

/**
 * `agent.act {tab_id,action,ref?,option?,text?}` — Stop, or an answer to one
 * of the conversation's cards, named by the `ref` the card carried.
 */
export function actOnAgent(
  client: RelayRpcClient,
  tabId: number,
  action: AgentAction,
  extra: { ref?: string | null; option?: string; text?: string } = {},
  opts?: { timeoutMs?: number },
): Promise<{ readonly done: true }> {
  if (companionTabId(tabId) === null) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  const params: Record<string, unknown> = { tab_id: tabId, action };
  if (action !== "stop") {
    if (typeof extra.ref !== "string" || extra.ref === "") return Promise.reject(new RelayRpcError("The relay request is invalid."));
    params.ref = extra.ref;
  }
  if (action === "answer") {
    if (typeof extra.option !== "string" || extra.option === "") return Promise.reject(new RelayRpcError("The relay request is invalid."));
    params.option = extra.option;
  }
  if (action === "answer_text") {
    if (typeof extra.text !== "string" || extra.text.trim() === "" || utf8ByteLength(extra.text) > AGENT_PROMPT_MAX_BYTES) {
      return Promise.reject(new RelayRpcError("The relay request is invalid."));
    }
    params.text = extra.text;
  }
  return call(client, "agent.act", params, parseAgentActResult, opts);
}

export interface AgentConversationListener {
  /** One stream message; `snapshot` marks the first of a (re)subscription. */
  onMessage(payload: unknown, info: { readonly snapshot: boolean }): void;
  /** The desktop refused or the stream was lost. Called at most once. */
  onError(error: RelayRpcError): void;
}

/**
 * Subscribe to `agent.<tabId>`: the built-in agent's conversation, everything
 * first and then only the rows that change.
 */
export function subscribeAgentConversation(
  client: RelayRpcClient,
  tabId: number,
  listener: AgentConversationListener,
): { close(): void } {
  let done = false;
  let subscription: RelaySubscription | null = null;
  const close = () => {
    if (done) return;
    done = true;
    void subscription?.unsubscribe().catch(() => {});
  };
  const fail = (error: RelayRpcError) => {
    if (done) return;
    done = true;
    void subscription?.unsubscribe().catch(() => {});
    listener.onError(error);
  };
  if (companionTabId(tabId) === null) {
    queueMicrotask(() => fail(new RelayRpcError("The relay request is invalid.")));
    return { close };
  }
  try {
    subscription = client.subscribe(
      `agent.${tabId}`,
      {},
      (payload, info) => {
        if (!done) listener.onMessage(payload, { snapshot: info.snapshot });
      },
      (error) => {
        if (done) return;
        done = true;
        listener.onError(error);
      },
    );
  } catch (error) {
    const reason = error instanceof RelayRpcError ? error : new RelayRpcError("The request failed.");
    queueMicrotask(() => fail(reason));
  }
  return { close };
}

/** `files.list {path, all:true}` — the whole folder (bounded by the desktop), for the file browser. */
export function listFolder(
  client: RelayRpcClient,
  path: string,
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcFileEntry[]> {
  const params = cleanPath(path);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "files.list", { ...params, all: true }, parseRelayRpcFilesResult, opts);
}

/** `files.search {query, base}` — names under one folder (a workspace). */
export function searchFolder(
  client: RelayRpcClient,
  base: string,
  query: string,
  opts?: { timeoutMs?: number },
): Promise<readonly RelayRpcFileEntry[]> {
  if (!cleanPath(base) || typeof query !== "string" || query.trim() === "" || query.length > 256) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "files.search", { query: query.trim(), base, limit: 200 }, parseRelayRpcFilesResult, opts);
}

export interface RelayRpcFilePage {
  readonly content: string;
  /** Total size of the file in bytes. */
  readonly size: number;
  /** Where the next page starts; null on the last page. */
  readonly nextOffset: number | null;
}

function parseFilePage(raw: unknown): RelayRpcFilePage | null {
  if (!isRecord(raw) || typeof raw.content !== "string" || raw.content.length > 4 * 1024 * 1024) return null;
  const size = typeof raw.size === "number" && Number.isSafeInteger(raw.size) && raw.size >= 0 ? raw.size : null;
  if (size === null) return null;
  const next = raw.next_offset;
  if (next !== null && !(typeof next === "number" && Number.isSafeInteger(next) && next > 0)) return null;
  return Object.freeze({ content: raw.content, size, nextOffset: next });
}

/** `files.open {path, offset}` — one page of a text file. */
export function openFilePage(
  client: RelayRpcClient,
  path: string,
  offset: number,
  opts?: { timeoutMs?: number },
): Promise<RelayRpcFilePage> {
  const params = cleanPath(path);
  if (!params || !Number.isSafeInteger(offset) || offset < 0) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "files.open", { ...params, offset }, parseFilePage, opts);
}

export type GitChangeStatus =
  | "modified" | "added" | "deleted" | "renamed" | "copied" | "type_changed" | "untracked" | "conflicted";

export interface GitChange {
  readonly path: string;
  readonly oldPath: string | null;
  readonly status: GitChangeStatus;
  /** Lines added / removed; null for binary files or when unknown. */
  readonly added: number | null;
  readonly removed: number | null;
}

export interface GitStatus {
  /** False when the folder isn't a git repository. */
  readonly repository: boolean;
  readonly branch: string | null;
  readonly upstream: string | null;
  readonly ahead: number;
  readonly behind: number;
  readonly staged: readonly GitChange[];
  readonly unstaged: readonly GitChange[];
  /** The desktop listed only the first changes. */
  readonly truncated: boolean;
}

const GIT_STATUSES: readonly GitChangeStatus[] = [
  "modified", "added", "deleted", "renamed", "copied", "type_changed", "untracked", "conflicted",
];

function parseGitChanges(raw: unknown): GitChange[] | null {
  const list = boundedList(raw, 600);
  if (!list) return null;
  const out: GitChange[] = [];
  for (const entry of list) {
    if (!isRecord(entry) || typeof entry.path !== "string" || entry.path === "" || entry.path.length > 4096) return null;
    const count = (value: unknown) => (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null);
    out.push(Object.freeze({
      path: entry.path,
      oldPath: typeof entry.old_path === "string" && entry.old_path !== "" ? entry.old_path : null,
      status: GIT_STATUSES.includes(entry.status as GitChangeStatus) ? (entry.status as GitChangeStatus) : "modified",
      added: count(entry.added),
      removed: count(entry.removed),
    }));
  }
  return out;
}

/** Validate a `git.status` result. */
export function parseGitStatus(raw: unknown): GitStatus | null {
  if (!isRecord(raw)) return null;
  const staged = parseGitChanges(raw.staged);
  const unstaged = parseGitChanges(raw.unstaged);
  if (!staged || !unstaged) return null;
  const count = (value: unknown) => (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : 0);
  return Object.freeze({
    repository: raw.repository !== false,
    branch: typeof raw.branch === "string" && raw.branch !== "" ? raw.branch.slice(0, 256) : null,
    upstream: typeof raw.upstream === "string" && raw.upstream !== "" ? raw.upstream.slice(0, 256) : null,
    ahead: count(raw.ahead),
    behind: count(raw.behind),
    staged: Object.freeze(staged),
    unstaged: Object.freeze(unstaged),
    truncated: raw.truncated === true,
  });
}

/** `git.status {path}` — the workspace's staged and unstaged changes. */
export function readGitStatus(client: RelayRpcClient, root: string, opts?: { timeoutMs?: number }): Promise<GitStatus> {
  const params = cleanPath(root);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "git.status", params, parseGitStatus, { timeoutMs: 25_000, ...opts });
}

export interface GitFileDiff {
  /** Unified diff text; empty for a binary file. */
  readonly diff: string;
  readonly binary: boolean;
  readonly truncated: boolean;
}

function parseGitDiff(raw: unknown): GitFileDiff | null {
  if (!isRecord(raw) || typeof raw.diff !== "string" || raw.diff.length > 1024 * 1024) return null;
  return Object.freeze({ diff: raw.diff, binary: raw.binary === true, truncated: raw.truncated === true });
}

/** `git.diff {path, file, side}` — one file's diff. `file` is relative to the root. */
export function readGitDiff(
  client: RelayRpcClient,
  root: string,
  file: string,
  side: "staged" | "unstaged" | "untracked",
  opts?: { timeoutMs?: number },
): Promise<GitFileDiff> {
  const params = cleanPath(root);
  if (!params || typeof file !== "string" || file === "" || file.startsWith("/") || file.split("/").includes("..")) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "git.diff", { ...params, file, side }, parseGitDiff, { timeoutMs: 25_000, ...opts });
}

/** What a phone can do in a browser tab on the desktop. */
export type BrowserAction =
  | "view" | "release" | "open" | "back" | "forward" | "reload" | "tap" | "scroll" | "scroll_to" | "peek" | "type" | "press";

export const BROWSER_KEYS = Object.freeze([
  "Enter", "Tab", "Backspace", "Delete", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
  "Home", "End", "PageUp", "PageDown", "Space",
] as const);
export type BrowserKey = (typeof BROWSER_KEYS)[number];

export type BrowserActResult =
  /** The phone left the tab. */
  | { readonly kind: "done" }
  /** No page is showing (a blank tab, a server not answering, a failed load). */
  | { readonly kind: "no_page"; readonly message: string }
  /** The page as it is after the action. */
  | {
      readonly kind: "page";
      readonly url: string;
      readonly title: string;
      /** The page's visible size on the desktop, in its own pixels. */
      readonly width: number;
      readonly height: number;
      readonly top: number;
      readonly full: number;
      /** A JPEG of the visible page (base64); null when it couldn't be captured. */
      readonly picture: { readonly image: string; readonly width: number; readonly height: number } | null;
      /** The page's background colour as CSS (`rgb(…)`), or null. */
      readonly background: string | null;
      /**
       * Elements that stay put while the page scrolls (a sticky header),
       * each a JPEG with its place on the screen in the page's pixels. The
       * picture above doesn't include them.
       */
      readonly overlays: readonly BrowserOverlay[];
    };

export interface BrowserOverlay {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
  readonly image: string;
}

/** Validate a `browser.act` result. */
export function parseBrowserActResult(raw: unknown): BrowserActResult | null {
  if (!isRecord(raw)) return null;
  if (raw.done === true) return Object.freeze({ kind: "done" as const });
  if (raw.page === false) {
    return Object.freeze({ kind: "no_page" as const, message: typeof raw.message === "string" ? raw.message.slice(0, 300) : "" });
  }
  const positive = (value: unknown) => (typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null);
  const width = positive(raw.width);
  const height = positive(raw.height);
  const top = positive(raw.top);
  const full = positive(raw.full);
  if (typeof raw.url !== "string" || typeof raw.title !== "string" || width === null || height === null || top === null || full === null) {
    return null;
  }
  let picture: { image: string; width: number; height: number } | null = null;
  if (raw.image !== undefined) {
    const imageWidth = positive(raw.image_width);
    const imageHeight = positive(raw.image_height);
    // About 700 KB of JPEG at most, and only base64 characters.
    if (
      typeof raw.image !== "string" || raw.image.length > 1_000_000 || !/^[A-Za-z0-9+/]+=*$/u.test(raw.image) ||
      !imageWidth || !imageHeight
    ) return null;
    picture = { image: raw.image, width: imageWidth, height: imageHeight };
  }
  const background = typeof raw.bg === "string" && /^rgba?\([0-9., ]{5,34}\)$/u.test(raw.bg) ? raw.bg : null;
  const overlays: BrowserOverlay[] = [];
  if (Array.isArray(raw.overlays)) {
    for (const entry of raw.overlays.slice(0, 6)) {
      if (!isRecord(entry)) return null;
      const x = positive(entry.x);
      const y = positive(entry.y);
      const w = positive(entry.width);
      const h = positive(entry.height);
      if (x === null || y === null || !w || !h) return null;
      if (typeof entry.image !== "string" || entry.image.length > 300_000 || !/^[A-Za-z0-9+/]+=*$/u.test(entry.image)) return null;
      overlays.push(Object.freeze({ x, y, width: w, height: h, image: entry.image }));
    }
  }
  return Object.freeze({
    kind: "page" as const, url: raw.url.slice(0, 4096), title: raw.title.slice(0, 300), width, height, top, full, picture,
    background, overlays: Object.freeze(overlays),
  });
}

/**
 * `browser.act {tab_id, action, …}` — one action in a browser tab on the
 * desktop, answered with the page as it is afterwards (address, position and
 * a picture) in the same round trip. The phone only names actions and passes
 * values (an address, a point, text, a key); it never sends script. `claim`
 * marks the first request after opening the tab.
 */
export function browserAct(
  client: RelayRpcClient,
  tabId: number,
  action: BrowserAction,
  extra: {
    url?: string; x?: number; y?: number; by?: number; text?: string; key?: BrowserKey; claim?: boolean;
    /** `scroll_to` / `peek`: a position in the page's own pixels from its top. */
    top?: number;
    /** Mobile view: lay the page out at this size (the phone's page area, in points). Omit for the desktop's own size. */
    mobile?: { width: number; height: number } | null;
  } = {},
  opts?: { timeoutMs?: number },
): Promise<BrowserActResult> {
  const invalid = () => Promise.reject<BrowserActResult>(new RelayRpcError("The relay request is invalid."));
  if (companionTabId(tabId) === null) return invalid();
  const params: Record<string, unknown> = { tab_id: tabId, action };
  if (extra.claim === true) params.claim = true;
  if (extra.mobile) {
    const { width, height } = extra.mobile;
    const side = (value: number, most: number) => Number.isFinite(value) && value >= 240 && value <= most;
    if (!side(width, 1024) || !side(height, 1600)) return invalid();
    params.mobile = { width: Math.round(width), height: Math.round(height) };
  }
  const unit = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1;
  if (action === "open") {
    if (typeof extra.url !== "string" || extra.url.trim() === "" || utf8ByteLength(extra.url) > 8 * 1024) return invalid();
    params.url = extra.url.trim();
  } else if (action === "tap") {
    if (!unit(extra.x) || !unit(extra.y)) return invalid();
    params.x = extra.x;
    params.y = extra.y;
  } else if (action === "scroll") {
    if (typeof extra.by !== "number" || !Number.isFinite(extra.by) || Math.abs(extra.by) > 20) return invalid();
    params.by = extra.by;
    // Where the finger is: what scrolls under that point is what moves.
    if (extra.x !== undefined || extra.y !== undefined) {
      if (!unit(extra.x) || !unit(extra.y)) return invalid();
      params.x = extra.x;
      params.y = extra.y;
    }
  } else if (action === "scroll_to" || action === "peek") {
    if (typeof extra.top !== "number" || !Number.isFinite(extra.top) || extra.top < 0 || extra.top > 10_000_000) return invalid();
    params.top = Math.round(extra.top);
  } else if (action === "type") {
    if (typeof extra.text !== "string" || extra.text === "" || utf8ByteLength(extra.text) > 8 * 1024) return invalid();
    params.text = extra.text;
  } else if (action === "press") {
    if (!BROWSER_KEYS.includes(extra.key as BrowserKey)) return invalid();
    params.key = extra.key;
  }
  return call(client, "browser.act", params, parseBrowserActResult, { timeoutMs: 22_000, ...opts });
}


/** What a source-control action answered: done, or git's words why not. */
export interface GitActionResult {
  readonly done: boolean;
  readonly message: string | null;
  /** `git.commit`: the new commit. */
  readonly commit: { readonly hash: string; readonly subject: string } | null;
  /** `git.pr_create`: the pull request's address. */
  readonly url: string | null;
}

function parseGitAction(raw: unknown): GitActionResult | null {
  if (!isRecord(raw) || typeof raw.done !== "boolean") return null;
  const commit = isRecord(raw.commit) && typeof raw.commit.hash === "string"
    ? Object.freeze({ hash: raw.commit.hash.slice(0, 64), subject: typeof raw.commit.subject === "string" ? raw.commit.subject.slice(0, 512) : "" })
    : null;
  return Object.freeze({
    done: raw.done,
    message: typeof raw.message === "string" ? raw.message.slice(0, 4000) : null,
    commit,
    url: typeof raw.url === "string" && raw.url.startsWith("https://") ? raw.url.slice(0, 2048) : null,
  });
}

function gitAction(
  client: RelayRpcClient,
  method: RelayRpcMethod,
  root: string,
  extra: Record<string, unknown>,
  timeoutMs = 60_000,
): Promise<GitActionResult> {
  const params = cleanPath(root);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, method, { ...params, ...extra }, parseGitAction, { timeoutMs });
}

function checkedFiles(files: readonly string[]): string[] | null {
  if (files.length === 0 || files.length > 500) return null;
  for (const file of files) {
    if (typeof file !== "string" || file === "" || file.startsWith("/") || file.startsWith("-") || file.split("/").includes("..")) return null;
  }
  return [...files];
}

/** `git.stage {path, files}`: add these files' changes to the next commit. */
export function gitStage(client: RelayRpcClient, root: string, files: readonly string[]): Promise<GitActionResult> {
  const list = checkedFiles(files);
  if (!list) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return gitAction(client, "git.stage", root, { files: list });
}

/** `git.unstage {path, files}`: take them out of the next commit (changes stay). */
export function gitUnstage(client: RelayRpcClient, root: string, files: readonly string[]): Promise<GitActionResult> {
  const list = checkedFiles(files);
  if (!list) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return gitAction(client, "git.unstage", root, { files: list });
}

/** `git.discard {path, files, untracked}`: throw away unstaged changes (deletes untracked files). */
export function gitDiscard(client: RelayRpcClient, root: string, files: readonly string[], untracked: boolean): Promise<GitActionResult> {
  const list = checkedFiles(files);
  if (!list) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return gitAction(client, "git.discard", root, { files: list, untracked });
}

/** `git.commit {path, message}`: commit what is staged. */
export function gitCommit(client: RelayRpcClient, root: string, message: string): Promise<GitActionResult> {
  if (message.trim() === "" || message.length > 64 * 1024) return Promise.reject(new RelayRpcError("Write a commit message first."));
  return gitAction(client, "git.commit", root, { message }, 120_000);
}

/** `git.push {path}`: push the branch (publishing it to origin if new). */
export function gitPush(client: RelayRpcClient, root: string): Promise<GitActionResult> {
  return gitAction(client, "git.push", root, {}, 120_000);
}

/** `git.switch {path, branch}`: change to another local branch. */
export function gitSwitch(client: RelayRpcClient, root: string, branch: string): Promise<GitActionResult> {
  if (branch === "" || branch.startsWith("-")) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return gitAction(client, "git.switch", root, { branch });
}

/** `git.pr_create {path, title, body, draft}`: open a pull request with the desktop's GitHub login. */
export function gitCreatePr(client: RelayRpcClient, root: string, title: string, body: string, draft: boolean): Promise<GitActionResult> {
  if (title.trim() === "") return Promise.reject(new RelayRpcError("Give the pull request a title."));
  return gitAction(client, "git.pr_create", root, { title, body, draft }, 120_000);
}

export interface GitCommitSummary {
  readonly hash: string;
  readonly short: string;
  readonly author: string;
  /** Unix seconds. */
  readonly at: number;
  readonly subject: string;
}

/** `git.log {path, limit}`: the branch's latest commits. */
export function readGitLog(client: RelayRpcClient, root: string, limit = 50): Promise<readonly GitCommitSummary[]> {
  const params = cleanPath(root);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "git.log", { ...params, limit }, (raw) => {
    if (!isRecord(raw)) return null;
    const list = boundedList(raw.commits, 200);
    if (!list) return null;
    const out: GitCommitSummary[] = [];
    for (const c of list) {
      if (!isRecord(c) || typeof c.hash !== "string" || typeof c.subject !== "string") return null;
      out.push(Object.freeze({
        hash: c.hash, short: typeof c.short === "string" ? c.short : c.hash.slice(0, 7),
        author: typeof c.author === "string" ? c.author : "", at: typeof c.at === "number" ? c.at : 0, subject: c.subject,
      }));
    }
    return Object.freeze(out);
  }, { timeoutMs: 25_000 });
}

/** `git.branches {path}`: local branches and the current one. */
export function readGitBranches(client: RelayRpcClient, root: string): Promise<{ readonly branches: readonly string[]; readonly current: string | null }> {
  const params = cleanPath(root);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "git.branches", params, (raw) => {
    if (!isRecord(raw)) return null;
    const list = boundedList(raw.branches, 200);
    if (!list || !list.every((b) => typeof b === "string")) return null;
    return Object.freeze({ branches: Object.freeze(list as string[]), current: typeof raw.current === "string" ? raw.current : null });
  }, { timeoutMs: 25_000 });
}


/**
 * `tabs.send {tab_id, text}`: type a message into an agent's terminal tab on
 * the desktop and submit it (pasted whole, then Enter). For the built-in
 * Dragent agent use `sendAgentPrompt`.
 */
export function sendTabText(client: RelayRpcClient, tabId: number, text: string): Promise<{ readonly sent: true }> {
  if (companionTabId(tabId) === null || text.trim() === "" || utf8ByteLength(text) > 32 * 1024 || text.includes("\u001b")) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.send", { tab_id: tabId, text }, (raw) => (isRecord(raw) && raw.sent === true ? Object.freeze({ sent: true as const }) : null), { timeoutMs: 20_000 });
}


export interface TranscriptRow {
  readonly id: string;
  readonly role: "user" | "assistant" | "tool";
  readonly text: string;
  readonly tool: string | null;
  readonly error: boolean;
  /** When the agent wrote it (Unix ms); absent from older desktops. */
  readonly at: number | null;
  /** A tool step still waiting for its result. */
  readonly running: boolean;
  /** The file a tool step read or changed. */
  readonly path: string | null;
  /** Lines a file change added and removed. */
  readonly added: number | null;
  readonly removed: number | null;
}

export interface AgentTranscript {
  /** False: the desktop can't read this agent's conversation (use the terminal). */
  readonly supported: boolean;
  readonly provider: string | null;
  readonly rows: readonly TranscriptRow[];
}

/** `agent.transcript {tab_id}`: a CLI agent's conversation, read from its own transcript. */
export function readAgentTranscript(client: RelayRpcClient, tabId: number): Promise<AgentTranscript> {
  if (companionTabId(tabId) === null) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "agent.transcript", { tab_id: tabId }, (raw) => {
    if (!isRecord(raw) || typeof raw.supported !== "boolean") return null;
    const list = boundedList(raw.rows, 400);
    if (!list) return null;
    const rows: TranscriptRow[] = [];
    for (const r of list) {
      if (!isRecord(r) || typeof r.id !== "string" || typeof r.text !== "string") return null;
      const role = r.role === "user" || r.role === "assistant" || r.role === "tool" ? r.role : null;
      if (!role) continue;
      const count = (value: unknown) => (typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null);
      rows.push(Object.freeze({
        id: r.id,
        role,
        text: r.text.slice(0, 16_384),
        tool: typeof r.tool === "string" ? r.tool : null,
        error: r.error === true,
        at: typeof r.at === "number" && Number.isFinite(r.at) ? r.at : null,
        running: r.running === true,
        path: typeof r.path === "string" ? r.path.slice(0, 4_096) : null,
        added: count(r.added),
        removed: count(r.removed),
      }));
    }
    return Object.freeze({ supported: raw.supported, provider: typeof raw.provider === "string" ? raw.provider : null, rows: Object.freeze(rows) });
  }, { timeoutMs: 20_000 });
}


export interface FileImage {
  readonly mime: string;
  /** Too big to send whole: show its size instead. */
  readonly tooLarge: boolean;
  readonly size: number;
  /** Base64 bytes; null when too large. */
  readonly data: string | null;
}

/** `files.image {path}`: an image file on the desktop, to show on the phone. */
export function readFileImage(client: RelayRpcClient, path: string): Promise<FileImage> {
  const params = cleanPath(path);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "files.image", params, (raw) => {
    if (!isRecord(raw) || typeof raw.mime !== "string" || !raw.mime.startsWith("image/")) return null;
    return Object.freeze({
      mime: raw.mime,
      tooLarge: raw.too_large === true,
      size: typeof raw.size === "number" ? raw.size : 0,
      data: typeof raw.data === "string" ? raw.data : null,
    });
  }, { timeoutMs: 30_000 });
}

/**
 * Tell the desktop this tab is on the phone's screen: what its agent
 * finished or failed counts as seen there, as opening the tab on the
 * desktop does, so it stops showing Review or Failed everywhere.
 */
export function markTabSeen(client: RelayRpcClient, tabId: number): Promise<void> {
  if (!Number.isSafeInteger(tabId) || tabId < 0) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(client, "tabs.seen", { tab_id: tabId }, (raw) =>
    typeof raw === "object" && raw !== null && (raw as { seen?: unknown }).seen === true ? true : null,
  ).then(() => undefined);
}
