import { test } from "node:test";
import assert from "node:assert/strict";
import { readTaskDetail, readTaskList } from "../../tasks/readers";
import { activityLine } from "../../tasks/types";

const task = {
  id: "t1",
  key: "DRA-1",
  title: "Fix login",
  description: "Steps…",
  description_truncated: false,
  status: "in_progress",
  priority: "high",
  assignee: { id: "u1", name: "ada@example.com" },
  creator: null,
  project: "app",
  labels: ["bug"],
  comment_count: 2,
  created_at: 1,
  updated_at: 2,
  due_at: null,
};

test("a task list from the desktop is read strictly", () => {
  const list = readTaskList({ prefix: "DRA", tasks: [task], truncated: false });
  assert.ok(list);
  assert.equal(list.tasks[0]?.key, "DRA-1");
  assert.equal(list.tasks[0]?.assignee?.name, "ada@example.com");
  assert.equal(readTaskList({ tasks: [{ ...task, status: "sideways" }], truncated: false }), null);
  assert.equal(readTaskList({ tasks: [task] }), null, "truncated is required");
});

test("a task detail carries comments and history", () => {
  const detail = readTaskDetail({
    task,
    comments: [{ id: "c1", author: null, by_agent: true, body: "Done", created_at: 3 }],
    activity: [{ id: "a1", kind: "status", actor: { id: "u1", name: "Ada" }, by_agent: false, from: "todo", to: "in_progress", at: 4 }],
  });
  assert.ok(detail);
  assert.equal(detail.comments[0]?.byAgent, true);
  assert.equal(activityLine(detail.activity[0]!), "Ada moved it to In progress");
  assert.equal(readTaskDetail({ task, comments: "x", activity: [] }), null);
});

test("links, project key and comment ownership come through; unsafe links don't open", async () => {
  const { readTask, readComment, readTaskOptions, readStartResult } = await import("../../tasks/readers");
  const read = readTask({
    ...task,
    project_key: "github.com/acme/app",
    links: [
      { id: "l1", kind: "pull_request", title: "Fix #1", url: "https://github.com/acme/app/pull/1", state: "open" },
      { id: "l2", kind: "item", title: "ENG-12", url: "javascript:alert(1)", state: null },
    ],
  });
  assert.ok(read);
  assert.equal(read.projectKey, "github.com/acme/app");
  assert.equal(read.links[0]?.url, "https://github.com/acme/app/pull/1");
  assert.equal(read.links[1]?.url, null);
  assert.equal(readTask({ ...task, links: [{ id: "l3", kind: "elsewhere", title: "x" }] }), null);
  assert.equal(readTask(task)?.links.length, 0, "older desktops send none");
  assert.equal(readComment({ id: "c1", author: null, body: "x", created_at: 1, mine: true })?.mine, true);
  assert.equal(readComment({ id: "c1", author: null, body: "x", created_at: 1 })?.mine, false);
  const options = readTaskOptions({
    projects: [{ path: "/repo", label: "repo", key: null }],
    agents: [{ key: "claude", label: "Claude Code" }],
    labels: ["bug"],
  });
  assert.equal(options?.agents[0]?.key, "claude");
  assert.equal(options?.members.length, 0);
  assert.equal(readTaskOptions({ projects: [{ path: "", label: "x" }], agents: [] }), null);
  assert.deepEqual(readStartResult({ started: false, reason: "Open it first." }), { started: false, reason: "Open it first." });
  assert.equal(readStartResult({}), null);
});

test("coordination detail reads the settings, the open question and tab ids", async () => {
  const { parseCoordinationDetailResult } = await import("../../coordination/readers");
  const result = parseCoordinationDetailResult({
    truncated: false,
    team: {
      id: "team-1",
      title: "Ship",
      project_path: "/repo",
      status: "running",
      goal: "Ship",
      tasks: [{ id: "t1", title: "A", status: "running", agent: "codex", model: null, depends_on: [], result_summary: null, worker_tab_id: 4294967297 }],
      messages: [],
      max_parallel: 3,
      auto_start: true,
      lead_answers_prompts: false,
      cost: "best",
      roster: ["claude", "codex"],
      question: { id: "9", kind: "needs_user", label: "Lead", text: "Which database?" },
      lead_tab_id: 4294967296,
    },
  });
  assert.ok(result);
  assert.equal(result.team.cost, "best");
  assert.equal(result.team.lead_answers_prompts, false);
  assert.equal(result.team.question?.text, "Which database?");
  assert.equal(result.team.lead_tab_id, 4294967296);
  assert.equal(result.team.tasks[0]?.worker_tab_id, 4294967297);
});

test("coordination detail reads task models, the new-lead flag and the clean-up plan", async () => {
  const { parseCoordinationDetailResult } = await import("../../coordination/readers");
  const result = parseCoordinationDetailResult({
    truncated: false,
    team: {
      id: "team-1",
      title: "Ship",
      project_path: "/repo",
      status: "done",
      goal: "Ship",
      tasks: [{ id: "t1", title: "A", status: "running", agent: "claude", model: "sonnet", depends_on: [], result_summary: null, model_locked: true, chosen_model: "opus", can_set_model: true }],
      messages: [],
      model_choices: { claude: [{ id: "opus", label: "Opus" }, { id: 4, label: "bad" }] },
      needs_new_lead: true,
      cleanup: { remove: ["t1 (repo-t1)"], keep: [], dirty: ["t2 (repo-t2)"] },
    },
  });
  assert.ok(result);
  const task = result.team.tasks[0]!;
  assert.equal(task.model_locked, true);
  assert.equal(task.chosen_model, "opus");
  assert.equal(task.can_set_model, true);
  assert.deepEqual(result.team.model_choices.claude, [{ id: "opus", label: "Opus" }]);
  assert.equal(result.team.needs_new_lead, true);
  assert.deepEqual(result.team.cleanup?.dirty, ["t2 (repo-t2)"]);
  const empty = parseCoordinationDetailResult({
    truncated: false,
    team: { id: "team-1", title: "x", project_path: "/repo", status: "done", goal: "x", tasks: [], messages: [], cleanup: { remove: [] } },
  });
  assert.equal(empty?.team.cleanup, null, "nothing to remove: no clean-up");
});

test("the due calendar lays out any month, and a picked day is local noon", async () => {
  const { monthGrid, dueDay } = await import("../../tasks/types");
  // November 2026 starts on a Sunday: the 1st is in the last column.
  const november = monthGrid(2026, 10);
  assert.deepEqual(november[0], [null, null, null, null, null, null, 1]);
  assert.equal(november.flat().filter((day) => day !== null).length, 30);
  assert.ok(november.every((week) => week.length === 7));
  assert.equal(monthGrid(2028, 1).flat().filter((day) => day !== null).length, 29, "a leap February");
  const picked = new Date(dueDay(2027, 2, 14));
  assert.deepEqual([picked.getFullYear(), picked.getMonth(), picked.getDate(), picked.getHours()], [2027, 2, 14, 12]);
});

test("tracker items are read strictly", async () => {
  const { readTrackers, readTrackerItems } = await import("../../tasks/readers");
  assert.deepEqual(readTrackers({ trackers: [{ key: "linear", label: "Linear" }] }), [{ key: "linear", label: "Linear" }]);
  const items = readTrackerItems({
    items: [
      { id: "abc", key: "ENG-12", title: "Fix", url: "https://linear.app/x", status: "in_progress", state_label: "In Progress", assignee: "Ada", labels: ["bug"], updated_at: 5, imported_task: "DRA-3" },
      { id: "def", key: "ENG-13", title: "Other", url: "file:///etc", status: "todo" },
    ],
  });
  assert.ok(items);
  assert.equal(items[0]?.importedTask, "DRA-3");
  assert.equal(items[1]?.url, null, "only web addresses open");
  assert.equal(readTrackerItems({ items: [{ id: "x", key: "k", title: "t", status: "lost" }] }), null);
});

test("starting several tasks waits for the desktop between them", async () => {
  const { startTasks } = await import("../../tasks/rpc");
  const answers = [
    { started: true },
    { started: false, reason: "The desktop is already starting a workspace. Try again in a moment." },
    { started: true },
    { started: false, reason: "DRA-3 isn't in a project. Choose the project to work in." },
  ];
  const calls: unknown[] = [];
  const client = {
    request: async (method: string, params: unknown) => {
      calls.push([method, params]);
      return answers.shift();
    },
  } as unknown as Parameters<typeof startTasks>[0];
  const heard: [string, boolean][] = [];
  let waited = 0;
  await startTasks(client, { tasks: ["DRA-1", "DRA-2", "DRA-3"], agent: "claude" }, (task, result) => heard.push([task, result.started]), async () => {
    waited += 1;
  });
  assert.deepEqual(heard, [["DRA-1", true], ["DRA-2", true], ["DRA-3", false]]);
  assert.equal(waited, 1, "waited once while the first was still opening");
  assert.equal(calls.length, 4);
});
