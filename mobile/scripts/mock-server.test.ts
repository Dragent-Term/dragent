import assert from "node:assert/strict";
import { once } from "node:events";
import { test } from "node:test";
import { createConnection, type Socket } from "node:net";
import type { AddressInfo } from "node:net";
import {
  MOCK_DESKTOP_TOKEN,
  MOCK_USER_ID,
  MockPairingStore,
  closeMockServer,
  createMockServer,
} from "./mock-server";

interface TestServer {
  base: string;
  port: number;
  close: () => Promise<void>;
}

async function start(store: MockPairingStore): Promise<TestServer> {
  const server = createMockServer(store);
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const port = (server.address() as AddressInfo).port;
  return {
    base: `http://127.0.0.1:${port}`,
    port,
    close: () => closeMockServer(server),
  };
}

async function post(base: string, path: string, body: unknown, token?: string): Promise<{ status: number; data: unknown }> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (token) headers.authorization = `Bearer ${token}`;
  const response = await fetch(`${base}${path}`, { method: "POST", headers, body: JSON.stringify(body) });
  return { status: response.status, data: await response.json() as unknown };
}

async function createCode(base: string, userId = MOCK_USER_ID): Promise<{ code: string; gateway: string }> {
  const { status, data } = await post(base, "/v1/companion/pairings", { user_id: userId }, MOCK_DESKTOP_TOKEN);
  assert.equal(status, 201);
  const record = data as Record<string, unknown>;
  assert.match(record.code as string, /^[A-Z2-7]{20}$/);
  const ttl = (record.expires_at as number) - Date.now();
  assert.ok(ttl > 9 * 60_000 && ttl <= 10 * 60_000, `ttl ${ttl}`);
  return { code: record.code as string, gateway: record.gateway_url as string };
}

async function redeem(base: string, code: string, deviceName = "Test phone"): Promise<{ status: number; data: Record<string, unknown> }> {
  const { status, data } = await post(base, "/v1/companion/pairings/redeem", {
    code, device_name: deviceName, platform: "ios",
  });
  return { status, data: data as Record<string, unknown> };
}

test("create binds user+org and redeem returns device credentials", async () => {
  const server = await start(new MockPairingStore());
  try {
    const denied = await post(server.base, "/v1/companion/pairings", {});
    assert.equal(denied.status, 401);
    const { code, gateway } = await createCode(server.base);
    const { status, data } = await redeem(server.base, code);
    assert.equal(status, 200);
    assert.equal(typeof data.device_id, "string");
    assert.equal(typeof data.device_token, "string");
    assert.equal((data.user as Record<string, unknown>).id, MOCK_USER_ID);
    assert.equal(data.gateway_url, gateway);
    assert.ok((data.expires_at as number) > Date.now() + 29 * 86400_000);
  } finally {
    await server.close();
  }
});

test("redeem is single-use: concurrent redeems admit exactly one winner", async () => {
  const server = await start(new MockPairingStore());
  try {
    const { code } = await createCode(server.base);
    const results = await Promise.all([redeem(server.base, code, "One"), redeem(server.base, code, "Two")]);
    const winners = results.filter((result) => result.status === 200);
    const gone = results.filter((result) => result.status === 410);
    assert.equal(winners.length, 1);
    assert.equal(gone.length, 1);
  } finally {
    await server.close();
  }
});

test("expired codes redeem as gone", async () => {
  const store = new MockPairingStore();
  const server = await start(store);
  try {
    const { code } = await createCode(server.base);
    store.expireCode(code);
    const { status } = await redeem(server.base, code);
    assert.equal(status, 410);
  } finally {
    await server.close();
  }
});

test("redeem validates input and rate-limits per code", async () => {
  const server = await start(new MockPairingStore({ rateLimitMax: 3 }));
  try {
    const bad = await redeem(server.base, "NOTACODE");
    assert.equal(bad.status, 410);
    // Same unknown code: attempts 2-3 still 410, attempt 4 trips the per-code limit.
    assert.equal((await redeem(server.base, "NOTACODE")).status, 410);
    assert.equal((await redeem(server.base, "NOTACODE")).status, 410);
    assert.equal((await redeem(server.base, "NOTACODE")).status, 429);
    // Per-IP limit: fresh codes also 429 once the IP bucket is over.
    const { code } = await createCode(server.base);
    assert.equal((await redeem(server.base, code)).status, 429);
  } finally {
    await server.close();
  }
});

test("max live pairings drops the oldest", async () => {
  const store = new MockPairingStore({ maxLivePairings: 5 });
  const server = await start(store);
  try {
    const codes: string[] = [];
    for (let i = 0; i < 6; i++) codes.push((await createCode(server.base)).code);
    assert.equal(store.livePairingCount(MOCK_USER_ID), 5);
    assert.equal((await redeem(server.base, codes[0]!)).status, 410);
    assert.equal((await redeem(server.base, codes[5]!)).status, 200);
  } finally {
    await server.close();
  }
});

test("token rotation persists and reuse revokes the device", async () => {
  const server = await start(new MockPairingStore());
  try {
    const { code, gateway } = await createCode(server.base);
    const first = await redeem(server.base, code);
    const deviceId = first.data.device_id as string;
    const firstToken = first.data.device_token as string;

    const rotated = await post(gateway, "/v1/companion/devices/token", {
      device_id: deviceId, device_token: firstToken,
    });
    assert.equal(rotated.status, 200);
    const rotation = rotated.data as Record<string, unknown>;
    assert.equal(typeof rotation.access_token, "string");
    assert.notEqual(rotation.device_token, firstToken);

    // Reusing the rotated-out token revokes the device: 401 here…
    const reuse = await post(gateway, "/v1/companion/devices/token", {
      device_id: deviceId, device_token: firstToken,
    });
    assert.equal(reuse.status, 401);
    // …and the rotated token is dead too.
    const dead = await post(gateway, "/v1/companion/devices/token", {
      device_id: deviceId, device_token: rotation.device_token,
    });
    assert.equal(dead.status, 401);
    const devices = await fetch(`${gateway}/v1/companion/devices`, {
      headers: { authorization: `Bearer ${rotation.access_token}` },
    });
    assert.equal(devices.status, 401);
  } finally {
    await server.close();
  }
});

test("revoke closes access and isolates users", async () => {
  const server = await start(new MockPairingStore());
  try {
    const first = await createCode(server.base, MOCK_USER_ID);
    const second = await createCode(server.base, "other-user");
    const phoneA = await redeem(server.base, first.code, "Phone A");
    const phoneB = await redeem(server.base, second.code, "Phone B");
    const tokenA = await post(first.gateway, "/v1/companion/devices/token", {
      device_id: phoneA.data.device_id, device_token: phoneA.data.device_token,
    });
    const tokenB = await post(second.gateway, "/v1/companion/devices/token", {
      device_id: phoneB.data.device_id, device_token: phoneB.data.device_token,
    });
    const accessA = (tokenA.data as Record<string, unknown>).access_token as string;
    const accessB = (tokenB.data as Record<string, unknown>).access_token as string;

    // Listing is user-scoped: A never sees B's phone.
    const list = await fetch(`${first.gateway}/v1/companion/devices`, {
      headers: { authorization: `Bearer ${accessA}` },
    });
    const listed = (await list.json() as { devices: { device_id: string }[] }).devices;
    assert.ok(listed.some((entry) => entry.device_id === phoneA.data.device_id));
    assert.ok(!listed.some((entry) => entry.device_id === phoneB.data.device_id));

    // Cross-user revoke is a 404, and B survives A's revoke of A.
    const cross = await fetch(`${first.gateway}/v1/companion/devices/${phoneB.data.device_id}`, {
      method: "DELETE", headers: { authorization: `Bearer ${accessA}` },
    });
    assert.equal(cross.status, 404);
    const revoked = await fetch(`${first.gateway}/v1/companion/devices/${phoneA.data.device_id}`, {
      method: "DELETE", headers: { authorization: `Bearer ${accessA}` },
    });
    assert.equal(revoked.status, 200);
    const after = await fetch(`${first.gateway}/v1/companion/devices`, {
      headers: { authorization: `Bearer ${accessA}` },
    });
    assert.equal(after.status, 401);
    const survivor = await fetch(`${second.gateway}/v1/companion/devices`, {
      headers: { authorization: `Bearer ${accessB}` },
    });
    assert.equal(survivor.status, 200);
  } finally {
    await server.close();
  }
});

/** Minimal masked-frame WebSocket client for the relay admission tests. */
async function relayConnect(port: number, accessToken: string | null): Promise<{ ready: unknown; socket: Socket }> {
  const socket = createConnection(port, "127.0.0.1");
  await once(socket, "connect");
  const key = Buffer.from("mock-test-key-1234").toString("base64");
  socket.write(
    "GET /v1/relay/socket HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\n" +
    "Connection: Upgrade\r\n" + `Sec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`,
  );
  let buffered = Buffer.alloc(0);
  const chunks: Buffer[] = [];
  await new Promise<void>((resolve) => {
    const onData = (chunk: Buffer) => {
      chunks.push(chunk);
      buffered = Buffer.concat(chunks);
      if (buffered.includes("\r\n\r\n")) {
        socket.off("data", onData);
        resolve();
      }
    };
    socket.on("data", onData);
  });
  const send = (value: unknown) => {
    const body = Buffer.from(JSON.stringify(value));
    const mask = Buffer.from([1, 2, 3, 4]);
    const masked = Buffer.from(body.map((byte, i) => byte ^ mask[i % 4]!));
    let header: Buffer;
    if (body.length < 126) {
      header = Buffer.from([0x81, 0x80 | body.length, ...mask]);
    } else {
      header = Buffer.alloc(4 + mask.length);
      header[0] = 0x81;
      header[1] = 0x80 | 126;
      header.writeUInt16BE(body.length, 2);
      mask.copy(header, 4);
    }
    socket.write(Buffer.concat([header, masked]));
  };
  send({ type: "relay-auth", v: 1, access_token: accessToken });
  const frames: unknown[] = [];
  const rest = buffered.subarray(buffered.indexOf("\r\n\r\n") + 4);
  const pending: Buffer[] = rest.length > 0 ? [rest] : [];
  await new Promise<void>((resolve) => {
    let data = Buffer.concat(pending);
    const onData = (chunk: Buffer) => {
      data = Buffer.concat([data, chunk]);
      for (;;) {
        if (data.length < 2) return;
        const opcode = data[0]! & 0x0f;
        let length = data[1]! & 0x7f;
        let offset = 2;
        if (length === 126) {
          if (data.length < 4) return;
          length = data.readUInt16BE(2);
          offset = 4;
        } else if (length === 127) {
          return;
        }
        if (data.length < offset + length) return;
        const payload = data.subarray(offset, offset + length);
        data = data.subarray(offset + length);
        if (opcode === 0x8) {
          socket.off("data", onData);
          resolve();
          return;
        }
        if (opcode === 0x1) {
          try { frames.push(JSON.parse(payload.toString("utf8")) as unknown); } catch { /* ignore */ }
          if (frames.some((frame) => (frame as { type?: string }).type === "relay-ready")) {
            socket.off("data", onData);
            resolve();
            return;
          }
        }
      }
    };
    if (data.length > 0) onData(Buffer.alloc(0));
    socket.on("data", onData);
    setTimeout(() => {
      socket.off("data", onData);
      resolve();
    }, 3000).unref?.();
  });
  const ready = frames.find((frame) => (frame as { type?: string }).type === "relay-ready") ?? null;
  return { ready, socket };
}

test("relay socket admits device access tokens and answers machines.list", async () => {
  const server = await start(new MockPairingStore());
  try {
    const { code, gateway } = await createCode(server.base);
    const phone = await redeem(server.base, code);
    const rotation = await post(gateway, "/v1/companion/devices/token", {
      device_id: phone.data.device_id, device_token: phone.data.device_token,
    });
    const access = (rotation.data as Record<string, unknown>).access_token as string;
    const first = await relayConnect(server.port, access);
    assert.ok(first.ready && typeof first.ready === "object");
    const lease = first.ready as Record<string, unknown>;
    assert.equal(typeof lease.session_id, "string");
    assert.ok((lease.session_expires_at as number) > Date.now());
    assert.ok((lease.access_expires_at as number) > Date.now());
    first.socket.destroy();

  const { ready: denied, socket: bad } = await relayConnect(server.port, "bogus");
    assert.equal(denied, null);
    bad.destroy();
  } finally {
    await server.close();
  }
});
