import type { StatusTone } from "../../components";
import type { PreviewSnapshot } from "../../preview/types";

/** Machine task status: open work vs finished work. Mirrors desktop inbox wording. */
export type TaskStatus = "open" | "closed";
export type TaskStatusFilter = TaskStatus | "all";

export interface PreviewTask {
  id: string;
  projectId: string;
  title: string;
  description: string;
  status: TaskStatus;
  author: string;
  ref: string;
  updatedAt: number;
}

export const TASK_STATUS: Record<TaskStatus, { label: string; tone: StatusTone }> = {
  open: { label: "Open", tone: "accent" },
  closed: { label: "Closed", tone: "neutral" },
};

export const TASK_STATUS_FILTERS = [
  { id: "open", label: "Open", hint: "Open tasks" },
  { id: "closed", label: "Closed", hint: "Closed tasks" },
  { id: "all", label: "All", hint: "Open and closed tasks" },
] as const;

export type TaskStatusFilterId = (typeof TASK_STATUS_FILTERS)[number]["id"];

export function parseTaskStatusFilter(value: unknown): TaskStatusFilter {
  return value === "closed" || value === "all" || value === "open" ? value : "open";
}

/**
 * Local sample tasks only: no coordinator, provider, network or persistence.
 * Project ids reuse the preview catalog so desktop ownership stays verifiable
 * through the shared snapshot; nothing here invents a new project.
 */
export const PREVIEW_TASKS: readonly PreviewTask[] = Object.freeze([
  {
    id: "task-empty-copy",
    projectId: "lumen-web",
    title: "Polish empty-state copy for narrow windows",
    description:
      "Sample task: keep the empty-state title short and give the reader one clear next action at 320px wide.",
    status: "open",
    author: "Maya",
    ref: "LUMEN-12",
    updatedAt: Date.UTC(2026, 0, 15, 9, 30),
  },
  {
    id: "task-keyboard-nav",
    projectId: "lumen-web",
    title: "Verify workspace navigation with keyboard",
    description:
      "Sample task: tab through projects, workspaces and tabs and confirm the focus order stays predictable.",
    status: "open",
    author: "You",
    ref: "LUMEN-13",
    updatedAt: Date.UTC(2026, 0, 15, 9, 10),
  },
  {
    id: "task-structure",
    projectId: "lumen-web",
    title: "Define project and workspace structure",
    description:
      "Sample task: agree on how projects group workspaces and branches before building navigation.",
    status: "closed",
    author: "Jo",
    ref: "LUMEN-09",
    updatedAt: Date.UTC(2026, 0, 14, 16, 0),
  },
  {
    id: "task-retry-docs",
    projectId: "relay-api",
    title: "Explain the retry window in the API docs",
    description:
      "Sample task: document the retry delays and window so callers know how long retries may take.",
    status: "open",
    author: "You",
    ref: "RELAY-04",
    updatedAt: Date.UTC(2026, 0, 15, 8, 20),
  },
  {
    id: "task-retry-bounds",
    projectId: "relay-api",
    title: "Keep retry attempts bounded",
    description: "Sample task: cap the retry schedule at three delays and record the limit.",
    status: "closed",
    author: "Maya",
    ref: "RELAY-03",
    updatedAt: Date.UTC(2026, 0, 14, 12, 0),
  },
  {
    id: "task-proofread",
    projectId: "field-notes",
    title: "Proofread the quick start",
    description:
      "Sample task: read the three-step outline aloud and fix any step that assumes desktop setup.",
    status: "open",
    author: "Jo",
    ref: "NOTES-02",
    updatedAt: Date.UTC(2026, 0, 15, 7, 45),
  },
  {
    id: "task-outline",
    projectId: "field-notes",
    title: "Write a three-step outline",
    description: "Sample task: outline choose-project, open-workspace and review in three short steps.",
    status: "closed",
    author: "You",
    ref: "NOTES-01",
    updatedAt: Date.UTC(2026, 0, 13, 10, 0),
  },
]);

export function taskProjectName(snapshot: PreviewSnapshot, projectId: string): string {
  return snapshot.projects.find((project) => project.id === projectId)?.name ?? "Unknown project";
};

/** Shared honest no-op copy for the preview-only Create-workspace action. */
export const CREATE_WORKSPACE_PREVIEW_MESSAGE =
  "Workspace creation is disabled in this local preview. Nothing was created — no branch or workspace was added.";

/** Project-style reference, e.g. `LUMEN-12` — matches the desktop inbox reference. */
export function taskReference(task: Pick<PreviewTask, "ref">): string {
  return task.ref;
}

/** Desktop `since()`: minutes, hours, then days from an epoch-millis timestamp. */
export function timeAgo(at: number, now: number = Date.now()): string {
  const minutes = Math.max(0, Math.floor((now - at) / 60000));
  if (!Number.isFinite(minutes)) return "unknown";
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  if (minutes < 1440) return `${Math.floor(minutes / 60)}h ago`;
  return `${Math.floor(minutes / 1440)}d ago`;
}

function taskOnDesktop(snapshot: PreviewSnapshot, desktopId: string, task: PreviewTask): boolean {
  if (!snapshot.desktops.some((desktop) => desktop.id === desktopId)) return false;
  return snapshot.projects.some(
    (project) => project.id === task.projectId && project.desktopId === desktopId,
  );
}

/** Strict desktop ownership via the shared snapshot; linked projects must match. */
export function selectPreviewTask(
  snapshot: PreviewSnapshot,
  desktopId: string,
  taskId: string,
): PreviewTask | null {
  const task = PREVIEW_TASKS.find((entry) => entry.id === taskId) ?? null;
  if (!task || !taskOnDesktop(snapshot, desktopId, task)) return null;
  return task;
}

export interface TaskFilterOptions {
  projectId?: string | undefined;
  status?: TaskStatusFilter | undefined;
  query?: string | undefined;
}

export function filterPreviewTasks(
  snapshot: PreviewSnapshot,
  desktopId: string,
  options: TaskFilterOptions = {},
): readonly PreviewTask[] {
  const { projectId, status = "open", query = "" } = options;
  if (status !== "open" && status !== "closed" && status !== "all") return [];
  const search = query.trim().toLocaleLowerCase();
  return PREVIEW_TASKS.filter((task) => {
    if (!taskOnDesktop(snapshot, desktopId, task)) return false;
    if (projectId && task.projectId !== projectId) return false;
    if (status !== "all" && task.status !== status) return false;
    if (!search) return true;
    const project = snapshot.projects.find((entry) => entry.id === task.projectId);
    return [task.title, task.description, task.author, task.ref, project?.name ?? ""]
      .join(" ")
      .toLocaleLowerCase()
      .includes(search);
  }).sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id));
}

export interface TaskSection {
  key: string;
  title: string;
  projectId: string;
  data: readonly PreviewTask[];
}

/** Group filtered rows by project for the inbox SectionList, by project name. */
export function groupTasksByProject(
  snapshot: PreviewSnapshot,
  tasks: readonly PreviewTask[],
): readonly TaskSection[] {
  const groups = new Map<string, PreviewTask[]>();
  for (const task of tasks) {
    const list = groups.get(task.projectId);
    if (list) list.push(task);
    else groups.set(task.projectId, [task]);
  }
  return [...groups.entries()]
    .map(([projectId, data]) => ({
      key: projectId,
      title: taskProjectName(snapshot, projectId),
      projectId,
      data: [...data].sort((a, b) => b.updatedAt - a.updatedAt || a.id.localeCompare(b.id)),
    }))
    .sort((a, b) => a.title.localeCompare(b.title) || a.projectId.localeCompare(b.projectId));
}
