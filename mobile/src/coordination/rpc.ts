/**
 * Typed `coordination.*` RPC calls over the machine socket's `RelayRpcClient`.
 * No dialing, no retries, no UI: params are validated client-side (64KB cap
 * shared with the transport core) and malformed replies fail closed with the
 * single user-safe message so callers show retry instead of empty.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import { utf8ByteLength } from "../transport/frames";
import {
  parseCoordinationDetailResult,
  parseCoordinationListResult,
} from "./readers";
import type { CoordinationCost, CoordinationDetailResult, CoordinationListResult } from "./types";
import { readStartResult, readTaskOptions } from "../tasks/readers";
import type { StartResult, TaskOptions } from "../tasks/types";

export const COORDINATION_RPC_MAX_PARAMS_CHARS = 64 * 1024;

function checkedParams(params: Record<string, unknown>): Record<string, unknown> | null {
  let encoded: string;
  try {
    encoded = JSON.stringify(params);
  } catch {
    return null;
  }
  return encoded.length > COORDINATION_RPC_MAX_PARAMS_CHARS ||
    utf8ByteLength(encoded) > COORDINATION_RPC_MAX_PARAMS_CHARS
    ? null
    : params;
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

function cleanProjectId(projectId: string | null | undefined): Record<string, unknown> | null {
  if (projectId === undefined || projectId === null) return {};
  const hasNul = typeof projectId === "string" && projectId.indexOf(String.fromCharCode(0)) !== -1;
  if (typeof projectId !== "string" || projectId.length === 0 || projectId.length > 1024 || hasNul) {
    return null;
  }
  return { project_id: projectId };
}

/** `coordination.list` — optional `project_id` filter; never invent fields. */
export function listCoordinations(
  client: RelayRpcClient,
  input: { project_id?: string } = {},
  opts?: { timeoutMs?: number },
): Promise<CoordinationListResult> {
  const params = cleanProjectId(input.project_id);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "coordination.list", params, parseCoordinationListResult, opts);
}

/** Team IDs are `team-` plus hex on the wire; validate the native shape. */
const TEAM_ID_PATTERN = /^[A-Za-z0-9_-]{1,128}$/;

function cleanTeamId(teamId: string): Record<string, unknown> | null {
  if (typeof teamId !== "string" || !TEAM_ID_PATTERN.test(teamId)) return null;
  return { team_id: teamId };
}

/** `coordination.get` — one team by id with its tasks and messages. */
export function getCoordination(
  client: RelayRpcClient,
  teamId: string,
  opts?: { timeoutMs?: number },
): Promise<CoordinationDetailResult> {
  const params = cleanTeamId(teamId);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, "coordination.get", params, parseCoordinationDetailResult, opts).then((result) => {
    // The broker echoes the requested team; a mismatched id fails closed.
    if (result.team.id !== teamId) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
    return result;
  });
}

/** Most workers of one agent the phone may allow at once (the desktop's bound). */
export const COORDINATION_MAX_PARALLEL = 8;

function teamChange(
  client: RelayRpcClient,
  method: string,
  teamId: string,
  extra: Record<string, unknown>,
): Promise<CoordinationDetailResult> {
  const params = cleanTeamId(teamId);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(client, method, { ...params, ...extra }, parseCoordinationDetailResult, { timeoutMs: 30_000 }).then((result) => {
    if (result.team.id !== teamId) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
    return result;
  });
}

/** Message the lead: it goes straight into the lead's session on the desktop. */
export function sendToLead(client: RelayRpcClient, teamId: string, body: string): Promise<CoordinationDetailResult> {
  const text = body.trim();
  if (!text || text.length > 8000) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return teamChange(client, "coordination.send", teamId, { body: text });
}

/** Stop the team: its lead and workers stop on the desktop. */
export function stopCoordination(client: RelayRpcClient, teamId: string): Promise<CoordinationDetailResult> {
  return teamChange(client, "coordination.stop", teamId, {});
}

/** How many workers of one agent run at once, and whether ready tasks start on their own. */
export function configureCoordination(
  client: RelayRpcClient,
  teamId: string,
  change: { maxParallel?: number; autoStart?: boolean; leadAnswersPrompts?: boolean; cost?: CoordinationCost },
): Promise<CoordinationDetailResult> {
  const extra: Record<string, unknown> = {};
  if (change.leadAnswersPrompts !== undefined) extra.lead_answers_prompts = change.leadAnswersPrompts;
  if (change.cost !== undefined) extra.cost = change.cost;
  if (change.maxParallel !== undefined) {
    if (!Number.isInteger(change.maxParallel) || change.maxParallel < 1 || change.maxParallel > COORDINATION_MAX_PARALLEL) {
      return Promise.reject(new RelayRpcError("The relay request is invalid."));
    }
    extra.max_parallel = change.maxParallel;
  }
  if (change.autoStart !== undefined) extra.auto_start = change.autoStart;
  if (Object.keys(extra).length === 0) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return teamChange(client, "coordination.configure", teamId, extra);
}

/** Cancel one task of the plan (the lead plans around it). */
export function cancelCoordinationTask(
  client: RelayRpcClient,
  teamId: string,
  taskId: string,
): Promise<CoordinationDetailResult> {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(taskId)) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return teamChange(client, "coordination.cancel_task", teamId, { task_id: taskId });
}

/** Take a finished or stopped team off the list (the desktop refuses a running one). */
export async function forgetCoordination(client: RelayRpcClient, teamId: string): Promise<void> {
  const params = cleanTeamId(teamId);
  if (!params) throw new RelayRpcError("The relay request is invalid.");
  await call(
    client,
    "coordination.forget",
    params,
    (raw) => (typeof raw === "object" && raw !== null && (raw as Record<string, unknown>).forgotten === true ? true : null),
    { timeoutMs: 30_000 },
  );
}

/** What a new coordination can use: the desktop's projects and agents. */
export function coordinationOptions(client: RelayRpcClient): Promise<TaskOptions> {
  return call(client, "coordination.options", {}, readTaskOptions, { timeoutMs: 30_000 });
}

/**
 * Start a coordination on the desktop: `lead` plans `goal` in `project` (a
 * desktop folder) and hands the parts to `workers` (agent keys).
 */
export function createCoordination(
  client: RelayRpcClient,
  input: { project: string; goal: string; lead: string; workers: readonly string[]; cost: CoordinationCost },
): Promise<StartResult> {
  const goal = input.goal.trim();
  if (!goal || goal.length > 8000 || !input.project || !input.lead) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return call(
    client,
    "coordination.create",
    { project: input.project, goal, lead: input.lead, workers: [...new Set(input.workers)].slice(0, 8), cost: input.cost },
    readStartResult,
    { timeoutMs: 30_000 },
  );
}

/** Move a task to `model`, or back to its agent's own choice (`null`), as the desktop's model picker does. */
export function setCoordinationTaskModel(
  client: RelayRpcClient,
  teamId: string,
  taskId: string,
  model: string | null,
): Promise<CoordinationDetailResult> {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(taskId) || (model !== null && (!model || model.length > 200 || model === "none"))) {
    return Promise.reject(new RelayRpcError("The relay request is invalid."));
  }
  return teamChange(client, "coordination.task_model", teamId, { task_id: taskId, model });
}

/** The team as it is now, or why the desktop didn't do it. */
export type CoordinationWindowResult =
  | { readonly done: true; readonly result: CoordinationDetailResult }
  | { readonly done: false; readonly reason: string };

function windowChange(client: RelayRpcClient, method: string, teamId: string): Promise<CoordinationWindowResult> {
  const params = cleanTeamId(teamId);
  if (!params) return Promise.reject(new RelayRpcError("The relay request is invalid."));
  return call(
    client,
    method,
    params,
    (raw): CoordinationWindowResult | null => {
      const refused = readStartResult(raw);
      if (refused && !refused.started) return { done: false, reason: refused.reason ?? "The desktop didn't do it." };
      const result = parseCoordinationDetailResult(raw);
      return result && result.team.id === teamId ? { done: true, result } : null;
    },
    { timeoutMs: 30_000 },
  );
}

/** Start a new lead for a team whose lead stopped (it opens on the desktop and picks up from the plan). */
export function startNewLead(client: RelayRpcClient, teamId: string): Promise<CoordinationWindowResult> {
  return windowChange(client, "coordination.new_lead", teamId);
}

/** Close and remove a finished team's worktrees, as the desktop's Clean up does (branches stay). */
export function cleanUpCoordination(client: RelayRpcClient, teamId: string): Promise<CoordinationWindowResult> {
  return windowChange(client, "coordination.clean_up", teamId);
}
