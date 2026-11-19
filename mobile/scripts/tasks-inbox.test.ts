import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { PreviewStore } from "../src/preview/PreviewStore";
import { BUNDLED_THEMES } from "../src/theme/catalog.generated";
import {
  CREATE_WORKSPACE_PREVIEW_MESSAGE,
  PREVIEW_TASKS,
  TASK_STATUS,
  TASK_STATUS_FILTERS,
  filterPreviewTasks,
  groupTasksByProject,
  parseTaskStatusFilter,
  selectPreviewTask,
  taskProjectName,
  taskReference,
  timeAgo,
} from "../src/features/tasks/taskSamples";

const here = dirname(fileURLToPath(import.meta.url));
const tasksDir = join(here, "..", "src", "features", "tasks");
const readTaskFile = (name: string): string => readFileSync(join(tasksDir, name), "utf8");

function snapshot() {
  return new PreviewStore().getSnapshot();
}

test("task selectors validate desktop and project ownership like other machine links", () => {
  const before = snapshot();
  for (const task of PREVIEW_TASKS) {
    const project = before.projects.find((entry) => entry.id === task.projectId)!;
    assert.ok(project);
    assert.ok(selectPreviewTask(before, project.desktopId, task.id));
  }
  assert.equal(selectPreviewTask(before, "preview-linux", "task-empty-copy"), null);
  assert.equal(selectPreviewTask(before, "preview-mac", "task-retry-docs"), null);
  assert.equal(selectPreviewTask(before, "missing", "task-empty-copy"), null);
  assert.equal(selectPreviewTask(before, "preview-mac", "missing"), null);
  assert.equal(before.pullRequests.length > 0, true, "task samples must not disturb other machine areas");
});

test("task inbox filters by project and status (open/closed), defaulting to open", () => {
  const before = snapshot();
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac").map((task) => task.id),
    ["task-empty-copy", "task-keyboard-nav", "task-proofread"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "closed" }).map((task) => task.id),
    ["task-structure", "task-outline"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all" }).map((task) => task.id),
    ["task-empty-copy", "task-keyboard-nav", "task-proofread", "task-structure", "task-outline"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-linux").map((task) => task.id),
    ["task-retry-docs"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-linux", { status: "all" }).map((task) => task.id),
    ["task-retry-docs", "task-retry-bounds"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { projectId: "lumen-web", status: "all" }).map(
      (task) => task.id,
    ),
    ["task-empty-copy", "task-keyboard-nav", "task-structure"],
  );
  assert.deepEqual(filterPreviewTasks(before, "preview-mac", { projectId: "relay-api" }), []);
  assert.deepEqual(filterPreviewTasks(before, "missing", { status: "all" }), []);
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "bogus" as never }),
    [],
  );
  assert.equal(parseTaskStatusFilter("closed"), "closed");
  assert.equal(parseTaskStatusFilter("all"), "all");
  assert.equal(parseTaskStatusFilter("open"), "open");
  assert.equal(parseTaskStatusFilter("bogus"), "open");
  assert.equal(parseTaskStatusFilter(undefined), "open");
});

test("task inbox search matches title, description, author and ref without leaving preview", () => {
  const before = snapshot();
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "  KEYBOARD " }).map(
      (task) => task.id,
    ),
    ["task-keyboard-nav"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "bounded retry" }).map(
      (task) => task.id,
    ),
    [],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-linux", { status: "all", query: "bounded" }).map(
      (task) => task.id,
    ),
    ["task-retry-bounds"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "maya" }).map((task) => task.id),
    ["task-empty-copy"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "lumen-09" }).map(
      (task) => task.id,
    ),
    ["task-structure"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "Field notes" }).map(
      (task) => task.id,
    ),
    ["task-proofread", "task-outline"],
  );
  assert.deepEqual(
    filterPreviewTasks(before, "preview-mac", { status: "all", query: "nothing matches this" }),
    [],
  );
});

test("task inbox groups rows by project for the sectioned list", () => {
  const before = snapshot();
  const sections = groupTasksByProject(
    before,
    filterPreviewTasks(before, "preview-mac", { status: "all" }),
  );
  assert.deepEqual(
    sections.map((section) => [section.key, section.title]),
    [
      ["field-notes", "Field notes"],
      ["lumen-web", "Lumen web"],
    ],
  );
  assert.deepEqual(
    sections[0]!.data.map((task) => task.id),
    ["task-proofread", "task-outline"],
  );
  assert.deepEqual(
    sections[1]!.data.map((task) => task.id),
    ["task-empty-copy", "task-keyboard-nav", "task-structure"],
  );
  assert.deepEqual(groupTasksByProject(before, []), []);
  assert.equal(groupTasksByProject(before, filterPreviewTasks(before, "preview-linux"))[0]!.title, "Relay API");
});

test("task status, references and timestamps reuse the shared compact row language", () => {
  assert.deepEqual(TASK_STATUS.open, { label: "Open", tone: "accent" });
  assert.deepEqual(TASK_STATUS.closed, { label: "Closed", tone: "neutral" });
  assert.deepEqual(
    TASK_STATUS_FILTERS.map((option) => option.id),
    ["open", "closed", "all"],
  );
  assert.equal(taskReference({ ref: "LUMEN-12" }), "LUMEN-12");
  assert.equal(taskProjectName(snapshot(), "lumen-web"), "Lumen web");
  assert.equal(taskProjectName(snapshot(), "missing"), "Unknown project");
  assert.equal(timeAgo(1000, 1000), "just now");
  assert.equal(timeAgo(0, 30 * 60000), "30m ago");
  assert.equal(timeAgo(0, 3 * 3600000), "3h ago");
  assert.equal(timeAgo(0, 3 * 86400000), "3d ago");
  assert.equal(timeAgo(NaN, NaN), "unknown");
});

test("task status tones resolve on all four themes without hardcoded colors", () => {
  assert.equal(BUNDLED_THEMES.length, 4);
  assert.deepEqual(
    BUNDLED_THEMES.map((theme) => theme.id).sort(),
    ["dark", "gruvbox_dark", "gruvbox_light", "light"],
  );
  for (const theme of BUNDLED_THEMES) {
    for (const status of Object.values(TASK_STATUS)) {
      const dot = status.tone === "neutral" ? "textSubtle" : status.tone;
      assert.ok(
        theme.colors[dot as keyof typeof theme.colors],
        `${theme.id} is missing ${dot} for ${status.label}`,
      );
    }
  }
  for (const name of ["TasksArea.tsx", "TaskDetail.tsx", "taskSamples.ts"]) {
    const source = readTaskFile(name);
    assert.doesNotMatch(source, /#[0-9a-fA-F]{6}/, `${name} must use theme colors, not hex literals`);
    assert.doesNotMatch(source, /fetch\(|WebSocket|localStorage/, `${name} must stay offline`);
  }
});

test("tasks screens reuse shared components and stay honest about preview limits", () => {
  const area = readTaskFile("TasksArea.tsx");
  for (const token of [
    "StatusBadge",
    "SelectPopover",
    "SegmentedControl",
    "SearchField",
    "EmptyState",
    "LoadingState",
    "Avatar",
    "useTheme",
    "groupTasksByProject",
    "filterPreviewTasks",
    "No tasks match these filters",
    "Preview refreshed locally. No provider was contacted.",
    "Sample tasks on",
    "minWidth: 0",
    "flexWrap",
    "numberOfLines",
  ]) {
    assert.ok(area.includes(token), `TasksArea.tsx is missing ${token}`);
  }
  const detail = readTaskFile("TaskDetail.tsx");
  for (const token of [
    "Card",
    "StatusBadge",
    "EmptyState",
    "Feedback",
    "selectPreviewTask",
    "Create workspace (preview)",
    "No discussion yet",
    "Description",
    "Status",
    "minWidth: 0",
  ]) {
    assert.ok(detail.includes(token), `TaskDetail.tsx is missing ${token}`);
  }
  assert.ok(
    detail.includes("CREATE_WORKSPACE_PREVIEW_MESSAGE"),
    "detail preview action must share one honest no-op message",
  );
  assert.ok(
    readTaskFile("taskSamples.ts").includes(CREATE_WORKSPACE_PREVIEW_MESSAGE),
    "shared preview message must live in the pure task model",
  );
  assert.match(CREATE_WORKSPACE_PREVIEW_MESSAGE, /disabled in this local preview/);
  assert.match(CREATE_WORKSPACE_PREVIEW_MESSAGE, /Nothing was created/);
});
