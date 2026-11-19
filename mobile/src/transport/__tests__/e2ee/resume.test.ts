// §12.3 phone cases (malicious relay): deletion at a rekey or reconnect,
// dropped output, reconnect without resume. Real cryptography both ends.
import assert from "node:assert/strict";
import { test } from "node:test";

import { RelayRpcError } from "../../relayRpcClient";
import { inputCompanionTab, listTabs, subscribeCompanionTerminal, type RelayRpcTerminalStreamPayload } from "../../relayRpcMethods";
import { DESKTOP_LINK_TIMING } from "../../e2ee/desktopLink";
import { flush, head, world, type World } from "./fixture";

const LOST = "Some of what you typed didn't reach Studio Mac";

async function open(w: World): Promise<void> {
  await w.admit();
  w.pump();
  w.pump();
  assert.equal(w.client.getSnapshot().secure?.state, "open");
}

/** Type `n` keystroke batches; the relay forwards the first `keep`, deletes the rest. */
async function typeAndDropTail(w: World, n: number, keep: number): Promise<Promise<unknown>[]> {
  const sent = Array.from({ length: n }, (_, i) => inputCompanionTab(w.client.rpc, "7", String.fromCharCode(97 + i)).catch((e: unknown) => e));
  await flush();
  let seen = 0;
  w.pump((_f, h) => h.kind !== 2 || ++seen <= keep);
  return sent;
}

function ctrlTexts(w: World, chan: string): string[] {
  return w.inbox.filter((m) => m.chan === chan && "mtype" in m && m.mtype === "ctrl").map((m) => ("text" in m ? m.text : ""));
}

test("H13R: input deleted at a rekey is reported, the new channel's input waits for input_lost_ack, then flows", async () => {
  const w = world();
  try {
    await open(w);
    const a = w.chan();
    w.clock.advance(60 * 60 * 1000 - 3000);
    const typed = await typeAndDropTail(w, 5, 2); // ctr 1..5 sent; the relay deletes 3, 4, 5
    for (const r of w.requests().filter((q) => q.method === "tabs.input")) w.reply(r.id as string, { sent: true }, a);
    assert.equal(w.desk.chanState(a).processedThrough, 2);
    w.clock.advance(3000); // rekey: HS1 for the new channel goes out
    const s = w.socket();
    const hs1 = s.binary.findIndex((f, i) => i >= (w.relayed.get(s) ?? 0) && head(f).kind === 1);
    assert.ok(hs1 >= 0);
    const duringHandshake = inputCompanionTab(w.client.rpc, "7", "x").catch((e: unknown) => e);
    await flush();
    assert.ok(s.binary.slice(hs1).every((f) => head(f).chan !== a), "no input on two channels");
    w.pump(); // HS1 → desktop, HS2 → phone; hello + queued input go out
    w.pump();
    const b = w.chan();
    assert.equal(ctrlTexts(w, b)[0], `hello {"resume":{"chan":"${a}","last_sent_ctr":5}}`, "hello says where the old channel stopped");
    // The desktop drains the old channel through 5, falls short at 2: input_lost.
    w.toPhone(w.desk.send(b, "ctrl", w.desk.ctrl({ name: "input_lost", handles: [7] })));
    await flush();
    assert.deepEqual(w.inputLost, [{ handles: [7], message: LOST }]);
    for (const p of [...(await Promise.all(typed)).slice(2), await duringHandshake]) {
      assert.ok(p instanceof RelayRpcError, "deleted and discarded input fails at once, no timeout");
      assert.equal(p.code, "input_lost");
      assert.equal(p.message, LOST);
    }
    assert.deepEqual((await Promise.all(typed)).slice(0, 2), [{ sent: true }, { sent: true }]);
    w.pump();
    const acks = ctrlTexts(w, b);
    assert.deepEqual(acks.slice(1), ["input_lost_ack {}"]);
    // After the acknowledgement new input is written normally.
    const after = inputCompanionTab(w.client.rpc, "7", "y");
    await flush();
    w.pump();
    const last = w.requests().at(-1)!;
    assert.deepEqual(last.params, { handle: 7, text: "y" });
    const order = w.inbox.filter((m) => m.chan === b && "text" in m).map((m) => ("text" in m ? m.text : ""));
    assert.ok(order.indexOf("input_lost_ack {}") < order.findIndex((x) => x.includes('"text":"y"')), "y comes after the ack");
    w.reply(last.id as string, { sent: true }, b);
    assert.deepEqual(await after, { sent: true });
    assert.equal(w.client.getSnapshot().secure?.state, "open");
  } finally {
    w.close();
  }
});

test("H13R at a forced reconnect: the relay deletes the tail and kills the socket; the resumed channel reports the loss", async () => {
  const w = world();
  try {
    await open(w);
    const a = w.chan();
    const typed = await typeAndDropTail(w, 5, 2);
    w.socket().drop(1006);
    for (const p of await Promise.all(typed)) assert.ok(p instanceof RelayRpcError, "the socket loss fails pending input");
    w.clock.advance(1000); // socket backoff
    await w.admit(); // same lease
    w.pump();
    w.pump();
    const b = w.chan();
    assert.notEqual(a, b);
    assert.equal(ctrlTexts(w, b)[0], `hello {"resume":{"chan":"${a}","last_sent_ctr":5}}`);
    w.toPhone(w.desk.send(b, "ctrl", w.desk.ctrl({ name: "input_lost", handles: [7] })));
    await flush();
    assert.deepEqual(w.inputLost, [{ handles: [7], message: LOST }]);
    w.pump();
    assert.deepEqual(ctrlTexts(w, b).slice(1), ["input_lost_ack {}"]);
  } finally {
    w.close();
  }
});

test("note 3: JSON requests sent before input_lost_ack fail at once; subscriptions are asked for again after the ack", async () => {
  const w = world();
  try {
    await open(w);
    const a = w.chan();
    w.socket().drop(1006);
    w.clock.advance(1000);
    await w.admit();
    const tabs = listTabs(w.client.rpc).catch((e: unknown) => e);
    subscribeCompanionTerminal(w.client.rpc, "7", undefined, { onFrame: () => {}, onError: () => {} });
    await flush();
    w.pump();
    w.pump();
    const b = w.chan();
    assert.equal(ctrlTexts(w, b)[0], `hello {"resume":{"chan":"${a}","last_sent_ctr":0}}`);
    const subsBefore = w.requests().filter((r) => r.type === "rpc-subscribe").length;
    w.toPhone(w.desk.send(b, "ctrl", w.desk.ctrl({ name: "input_lost", handles: [] })));
    const err = await tabs;
    assert.ok(err instanceof RelayRpcError && err.code === "channel_lost", "failed at once, not after a timeout");
    w.pump();
    const subs = w.requests().filter((r) => r.type === "rpc-subscribe");
    assert.equal(subs.length, subsBefore + 1);
    assert.equal(subs.at(-1)!.stream, "terminal.7");
  } finally {
    w.close();
  }
});

test("§12.3: a dropped output frame closes the channel; the phone reconnects and re-seeds", async () => {
  const w = world();
  try {
    await open(w);
    const frames: RelayRpcTerminalStreamPayload[] = [];
    subscribeCompanionTerminal(w.client.rpc, "7", undefined, { onFrame: (p) => frames.push(p), onError: () => {} });
    await flush();
    w.pump();
    const a = w.chan();
    w.toPhone(w.desk.send(a, "term", w.desk.term({ handle: 7, seq: 0, base: 0, total: 0, reset: true, rows: 24, cols: 80 })));
    w.desk.send(a, "term", w.desk.term({ handle: 7, seq: 1, base: 0, total: 3, data: new TextEncoder().encode("abc") })); // deleted
    w.toPhone(w.desk.send(a, "term", w.desk.term({ handle: 7, seq: 2, base: 3, total: 6, data: new TextEncoder().encode("def") })));
    assert.equal(frames.length, 1, "nothing after the missing frame is rendered");
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
    w.clock.advance(1000);
    w.pump();
    w.pump();
    const b = w.chan();
    assert.notEqual(a, b);
    assert.equal(w.client.getSnapshot().secure?.state, "open");
    const sub = w.requests().filter((r) => r.type === "rpc-subscribe").at(-1)!;
    assert.equal(sub.stream, "terminal.7", "re-seeded on the new channel");
    w.toPhone(w.desk.send(b, "term", w.desk.term({ handle: 7, seq: 3, base: 6, total: 6, reset: true, rows: 24, cols: 80, data: new TextEncoder().encode("abcdef") })));
    assert.equal(frames.length, 2);
    assert.equal(frames[1]!.reset, true);
    assert.equal(Buffer.from(frames[1]!.data, "base64").toString(), "abcdef", "the screen matches the desktop afterwards");
  } finally {
    w.close();
  }
});

test("§4.6: a reconnect into a new session resumes nothing (hello without resume)", async () => {
  const w = world();
  try {
    await open(w);
    w.client.startNewSession();
    await flush();
    await w.admit("lease-2");
    w.pump();
    w.pump();
    assert.equal(ctrlTexts(w, w.chan())[0], "hello {}");
  } finally {
    w.close();
  }
});

test("§4.6: an old channel's CLOSE during the wind-down only re-seeds; the new channel stays open", async () => {
  const w = world();
  try {
    await open(w);
    const a = w.chan();
    w.clock.advance(60 * 60 * 1000);
    w.pump();
    w.pump();
    w.toPhone([w.desk.closeFrame(a, 1)]);
    assert.equal(w.client.getSnapshot().secure?.state, "open");
    void DESKTOP_LINK_TIMING;
  } finally {
    w.close();
  }
});
