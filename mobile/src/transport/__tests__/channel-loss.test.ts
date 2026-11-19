// A dropped encrypted channel fails only what travelled through it: the
// catalog and machine list come from the relay and keep working. Failing
// everything blanked the machine screen ("catalog is unreachable") whenever
// the channel had a hiccup, e.g. on opening a tab.
import assert from "node:assert/strict";
import { test } from "node:test";

import { RelayRpcClient } from "../relayRpcClient";

const relayServed = (kind: "request" | "stream", name: string) =>
  kind === "request" ? name === "machines.list" : name === "catalog";

test("a channel loss fails channel traffic and keeps what the relay serves", async () => {
  const sent: Record<string, unknown>[] = [];
  const rpc = new RelayRpcClient({ send: (frame) => { sent.push(frame); return true; } });

  const machines = rpc.request("machines.list");
  const attach = rpc.request("tabs.attach", { tab_id: 7 });
  let catalogError: unknown = null;
  let terminalError: unknown = null;
  rpc.subscribe("catalog", {}, () => {}, (error) => { catalogError = error; });
  rpc.subscribe("terminal.1", {}, () => {}, (error) => { terminalError = error; });

  rpc.failChannelTraffic(relayServed);

  await assert.rejects(attach, "the channel's request fails");
  assert.equal(rpc.pendingCount(), 1, "the machine list request is still waiting");
  assert.deepEqual(rpc.streamNames().sort(), ["catalog", "terminal.1"], "subscriptions stay, to resume");
  assert.equal(catalogError, null, "the catalog stream isn't failed");
  assert.equal(terminalError, null);

  // The client is still connected: relay-served calls keep going out.
  const before = sent.length;
  void rpc.request("machines.list").catch(() => {});
  assert.equal(sent.length, before + 1);
  void machines.catch(() => {});
});
