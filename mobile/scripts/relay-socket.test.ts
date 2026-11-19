import assert from "node:assert/strict";
import { test } from "node:test";
import { RelaySocketClient, resolveRelaySocketURL, RELAY_SOCKET_TIMING } from "../src/transport/RelaySocketClient";
import { RelayRpcClient, RelayRpcError, RELAY_RPC_LIMITS } from "../src/transport/relayRpcClient";
import { listTabs } from "../src/transport/relayRpcMethods";
import type { RealtimeClock } from "../src/transport/clock";
import type { RealtimeSocket } from "../src/transport/socket";
class Clock implements RealtimeClock {
  time = 0;
  sequence = 0;
  timers = new Map<number, {
    at: number;
    fn: () => void;
  }>();
  now = () => this.time;
  setTimeout = (fn: () => void, ms: number) => { const id = ++this.sequence; this.timers.set(id, { at: this.time + ms, fn }); return id; };
  clearTimeout = (id: unknown) => { this.timers.delete(id as number); };
  advance(ms: number) {
    const end = this.time + ms;
    for (;;) {
      const first = [...this.timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
      if (!first || first[1].at > end)
        break;
      this.time = first[1].at;
      this.timers.delete(first[0]);
      first[1].fn();
    }
    this.time = end;
  }
}
class Socket implements RealtimeSocket {
  readyState = 0;
  onopen: RealtimeSocket["onopen"] = null;
  onmessage: RealtimeSocket["onmessage"] = null;
  onerror: RealtimeSocket["onerror"] = null;
  onclose: RealtimeSocket["onclose"] = null;
  sent: Record<string, unknown>[] = [];
  send(text: string) { this.sent.push(JSON.parse(text)); }
  close() { this.readyState = 3; }
  open() { this.readyState = 1; this.onopen?.({}); }
  receive(frame: unknown) { this.onmessage?.({ data: JSON.stringify(frame) }); }
  disconnect(code = 1006) { this.readyState = 3; this.onclose?.({ code }); }
}
async function flush() { for (let i = 0; i < 8; i++)
  await Promise.resolve(); }
function harness() {
  const clock = new Clock();
  const sockets: Socket[] = [];
  const urls: string[] = [];
  const options: unknown[] = [];
  let ids = 0, tokens = 0;
  const client = new RelaySocketClient({ gateway: "wss://relay.example/realtime", deviceId: "desktop-1", clock,
    resolveAccessToken: async () => `account-token-${++tokens}`,
    createSocket: (url, init) => { urls.push(url); options.push(init); const socket = new Socket(); sockets.push(socket); return socket; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`, random: () => 0.5,
  });
  const admit = async () => { client.connect(); await flush(); const socket = sockets.at(-1)!; socket.open(); socket.receive({ type: "relay-ready", v: 1 }); return socket; };
  return { clock, sockets, urls, options, client, admit, tokenCount: () => tokens };
}
test("relay URL rejects credential carriers and requires encrypted remote transport", () => {
  assert.equal(resolveRelaySocketURL("wss://relay.example/realtime"), "wss://relay.example/v1/relay/socket");
  assert.equal(resolveRelaySocketURL("https://relay.example/anything"), "wss://relay.example/v1/relay/socket");
  assert.equal(resolveRelaySocketURL("ws://localhost:8080/realtime"), "ws://localhost:8080/v1/relay/socket");
  for (const value of ["bad", "wss://user:secret@relay.example", "wss://relay.example?access_token=secret", "wss://relay.example/#secret", "ws://remote.example/realtime", "ftp://relay.example"])
    assert.equal(resolveRelaySocketURL(value), null);
});
test("RPC cannot send before authenticated ready; auth uses first frame with no headers or query secrets", async () => {
  const h = harness();
  h.client.connect();
  await assert.rejects(listTabs(h.client.rpc), /Connection lost/);
  await flush();
  const socket = h.sockets[0]!;
  assert.equal(socket.sent.length, 0);
  socket.open();
  assert.deepEqual([...socket.sent], [{ type: "relay-auth", v: 1, access_token: "account-token-1", e2e: 1, device_id: "desktop-1" }]);
  assert.deepEqual(h.options, [{}]);
  assert.equal(h.urls[0], "wss://relay.example/v1/relay/socket");
  await assert.rejects(listTabs(h.client.rpc), /Connection lost/);
  socket.receive({ type: "relay-ready", v: 1 });
  const request = h.client.rpc.request("machines.list", { device_id: "attacker-selected-device" });
  const sent = socket.sent.at(-1)!;
  assert.deepEqual(sent.params, { device_id: "desktop-1" });
  socket.receive({ type: "rpc-reply", v: 1, id: sent.id, ok: true, result: { machines: [] } });
  assert.deepEqual(await request, { machines: [] });
  assert.doesNotMatch(JSON.stringify(h.client.getSnapshot()), /account-token/);
  h.client.close();
  assert.equal(h.clock.timers.size, 0);
});
test("disconnect rejects pending mutations promptly without replay after fresh token admission", async () => {
  const h = harness(), first = await h.admit();
  const request = h.client.rpc.request("machines.list", {});
  const rejected = assert.rejects(request, /Connection lost/);
  first.disconnect();
  await rejected;
  h.clock.advance(RELAY_SOCKET_TIMING.firstRetryMs);
  await flush();
  const next = h.sockets[1]!;
  next.open();
  assert.equal(next.sent[0]?.access_token, "account-token-2");
  next.receive({ type: "relay-ready", v: 1 });
  assert.equal(next.sent.filter((frame) => frame.type === "rpc-request").length, 0);
  assert.equal(h.client.getSnapshot().connectionEpoch, 2);
  h.client.close();
});
test("foreground pings a socket that looks ready and redials when the relay doesn't answer", async () => {
  const h = harness(), first = await h.admit();
  h.clock.advance(10_000);
  h.client.notifyForeground();
  assert.deepEqual(first.sent.at(-1), { type: "relay-ping", v: 1 });
  assert.equal(h.sockets.length, 1);
  // iOS killed it while suspended: nothing comes back.
  h.clock.advance(RELAY_SOCKET_TIMING.foregroundProbeMs);
  await flush();
  assert.equal(h.sockets.length, 2);
  h.client.close();
});
test("after a real absence foreground replaces the connection at once", async () => {
  const h = harness();
  await h.admit();
  h.client.notifyForeground(RELAY_SOCKET_TIMING.foregroundRedialAfterMs);
  await flush();
  assert.equal(h.sockets.length, 2);
  h.client.close();
});
test("one return reported by every screen sharing the socket redials once", async () => {
  const h = harness();
  await h.admit();
  h.client.notifyForeground(RELAY_SOCKET_TIMING.foregroundRedialAfterMs);
  h.client.notifyForeground(RELAY_SOCKET_TIMING.foregroundRedialAfterMs);
  h.client.notifyForeground(RELAY_SOCKET_TIMING.foregroundRedialAfterMs);
  await flush();
  assert.equal(h.sockets.length, 2);
  h.client.close();
});
test("foreground keeps a socket that answers the ping", async () => {
  const h = harness(), first = await h.admit();
  h.clock.advance(10_000);
  h.client.notifyForeground();
  h.clock.advance(200);
  first.receive({ type: "relay-pong", v: 1 });
  h.clock.advance(RELAY_SOCKET_TIMING.foregroundProbeMs);
  await flush();
  assert.equal(h.sockets.length, 1);
  assert.equal(h.client.getSnapshot().status, "ready");
  h.client.close();
});
test("network handoff redials a healthy connection and rejects late packets from the old socket", async () => {
  const h = harness(), first = await h.admit();
  const oldMessage = first.onmessage!;
  h.client.notifyNetworkChange();
  await flush();
  const next = h.sockets[1]!;
  next.open();
  oldMessage({ data: JSON.stringify({ type: "relay-ready", v: 1 }) });
  assert.equal(h.client.getSnapshot().status, "connecting");
  oldMessage({ data: JSON.stringify({ type: "relay-error", v: 1, loginRequired: true }) });
  next.receive({ type: "relay-ready", v: 1 });
  assert.equal(h.client.getSnapshot().status, "ready");
  assert.equal(h.tokenCount(), 2);
  h.client.close();
});
test("auth rejection stops retries until explicit reconnect; foreground cannot override it", async () => {
  const h = harness();
  h.client.connect();
  await flush();
  const first = h.sockets[0]!;
  first.open();
  first.receive({ type: "relay-error", v: 1, loginRequired: true, message: "secret internals" });
  assert.equal(h.client.getSnapshot().loginRequired, true);
  assert.doesNotMatch(h.client.getSnapshot().error!, /secret/);
  h.clock.advance(120000);
  h.client.notifyForeground();
  h.client.notifyNetworkChange();
  await flush();
  assert.equal(h.sockets.length, 1);
  h.client.connect();
  await flush();
  assert.equal(h.sockets.length, 2);
  h.sockets[1]!.open();
  h.sockets[1]!.receive({ type: "relay-ready", v: 1 });
  assert.equal(h.client.getSnapshot().status, "ready");
  h.client.close();
});
test("post-admission 4401 recovers bounded with resume, then latches credential failure", async () => {
  const h = harness(), socket = await h.admit();
  const pending = listTabs(h.client.rpc);
  const rejected = assert.rejects(pending, (error: unknown) => error instanceof RelayRpcError && error.loginRequired === false);
  socket.disconnect(4401);
  await rejected;
  // First post-admission 4401 recovers (no session held: fresh token, no resume).
  assert.equal(h.client.getSnapshot().loginRequired, false);
  assert.equal(h.client.getSnapshot().status, "reconnecting");
  // Two recoveries re-admit (attempts 1 and 2 of the 3-attempt cap).
  for (const wait of [RELAY_SOCKET_TIMING.firstRetryMs, RELAY_SOCKET_TIMING.firstRetryMs * 2]) {
    h.clock.advance(wait);
    await flush();
    const next = h.sockets.at(-1)!;
    next.open();
    next.receive({ type: "relay-ready", v: 1 });
    assert.equal(h.client.getSnapshot().status, "ready");
    next.disconnect(4401);
    await flush();
    assert.equal(h.client.getSnapshot().status, "reconnecting");
  }
  // Third recovery dials, then admission rejects: revoked credential latches.
  h.clock.advance(RELAY_SOCKET_TIMING.firstRetryMs * 4);
  await flush();
  const last = h.sockets.at(-1)!;
  last.open();
  last.receive({ type: "relay-error", v: 1, loginRequired: true, message: "revoked" });
  await flush();
  assert.equal(h.client.getSnapshot().loginRequired, true);
  assert.equal(h.client.getSnapshot().status, "error");
  h.clock.advance(120000);
  await flush();
  assert.equal(h.sockets.length, 4);
  h.client.close();
});
test("subscription waits for ready, requires snapshot sequence 1 and resubscribes after reconnect", async () => {
  const h = harness();
  const snapshots: unknown[] = [], events: unknown[] = [];
  h.client.rpc.subscribe("catalog", {}, (payload, info) => (info.snapshot ? snapshots : events).push(payload));
  h.client.connect();
  await flush();
  const first = h.sockets[0]!;
  first.open();
  assert.equal(first.sent.length, 1);
  first.receive({ type: "relay-ready", v: 1 });
  assert.deepEqual(first.sent.at(-1)?.params, { device_id: "desktop-1" });
  first.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 8, payload: { events: [] } });
  assert.equal(snapshots.length, 0);
  first.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload: { events: [] } });
  first.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 2, payload: { kind: "control_granted", title: "Edit", body: "Allowed" } });
  assert.equal(snapshots.length, 1);
  assert.equal(events.length, 1);
  first.disconnect();
  h.clock.advance(500);
  await flush();
  const next = h.sockets[1]!;
  next.open();
  assert.equal(next.sent.filter((frame) => frame.type === "rpc-subscribe").length, 0);
  next.receive({ type: "relay-ready", v: 1 });
  assert.equal(next.sent.filter((frame) => frame.type === "rpc-subscribe").length, 1);
  next.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload: { events: [] } });
  assert.equal(snapshots.length, 2);
  h.client.close();
});
test("core registers requests before synchronous replies and never loses their resolution", async () => {
  const id = "00000000-0000-4000-8000-000000000001";
  let client: RelayRpcClient;
  client = new RelayRpcClient({ createRequestId: () => id, send: (frame) => { client.handleMessage({ type: "rpc-reply", v: 1, id: frame.id, ok: true, result: { tabs: [] } }); return true; } });
  assert.deepEqual(await client.request("tabs.list", {}), { tabs: [] });
  assert.equal(client.pendingCount(), 0);
  client.close();
});
test("core bounds pending requests and UTF8 params, and closes held unsubscribe on disconnect", async () => {
  const clock = new Clock();
  let ids = 0;
  const sent: unknown[] = [];
  const client = new RelayRpcClient({ clock, send: (frame) => { sent.push(frame); return true; }, createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}` });
  await assert.rejects(client.request("files.search", { query: "😀".repeat(20000) }), /invalid/);
  await assert.rejects(client.request("files.search", []), /invalid/);
  const cyclic: Record<string, unknown> = {};
  cyclic.self = cyclic;
  await assert.rejects(client.request("files.search", cyclic), /invalid/);
  assert.equal(sent.length, 0);
  const waiting = Array.from({ length: RELAY_RPC_LIMITS.pending }, () => client.request("tabs.list", {}).catch((error: unknown) => error));
  await assert.rejects(client.request("tabs.list", {}), /Too many/);
  const sub = client.subscribe("runtime.events", {}, () => { }), held = sub.unsubscribe();
  client.disconnect();
  await held;
  await Promise.all(waiting);
  assert.equal(sub.active, false);
  assert.equal(client.pendingCount(), 0);
  assert.equal(clock.timers.size, 0);
  client.close();
});
test("held unsubscribe has a deadline when a server never sends a snapshot", async () => {
  const clock = new Clock();
  let ids = 0;
  const client = new RelayRpcClient({ clock, send: () => true, createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}` });
  const sub = client.subscribe("runtime.events", {}, () => { }), held = sub.unsubscribe();
  clock.advance(15000);
  await held;
  assert.equal(sub.active, false);
  assert.equal(clock.timers.size, 0);
  client.close();
});
test("briefly admitted sockets keep exponential backoff instead of flooding reconnects", async () => {
  const h = harness();
  const first = await h.admit();
  first.disconnect();
  h.clock.advance(500);
  await flush();
  const second = h.sockets[1]!;
  second.open();
  second.receive({ type: "relay-ready", v: 1 });
  second.disconnect();
  h.clock.advance(999);
  await flush();
  assert.equal(h.sockets.length, 2);
  h.clock.advance(1);
  await flush();
  assert.equal(h.sockets.length, 3);
  h.client.close();
});
test("token refresh network failure retries without latching sign-in rejection", async () => {
  const clock = new Clock();
  let attempts = 0;
  const sockets: Socket[] = [];
  const client = new RelaySocketClient({ gateway: "wss://relay.example/realtime", deviceId: "desktop", clock, random: () => 0.5,
    resolveAccessToken: async () => { if (++attempts === 1)
      throw new Error("private network detail"); return "fresh-token"; },
    createSocket: () => { const socket = new Socket(); sockets.push(socket); return socket; },
  });
  client.connect();
  await flush();
  assert.equal(client.getSnapshot().loginRequired, false);
  assert.equal(client.getSnapshot().status, "reconnecting");
  clock.advance(500);
  await flush();
  assert.equal(sockets.length, 1);
  sockets[0]!.open();
  sockets[0]!.receive({ type: "relay-ready", v: 1 });
  assert.equal(client.getSnapshot().status, "ready");
  client.close();
});

test("closing while account auth resolves cannot create a late socket", async () => {
  const clock = new Clock(); let finish: ((token: string) => void) | undefined; let sockets = 0;
  const client = new RelaySocketClient({ gateway: "wss://relay.example/realtime", deviceId: "desktop", clock,
    resolveAccessToken: () => new Promise<string>((resolve) => { finish = resolve; }),
    createSocket: () => { sockets++; return new Socket(); },
  });
  client.connect(); await flush(); client.close(); finish?.("late-private-token"); await flush();
  assert.equal(sockets, 0); assert.equal(clock.timers.size, 0); assert.equal(client.getSnapshot().status, "idle");
});

test("an open socket that never admits times out and refreshes credentials before retry", async () => {
  const h = harness(); h.client.connect(); await flush(); h.sockets[0]!.open();
  h.clock.advance(RELAY_SOCKET_TIMING.admissionMs);
  assert.equal(h.client.getSnapshot().status, "reconnecting");
  h.clock.advance(500); await flush(); assert.equal(h.sockets.length, 2); h.sockets[1]!.open();
  assert.equal(h.sockets[1]!.sent[0]?.access_token, "account-token-2"); h.client.close();
});

// ── Companion session lifecycle (fake clock, memory store) ────────────────

const SESSION_ID_A = "00000000-0000-4000-8000-000000000101";
const SESSION_ID_B = "00000000-0000-4000-8000-000000000102";
const T0 = 1_000_000;
const DAY_MS = 86_400_000;

function memoryStore(
  initial: { sessionId: string; expiresAt: number } | null = null,
  fail: { load?: boolean; clear?: boolean } = {},
) {
  let saved = initial ? { ...initial } : null;
  const calls: string[] = [];
  return {
    calls,
    peek: () => saved,
    store: {
      loadSession: async () => {
        calls.push("load");
        if (fail.load) throw new Error("storage down");
        return saved ? { ...saved } : null;
      },
      saveSession: async (lease: { sessionId: string; expiresAt: number }) => {
        calls.push("save");
        saved = { ...lease };
      },
      clearSession: async () => {
        calls.push("clear");
        if (fail.clear) throw new Error("clear down");
        saved = null;
      },
    },
  };
}

function sessionHarness(store?: { loadSession: () => Promise<{ sessionId: string; expiresAt: number } | null>; saveSession: (lease: { sessionId: string; expiresAt: number }) => Promise<void>; clearSession: () => Promise<void> }) {
  const clock = new Clock();
  clock.time = T0;
  const sockets: Socket[] = [];
  let ids = 0, tokens = 0;
  const client = new RelaySocketClient({ gateway: "wss://relay.example/realtime", deviceId: "desktop-1", clock,
    resolveAccessToken: async () => `account-token-${++tokens}`,
    ...(store ? store : {}),
    createSocket: () => { const socket = new Socket(); sockets.push(socket); return socket; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`, random: () => 0.5,
  });
  const readyLease = (sessionId: string, sessionExp: number, accessExp: number) => ({
    type: "relay-ready", v: 1, session_id: sessionId, session_expires_at: sessionExp, access_expires_at: accessExp,
  });
  return { clock, sockets, client, readyLease, tokenCount: () => tokens };
}

async function admitLease(h: ReturnType<typeof sessionHarness>, sessionId: string, sessionExp: number, accessExp: number) {
  h.client.connect();
  await flush();
  const socket = h.sockets.at(-1)!;
  socket.open();
  socket.receive(h.readyLease(sessionId, sessionExp, accessExp));
  await flush();
  return socket;
}

test("lease admission persists, resumes on reconnect, and resubscribes streams", async () => {
  const mem = memoryStore();
  const h = sessionHarness(mem.store);
  const snapshots: unknown[] = [];
  h.client.rpc.subscribe("catalog", {}, (payload, info) => { if (info.snapshot) snapshots.push(payload); });
  const first = await admitLease(h, SESSION_ID_A, T0 + DAY_MS, T0 + 900_000);
  assert.deepEqual(first.sent[0], { type: "relay-auth", v: 1, access_token: "account-token-1", e2e: 1, device_id: "desktop-1" });
  assert.equal(h.client.getSnapshot().status, "ready");
  assert.equal(h.client.getSnapshot().sessionStatus, "active");
  assert.equal(h.client.getSnapshot().sessionEndsAt, T0 + DAY_MS);
  assert.ok(mem.calls.includes("save"));
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS });
  first.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload: { events: [] } });
  assert.equal(snapshots.length, 1);
  // Socket loss redials with the same resume id; streams resubscribe, nothing replays.
  first.disconnect();
  h.clock.advance(500);
  await flush();
  const next = h.sockets.at(-1)!;
  next.open();
  assert.deepEqual(next.sent[0], { type: "relay-auth", v: 1, access_token: "account-token-2", e2e: 1, device_id: "desktop-1", resume_session_id: SESSION_ID_A });
  next.receive(h.readyLease(SESSION_ID_A, T0 + DAY_MS, T0 + 900_000));
  await flush();
  assert.equal(h.client.getSnapshot().status, "ready");
  assert.equal(h.client.getSnapshot().connectionEpoch, 2);
  assert.equal(next.sent.filter((frame) => frame.type === "rpc-subscribe").length, 1);
  next.receive({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload: { events: [] } });
  assert.equal(snapshots.length, 2);
  assert.equal(next.sent.filter((frame) => frame.type === "rpc-request").length, 0);
  h.client.close();
});

test("expired lease on load surfaces ended without dialing", async () => {
  const mem = memoryStore({ sessionId: SESSION_ID_A, expiresAt: T0 - 1000 });
  const h = sessionHarness(mem.store);
  h.client.connect();
  await flush();
  assert.equal(h.sockets.length, 0);
  assert.equal(h.client.getSnapshot().status, "error");
  assert.match(h.client.getSnapshot().error!, /ended/);
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  assert.equal(h.client.getSnapshot().sessionEndsAt, T0 - 1000);
  // The expired lease is retained, never replaced: still no socket later.
  h.clock.advance(60000);
  await flush();
  assert.equal(h.sockets.length, 0);
  assert.ok(!mem.calls.includes("clear"));
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 - 1000 });
  h.client.close();
});

test("startNewSession clears the stored lease and connects fresh", async () => {
  const mem = memoryStore({ sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS });
  const h = sessionHarness(mem.store);
  h.client.startNewSession();
  await flush();
  assert.deepEqual(mem.calls.slice(0, 2), ["clear", "load"]);
  assert.equal(h.sockets.length, 1);
  const socket = h.sockets[0]!;
  socket.open();
  assert.deepEqual(socket.sent, [{ type: "relay-auth", v: 1, access_token: "account-token-1", e2e: 1, device_id: "desktop-1" }]);
  socket.receive(h.readyLease(SESSION_ID_B, T0 + DAY_MS, T0 + 900_000));
  await flush();
  assert.equal(h.client.getSnapshot().status, "ready");
  assert.equal(h.client.getSnapshot().sessionStatus, "active");
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_B, expiresAt: T0 + DAY_MS });
  h.client.close();
});

test("4408 retains the lease, latches ended, and plain connect never mints", async () => {
  const mem = memoryStore();
  const h = sessionHarness(mem.store);
  const socket = await admitLease(h, SESSION_ID_A, T0 + DAY_MS, T0 + 900_000);
  socket.disconnect(4408);
  await flush();
  assert.equal(h.client.getSnapshot().status, "error");
  assert.match(h.client.getSnapshot().error!, /ended/);
  assert.equal(h.client.getSnapshot().loginRequired, false);
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  assert.ok(!mem.calls.includes("clear"));
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS });
  // Plain connect surfaces ended; no new lease, no socket.
  h.client.connect();
  await flush();
  assert.equal(h.sockets.length, 1);
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  h.client.notifyForeground();
  h.client.notifyNetworkChange();
  await flush();
  assert.equal(h.sockets.length, 1);
  // Only the explicit user action starts fresh.
  h.client.startNewSession();
  await flush();
  assert.equal(h.sockets.length, 2);
  h.sockets[1]!.open();
  assert.deepEqual(h.sockets[1]!.sent, [{ type: "relay-auth", v: 1, access_token: "account-token-2", e2e: 1, device_id: "desktop-1" }]);
  h.client.close();
});

test("session-ended error frame ends immediately without waiting for the 4408 close", async () => {
  const mem = memoryStore();
  const h = sessionHarness(mem.store);
  const socket = await admitLease(h, SESSION_ID_A, T0 + DAY_MS, T0 + 900_000);
  // The server sends relay-error {loginRequired:false, sessionEnded:true}
  // first, then closes 4408. Ending on the frame (not the close) keeps the
  // teardown from hiding the 4408 inside a retry loop.
  socket.receive({ type: "relay-error", v: 1, loginRequired: false, sessionEnded: true, message: "session ended" });
  await flush();
  assert.equal(h.client.getSnapshot().status, "error");
  assert.match(h.client.getSnapshot().error!, /ended/);
  assert.equal(h.client.getSnapshot().loginRequired, false);
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  assert.equal(h.client.getSnapshot().sessionEndsAt, T0 + DAY_MS);
  // The trailing 4408 lands on detached handlers: no-op, no redial.
  socket.disconnect(4408);
  await flush();
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  h.clock.advance(120000);
  h.client.notifyForeground();
  h.client.notifyNetworkChange();
  await flush();
  assert.equal(h.sockets.length, 1);
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS });
  assert.ok(!mem.calls.includes("clear"));
  h.client.close();

  // A non-boolean sessionEnded is not an end signal: existing error path.
  const g = sessionHarness();
  g.client.connect();
  await flush();
  const other = g.sockets[0]!;
  other.open();
  other.receive({ type: "relay-ready", v: 1 });
  other.receive({ type: "relay-error", v: 1, sessionEnded: "yes" });
  await flush();
  assert.equal(g.client.getSnapshot().status, "reconnecting");
  g.client.close();
});

test("proactive redial fires before access expiry with the held resume", async () => {
  const mem = memoryStore();
  const h = sessionHarness(mem.store);
  const first = await admitLease(h, SESSION_ID_A, T0 + DAY_MS, T0 + 120_000);
  // Margin 60s: redial at +60s; heartbeats at +25s/+50s stay under staleMs.
  h.clock.advance(60000);
  await flush();
  assert.equal(h.sockets.length, 2);
  const next = h.sockets[1]!;
  next.open();
  assert.deepEqual(next.sent[0], { type: "relay-auth", v: 1, access_token: "account-token-2", e2e: 1, device_id: "desktop-1", resume_session_id: SESSION_ID_A });
  next.receive(h.readyLease(SESSION_ID_A, T0 + DAY_MS, T0 + 1_200_000));
  await flush();
  assert.equal(h.client.getSnapshot().status, "ready");
  assert.equal(h.client.getSnapshot().connectionEpoch, 2);
  assert.equal(h.client.getSnapshot().sessionStatus, "active");
  assert.equal(first.sent.filter((frame) => frame.type === "rpc-request").length, 0);
  h.client.close();
});

test("absolute session expiry ends even without traffic", async () => {
  const mem = memoryStore();
  const h = sessionHarness(mem.store);
  await admitLease(h, SESSION_ID_A, T0 + 120_000, T0 + 3_600_000);
  // Heartbeat would kill an idle socket at staleMs: answer pongs on the way.
  for (let step = 0; step < 4; step++) {
    h.clock.advance(25000);
    h.sockets[0]!.receive({ type: "relay-pong", v: 1 });
  }
  h.clock.advance(20000);
  await flush();
  assert.equal(h.client.getSnapshot().sessionStatus, "ended");
  assert.match(h.client.getSnapshot().error!, /ended/);
  assert.equal(h.sockets.length, 1);
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + 120_000 });
  assert.ok(!mem.calls.includes("clear"));
  h.client.close();
});

test("storage load failure retries without minting a new session", async () => {
  const mem = memoryStore(null, { load: true });
  const h = sessionHarness(mem.store);
  h.client.connect();
  await flush();
  assert.equal(h.sockets.length, 0);
  assert.equal(h.client.getSnapshot().status, "reconnecting");
  assert.match(h.client.getSnapshot().error!, /unavailable/);
  h.clock.advance(500);
  await flush();
  assert.equal(h.sockets.length, 0);
  assert.ok(!mem.calls.includes("save"));
  assert.equal(mem.peek(), null);
  h.client.close();
});

test("storage clear failure blocks a fresh session and keeps the old lease", async () => {
  const mem = memoryStore({ sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS }, { clear: true });
  const h = sessionHarness(mem.store);
  h.client.startNewSession();
  await flush();
  assert.equal(h.sockets.length, 0);
  assert.equal(h.client.getSnapshot().status, "error");
  assert.match(h.client.getSnapshot().error!, /unavailable/);
  assert.ok(mem.calls.includes("clear"));
  assert.ok(!mem.calls.includes("save"));
  assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS });
  h.client.close();
});

test("held lease with replaced id or extended expiry fails closed", async () => {
  for (const [name, sessionId, expiresAt] of [
    ["replaced id", SESSION_ID_B, T0 + DAY_MS],
    ["extended expiry", SESSION_ID_A, T0 + DAY_MS + 5000],
  ] as const) {
    const mem = memoryStore();
    const h = sessionHarness(mem.store);
    const first = await admitLease(h, SESSION_ID_A, T0 + DAY_MS, T0 + 900_000);
    first.disconnect();
    h.clock.advance(500);
    await flush();
    const next = h.sockets.at(-1)!;
    next.open();
    next.receive(h.readyLease(sessionId, expiresAt, T0 + 900_000));
    await flush();
    assert.equal(h.client.getSnapshot().status, "error", name);
    assert.match(h.client.getSnapshot().error!, /could not be read/, name);
    assert.equal(h.client.getSnapshot().loginRequired, false, name);
    h.clock.advance(120000);
    await flush();
    assert.equal(h.sockets.length, 2, name);
    assert.deepEqual(mem.peek(), { sessionId: SESSION_ID_A, expiresAt: T0 + DAY_MS }, name);
    h.client.close();
  }
});

test("malformed partial ready fails closed; legacy ready stays untracked", async () => {
  const bad = sessionHarness();
  bad.client.connect();
  await flush();
  const first = bad.sockets[0]!;
  first.open();
  first.receive({ type: "relay-ready", v: 1, session_id: "abc-123" });
  await flush();
  assert.equal(bad.client.getSnapshot().status, "error");
  assert.match(bad.client.getSnapshot().error!, /could not be read/);
  bad.clock.advance(60000);
  await flush();
  assert.equal(bad.sockets.length, 1);
  bad.client.close();

  const legacy = sessionHarness();
  legacy.client.connect();
  await flush();
  const socket = legacy.sockets[0]!;
  socket.open();
  socket.receive({ type: "relay-ready", v: 1 });
  await flush();
  assert.equal(legacy.client.getSnapshot().status, "ready");
  assert.equal(legacy.client.getSnapshot().sessionStatus, "none");
  assert.equal(legacy.client.getSnapshot().sessionEndsAt, null);
  legacy.client.close();
});
