import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { CompanionTerminalEngine, type CompanionTerminalRpc } from "../src/terminal/companion/engine";
import { createCompanionHandleRegistry } from "../src/terminal/companion/registry";
import {
  DRAFT_EMPTY_MESSAGE,
  DRAFT_TOO_LARGE_MESSAGE,
  SUBMIT_CR_DELAY_MS,
  draftAfterDelivery,
  draftRefusal,
  encodeDraft,
  normalizeDraft,
  sendDraftOnce,
  type DeliveryResult,
} from "../src/terminal/companion/submitDraft";
import { COMPANION_TERMINAL_MESSAGES as MESSAGES } from "../src/terminal/companion/types";
import { COMPANION_TERMINAL_LIMITS } from "../src/transport/relayRpcMethods";

const H1 = "11111111-1111-4111-8111-111111111111";
const H2 = "22222222-2222-4222-8222-222222222222";
const ESC = "\u001b";
const ON = { bracketedPaste: true };
const OFF = { bracketedPaste: false };

function owner() {
  return { accountId: "a", deviceId: "d", tabId: 7, client: "c", epoch: 1 };
}

/** The gap is a promise the test releases, and the event log shows the order. */
function rig(opts: { bracketedPaste?: boolean; refresh?: () => Promise<any>; input?: (text: string) => Promise<any> } = {}) {
  const log: string[] = [];
  const writes: Array<{ handle: string; text: string }> = [];
  const delays: number[] = [];
  let openGap!: () => void;
  let gapStarted!: () => void;
  const gapReached = new Promise<void>((resolve) => { gapStarted = resolve; });
  const state = { screen: opts.refresh, input: opts.input, nextHandle: H2 };
  const rpc: CompanionTerminalRpc = {
    attach: async () => ({ handle: H1, cols: 80, rows: 24 }) as never,
    read: async (_h, offset) => ({ base: offset, total: offset, data: "", alive: true, exitCode: null }) as never,
    screen: async () => {
      log.push("refresh");
      if (state.screen) return state.screen();
      return { complete: false, total: 0, delta: null, bracketedPaste: opts.bracketedPaste ?? true } as never;
    },
    input: async (handle, text) => {
      log.push(`write:${JSON.stringify(text)}`);
      writes.push({ handle, text });
      if (state.input) return state.input(text);
      return { sent: true } as const;
    },
    resize: async () => ({ resized: true }) as const,
    detach: async () => ({ detached: true }) as const,
  };
  const engine = new CompanionTerminalEngine(
    owner() as never,
    rpc,
    { receive() {}, reset() {} },
    createCompanionHandleRegistry(),
    (ms) => {
      delays.push(ms);
      log.push(`delay:${ms}`);
      gapStarted();
      return new Promise<void>((resolve) => { openGap = resolve; });
    },
  );
  return { engine, rpc, state, log, writes, delays, gapReached, openGap: () => openGap() };
}

// ---- Encoding ------------------------------------------------------------

test("line endings and outer whitespace match the desktop", () => {
  assert.equal(normalizeDraft("  a\r\nb\rc\nd\n\n"), "a\rb\rc\rd");
});

test("exact bytes with bracketed paste on", () => {
  assert.equal(encodeDraft("hello\nworld", ON), `${ESC}[200~hello\rworld${ESC}[201~`);
});

test("exact bytes with bracketed paste off: plain text and no Enter of its own", () => {
  assert.equal(encodeDraft("hello", OFF), "hello");
});

test("multi-line with bracketed paste off uses the desktop's line endings, never LF", () => {
  const encoded = encodeDraft("a\nb\r\nc", OFF);
  assert.equal(encoded, "a\rb\rc");
  assert.ok(!encoded.includes("\n"));
});

test("a draft cannot end the paste early", () => {
  for (const draft of [`x${ESC}[201~y`, `x${ESC}${ESC}[201~y`, `x\u009b201~y`]) {
    const encoded = encodeDraft(draft, ON);
    const inner = encoded.slice(`${ESC}[200~`.length, -`${ESC}[201~`.length);
    assert.ok(!inner.includes(ESC), JSON.stringify(encoded));
    assert.ok(!inner.includes("\u009b"), JSON.stringify(encoded));
    assert.equal(encoded.indexOf(`${ESC}[201~`), encoded.length - `${ESC}[201~`.length);
  }
  assert.equal(encodeDraft(`x${ESC}[201~y`, ON), `${ESC}[200~x[201~y${ESC}[201~`);
});

test("refusals: blank, escapes only, and over the input limit once wrapped", () => {
  assert.equal(draftRefusal(" \n "), DRAFT_EMPTY_MESSAGE);
  assert.equal(draftRefusal(`${ESC}${ESC}`), DRAFT_EMPTY_MESSAGE);
  const limit = COMPANION_TERMINAL_LIMITS.inputUtf8Bytes;
  assert.equal(draftRefusal("x".repeat(limit - 12)), null);
  assert.equal(draftRefusal("x".repeat(limit - 11)), DRAFT_TOO_LARGE_MESSAGE);
});

// ---- Engine: order and gap -----------------------------------------------

test("submit refreshes modes, writes the paste, waits 160 ms, then a lone CR", async () => {
  const r = rig({ bracketedPaste: true });
  await r.engine.attach();
  const done = r.engine.submitDraft("fix it");
  await r.gapReached;
  assert.deepEqual(r.writes.map((w) => w.text), [`${ESC}[200~fix it${ESC}[201~`]);
  r.openGap();
  assert.deepEqual(await done, { ok: true });
  assert.deepEqual(r.log, [
    "refresh",
    `write:${JSON.stringify(`${ESC}[200~fix it${ESC}[201~`)}`,
    "delay:160",
    `write:${JSON.stringify("\r")}`,
  ]);
  assert.equal(SUBMIT_CR_DELAY_MS, 160);
});

test("fresh modes decide the encoding: bracketed paste off sends plain text", async () => {
  const r = rig({ bracketedPaste: false });
  await r.engine.attach();
  const done = r.engine.submitDraft("ls");
  await r.gapReached;
  r.openGap();
  assert.equal((await done).ok, true);
  assert.deepEqual(r.writes.map((w) => w.text), ["ls", "\r"]);
});

test("a failed refresh falls back to the last known modes", async () => {
  const r = rig({ bracketedPaste: true });
  await r.engine.attach();
  await r.engine.refreshScreenModes();
  r.state.screen = async () => { throw new Error("lost"); };
  const done = r.engine.submitDraft("hi");
  await r.gapReached;
  r.openGap();
  assert.equal((await done).ok, true);
  assert.deepEqual(r.writes.map((w) => w.text), [`${ESC}[200~hi${ESC}[201~`, "\r"]);
});

// ---- Engine: failure and session change ----------------------------------

test("a failed text write never sends the CR and reports unconfirmed", async () => {
  const r = rig({ input: async () => { throw new Error("lost"); } });
  await r.engine.attach();
  const result = await r.engine.submitDraft("hi");
  assert.deepEqual(result, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.equal(r.writes.length, 1);
  assert.deepEqual(r.delays, []);
});

test("an unconfirmed CR fails with the unconfirmed message and is not resent", async () => {
  const r = rig({ input: async (text) => {
    if (text === "\r") throw new Error("lost");
    return { sent: true };
  } });
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  await r.gapReached;
  r.openGap();
  assert.deepEqual(await done, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.equal(r.writes.filter((w) => w.text === "\r").length, 1);
});

test("a new handle during the gap means no CR anywhere", async () => {
  const r = rig();
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  await r.gapReached;
  // A failing poll replaces the handle the way a host restart does.
  r.rpc.attach = async () => ({ handle: H2, cols: 80, rows: 24 }) as never;
  r.rpc.read = async () => { throw new Error("stale handle"); };
  await r.engine.pollOnce();
  assert.equal(r.engine.getStatus().handle, H2);
  r.openGap();
  assert.deepEqual(await done, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.equal(r.writes.length, 1);
  assert.equal(r.writes[0]!.handle, H1);
});

test("a detach during the gap means no CR", async () => {
  const r = rig();
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  await r.gapReached;
  await r.engine.detach();
  r.openGap();
  assert.deepEqual(await done, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.equal(r.writes.length, 1);
});

test("a teardown while the text is on the wire is unconfirmed, not 'not sent'", async () => {
  let ack!: () => void;
  const r = rig({ input: () => new Promise((resolve) => { ack = () => resolve({ sent: true }); }) });
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  while (r.writes.length === 0) await new Promise((resolve) => setImmediate(resolve));
  await r.engine.detach();
  ack();
  assert.deepEqual(await done, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.equal(r.writes.length, 1);
});

test("a session change during the refresh writes nothing and truthfully says not sent", async () => {
  let release!: () => void;
  const r = rig({ refresh: () => new Promise((_, reject) => { release = () => reject(new Error("lost")); }) });
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  while (!r.log.includes("refresh")) await new Promise((resolve) => setImmediate(resolve));
  await r.engine.detach();
  release();
  assert.deepEqual(await done, { ok: false, message: MESSAGES.sendNotReady });
  assert.equal(r.writes.length, 0);
});

test("an oversize or blank draft is refused without touching the wire", async () => {
  const r = rig();
  await r.engine.attach();
  const big = await r.engine.submitDraft("x".repeat(COMPANION_TERMINAL_LIMITS.inputUtf8Bytes));
  assert.deepEqual(big, { ok: false, message: DRAFT_TOO_LARGE_MESSAGE });
  assert.deepEqual(await r.engine.submitDraft("  "), { ok: false, message: DRAFT_EMPTY_MESSAGE });
  assert.deepEqual(r.log, []);
});

test("submit before attach reports not sent", async () => {
  const r = rig();
  assert.deepEqual(await r.engine.submitDraft("hi"), { ok: false, message: MESSAGES.sendNotReady });
  assert.deepEqual(r.log, []);
});

// ---- Engine: the queue ---------------------------------------------------

test("a key pressed mid-submit arrives after the CR", async () => {
  const r = rig();
  await r.engine.attach();
  const done = r.engine.submitDraft("hi");
  await r.gapReached;
  const key = r.engine.sendInput(`${ESC}[A`);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(r.writes.length, 1, "the key waits out the gap");
  r.openGap();
  assert.equal((await done).ok, true);
  assert.equal((await key).ok, true);
  assert.deepEqual(r.writes.map((w) => w.text), [`${ESC}[200~hi${ESC}[201~`, "\r", `${ESC}[A`]);
});

test("a key pressed before Send finishes first, then the whole submit runs", async () => {
  let ack!: () => void;
  const r = rig({ input: (text) => text === "k"
    ? new Promise((resolve) => { ack = () => resolve({ sent: true }); })
    : Promise.resolve({ sent: true }) });
  await r.engine.attach();
  const key = r.engine.sendInput("k");
  const done = r.engine.submitDraft("hi");
  while (r.writes.length === 0) await new Promise((resolve) => setImmediate(resolve));
  assert.ok(!r.log.includes("refresh"), "submit keeps its place behind the key");
  ack();
  assert.equal((await key).ok, true);
  await r.gapReached;
  r.openGap();
  assert.equal((await done).ok, true);
  assert.deepEqual(r.writes.map((w) => w.text), ["k", `${ESC}[200~hi${ESC}[201~`, "\r"]);
});

test("one lost acknowledgement does not lock typing on a live session", async () => {
  let lose = true;
  const r = rig({ input: async () => {
    if (lose) { lose = false; throw new Error("lost"); }
    return { sent: true };
  } });
  await r.engine.attach();
  assert.deepEqual(await r.engine.sendInput("a"), { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.deepEqual(await r.engine.sendInput("b"), { ok: true });
  assert.deepEqual(r.writes.map((w) => w.text), ["a", "b"]);
});

test("input already queued behind a failed write is dropped, never run out of context", async () => {
  let fail!: () => void;
  const r = rig({ input: (text) => text === "hello"
    ? new Promise((_, reject) => { fail = () => reject(new Error("lost")); })
    : Promise.resolve({ sent: true }) });
  await r.engine.attach();
  const first = r.engine.sendInput("hello");
  const behind = r.engine.sendInput("\r");
  while (r.writes.length === 0) await new Promise((resolve) => setImmediate(resolve));
  fail();
  assert.deepEqual(await first, { ok: false, message: MESSAGES.sendUnconfirmed });
  assert.deepEqual(await behind, { ok: false, message: MESSAGES.sendNotReady });
  assert.deepEqual(r.writes.map((w) => w.text), ["hello"]);
});

test("a refused draft does not drop keys queued behind it", async () => {
  const r = rig();
  await r.engine.attach();
  const refused = r.engine.submitDraft("   ");
  const key = r.engine.sendInput("k");
  assert.equal((await refused).ok, false);
  assert.equal((await key).ok, true);
  assert.deepEqual(r.writes.map((w) => w.text), ["k"]);
});

// ---- Draft field ---------------------------------------------------------

function sendRecorder() {
  const events: string[] = [];
  const results: DeliveryResult[] = [];
  return {
    events,
    results,
    ui: {
      onStart: () => { events.push("start"); },
      onDone: (result: DeliveryResult) => { events.push("done"); results.push(result); },
    },
  };
}

test("a second press while sending does nothing", async () => {
  const guard = { current: false };
  let calls = 0;
  let finish!: (result: DeliveryResult) => void;
  const submit = () => {
    calls += 1;
    return new Promise<DeliveryResult>((resolve) => { finish = resolve; });
  };
  const rec = sendRecorder();
  const first = sendDraftOnce(guard, submit, rec.ui);
  await sendDraftOnce(guard, submit, rec.ui);
  assert.equal(calls, 1);
  finish({ ok: true });
  await first;
  assert.deepEqual(rec.events, ["start", "done"]);
  assert.equal(guard.current, false, "the next press works");
});

test("a failure is handed to the field, and a throwing submit counts as unconfirmed", async () => {
  const guard = { current: false };
  const rec = sendRecorder();
  await sendDraftOnce(guard, async () => ({ ok: false, message: "nope" }), rec.ui);
  await sendDraftOnce(guard, async () => { throw new Error("boom"); }, rec.ui);
  assert.deepEqual(rec.results, [
    { ok: false, message: "nope" },
    { ok: false, message: MESSAGES.sendUnconfirmed },
  ]);
  assert.equal(guard.current, false);
});

test("delivery clears only what was sent", () => {
  assert.equal(draftAfterDelivery("hello", "hello"), "");
  assert.equal(draftAfterDelivery("hello more", "hello"), " more");
  assert.equal(draftAfterDelivery("help", "hello"), "help");
});

// No component renderer in this suite: these read the source for what would
// otherwise only show on a device.
const inputSource = readFileSync(new URL("../src/terminal/companion/CompanionInput.tsx", import.meta.url), "utf8");

test("the draft field is never made non-editable, which would close the iOS keyboard", () => {
  assert.ok(!/editable=/.test(inputSource));
});

test("Send goes through submitDraft and never appends a CR to the draft", () => {
  assert.ok(inputSource.includes("submitDraft(sent)"));
  assert.ok(!inputSource.includes('"\\r"'));
});

test("terminal keys are single raw writes, not gated by a send in flight", () => {
  assert.ok(inputSource.includes("sendInput(bytes)"));
  assert.ok(inputSource.includes("disabled={!connected} onPress={() => { void sendKey(key.bytes); }}"));
});

test("every key, Hide keyboard and Send have a 44 pt tap target", () => {
  assert.equal(inputSource.match(/size="sm"[^>]*style=\{styles\.tapTarget\}/g)?.length, 3);
  assert.equal(inputSource.match(/size="sm"/g)?.length, 3);
  assert.match(inputSource, /tapTarget: \{ minHeight: 44 \}/);
});

test("the multiline field grows from one row to a bound, then scrolls", () => {
  assert.match(inputSource, /input: \{ minHeight: 44, maxHeight: 120 \}/);
  assert.ok(!/\bheight:/.test(inputSource), "a fixed height would stop it growing");
  assert.ok(!/scrollEnabled/.test(inputSource), "scrolling past the bound stays on");
});
