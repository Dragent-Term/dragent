import assert from "node:assert/strict";
import { test } from "node:test";
import { companionSessionRepository } from "../src/machines/companionSessionRepository";
import type { KeyValueStore } from "../src/storage/types";

function memory(): KeyValueStore {
  const values = new Map<string, string>();
  return {
    getItem: async (key) => values.get(key) ?? null,
    setItem: async (key, value) => { values.set(key, value); },
    removeItem: async (key) => { values.delete(key); },
  };
}
const owner = { accountId: "alice", gateway: "https://relay.invalid", deviceId: "desktop" };
const lease = { sessionId: "1234-abcd", expiresAt: 1000 };

test("lease persistence is isolated by account, gateway and desktop", async () => {
  const storage = memory();
  const repo = companionSessionRepository(storage, owner);
  await repo.saveSession(lease);
  assert.deepEqual(await companionSessionRepository(storage, owner).loadSession(), lease);
  for (const other of [{ ...owner, accountId: "bob" }, { ...owner, gateway: "https://other.invalid" }, { ...owner, deviceId: null }]) {
    assert.equal(await companionSessionRepository(storage, other).loadSession(), null);
  }
});

test("expired lease is retained so restarting cannot silently reset its deadline", async () => {
  const repo = companionSessionRepository(memory(), owner);
  await repo.saveSession(lease);
  assert.deepEqual(await repo.loadSession(), lease);
  await repo.clearSession();
  assert.equal(await repo.loadSession(), null);
});

test("queued save and explicit clear cannot resurrect a retired lease", async () => {
  const repo = companionSessionRepository(memory(), owner);
  const save = repo.saveSession(lease);
  const clear = repo.clearSession();
  await Promise.all([save, clear]);
  assert.equal(await repo.loadSession(), null);
  await assert.rejects(repo.saveSession({ sessionId: "bad/id", expiresAt: 1000 }));
});
