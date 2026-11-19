import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import {
  RelayCatalogError, dragentActivity, dragentStatusLine, fetchRelayCatalog, isDragentTab, isRelayTabKind, parseRelayCatalog, resolveRelayCatalogURL,
} from "../src/transport/relayCatalog";

test("catalog URL derives from the realtime gateway and rejects credential carriers", () => {
  assert.equal(resolveRelayCatalogURL("wss://example.test/realtime"), "https://example.test/v1/relay/catalog");
  assert.equal(resolveRelayCatalogURL("ws://localhost:8080/realtime"), "http://localhost:8080/v1/relay/catalog");
  assert.equal(resolveRelayCatalogURL("https://example.test/realtime"), "https://example.test/v1/relay/catalog");
  assert.equal(resolveRelayCatalogURL("wss://user:pass@example.test/realtime"), null);
  assert.equal(resolveRelayCatalogURL("wss://example.test/realtime?secret=abc"), null);
  assert.equal(resolveRelayCatalogURL("wss://example.test/realtime#join"), null);
  assert.equal(resolveRelayCatalogURL("ftp://example.test/realtime"), null);
  assert.equal(resolveRelayCatalogURL("not a url"), null);
});

test("catalog wire shape stays in sync with the Go relay package", () => {
  const go = readFileSync(new URL("../../server/internal/relay/catalog.go", import.meta.url), "utf8");
  for (const tag of [
    '`json:"projects"`', '`json:"workspaces"`', '`json:"tabs"`',
    '`json:"name"`', '`json:"path"`', '`json:"branch,omitempty"`',
    '`json:"project,omitempty"`', '`json:"id"`', '`json:"kind"`', '`json:"title,omitempty"`',
  ]) {
    assert.ok(go.includes(tag), `Go relay catalog is missing ${tag}`);
  }
  for (const kind of ["terminal", "browser", "settings"]) {
    assert.ok(go.includes(`"${kind}"`), `Go relay catalog is missing tab kind ${kind}`);
  }
});

test("parser accepts host CLI shapes and rejects unknown tab kinds", () => {
  assert.equal(isRelayTabKind("terminal"), true);
  assert.equal(isRelayTabKind("browser"), true);
  assert.equal(isRelayTabKind("settings"), true);
  assert.equal(isRelayTabKind("editor"), false);
  assert.equal(isRelayTabKind(null), false);

  const catalog = parseRelayCatalog({
    projects: [{ name: "dragent", path: "/Users/me/dragent" }],
    workspaces: [{ name: "relay", path: "/Users/me/dragent-wt", branch: "feat/relay-catalog", project: "/Users/me/dragent" }],
    tabs: [
      { id: 7, kind: "terminal", title: "zsh", active: true, visible: true },
      { id: 9, kind: "browser" },
    ],
  });
  assert.ok(catalog);
  assert.equal(catalog.projects.length, 1);
  assert.equal(catalog.workspaces[0]?.branch, "feat/relay-catalog");
  assert.equal(catalog.tabs[1]?.title, "");
  assert.equal(catalog.tabs[1]?.active, false);

  // Branchless local checkouts and session folders omit the optional fields.
  const bare = parseRelayCatalog({
    projects: [],
    workspaces: [{ name: "local", path: "/tmp/work" }],
    tabs: [],
  });
  assert.equal(bare?.workspaces[0]?.branch, null);
  assert.equal(bare?.workspaces[0]?.project, null);

  for (const raw of [
    null, [], "catalog",
    { projects: [], workspaces: [], tabs: [{ id: 1, kind: "editor" }] },
    { projects: [{ name: "", path: "/x" }], workspaces: [], tabs: [] },
    { projects: [{ name: "x" }], workspaces: [], tabs: [] },
    { projects: [], workspaces: [{ name: "w", path: "/w", branch: 42 }], tabs: [] },
    { projects: [], workspaces: [], tabs: [{ id: 1.5, kind: "terminal" }] },
    { projects: [], workspaces: "none", tabs: [] },
  ]) {
    assert.equal(parseRelayCatalog(raw), null, `accepted invalid payload: ${JSON.stringify(raw)}`);
  }
});

type FakeResponse = { status: number; payload?: unknown; broken?: boolean };

function fakeFetch(responses: FakeResponse[], seen: { url: string; headers: Record<string, string> }[]) {
  return async (url: string, init?: { headers?: Record<string, string> }) => {
    seen.push({ url, headers: { ...init?.headers } });
    const next = responses.shift() ?? { status: 500 };
    return {
      status: next.status,
      json: async () => {
        if (next.broken) throw new Error("bad json");
        return next.payload;
      },
    };
  };
}

const catalogPayload = {
  projects: [{ name: "dragent", path: "/Users/me/dragent" }],
  workspaces: [{ name: "relay", path: "/Users/me/dragent-wt", branch: "feat/relay-catalog", project: "/Users/me/dragent" }],
  tabs: [{ id: 7, kind: "terminal", title: "zsh", active: true, visible: true }],
};

function dragentTab(extra: Record<string, unknown> = {}) {
  return {
    id: 3, kind: "agent", title: "Dragent", active: true, visible: true,
    agent_kind: "dragent", agent_state: "working",
    agent_work: "Login authentication",
    agent_step: "Dispatched: edit",
    agent_activity: [
      { sequence: 1, kind: "thinking", text: "Thinking…" },
      { sequence: 2, kind: "tool", text: "Dispatched: edit" },
    ],
    ...extra,
  };
}

function catalogWithTabs(tabs: unknown[]) {
  const catalog = parseRelayCatalog({ projects: [], workspaces: [], tabs });
  assert.ok(catalog, "catalog parses");
  return catalog;
}

test("dragent live context parses with wire-compatible extension fields", () => {
  const catalog = catalogWithTabs([dragentTab()]);
  const tab = catalog.tabs[0]!;
  assert.equal(tab.agent_work, "Login authentication");
  assert.equal(tab.agent_step, "Dispatched: edit");
  assert.deepEqual(
    tab.agent_activity?.map((entry) => [entry.sequence, entry.kind, entry.text]),
    [[1, "thinking", "Thinking…"], [2, "tool", "Dispatched: edit"]],
  );
  assert.equal(isDragentTab(tab), true);
  assert.equal(dragentStatusLine(tab), "Dragent · Login authentication · Dispatched: edit");
  assert.equal(dragentActivity(tab).length, 2);

  // camelCase aliases read the same.
  const aliased = catalogWithTabs([{
    id: 4, kind: "agent", agent_kind: "dragent", agent_state: "working",
    agentWork: "Refactor", agentStep: "Thinking…",
    agentActivity: [{ sequence: 7, kind: "narration", text: "Coordinating…" }],
  }]).tabs[0]!;
  assert.equal(aliased.agent_work, "Refactor");
  assert.equal(aliased.agent_step, "Thinking…");
  assert.equal(aliased.agent_activity?.length, 1);
  assert.equal(dragentStatusLine(aliased), "Dragent · Refactor · Thinking…");
});

test("missing dragent context reads as the old behavior, never a dropped tab", () => {
  // Older desktops send no context: nulls, bare status line, empty feed.
  const legacy = catalogWithTabs([{
    id: 5, kind: "agent", title: "Agent", active: false, visible: true,
    agent_kind: "dragent", agent_state: "working",
  }]).tabs[0]!;
  assert.equal(legacy.agent_work, null);
  assert.equal(legacy.agent_step, null);
  assert.equal(legacy.agent_activity, null);
  assert.equal(isDragentTab(legacy), true);
  assert.equal(dragentStatusLine(legacy), "Dragent");
  assert.deepEqual(dragentActivity(legacy), []);

  // Non-Dragent tabs never carry context, even when the host sends it.
  const claude = catalogWithTabs([dragentTab({ agent_kind: "claude" })]).tabs[0]!;
  assert.equal(isDragentTab(claude), false);
  assert.equal(dragentStatusLine(claude), null);
  assert.deepEqual(dragentActivity(claude), []);
  const terminal = catalogWithTabs([{
    id: 6, kind: "terminal", agent_work: "Work", agent_step: "Step",
    agent_activity: [{ sequence: 1, kind: "tool", text: "x" }],
  }]).tabs[0]!;
  assert.equal(terminal.agent_work, null);
  assert.equal(terminal.agent_activity, null);
});

test("malformed dragent context degrades to nothing instead of dropping the catalog", () => {
  // Wrong-typed fields read as null; invalid entries are skipped.
  const catalog = catalogWithTabs([dragentTab({
    agent_work: 42,
    agent_step: "",
    agent_activity: [
      { sequence: 1, kind: "tool", text: "kept" },
      { sequence: -1, kind: "tool", text: "bad sequence" },
      { sequence: 2, kind: "", text: "bad kind" },
      { sequence: 3, kind: "tool", text: "   " },
      "not a record",
      { sequence: 4, kind: "tool", text: "also kept" },
    ],
  })]);
  const tab = catalog.tabs[0]!;
  assert.equal(tab.agent_work, null);
  assert.equal(tab.agent_step, null);
  assert.deepEqual(tab.agent_activity?.map((entry) => entry.text), ["kept", "also kept"]);

  // Non-list activity reads as absent, not as an unreadable catalog.
  const absent = catalogWithTabs([dragentTab({ agent_activity: "soon" })]).tabs[0]!;
  assert.equal(absent.agent_activity, null);

  // Entries sort oldest-first by sequence and keep the most recent on skew.
  const many = Array.from({ length: 40 }, (_, i) => ({ sequence: 100 - i, kind: "tool", text: `step ${100 - i}` }));
  const trimmed = catalogWithTabs([dragentTab({ agent_activity: many })]).tabs[0]!;
  assert.ok((trimmed.agent_activity?.length ?? 0) <= 32);
  const sequences = trimmed.agent_activity!.map((entry) => entry.sequence);
  assert.deepEqual(sequences, [...sequences].sort((a, b) => a - b));
  assert.equal(sequences.at(-1), 100);
});

test("fetch sends the device query and a fresh account token per call", async () => {
  const seen: { url: string; headers: Record<string, string> }[] = [];
  let calls = 0;
  const catalog = await fetchRelayCatalog({
    gateway: "wss://example.test/realtime",
    deviceId: "phone 1",
    resolveAccessToken: async () => `fresh-token-${++calls}`,
    fetchFn: fakeFetch([{ status: 200, payload: catalogPayload }], seen),
  });
  assert.equal(catalog.projects[0]?.name, "dragent");
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.url, "https://example.test/v1/relay/catalog?device_id=phone%201");
  assert.equal(seen[0]?.headers.Authorization, "Bearer fresh-token-1");

  // A second call resolves a new token rather than reusing the first.
  const seen2: { url: string; headers: Record<string, string> }[] = [];
  await fetchRelayCatalog({
    gateway: "wss://example.test/realtime",
    deviceId: "phone 1",
    resolveAccessToken: async () => `fresh-token-${++calls}`,
    fetchFn: fakeFetch([{ status: 200, payload: catalogPayload }], seen2),
  });
  assert.equal(seen2[0]?.headers.Authorization, "Bearer fresh-token-2");
});

test("fetch omits Authorization without a token and maps failures without leaking secrets", async () => {
  const seen: { url: string; headers: Record<string, string> }[] = [];
  await assert.rejects(
    fetchRelayCatalog({
      gateway: "wss://example.test/realtime", deviceId: "device-1",
      fetchFn: fakeFetch([{ status: 401 }], seen),
    }),
    (error: unknown) => {
      assert.ok(error instanceof RelayCatalogError);
      assert.equal(error.status, 401);
      assert.equal(error.loginRequired, true);
      return true;
    },
  );
  assert.deepEqual(seen[0]?.headers, {});

  const unavailable = fetchRelayCatalog({
    gateway: "wss://example.test/realtime", deviceId: "device-1", accessToken: "private-token",
    fetchFn: fakeFetch([{ status: 404 }], []),
  });
  await assert.rejects(unavailable, /has not shared its catalog/);

  const failing = fetchRelayCatalog({
    gateway: "wss://example.test/realtime", deviceId: "device-1", accessToken: "private-token",
    fetchFn: async () => { throw new Error("fetch failed for https://example.test with private-token"); },
  });
  await assert.rejects(failing, (error: unknown) => {
    assert.ok(error instanceof RelayCatalogError);
    assert.doesNotMatch((error as Error).message, /private-token|example\.test/);
    return true;
  });

  const unreadable = fetchRelayCatalog({
    gateway: "wss://example.test/realtime", deviceId: "device-1",
    fetchFn: fakeFetch([{ status: 200, broken: true }], []),
  });
  await assert.rejects(unreadable, /unreadable/);

  const wrongShape = fetchRelayCatalog({
    gateway: "wss://example.test/realtime", deviceId: "device-1",
    fetchFn: fakeFetch([{ status: 200, payload: { projects: [] } }], []),
  });
  await assert.rejects(wrongShape, /unreadable/);

  await assert.rejects(
    fetchRelayCatalog({ gateway: "wss://user:secret@example.test/realtime", deviceId: "device-1", fetchFn: fakeFetch([], []) }),
    /address is invalid/,
  );
  await assert.rejects(
    fetchRelayCatalog({ gateway: "wss://example.test/realtime", deviceId: "  ", fetchFn: fakeFetch([], []) }),
    /address is invalid/,
  );
});
