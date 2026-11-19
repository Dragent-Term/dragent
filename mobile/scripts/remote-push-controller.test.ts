import assert from "node:assert/strict";
import { test } from "node:test";
import { PushController, PUSH_RENEWAL_MS, type PushNativeDriver } from "../src/notifications/pushController";
import { resolvePushBaseURL } from "../src/notifications/pushApi";
import type { KeyValueStore } from "../src/storage/types";

function memoryPrefs(initial: Record<string, string> = {}): KeyValueStore & { dump(): Record<string, string> } {
  const store = new Map(Object.entries(initial));
  return {
    async getItem(key) { return store.has(key) ? store.get(key)! : null; },
    async setItem(key, value) { store.set(key, value); },
    async removeItem(key) { store.delete(key); },
    dump() { return Object.fromEntries(store); },
  };
}

interface FakeFetchCall { url: string; init?: { method?: string; headers?: Record<string, string>; body?: string } | undefined }
function fakeFetch(handler: (call: FakeFetchCall) => { status: number; body: unknown }) {
  const calls: FakeFetchCall[] = [];
  const fn = async (url: string, init?: FakeFetchCall["init"]) => {
    if (init === undefined) calls.push({ url });
    else calls.push({ url, init });
    const { status, body } = handler({ url, init });
    return {
      ok: status >= 200 && status < 300,
      status,
      async text() { return JSON.stringify(body); },
    };
  };
  return { fn, calls };
}

interface FakeTestDriver extends PushNativeDriver {
  readonly permissionCalls: number;
  readonly tokenCalls: number;
  readonly channelCalls: number;
  token: string;
  granted: boolean;
}

function fakeDriver(overrides: Partial<PushNativeDriver> = {}): FakeTestDriver {
  const state = {
    permissionCalls: 0, tokenCalls: 0, channelCalls: 0,
    token: "native-device-token-1", granted: true,
    available: true, unavailableReason: null as string | null, platform: "apns" as const,
    ...overrides,
  };
  return {
    get available() { return state.available; },
    get unavailableReason() { return state.unavailableReason; },
    get platform() { return state.platform; },
    get permissionCalls() { return state.permissionCalls; },
    get tokenCalls() { return state.tokenCalls; },
    get channelCalls() { return state.channelCalls; },
    get token() { return state.token; },
    set token(value: string) { state.token = value; },
    get granted() { return state.granted; },
    set granted(value: boolean) { state.granted = value; },
    async requestPermission() { state.permissionCalls++; return state.granted; },
    async getDevicePushToken() { state.tokenCalls++; if (!state.token.trim()) throw new Error("The device did not return a push token."); return state.token; },
    async ensureAndroidChannel() { state.channelCalls++; },
  };
}

function harness(opts: {
  accountId?: string | null;
  gateway?: string | null;
  accessToken?: string | null;
  driver?: PushNativeDriver;
  now?: () => number;
} = {}) {
  const prefs = memoryPrefs();
  let accountId: string | null = opts.accountId ?? "account-a";
  let gateway: string | null = opts.gateway ?? "https://gateway.example";
  let accessToken: string | null = opts.accessToken === undefined ? "test-jwt" : opts.accessToken;
  const driver = opts.driver ?? fakeDriver();
  let now = 1_000_000;
  const fetch = fakeFetch(() => ({ status: 200, body: { registered: true, device_id: "device-1", platform: "apns", expires_at: "2030-01-01T00:00:00Z" } }));
  const controller = new PushController({
    prefs,
    getAccountId: () => accountId,
    getGateway: () => gateway,
    getAccessToken: async () => accessToken,
    driver,
    fetchFn: fetch.fn,
    now: opts.now ?? (() => now),
  });
  return {
    prefs, driver, fetch, controller,
    setAccount(id: string | null) { accountId = id; },
    setGateway(g: string | null) { gateway = g; },
    setAccessToken(t: string | null) { accessToken = t; },
    advance(ms: number) { now += ms; },
  };
}

test("push base URL derives https origin and rejects credential carriers", () => {
  assert.equal(resolvePushBaseURL("wss://example.test/relay"), "https://example.test");
  assert.equal(resolvePushBaseURL("ws://127.0.0.1:8080/relay"), "http://127.0.0.1:8080");
  assert.equal(resolvePushBaseURL("https://example.test/x"), "https://example.test");
  assert.equal(resolvePushBaseURL("wss://user:pass@example.test/relay"), null);
  assert.equal(resolvePushBaseURL("wss://example.test/relay?secret=abc"), null);
  assert.equal(resolvePushBaseURL("wss://example.test/relay#join"), null);
  assert.equal(resolvePushBaseURL("ftp://example.test/relay"), null);
  assert.equal(resolvePushBaseURL("not a url"), null);
  // Non-loopback http is rejected: bearer JWTs require TLS.
  assert.equal(resolvePushBaseURL("ws://example.test/relay"), null);
});

test("opt-in requests permission only on Enable and persists a scoped flag, never the token", async () => {
  const h = harness();
  // Denied permission: error, no HTTP, no persisted opt-in.
  (h.driver as unknown as FakeTestDriver).granted = false;
  await h.controller.enable();
  assert.equal(h.controller.getSnapshot().status, "error");
  assert.equal(h.fetch.calls.length, 0);
  assert.deepEqual(h.prefs.dump(), {});

  // Granted: one register with bearer header, scoped pref set, token absent.
  (h.driver as unknown as FakeTestDriver).granted = true;
  await h.controller.enable();
  assert.equal(h.controller.getSnapshot().status, "enabled");
  assert.equal(h.fetch.calls.length, 1);
  const call = h.fetch.calls[0]!;
  assert.ok(call.url.endsWith("/v1/push/register"));
  assert.equal(call.init?.headers?.Authorization, "Bearer test-jwt");
  assert.ok(!call.url.includes("test-jwt"), "JWT must not be in the URL");
  const body = JSON.parse(call.init?.body ?? "{}") as Record<string, unknown>;
  assert.equal(body.platform, "apns");
  assert.equal(body.push_token, "native-device-token-1");
  assert.ok(!("device_id" in body), "mobile must not decode the JWT for device_id");
  const keys = Object.keys(h.prefs.dump());
  assert.equal(keys.length, 1);
  assert.ok(keys[0]!.includes("account-a") && keys[0]!.includes("https://gateway.example"));
  const dumped = JSON.stringify(h.prefs.dump());
  assert.ok(!dumped.includes("native-device-token-1"), "push token must never be persisted");
});

test("stale account async results never enable the wrong generation", async () => {
  const h = harness({ accountId: "account-a" });
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const slowFetch = async (_url: string, _init?: { method?: string; headers?: Record<string, string>; body?: string }) => {
    await gate;
    return {
      ok: true, status: 200,
      async text() { return JSON.stringify({ registered: true, device_id: "device-1", platform: "apns", expires_at: "2030-01-01T00:00:00Z" }); },
    };
  };
  const slow = new PushController({
    prefs: h.prefs,
    getAccountId: () => "account-a",
    getGateway: () => "https://gateway.example",
    getAccessToken: async () => "test-jwt-a",
    driver: h.driver,
    fetchFn: slowFetch,
  });
  const pending = slow.enable();
  // Switch identity before the network resolves.
  release();
  await pending;
  // The slow controller resolved for account-a (its own generation); a new
  // controller for account-b must start disabled, never inherit it.
  const other = new PushController({
    prefs: h.prefs,
    getAccountId: () => "account-b",
    getGateway: () => "https://gateway.example",
    getAccessToken: async () => "test-jwt-b",
    driver: h.driver,
    fetchFn: slowFetch,
  });
  await other.restore();
  assert.equal(other.getSnapshot().status, "disabled");
});

test("foreground renewal re-registers and rotation sends the new token", async () => {
  const h = harness();
  await h.controller.enable();
  assert.equal(h.fetch.calls.length, 1);
  // Within the window: no extra network.
  await h.controller.handleForeground();
  assert.equal(h.fetch.calls.length, 1);
  // Past the renewal window: re-register (rotation covered by re-read).
  h.advance(PUSH_RENEWAL_MS + 1);
  (h.driver as unknown as FakeTestDriver).token = "native-device-token-2";
  await h.controller.handleForeground();
  assert.equal(h.fetch.calls.length, 2);
  const body = JSON.parse(h.fetch.calls[1]!.init?.body ?? "{}") as Record<string, unknown>;
  assert.equal(body.push_token, "native-device-token-2");
});

test("disable and sign-out revoke unregister; unmount cleanup never unregisters", async () => {
  const h = harness();
  await h.controller.enable();
  const registers = h.fetch.calls.length;
  await h.controller.disable();
  assert.equal(h.controller.getSnapshot().status, "disabled");
  assert.equal(h.fetch.calls.length, registers + 1);
  assert.ok(h.fetch.calls.at(-1)!.url.endsWith("/v1/push/unregister"));
  assert.deepEqual(h.prefs.dump(), {});

  // Sign-out drain path: revoke best-effort, then close silences timers.
  await h.controller.enable();
  const beforeRevoke = h.fetch.calls.length;
  await h.controller.revoke();
  assert.equal(h.fetch.calls.length, beforeRevoke + 1);
  const callsBeforeClose = h.fetch.calls.length;
  const permissionBefore = (h.driver as unknown as FakeTestDriver).permissionCalls;
  h.controller.close();
  assert.equal(h.fetch.calls.length, callsBeforeClose);
  assert.equal((h.driver as unknown as FakeTestDriver).permissionCalls, permissionBefore);
});

test("missing or revoked JWT fails closed without success", async () => {
  const denied = harness({ accessToken: null });
  await denied.controller.enable();
  assert.equal(denied.controller.getSnapshot().status, "error");
  assert.equal(denied.fetch.calls.length, 0);

  // Server 401 (revoked): loginRequired error, never enabled.
  const revokedCalls: { url: string }[] = [];
  const revokedFetch = async (url: string) => {
    revokedCalls.push({ url });
    return { ok: false, status: 401, async text() { return JSON.stringify({ error: "authentication required" }); } };
  };
  const revoked = new PushController({
    prefs: memoryPrefs(),
    getAccountId: () => "account-a",
    getGateway: () => "https://gateway.example",
    getAccessToken: async () => "revoked-jwt",
    driver: fakeDriver(),
    fetchFn: revokedFetch,
  });
  await revoked.enable();
  assert.equal(revoked.getSnapshot().status, "error");
  assert.match(revoked.getSnapshot().reason ?? "", /Pair this phone/);
  assert.equal(revokedCalls.length, 1);
});

test("unavailable drivers stay honest with no native or network calls", async () => {
  const driver = fakeDriver({ available: false, unavailableReason: "Push notifications require a development build." });
  const h = harness({ driver });
  await h.controller.enable();
  assert.equal(h.controller.getSnapshot().status, "unavailable");
  assert.match(h.controller.getSnapshot().reason ?? "", /development build/);
  assert.equal((driver as { permissionCalls: number }).permissionCalls, 0);
  assert.equal(h.fetch.calls.length, 0);
});

test("account switch during permission prompt causes no registration request", async () => {
  let grant!: (value: boolean) => void;
  const driver = { ...fakeDriver(), requestPermission: () => new Promise<boolean>((resolve) => { grant = resolve; }) };
  const h = harness({ driver });
  const pending = h.controller.enable();
  h.setAccount("account-b");
  grant(true);
  await pending;
  assert.equal(h.fetch.calls.length, 0);
});

test("closing while resolving credentials cannot send a late registration", async () => {
  let release!: (value: string) => void;
  let entered!: () => void;
  const tokenStarted = new Promise<void>((resolve) => { entered = resolve; });
  const fetch = fakeFetch(() => ({ status: 200, body: { registered: true } }));
  const controller = new PushController({
    prefs: memoryPrefs(), getAccountId: () => "account-a", getGateway: () => "https://gateway.example",
    getAccessToken: () => { entered(); return new Promise<string>((resolve) => { release = resolve; }); },
    driver: fakeDriver(), fetchFn: fetch.fn,
  });
  const pending = controller.enable();
  await tokenStarted;
  controller.close();
  release("successor-credential");
  await pending;
  assert.equal(fetch.calls.length, 0);
});

test("rotation renews only in foreground and listener is retired on close", async () => {
  let foreground = true;
  let detached = 0;
  const events: { rotate?: () => void } = {};
  const native = fakeDriver();
  const fetch = fakeFetch(() => ({ status: 200, body: { registered: true, device_id: "device-1", platform: "apns", expires_at: "2030-01-01T00:00:00Z" } }));
  const controller = new PushController({
    prefs: memoryPrefs(), getAccountId: () => "account-a", getGateway: () => "https://gateway.example",
    getAccessToken: async () => "test-jwt", fetchFn: fetch.fn, isForeground: () => foreground,
    driver: { ...native, onTokenRotation: (listener) => { events.rotate = listener; return () => { detached++; }; } },
  });
  await controller.enable();
  assert.equal(fetch.calls.length, 1);
  assert.ok(events.rotate);
  foreground = false;
  native.token = "native-rotated-token";
  events.rotate();
  await controller.handleForeground();
  assert.equal(fetch.calls.length, 1);
  foreground = true;
  await controller.handleForeground();
  assert.equal(fetch.calls.length, 2);
  assert.equal(JSON.parse(fetch.calls[1]!.init!.body!).push_token, "native-rotated-token");
  controller.close();
  assert.equal(detached, 1);
  events.rotate();
  await controller.handleForeground();
  assert.equal(fetch.calls.length, 2);
});
