import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CompanionTerminalEngine,
  decideCompanionRead,
  decideCompanionScreen,
  isUsableCompanionInput,
  isUsableCompanionResize,
  type CompanionTerminalRpc,
} from "../src/terminal/companion/engine";
import { createCompanionHandleRegistry } from "../src/terminal/companion/registry";
import { COMPANION_TERMINAL_MESSAGES } from "../src/terminal/companion/types";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../src/transport/relayRpcMethods";
import { RelayRpcError } from "../src/transport/relayRpcClient";
import type { TerminalDelta } from "../src/transport/protocol";

const H1 = "11111111-1111-4111-8111-111111111111";
const H2 = "22222222-2222-4222-8222-222222222222";
const H3 = "33333333-3333-4333-8333-333333333333";
const b64 = (text: string): string => Buffer.from(text, "utf8").toString("base64");

function gridDelta(): Record<string, unknown> {
  return {
    kind: "alt_screen",
    rows: 2,
    cols: 10,
    cursor_row: 0,
    cursor_col: 0,
    cursor_visible: true,
    lines: [{ spans: [{ text: "hi" }] }],
  };
}

interface FakeState {
  reads: Array<{ handle: string; offset: number }>;
  screens: string[];
  detached: string[];
  attached: number[];
  resets: number;
  received: TerminalDelta[];
  readImpl?: (handle: string, offset: number) => Promise<any>;
  screenImpl?: (handle: string) => Promise<any>;
  attachImpl?: (tabId: number) => Promise<any>;
  inputImpl?: (handle: string, text: string) => Promise<any>;
  resizeImpl?: (handle: string, rows: number, cols: number) => Promise<any>;
}

function fakeRpc(state: FakeState): CompanionTerminalRpc {
  return {
    attach: async (tabId: number) => {
      state.attached.push(tabId);
      if (state.attachImpl) return state.attachImpl(tabId);
      return { handle: H1, cols: 80, rows: 24 };
    },
    read: async (handle: string, offset: number) => {
      state.reads.push({ handle, offset });
      if (state.readImpl) return state.readImpl(handle, offset);
      return { base: offset, total: offset, data: "", alive: true, exitCode: null };
    },
    screen: async (handle: string) => {
      state.screens.push(handle);
      if (state.screenImpl) return state.screenImpl(handle);
      return { complete: true, total: 0, delta: gridDelta() } as never;
    },
    input: async (handle: string, text: string) => {
      if (state.inputImpl) return state.inputImpl(handle, text);
      return { sent: true };
    },
    resize: async (handle: string, rows: number, cols: number) => {
      if (state.resizeImpl) return state.resizeImpl(handle, rows, cols);
      return { resized: true };
    },
    detach: async (handle: string) => {
      state.detached.push(handle);
      return { detached: true };
    },
  };
}

function harness(overrides: Partial<FakeState> = {}) {
  const state: FakeState = {
    reads: [],
    screens: [],
    detached: [],
    attached: [],
    resets: 0,
    received: [],
    ...overrides,
  };
  const rpc = fakeRpc(state);
  const sink = {
    receive: (delta: TerminalDelta) => {
      state.received.push(delta);
    },
    reset: () => {
      state.resets += 1;
    },
  };
  const registry = createCompanionHandleRegistry();
  const owner = { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 };
  const engine = new CompanionTerminalEngine(owner, rpc, sink, registry);
  return { state, rpc, sink, registry, owner, engine };
}

test("read sequence advances by base+decoded only, sequential and exact", async () => {
  const { engine, state } = harness({
    readImpl: async (_h, offset) => ({ base: offset, total: offset + 2, data: b64("hi"), alive: true, exitCode: null }),
  });
  await engine.attach();
  assert.equal(engine.getStatus().phase, "live-raw");
  await engine.pollOnce();
  assert.equal(engine.getStatus().offset, 2);
  assert.equal(state.received.length, 1);
  await engine.pollOnce();
  assert.deepEqual(state.reads[1], { handle: H1, offset: 2 });
  assert.equal(engine.getStatus().offset, 4);
  assert.equal(state.received.length, 2);
});

test("gap switches to snapshot grid mode and never mixes raw VT", async () => {
  const { engine, state } = harness({
    readImpl: async (_h, offset) => {
      if (offset === 0) return { base: 0, total: 2, data: b64("hi"), alive: true, exitCode: null };
      return { base: 99, total: 101, data: b64("xx"), alive: true, exitCode: null };
    },
  });
  await engine.attach();
  await engine.pollOnce();
  assert.equal(engine.getStatus().mode, "raw");
  await engine.pollOnce();
  assert.equal(engine.getStatus().mode, "snapshot");
  assert.equal(engine.getStatus().phase, "live-snapshot");
  const rawCount = state.received.filter((d) => d.kind === "output_chunk").length;
  await engine.pollOnce();
  // Snapshot poll feeds only the authoritative grid.
  assert.ok(state.screens.length >= 1);
  assert.ok(state.received.some((d) => d.kind === "alt_screen"));
  assert.equal(state.received.filter((d) => d.kind === "output_chunk").length, rawCount);
});

test("malformed payload fails closed without reattach or offset advance", async () => {
  const unreadable = new RelayRpcError(RELAY_RPC_UNREADABLE_MESSAGE);
  const { engine, state } = harness({
    readImpl: async () => {
      throw unreadable;
    },
  });
  await engine.attach();
  await engine.pollOnce();
  assert.equal(engine.getStatus().phase, "error");
  assert.equal(engine.getStatus().canWrite, false);
  assert.equal(engine.getStatus().offset, 0);
  assert.equal(state.received.length, 0);
  // No bounded reattach for malformed: no new attach, no reset.
  assert.equal(state.attached.length, 1);
  assert.equal(state.resets, 1);
});

test("input/resize gates report not-sent before wire, unconfirmed after wire loss", async () => {
  const { engine, state } = harness();
  // Pre-wire gate: not attached yet after fresh engine without attach.
  const gated = await engine.sendInput("hi");
  assert.equal(gated.ok, false);
  // Attach then test bounds without touching the wire.
  await engine.attach();
  let inputCalls = 0;
  state.inputImpl = async () => {
    inputCalls += 1;
    throw new RelayRpcError("The request failed.");
  };
  const empty = await engine.sendInput("");
  assert.equal(empty.ok, false);
  assert.equal(empty.message, COMPANION_TERMINAL_MESSAGES.sendNotReady);
  assert.equal(inputCalls, 0);
  const oversize = await engine.sendInput("x".repeat(20 * 1024));
  assert.equal(oversize.ok, false);
  assert.equal(inputCalls, 0);
  // Post-wire loss: delivery unknown, never claim unsent, never replay.
  const lost = await engine.sendInput("ls");
  assert.equal(lost.ok, false);
  assert.equal(lost.message, COMPANION_TERMINAL_MESSAGES.sendUnconfirmed);
  assert.equal(inputCalls, 1);

  let resizeCalls = 0;
  state.resizeImpl = async () => {
    resizeCalls += 1;
    throw new RelayRpcError("The request failed.");
  };
  const badResize = await engine.sendResize(0, 80);
  assert.equal(badResize.ok, false);
  assert.equal(badResize.message, COMPANION_TERMINAL_MESSAGES.resizeNotReady);
  assert.equal(resizeCalls, 0);
  const lostResize = await engine.sendResize(24, 80);
  assert.equal(lostResize.ok, false);
  assert.equal(lostResize.message, COMPANION_TERMINAL_MESSAGES.resizeUnconfirmed);
  assert.equal(resizeCalls, 1);

  assert.equal(isUsableCompanionInput("ok"), true);
  assert.equal(isUsableCompanionInput(""), false);
  assert.equal(isUsableCompanionResize(24, 80), true);
  assert.equal(isUsableCompanionResize(0, 80), false);
  assert.equal(isUsableCompanionResize(240, 512), false);
});

test("a desktop that took the terminal back ends the session without retrying", async () => {
  const taken = () => new RelayRpcError("Your desktop is using this terminal.", false, "desktop_in_control");
  // A refused resize: a definite refusal, and the screen is told to leave.
  const { engine, state } = harness();
  await engine.attach();
  state.resizeImpl = async () => {
    throw taken();
  };
  const refused = await engine.sendResize(24, 40);
  assert.deepEqual(refused, {
    ok: false,
    message: COMPANION_TERMINAL_MESSAGES.desktopInControl,
    desktopInControl: true,
  });
  const status = engine.getStatus();
  assert.equal(status.takenByDesktop, true);
  assert.equal(status.phase, "detached");
  assert.equal(status.canWrite, false);
  // Never reattached: that would take the terminal straight back.
  assert.equal(state.attached.length, 1);
});

test("detach releases transport only; late attach after unmount detaches", async () => {
  const { engine, state, registry } = harness();
  await engine.attach();
  assert.equal(registry.has(H1), true);
  await engine.detach();
  assert.deepEqual(state.detached, [H1]);
  assert.equal(registry.has(H1), false);
  assert.equal(engine.getStatus().phase, "detached");

  // Late attach after dispose detaches without keeping the handle.
  const late: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  let resolveAttach!: (value: { handle: string; cols: number; rows: number }) => void;
  const pending = new Promise<{ handle: string; cols: number; rows: number }>((resolve) => {
    resolveAttach = resolve;
  });
  const lateRpc = fakeRpc(late);
  const lateEngine = new (engine.constructor as typeof CompanionTerminalEngine)(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    { ...lateRpc, attach: () => pending },
    { receive: () => {}, reset: () => {} },
    createCompanionHandleRegistry(),
  );
  const attachPromise = lateEngine.attach();
  lateEngine.dispose();
  resolveAttach({ handle: H3, cols: 80, rows: 24 });
  await attachPromise;
  assert.deepEqual(late.detached, [H3]);
});

test("registry bound 4 never steals others; stale account masked", async () => {
  const registry = createCompanionHandleRegistry(4);
  assert.equal(registry.tryAdd(H1), true);
  assert.equal(registry.tryAdd(H2), true);
  assert.equal(registry.tryAdd(H3), true);
  assert.equal(registry.tryAdd("44444444-4444-4444-4444-444444444444"), true);
  assert.equal(registry.tryAdd("55555555-5555-5555-5555-555555555555"), false);
  assert.equal(registry.has(H1), true);

  const { engine, owner } = harness();
  assert.equal(engine.isCurrent(owner), true);
  assert.equal(engine.isCurrent({ ...owner, accountId: "a2" }), false);
  assert.equal(engine.isCurrent({ ...owner, deviceId: "d2" }), false);
  assert.equal(engine.isCurrent({ ...owner, tabId: 8 }), false);
  assert.equal(engine.isCurrent({ ...owner, epoch: 2 }), false);
  engine.dispose();
  assert.equal(engine.isCurrent(owner), false);
});

test("host generation recovery: stale handle reattaches bounded, resets epoch, no replay", async () => {
  const state: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  let reads = 0;
  const rpc = fakeRpc(state);
  rpc.read = async (handle: string, offset: number) => {
    state.reads.push({ handle, offset });
    reads += 1;
    if (reads === 1) throw new RelayRpcError("The request failed.");
    return { base: 0, total: 0, data: "", alive: true, exitCode: null };
  };
  let attaches = 0;
  rpc.attach = async (tabId: number) => {
    state.attached.push(tabId);
    attaches += 1;
    return attaches === 1 ? { handle: H1, cols: 80, rows: 24 } : { handle: H2, cols: 80, rows: 24 };
  };
  const resets: number[] = [];
  const registry = createCompanionHandleRegistry();
  const owner = { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 };
  const engine = new CompanionTerminalEngine(
    owner,
    rpc,
    {
      receive: (d) => state.received.push(d),
      reset: () => {
        state.resets += 1;
        resets.push(Date.now());
      },
    },
    registry,
  );
  await engine.attach();
  assert.equal(engine.getStatus().handle, H1);
  const resetsAfterAttach = state.resets;
  await engine.pollOnce();
  // Bounded reattach happened: old handle detached best-effort, epoch reset, new handle live.
  assert.equal(engine.getStatus().handle, H2);
  assert.equal(engine.getStatus().phase, "live-raw");
  assert.equal(engine.getStatus().offset, 0);
  assert.ok(state.detached.includes(H1));
  assert.ok(state.resets > resetsAfterAttach);
  assert.equal(registry.has(H1), false);
  assert.equal(registry.has(H2), true);
});

test("loginRequired stops without reattach; unknown errors bound to 3 attempts", async () => {
  const loginState: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  const loginRpc = fakeRpc(loginState);
  loginRpc.read = async () => {
    throw new RelayRpcError("Sign in", true);
  };
  const loginEngine = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    loginRpc,
    { receive: () => {}, reset: () => {} },
    createCompanionHandleRegistry(),
  );
  await loginEngine.attach();
  await loginEngine.pollOnce();
  assert.equal(loginEngine.getStatus().phase, "unavailable");
  assert.equal(loginState.attached.length, 1);

  const failState: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  const failRpc = fakeRpc(failState);
  failRpc.read = async () => {
    throw new RelayRpcError("The request failed.");
  };
  let failAttaches = 0;
  const baseAttach = failRpc.attach;
  failRpc.attach = async (tabId: number) => {
    failAttaches += 1;
    if (failAttaches === 1) return baseAttach(tabId);
    failState.attached.push(tabId);
    throw new RelayRpcError("The request failed.");
  };
  const failEngine = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    failRpc,
    { receive: () => {}, reset: () => {} },
    createCompanionHandleRegistry(),
  );
  await failEngine.attach();
  const initialAttaches = failState.attached.length;
  for (let i = 0; i < 5; i += 1) await failEngine.pollOnce();
  // Bounded: initial + at most 3 reattach attempts, then error for manual retry.
  assert.ok(failState.attached.length <= initialAttaches + 3);
  assert.equal(failEngine.getStatus().phase, "error");
});

test("pure decisions use base+decoded and explicit resync", () => {
  const data = b64("ab");
  const ok = decideCompanionRead(10, { base: 10, total: 999, data, alive: true, exitCode: null });
  assert.ok(ok.kind === "data" && ok.nextOffset === 12 && ok.decoded === 2);
  assert.equal(decideCompanionRead(10, { base: 11, total: 13, data, alive: true, exitCode: null }).kind, "gap");
  assert.equal(decideCompanionScreen({ complete: false, total: 5, delta: null }).kind, "resync");
  assert.equal(decideCompanionScreen({ complete: true, total: 5, delta: gridDelta() as never }).kind, "grid");
});

test("attach uses host dimensions and active alternate screen", async () => {
  const { engine, sink, state } = harness({ attachImpl: async () => ({ handle: H1, rows: 40, cols: 120, alternateScreen: true }) });
  const sizes: number[][] = [];
  Object.assign(sink, { setViewport: (rows: number, cols: number) => sizes.push([rows, cols]) });
  await engine.attach();
  assert.deepEqual(sizes, [[40, 120]]);
  assert.equal(engine.getStatus().mode, "snapshot");
  // Alt-screen attach shows the current screen at once, not on next poll.
  assert.ok(state.screens.length >= 1);
  assert.ok(state.received.some((d) => d.kind === "alt_screen"));
  assert.equal(state.reads.length, 0);
  const screensAfterAttach = state.screens.length;
  await engine.pollOnce();
  assert.equal(state.screens.length, screensAfterAttach + 1);
  assert.equal(state.reads.length, 0);
});

test("screen modes track every screen result and forward to the sink", async () => {
  const s3: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  const r3 = fakeRpc(s3);
  r3.attach = async () => ({ handle: H1, rows: 24, cols: 80, alternateScreen: true, mouseReporting: false, sgrMouse: false, bracketedPaste: false }) as never;
  let screenModes: Record<string, unknown> = { complete: true, total: 1, delta: gridDelta(), alternateScreen: true, mouseReporting: true, sgrMouse: true, bracketedPaste: false };
  r3.screen = async (h: string) => { s3.screens.push(h); return screenModes as never; };
  const modesForwarded: unknown[] = [];
  const eng3 = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    r3,
    { receive: (d) => s3.received.push(d), reset: () => { s3.resets += 1; }, setScreenModes: (m) => modesForwarded.push(m) },
    createCompanionHandleRegistry(),
  );
  await eng3.attach();
  assert.equal(eng3.getScreenModes().alternateScreen, true);
  // Attach already fetched once; latest screen result updates all modes.
  screenModes = { complete: true, total: 1, delta: gridDelta(), alternateScreen: true, mouseReporting: true, sgrMouse: true, bracketedPaste: true };
  await eng3.pollOnce();
  assert.deepEqual(eng3.getScreenModes(), { alternateScreen: true, mouseReporting: true, sgrMouse: true, bracketedPaste: true });
  assert.ok(modesForwarded.length >= 2);
  assert.deepEqual(modesForwarded.at(-1), { alternateScreen: true, mouseReporting: true, sgrMouse: true, bracketedPaste: true });
});

test("snapshot mode leaving alt-screen stays snapshot with alternateScreen:false", async () => {
  const s: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  const r = fakeRpc(s);
  r.attach = async () => ({ handle: H1, rows: 24, cols: 80, alternateScreen: true }) as never;
  // Program leaves alt-screen: host reports alternateScreen:false but still a grid.
  r.screen = async (h: string) => {
    s.screens.push(h);
    return { complete: true, total: 2, delta: gridDelta(), alternateScreen: false, mouseReporting: false, sgrMouse: false, bracketedPaste: false } as never;
  };
  const eng = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    r,
    { receive: (d) => s.received.push(d), reset: () => { s.resets += 1; } },
    createCompanionHandleRegistry(),
  );
  await eng.attach();
  assert.equal(eng.getStatus().mode, "snapshot");
  await eng.pollOnce();
  assert.equal(eng.getScreenModes().alternateScreen, false);
  // Must not resume raw parsing: stays snapshot.
  assert.equal(eng.getStatus().mode, "snapshot");
  assert.equal(eng.getStatus().phase, "live-snapshot");
  assert.equal(s.reads.length, 0);
});

test("refreshScreenModes returns fresh modes and rejects without changing old values", async () => {
  const s: FakeState = { reads: [], screens: [], detached: [], attached: [], resets: 0, received: [] };
  const r = fakeRpc(s);
  r.attach = async () => ({ handle: H1, rows: 24, cols: 80, alternateScreen: false }) as never;
  r.screen = async (h: string) => {
    s.screens.push(h);
    return { complete: true, total: 1, delta: gridDelta(), alternateScreen: true, mouseReporting: true, sgrMouse: false, bracketedPaste: true } as never;
  };
  const forwarded: unknown[] = [];
  const eng = new CompanionTerminalEngine(
    { accountId: "a1", deviceId: "d1", tabId: 7, client: {}, epoch: 1 },
    r,
    { receive: (d) => s.received.push(d), reset: () => {}, setScreenModes: (m) => forwarded.push(m) },
    createCompanionHandleRegistry(),
  );
  await eng.attach();
  assert.deepEqual(eng.getScreenModes(), { alternateScreen: false, mouseReporting: false, sgrMouse: false, bracketedPaste: false });
  const fresh = await eng.refreshScreenModes();
  assert.deepEqual(fresh, { alternateScreen: true, mouseReporting: true, sgrMouse: false, bracketedPaste: true });
  assert.deepEqual(eng.getScreenModes(), fresh);
  assert.deepEqual(forwarded.at(-1), fresh);
  // Failure rejects and leaves old values.
  r.screen = async () => { throw new RelayRpcError("The request failed."); };
  await assert.rejects(eng.refreshScreenModes());
  assert.deepEqual(eng.getScreenModes(), fresh);
});

test("input waits for acknowledgement before sending the next keystroke", async () => {
  let release!: () => void;
  const first = new Promise<void>((resolve) => { release = resolve; });
  const writes: string[] = [];
  const { engine } = harness({ inputImpl: async (_handle, text) => {
    writes.push(text);
    if (text === "hello") await first;
    return { sent: true };
  } });
  await engine.attach();
  const a = engine.sendInput("hello");
  const b = engine.sendInput("\r");
  await Promise.resolve();
  assert.deepEqual(writes, ["hello"]);
  release();
  assert.equal((await a).ok, true);
  assert.equal((await b).ok, true);
  assert.deepEqual(writes, ["hello", "\r"]);
});
