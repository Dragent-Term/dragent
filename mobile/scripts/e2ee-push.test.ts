// §8 / §6.6 phone app side: push context, push-key rotation, reconcile on open.
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  PUSH_ROTATION_MS,
  PushKeySync,
  parseDoneReply,
  parseRotationRecord,
  reconcileDone,
  rotationKey,
  attachPushChannel,
  onChannelOpen,
  type PushKeyNative,
} from "../src/notifications/e2eePush";

const ACCOUNT = "11111111-1111-4111-8111-111111111111";
const PHONE = "22222222-2222-4222-8222-222222222222";
const D1 = "33333333-3333-4333-8333-333333333333";
const D2 = "44444444-4444-4444-8444-444444444444";
const DAY = 24 * 60 * 60 * 1000;

function memoryPrefs() {
  const map = new Map<string, string>();
  return {
    map,
    getItem: async (k: string) => map.get(k) ?? null,
    setItem: async (k: string, v: string) => { map.set(k, v); },
    removeItem: async (k: string) => { map.delete(k); },
  };
}

function fakeNative(initial: number | null = 1) {
  let key: number | null = initial;
  const calls: string[] = [];
  const native: PushKeyNative = {
    pushPublicKey: () => (key === null ? null : new Uint8Array(32).fill(key)),
    rotatePushKey: (now) => { calls.push(`rotate@${now}`); key = (key ?? 0) + 1; return new Uint8Array(32).fill(key); },
    setPushContext: (a, p) => { calls.push(`context:${a}:${p}`); },
  };
  return { native, calls, current: () => key };
}

test("the push context is the account and this phone", () => {
  const { native, calls } = fakeNative();
  new PushKeySync({ native, prefs: memoryPrefs(), account: ACCOUNT, phone: PHONE, pairedDesktops: () => [] }).ensureContext();
  assert.deepEqual(calls, [`context:${ACCOUNT}:${PHONE}`]);
});

test("the push key rotates every 180 days and each desktop is told once, inside its channel", async () => {
  const prefs = memoryPrefs();
  const { native, calls, current } = fakeNative();
  let now = 1_000 * DAY;
  const sync = new PushKeySync({ native, prefs, account: ACCOUNT, phone: PHONE, pairedDesktops: () => [D1, D2], now: () => now });
  // First run records today; nothing rotates.
  assert.equal(await sync.maybeRotate(), false);
  now += PUSH_ROTATION_MS - 1;
  assert.equal(await sync.maybeRotate(), false);
  assert.equal(await sync.announce(D1, () => true), false, "nothing pending before a rotation");
  now += 1;
  assert.equal(await sync.maybeRotate(), true);
  assert.deepEqual(calls, [`rotate@${now}`]);
  assert.equal(current(), 2);
  assert.ok(await sync.isPending(D1));
  assert.ok(await sync.isPending(D2));
  // D1's channel opens but the send fails: still pending.
  assert.equal(await sync.announce(D1, () => false), false);
  assert.ok(await sync.isPending(D1));
  // Then it works, with the new key.
  const sent: number[] = [];
  assert.equal(await sync.announce(D1, (k) => { sent.push(k[0]!); return true; }), true);
  assert.deepEqual(sent, [2]);
  assert.equal(await sync.isPending(D1), false);
  assert.equal(await sync.announce(D1, () => true), false, "told once");
  assert.ok(await sync.isPending(D2));
  // Not due again for another 180 days.
  assert.equal(await sync.maybeRotate(), false);
});

test("a phone without a push key never rotates", async () => {
  const { native, calls } = fakeNative(null);
  const sync = new PushKeySync({ native, prefs: memoryPrefs(), account: ACCOUNT, phone: PHONE, pairedDesktops: () => [D1] });
  assert.equal(await sync.maybeRotate(), false);
  assert.deepEqual(calls, []);
});

test("unpaired desktops drop out of the pending list", async () => {
  const prefs = memoryPrefs();
  const { native } = fakeNative();
  let paired = [D1, D2];
  let now = 0;
  const sync = new PushKeySync({ native, prefs, account: ACCOUNT, phone: PHONE, pairedDesktops: () => paired, now: () => now });
  await sync.maybeRotate();
  now = PUSH_ROTATION_MS;
  await sync.maybeRotate();
  paired = [D1];
  await sync.announce(D1, () => true);
  assert.equal(await sync.isPending(D2), false);
});

test("rotation records are strict", () => {
  assert.equal(parseRotationRecord(null), null);
  assert.equal(parseRotationRecord("nope"), null);
  assert.equal(parseRotationRecord(JSON.stringify({ v: 2, rotatedAt: 1, pending: [] })), null);
  assert.equal(parseRotationRecord(JSON.stringify({ v: 1, rotatedAt: -1, pending: [] })), null);
  assert.equal(parseRotationRecord(JSON.stringify({ v: 1, rotatedAt: 1, pending: ["x"] })), null);
  assert.deepEqual(parseRotationRecord(JSON.stringify({ v: 1, rotatedAt: 5, pending: [D1, D1] })), { v: 1, rotatedAt: 5, pending: [D1] });
  assert.equal(rotationKey(ACCOUNT, PHONE), `dragent.push.rotation.v1:${ACCOUNT}:${PHONE}`);
});

const COLLAPSE = "AbCdEfGhIjKlMnOpQrSt_-";
const TAG = "0123456789abcdef0123456789abcdef";

test("notifications.done replies are strict", () => {
  assert.deepEqual(parseDoneReply({ ids: [COLLAPSE], tags: [TAG] }), { ids: [COLLAPSE], tags: [TAG] });
  for (const bad of [null, [], "x", { ids: [] }, { ids: [], tags: [], x: 1 }, { ids: ["short"], tags: [] }, { ids: [], tags: ["XYZ"] }, { ids: [1], tags: [] }, { ids: Array(257).fill(COLLAPSE), tags: [] }]) {
    assert.equal(parseDoneReply(bad), null, JSON.stringify(bad));
  }
});

test("reconcile on open removes handled notifications by collapse id on iOS and by tag on Android", async () => {
  const requests: [string, unknown][] = [];
  const rpc = { request: async <T,>(method: string, params?: unknown) => { requests.push([method, params]); return { ids: [COLLAPSE], tags: [TAG] } as T; } };
  const removed: string[][] = [];
  assert.equal(await reconcileDone(rpc, "ios", async (ids) => { removed.push([...ids]); }), 1);
  assert.equal(await reconcileDone(rpc, "android", async (ids) => { removed.push([...ids]); }), 1);
  assert.deepEqual(requests, [["notifications.done", { platform: "ios" }], ["notifications.done", { platform: "android" }]]);
  assert.deepEqual(removed, [[COLLAPSE], [TAG]]);
});

test("reconcile failures are quiet and remove nothing", async () => {
  let dismissed = false;
  const dismiss = async () => { dismissed = true; };
  assert.equal(await reconcileDone({ request: async () => { throw new Error("gone"); } }, "ios", dismiss), 0);
  assert.equal(await reconcileDone({ request: async <T,>() => ({ ids: "x" }) as T }, "ios", dismiss), 0);
  assert.equal(await reconcileDone({ request: async <T,>() => ({ ids: [], tags: [] }) as T }, "ios", dismiss), 0);
  assert.equal(dismissed, false);
  assert.equal(await reconcileDone({ request: async <T,>() => ({ ids: [COLLAPSE], tags: [] }) as T }, "ios", async () => { throw new Error("x"); }), 0);
});

function fakeSocket() {
  const listeners = new Set<() => void>();
  let state = "connecting";
  return {
    set(next: string) { state = next; for (const l of listeners) l(); },
    subscribe: (l: () => void) => { listeners.add(l); return () => { listeners.delete(l); }; },
    getSnapshot: () => ({ secure: { state } }),
    listeners,
  };
}

const flush = () => new Promise((r) => setTimeout(r, 10));

test("onChannelOpen fires on each transition into open, and stops when unsubscribed", () => {
  const s = fakeSocket();
  let n = 0;
  const stop = onChannelOpen(s, () => { n += 1; });
  s.set("open");
  s.set("open");
  s.set("retrying");
  s.set("open");
  assert.equal(n, 2);
  stop();
  assert.equal(s.listeners.size, 0);
});

test("a desktop channel opening writes the context, announces a pending push key, then reconciles", async () => {
  const prefs = memoryPrefs();
  const { native, calls } = fakeNative();
  let now = 0;
  const sync = new PushKeySync({ native, prefs, account: ACCOUNT, phone: PHONE, pairedDesktops: () => [D1], now: () => now });
  await sync.maybeRotate();
  now = PUSH_ROTATION_MS;
  const s = fakeSocket();
  const order: string[] = [];
  const client = {
    ...s,
    rpc: { request: async <T,>(method: string) => { order.push(method); return { ids: [COLLAPSE], tags: [] } as T; } },
    announcePushKey: (k: Uint8Array) => { order.push(`announce:${k[0]}`); return true; },
  };
  const removed: string[] = [];
  const stop = attachPushChannel(client, D1, { sync, platform: "ios", dismiss: async (ids) => { removed.push(...ids); }, now: () => now });
  s.set("open");
  await flush();
  assert.ok(calls.includes(`context:${ACCOUNT}:${PHONE}`));
  assert.ok(calls.includes(`rotate@${PUSH_ROTATION_MS}`));
  assert.deepEqual(order, ["announce:2", "notifications.done"]);
  assert.deepEqual(removed, [COLLAPSE]);
  // A rekey reopens within 30 s: no second announce, no second reconcile.
  s.set("connecting");
  s.set("open");
  await flush();
  assert.deepEqual(order, ["announce:2", "notifications.done"]);
  // Later it reconciles again.
  now += 30_000;
  s.set("connecting");
  s.set("open");
  await flush();
  assert.deepEqual(order, ["announce:2", "notifications.done", "notifications.done"]);
  stop();
  assert.equal(s.listeners.size, 0);
});
