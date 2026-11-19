/**
 * Strict readers for the desktop's `tasks.*` replies: anything malformed is
 * `null`, so a bad reply shows an error instead of wrong data.
 */
import {
  TASK_STATUSES,
  type LiveTask,
  type TaskActivity,
  type TaskComment,
  type TaskLink,
  type TaskLinkKind,
  type TaskOptions,
  type StartAgent,
  type StartProject,
  type StartResult,
  type Tracker,
  type TrackerItem,
  type TaskDetailResult,
  type TaskListResult,
  type TaskPerson,
  type TaskPriority,
  type TaskStatus,
} from "./types";

const PRIORITIES: readonly string[] = ["none", "urgent", "high", "medium", "low"];
const MAX_TASKS = 500;
const MAX_ENTRIES = 200;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown, max: number): string | null {
  return typeof value === "string" && value.length <= max ? value : null;
}

function time(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function id(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 && value.length <= 128 ? value : null;
}

export function readPerson(value: unknown): TaskPerson | null | undefined {
  if (value === null || value === undefined) return null;
  if (!isRecord(value)) return undefined;
  const personId = text(value.id, 128);
  const name = text(value.name, 320);
  if (personId === null || name === null) return undefined;
  return Object.freeze({ id: personId, name });
}

const LINK_KINDS: readonly string[] = ["workspace", "pull_request", "item"];

function readLink(value: unknown): TaskLink | null {
  if (!isRecord(value)) return null;
  const linkId = id(value.id);
  const title = text(value.title, 2000);
  if (linkId === null || title === null || typeof value.kind !== "string" || !LINK_KINDS.includes(value.kind)) return null;
  const url = typeof value.url === "string" && /^https?:\/\//.test(value.url) && value.url.length <= 2048 ? value.url : null;
  const state = typeof value.state === "string" ? value.state.slice(0, 32) : null;
  const workspaceRoot =
    value.kind === "workspace" && typeof value.workspace_root === "string" && value.workspace_root.length <= 4096
      ? value.workspace_root
      : null;
  return Object.freeze({ id: linkId, kind: value.kind as TaskLinkKind, title, url, state, workspaceRoot });
}

export function readTask(value: unknown): LiveTask | null {
  if (!isRecord(value)) return null;
  const taskId = id(value.id);
  const key = text(value.key, 64);
  const title = text(value.title, 2000);
  const description = text(value.description, 120_000);
  const status = value.status;
  const priority = value.priority;
  const createdAt = time(value.created_at);
  const updatedAt = time(value.updated_at);
  const assignee = readPerson(value.assignee);
  const creator = readPerson(value.creator);
  if (
    taskId === null || key === null || title === null || description === null ||
    createdAt === null || updatedAt === null || assignee === undefined || creator === undefined ||
    typeof status !== "string" || !(TASK_STATUSES as readonly string[]).includes(status) ||
    typeof priority !== "string" || !PRIORITIES.includes(priority)
  ) {
    return null;
  }
  const project = value.project === null || value.project === undefined ? null : text(value.project, 512);
  if (project === null && value.project !== null && value.project !== undefined) return null;
  const labels = Array.isArray(value.labels)
    ? value.labels.filter((label): label is string => typeof label === "string" && label.length <= 64).slice(0, 20)
    : [];
  const dueAt = value.due_at === null || value.due_at === undefined ? null : time(value.due_at);
  const projectKey = typeof value.project_key === "string" && value.project_key.length <= 300 ? value.project_key : null;
  // Older desktops send no links: none.
  const links = value.links === undefined ? [] : list(value.links, 100, readLink);
  if (links === null) return null;
  return Object.freeze({
    id: taskId,
    key,
    title,
    description,
    descriptionTruncated: value.description_truncated === true,
    status: status as TaskStatus,
    priority: priority as TaskPriority,
    assignee,
    creator,
    project,
    projectKey,
    labels: Object.freeze(labels),
    links: Object.freeze(links),
    commentCount: typeof value.comment_count === "number" && value.comment_count >= 0 ? value.comment_count : 0,
    createdAt,
    updatedAt,
    dueAt,
  });
}

export function readComment(value: unknown): TaskComment | null {
  if (!isRecord(value)) return null;
  const commentId = id(value.id);
  const body = text(value.body, 40_000);
  const createdAt = time(value.created_at);
  const author = readPerson(value.author);
  if (commentId === null || body === null || createdAt === null || author === undefined) return null;
  return Object.freeze({
    id: commentId,
    mine: value.mine === true,
    author,
    byAgent: value.by_agent === true,
    body,
    createdAt,
  });
}

function readActivity(value: unknown): TaskActivity | null {
  if (!isRecord(value)) return null;
  const entryId = id(value.id);
  const kind = text(value.kind, 64);
  const at = time(value.at);
  const actor = readPerson(value.actor);
  if (entryId === null || kind === null || at === null || actor === undefined) return null;
  const simple = (raw: unknown) => (typeof raw === "string" ? raw.slice(0, 320) : null);
  return Object.freeze({
    id: entryId,
    kind,
    actor,
    byAgent: value.by_agent === true,
    from: simple(value.from),
    to: simple(value.to),
    at,
  });
}

function list<T>(value: unknown, max: number, read: (entry: unknown) => T | null): T[] | null {
  if (!Array.isArray(value) || value.length > max) return null;
  const out: T[] = [];
  for (const entry of value) {
    const parsed = read(entry);
    if (parsed === null) return null;
    out.push(parsed);
  }
  return out;
}

export function readTaskList(value: unknown): TaskListResult | null {
  if (!isRecord(value)) return null;
  const tasks = list(value.tasks, MAX_TASKS, readTask);
  if (tasks === null || typeof value.truncated !== "boolean") return null;
  return Object.freeze({
    prefix: typeof value.prefix === "string" ? value.prefix.slice(0, 16) : "",
    tasks: Object.freeze(tasks),
    truncated: value.truncated,
  });
}

export function readTaskDetail(value: unknown): TaskDetailResult | null {
  if (!isRecord(value)) return null;
  const task = readTask(value.task);
  const comments = list(value.comments, MAX_ENTRIES, readComment);
  const activity = list(value.activity, MAX_ENTRIES, readActivity);
  if (task === null || comments === null || activity === null) return null;
  return Object.freeze({ task, comments: Object.freeze(comments), activity: Object.freeze(activity) });
}

export function readTaskReply(value: unknown): LiveTask | null {
  return isRecord(value) ? readTask(value.task) : null;
}

export function readCommentReply(value: unknown): TaskComment | null {
  return isRecord(value) ? readComment(value.comment) : null;
}

export function readMembers(value: unknown): readonly TaskPerson[] | null {
  if (!isRecord(value)) return null;
  const members = list(value.members, 1000, (entry) => readPerson(entry) ?? null);
  return members === null ? null : Object.freeze(members);
}

function readStartProject(value: unknown): StartProject | null {
  if (!isRecord(value)) return null;
  const path = text(value.path, 4096);
  const label = text(value.label, 512);
  if (!path || label === null) return null;
  const key = typeof value.key === "string" && value.key.length <= 300 ? value.key : null;
  return Object.freeze({ path, label, key });
}

function readStartAgent(value: unknown): StartAgent | null {
  if (!isRecord(value)) return null;
  const key = text(value.key, 64);
  const label = text(value.label, 128);
  // Older desktops send no models (the desktop's own choice); a model that
  // can't be read is left out rather than losing the agent.
  const models = Array.isArray(value.models)
    ? value.models.slice(0, 100).flatMap((entry) => {
        if (!isRecord(entry)) return [];
        const id = text(entry.id, 128);
        const name = text(entry.label, 128);
        return id && name ? [Object.freeze({ id, label: name })] : [];
      })
    : [];
  return key && label ? Object.freeze({ key, label, models: Object.freeze(models) }) : null;
}

export function readTaskOptions(value: unknown): TaskOptions | null {
  if (!isRecord(value)) return null;
  const projects = list(value.projects, 500, readStartProject);
  const agents = list(value.agents, 100, readStartAgent);
  const members = value.members === undefined ? [] : list(value.members, 1000, (entry) => readPerson(entry) ?? null);
  const labels = value.labels === undefined ? [] : list(value.labels, 1000, (entry) => text(entry, 64));
  if (projects === null || agents === null || members === null || labels === null) return null;
  return Object.freeze({
    projects: Object.freeze(projects),
    agents: Object.freeze(agents),
    members: Object.freeze(members),
    labels: Object.freeze(labels),
  });
}

export function readStartResult(value: unknown): StartResult | null {
  if (!isRecord(value) || typeof value.started !== "boolean") return null;
  const reason = typeof value.reason === "string" ? value.reason.slice(0, 500) : null;
  return Object.freeze({ started: value.started, reason });
}

export function readEmpty(value: unknown): true | null {
  return isRecord(value) ? true : null;
}

export function readTrackers(value: unknown): readonly Tracker[] | null {
  if (!isRecord(value)) return null;
  const trackers = list(value.trackers, 50, (entry) => {
    if (!isRecord(entry)) return null;
    const key = text(entry.key, 32);
    const label = text(entry.label, 64);
    return key && label ? Object.freeze({ key, label }) : null;
  });
  return trackers === null ? null : Object.freeze(trackers);
}

export function readTrackerItem(value: unknown): TrackerItem | null {
  if (!isRecord(value)) return null;
  const itemId = text(value.id, 256);
  const key = text(value.key, 128);
  const title = text(value.title, 2000);
  if (!itemId || key === null || title === null) return null;
  if (typeof value.status !== "string" || !(TASK_STATUSES as readonly string[]).includes(value.status)) return null;
  const url = typeof value.url === "string" && /^https?:\/\//.test(value.url) && value.url.length <= 2048 ? value.url : null;
  const optional = (raw: unknown, max: number) => (typeof raw === "string" ? raw.slice(0, max) : null);
  return Object.freeze({
    id: itemId,
    key,
    title,
    url,
    status: value.status as TaskStatus,
    stateLabel: optional(value.state_label, 64),
    assignee: optional(value.assignee, 200),
    labels: Object.freeze(
      Array.isArray(value.labels) ? value.labels.filter((label): label is string => typeof label === "string").slice(0, 20) : [],
    ),
    updatedAt: time(value.updated_at),
    importedTask: optional(value.imported_task, 64),
  });
}

export function readTrackerItems(value: unknown): readonly TrackerItem[] | null {
  if (!isRecord(value)) return null;
  const items = list(value.items, 500, readTrackerItem);
  return items === null ? null : Object.freeze(items);
}

export function readTrackerItemReply(value: unknown): TrackerItem | null {
  return isRecord(value) ? readTrackerItem(value.item) : null;
}
