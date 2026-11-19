// Several screens show one desktop's catalog over one socket: the home row
// and the machine's own screens. A socket allows one subscription per
// stream, so a second screen used to throw ("already subscribed") and show
// the catalog as unreachable.
import assert from "node:assert/strict";
import { test } from "node:test";

import { RelayRpcClient } from "../relayRpcClient";
import type { RelaySocketClient } from "../RelaySocketClient";
import { subscribeSharedCatalog } from "../sharedCatalog";

function fakeClient() {
  const sent: Record<string, unknown>[] = [];
  const rpc = new RelayRpcClient({ send: (frame) => { sent.push(frame); return true; } });
  const client = { rpc, openCatalog: () => ({ kind: "unreadable" }) } as unknown as RelaySocketClient;
  const subscribes = () => sent.filter((f) => f.type === "rpc-subscribe" && f.stream === "catalog").length;
  return { client, rpc, subscribes };
}

const listener = () => ({ onSnapshot: () => {}, onUpdate: () => {} });

test("two screens share one catalog subscription", () => {
  const { client, rpc, subscribes } = fakeClient();
  const home = subscribeSharedCatalog(client, listener());
  assert.doesNotThrow(() => subscribeSharedCatalog(client, listener()), "the machine screen joins");
  assert.equal(subscribes(), 1, "one subscription on the wire");
  assert.deepEqual(rpc.streamNames(), ["catalog"]);
  void home.unsubscribe();
  assert.deepEqual(rpc.streamNames(), ["catalog"], "still open for the other screen");
});

test("leaving and rejoining at once (a new connection epoch) reuses the subscription", () => {
  const { client, subscribes } = fakeClient();
  const first = subscribeSharedCatalog(client, listener());
  void first.unsubscribe();
  assert.doesNotThrow(() => subscribeSharedCatalog(client, listener()));
  assert.equal(subscribes(), 1, "no second subscribe while the first is still pending");
});

test("a retry starts a new subscription and keeps the screens already listening", () => {
  const { client, rpc, subscribes } = fakeClient();
  const seen: string[] = [];
  subscribeSharedCatalog(client, { onSnapshot: () => seen.push("home"), onUpdate: () => {} });
  // The first subscription never delivered: the machine screen retries.
  subscribeSharedCatalog(client, { onSnapshot: () => seen.push("machine"), onUpdate: () => {} }, { fresh: true });
  assert.equal(subscribes(), 2, "a second subscribe went out");
  assert.deepEqual(rpc.streamNames(), ["catalog"], "only the new one is open");
  assert.deepEqual(seen, [], "nobody was told the old one failed");
});
