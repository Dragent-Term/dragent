import assert from "node:assert/strict";
import { test } from "node:test";

import { parseWorkspaceDeleteResult } from "../transport/relayRpcMethods";
import type { RelayCatalog } from "../transport/relayCatalog";
import { blockedMessage } from "./workspaceDelete";
import { PINNED_SECTION, workspaceDirectory } from "./workspaceDirectory";

test("the desktop's delete answer is read strictly", () => {
  assert.deepEqual(parseWorkspaceDeleteResult({ deleted: true, kept_files: true }), { deleted: true, keptFiles: true });
  assert.deepEqual(parseWorkspaceDeleteResult({ deleted: true }), { deleted: true, keptFiles: false });
  assert.deepEqual(parseWorkspaceDeleteResult({ deleted: false, agents: ["codex (a1)", 4], changes: 3 }), {
    deleted: false,
    agents: ["codex (a1)"],
    changes: 3,
  });
  // A refusal that names nothing to lose is unreadable, not "deleted".
  assert.equal(parseWorkspaceDeleteResult({ deleted: false, agents: [], changes: 0 }), null);
  assert.equal(parseWorkspaceDeleteResult({ deleted: "yes" }), null);
  assert.equal(parseWorkspaceDeleteResult(null), null);
});

test("a held-back delete says what it would lose", () => {
  assert.equal(
    blockedMessage({ deleted: false, agents: ["claude (b2)"], changes: 1 }),
    "claude (b2) is still working in it. 1 uncommitted change would be lost.",
  );
  assert.equal(blockedMessage({ deleted: false, agents: ["a", "b"], changes: 0 }), "2 agents are still working in it.");
  assert.equal(blockedMessage({ deleted: false, agents: [], changes: 4 }), "4 uncommitted changes would be lost.");
});

const catalog: RelayCatalog = {
  machine: null,
  projects: [{ id: "p1", name: "app", path: "/src/app" }],
  workspaces: [
    { id: "w1", name: "main", path: "/src/app", branch: "main", project: null, project_id: "p1" },
    { id: "w2", name: "fix-auth", path: "/wt/fix-auth", branch: "fix-auth", project: null, project_id: "p1" },
    { id: "w3", name: "docs", path: "/wt/docs", branch: "docs", project: null, project_id: "p1" },
  ],
  tabs: [],
};

test("pinned workspaces leave their project for a Pinned section on top", () => {
  const sections = workspaceDirectory(catalog, "", { pinned: ["w3", "gone"] });
  assert.deepEqual(sections.map((s) => [s.key, s.data.map((w) => w.key)]), [
    [PINNED_SECTION, ["w3"]],
    ["p1", ["w1", "w2"]],
  ]);
  assert.equal(sections[0]!.title, "Pinned");
  assert.deepEqual(workspaceDirectory(catalog).map((s) => s.key), ["p1"], "no pins, no section");
});

test("the project's own checkout is told apart from a worktree", () => {
  const [project] = workspaceDirectory(catalog);
  assert.deepEqual(project!.data.map((w) => [w.key, w.mainCheckout]), [["w1", true], ["w2", false], ["w3", false]]);
});

test("home shows two blocked tabs, counts the rest, and folds reviews into one count", async () => {
  const { homeAttention } = await import("./workspaceDirectory");
  const t = (id: number, extra: object) => ({
    id, kind: "terminal" as const, title: `T${id}`, active: false, visible: true, workspace_id: "w2", project_id: "p1",
    agent_kind: "codex", agent_state: "idle" as const, attention: null, agent_work: null, agent_step: null, agent_activity: null, ...extra,
  });
  const many: RelayCatalog = {
    ...catalog,
    tabs: [
      t(1, { agent_state: "review" }), t(2, { attention: "failed" }), t(3, { agent_state: "review" }),
      t(4, { attention: "needs_input" }), t(5, { attention: "needs_input" }), t(6, { agent_state: "review" }),
      t(7, { agent_state: "working" }),
    ],
  };
  const home = homeAttention(many);
  assert.deepEqual(home.urgent.map((tab) => tab.id), [4, 5], "input first");
  assert.equal(home.moreUrgent, 1, "the failure is counted, not listed");
  assert.equal(home.reviews, 3);
  assert.deepEqual(homeAttention({ ...catalog, tabs: [t(1, { agent_state: "working" })] }), { urgent: [], moreUrgent: 0, reviews: 0 });
});

test("coordination work is listed once per coordination, under its lead's project", async () => {
  const { directoryTeams } = await import("./workspaceDirectory");
  const team = { id: "t1", title: "Auth rewrite", lead: false, task: "Fix login" };
  const withTeam: RelayCatalog = {
    ...catalog,
    workspaces: [
      catalog.workspaces[0]!,
      { ...catalog.workspaces[1]!, team },
      { ...catalog.workspaces[2]!, team: { ...team, lead: true, task: "Claude Code" } },
    ],
  };
  // Browsing: the project shows only its own work; the team is one entry, lead first.
  assert.deepEqual(workspaceDirectory(withTeam).map((s) => [s.key, s.data.map((w) => w.key)]), [["p1", ["w1"]]]);
  const teams = directoryTeams(withTeam);
  assert.deepEqual(teams.map((t) => [t.id, t.title, t.workspaces.map((w) => w.key)]), [["t1", "Auth rewrite", ["w3", "w2"]]]);
  assert.equal(teams[0]!.projectKey, withTeam.workspaces[2]!.project_id ?? withTeam.workspaces[2]!.project ?? "", "listed in the lead's project");
  // Searching finds coordination workspaces among the rest.
  assert.deepEqual(workspaceDirectory(withTeam, "fix").flatMap((s) => s.data.map((w) => w.key)), ["w2"]);
  // A pinned one stays in Pinned only.
  assert.deepEqual(directoryTeams(withTeam, ["w2"])[0]!.workspaces.map((w) => w.key), ["w3"]);
  assert.deepEqual(directoryTeams(catalog), [], "no coordinations");
});
