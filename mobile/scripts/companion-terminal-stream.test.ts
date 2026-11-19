import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPANION_STREAM_FIRST_FRAME_TIMEOUT_MS,
  COMPANION_STREAM_MAX_RESUBSCRIBES,
  CompanionTerminalEngine,
  type CompanionTerminalRpc,
  type CompanionTimers,
} from "../src/terminal/companion/engine";
import { createCompanionHandleRegistry } from "../src/terminal/companion/registry";
import { COMPANION_TERMINAL_MESSAGES } from "../src/terminal/companion/types";
import {
  RELAY_RPC_UNREADABLE_MESSAGE,
  parseRelayRpcTerminalStreamPayload,
  type CompanionTerminalStreamListener,
  type RelayRpcTerminalStreamPayload,
} from "../src/transport/relayRpcMethods";
import { RelayRpcError } from "../src/transport/relayRpcClient";
import type { TerminalDelta } from "../src/transport/protocol";

const H1 = "11111111-1111-4111-8111-111111111111";
const H2 = "22222222-2222-4222-8222-222222222222";
const b64 = (text: string): string => Buffer.from(text, "utf8").toString("base64");
const tick = () => new Promise((resolve) => setImmediate(resolve));

function raw(base: number, text: string, extra: Partial<RelayRpcTerminalStreamPayload> = {}): RelayRpcTerminalStreamPayload {
  const decoded = Buffer.byteLength(text);
  return {
    base, total: base + decoded, data: b64(text), decoded, alive: true, exitCode: null, reset: false, rows: null, cols: null, ...extra,
  };
}
function seed(base: number, text: string, rows = 30, cols = 100): RelayRpcTerminalStreamPayload {
  return { base, total: base, data: b64(text), decoded: Buffer.byteLength(text), alive: true, exitCode: null, reset: true, rows, cols };
}

class FakeTimers implements CompanionTimers {
  time = 0;
  private next = 1;
  private pending = new Map<number, { at: number; fn: () => void }>();
  now = () => this.time;
  setTimeout = (fn: () => void, ms: number) => {
    const id = this.next++;
    this.pending.set(id, { at: this.time + ms, fn });
    return id;
  };
  clearTimeout = (id: unknown) => { this.pending.delete(id as number); };
  advance(ms: number): void {
    const until = this.time + ms;
    for (;;) {
      const due = [...this.pending.entries()].filter(([, t]) => t.at <= until).sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      this.pending.delete(due[0]);
      this.time = due[1].at;
      due[1].fn();
    }
    this.time = until;
  }
}

interface Sub {
  handle: string;
  offset: number | undefined;
  listener: CompanionTerminalStreamListener;
  closed: boolean;
}

function rig(opts: { alternateScreen?: boolean; noStream?: boolean } = {}) {
  const subs: Sub[] = [];
  const reads: number[] = [];
  const screens: string[] = [];
  const inputs: string[] = [];
  const received: TerminalDelta[] = [];
  const viewports: number[][] = [];
  const modes: unknown[] = [];
  let resets = 0;
  let attachCount = 0;
  const timers = new FakeTimers();
  const rpc: CompanionTerminalRpc = {
    attach: async () => {
      attachCount += 1;
      return { handle: attachCount === 1 ? H1 : H2, rows: 24, cols: 80, alternateScreen: opts.alternateScreen ?? false };
    },
    read: async (_h, offset) => {
      reads.push(offset);
      return { base: offset, total: offset, data: "", alive: true, exitCode: null };
    },
    screen: async (h) => {
      screens.push(h);
      return { complete: true, total: 0, alternateScreen: true, mouseReporting: true, sgrMouse: true, bracketedPaste: true,
        delta: { kind: "alt_screen", rows: 2, cols: 10, cursor_row: 0, cursor_col: 0, cursor_visible: true, lines: [] } };
    },
    input: async (_h, text) => { inputs.push(text); return { sent: true }; },
    resize: async () => ({ resized: true }),
    detach: async () => ({ detached: true }),
  };
  if (!opts.noStream) {
    rpc.subscribe = (handle, offset, listener) => {
      const sub: Sub = { handle, offset, listener, closed: false };
      subs.push(sub);
      return { close: () => { sub.closed = true; } };
    };
  }
  const engine = new CompanionTerminalEngine(
    { accountId: "a", deviceId: "d", tabId: 1, client: {}, epoch: 1 },
    rpc,
    {
      receive: (delta) => received.push(delta),
      reset: () => { resets += 1; },
      setViewport: (rows, cols) => viewports.push([rows, cols]),
      setScreenModes: (m) => modes.push(m),
    },
    createCompanionHandleRegistry(),
    async () => {},
    timers,
  );
  const last = () => subs[subs.length - 1] as Sub;
  const chunks = () => received.filter((d) => d.kind === "output_chunk") as Array<{ seq: number; data_base64: string }>;
  return { engine, subs, last, reads, screens, inputs, received, chunks, viewports, modes, timers, rpc, get resets() { return resets; } };
}

test("stream happy path: primary screen subscribes from 0, feeds frames, polls nothing", async () => {
  const r = rig();
  await r.engine.attach();
  assert.equal(r.subs.length, 1);
  assert.equal(r.last().offset, 0);
  assert.equal(r.last().handle, H1);
  r.last().listener.onFrame(raw(0, "$ "), { first: true });
  r.last().listener.onFrame(raw(2, "ls\r\n"), { first: false });
  assert.deepEqual(r.chunks().map((c) => [c.seq, Buffer.from(c.data_base64, "base64").toString()]), [[1, "$ "], [2, "ls\r\n"]]);
  assert.equal(r.engine.getStatus().offset, 6);
  assert.equal(r.engine.getStatus().phase, "live-raw");
  for (let i = 0; i < 5; i++) await r.engine.pollOnce();
  assert.deepEqual(r.reads, [], "no polling while a stream is live");
  assert.equal(r.engine.getStreamState().support, "works");
});

test("status listeners are not woken per frame; only on visible changes", async () => {
  const r = rig();
  await r.engine.attach();
  let published = 0;
  r.engine.subscribe(() => { published += 1; });
  r.last().listener.onFrame(raw(0, "a"), { first: true });
  for (let i = 1; i < 50; i++) r.last().listener.onFrame(raw(i, "b"), { first: false });
  assert.equal(published, 0);
  assert.equal(r.engine.getStatus().offset, 50);
});

test("an idle terminal's empty first frame means streaming works; no fallback later", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onFrame({ base: 0, total: 0, data: "", decoded: 0, alive: true, exitCode: null, reset: false, rows: null, cols: null }, { first: true });
  r.timers.advance(COMPANION_STREAM_FIRST_FRAME_TIMEOUT_MS * 3);
  await r.engine.pollOnce();
  assert.deepEqual(r.reads, []);
  assert.equal(r.engine.getStreamState().support, "works");
  assert.equal(r.last().closed, false);
  assert.equal(r.chunks().length, 0);
});

test("alt-screen attach subscribes for a seed: new epoch, viewport, seed fed first, no snapshots", async () => {
  const r = rig({ alternateScreen: true });
  await r.engine.attach();
  assert.equal(r.last().offset, undefined, "no offset: the first frame is a seed");
  assert.deepEqual(r.screens, [], "no snapshot fetch on attach");
  assert.equal(r.engine.getStatus().mode, "raw");
  const resetsBefore = r.resets;
  r.last().listener.onFrame(seed(5000, "\u001b[?1049hSEED", 40, 120), { first: true });
  assert.equal(r.resets, resetsBefore + 1, "seed starts a new runtime epoch");
  assert.deepEqual(r.viewports.at(-1), [40, 120]);
  assert.deepEqual(r.chunks().map((c) => c.seq), [1]);
  assert.equal(Buffer.from(r.chunks()[0]!.data_base64, "base64").toString(), "\u001b[?1049hSEED");
  assert.equal(r.engine.getStatus().offset, 5000, "offset = seed base");
  r.last().listener.onFrame(raw(5000, "x"), { first: false });
  assert.deepEqual(r.chunks().map((c) => c.seq), [1, 2]);
  assert.equal(r.engine.getStatus().offset, 5001);
  for (let i = 0; i < 3; i++) await r.engine.pollOnce();
  assert.deepEqual(r.screens, []);
  assert.deepEqual(r.reads, []);
  // Modes: one refresh after the seed settles, never per frame.
  r.timers.advance(1000);
  await tick();
  assert.equal(r.screens.length, 1);
  assert.equal(r.engine.getScreenModes().bracketedPaste, true);
  for (let i = 1; i < 20; i++) r.last().listener.onFrame(raw(5000 + i, "y"), { first: false });
  r.timers.advance(1000);
  await tick();
  assert.equal(r.screens.length, 1, "continuous output does not refresh modes");
  // Output after a quiet spell refreshes once.
  r.timers.advance(5000);
  r.last().listener.onFrame(raw(5020, "z"), { first: false });
  r.timers.advance(1000);
  await tick();
  assert.equal(r.screens.length, 2);
});

test("a gap resubscribes without offset for a seed and never concatenates", async () => {
  const r = rig();
  await r.engine.attach();
  const first = r.last();
  first.listener.onFrame(raw(0, "abc"), { first: true });
  first.listener.onFrame(raw(10, "LATE"), { first: false });
  assert.equal(r.chunks().length, 1, "the bytes after the hole are not fed");
  assert.equal(first.closed, true);
  assert.equal(r.subs.length, 2);
  assert.equal(r.last().offset, undefined);
  // Frames of the old subscription are ignored from now on.
  first.listener.onFrame(raw(3, "old"), { first: false });
  assert.equal(r.chunks().length, 1);
  r.last().listener.onFrame(seed(20, "SCREEN"), { first: true });
  assert.equal(r.engine.getStatus().offset, 20);
  assert.deepEqual(r.chunks().map((c) => c.seq), [1, 1], "seed restarts the chunk sequence in its new epoch");
});

test("subscribe error before any frame falls back to today's polling", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onError(new RelayRpcError("unknown stream"));
  assert.equal(r.engine.getStreamState().support, "unsupported");
  await r.engine.pollOnce();
  assert.deepEqual(r.reads, [0]);
});

test("alt-screen subscribe error before any frame falls back to snapshot polling", async () => {
  const r = rig({ alternateScreen: true });
  await r.engine.attach();
  r.last().listener.onError(new RelayRpcError("unknown stream"));
  await tick();
  assert.equal(r.engine.getStatus().mode, "snapshot");
  assert.equal(r.screens.length, 1, "current screen fetched at once");
  assert.ok(r.received.some((d) => d.kind === "alt_screen"));
  await r.engine.pollOnce();
  assert.equal(r.screens.length, 2);
});

test("no frame at all before the safety-net timeout falls back to polling", async () => {
  const r = rig();
  await r.engine.attach();
  r.timers.advance(COMPANION_STREAM_FIRST_FRAME_TIMEOUT_MS);
  assert.equal(r.last().closed, true);
  await r.engine.pollOnce();
  assert.deepEqual(r.reads, [0]);
});

test("an error after frames resubscribes from the current offset with backoff, then reattaches", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onFrame(raw(0, "hello"), { first: true });
  r.last().listener.onError(new RelayRpcError("phone fell behind"));
  assert.equal(r.subs.length, 1, "waits for the backoff");
  await r.engine.pollOnce();
  assert.deepEqual(r.reads, [], "no polling during stream recovery");
  r.timers.advance(250);
  assert.equal(r.subs.length, 2);
  assert.equal(r.last().offset, 5);
  // A frame resets the budget; consecutive failures exhaust it.
  r.last().listener.onFrame(raw(5, ""), { first: true });
  for (let i = 0; i < COMPANION_STREAM_MAX_RESUBSCRIBES; i++) {
    r.last().listener.onError(new RelayRpcError("desktop stream ended"));
    r.timers.advance(10_000);
  }
  assert.equal(r.subs.length, 2 + COMPANION_STREAM_MAX_RESUBSCRIBES);
  assert.ok(r.subs.slice(2).every((s) => s.offset === 5));
  r.last().listener.onError(new RelayRpcError("desktop stream ended"));
  await tick();
  await tick();
  // Reattach got a new handle and streams again from 0 on it.
  assert.equal(r.engine.getStatus().handle, H2);
  assert.equal(r.last().handle, H2);
  assert.equal(r.last().offset, 0);
  assert.deepEqual(r.reads, []);
});

test("an unreadable frame after a working stream counts as a stream failure", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onFrame(raw(0, "ok"), { first: true });
  r.last().listener.onError(new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE));
  r.timers.advance(250);
  assert.equal(r.subs.length, 2);
  assert.equal(r.last().offset, 2);
});

test("exit closes the view with the exit code and stops streaming", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onFrame(raw(0, "bye"), { first: true });
  r.last().listener.onFrame({ ...raw(3, ""), alive: false, exitCode: 2 }, { first: false });
  const status = r.engine.getStatus();
  assert.equal(status.phase, "closed");
  assert.equal(status.exitCode, 2);
  assert.equal(status.alive, false);
  assert.equal(status.message, `${COMPANION_TERMINAL_MESSAGES.closed} (exit 2)`);
  assert.equal(r.last().closed, true);
  await r.engine.pollOnce();
  assert.deepEqual(r.reads, []);
});

test("dispose unsubscribes and late frames never reach the store", async () => {
  const r = rig();
  await r.engine.attach();
  const sub = r.last();
  sub.listener.onFrame(raw(0, "a"), { first: true });
  r.engine.dispose();
  assert.equal(sub.closed, true);
  sub.listener.onFrame(raw(1, "late"), { first: false });
  sub.listener.onError(new RelayRpcError("late"));
  r.timers.advance(60_000);
  assert.equal(r.chunks().length, 1);
  assert.equal(r.subs.length, 1);
});

test("a renderer needing dropped bytes asks for a seed; a seed in flight is not asked twice", async () => {
  const r = rig();
  await r.engine.attach();
  r.last().listener.onFrame(raw(0, "a"), { first: true });
  r.engine.requestReseed();
  assert.equal(r.subs.length, 2);
  assert.equal(r.subs[0]!.closed, true);
  assert.equal(r.last().offset, undefined);
  r.engine.requestReseed();
  r.engine.requestReseed();
  assert.equal(r.subs.length, 2, "seed already on its way");
});

test("polling fallback: a renderer needing dropped bytes switches to snapshots", async () => {
  const r = rig({ noStream: true });
  await r.engine.attach();
  r.engine.requestReseed();
  assert.equal(r.engine.getStatus().mode, "snapshot");
});

test("polling fallback no longer caps raw history in the engine", async () => {
  const r = rig({ noStream: true });
  let offset = 0;
  r.rpc.read = async (_h, at) => { r.reads.push(at); offset = at + 1; return { base: at, total: at + 1, data: b64("x"), alive: true, exitCode: null }; };
  await r.engine.attach();
  for (let i = 0; i < 600; i++) await r.engine.pollOnce();
  assert.equal(r.engine.getStatus().mode, "raw");
  assert.equal(r.chunks().length, 600);
  assert.equal(offset, 600);
});

// ---- Keystroke merging ---------------------------------------------------

function gatedInput(r: ReturnType<typeof rig>) {
  const gates: Array<(ok: boolean) => void> = [];
  r.rpc.input = (_h, text) => {
    r.inputs.push(text);
    return new Promise((resolve, reject) => {
      gates.push((ok) => (ok ? resolve({ sent: true }) : reject(new Error("lost"))));
    });
  };
  return gates;
}

test("keys typed while an input is in flight go out together, in order", async () => {
  const r = rig();
  await r.engine.attach();
  const gates = gatedInput(r);
  const a = r.engine.sendInput("h");
  await tick();
  assert.deepEqual(r.inputs, ["h"]);
  const b = r.engine.sendInput("e");
  const c = r.engine.sendInput("l");
  const d = r.engine.sendInput("lo");
  gates[0]!(true);
  await tick();
  assert.deepEqual(r.inputs, ["h", "ello"]);
  gates[1]!(true);
  assert.deepEqual(await Promise.all([a, b, c, d]), [{ ok: true }, { ok: true }, { ok: true }, { ok: true }]);
});

test("a merged request stays within 16 KiB of UTF-8", async () => {
  const r = rig();
  await r.engine.attach();
  const gates = gatedInput(r);
  void r.engine.sendInput("x");
  await tick();
  const big = "é".repeat(4096); // 8 KiB
  void r.engine.sendInput(big);
  void r.engine.sendInput(big); // exactly 16 KiB together
  void r.engine.sendInput("!"); // would overflow: its own request
  gates[0]!(true);
  await tick();
  assert.equal(r.inputs[1], big + big);
  gates[1]!(true);
  await tick();
  assert.equal(r.inputs[2], "!");
  assert.ok(r.inputs.every((t) => Buffer.byteLength(t) <= 16 * 1024));
});

test("keys are never merged across a Send job", async () => {
  const r = rig();
  await r.engine.attach();
  const gates = gatedInput(r);
  void r.engine.sendInput("a");
  await tick();
  void r.engine.sendInput("b");
  const sent = r.engine.submitDraft("draft");
  void r.engine.sendInput("c");
  void r.engine.sendInput("d");
  gates[0]!(true); await tick(); // "b"
  gates[1]!(true); await tick(); await tick(); // draft text
  gates[2]!(true); await tick(); await tick(); // CR
  gates[3]!(true);
  assert.equal((await sent).ok, true);
  await tick();
  assert.equal(r.inputs[0], "a");
  assert.equal(r.inputs[1], "b");
  assert.ok(r.inputs[2]!.includes("draft"));
  assert.equal(r.inputs[3], "\r");
  assert.equal(r.inputs[4], "cd");
});

test("a failed write fails its merged keys and drops what queued behind; later typing works", async () => {
  const r = rig();
  await r.engine.attach();
  const gates = gatedInput(r);
  const a = r.engine.sendInput("a");
  await tick();
  const b = r.engine.sendInput("b");
  const c = r.engine.sendInput("c");
  gates[0]!(false);
  assert.deepEqual(await a, { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed });
  assert.deepEqual(await b, { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendNotReady });
  assert.deepEqual(await c, { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendNotReady });
  assert.deepEqual(r.inputs, ["a"]);
  const d = r.engine.sendInput("d");
  await tick();
  gates[1]!(true);
  assert.deepEqual(await d, { ok: true });
  assert.deepEqual(r.inputs, ["a", "d"]);
});

test("a failed merged write fails every key in it", async () => {
  const r = rig();
  await r.engine.attach();
  const gates = gatedInput(r);
  void r.engine.sendInput("a");
  await tick();
  const b = r.engine.sendInput("b");
  const c = r.engine.sendInput("c");
  gates[0]!(true);
  await tick();
  assert.equal(r.inputs[1], "bc");
  gates[1]!(false);
  assert.deepEqual(await b, { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed });
  assert.deepEqual(await c, { ok: false, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed });
});

// ---- Payload parser ------------------------------------------------------

test("terminal stream payload parser is strict", () => {
  const ok = parseRelayRpcTerminalStreamPayload({ base: 3, total: 5, data: b64("ab"), alive: true, reset: false });
  assert.ok(ok && ok.decoded === 2 && ok.rows === null);
  assert.ok(parseRelayRpcTerminalStreamPayload({ base: 3, total: 3, data: "", alive: true, reset: false }), "empty first frame");
  const seedFrame = parseRelayRpcTerminalStreamPayload({ base: 9, total: 12, data: b64("S"), alive: true, reset: true, rows: 24, cols: 80 });
  assert.ok(seedFrame && seedFrame.rows === 24 && seedFrame.cols === 80);
  const exited = parseRelayRpcTerminalStreamPayload({ base: 3, total: 3, data: "", alive: false, exit_code: 1, reset: false });
  assert.equal(exited?.exitCode, 1);
  const bad: unknown[] = [
    null, [], "x",
    { base: 3, total: 4, data: b64("ab"), alive: true, reset: false }, // base + decoded > total
    { base: 5, total: 4, data: "", alive: true, reset: false }, // base > total
    { base: -1, total: 4, data: "", alive: true, reset: false },
    { base: 1.5, total: 4, data: "", alive: true, reset: false },
    { base: 0, total: 4, data: "!!!!", alive: true, reset: false }, // not base64
    { base: 0, total: 4, data: "abc", alive: true, reset: false }, // bad length
    { base: 0, total: 4, data: "", alive: "yes", reset: false },
    { base: 0, total: 4, data: "", alive: true }, // no reset
    { base: 0, total: 4, data: "", alive: true, reset: false, exit_code: 0 }, // exit code while alive
    { base: 0, total: 4, data: "", alive: false, reset: false, exit_code: 1.5 },
    { base: 0, total: 4, data: "", alive: true, reset: false, rows: 24, cols: 80 }, // grid on raw
    { base: 0, total: 0, data: "", alive: true, reset: true }, // seed without grid
    { base: 0, total: 0, data: "", alive: true, reset: true, rows: 0, cols: 80 },
    { base: 0, total: 0, data: "", alive: true, reset: true, rows: 241, cols: 80 },
    { base: 0, total: 0, data: "", alive: true, reset: true, rows: 200, cols: 400 }, // too many cells
    { base: 0, total: 64 * 1024 + 1, data: Buffer.alloc(64 * 1024 + 1).toString("base64"), alive: true, reset: false },
    // §4.5: seeds arrive through the part protocol, capped at 16 MiB.
    { base: 0, total: 0, data: Buffer.alloc(16 * 1024 * 1024 + 1).toString("base64"), alive: true, reset: true, rows: 24, cols: 80 },
  ];
  for (const value of bad) assert.equal(parseRelayRpcTerminalStreamPayload(value), null, JSON.stringify(value)?.slice(0, 120));
  assert.ok(parseRelayRpcTerminalStreamPayload({ base: 0, total: 64 * 1024, data: Buffer.alloc(64 * 1024).toString("base64"), alive: true, reset: false }));
  assert.ok(parseRelayRpcTerminalStreamPayload({ base: 0, total: 0, data: Buffer.alloc(16 * 1024 * 1024).toString("base64"), alive: true, reset: true, rows: 24, cols: 80 }));
});
