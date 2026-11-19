import assert from "node:assert/strict";
import { test } from "node:test";
import { cardContent, summaryOf, type CardRow } from "../../agentCard/model";
import { getCard, publishDesktop, resetCardStore } from "../../agentCard/store";
import type { RelayCatalog, RelayTab } from "../relayCatalog";

const row = (key: string, state: CardRow["state"], since: number): CardRow =>
  ({ key, desktopId: "d1", tabId: 1, title: key, place: "ws · Mac", state, since });

test("the card puts what wants you first, then the most recent", () => {
  const card = cardContent([row("busy", "working", 50), row("done", "review", 10), row("ask", "needs_input", 5), row("done2", "review", 20)]);
  assert.ok(card);
  assert.deepEqual(card.rows.map((r) => r.key), ["ask", "done2", "done", "busy"]);
  assert.equal(card.summary, "1 needs you · 2 done · 1 working");
  assert.equal(cardContent([]), null, "nothing working or waiting: no card");
  assert.equal(summaryOf(2, 0, 0), "2 need you");
});

const tab = (id: number, patch: Partial<RelayTab>): RelayTab =>
  ({ id, kind: "terminal", title: `tab ${id}`, active: false, visible: true, workspace_id: "w1", project_id: null, agent_kind: "claude", agent_state: null, attention: null, agent_work: null, ...patch }) as RelayTab;

const catalog = (tabs: RelayTab[]): RelayCatalog =>
  ({ tabs, workspaces: [{ id: "w1", name: "api", path: "/x", branch: null, project: null, project_id: null }], projects: [] }) as unknown as RelayCatalog;

test("each agent keeps the time it entered its state, across updates", () => {
  resetCardStore();
  publishDesktop("d1", "Mac", catalog([tab(1, { agent_state: "working" }), tab(2, {})]), 1000);
  assert.equal(getCard()?.rows.length, 1, "an idle tab isn't on the card");
  assert.equal(getCard()?.rows[0]?.place, "api · Mac");
  publishDesktop("d1", "Mac", catalog([tab(1, { agent_state: "working" })]), 5000);
  assert.equal(getCard()?.rows[0]?.since, 1000, "still working: same start");
  publishDesktop("d1", "Mac", catalog([tab(1, { agent_state: "review" })]), 9000);
  assert.equal(getCard()?.rows[0]?.since, 9000, "a new state starts its own clock");
  publishDesktop("d1", "Mac", null);
  assert.equal(getCard(), null, "the desktop went away");
});
