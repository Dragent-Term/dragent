import assert from "node:assert/strict";
import { test } from "node:test";
import { RelaySocketClient } from "../src/transport/RelaySocketClient";
import { RelayRpcClient, RelayRpcError } from "../src/transport/relayRpcClient";
import { listMachines, subscribeMachineCatalog } from "../src/transport/relayRpcMethods";
import { selectOwnedState } from "../src/transport/ownedResult";
import { createMachineSocketSource } from "../src/machines/socketSource";
import type { RealtimeClock } from "../src/transport/clock";
import type { RealtimeSocket } from "../src/transport/socket";

class Clock implements RealtimeClock {
  time = 0;
  sequence = 0;
  timers = new Map<number, { at: number; fn: () => void }>();
  now = () => this.time;
  setTimeout = (fn: () => void, ms: number) => {
    const id = ++this.sequence;
    this.timers.set(id, { at: this.time + ms, fn });
    return id;
  };
  clearTimeout = (id: unknown) => {
    this.timers.delete(id as number);
  };
}

class Socket implements RealtimeSocket {
  readyState = 0;
  onopen: RealtimeSocket["onopen"] = null;
  onmessage: RealtimeSocket["onmessage"] = null;
  onerror: RealtimeSocket["onerror"] = null;
  onclose: RealtimeSocket["onclose"] = null;
  sent: Record<string, unknown>[] = [];
  send(text: string) {
    this.sent.push(JSON.parse(text));
  }
  close() {
    this.readyState = 3;
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

async function flush() {
  for (let i = 0; i < 8; i++) await Promise.resolve();
}

test("account sources carry per-account identity even with a stable controller method", async () => {
  // The controller's getAccessToken stays stable across users; the source
  // must still identify the account or the old socket survives the switch.
  const stableGetAccessToken = async () => "current-account-token";
  const forAlice = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: null,
    requireDevice: false,
    accountId: "user-alice",
    signedIn: true,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: null,
  });
  const forAliceAgain = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: null,
    requireDevice: false,
    accountId: "user-alice",
    signedIn: true,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: null,
  });
  const forBob = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: null,
    requireDevice: false,
    accountId: "user-bob",
    signedIn: true,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: null,
  });
  assert.equal(forAlice.accountId, "user-alice");
  assert.equal(forAliceAgain.accountId, "user-alice");
  assert.equal(forBob.accountId, "user-bob");
  // Fresh resolver identity per account generation.
  assert.notEqual(forAlice.resolveAccessToken, forBob.resolveAccessToken);
  assert.equal(await forAlice.resolveAccessToken!(), "current-account-token");
  assert.equal(await forBob.resolveAccessToken!(), "current-account-token");
  const signedOut = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: "desktop-1",
    accountId: null,
    signedIn: false,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: null,
  });
  assert.equal(signedOut.accountId, null);
  assert.equal(signedOut.resolveAccessToken, null);
});

test("account sources reject blank identities and pass device requirements through", async () => {
  const stableGetAccessToken = async () => "current-account-token";
  const base = {
    gateway: "wss://relay.example/realtime",
    deviceId: "desktop-1",
    signedIn: true,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: "phone-1",
  } as const;
  // Blank or missing account ids never authenticate, even when signed in.
  for (const accountId of [null, undefined, "", "   "]) {
    const source = createMachineSocketSource({ ...base, accountId });
    assert.equal(source.accountId, null);
    assert.equal(source.resolveAccessToken, null);
  }
  // A stale signed-in flag with an account id still yields no resolver.
  const stale = createMachineSocketSource({ ...base, accountId: "user-alice", signedIn: false });
  assert.equal(stale.accountId, null);
  assert.equal(stale.resolveAccessToken, null);
  // Targeted machine sources keep their device; discovery sources stay
  // device-free with an explicit opt-out that the hook enforces.
  const targeted = createMachineSocketSource({ ...base, accountId: "user-alice" });
  assert.equal(targeted.deviceId, "desktop-1");
  assert.equal(targeted.requireDevice, undefined);
  // The phone's own id reaches the socket: without it there is no E2EE
  // security and nothing from the desktop can be opened (live bug, 2026-11-10).
  assert.equal(targeted.phoneDeviceId, "phone-1");
  // No paired account: no phone identity either.
  assert.equal(stale.phoneDeviceId, null);
  const discovery = createMachineSocketSource({
    ...base,
    deviceId: null,
    requireDevice: false,
    accountId: "user-alice",
  });
  assert.equal(discovery.deviceId, null);
  assert.equal(discovery.requireDevice, false);
  assert.equal(discovery.gateway, "wss://relay.example/realtime");
});

test("account change admits fresh auth and never renders the old account's result", async () => {
  const clock = new Clock();
  const sockets: Socket[] = [];
  const tokens = { alice: "account-token-alice", bob: "account-token-bob" };
  let ids = 0;
  const makeClient = (source: ReturnType<typeof createMachineSocketSource>) =>
    new RelaySocketClient({
      gateway: source.gateway!,
      ...(typeof source.deviceId === "string" && source.deviceId
        ? { deviceId: source.deviceId }
        : {}),
      resolveAccessToken: source.resolveAccessToken!,
      clock,
      createSocket: (url) => {
        assert.equal(url, "wss://relay.example/v1/relay/socket");
        const socket = new Socket();
        sockets.push(socket);
        return socket;
      },
      createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
      random: () => 0.5,
    });

  const stableGetAccessToken = async () => tokens.alice;
  const sourceA = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: null,
    requireDevice: false,
    accountId: "user-alice",
    signedIn: true,
    getAccessToken: stableGetAccessToken,
    phoneDeviceId: null,
  });
  const clientA = makeClient(sourceA);
  clientA.connect();
  await flush();
  const socketA = sockets.at(-1)!;
  socketA.open();
  assert.equal(socketA.sent[0]?.access_token, tokens.alice);
  socketA.receive({ type: "relay-ready", v: 1 });
  assert.equal(clientA.getSnapshot().status, "ready");
  assert.equal(clientA.getSnapshot().connectionEpoch, 1);

  // Alice's discovery resolves.
  const foundA = listMachines(clientA.rpc);
  const requestA = socketA.sent.at(-1)!;
  assert.deepEqual(requestA.params, {});
  socketA.receive({
    type: "rpc-reply", v: 1, id: requestA.id, ok: true,
    result: { machines: [{ device_id: "desktop-1", name: "Studio", platform: "macOS", last_seen_at: 1_000, online: true }] },
  });
  const machinesA = await foundA;
  assert.equal(machinesA.length, 1);

  // Account switch: Bob's source forces a new resolver identity and client.
  // The controller method is unchanged underneath (stable identity).
  const sourceB = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: null,
    requireDevice: false,
    accountId: "user-bob",
    signedIn: true,
    getAccessToken: async () => tokens.bob,
    phoneDeviceId: null,
  });
  assert.notEqual(sourceB.resolveAccessToken, sourceA.resolveAccessToken);
  assert.notEqual(sourceB.accountId, sourceA.accountId);
  const clientB = makeClient(sourceB);
  clientB.connect();
  await flush();
  const socketB = sockets.at(-1)!;
  socketB.open();
  // Fresh auth admission for the new account — never Alice's token.
  assert.equal(socketB.sent[0]?.access_token, tokens.bob);
  assert.notEqual(socketB.sent[0]?.access_token, tokens.alice);
  socketB.receive({ type: "relay-ready", v: 1 });
  assert.equal(clientB.getSnapshot().connectionEpoch, 1);

  // Both generations sit at epoch 1: epoch-only keying would leak Alice's
  // list into Bob's render before passive cleanup. Client identity masks it.
  assert.equal(clientA.getSnapshot().connectionEpoch, clientB.getSnapshot().connectionEpoch);
  const storedFromA = { client: clientA, epoch: 1 as number, state: machinesA };
  assert.equal(selectOwnedState(storedFromA, clientB, 1), null);
  assert.equal(selectOwnedState(storedFromA, clientA, 1), machinesA);
  assert.equal(selectOwnedState(storedFromA, null, 1), null);
  assert.equal(selectOwnedState(storedFromA, clientA, 2), null);
  assert.equal(selectOwnedState(null, clientB, 1), null);

  clientA.close();
  clientB.close();
  assert.equal(clock.timers.size, 0);
});

test("sign-out rejects pending discovery and drops its late reply", async () => {
  const clock = new Clock();
  const sockets: Socket[] = [];
  let ids = 0;
  const client = new RelaySocketClient({
    gateway: "wss://relay.example/realtime",
    resolveAccessToken: async () => "account-token",
    clock,
    createSocket: () => {
      const socket = new Socket();
      sockets.push(socket);
      return socket;
    },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
    random: () => 0.5,
  });
  client.connect();
  await flush();
  const socket = sockets.at(-1)!;
  socket.open();
  socket.receive({ type: "relay-ready", v: 1 });

  const pending = listMachines(client.rpc);
  const request = socket.sent.at(-1)!;
  const rejected = assert.rejects(pending, /Connection lost/);
  // Sign-out closes the socket: the old account's response can never land.
  client.close();
  await rejected;
  socket.receive({ type: "rpc-reply", v: 1, id: request.id, ok: true, result: { machines: [] } });
  await flush();
  assert.equal(client.rpc.pendingCount(), 0);
  assert.equal(clock.timers.size, 0);
});

test("catalog subscription failures report through onError at once", () => {
  const sent: Record<string, unknown>[] = [];
  let ids = 0;
  const client = new RelayRpcClient({
    send: (frame) => {
      sent.push(frame);
      return true;
    },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
  });
  const snapshots: unknown[] = [];
  const updates: unknown[] = [];
  const errors: unknown[] = [];
  // The relay stores only sealed catalogs; this stands in for the pin-store
  // verification (tested with real sealing in src/transport/__tests__/e2ee).
  const verified = (p: unknown) => {
    const { catalog, machine } = (p ?? {}) as { catalog?: unknown; machine?: unknown };
    return { kind: "catalog" as const, json: catalog, version: 1, machine };
  };
  subscribeMachineCatalog(client, {
    onSnapshot: (snapshot) => snapshots.push(snapshot),
    onUpdate: (snapshot) => updates.push(snapshot),
    onError: (error) => errors.push(error),
  }, verified);
  const subscribe = sent.find((frame) => frame.type === "rpc-subscribe")!;
  assert.equal(subscribe.stream, "catalog");
  assert.deepEqual(subscribe.params, {});
  const now = Date.now();
  const payload = {
    catalog: { projects: [], workspaces: [], tabs: [] },
    machine: { device_id: "desktop-1", name: "Studio", platform: "macOS", last_seen_at: now, online: true },
  };
  client.handleMessage({ type: "rpc-stream", v: 1, stream: "catalog", seq: 1, payload });
  assert.equal(snapshots.length, 1);
  assert.equal(errors.length, 0);
  // A malformed update is dropped without breaking the sequence: the next
  // valid frame still flows through.
  client.handleMessage({ type: "rpc-stream", v: 1, stream: "catalog", seq: 2, payload: { catalog: null } });
  assert.equal(updates.length, 0);
  client.handleMessage({ type: "rpc-stream", v: 1, stream: "catalog", seq: 3, payload });
  assert.equal(updates.length, 1);

  // A rejected subscription reports immediately through onError.
  const failing = new RelayRpcClient({
    send: (frame) => {
      sent.push(frame);
      return true;
    },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
  });
  const seen: unknown[] = [];
  subscribeMachineCatalog(failing, {
    onSnapshot: (snapshot) => snapshots.push(snapshot),
    onUpdate: (snapshot) => updates.push(snapshot),
    onError: (error) => seen.push(error),
  }, undefined);
  const rejected = sent.filter((frame) => frame.type === "rpc-subscribe").at(-1)!;
  failing.handleMessage({
    type: "rpc-reply", v: 1, id: rejected.id, ok: false,
    error: { message: "this machine has not shared its catalog", loginRequired: false },
  });
  assert.equal(seen.length, 1);
  assert.ok(seen[0] instanceof RelayRpcError);
  assert.match((seen[0] as Error).message, /has not shared its catalog/);
  client.close();
  failing.close();
});

test("targeted machine socket still forcibly owns its device per account", async () => {
  const clock = new Clock();
  const sockets: Socket[] = [];
  const source = createMachineSocketSource({
    gateway: "wss://relay.example/realtime",
    deviceId: "desktop-1",
    accountId: "user-alice",
    signedIn: true,
    getAccessToken: async () => "account-token",
    phoneDeviceId: "phone-1",
  });
  const client = new RelaySocketClient({
    gateway: source.gateway!,
    deviceId: typeof source.deviceId === "string" ? source.deviceId : "",
    resolveAccessToken: source.resolveAccessToken!,
    clock,
    createSocket: () => {
      const socket = new Socket();
      sockets.push(socket);
      return socket;
    },
  });
  client.connect();
  await flush();
  const socket = sockets.at(-1)!;
  socket.open();
  socket.receive({ type: "relay-ready", v: 1 });
  // catalog.read now goes to the desktop inside the encrypted channel; the
  // relay-answered route still has its target forced.
  const request = client.rpc.request("machines.list", { device_id: "attacker-device" });
  const sent = socket.sent.at(-1)!;
  assert.deepEqual(sent.params, { device_id: "desktop-1" });
  socket.receive({ type: "rpc-reply", v: 1, id: sent.id, ok: true, result: { machines: [] } });
  await request;
  client.close();
});
