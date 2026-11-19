// A new agent session from the phone: what the request carries, and the
// agents (with their models) the desktop offers.
import assert from "node:assert/strict";
import { test } from "node:test";
import { sessionParams } from "../src/sessions/rpc";
import { readTaskOptions } from "../src/tasks/readers";

test("a new session names its project, agent and task, and only what was chosen", () => {
  assert.deepEqual(sessionParams({ project: "/repo", agent: "claude", prompt: "  Fix login  " }), {
    project: "/repo", agent: "claude", prompt: "Fix login",
  });
  assert.deepEqual(
    sessionParams({ project: "/repo", agent: "codex", prompt: "x", model: "gpt-5", branch: "main", workspace: null }),
    { project: "/repo", agent: "codex", prompt: "x", model: "gpt-5", branch: "main" },
  );
  assert.deepEqual(sessionParams({ project: "/repo", agent: "claude", prompt: "x", workspace: "/repo/.wt/a" }), {
    project: "/repo", agent: "claude", prompt: "x", workspace: "/repo/.wt/a",
  });
});

test("an empty task, a missing project or agent, or a branch for an existing workspace never leave the phone", () => {
  assert.equal(sessionParams({ project: "/repo", agent: "claude", prompt: "   " }), null);
  assert.equal(sessionParams({ project: "", agent: "claude", prompt: "x" }), null);
  assert.equal(sessionParams({ project: "/repo", agent: "", prompt: "x" }), null);
  assert.equal(sessionParams({ project: "/repo", agent: "claude", prompt: "x", workspace: "/w", branch: "main" }), null);
  assert.equal(sessionParams({ project: "/repo", agent: "claude", prompt: "x".repeat(8001) }), null);
});

test("agents come with their models; older desktops send none", () => {
  const options = readTaskOptions({
    projects: [{ path: "/repo", label: "repo", key: null }],
    agents: [
      { key: "dragent", label: "Dragent", models: [] },
      { key: "claude", label: "Claude Code", models: [{ id: "opus", label: "Opus" }, { id: 7, label: "bad" }] },
      { key: "codex", label: "Codex" },
    ],
  });
  assert.ok(options);
  assert.deepEqual(options.agents.map((agent) => [agent.key, agent.models.map((model) => model.id)]), [
    ["dragent", []],
    ["claude", ["opus"]],
    ["codex", []],
  ]);
});
