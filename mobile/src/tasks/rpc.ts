/**
 * Typed `tasks.*` calls over the machine's RPC client (served by the paired
 * desktop inside the encrypted channel). No dialing, no retries, no UI:
 * inputs are checked here, replies by the strict readers.
 */
import { RelayRpcClient, RelayRpcError } from "../transport/relayRpcClient";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../transport/relayRpcMethods";
import {
  readTrackerItemReply,
  readTrackerItems,
  readTrackers,
  readCommentReply,
  readEmpty,
  readStartResult,
  readTaskOptions,
  readMembers,
  readTaskDetail,
  readTaskList,
  readTaskReply,
} from "./readers";
import type {
  LiveTask,
  StartResult,
  TaskComment,
  TaskOptions,
  Tracker,
  TrackerItem,
  TaskDetailResult,
  TaskListFilter,
  TaskListResult,
  TaskPerson,
  TaskPriority,
  TaskStatus,
} from "./types";

const INVALID = "The request is invalid.";
const TASK_REF = /^[A-Za-z0-9_-]{1,64}$/;

async function call<T>(
  client: RelayRpcClient,
  method: string,
  params: Record<string, unknown>,
  read: (raw: unknown) => T | null,
): Promise<T> {
  let raw: unknown;
  try {
    raw = await client.request(method, params, { timeoutMs: 30_000 });
  } catch (error) {
    if (error instanceof RelayRpcError) throw error;
    throw new RelayRpcError("The request failed.");
  }
  const parsed = read(raw);
  if (parsed === null) throw new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  return parsed;
}

function taskRef(task: string): string {
  if (!TASK_REF.test(task)) throw new RelayRpcError(INVALID);
  return task;
}

export function listTasks(
  client: RelayRpcClient,
  input: { status: TaskListFilter; q?: string; mine?: boolean; assignee?: string; label?: string },
): Promise<TaskListResult> {
  const params: Record<string, unknown> = { status: input.status };
  const q = input.q?.trim();
  if (q) params.q = q.slice(0, 200);
  // `me`, `none` or a member's id.
  const assignee = input.mine ? "me" : input.assignee;
  if (assignee && /^[A-Za-z0-9_-]{1,128}$/.test(assignee)) params.assignee = assignee;
  const label = input.label?.trim();
  if (label) params.label = label.slice(0, 48);
  return call(client, "tasks.list", params, readTaskList);
}

export function getTask(client: RelayRpcClient, task: string): Promise<TaskDetailResult> {
  return Promise.resolve()
    .then(() => taskRef(task))
    .then((ref) => call(client, "tasks.get", { task: ref }, readTaskDetail));
}

export function createTask(
  client: RelayRpcClient,
  input: { title: string; description?: string; status?: TaskStatus; priority?: TaskPriority; assigneeId?: string },
): Promise<LiveTask> {
  const title = input.title.trim();
  if (!title || title.length > 500) return Promise.reject(new RelayRpcError(INVALID));
  const params: Record<string, unknown> = { title };
  const description = input.description?.trim();
  if (description) params.description = description.slice(0, 20_000);
  if (input.status) params.status = input.status;
  if (input.priority && input.priority !== "none") params.priority = input.priority;
  if (input.assigneeId) params.assignee_id = input.assigneeId;
  return call(client, "tasks.create", params, readTaskReply);
}

export interface TaskChange {
  title?: string;
  description?: string;
  status?: TaskStatus;
  priority?: TaskPriority;
  /** `""` unassigns. */
  assigneeId?: string;
  labels?: readonly string[];
  /** `null` clears the due date. */
  dueAt?: number | null;
  /** A shared project key; `""` takes the task out of its project. */
  projectKey?: string;
}

/** Change any of a task's fields; only what's given changes. */
export function updateTask(client: RelayRpcClient, task: string, change: TaskChange): Promise<LiveTask> {
  const params: Record<string, unknown> = { task };
  if (change.title !== undefined) {
    const title = change.title.trim();
    if (!title || title.length > 500) return Promise.reject(new RelayRpcError(INVALID));
    params.title = title;
  }
  if (change.description !== undefined) params.description = change.description.slice(0, 20_000);
  if (change.status) params.status = change.status;
  if (change.priority) params.priority = change.priority;
  if (change.assigneeId !== undefined) params.assignee_id = change.assigneeId;
  if (change.labels !== undefined) params.labels = change.labels.slice(0, 20).map((label) => label.slice(0, 48));
  if (change.dueAt !== undefined) params.due_at = change.dueAt;
  if (change.projectKey !== undefined) params.project = change.projectKey;
  if (Object.keys(params).length === 1) return Promise.reject(new RelayRpcError(INVALID));
  return Promise.resolve()
    .then(() => taskRef(task))
    .then(() => call(client, "tasks.update", params, readTaskReply));
}

export function commentOnTask(client: RelayRpcClient, task: string, body: string): Promise<TaskComment> {
  const text = body.trim();
  if (!text || text.length > 20_000) return Promise.reject(new RelayRpcError(INVALID));
  return Promise.resolve()
    .then(() => taskRef(task))
    .then((ref) => call(client, "tasks.comment", { task: ref, body: text }, readCommentReply));
}

export function listMembers(client: RelayRpcClient): Promise<readonly TaskPerson[]> {
  return call(client, "tasks.members", {}, readMembers);
}

function withTask<T>(task: string, run: (ref: string) => Promise<T>): Promise<T> {
  return Promise.resolve().then(() => taskRef(task)).then(run);
}

export function deleteTask(client: RelayRpcClient, task: string): Promise<true> {
  return withTask(task, (ref) => call(client, "tasks.delete", { task: ref }, readEmpty));
}

export function restoreTask(client: RelayRpcClient, task: string): Promise<LiveTask> {
  return withTask(task, (ref) => call(client, "tasks.restore", { task: ref }, readTaskReply));
}

export function editComment(client: RelayRpcClient, task: string, comment: string, body: string): Promise<TaskComment> {
  const text = body.trim();
  if (!text || text.length > 20_000) return Promise.reject(new RelayRpcError(INVALID));
  return withTask(task, (ref) => call(client, "tasks.comment_edit", { task: ref, comment, body: text }, readCommentReply));
}

export function deleteComment(client: RelayRpcClient, task: string, comment: string): Promise<true> {
  return withTask(task, (ref) => call(client, "tasks.comment_delete", { task: ref, comment }, readEmpty));
}

/**
 * Link a pull request (its web address) or a tracker item (`linear:ENG-12`).
 * Which one is decided from the text.
 */
export function linkToTask(client: RelayRpcClient, task: string, target: string, title?: string): Promise<LiveTask> {
  const value = target.trim();
  if (!value || value.length > 2048) return Promise.reject(new RelayRpcError(INVALID));
  const params: Record<string, unknown> = /^https?:\/\//i.test(value) ? { url: value } : { item: value };
  if (!params.url && !/^[^:\s]+:\S+$/.test(value)) return Promise.reject(new RelayRpcError(INVALID));
  const named = title?.trim();
  if (named) params.title = named.slice(0, 500);
  return withTask(task, (ref) => call(client, "tasks.link", { task: ref, ...params }, readTaskReply));
}

export function unlinkFromTask(client: RelayRpcClient, task: string, link: string): Promise<LiveTask> {
  return withTask(task, (ref) => call(client, "tasks.unlink", { task: ref, link }, readTaskReply));
}

export function taskOptions(client: RelayRpcClient): Promise<TaskOptions> {
  return call(client, "tasks.options", {}, readTaskOptions);
}

/**
 * Start an agent on the task on the desktop, the way its Start does: a new
 * workspace on the task's project (or `project`, a desktop folder).
 */
export function startTask(
  client: RelayRpcClient,
  input: { task: string; agent: string; project?: string },
): Promise<StartResult> {
  const params: Record<string, unknown> = { agent: input.agent };
  if (input.project) params.project = input.project;
  return withTask(input.task, (ref) => call(client, "tasks.start", { task: ref, ...params }, readStartResult));
}

/** The trackers connected on the desktop. */
export function listTrackers(client: RelayRpcClient): Promise<readonly Tracker[]> {
  return call(client, "tasks.trackers", {}, readTrackers);
}

function trackerParams(provider: string, project: string | null): Record<string, unknown> {
  if (!/^[a-z_]{1,32}$/.test(provider)) throw new RelayRpcError(INVALID);
  return project ? { provider, project } : { provider };
}

/** A tracker's open items, asked from `project` (a desktop folder) or the desktop's home. */
export function listTrackerItems(client: RelayRpcClient, provider: string, project: string | null): Promise<readonly TrackerItem[]> {
  return Promise.resolve().then(() => call(client, "tasks.tracker_items", trackerParams(provider, project), readTrackerItems));
}

export function moveTrackerItem(
  client: RelayRpcClient,
  input: { provider: string; project: string | null; item: string; status: TaskStatus },
): Promise<TrackerItem> {
  return Promise.resolve().then(() =>
    call(client, "tasks.tracker_move", { ...trackerParams(input.provider, input.project), item: input.item, status: input.status }, readTrackerItemReply),
  );
}

/** Save an item as a task (or get the task it was saved as before). */
export function importTrackerItem(
  client: RelayRpcClient,
  input: { provider: string; project: string | null; item: string },
): Promise<LiveTask> {
  return Promise.resolve().then(() =>
    call(client, "tasks.tracker_import", { ...trackerParams(input.provider, input.project), item: input.item }, readTaskReply),
  );
}

export function createTrackerItem(
  client: RelayRpcClient,
  input: { provider: string; project: string | null; title: string; body?: string; place?: string },
): Promise<TrackerItem> {
  const title = input.title.trim();
  if (!title || title.length > 500) return Promise.reject(new RelayRpcError(INVALID));
  return Promise.resolve().then(() => {
    const params: Record<string, unknown> = { ...trackerParams(input.provider, input.project), title };
    const body = input.body?.trim();
    if (body) params.body = body.slice(0, 20_000);
    const place = input.place?.trim();
    if (place) params.place = place.slice(0, 200);
    return call(client, "tasks.tracker_create", params, readTrackerItemReply);
  });
}

/** Start an agent on a tracker item, on the desktop, as its Start does. */
export function startTrackerItem(
  client: RelayRpcClient,
  input: { provider: string; project: string | null; item: string; agent: string },
): Promise<StartResult> {
  return Promise.resolve().then(() =>
    call(
      client,
      "tasks.tracker_start",
      { ...trackerParams(input.provider, input.project), item: input.item, agent: input.agent },
      readStartResult,
    ),
  );
}

/**
 * Start several tasks one after another, as the desktop does: each waits
 * for the one before to have its workspace. `onProgress` hears each result.
 */
export async function startTasks(
  client: RelayRpcClient,
  input: { tasks: readonly string[]; agent: string; project?: string },
  onProgress: (task: string, result: StartResult) => void,
  wait: (ms: number) => Promise<void> = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
): Promise<void> {
  for (const task of input.tasks) {
    let result: StartResult = { started: false, reason: null };
    for (let attempt = 0; attempt < 60; attempt += 1) {
      result = await startTask(client, { task, agent: input.agent, ...(input.project ? { project: input.project } : {}) });
      // The desktop is still opening the one before: wait for it.
      if (result.started || !result.reason?.includes("already starting")) break;
      await wait(1500);
    }
    onProgress(task, result);
  }
}
