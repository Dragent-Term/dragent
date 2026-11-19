import assert from "node:assert/strict";
import { test } from "node:test";
import {
  RelayRpcClient, RelayRpcError,
  RELAY_RPC_DEFAULT_TIMEOUT_MS, RELAY_RPC_TIMEOUT_MESSAGE,
} from "../src/transport/relayRpcClient";
import type { RealtimeClock } from "../src/transport/clock";
import { isValidRequestId, parseServerFrame } from "../src/transport/frames";

class FakeClock implements RealtimeClock {
  private time = 0;
  private sequence = 0;
  readonly timers = new Map<number, { at: number; callback: () => void }>();
  now = () => this.time;
  setTimeout = (callback: () => void, delay: number) => {
    const id = ++this.sequence;
    this.timers.set(id, { at: this.time + delay, callback });
    return id;
  };
  clearTimeout = (handle: unknown) => { this.timers.delete(handle as number); };
  advance(ms: number) {
    const end = this.time + ms;
    for (;;) {
      const next = [...this.timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
      if (!next || next[1].at > end) break;
      this.time = Math.max(this.time, next[1].at);
      this.timers.delete(next[0]);
      next[1].callback();
    }
    this.time = end;
  }
}

function harness() {
  const clock = new FakeClock();
  const sent: Record<string, unknown>[] = [];
  const drops: string[] = [];
  let counter = 0;
  const createRequestId = () => {
    counter += 1;
    return `00000000-0000-4000-8000-${String(counter).padStart(12, "0")}`;
  };
  const client = new RelayRpcClient({
    send: (frame) => {
      sent.push(frame);
      return true;
    },
    createRequestId,
    clock,
    log: (message) => { drops.push(message); },
  });
  client.setConnectionEpoch(1);
  const inbound = (frame: unknown) => client.handleMessage(JSON.stringify(frame));
  const inboundRaw = (raw: string) => client.handleMessage(raw);
  return { client, clock, sent, drops, inbound, inboundRaw };
}

test("rpc envelope readers accept valid reply/stream and reject bad shapes", () => {
  const replyId = "00000000-0000-4000-8000-000000000001";
  const okReply = parseServerFrame(JSON.stringify({ type: "rpc-reply", v: 1, id: replyId, ok: true, result: { tabs: [] } }));
  assert.ok(okReply?.type === "rpc-reply" && okReply.ok === true);
  const errReply = parseServerFrame(JSON.stringify({
    type: "rpc-reply", v: 1, id: replyId, ok: false, error: { message: "Sign in to view this machine.", loginRequired: true },
  }));
  assert.ok(errReply?.type === "rpc-reply" && errReply.ok === false && errReply.error.loginRequired === true);
  const stream = parseServerFrame(JSON.stringify({ type: "rpc-stream", v: 1, stream: "tabs", seq: 7, payload: { tabs: [] } }));
  assert.ok(stream?.type === "rpc-stream" && stream.seq === 7);

  for (const raw of [
    { type: "rpc-reply", v: 2, id: replyId, ok: true, result: {} },
    { type: "rpc-reply", v: 1, id: "bad-id", ok: true, result: {} },
    { type: "rpc-reply", v: 1, id: "00000000-0000-0000-0000-000000000000", ok: true, result: {} },
    { type: "rpc-reply", v: 1, id: replyId, ok: true },
    { type: "rpc-reply", v: 1, id: replyId, ok: false, error: { message: "", loginRequired: false } },
    { type: "rpc-reply", v: 1, id: replyId, ok: false, error: { message: "nope" } },
    { type: "rpc-reply", v: 1, id: replyId, ok: "yes", result: {} },
    { type: "rpc-stream", v: 1, stream: "", seq: 0, payload: {} },
    { type: "rpc-stream", v: 1, stream: "tabs", seq: -1, payload: {} },
    { type: "rpc-stream", v: 1, stream: "tabs", seq: 1.5, payload: {} },
    { type: "rpc-stream", v: 1, stream: "tabs", seq: 0 },
    { type: "rpc-stream", v: 2, stream: "tabs", seq: 0, payload: {} },
  ]) {
    assert.equal(parseServerFrame(JSON.stringify(raw)), null, `accepted invalid rpc: ${JSON.stringify(raw)}`);
  }
});

test("correlated request resolves by id with unique ids and user-safe errors", async () => {
  const h = harness();
  assert.equal(RELAY_RPC_DEFAULT_TIMEOUT_MS, 15_000);
  const first = h.client.request("tabs.list", {});
  const second = h.client.request("worktree.list", {});
  assert.equal(h.sent.length, 2);
  const firstId = h.sent[0]?.id;
  const secondId = h.sent[1]?.id;
  assert.ok(isValidRequestId(firstId) && isValidRequestId(secondId));
  assert.notEqual(firstId, secondId);
  assert.deepEqual(h.sent[0], { type: "rpc-request", v: 1, id: firstId, method: "tabs.list", params: {} });
  assert.equal(h.client.pendingCount(), 2);

  h.inbound({ type: "rpc-reply", v: 1, id: secondId, ok: false, error: { message: "Sign in to view this machine.", loginRequired: true } });
  await assert.rejects(second, (error: unknown) => {
    assert.ok(error instanceof RelayRpcError);
    assert.equal(error.message, "Sign in to view this machine.");
    assert.equal(error.loginRequired, true);
    return true;
  });
  assert.equal(h.client.pendingCount(), 1);

  h.inbound({ type: "rpc-reply", v: 1, id: firstId, ok: true, result: { tabs: [] } });
  assert.deepEqual(await first, { tabs: [] });
  assert.equal(h.client.pendingCount(), 0);
  h.client.close();
});

test("request times out after 15s without retrying", async () => {
  const h = harness();
  const pending = h.client.request("tabs.list", {});
  const failure = assert.rejects(pending, (error: unknown) => {
    assert.ok(error instanceof RelayRpcError);
    assert.match(error.message, /request timed out/);
    return true;
  });
  assert.equal(h.sent.length, 1);
  h.clock.advance(RELAY_RPC_DEFAULT_TIMEOUT_MS - 1);
  assert.equal(h.client.pendingCount(), 1);
  h.clock.advance(1);
  await failure;
  assert.equal(h.client.pendingCount(), 0);
  assert.equal(h.sent.length, 1);
  assert.equal(h.drops.length, 0);
  // A late reply for the timed-out id is dropped, never delivered.
  h.inbound({ type: "rpc-reply", v: 1, id: h.sent[0]?.id, ok: true, result: {} });
  assert.ok(h.drops.length >= 1);
  h.client.close();
});

test("replies for unknown methods are dropped and the pending request survives", async () => {
  const h = harness();
  const pending = h.client.request("tabs.list", {});
  const unknownId = "00000000-0000-4000-8000-000000009999";
  assert.equal(h.client.handleMessage(JSON.stringify({
    type: "rpc-reply", v: 1, id: unknownId, ok: true, result: {},
  })), true);
  assert.ok(h.drops.length >= 1);
  assert.equal(h.client.pendingCount(), 1);
  // Unknown rpc type is also dropped, not routed to terminal handlers.
  assert.equal(h.client.handleMessage(JSON.stringify({ type: "rpc-unknown", v: 1 })), true);
  assert.equal(h.client.handleMessage(JSON.stringify({ type: "delta", delta: { kind: "alt_screen_ended" } })), false);
  h.inbound({ type: "rpc-reply", v: 1, id: h.sent[0]?.id, ok: true, result: { tabs: [] } });
  assert.deepEqual(await pending, { tabs: [] });
  h.client.close();
});

test("malformed payloads are dropped without touching pending or streams", async () => {
  const h = harness();
  const received: unknown[] = [];
  h.client.subscribe("tabs", {}, (payload) => { received.push(payload); });
  const pending = h.client.request("tabs.list", {});
  const sentCount = h.sent.length;
  const malformed: unknown[] = [
    "{broken",
    JSON.stringify({ type: "rpc-reply", v: 1, id: h.sent[1]?.id, ok: true }),
    JSON.stringify({ type: "rpc-reply", v: 1, id: "not-a-uuid", ok: true, result: {} }),
    JSON.stringify({ type: "rpc-reply", v: 2, id: h.sent[1]?.id, ok: true, result: {} }),
    JSON.stringify({ type: "rpc-stream", v: 1, stream: "tabs", seq: -1, payload: {} }),
    JSON.stringify({ type: "rpc-stream", v: 1, stream: "", seq: 0, payload: {} }),
    JSON.stringify({ type: "rpc-stream", v: 1, stream: "tabs", payload: {} }),
    JSON.stringify({ type: "rpc-stream", v: 1, stream: "tabs", seq: 0 }),
  ];
  for (const raw of malformed) {
    if (raw === "{broken") assert.equal(h.client.handleMessage(raw), false);
    else assert.equal(h.client.handleMessage(raw), true);
  }
  assert.ok(h.drops.length >= malformed.length - 1);
  assert.equal(h.client.pendingCount(), 1);
  assert.deepEqual(received, []);
  assert.equal(h.sent.length, sentCount);
  // The connection still works after the junk.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 0, payload: { tabs: [] } });
  assert.deepEqual(received, [{ tabs: [] }]);
  h.inbound({ type: "rpc-reply", v: 1, id: h.sent[1]?.id, ok: true, result: { tabs: [] } });
  assert.deepEqual(await pending, { tabs: [] });
  h.client.close();
});

test("stream gap triggers resubscription without rendering a partial sequence", () => {
  const h = harness();
  const events: { payload: unknown; snapshot: boolean; seq: number }[] = [];
  h.client.subscribe("tabs", {}, (payload, info) => { events.push({ payload, snapshot: info.snapshot, seq: info.seq }); });
  assert.deepEqual(h.sent[0], { type: "rpc-subscribe", v: 1, id: h.sent[0]?.id, stream: "tabs", params: {} });

  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 7, payload: { tabs: ["a"] } });
  assert.equal(events.length, 1);
  assert.equal(events[0]?.snapshot, true);

  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 8, payload: { tabs: ["b"] } });
  assert.equal(events.length, 2);
  assert.equal(events[1]?.snapshot, false);

  // Duplicate and stale frames are dropped.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 8, payload: { tabs: ["dup"] } });
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 3, payload: { tabs: ["stale"] } });
  assert.equal(events.length, 2);

  // Gap (missing 9): the seq-10 payload is never rendered; client resubscribes.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 10, payload: { tabs: ["gap"] } });
  assert.equal(events.length, 2);
  assert.equal(h.sent.length, 2);
  assert.deepEqual(h.sent[1], { type: "rpc-subscribe", v: 1, id: h.sent[1]?.id, stream: "tabs", params: {} });
  assert.notEqual(h.sent[0]?.id, h.sent[1]?.id);
  assert.ok(h.drops.join("\n").match(/gap/));

  // Fresh snapshot after resubscription resumes the stream.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 0, payload: { tabs: ["fresh"] } });
  assert.equal(events.length, 3);
  assert.equal(events[2]?.snapshot, true);
  assert.deepEqual(events[2]?.payload, { tabs: ["fresh"] });

  // Unknown streams are dropped.
  assert.equal(h.inbound({ type: "rpc-stream", v: 1, stream: "nope", seq: 0, payload: {} }), true);
  assert.equal(events.length, 3);
  h.client.close();
});

test("unsubscribe before the first snapshot holds, delivers, then confirms", async () => {
  const h = harness();
  const received: unknown[] = [];
  const sub = h.client.subscribe("tabs", {}, (payload) => { received.push(payload); });
  assert.equal(h.sent.length, 1);
  const held = sub.unsubscribe();
  // Held: no unsubscribe frame yet.
  assert.equal(h.sent.length, 1);
  assert.deepEqual(received, []);

  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 4, payload: { tabs: ["snap"] } });
  await held;
  // Snapshot was still delivered, then the unsubscribe was confirmed.
  assert.deepEqual(received, [{ tabs: ["snap"] }]);
  assert.equal(h.sent.length, 2);
  assert.deepEqual(h.sent[1], { type: "rpc-unsubscribe", v: 1, id: h.sent[1]?.id, stream: "tabs" });
  assert.equal(sub.active, false);

  // Later events for the closed stream are dropped.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 5, payload: {} });
  assert.deepEqual(received, [{ tabs: ["snap"] }]);
  h.client.close();
});

test("connection-epoch change rejects requests once and resubscribes streams", async () => {
  const h = harness();
  const received: { payload: unknown; snapshot: boolean }[] = [];
  h.client.subscribe("tabs", { filter: "a" }, (payload, info) => { received.push({ payload, snapshot: info.snapshot }); });
  const pending = h.client.request("tabs.list", {});
  const rejected = assert.rejects(pending, (error: unknown) => {
    assert.ok(error instanceof RelayRpcError);
    assert.match(error.message, /Connection lost/);
    return true;
  });
  const subscribesBefore = h.sent.filter((frame) => frame.type === "rpc-subscribe").length;
  const requestsBefore = h.sent.filter((frame) => frame.type === "rpc-request").length;

  h.client.setConnectionEpoch(2);
  await rejected;
  assert.equal(h.client.pendingCount(), 0);
  // Streams resubscribe with a fresh id; requests are never replayed.
  assert.equal(h.sent.filter((frame) => frame.type === "rpc-subscribe").length, subscribesBefore + 1);
  assert.equal(h.sent.filter((frame) => frame.type === "rpc-request").length, requestsBefore);
  assert.deepEqual(h.sent.at(-1), {
    type: "rpc-subscribe", v: 1, id: h.sent.at(-1)?.id, stream: "tabs", params: { filter: "a" },
  });

  // Pre-replay history is never delivered: the old seq is gone, so the next
  // message is a fresh snapshot even with an old-looking seq.
  h.inbound({ type: "rpc-stream", v: 1, stream: "tabs", seq: 3, payload: { tabs: ["new-epoch"] } });
  assert.equal(received.length, 1);
  assert.equal(received[0]?.snapshot, true);

  // Same-epoch calls are no-ops.
  const sentCount = h.sent.length;
  h.client.setConnectionEpoch(2);
  assert.equal(h.sent.length, sentCount);
  h.client.close();
});

test("error messages stay user-safe and never carry tokens or urls", async () => {
  const h = harness();
  const secret = "private-account-token";
  const pending = h.client.request("tabs.list", {});
  h.clock.advance(RELAY_RPC_DEFAULT_TIMEOUT_MS);
  await assert.rejects(pending, (error: unknown) => {
    assert.ok(error instanceof RelayRpcError);
    assert.equal(error.message, RELAY_RPC_TIMEOUT_MESSAGE);
    assert.doesNotMatch(error.message, /private-account-token/);
    assert.doesNotMatch(JSON.stringify(error), /private-account-token/);
    return true;
  });
  // Drop logs never include secrets or payloads, only kinds and handles.
  h.inbound({ type: "rpc-reply", v: 1, id: "00000000-0000-4000-8000-000000009999", ok: true, result: { token: secret } });
  assert.doesNotMatch(h.drops.join("\n"), /private-account-token/);
  assert.doesNotMatch(JSON.stringify(h.client), /private-account-token/);
  h.client.close();
});
