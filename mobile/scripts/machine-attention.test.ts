import assert from "node:assert/strict";
import { test } from "node:test";
import { parseRelayCatalog } from "../src/transport/relayCatalog";
import { countNeeds, mostUrgent, needDot, needLabel, needsSummary, NO_NEEDS, statusDot, tabNeed } from "../src/machines/attention";

function catalogWith(tabs: unknown[]) {
  const catalog = parseRelayCatalog({
    projects: [{ id: "/repo", name: "repo", path: "/repo" }],
    workspaces: [{ id: "w1", name: "main", path: "/repo", branch: "main", project: "repo", project_id: "/repo" }],
    tabs,
  });
  assert.ok(catalog, "catalog parses");
  return catalog;
}

const agent = (id: number, extra: Record<string, unknown>) => ({
  id,
  kind: "agent",
  title: `agent ${id}`,
  workspace_id: "w1",
  project_id: "/repo",
  agent_kind: "claude",
  ...extra,
});

test("a desktop's agents say what they wait for, most urgent first", () => {
  const catalog = catalogWith([
    agent(1, { agent_state: "review", attention: "needs_input" }),
    agent(2, { agent_state: "review", attention: "needs_input" }),
    agent(3, { agent_state: "review", attention: "failed" }),
    agent(4, { agent_state: "review" }),
    agent(5, { agent_state: "working" }),
    agent(6, { agent_state: "idle" }),
    { id: 7, kind: "terminal", title: "shell", workspace_id: "w1", project_id: "/repo" },
  ]);
  const counts = countNeeds(catalog.tabs);
  assert.deepEqual({ ...counts }, { needsInput: 2, failed: 1, review: 1, working: 1 });
  assert.equal(mostUrgent(counts), "needs_input");
  assert.equal(needLabel("needs_input", counts), "2 need input");
  assert.equal(needsSummary(counts), "2 need input · 1 failed · 1 to review");
  assert.equal(needDot("needs_input"), "waiting");
  assert.equal(needDot("failed"), "failed");
  assert.equal(needDot("review"), "done");
});

test("one waiting reads in the singular, and nothing waiting says nothing", () => {
  const one = countNeeds(catalogWith([agent(1, { agent_state: "review", attention: "needs_input" })]).tabs);
  assert.equal(needLabel("needs_input", one), "1 needs input");
  assert.equal(mostUrgent(NO_NEEDS), null);
  assert.equal(needsSummary(NO_NEEDS), null);
  const review = countNeeds(catalogWith([agent(1, { agent_state: "review" })]).tabs);
  assert.equal(mostUrgent(review), "review");
  assert.equal(needsSummary(review), "1 to review");
});

test("older desktops (no attention) still show review; unknown attention keeps the tab", () => {
  const catalog = catalogWith([
    agent(1, { agent_state: "review" }),
    agent(2, { agent_state: "review", attention: "on_fire" }),
  ]);
  assert.equal(catalog.tabs.length, 2, "an unknown value never drops the tab");
  assert.equal(catalog.tabs[1]?.attention, null);
  assert.equal(tabNeed(catalog.tabs[0]!), "review");
  assert.equal(tabNeed(catalog.tabs[1]!), "review");
});

test("a row's icon shows what matters most, in the desktop sidebar's order", () => {
  const counts = (tabs: unknown[]) => countNeeds(catalogWith(tabs).tabs);
  assert.equal(statusDot(counts([agent(1, { agent_state: "idle" })])), null);
  assert.equal(statusDot(counts([agent(1, { agent_state: "review" })])), "done");
  // Work in progress outranks a finished turn waiting to be looked at...
  assert.equal(
    statusDot(counts([agent(1, { agent_state: "review" }), agent(2, { agent_state: "working" })])),
    "working",
  );
  // ...and anything waiting on the person outranks both.
  assert.equal(
    statusDot(counts([agent(1, { agent_state: "working" }), agent(2, { agent_state: "review", attention: "failed" })])),
    "failed",
  );
  assert.equal(
    statusDot(counts([agent(1, { agent_state: "review", attention: "failed" }), agent(2, { agent_state: "review", attention: "needs_input" })])),
    "waiting",
  );
});
