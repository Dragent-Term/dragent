import assert from "node:assert/strict";
import { test } from "node:test";
import {
  NEW_TAB_APPEAR_TIMEOUT_MS,
  NEW_TAB_MENU_ITEMS,
  newTabWaitStatus,
  newTabWorkspaceId,
  shouldOpenNewTabMenuOnArrival,
} from "../src/machines/newTab";
import {
  createWorkspaceTab,
  isRelayRpcMethod,
  newTabParams,
  parseRelayRpcNewTabResult,
  RELAY_NEW_TAB_TIMEOUT_MS,
} from "../src/transport/relayRpcMethods";
import { RelayRpcError, type RelayRpcClient } from "../src/transport/relayRpcClient";

test("menu offers exactly the desktop's three items, word for word", () => {
  assert.deepEqual(
    NEW_TAB_MENU_ITEMS.map((item) => [item.kind, item.title, item.detail]),
    [
      ["terminal", "Terminal", "A shell in this workspace"],
      ["browser", "Browser", "Preview your app or open any site"],
      ["dragent_cli", "Dragent CLI", "Script workspaces, agents and automations"],
    ],
  );
});

test("params accept only published wire ids and the three kinds", () => {
  assert.deepEqual(newTabParams("w0w3", "terminal"), { workspace_id: "w0w3", kind: "terminal" });
  assert.deepEqual(newTabParams("w12w7", "dragent_cli"), { workspace_id: "w12w7", kind: "dragent_cli" });
  for (const [workspace, kind] of [
    ["w0w3", "agent_coordination"],
    ["w0w3", "Terminal"],
    ["w0w3", "settings"],
    ["w0w3", undefined],
    ["", "terminal"],
    ["/Users/me/project", "terminal"],
    ["../w0w3", "terminal"],
    ["w".repeat(65), "terminal"],
    [3, "terminal"],
    [null, "browser"],
  ] as const) {
    assert.equal(newTabParams(workspace, kind), null, `${String(workspace)} ${String(kind)}`);
  }
});

test("only workspaces with a wire id can take a new tab", () => {
  assert.equal(newTabWorkspaceId({ id: "w0w3" }), "w0w3");
  assert.equal(newTabWorkspaceId({ id: null }), null);
  assert.equal(newTabWorkspaceId({ id: "/repo/worktree" }), null);
});

test("result is exactly a safe tab id", () => {
  assert.deepEqual(parseRelayRpcNewTabResult({ id: 4294967297 }), { id: 4294967297 });
  assert.deepEqual(parseRelayRpcNewTabResult({ id: 0 }), { id: 0 });
  for (const raw of [null, {}, { id: -1 }, { id: 1.5 }, { id: "3" }, { id: 2 ** 53 }, { id: 1, title: "x" }]) {
    assert.equal(parseRelayRpcNewTabResult(raw), null, JSON.stringify(raw));
  }
});

test("tabs.create is a known method and sends only workspace and kind", async () => {
  assert.equal(isRelayRpcMethod("tabs.create"), true);
  const sent: { method: string; params: unknown; timeoutMs: number | undefined }[] = [];
  const client = {
    request: async (method: string, params: unknown, opts?: { timeoutMs?: number }) => {
      sent.push({ method, params, timeoutMs: opts?.timeoutMs });
      return { id: 9 };
    },
  } as unknown as RelayRpcClient;
  assert.deepEqual(await createWorkspaceTab(client, "w0w3", "browser"), { id: 9 });
  assert.deepEqual(sent, [{ method: "tabs.create", params: { workspace_id: "w0w3", kind: "browser" }, timeoutMs: RELAY_NEW_TAB_TIMEOUT_MS }]);
  await assert.rejects(createWorkspaceTab(client, "/path", "browser"), RelayRpcError);
  assert.equal(sent.length, 1);
});

test("a misshapen desktop answer is a user-safe error", async () => {
  const client = { request: async () => ({ id: "nope" }) } as unknown as RelayRpcClient;
  await assert.rejects(createWorkspaceTab(client, "w0w3", "terminal"), RelayRpcError);
});

test("waiting for the new tab: ready when the catalog has it, timeout after the deadline", () => {
  const started = 1_000;
  assert.equal(newTabWaitStatus([1, 2], 7, started, started), "waiting");
  assert.equal(newTabWaitStatus([1, 2], 7, started, started + NEW_TAB_APPEAR_TIMEOUT_MS - 1), "waiting");
  assert.equal(newTabWaitStatus([1, 2], 7, started, started + NEW_TAB_APPEAR_TIMEOUT_MS), "timeout");
  assert.equal(newTabWaitStatus([1, 7], 7, started, started + 50), "ready");
  // Arriving late still opens it rather than reporting a timeout.
  assert.equal(newTabWaitStatus([7], 7, started, started + NEW_TAB_APPEAR_TIMEOUT_MS * 2), "ready");
  assert.equal(NEW_TAB_APPEAR_TIMEOUT_MS, 10_000);
});

test("newTab=1 opens the menu on arrival, once", () => {
  assert.equal(shouldOpenNewTabMenuOnArrival("1", false), true);
  assert.equal(shouldOpenNewTabMenuOnArrival("1", true), false);
  assert.equal(shouldOpenNewTabMenuOnArrival(undefined, false), false);
  assert.equal(shouldOpenNewTabMenuOnArrival("0", false), false);
  assert.equal(shouldOpenNewTabMenuOnArrival(["1", "1"], false), false);
});
