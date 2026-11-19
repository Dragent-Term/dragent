/**
 * The organisation's Tasks as the phone sees them, through the paired
 * desktop (`realtime/companion_tasks.rs` serves `tasks.*` inside the
 * end-to-end encrypted channel with the desktop's own sign-in).
 */
import type { StatusTone } from "../components";

/** The six statuses every organisation uses, in board order. */
export type TaskStatus = "backlog" | "todo" | "in_progress" | "in_review" | "done" | "canceled";

export const TASK_STATUSES: readonly TaskStatus[] = Object.freeze([
  "backlog",
  "todo",
  "in_progress",
  "in_review",
  "done",
  "canceled",
]);

export const TASK_STATUS_META: Record<TaskStatus, { label: string; tone: StatusTone }> = {
  backlog: { label: "Backlog", tone: "neutral" },
  todo: { label: "Todo", tone: "neutral" },
  in_progress: { label: "In progress", tone: "accent" },
  in_review: { label: "In review", tone: "warning" },
  done: { label: "Done", tone: "success" },
  canceled: { label: "Canceled", tone: "neutral" },
};

export type TaskPriority = "none" | "urgent" | "high" | "medium" | "low";

export const TASK_PRIORITIES: readonly TaskPriority[] = Object.freeze(["urgent", "high", "medium", "low", "none"]);

export const TASK_PRIORITY_LABEL: Record<TaskPriority, string> = {
  urgent: "Urgent",
  high: "High",
  medium: "Medium",
  low: "Low",
  none: "No priority",
};

/** Which tasks a list shows. */
export type TaskListFilter = "open" | "closed" | "all";

export interface TaskPerson {
  readonly id: string;
  readonly name: string;
}

export interface LiveTask {
  readonly id: string;
  /** `DRA-12`. */
  readonly key: string;
  readonly title: string;
  readonly description: string;
  readonly descriptionTruncated: boolean;
  readonly status: TaskStatus;
  readonly priority: TaskPriority;
  readonly assignee: TaskPerson | null;
  readonly creator: TaskPerson | null;
  readonly project: string | null;
  /** The shared project key (`github.com/acme/app`); null when none. */
  readonly projectKey: string | null;
  readonly labels: readonly string[];
  /** Linked workspaces, pull requests and tracker items. */
  readonly links: readonly TaskLink[];
  readonly commentCount: number;
  readonly createdAt: number;
  readonly updatedAt: number;
  readonly dueAt: number | null;
}

export type TaskLinkKind = "workspace" | "pull_request" | "item";

export interface TaskLink {
  readonly id: string;
  readonly kind: TaskLinkKind;
  readonly title: string;
  /** A web address (never for a workspace, which is a folder on one machine). */
  readonly url: string | null;
  readonly state: string | null;
  /** A workspace on the paired desktop: its folder (to open it here). */
  readonly workspaceRoot: string | null;
}

export interface TaskComment {
  readonly id: string;
  /** Written by you: you may edit or delete it. */
  readonly mine: boolean;
  readonly author: TaskPerson | null;
  readonly byAgent: boolean;
  readonly body: string;
  readonly createdAt: number;
}

export interface TaskActivity {
  readonly id: string;
  /** `created`, `status`, `assignee`, … */
  readonly kind: string;
  readonly actor: TaskPerson | null;
  readonly byAgent: boolean;
  readonly from: string | null;
  readonly to: string | null;
  readonly at: number;
}

export interface TaskListResult {
  readonly prefix: string;
  readonly tasks: readonly LiveTask[];
  readonly truncated: boolean;
}

export interface TaskDetailResult {
  readonly task: LiveTask;
  readonly comments: readonly TaskComment[];
  readonly activity: readonly TaskActivity[];
}

/** One line for a history entry: "Ada moved it to In progress". */
export function activityLine(entry: TaskActivity): string {
  const who = entry.actor?.name ?? "Someone";
  const via = entry.byAgent ? " (through an agent)" : "";
  const to = entry.to ?? "";
  const statusLabel = (key: string) =>
    (TASK_STATUS_META as Record<string, { label: string } | undefined>)[key]?.label ?? key;
  const priorityLabel = (key: string) =>
    (TASK_PRIORITY_LABEL as Record<string, string | undefined>)[key] ?? key;
  switch (entry.kind) {
    case "created":
      return `${who}${via} created it`;
    case "status":
      return `${who}${via} moved it to ${statusLabel(to)}`;
    case "priority":
      return `${who}${via} set priority to ${priorityLabel(to)}`;
    case "assignee":
      return to ? `${who}${via} assigned it to ${to}` : `${who}${via} unassigned it`;
    case "title":
      return `${who}${via} renamed it`;
    case "description":
      return `${who}${via} edited the description`;
    case "labels":
      return `${who}${via} changed labels`;
    case "deleted":
      return `${who}${via} deleted it`;
    case "restored":
      return `${who}${via} restored it`;
    default:
      return `${who}${via} changed ${entry.kind.replace(/_/g, " ")}`;
  }
}

/** "just now", "5m", "3h", "2d", or a date. */
export function shortAge(ms: number, now = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.floor(seconds / 3600)}h`;
  if (seconds < 30 * 86_400) return `${Math.floor(seconds / 86_400)}d`;
  return new Date(ms).toLocaleDateString();
}

export interface StartProject {
  /** The folder on the desktop (the catalog's project id). */
  readonly path: string;
  readonly label: string;
  /** Its shared project key, when it has a remote. */
  readonly key: string | null;
}

export interface StartAgent {
  /** `dragent`, `claude`, `codex`, … */
  readonly key: string;
  readonly label: string;
  /** Models the agent can run (empty: the desktop's choice). */
  readonly models: readonly StartModel[];
}

export interface StartModel {
  readonly id: string;
  readonly label: string;
}

/** What the start forms and pickers offer, from the desktop. */
export interface TaskOptions {
  readonly members: readonly TaskPerson[];
  readonly labels: readonly string[];
  readonly projects: readonly StartProject[];
  readonly agents: readonly StartAgent[];
}

export interface StartResult {
  readonly started: boolean;
  /** Why it didn't start, in plain words. */
  readonly reason: string | null;
}

/** A due day, as the desktop stores one: local noon that day. */
export function dueDay(year: number, month: number, day: number): number {
  return new Date(year, month, day, 12, 0, 0, 0).getTime();
}

/** Due date presets, counted in days from today. */
export function dueOn(daysFromToday: number, now = new Date()): number {
  return dueDay(now.getFullYear(), now.getMonth(), now.getDate() + daysFromToday);
}

/** The weeks of a month (month 0-11), Monday first; null outside it. */
export function monthGrid(year: number, month: number): (number | null)[][] {
  const days = new Date(year, month + 1, 0).getDate();
  const offset = (new Date(year, month, 1).getDay() + 6) % 7;
  const weeks: (number | null)[][] = [];
  let week: (number | null)[] = Array.from({ length: offset }, () => null);
  for (let day = 1; day <= days; day += 1) {
    week.push(day);
    if (week.length === 7) {
      weeks.push(week);
      week = [];
    }
  }
  if (week.length > 0) weeks.push([...week, ...Array.from({ length: 7 - week.length }, () => null)]);
  return weeks;
}

/** A connected tracker (Linear, GitHub, …). */
export interface Tracker {
  readonly key: string;
  readonly label: string;
}

/** An item in a connected tracker, shaped like a task row. */
export interface TrackerItem {
  readonly id: string;
  readonly key: string;
  readonly title: string;
  readonly url: string | null;
  readonly status: TaskStatus;
  /** The tracker's own word for its state ("In Progress"). */
  readonly stateLabel: string | null;
  readonly assignee: string | null;
  readonly labels: readonly string[];
  readonly updatedAt: number | null;
  /** The task it was saved as, when it was. */
  readonly importedTask: string | null;
}

export function dueLabel(dueAt: number, now = Date.now()): string {
  const date = new Date(dueAt);
  const today = new Date(now);
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(date) - startOf(today)) / 86_400_000);
  if (days === 0) return "Due today";
  if (days === 1) return "Due tomorrow";
  if (days === -1) return "Due yesterday";
  if (days < 0) return `Overdue ${-days} days`;
  if (days < 7) return `Due in ${days} days`;
  return `Due ${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
}
