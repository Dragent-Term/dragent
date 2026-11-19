import assert from "node:assert/strict";
import { test } from "node:test";
import { RELAY_RPC_LIMITS, RelayRpcClient, type RelayRpcError } from "../src/transport/relayRpcClient";
import {
  RELAY_RPC_UNREADABLE_MESSAGE,
  listTabs,
  subscribeCompanionTerminal,
  type RelayRpcTerminalStreamPayload,
} from "../src/transport/relayRpcMethods";

const H = "11111111-1111-4111-8111-111111111111";
const b64 = (text: string): string => Buffer.from(text, "utf8").toString("base64");

function harness() {
  const sent: Record<string, unknown>[] = [];
  let ids = 0;
  const client = new RelayRpcClient({
    send: (frame) => { sent.push(frame); return true; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
    snapshotSequence: 1,
    log: () => {},
  });
  const frame = (seq: number, payload: unknown) =>
    client.handleMessage(JSON.stringify({ type: "rpc-stream", v: 1, stream: `terminal.${H}`, seq, payload }));
  return { client, sent, frame };
}

function listen() {
  const frames: RelayRpcTerminalStreamPayload[] = [];
  const errors: RelayRpcError[] = [];
  return {
    frames,
    errors,
    listener: { onFrame: (p: RelayRpcTerminalStreamPayload) => frames.push(p), onError: (e: RelayRpcError) => errors.push(e) },
  };
}

test("subscribe names the stream after the handle and sends the offset only when given", () => {
  const h = harness();
  const a = listen();
  const sub = subscribeCompanionTerminal(h.client, H, 42, a.listener);
  assert.deepEqual(h.sent[0], {
    type: "rpc-subscribe", v: 1, id: h.sent[0]!.id, stream: `terminal.${H}`, params: { handle: H, offset: 42 },
  });
  sub.close();
  assert.equal(h.sent.length, 1, "no frame yet: the unsubscribe waits for the first frame or a timeout");
  const b = listen();
  const h2 = harness();
  subscribeCompanionTerminal(h2.client, H, undefined, b.listener);
  assert.deepEqual(h2.sent[0]!.params, { handle: H });
});

test("frames are parsed and delivered in order; an unreadable frame ends the stream", () => {
  const h = harness();
  const a = listen();
  subscribeCompanionTerminal(h.client, H, undefined, a.listener);
  h.frame(1, { base: 7, total: 7, data: b64("SEED"), alive: true, reset: true, rows: 24, cols: 80 });
  h.frame(2, { base: 7, total: 9, data: b64("ok"), alive: true, reset: false });
  assert.deepEqual(a.frames.map((f) => [f.reset, f.base, f.decoded]), [[true, 7, 4], [false, 7, 2]]);
  h.frame(3, { base: 9, total: 9, data: "not base64!", alive: true, reset: false });
  assert.equal(a.errors.length, 1);
  assert.equal(a.errors[0]!.message, RELAY_RPC_UNREADABLE_MESSAGE);
  assert.equal(h.sent.at(-1)!.type, "rpc-unsubscribe");
  h.frame(4, { base: 9, total: 10, data: b64("x"), alive: true, reset: false });
  assert.equal(a.frames.length, 2, "nothing after the failure");
});

test("a server error on the subscribe id reaches onError once; nothing after close", () => {
  const h = harness();
  const a = listen();
  subscribeCompanionTerminal(h.client, H, 0, a.listener);
  const id = h.sent[0]!.id;
  h.client.handleMessage(JSON.stringify({ type: "rpc-reply", v: 1, id, ok: false, error: { message: "unknown stream", loginRequired: false } }));
  assert.equal(a.errors.length, 1);
  // The stream name is free again for a resubscribe.
  const b = listen();
  const sub = subscribeCompanionTerminal(h.client, H, 0, b.listener);
  h.frame(1, { base: 0, total: 0, data: "", alive: true, reset: false });
  assert.equal(b.frames.length, 1);
  sub.close();
  assert.equal(h.sent.at(-1)!.type, "rpc-unsubscribe");
  h.frame(2, { base: 0, total: 1, data: b64("x"), alive: true, reset: false });
  assert.equal(b.frames.length, 1);
});

test("after unsubscribe + resubscribe, late frames of the old stream are not taken for the new one", () => {
  const h = harness();
  const a = listen();
  const old = subscribeCompanionTerminal(h.client, H, 0, a.listener);
  h.frame(1, { base: 0, total: 1, data: b64("a"), alive: true, reset: false });
  old.close();
  const b = listen();
  subscribeCompanionTerminal(h.client, H, undefined, b.listener);
  h.frame(2, { base: 1, total: 2, data: b64("b"), alive: true, reset: false }); // late, old subscription
  h.frame(1, { base: 2, total: 2, data: b64("SEED"), alive: true, reset: true, rows: 24, cols: 80 });
  assert.deepEqual(b.frames.map((f) => f.reset), [true]);
});

test("the exit frame is delivered and closes the stream", () => {
  const h = harness();
  const a = listen();
  subscribeCompanionTerminal(h.client, H, 0, a.listener);
  h.frame(1, { base: 0, total: 0, data: "", alive: false, exit_code: 0, reset: false });
  assert.equal(a.frames[0]!.alive, false);
  assert.equal(a.frames[0]!.exitCode, 0);
  assert.equal(h.sent.at(-1)!.type, "rpc-unsubscribe");
  assert.deepEqual(h.client.streamNames(), []);
});

test("an invalid handle or offset fails asynchronously without touching the wire", async () => {
  const h = harness();
  const a = listen();
  subscribeCompanionTerminal(h.client, "not-a-handle", 0, a.listener);
  subscribeCompanionTerminal(h.client, H, -1, a.listener);
  assert.equal(a.errors.length, 0);
  await Promise.resolve();
  assert.equal(a.errors.length, 2);
  assert.equal(h.sent.length, 0);
});

test("request ids are a rolling window: a long session never runs out", async () => {
  const h = harness();
  const total = RELAY_RPC_LIMITS.idsPerEpoch * 2 + 5;
  for (let i = 0; i < total; i++) {
    const call = listTabs(h.client);
    const request = h.sent.at(-1)!;
    assert.equal(request.type, "rpc-request", `request ${i} went out`);
    h.client.handleMessage(JSON.stringify({ type: "rpc-reply", v: 1, id: request.id, ok: true, result: { tabs: [] } }));
    await call;
  }
  assert.equal(h.sent.length, total);
});

test("the id window never forgets ids still in use, and still refuses duplicates", async () => {
  const sent: Record<string, unknown>[] = [];
  let n = 0;
  let repeat: string | null = null;
  const client = new RelayRpcClient({
    send: (frame) => { sent.push(frame); return true; },
    createRequestId: () => repeat ?? `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}`,
    log: () => {},
  });
  // A request left pending holds the oldest id.
  const pending = client.request("tabs.list", {});
  const pendingId = sent[0]!.id as string;
  for (let i = 0; i < RELAY_RPC_LIMITS.idsPerEpoch + 10; i++) {
    const call = client.request("tabs.list", {});
    client.handleMessage(JSON.stringify({ type: "rpc-reply", v: 1, id: sent.at(-1)!.id, ok: true, result: {} }));
    await call;
  }
  // Reusing the pending id is still refused.
  repeat = pendingId;
  await assert.rejects(client.request("tabs.list", {}));
  repeat = null;
  client.handleMessage(JSON.stringify({ type: "rpc-reply", v: 1, id: pendingId, ok: true, result: { still: "routed" } }));
  assert.deepEqual(await pending, { still: "routed" });
});
