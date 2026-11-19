import assert from "node:assert/strict";
import { test } from "node:test";
import {
  initialRelayCatalogState, parseRelayCatalogSnapshot,
} from "../src/transport/relayCatalogState";

test("the hook starts unavailable without a gateway or device id", () => {
  for (const source of [
    { gateway: null, deviceId: null },
    { gateway: "wss://relay.example/realtime", deviceId: null },
    { gateway: null, deviceId: "device-1" },
    { gateway: "", deviceId: "" },
  ]) {
    const state = initialRelayCatalogState(source);
    assert.equal(state.status, "unavailable");
    assert.equal(state.catalog, null);
    assert.match(state.reason ?? "", /live catalog|shared/i);
  }
});

test("the hook starts loading when a gateway and device id are known", () => {
  const state = initialRelayCatalogState({ gateway: "wss://relay.example/realtime", deviceId: "device-1" });
  assert.equal(state.status, "loading");
  assert.equal(state.catalog, null);
});

test("snapshot parsing accepts and rejects relay-shaped payloads", () => {
  assert.equal(parseRelayCatalogSnapshot(null), null);
  assert.equal(parseRelayCatalogSnapshot({ projects: [] }), null);
  // Unknown tab kinds are still rejected; the enriched agent kind is accepted
  // with explicit identity fields (never inferred).
  assert.equal(parseRelayCatalogSnapshot({
    projects: [{ name: "Web", path: "/repo" }],
    workspaces: [], tabs: [{ id: 1, kind: "editor", title: "Editor", active: true, visible: true }],
  }), null);
  const agent = parseRelayCatalogSnapshot({
    projects: [{ name: "Web", path: "/repo" }],
    workspaces: [],
    tabs: [{ id: 1, kind: "agent", title: "Agent", active: true, visible: true, agent_kind: "codex", agent_state: "working" }],
  });
  assert.ok(agent);
  assert.equal(agent!.tabs[0]!.agent_kind, "codex");
  assert.equal(agent!.tabs[0]!.workspace_id, null);
  const catalog = parseRelayCatalogSnapshot({
    projects: [{ name: "Web", path: "/repo" }],
    workspaces: [{ name: "Nav", path: "/repo", branch: "main", project: "Web" }],
    tabs: [{ id: 7, kind: "terminal", title: "Term", active: true, visible: true }],
  });
  assert.ok(catalog);
  assert.equal(catalog!.projects.length, 1);
  assert.equal(catalog!.workspaces[0]!.branch, "main");
  assert.equal(catalog!.tabs[0]!.id, 7);
});
