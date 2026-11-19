// §12.2 phone cases: a real KK channel (Rust binding) against a scripted
// desktop (real dragent_e2ee) through a scripted, malicious relay.
import assert from "node:assert/strict";
import { test } from "node:test";

import { RelayRpcError } from "../../relayRpcClient";
import { listTabs, subscribeCompanionTerminal, type RelayRpcTerminalStreamPayload } from "../../relayRpcMethods";
import { DESKTOP_LINK_TIMING } from "../../e2ee/desktopLink";
import { DESKTOP, enc, flush, head, PHONE, world, type World } from "./fixture";

async function open(w: World): Promise<void> {
  await w.admit();
  w.pump(); // HS1 → desktop, HS2 → phone
  w.pump(); // hello (+ anything queued) → desktop
  assert.equal(w.client.getSnapshot().secure?.state, "open");
}

/** Subscribe to a desktop-routed stream and collect its payloads. */
function collect(w: World, stream = "runtime.events"): { payloads: unknown[]; errors: RelayRpcError[] } {
  const out = { payloads: [] as unknown[], errors: [] as RelayRpcError[] };
  w.client.rpc.subscribe(stream, {}, (p) => out.payloads.push(p), (e) => out.errors.push(e));
  return out;
}

function streamFrames(w: World, stream: string, n: number): Uint8Array[] {
  const frames: Uint8Array[] = [];
  for (let i = 1; i <= n; i++) frames.push(...w.desk.sendJson(w.chan(), { type: "rpc-stream", v: 1, stream, seq: i, payload: { events: [], n: i } }));
  return frames;
}

function lastPhoneFrame(w: World): Uint8Array { return w.socket().binary.at(-1)!; }

function setCtr(frame: Uint8Array, ctr: bigint): Uint8Array {
  const out = frame.slice();
  new DataView(out.buffer).setBigUint64(36, ctr);
  return out;
}

test("handshake: e2e:1, HS1 targets the desktop, hello is DATA ctr 0, requests ride behind it, nothing as text", async () => {
  const w = world();
  try {
    const s = await w.admit();
    assert.equal(s.text[0]?.e2e, 1, "relay-auth carries e2e:1");
    assert.equal(s.text[0]?.access_token, "account-token");
    assert.equal(s.binary.length, 1, "only HS1 before HS2");
    const tabs = listTabs(w.client.rpc);
    await flush();
    assert.equal(s.binary.length, 1, "no DATA before HS2: the request waits");
    w.pump();
    w.pump();
    const first = w.inbox[0]!;
    assert.ok("mtype" in first && first.mtype === "ctrl" && first.ctr === 0 && first.text === "hello {}" && first.ctrlCanonical);
    const req = w.requests()[0]!;
    assert.equal(req.method, "tabs.list");
    assert.equal((req.params as Record<string, unknown>).device_id, undefined, "no routing ids inside the channel");
    w.reply(req.id as string, { tabs: [{ id: 3, kind: "terminal", title: "zsh", active: true, visible: true }] });
    assert.equal((await tabs)[0]?.title, "zsh");
    assert.deepEqual(s.text.map((f) => f.type), ["relay-auth"], "no desktop traffic as text, ever");
  } finally {
    w.close();
  }
});

test("§4.3: counters 0,1,2 are clean and a replayed frame is dropped silently", async () => {
  const w = world();
  try {
    await open(w);
    const got = collect(w);
    await flush();
    const [f1, f2, f3] = streamFrames(w, "runtime.events", 3);
    w.toPhone([f1!, f2!, f2!, f3!, f1!]);
    assert.equal(got.payloads.length, 3);
    assert.equal(w.client.getSnapshot().secure?.state, "open");
  } finally {
    w.close();
  }
});

test("§4.3: 0,1,3 is protocol_error at frame 3; frame 3 is never processed; pending requests fail at once", async () => {
  const w = world();
  try {
    await open(w);
    const got = collect(w);
    const pending = listTabs(w.client.rpc).catch((e: unknown) => e);
    await flush();
    w.pump();
    const [f1, f2, , f4] = streamFrames(w, "runtime.events", 4);
    const before = w.socket().binary.length;
    w.toPhone([f1!, f2!, f4!]);
    assert.equal(got.payloads.length, 2, "nothing at or after the gap is delivered");
    const err = await pending;
    assert.ok(err instanceof RelayRpcError);
    const snap = w.client.getSnapshot();
    assert.equal(snap.secure?.state, "retrying");
    assert.equal(snap.secure?.message, "Connection to Studio Mac was interrupted. Reconnecting…");
    const close = w.socket().binary.slice(before);
    assert.equal(close.length, 1);
    assert.equal(head(close[0]!).kind, 3, "CLOSE");
    assert.equal(close[0]![44], 1, "reason protocol_error");
    // A later frame of the dead channel is ignored (keys are zeroised).
    w.toPhone(streamFrames(w, "runtime.events", 1));
    assert.equal(got.payloads.length, 2);
  } finally {
    w.close();
  }
});

test("§4.3: a first frame of 2 is a protocol error", async () => {
  const w = world();
  try {
    await open(w);
    const got = collect(w);
    const frames = streamFrames(w, "runtime.events", 3);
    w.toPhone([frames[2]!]);
    assert.equal(got.payloads.length, 0);
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
  } finally {
    w.close();
  }
});

for (const [name, tamper] of [
  ["a bit flip", (f: Uint8Array) => { const o = f.slice(); o[o.length - 20]! ^= 1; return o; }],
  ["a forged high counter (2^20) with a bad tag", (f: Uint8Array) => setCtr(f, 1n << 20n)],
  ["ctr ≥ 2^32", (f: Uint8Array) => setCtr(f, 1n << 32n)],
] as const) {
  test(`§4.3/§12.2: ${name} is rejected and closes the channel`, async () => {
    const w = world();
    try {
      await open(w);
      const got = collect(w);
      const [f1] = streamFrames(w, "runtime.events", 1);
      w.toPhone([tamper(f1!)]);
      assert.equal(got.payloads.length, 0);
      assert.equal(w.client.getSnapshot().secure?.state, "retrying");
      assert.equal(head(lastPhoneFrame(w)).kind, 3);
    } finally {
      w.close();
    }
  });
}

test("§12.2: reordered frames are rejected (no frame past the gap is processed)", async () => {
  const w = world();
  try {
    await open(w);
    const got = collect(w);
    const [f1, f2] = streamFrames(w, "runtime.events", 2);
    w.toPhone([f2!, f1!]);
    assert.equal(got.payloads.length, 0);
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
  } finally {
    w.close();
  }
});

test("§12.2: a substituted peer (prologue mismatch) is refused; the phone never downgrades", async () => {
  const w = world();
  try {
    const other = "0a0b0c0d-0000-4000-8000-0000000000ff";
    w.desk.pin(other, w.native.identityPublicKey()!); // same key, wrong id in the prologue
    await w.admit();
    const tabs = listTabs(w.client.rpc).catch((e: unknown) => e);
    await flush();
    w.hs1Peer = other; // the relay lies about who initiated
    w.pump();
    assert.ok(await tabs instanceof RelayRpcError, "the queued request fails, nothing is sent in the clear");
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
    assert.deepEqual(w.socket().text.map((f) => f.type), ["relay-auth"]);
  } finally {
    w.close();
  }
});

test("§12.2: a tampered HS2 fails the handshake; the phone retries with a fresh HS1 after a backoff", async () => {
  const w = world();
  try {
    const s = await w.admit();
    const h1 = s.binary[0]!;
    const r = w.desk.listen((await import("./fixture")).rewritePeer(h1, PHONE), w.clock.now());
    const bad = r.hs2!.slice();
    bad[bad.length - 1]! ^= 1;
    w.relayed.set(s, 1);
    w.toPhone([bad]);
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
    assert.equal(w.client.getSnapshot().secure?.problem, "handshake");
    assert.equal(s.binary.length, 1, "no DATA after a failed handshake");
    w.clock.advance(1000); // first backoff: 500 ms (±20%)
    assert.equal(s.binary.length, 2);
    assert.equal(head(s.binary[1]!).kind, 1, "a fresh HS1");
    w.pump();
    w.pump();
    assert.equal(w.client.getSnapshot().secure?.state, "open");
  } finally {
    w.close();
  }
});

test("§12.2: a handshake that never completes times out; no DATA is ever sent", async () => {
  const w = world();
  try {
    const s = await w.admit();
    w.clock.advance(DESKTOP_LINK_TIMING.handshakeMs + 1000);
    assert.equal(w.client.getSnapshot().secure?.problem, "handshake");
    assert.ok(s.binary.every((f) => head(f).kind === 1));
  } finally {
    w.close();
  }
});

test("§12.2: a frame from another channel spliced into this one fails AEAD", async () => {
  const w = world();
  try {
    await open(w);
    const first = w.chan();
    const got = collect(w);
    await flush();
    // Rekey to a second channel, then splice a first-channel frame into it.
    w.clock.advance(60 * 60 * 1000);
    w.pump();
    w.pump();
    const second = w.chan();
    assert.notEqual(first, second);
    const [oldFrame] = streamFrames(w, "runtime.events", 1).map(() => w.desk.sendJson(first, { type: "rpc-stream", v: 1, stream: "runtime.events", seq: 1, payload: { events: [] } })[0]!);
    const spliced = oldFrame!.slice();
    spliced.set(Buffer.from(second, "hex"), 4);
    w.toPhone([spliced]);
    assert.equal(got.payloads.length, 0);
    assert.equal(w.client.getSnapshot().secure?.state, "retrying");
  } finally {
    w.close();
  }
});

test("§4.6 rekey: after HS1 nothing more goes on the old channel; hello resumes it; bye{rekeyed} retires it with no re-seed", async () => {
  const w = world();
  try {
    await open(w);
    const a = w.chan();
    w.clock.advance(60 * 60 * 1000 - 3000);
    const t1 = listTabs(w.client.rpc).catch((e: unknown) => e);
    const t2 = listTabs(w.client.rpc).catch((e: unknown) => e);
    await flush();
    w.pump();
    w.clock.advance(3000); // rekey due (60 minutes, §4.6)
    const s = w.socket();
    const hs1At = s.binary.findIndex((f, i) => i >= (w.relayed.get(s) ?? 0) && head(f).kind === 1);
    assert.ok(hs1At >= 0, "a new HS1 went out");
    const queued = listTabs(w.client.rpc);
    await flush();
    assert.ok(s.binary.slice(hs1At).every((f) => head(f).chan !== a), "no DATA on the old channel after HS1");
    w.pump();
    w.pump();
    const b = w.chan();
    const hello = w.inbox.find((m) => m.chan === b && "text" in m)!;
    assert.ok("text" in hello);
    assert.equal(hello.text, `hello {"resume":{"chan":"${a}","last_sent_ctr":2}}`);
    assert.ok(hello.ctrlCanonical);
    const req = w.requests().at(-1)!;
    w.reply(req.id as string, { tabs: [] }, b);
    assert.deepEqual(await queued, []);
    // Owed replies still finish on the old channel, then bye{rekeyed}.
    const old = w.requests().filter((r) => r.method === "tabs.list").slice(0, 2);
    w.reply(old[0]!.id as string, { tabs: [] }, a);
    w.reply(old[1]!.id as string, { tabs: [] }, a);
    assert.deepEqual(await t1, []);
    assert.deepEqual(await t2, []);
    const subs = w.requests().length;
    w.toPhone(w.desk.send(a, "ctrl", w.desk.ctrl({ name: "bye", reason: "rekeyed" })));
    w.clock.advance(DESKTOP_LINK_TIMING.windDownMs + 1000);
    w.pump();
    assert.equal(w.requests().length, subs, "a clean rekey re-seeds nothing");
    assert.equal(w.client.getSnapshot().secure?.state, "open");
  } finally {
    w.close();
  }
});

test("note 1: an old channel that ends without bye{rekeyed} re-seeds every terminal stream", async () => {
  const w = world();
  try {
    await open(w);
    const frames: RelayRpcTerminalStreamPayload[] = [];
    subscribeCompanionTerminal(w.client.rpc, "7", undefined, { onFrame: (p) => frames.push(p), onError: () => {} });
    await flush();
    w.pump();
    w.toPhone(w.desk.send(w.chan(), "term", w.desk.term({ handle: 7, seq: 0, base: 10, total: 10, reset: true, rows: 24, cols: 80 })));
    assert.equal(frames.length, 1);
    w.clock.advance(60 * 60 * 1000);
    w.pump();
    w.pump();
    const before = w.requests().filter((r) => r.type === "rpc-subscribe").length;
    w.clock.advance(DESKTOP_LINK_TIMING.windDownMs + 1000);
    w.pump();
    const subs = w.requests().filter((r) => r.type === "rpc-subscribe");
    assert.equal(subs.length, before + 1, "re-seeded");
    assert.equal(subs.at(-1)!.stream, "terminal.7");
  } finally {
    w.close();
  }
});

test("TERM → today's terminal.output payload; a discontinuity re-seeds instead of rendering past it", async () => {
  const w = world();
  try {
    await open(w);
    const frames: RelayRpcTerminalStreamPayload[] = [];
    subscribeCompanionTerminal(w.client.rpc, "7", undefined, { onFrame: (p) => frames.push(p), onError: () => {} });
    await flush();
    w.pump();
    const sub = w.requests().at(-1)!;
    assert.deepEqual(sub.params, { handle: 7 }, "the desktop-minted u32 handle on the wire");
    assert.equal(sub.stream, "terminal.7");
    const c = w.chan();
    w.toPhone(w.desk.send(c, "term", w.desk.term({ handle: 7, seq: 0, base: 100, total: 100, reset: true, rows: 24, cols: 80, data: enc("\u001b[2J$ ") })));
    w.toPhone(w.desk.send(c, "term", w.desk.term({ handle: 7, seq: 1, base: 100, total: 105, data: enc("hello") })));
    assert.equal(frames.length, 2);
    assert.deepEqual({ ...frames[0] }, { base: 100, total: 100, data: Buffer.from("\u001b[2J$ ").toString("base64"), decoded: 6, alive: true, exitCode: null, reset: true, rows: 24, cols: 80 });
    assert.deepEqual({ ...frames[1] }, { base: 100, total: 105, data: Buffer.from("hello").toString("base64"), decoded: 5, alive: true, exitCode: null, reset: false, rows: null, cols: null });
    // Skip seq 2: never rendered, and the stream asks for a fresh seed.
    w.toPhone(w.desk.send(c, "term", w.desk.term({ handle: 7, seq: 3, base: 110, total: 115, data: enc("world") })));
    assert.equal(frames.length, 2);
    w.pump();
    assert.equal(w.requests().at(-1)!.type, "rpc-subscribe");
    assert.equal(w.client.getSnapshot().secure?.state, "open", "a stream gap is not a channel error");
    // The previous subscription's tail is ignored until the new seed.
    w.toPhone(w.desk.send(c, "term", w.desk.term({ handle: 7, seq: 4, base: 115, total: 116, data: enc("x") })));
    assert.equal(frames.length, 2);
    w.toPhone(w.desk.send(c, "term", w.desk.term({ handle: 7, seq: 5, base: 116, total: 116, reset: true, rows: 24, cols: 80 })));
    assert.equal(frames.length, 3);
    assert.equal(frames[2]!.reset, true);
  } finally {
    w.close();
  }
});

test("§4.5 parts: a seed is applied atomically, with its declared omission; a hash mismatch is seed_failed, not a reconnect", async () => {
  const w = world();
  try {
    await open(w);
    const frames: RelayRpcTerminalStreamPayload[] = [];
    const errors: RelayRpcError[] = [];
    subscribeCompanionTerminal(w.client.rpc, "9", undefined, { onFrame: (p) => frames.push(p), onError: (e) => errors.push(e) });
    await flush();
    w.pump();
    const c = w.chan();
    const seed = enc(JSON.stringify({ handle: 9, base: 500, total: 500, rows: 30, cols: 100, data: Buffer.from("x".repeat(200_000)).toString("base64"), scrollback_omitted_lines: 42 }));
    const half = Math.ceil(seed.length / 2);
    const send = (sha?: string) => {
      w.toPhone(w.desk.send(c, "ctrl", w.desk.partBegin({ partId: "s1", purpose: "seed", streamHandle: 9, partCount: 2, payload: seed, ...(sha ? { sha256: sha } : {}) })));
      w.toPhone(w.desk.send(c, "json", w.desk.partChunk("s1", 0, seed.subarray(0, half))));
      assert.equal(frames.length, 0, "nothing partial is ever rendered");
      w.toPhone(w.desk.send(c, "json", w.desk.partChunk("s1", 1, seed.subarray(half))));
      w.toPhone(w.desk.send(c, "ctrl", w.desk.partEnd("s1")));
    };
    send();
    assert.equal(frames.length, 1);
    assert.equal(frames[0]!.reset, true);
    assert.equal(frames[0]!.decoded, 200_000);
    assert.equal(frames[0]!.scrollbackOmittedLines, 42);
    // Now a bad hash on a second stream.
    const frames2: RelayRpcTerminalStreamPayload[] = [];
    subscribeCompanionTerminal(w.client.rpc, "10", undefined, { onFrame: (p) => frames2.push(p), onError: (e) => errors.push(e) });
    await flush();
    w.pump();
    const seed2 = enc(JSON.stringify({ handle: 10, base: 0, total: 0, rows: 30, cols: 100, data: "" }));
    w.toPhone(w.desk.send(c, "ctrl", w.desk.partBegin({ partId: "s2", purpose: "seed", streamHandle: 10, partCount: 1, payload: seed2, sha256: "00".repeat(32) })));
    w.toPhone(w.desk.send(c, "json", w.desk.partChunk("s2", 0, seed2)));
    w.toPhone(w.desk.send(c, "ctrl", w.desk.partEnd("s2")));
    assert.equal(frames2.length, 0);
    assert.equal(errors.length, 1);
    assert.equal(errors[0]!.code, "seed_failed");
    assert.equal(errors[0]!.message, "Couldn't load this terminal's history");
    assert.equal(w.client.getSnapshot().secure?.state, "open", "no reconnect loop");
  } finally {
    w.close();
  }
});

test("§4.5 parts: a large reply is reassembled and verified; an aborted one fails at once with reply_failed", async () => {
  const w = world();
  try {
    await open(w);
    const ok = w.client.rpc.request("files.open", { path: "/a" }).catch((e: unknown) => e);
    const bad = w.client.rpc.request("files.open", { path: "/b" }).catch((e: unknown) => e);
    await flush();
    w.pump();
    const [r1, r2] = w.requests().slice(-2);
    const c = w.chan();
    const big = enc(JSON.stringify({ type: "rpc-reply", v: 1, id: r1!.id, ok: true, result: { content: "y".repeat(900_000), truncated: false } }));
    const cut = 500_000;
    w.toPhone(w.desk.send(c, "ctrl", w.desk.partBegin({ partId: "r1", purpose: "reply", requestId: r1!.id, partCount: 2, payload: big })));
    w.toPhone(w.desk.send(c, "json", w.desk.partChunk("r1", 0, big.subarray(0, cut))));
    w.toPhone(w.desk.send(c, "json", w.desk.partChunk("r1", 1, big.subarray(cut))));
    w.toPhone(w.desk.send(c, "ctrl", w.desk.partEnd("r1")));
    assert.equal(((await ok) as { content: string }).content.length, 900_000);
    const small = enc(JSON.stringify({ type: "rpc-reply", v: 1, id: r2!.id, ok: true, result: {} }));
    w.toPhone(w.desk.send(c, "ctrl", w.desk.partBegin({ partId: "r2", purpose: "reply", requestId: r2!.id, partCount: 1, payload: small })));
    w.toPhone(w.desk.send(c, "ctrl", enc('part-abort {"part_id":"r2"}')));
    const e = await bad;
    assert.ok(e instanceof RelayRpcError && e.code === "reply_failed");
    assert.equal(w.client.getSnapshot().secure?.state, "open");
  } finally {
    w.close();
  }
});

test("§12.7: a targeted socket without E2EE never sends desktop traffic; the web refuses", async () => {
  const { resolveE2ee } = await import("../../../../modules/dragent-e2ee/src/api");
  assert.throws(() => resolveE2ee("web", () => null), /only in the Dragent iOS and Android apps/);
  const w = world({ e2ee: { unavailable: "End-to-end encryption runs only in the Dragent iOS and Android apps." } });
  try {
    const s = await w.admit();
    await assert.rejects(listTabs(w.client.rpc), /Could not send/);
    assert.deepEqual(s.text.map((f) => f.type), ["relay-auth"]);
    assert.equal(s.binary.length, 0);
    assert.equal(w.client.getSnapshot().secure?.state, "unavailable");
    assert.equal(w.client.getSnapshot().secure?.message, "End-to-end encryption runs only in the Dragent iOS and Android apps.");
  } finally {
    w.close();
  }
});

test("§12.7: a pre-admission 4403 refusal latches (old build / pair again) with no retry", async () => {
  const w = world();
  try {
    w.client.connect();
    await flush();
    const s = w.socket();
    s.open();
    s.receiveText({ type: "relay-error", v: 1, message: "Pair this phone again to keep using it. Phone access is now end-to-end encrypted.", repairRequired: true });
    s.drop(4403);
    w.clock.advance(120_000);
    assert.equal(w.sockets.length, 1, "no redial");
    assert.equal(w.client.getSnapshot().status, "error");
    assert.equal(w.client.getSnapshot().error, "Pair this phone again to keep using it. Phone access is now end-to-end encrypted.");
    assert.equal(s.binary.length, 0, "never admitted, nothing sent");
    void DESKTOP;
  } finally {
    w.close();
  }
});
