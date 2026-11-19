import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPANION_SIGN_OUT_TIMEOUT_MS, createCompanionConnections,
  type CompanionConnection, type CompanionConnectionsClock,
} from "../src/machines/companionConnections";

class Clock implements CompanionConnectionsClock {
  private next = 0;
  readonly timers = new Map<number, () => void>();
  readonly delays: number[] = [];
  setTimeout(callback: () => void, delayMs: number): unknown {
    const id = ++this.next;
    this.timers.set(id, callback);
    this.delays.push(delayMs);
    return id;
  }
  clearTimeout(handle: unknown): void { this.timers.delete(handle as number); }
  expire(): void {
    const pending = [...this.timers.values()];
    this.timers.clear();
    for (const callback of pending) callback();
  }
}

function connection(revoke?: () => Promise<void>) {
  const calls = { revoked: 0, closed: 0 };
  const client: CompanionConnection = {
    revoke: () => { calls.revoked++; return revoke?.() ?? Promise.resolve(); },
    close: () => { calls.closed++; },
  };
  return { client, calls };
}

function pending() {
  let resolve!: () => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<void>((success, failure) => { resolve = success; reject = failure; });
  return { promise, resolve, reject };
}

test("sign-out revokes only its account, closes clients and clears the deadline", async () => {
  const clock = new Clock(), registry = createCompanionConnections(clock);
  const first = connection(), second = connection(), foreign = connection();
  registry.registerCompanionConnection("a", first.client);
  registry.registerCompanionConnection("a", second.client);
  registry.registerCompanionConnection("b", foreign.client);
  await registry.endCompanionConnections("a");
  assert.deepEqual(first.calls, { revoked: 1, closed: 1 });
  assert.deepEqual(second.calls, { revoked: 1, closed: 1 });
  assert.deepEqual(foreign.calls, { revoked: 0, closed: 0 });
  assert.equal(clock.timers.size, 0);
  await registry.endCompanionConnections("b");
  assert.deepEqual(foreign.calls, { revoked: 1, closed: 1 });
});

test("unregister/navigation performs no revocation or closure and cleanup is idempotent", async () => {
  const registry = createCompanionConnections(new Clock()), row = connection();
  const unregister = registry.registerCompanionConnection("a", row.client);
  unregister(); unregister();
  await registry.endCompanionConnections("a");
  assert.deepEqual(row.calls, { revoked: 0, closed: 0 });
});

test("duplicate consumers can unregister independently and a shared client closes once", async () => {
  const registry = createCompanionConnections(new Clock()), row = connection();
  const first = registry.registerCompanionConnection("a", row.client);
  registry.registerCompanionConnection("a", row.client);
  first();
  await registry.endCompanionConnections("a");
  assert.deepEqual(row.calls, { revoked: 1, closed: 1 });
});

test("rejected and synchronously throwing revokes do not prevent other clients closing", async () => {
  const registry = createCompanionConnections(new Clock());
  const rejected = connection(() => Promise.reject(new Error("revoke failed")));
  const throwing = connection(() => { throw new Error("synchronous revoke failure"); });
  const healthy = connection();
  rejected.client.close = () => { rejected.calls.closed++; throw new Error("close failed"); };
  for (const row of [rejected, throwing, healthy]) registry.registerCompanionConnection("a", row.client);
  await registry.endCompanionConnections("a");
  for (const row of [rejected, throwing, healthy]) assert.deepEqual(row.calls, { revoked: 1, closed: 1 });
});

test("revocations start in parallel and a hung revoke cannot exceed the shared deadline", async () => {
  const clock = new Clock(), registry = createCompanionConnections(clock);
  const held = pending(), first = connection(() => held.promise), second = connection();
  registry.registerCompanionConnection("a", first.client);
  registry.registerCompanionConnection("a", second.client);
  const ended = registry.endCompanionConnections("a");
  assert.equal(first.calls.revoked, 1);
  assert.equal(second.calls.revoked, 1);
  assert.equal(first.calls.closed, 0);
  assert.deepEqual(clock.delays, [COMPANION_SIGN_OUT_TIMEOUT_MS]);
  clock.expire(); await ended;
  assert.equal(first.calls.closed, 1);
  assert.equal(second.calls.closed, 1);
  held.reject(new Error("late rejection after timeout"));
  await Promise.resolve();
  assert.equal(clock.timers.size, 0);
});

test("a concurrent second drain cannot revoke or close the captured clients twice", async () => {
  const clock = new Clock(), registry = createCompanionConnections(clock);
  const held = pending(), row = connection(() => held.promise);
  registry.registerCompanionConnection("a", row.client);
  const first = registry.endCompanionConnections("a");
  await registry.endCompanionConnections("a");
  assert.deepEqual(row.calls, { revoked: 1, closed: 0 });
  held.resolve(); await first;
  assert.deepEqual(row.calls, { revoked: 1, closed: 1 });
});

test("new registrations survive an older drain and its stale unregister callback", async () => {
  const clock = new Clock(), registry = createCompanionConnections(clock);
  const held = pending(), previous = connection(() => held.promise), current = connection();
  const oldUnregister = registry.registerCompanionConnection("a", previous.client);
  const oldDrain = registry.endCompanionConnections("a");
  registry.registerCompanionConnection("a", current.client);
  oldUnregister(); held.resolve(); await oldDrain;
  assert.deepEqual(current.calls, { revoked: 0, closed: 0 });
  await registry.endCompanionConnections("a");
  assert.deepEqual(current.calls, { revoked: 1, closed: 1 });
});

test("registration from inside a revoke belongs to the next owner generation", async () => {
  const registry = createCompanionConnections(new Clock()), replacement = connection();
  const original = connection(() => {
    registry.registerCompanionConnection("a", replacement.client);
    return Promise.resolve();
  });
  registry.registerCompanionConnection("a", original.client);
  await registry.endCompanionConnections("a");
  assert.deepEqual(original.calls, { revoked: 1, closed: 1 });
  assert.deepEqual(replacement.calls, { revoked: 0, closed: 0 });
  await registry.endCompanionConnections("a");
  assert.deepEqual(replacement.calls, { revoked: 1, closed: 1 });
});

test("timeout handles remain opaque, including an adapter's undefined handle", async () => {
  let cleared = 0;
  const registry = createCompanionConnections({
    setTimeout: () => undefined,
    clearTimeout: (handle) => { assert.equal(handle, undefined); cleared++; },
  });
  const row = connection();
  registry.registerCompanionConnection("a", row.client);
  await registry.endCompanionConnections("a");
  assert.equal(cleared, 1);
  assert.deepEqual(row.calls, { revoked: 1, closed: 1 });
});
