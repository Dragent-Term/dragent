import assert from "node:assert/strict";
import { test } from "node:test";

import type { RelayTab } from "../transport/relayCatalog";
import { groupWorkspaceTabs, hiddenAttention, othersNeedingYou, tabGroup } from "./workspaceTabs";

function tab(id: number, extra: Partial<RelayTab> = {}): RelayTab {
  return {
    id,
    kind: "terminal",
    title: `Tab ${id}`,
    active: false,
    visible: true,
    workspace_id: "w1",
    project_id: "p1",
    agent_kind: "claude",
    agent_state: "idle",
    attention: null,
    agent_work: null,
    agent_step: null,
    agent_activity: null,
    ...extra,
  };
}

test("a tab waits for the person, works, or neither", () => {
  assert.equal(tabGroup(tab(1, { attention: "needs_input" })), "needs_you");
  assert.equal(tabGroup(tab(2, { attention: "failed" })), "needs_you");
  assert.equal(tabGroup(tab(3, { agent_state: "review" })), "needs_you");
  assert.equal(tabGroup(tab(4, { agent_state: "working" })), "working");
  assert.equal(tabGroup(tab(5)), "other");
});

test("the workspace's tabs group as Needs you, Working, Other, input first", () => {
  const sections = groupWorkspaceTabs([
    tab(1),
    tab(2, { agent_state: "review" }),
    tab(3, { agent_state: "working" }),
    tab(4, { attention: "failed" }),
    tab(5, { attention: "needs_input" }),
  ]);
  assert.deepEqual(sections.map((s) => [s.key, s.title, s.data.map((t) => t.id)]), [
    ["needs_you", "Needs you", [5, 4, 2]],
    ["working", "Working", [3]],
    ["other", "Other tabs", [1]],
  ]);
  assert.deepEqual(groupWorkspaceTabs([tab(1)]).map((s) => s.key), ["other"], "empty groups are left out");
});

test("the strip counts the other tabs that need the person", () => {
  const tabs = [tab(1, { attention: "needs_input" }), tab(2, { agent_state: "review" }), tab(3)];
  assert.equal(othersNeedingYou(tabs, 1), 1);
  assert.equal(othersNeedingYou(tabs, 3), 2);
});

test("a tab needing the person out of sight lights that edge of the strip", () => {
  const layouts = new Map([
    ["a", { x: 0, width: 100 }],
    ["b", { x: 100, width: 100 }],
    ["c", { x: 500, width: 100 }],
  ]);
  // Viewport shows 150..450.
  assert.deepEqual(hiddenAttention(layouts, new Set(["a"]), 150, 300), { left: true, right: false });
  assert.deepEqual(hiddenAttention(layouts, new Set(["c"]), 150, 300), { left: false, right: true });
  assert.deepEqual(hiddenAttention(layouts, new Set(["b"]), 150, 300), { left: false, right: false }, "partly shown");
  assert.deepEqual(hiddenAttention(layouts, new Set(["a", "c"]), 150, 0), { left: false, right: false }, "not laid out yet");
});
