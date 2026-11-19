// The desktop screen lists every workspace under its project, so a
// workspace is one tap away; opening it lands on the tab that needs you.
import assert from "node:assert/strict";
import { test } from "node:test";

import type { RelayCatalog, RelayTab } from "../relayCatalog";
import { directoryAgents, openingTab, tabsNeedingYou, workspaceDirectory } from "../../machines/workspaceDirectory";

function tab(id: number, workspace: string, extra: Partial<RelayTab> = {}): RelayTab {
  return {
    id, kind: "terminal", title: `tab ${id}`, active: false, visible: false,
    workspace_id: workspace, project_id: "p1", agent_kind: null, agent_state: null,
    attention: null, agent_work: null, agent_step: null, agent_activity: null, ...extra,
  };
}

const catalog: RelayCatalog = {
  machine: null,
  projects: [
    { id: "p1", name: "Dragent", path: "/work/dragent" },
    { id: "p2", name: "Site", path: "/work/site" },
  ],
  workspaces: [
    { id: "w1", name: "main", path: "/work/dragent", branch: "main", project: null, project_id: "p1" },
    { id: "w2", name: "login", path: "/wt/login", branch: "login-flow", project: null, project_id: "p1" },
    { id: "w3", name: "landing", path: "/work/site", branch: null, project: null, project_id: "p2" },
    { id: "w4", name: "stray", path: "/tmp/stray", branch: "x", project: null, project_id: null },
  ],
  tabs: [
    tab(1, "w1", { active: true }),
    tab(2, "w2", { kind: "agent", agent_kind: "claude", agent_state: "working" }),
    tab(3, "w2", { kind: "agent", agent_kind: "codex", attention: "needs_input", title: "Fix auth" }),
    tab(4, "w3"),
  ],
};

test("workspaces group under their projects, the one needing you first", () => {
  const sections = workspaceDirectory(catalog);
  assert.deepEqual(sections.map((s) => s.title), ["Dragent", "Site", "Other workspaces"]);
  assert.deepEqual(sections[0]!.data.map((w) => w.name), ["login", "main"], "login needs input: first");
  assert.equal(sections[0]!.needs.needsInput, 1);
  assert.equal(sections[1]!.data[0]!.branch, "No branch");
});

test("opening a workspace lands on the tab that needs you", () => {
  const login = workspaceDirectory(catalog)[0]!.data[0]!;
  assert.equal(login.openTabId, 3, "the waiting Codex tab, not the working one");
  assert.equal(openingTab([tab(9, "w", { agent_state: "working" }), tab(8, "w", { active: true })])?.id, 8, "else the one in front");
  assert.equal(openingTab([]), null);
});

test("search matches workspace, branch, project and tab titles, and hides empty projects", () => {
  assert.deepEqual(workspaceDirectory(catalog, "login").map((s) => s.title), ["Dragent"]);
  assert.deepEqual(workspaceDirectory(catalog, "fix auth")[0]!.data.map((w) => w.name), ["login"]);
  assert.deepEqual(workspaceDirectory(catalog, "site").map((s) => s.title), ["Site"]);
  assert.equal(workspaceDirectory(catalog, "nothing like this").length, 0);
});

test("the tabs needing you, most urgent first", () => {
  assert.deepEqual(tabsNeedingYou(catalog).map((t) => t.id), [3]);
});

test("filters by status and agent, and sorts by name or activity, within projects", () => {
  const names = (sections: ReturnType<typeof workspaceDirectory>) => sections.map((s) => [s.title, s.data.map((w) => w.name)]);
  assert.deepEqual(names(workspaceDirectory(catalog, "", { status: "needs_you" })), [["Dragent", ["login"]]]);
  assert.deepEqual(names(workspaceDirectory(catalog, "", { status: "working" })), [["Dragent", ["login"]]]);
  assert.deepEqual(names(workspaceDirectory(catalog, "", { status: "failed" })), []);
  assert.deepEqual(names(workspaceDirectory(catalog, "", { agent: "codex" })), [["Dragent", ["login"]]]);
  assert.deepEqual(names(workspaceDirectory(catalog, "", { agent: "gemini" })), []);
  assert.deepEqual(directoryAgents(catalog), ["claude", "codex"]);
  assert.deepEqual(workspaceDirectory(catalog, "", { sort: "name" })[0]!.data.map((w) => w.name), ["login", "main"]);
  assert.deepEqual(workspaceDirectory(catalog, "", { sort: "active" })[0]!.data.map((w) => w.name), ["login", "main"]);
  // Search and filters together.
  assert.deepEqual(names(workspaceDirectory(catalog, "main", { status: "needs_you" })), []);
});
