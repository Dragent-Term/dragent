import assert from "node:assert/strict";
import { test } from "node:test";
import { parseRelayCatalog } from "../src/transport/relayCatalog";
import {
  buildTabStripItems,
  liveTabDot,
  liveTabKindLabel,
  liveTabStripLabel,
  workspaceNewTabRoute,
} from "../src/machines/tabStrip";

function catalogWith(tabs: unknown[]) {
  const catalog = parseRelayCatalog({
    projects: [{ id: "/repo", name: "repo", path: "/repo" }],
    workspaces: [{ id: "w1", name: "main", path: "/repo", branch: "main", project: "repo", project_id: "/repo" }],
    tabs,
  });
  assert.ok(catalog, "catalog parses");
  return catalog;
}

const tab = (id: number, extra: Record<string, unknown> = {}) => ({
  id,
  kind: "terminal",
  title: "",
  workspace_id: "w1",
  project_id: "/repo",
  ...extra,
});

test("strip labels use the title, falling back to the kind like the tab row", () => {
  const catalog = catalogWith([
    tab(1, { kind: "agent", agent_kind: "claude", title: "Auth flow" }),
    tab(2, { kind: "agent", agent_kind: "claude" }),
    tab(3, { kind: "agent" }),
    tab(4, { kind: "terminal", agent_kind: "codex" }),
    tab(5, { kind: "terminal" }),
    tab(6, { kind: "browser" }),
    tab(7, { kind: "settings" }),
  ]);
  const [titled, agent, bareAgent, termAgent, term, browser, settings] = catalog.tabs;
  assert.equal(liveTabStripLabel(titled!), "Auth flow");
  assert.equal(liveTabStripLabel(agent!), "claude tab");
  assert.equal(liveTabStripLabel(bareAgent!), "Agent tab");
  assert.equal(liveTabStripLabel(termAgent!), "Terminal · codex");
  assert.equal(liveTabStripLabel(term!), "Terminal tab");
  assert.equal(liveTabStripLabel(browser!), "Browser tab");
  assert.equal(liveTabStripLabel(settings!), "Settings tab");
  assert.equal(liveTabKindLabel(agent!), "claude tab");
  assert.equal(liveTabKindLabel(term!), "Terminal tab");
});

test("one tab's dot follows what it waits for, idle when nothing is going on", () => {
  const waiting = catalogWith([tab(1, { kind: "agent", agent_kind: "claude", agent_state: "review", attention: "needs_input" })]).tabs[0]!;
  const failed = catalogWith([tab(1, { kind: "agent", agent_kind: "claude", agent_state: "review", attention: "failed" })]).tabs[0]!;
  const working = catalogWith([tab(1, { kind: "agent", agent_kind: "claude", agent_state: "working" })]).tabs[0]!;
  const review = catalogWith([tab(1, { kind: "agent", agent_kind: "claude", agent_state: "review" })]).tabs[0]!;
  const idle = catalogWith([tab(1, { agent_state: "idle" })]).tabs[0]!;
  assert.equal(liveTabDot(waiting), "waiting");
  assert.equal(liveTabDot(failed), "failed");
  assert.equal(liveTabDot(working), "working");
  assert.equal(liveTabDot(review), "done");
  assert.equal(liveTabDot(idle), "idle");
});

test("strip options keep catalog order with string ids and the agent identity", () => {
  const catalog = catalogWith([
    tab(11, { kind: "agent", agent_kind: "claude", title: "Auth flow", agent_state: "working" }),
    tab(12, { kind: "terminal", title: "shell" }),
    tab(13, { kind: "terminal" }),
  ]);
  const items = buildTabStripItems(catalog.tabs);
  assert.equal(items.length, 3);
  assert.deepEqual(items.map((item) => item.tabId), [11, 12, 13]);
  assert.deepEqual(items.map((item) => item.id), ["11", "12", "13"]);
  assert.deepEqual(items.map((item) => item.label), ["Auth flow", "shell", "Terminal tab"]);
  assert.deepEqual(items.map((item) => item.dot), ["working", "idle", "idle"]);
  assert.deepEqual(items.map((item) => item.agent), ["claude", null, null]);
  assert.equal(items[0]?.kind, "agent");
  assert.equal(items[0]?.kindLabel, "claude tab");
  assert.equal(items[0]?.title, "Auth flow");
});

test("new tab routes to the workspace screen with its new-tab menu open", () => {
  const route = workspaceNewTabRoute("dev-1", "w1");
  assert.equal(route.pathname, "/machine/[deviceId]/workspace/[workspaceId]");
  assert.deepEqual(route.params, { deviceId: "dev-1", workspaceId: "w1", newTab: "1" });
});
