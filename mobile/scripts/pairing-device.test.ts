import assert from "node:assert/strict";
import { test } from "node:test";
import { DeviceController } from "../src/pairing/controller";
import { deviceCredentialStore, type DeviceCredential } from "../src/pairing/deviceStore";
import type { KeyValueStore } from "../src/storage/types";
import type { PairingFetchFn } from "../src/pairing/api";

function memory(): KeyValueStore {
  const values = new Map<string, string>();
  return {
    getItem: async (key) => values.get(key) ?? null,
    setItem: async (key, value) => { values.set(key, value); },
    removeItem: async (key) => { values.delete(key); },
  };
}

const GATEWAY = "https://api.example.test";
const credential: DeviceCredential = Object.freeze({
  gateway: GATEWAY,
  deviceId: "dev-1",
  deviceToken: "tok-1",
  deviceName: "My phone",
  userId: "u-1",
  displayName: "Ada",
  pairedAt: 1000,
});

test("credentials persist per gateway and reject cross-gateway reads", async () => {
  const storage = memory();
  await deviceCredentialStore(storage, GATEWAY).save(credential);
  assert.deepEqual(await deviceCredentialStore(storage, GATEWAY).load(), credential);
  assert.equal(await deviceCredentialStore(storage, "https://other.example.test").load(), null);
  await deviceCredentialStore(storage, GATEWAY).clear();
  assert.equal(await deviceCredentialStore(storage, GATEWAY).load(), null);
});

test("controller restores, refreshes once, and rotates the stored token", async () => {
  const storage = memory();
  await deviceCredentialStore(storage, GATEWAY).save(credential);
  let calls = 0;
  const fetchFn: PairingFetchFn = async () => {
    calls += 1;
    return {
      status: 200,
      ok: true,
      text: async () => JSON.stringify({
        access_token: `a-${calls}`, access_expires_at: Date.now() + 3600_000,
        device_token: `tok-${calls + 1}`, expires_at: Date.now() + 30 * 86400_000,
      }),
    };
  };
  const controller = new DeviceController({ fetchFn, storage });
  await controller.restore(GATEWAY);
  assert.equal(controller.getSnapshot().status, "paired");
  const [first, second] = await Promise.all([controller.getAccessToken(), controller.getAccessToken()]);
  assert.equal(first, "a-1");
  assert.equal(second, "a-1");
  assert.equal(calls, 1);
  const saved = await deviceCredentialStore(storage, GATEWAY).load();
  assert.equal(saved?.deviceToken, "tok-2");
  controller.dispose();
});

test("unauthorized refresh rejects the pairing exactly once", async () => {
  const storage = memory();
  await deviceCredentialStore(storage, GATEWAY).save(credential);
  let calls = 0;
  const fetchFn: PairingFetchFn = async () => {
    calls += 1;
    return { status: 401, ok: false, text: async () => JSON.stringify({ error: "revoked" }) };
  };
  const controller = new DeviceController({ fetchFn, storage });
  await controller.restore(GATEWAY);
  const [first, second] = await Promise.all([controller.getAccessToken(), controller.getAccessToken()]);
  assert.equal(first, null);
  assert.equal(second, null);
  assert.equal(calls, 1);
  assert.equal(controller.getSnapshot().status, "unpaired");
  assert.match(controller.getSnapshot().error ?? "", /unpaired/i);
  assert.equal(await deviceCredentialStore(storage, GATEWAY).load(), null);
  controller.dispose();
});

test("completePairing adopts credentials and forget clears them", async () => {
  const storage = memory();
  const now = Date.now();
  const controller = new DeviceController({
    fetchFn: async () => { throw new Error("must not call network"); },
    storage,
    now: () => now,
  });
  await controller.restore(GATEWAY);
  assert.equal(controller.getSnapshot().status, "unpaired");
  const adopted = await controller.completePairing({
    deviceId: "dev-9", deviceToken: "tok-9",
    user: { id: "u-9", displayName: "Bo" },
    gatewayUrl: GATEWAY, expiresAt: now + 1000,
  }, "Bo's phone");
  assert.equal(adopted, true);
  assert.equal(controller.getSnapshot().device?.deviceId, "dev-9");
  await controller.forget();
  assert.equal(controller.getSnapshot().status, "unpaired");
  assert.equal(await deviceCredentialStore(storage, GATEWAY).load(), null);
  controller.dispose();
});
