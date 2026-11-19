// The phone link's state machine (docs/design/e2ee.md §4.2–§4.6) over a FAKE
// crypto adapter, so everyday `npm test` needs no cargo. The fake keeps the
// §4.3 counter rules but has no cryptography at all; the real Rust binding
// runs the same scenarios under a malicious relay in `npm run test:e2ee`.
import assert from "node:assert/strict";
import { test } from "node:test";

import type { Channel, Frame, NativeE2ee, PinStore } from "../../../modules/dragent-e2ee/src/types";
import { E2eeError } from "../../../modules/dragent-e2ee/src/api";
import type { RealtimeClock } from "../clock";
import { fromHex, toHex } from "../e2ee/bytes";
import { E2eeDesktopLink, DESKTOP_LINK_TIMING } from "../e2ee/desktopLink";
import { PhoneSecurity } from "../e2ee/security";
import { RelayRpcClient, RelayRpcError } from "../relayRpcClient";

const enc = (v: unknown) => new TextEncoder().encode(JSON.stringify(v));
const dec = <T>(b: Uint8Array) => JSON.parse(new TextDecoder().decode(b)) as T;

type FakeFrame = { kind: Frame["kind"]; step: number; chan: string; ctr: number; body: string; peer: string | null };
type FakeRecord = { mtype: "json" | "term" | "ctrl"; content: string };

function fakeChannel(id: Uint8Array): Channel {
  let next = 0;
  let prior: number | null = null;
  let quiesced = false;
  let closed: string | null = null;
  const tag = toHex(id);
  return {
    chanId: id,
    state: () => (closed ? { state: "closed", reason: closed } : { state: "open" }),
    handshakeHash: () => new Uint8Array(32),
    remoteStatic: () => new Uint8Array(32),
    hasKeys: () => closed === null,
    processedThrough: () => prior,
    lastSentCtr: () => (next === 0 ? null : next - 1),
    isQuiesced: () => quiesced,
    seal: () => { throw new Error("unused"); },
    sealFrame: (record) => {
      if (closed || quiesced) throw new E2eeError("E2EE_NOT_READY", "no");
      const f: FakeFrame = { kind: "data", step: 0, chan: tag, ctr: next++, body: toHex(record) + tag, peer: null };
      return enc(f);
    },
    receive: (ctr, ciphertext) => {
      if (closed) throw new E2eeError("E2EE_CHANNEL_CLOSED", closed);
      const fail = () => { closed = "protocol_error"; throw new E2eeError("E2EE_PROTOCOL_ERROR", "§4.3"); };
      if (ctr >= 2 ** 32) fail();
      if (prior !== null && ctr <= prior) return { kind: "duplicate" };
      const body = new TextDecoder().decode(ciphertext);
      if (!body.endsWith(tag)) fail(); // stands in for the AEAD check
      if (ctr !== (prior === null ? 0 : prior + 1)) fail();
      prior = ctr;
      return { kind: "record", record: fromHex(body.slice(0, -tag.length))! };
    },
    quiesce: () => { quiesced = true; return { chan: id, lastSentCtr: next === 0 ? null : next - 1 }; },
    poll: () => null,
    rekeyDue: () => false,
    close: (r) => { closed ??= r; },
    release: () => {},
  };
}

function fakeNative(): NativeE2ee {
  let chanSeq = 0;
  const store: PinStore = {
    generation: () => 1,
    desktops: () => [{ id: "d", identityKey: new Uint8Array(32), name: "Mac", pinnedAtMs: 0, rotationKey: null }],
    lookup: () => ({ state: "pinned", identityKey: new Uint8Array(32) }),
    candidateKeys: () => [],
    pinDesktop: () => {},
    announceRotation: () => {},
    confirmRotation: () => false,
    revoke: () => true,
    openCatalog: () => { throw new Error("unused"); },
    catalogHighWater: () => null,
    commit: () => 1,
    release: () => {},
  };
  const n: Partial<NativeE2ee> = {
    openPinStore: () => store,
    removePushPin: () => {},
    randomChannelId: () => Uint8Array.from({ length: 16 }, (_, i) => (i === 15 ? ++chanSeq : 0)),
    startKk: (ids) => ({
      hs1: new Uint8Array([1]),
      peerCaps: [],
      finish: (hs2) => {
        if (hs2[0] !== 2) throw new E2eeError("E2EE_REJECTED", "bad hs2");
        return fakeChannel(ids.chan);
      },
      abandon: () => {},
      release: () => {},
    }),
    encodeHs1Frame: (_p, chan, peer, body) => enc({ kind: "hs", step: 1, chan: toHex(chan), ctr: 0, body: toHex(body), peer } satisfies FakeFrame),
    encodeFrame: (f) => enc({ kind: f.kind, step: f.step, chan: toHex(f.chan), ctr: f.ctr, body: toHex(f.body), peer: f.peer } satisfies FakeFrame),
    decodeFrame: (bytes) => {
      const f = dec<FakeFrame>(bytes);
      return { kind: f.kind, step: f.step, purpose: "companion", chan: fromHex(f.chan)!, peer: f.peer, ctr: f.ctr, body: f.kind === "data" ? new TextEncoder().encode(f.body) : fromHex(f.body)! };
    },
    encodeRecords: (mtype, _id, content) => [enc({ mtype, content: toHex(content) } satisfies FakeRecord)],
    createReassembler: () => ({
      feed: (record) => { const r = dec<FakeRecord>(record); return { mtype: r.mtype, content: fromHex(r.content)! }; },
      buffered: () => 0, clear: () => {}, release: () => {},
    }),
  };
  return n as NativeE2ee;
}

class Clock implements RealtimeClock {
  time = 0;
  private seq = 0;
  private readonly timers = new Map<number, { at: number; fn: () => void }>();
  now = () => this.time;
  setTimeout = (fn: () => void, ms: number) => { const id = ++this.seq; this.timers.set(id, { at: this.time + ms, fn }); return id; };
  clearTimeout = (id: unknown) => { this.timers.delete(id as number); };
  advance(ms: number) {
    const end = this.time + ms;
    for (;;) {
      const next = [...this.timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
      if (!next || next[1].at > end) break;
      this.time = next[1].at;
      this.timers.delete(next[0]);
      next[1].fn();
    }
    this.time = end;
  }
}

function setup() {
  const clock = new Clock();
  const native = fakeNative();
  const out: FakeFrame[] = [];
  let ids = 0;
  const rpc = new RelayRpcClient({ send: (f) => link.sendJson(f), clock, createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}` });
  const lost: unknown[] = [];
  const link: E2eeDesktopLink = new E2eeDesktopLink({
    native, security: new PhoneSecurity(native, clock.now), account: "a", phone: "p", desktop: "d", clock, rpc,
    sendBinary: (b) => { out.push(dec<FakeFrame>(b)); return true; },
    onInputLost: (handles, message) => lost.push({ handles, message }),
  });
  const deskSeq = new Map<string, number>();
  /** The desktop sends one record on `chan`. */
  const desk = (chan: string, mtype: FakeRecord["mtype"], content: Uint8Array, ctr?: number) => {
    const c = ctr ?? deskSeq.get(chan) ?? 0;
    deskSeq.set(chan, c + 1);
    const record = toHex(enc({ mtype, content: toHex(content) } satisfies FakeRecord));
    link.onFrame(enc({ kind: "data", step: 0, chan, ctr: c, body: record + chan, peer: null } satisfies FakeFrame));
  };
  const hs2 = (chan: string, ok = true) => link.onFrame(enc({ kind: "hs", step: 2, chan, ctr: 0, body: ok ? "02" : "00", peer: null } satisfies FakeFrame));
  const records = (chan: string) => out.filter((f) => f.kind === "data" && f.chan === chan)
    .map((f) => dec<FakeRecord>(fromHex(f.body.slice(0, -chan.length))!))
    .map((r) => ({ mtype: r.mtype, text: new TextDecoder().decode(fromHex(r.content)!) }));
  return { clock, rpc, link, out, desk, hs2, records, lost };
}

test("§4.2: nothing but HS1 before HS2; hello is the first DATA; queued requests follow it", async () => {
  const t = setup();
  t.rpc.setConnectionEpoch(1);
  t.link.attach();
  assert.deepEqual(t.out.map((f) => f.kind), ["hs"]);
  assert.equal(t.out[0]!.peer, "d");
  const req = t.rpc.request("tabs.list", {});
  assert.equal(t.out.length, 1, "queued, not sent");
  const chan = t.out[0]!.chan;
  t.hs2(chan);
  const recs = t.records(chan);
  assert.deepEqual(recs[0], { mtype: "ctrl", text: "hello {}" });
  assert.equal(JSON.parse(recs[1]!.text).method, "tabs.list");
  t.desk(chan, "json", enc({ type: "rpc-reply", v: 1, id: JSON.parse(recs[1]!.text).id, ok: true, result: { tabs: [] } }));
  assert.deepEqual(await req, { tabs: [] });
});

test("§4.3: replays are dropped; a gap closes with CLOSE protocol_error and retries with a resume", async () => {
  const t = setup();
  t.rpc.setConnectionEpoch(1);
  t.link.attach();
  const a = t.out[0]!.chan;
  t.hs2(a);
  const seen: unknown[] = [];
  t.rpc.subscribe("runtime.events", {}, (p) => seen.push(p));
  const ev = (seq: number) => enc({ type: "rpc-stream", v: 1, stream: "runtime.events", seq, payload: { seq } });
  t.desk(a, "json", ev(1), 0);
  t.desk(a, "json", ev(1), 0); // replay
  t.desk(a, "json", ev(3), 2); // gap
  assert.equal(seen.length, 1);
  assert.equal(t.out.at(-1)!.kind, "close");
  assert.equal(t.link.getStatus().state, "retrying");
  t.clock.advance(DESKTOP_LINK_TIMING.firstRetryMs * 2);
  const b = t.out.at(-1)!;
  assert.equal(b.kind, "hs");
  t.hs2(b.chan);
  const hello = t.records(b.chan)[0]!.text;
  assert.match(hello, new RegExp(`^hello \\{"resume":\\{"chan":"${a}","last_sent_ctr":\\d+\\}\\}$`));
});

test("§4.6: input_lost drops queued input, fails what the desktop discarded, reports, then acks", async () => {
  const t = setup();
  t.rpc.setConnectionEpoch(1);
  t.link.attach();
  const a = t.out[0]!.chan;
  t.hs2(a);
  t.link.detach(); // socket lost: the channel's resume point is kept
  t.rpc.disconnect();
  t.rpc.setConnectionEpoch(2);
  t.link.attach();
  const b = t.out.at(-1)!.chan;
  const typed = t.rpc.request("tabs.input", { handle: 7, text: "x" }).catch((e: unknown) => e);
  t.hs2(b);
  assert.match(t.records(b)[0]!.text, /"resume"/);
  t.desk(b, "ctrl", new TextEncoder().encode('input_lost {"handles":[7]}'));
  const err = await typed;
  assert.ok(err instanceof RelayRpcError && err.code === "input_lost");
  assert.deepEqual(t.lost, [{ handles: [7], message: "Some of what you typed didn't reach Mac" }]);
  assert.equal(t.records(b).at(-1)!.text, "input_lost_ack {}");
});

test("a failed handshake never sends DATA and never falls back", () => {
  const t = setup();
  t.rpc.setConnectionEpoch(1);
  t.link.attach();
  void t.rpc.request("tabs.list", {}).catch(() => {});
  t.hs2(t.out[0]!.chan, false);
  assert.equal(t.link.getStatus().problem, "handshake");
  assert.deepEqual(t.out.map((f) => f.kind), ["hs"]);
});
