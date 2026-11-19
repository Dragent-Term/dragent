import assert from "node:assert/strict";
import { test } from "node:test";
import {
  PairingError,
  listPairedPhones,
  redeemPairingCode,
  refreshDeviceToken,
  revokePairedPhone,
  type PairingFetchFn,
} from "../src/pairing/api";

function json(status: number, body: unknown): { ok: boolean; status: number; text: () => Promise<string> } {
  return { ok: status >= 200 && status < 300, status, text: async () => JSON.stringify(body) };
}

test("redeem sends no device name, only code and platform", async () => {
  const fetchFn: PairingFetchFn = async (url, init) => {
    assert.match(url, /\/v1\/companion\/pairings\/redeem$/);
    assert.equal(init?.method, "POST");
    const body = JSON.parse(init?.body ?? "{}") as Record<string, unknown>;
    assert.deepEqual(Object.keys(body).sort(), ["code", "platform"]);
    assert.equal(body.code, "CODE");
    assert.equal(body.platform, "ios");
    return json(200, {
      device_id: "dev-1", device_token: "tok-1",
      user: { id: "u-1", display_name: "Ada" },
      gateway_url: "https://api.example.test", expires_at: 999,
    });
  };
  const result = await redeemPairingCode(fetchFn, "https://api.example.test", {
    code: "CODE", platform: "ios",
  });
  assert.equal(result.deviceId, "dev-1");
  assert.equal(result.user.displayName, "Ada");
  assert.equal(result.expiresAt, 999);
});

test("redeem maps gone, rate-limited and offline", async () => {
  const gone: PairingFetchFn = async () => json(410, { error: "used" });
  await assert.rejects(
    redeemPairingCode(gone, "https://g.test", { code: "C", platform: "android" }),
    (error: unknown) => error instanceof PairingError && error.kind === "gone",
  );
  const limited: PairingFetchFn = async () => json(429, {});
  await assert.rejects(
    redeemPairingCode(limited, "https://g.test", { code: "C", platform: "ios" }),
    (error: unknown) => error instanceof PairingError && error.kind === "rate_limited",
  );
  const down: PairingFetchFn = async () => { throw new Error("boom"); };
  await assert.rejects(
    redeemPairingCode(down, "https://g.test", { code: "C", platform: "ios" }),
    (error: unknown) => error instanceof PairingError && error.kind === "offline",
  );
});

test("token refresh rotates and maps unauthorized", async () => {
  const fetchFn: PairingFetchFn = async (url) => {
    assert.match(url, /\/v1\/companion\/devices\/token$/);
    return json(200, {
      access_token: "a-2", access_expires_at: 200,
      device_token: "tok-2", expires_at: 300,
    });
  };
  const result = await refreshDeviceToken(fetchFn, "https://g.test", {
    deviceId: "dev-1", deviceToken: "tok-1",
  });
  assert.equal(result.accessToken, "a-2");
  assert.equal(result.deviceToken, "tok-2");

  const revoked: PairingFetchFn = async () => json(401, {});
  await assert.rejects(
    refreshDeviceToken(revoked, "https://g.test", { deviceId: "dev-1", deviceToken: "tok-1" }),
    (error: unknown) => error instanceof PairingError && error.kind === "unauthorized",
  );
});

test("devices list skips malformed rows and revoke accepts 204", async () => {
  const list: PairingFetchFn = async (url, init) => {
    assert.match(url, /\/v1\/companion\/devices$/);
    assert.equal(init?.headers?.Authorization, "Bearer a-1");
    return json(200, { devices: [
      { device_id: "dev-1", name: "Phone", platform: "ios", created_at: 10, last_seen_at: 20 },
      { device_id: "", name: "Bad" },
    ] });
  };
  const phones = await listPairedPhones(list, "https://g.test", "a-1");
  assert.equal(phones.length, 1);
  assert.equal(phones[0]?.name, "Phone");
  assert.equal(phones[0]?.lastSeenAt, 20);

  const revoke: PairingFetchFn = async (url, init) => {
    assert.match(url, /\/v1\/companion\/devices\/dev-1$/);
    assert.equal(init?.method, "DELETE");
    return { status: 204, ok: true, text: async () => "" };
  };
  await revokePairedPhone(revoke, "https://g.test", "a-1", "dev-1");
});

test("gateway RFC3339 timestamps work for redeem, refresh and device listing", async () => {
  const date = "2026-11-07T12:34:56Z";
  const expected = Date.parse(date);
  const redeemed = await redeemPairingCode(async () => json(200, {
    device_id: "dev-1", device_token: "tok-1", user: { id: "u-1" },
    gateway_url: "https://g.test", expires_at: date,
  }), "https://g.test", { code: "C", platform: "ios" });
  assert.equal(redeemed.expiresAt, expected);
  const refreshed = await refreshDeviceToken(async () => json(200, {
    access_token: "a-1", device_token: "tok-2", access_expires_at: date, expires_at: date,
  }), "https://g.test", { deviceId: "dev-1", deviceToken: "tok-1" });
  assert.equal(refreshed.accessExpiresAt, expected);
  assert.equal(refreshed.expiresAt, expected);
  const phones = await listPairedPhones(async () => json(200, { devices: [
    { device_id: "dev-1", name: "Phone", platform: "ios", created_at: date, last_seen_at: date },
  ] }), "https://g.test", "a-1");
  assert.equal(phones[0]?.createdAt, expected);
  assert.equal(phones[0]?.lastSeenAt, expected);
});

test("redeem still rejects malformed expiration timestamps", async () => {
  for (const expires_at of [null, "tomorrow", "123", 0, -1]) {
    await assert.rejects(redeemPairingCode(async () => json(200, {
      device_id: "dev-1", device_token: "tok-1", user: { id: "u-1" },
      gateway_url: "https://g.test", expires_at,
    }), "https://g.test", { code: "C", platform: "ios" }),
    (error: unknown) => error instanceof PairingError && error.kind === "invalid");
  }
});
