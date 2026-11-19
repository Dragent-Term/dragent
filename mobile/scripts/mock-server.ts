/**
 * Pairing + relay mock for mobile development and tests.
 *
 * Implements the phone-pairing contract against loopback only:
 * - POST /v1/companion/pairings            (desktop bearer, test fixture)
 * - POST /v1/companion/pairings/redeem     (unauthenticated, rate-limited)
 * - POST /v1/companion/devices/token       (rotating device token)
 * - GET  /v1/companion/devices             (access token)
 * - DELETE /v1/companion/devices/{id}      (access token, revokes + closes sockets)
 * - WS   /v1/relay/socket                  (relay-auth admission, machines.list, catalog stream)
 *
 * No real desktop, no real auth, no E2EE: crypto-random fixtures bound to
 * loopback. Device tokens are stored hashed; access tokens are opaque.
 *
 *   pnpm mock-server                  # listens on 6769
 *   PORT=6771 pnpm mock-server
 */

import { createHash, randomBytes } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { Socket } from "node:net";

const PORT = Number(process.env.PORT ?? 6769);
const GUID = "258EAFA5-E914-47DA-CA5AB0DC85B1";
const MAX_FRAME_BYTES = 65_535;
const MAX_BODY_BYTES = 64 * 1024;

export const MOCK_USER_ID = "mock-user-1";
export const MOCK_ORG_ID = "mock-org-1";
/** Desktop-side fixture bearer accepted by the mock create endpoint. */
export const MOCK_DESKTOP_TOKEN = "mock-desktop-token";

export interface MockPairingStoreOptions {
  now?: () => number;
  /** Pairing code TTL; default 10 minutes. */
  codeTTLms?: number;
  /** Device lifetime; default 30 days. */
  deviceTTLms?: number;
  /** Access token lifetime; default 15 minutes. */
  accessTTLms?: number;
  /** Max live pairings per user; default 5, oldest dropped. */
  maxLivePairings?: number;
  /** Redeem rate-limit window; default 60s. */
  rateLimitWindowMs?: number;
  /** Redeem attempts per window (per IP and per code); default 30. */
  rateLimitMax?: number;
}

export interface CreatedPairing {
  readonly pairingId: string;
  readonly code: string;
  readonly expiresAt: number;
}

export interface RedeemedDevice {
  readonly deviceId: string;
  readonly deviceToken: string;
  readonly userId: string;
  readonly displayName: string;
  readonly expiresAt: number;
}

export interface DeviceTokenRotation {
  readonly accessToken: string;
  readonly accessExpiresAt: number;
  readonly deviceToken: string;
  readonly expiresAt: number;
}

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

function randomCode(): string {
  const bytes = randomBytes(20);
  let code = "";
  for (let i = 0; i < 20; i++) code += BASE32[(bytes[i] ?? 0) & 31];
  return code;
}

function randomToken(prefix: string): string {
  return `${prefix}_${randomBytes(24).toString("hex")}`;
}

function hashToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

interface PairingRecord {
  pairingId: string;
  code: string;
  userId: string;
  orgId: string;
  createdAt: number;
  expiresAt: number;
  redeemedDeviceId: string | null;
}

interface DeviceRecord {
  deviceId: string;
  userId: string;
  orgId: string;
  name: string;
  platform: string;
  tokenHash: string;
  createdAt: number;
  expiresAt: number;
  lastSeenAt: number;
  revoked: boolean;
}

interface AccessRecord {
  token: string;
  deviceId: string;
  expiresAt: number;
}

/**
 * In-memory pairing store. Redeem is synchronous from check to mark, so two
 * concurrent redeems of one code admit exactly one winner (single use).
 */
export class MockPairingStore {
  private readonly now: () => number;
  private readonly codeTTLms: number;
  private readonly deviceTTLms: number;
  private readonly accessTTLms: number;
  private readonly maxLivePairings: number;
  private readonly rateLimitWindowMs: number;
  private readonly rateLimitMax: number;
  private readonly pairings = new Map<string, PairingRecord>();
  private readonly devices = new Map<string, DeviceRecord>();
  private readonly access = new Map<string, AccessRecord>();
  private readonly attempts = new Map<string, number[]>();
  private counter = 0;

  constructor(options: MockPairingStoreOptions = {}) {
    this.now = options.now ?? Date.now;
    this.codeTTLms = options.codeTTLms ?? 10 * 60_000;
    this.deviceTTLms = options.deviceTTLms ?? 30 * 86400_000;
    this.accessTTLms = options.accessTTLms ?? 15 * 60_000;
    this.maxLivePairings = options.maxLivePairings ?? 5;
    this.rateLimitWindowMs = options.rateLimitWindowMs ?? 60_000;
    this.rateLimitMax = options.rateLimitMax ?? 30;
  }

  private id(prefix: string): string {
    this.counter += 1;
    return `${prefix}-${this.counter}-${randomBytes(8).toString("hex")}`;
  }

  createPairing(userId = MOCK_USER_ID, orgId = MOCK_ORG_ID): CreatedPairing {
    const now = this.now();
    const pairing: PairingRecord = {
      pairingId: this.id("pairing"),
      code: randomCode(),
      userId,
      orgId,
      createdAt: now,
      expiresAt: now + this.codeTTLms,
      redeemedDeviceId: null,
    };
    // Max live pairings per user: oldest dropped.
    const live = [...this.pairings.values()]
      .filter((entry) => entry.userId === userId && entry.expiresAt > now && entry.redeemedDeviceId === null)
      .sort((a, b) => a.createdAt - b.createdAt);
    while (live.length >= this.maxLivePairings) {
      const oldest = live.shift();
      if (oldest) this.pairings.delete(oldest.code);
    }
    this.pairings.set(pairing.code, pairing);
    return { pairingId: pairing.pairingId, code: pairing.code, expiresAt: pairing.expiresAt };
  }

  /** Test hook: force a code to read as expired. */
  expireCode(code: string): void {
    const pairing = this.pairings.get(code.toUpperCase());
    if (pairing) pairing.expiresAt = this.now() - 1;
  }

  private limited(key: string): boolean {
    const now = this.now();
    const windowStart = now - this.rateLimitWindowMs;
    const hits = (this.attempts.get(key) ?? []).filter((at) => at > windowStart);
    hits.push(now);
    this.attempts.set(key, hits);
    return hits.length > this.rateLimitMax;
  }

  redeem(code: string, deviceName: string, platform: string, ip: string):
    | { status: 200; device: RedeemedDevice }
    | { status: 400 | 410 | 429; error: string } {
    const normalized = code.toUpperCase();
    if (this.limited(`ip:${ip}`) || this.limited(`code:${normalized}`)) {
      return { status: 429, error: "Too many attempts. Wait a little and try again." };
    }
    const pairing = this.pairings.get(normalized);
    if (!pairing || pairing.expiresAt <= this.now() || pairing.redeemedDeviceId !== null) {
      return { status: 410, error: "This code has already been used or has expired." };
    }
    if (!deviceName || deviceName.length > 64) {
      return { status: 400, error: "Name this phone in 64 characters or fewer." };
    }
    if (platform !== "ios" && platform !== "android" && platform !== "web") {
      return { status: 400, error: "The platform is invalid." };
    }
    // Atomic single use: marked before any other work.
    const now = this.now();
    // The presented token is the only copy; only its hash is stored.
    const presented = randomToken("devtok");
    const device: DeviceRecord = {
      deviceId: this.id("device"),
      userId: pairing.userId,
      orgId: pairing.orgId,
      name: deviceName,
      platform,
      tokenHash: hashToken(presented),
      createdAt: now,
      expiresAt: now + this.deviceTTLms,
      lastSeenAt: now,
      revoked: false,
    };
    pairing.redeemedDeviceId = device.deviceId;
    this.devices.set(device.deviceId, device);
    return {
      status: 200,
      device: {
        deviceId: device.deviceId,
        deviceToken: presented,
        userId: device.userId,
        displayName: `Mock user ${device.userId}`,
        expiresAt: device.expiresAt,
      },
    };
  }

  refresh(deviceId: string, deviceToken: string):
    | { status: 200; rotation: DeviceTokenRotation }
    | { status: 400 | 401; error: string } {
    const device = this.devices.get(deviceId);
    if (!device || device.revoked || device.expiresAt <= this.now()) {
      return { status: 401, error: "This phone's pairing is no longer valid." };
    }
    // Reusing a rotated-out token revokes the device.
    if (device.tokenHash !== hashToken(deviceToken)) {
      device.revoked = true;
      for (const [token, record] of this.access) {
        if (record.deviceId === deviceId) this.access.delete(token);
      }
      return { status: 401, error: "This phone's pairing is no longer valid." };
    }
    const now = this.now();
    const presented = randomToken("devtok");
    device.tokenHash = hashToken(presented);
    device.lastSeenAt = now;
    const accessToken = randomToken("mockat");
    const access: AccessRecord = { token: accessToken, deviceId, expiresAt: now + this.accessTTLms };
    this.access.set(accessToken, access);
    return {
      status: 200,
      rotation: {
        accessToken,
        accessExpiresAt: access.expiresAt,
        deviceToken: presented,
        expiresAt: device.expiresAt,
      },
    };
  }

  deviceForAccess(accessToken: string): DeviceRecord | null {
    const record = this.access.get(accessToken);
    if (!record || record.expiresAt <= this.now()) {
      if (record) this.access.delete(accessToken);
      return null;
    }
    const device = this.devices.get(record.deviceId);
    if (!device || device.revoked || device.expiresAt <= this.now()) return null;
    return device;
  }

  listDevices(accessToken: string): DeviceRecord[] | null {
    const caller = this.deviceForAccess(accessToken);
    if (!caller) return null;
    const now = this.now();
    return [...this.devices.values()].filter(
      (device) => device.userId === caller.userId && !device.revoked && device.expiresAt > now,
    );
  }

  revokeDevice(accessToken: string, deviceId: string): "ok" | "unauthorized" | "missing" {
    const caller = this.deviceForAccess(accessToken);
    if (!caller) return "unauthorized";
    const device = this.devices.get(deviceId);
    if (!device || device.userId !== caller.userId) return "missing";
    device.revoked = true;
    for (const [token, record] of this.access) {
      if (record.deviceId === deviceId) this.access.delete(token);
    }
    return "ok";
  }

  livePairingCount(userId: string): number {
    const now = this.now();
    return [...this.pairings.values()].filter(
      (entry) => entry.userId === userId && entry.expiresAt > now && entry.redeemedDeviceId === null,
    ).length;
  }
}

/** Encode an unmasked server-to-client frame. */
function serverFrame(opcode: number, body: Buffer): Buffer {
  const headerLen = body.length < 126 ? 2 : 4;
  const header = Buffer.alloc(headerLen);
  header[0] = 0x80 | opcode; // FIN + opcode
  if (body.length < 126) {
    header[1] = body.length;
  } else {
    header[1] = 126;
    header.writeUInt16BE(body.length, 2);
  }
  return Buffer.concat([header, body]);
}

function textFrame(value: unknown): Buffer {
  return serverFrame(0x1, Buffer.from(JSON.stringify(value), "utf8"));
}

/** Decode a masked, unfragmented client frame; null means more bytes needed. */
function parseClientFrame(buf: Buffer): {
  opcode: number;
  payload: Buffer;
  rest: Buffer;
} | null {
  if (buf.length < 2) return null;
  const opcode = buf[0]! & 0x0f;
  const masked = (buf[1]! & 0x80) !== 0;
  if (!masked || (buf[0]! & 0xf0) !== 0x80 || ![0x1, 0x8, 0x9, 0xa].includes(opcode)) {
    throw new Error("Mock requires masked, unfragmented text or control frames");
  }
  let length = buf[1]! & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buf.length < 4) return null;
    length = buf.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    throw new Error("Mock frame is too large");
  }
  if (opcode >= 0x8 && (length > 125 || (opcode === 0x8 && length === 1))) {
    throw new Error("Invalid control frame");
  }
  if (buf.length < offset + 4 + length) return null;
  const payload = Buffer.from(buf.subarray(offset + 4, offset + 4 + length));
  for (let i = 0; i < payload.length; i++) {
    payload[i] = payload[i]! ^ buf[offset + (i % 4)]!;
  }
  return { opcode, payload, rest: buf.subarray(offset + 4 + length) };
}

function acceptKey(key: string): string {
  return createHash("sha1").update(key + GUID).digest("base64");
}

function sampleMachine(now: number): Record<string, unknown> {
  return {
    device_id: "mock-desktop-1",
    name: "Mock desktop",
    platform: "darwin",
    version: "mock",
    last_seen_at: now,
    online: true,
  };
}

export interface MockRelayOptions {
  store: MockPairingStore;
  now?: () => number;
  /** Called when a device is revoked so the server can close its sockets. */
  trackSocket?: (deviceId: string, close: () => void) => void;
}

const trackedConnections = new WeakMap<Server, Set<Socket>>();

export function createMockServer(store: MockPairingStore, options: { now?: () => number } = {}): Server {
  const now = options.now ?? Date.now;
  // Upgraded relay sockets never release server.close() on their own
  // (half-open after the client goes away), so every connection is tracked
  // for closeMockServer.
  const connections = new Set<Socket>();
  const liveSockets = new Map<string, Set<Socket>>();
  const track = (deviceId: string, socket: Socket) => {
    let set = liveSockets.get(deviceId);
    if (!set) {
      set = new Set();
      liveSockets.set(deviceId, set);
    }
    set.add(socket);
    socket.on("close", () => {
      const owned = liveSockets.get(deviceId);
      if (owned) {
        owned.delete(socket);
        if (owned.size === 0) liveSockets.delete(deviceId);
      }
    });
  };
  const closeDeviceSockets = (deviceId: string) => {
    for (const socket of liveSockets.get(deviceId) ?? []) {
      try {
        socket.end(serverFrame(0x8, Buffer.alloc(0)));
      } catch { /* already gone */ }
    }
  };

  function sendJson(res: ServerResponse, status: number, body: unknown): void {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(body));
  }

  function readBody(req: IncomingMessage): Promise<unknown> {
    return new Promise((resolve) => {
      const chunks: Buffer[] = [];
      let size = 0;
      req.on("data", (chunk: Buffer) => {
        size += chunk.length;
        if (size <= MAX_BODY_BYTES) chunks.push(chunk);
      });
      req.on("end", () => {
        try {
          resolve(chunks.length > 0 ? (JSON.parse(Buffer.concat(chunks).toString("utf8")) as unknown) : {});
        } catch {
          resolve(null);
        }
      });
      req.on("error", () => resolve(null));
    });
  }

  const bearer = (req: IncomingMessage): string | null => {
    const header = req.headers.authorization;
    if (!header || !header.startsWith("Bearer ")) return null;
    const token = header.slice("Bearer ".length).trim();
    return token || null;
  };

  const server = createServer(async (req, res) => {    const url = new URL(req.url ?? "/", "http://mock");
    try {
      if (req.method === "POST" && url.pathname === "/v1/companion/pairings") {
        const token = bearer(req);
        if (!token) {
          sendJson(res, 401, { error: "Desktop sign-in is required to pair a phone." });
          return;
        }
        const body = await readBody(req);
        const record: Record<string, unknown> = body !== null && typeof body === "object" && !Array.isArray(body)
          ? body as Record<string, unknown>
          : {};
        const userId = typeof record.user_id === "string" && record.user_id ? record.user_id : MOCK_USER_ID;
        const orgId = typeof record.org_id === "string" && record.org_id ? record.org_id : MOCK_ORG_ID;
        const created = store.createPairing(userId, orgId);
        const host = req.headers.host ?? `127.0.0.1:${PORT}`;
        sendJson(res, 201, {
          pairing_id: created.pairingId,
          code: created.code,
          expires_at: created.expiresAt,
          gateway_url: `http://${host}`,
        });
        return;
      }
      if (req.method === "POST" && url.pathname === "/v1/companion/pairings/redeem") {
        const body = await readBody(req);
        if (body === null || typeof body !== "object" || Array.isArray(body)) {
          sendJson(res, 400, { error: "The pairing request was invalid." });
          return;
        }
        const { code, device_name, platform } = body as Record<string, unknown>;
        if (typeof code !== "string" || typeof device_name !== "string" || typeof platform !== "string") {
          sendJson(res, 400, { error: "The pairing request was invalid." });
          return;
        }
        const ip = req.socket.remoteAddress ?? "unknown";
        const result = store.redeem(code, device_name, platform, ip);
        if (result.status !== 200) {
          sendJson(res, result.status, { error: result.error });
          return;
        }
        const host = req.headers.host ?? `127.0.0.1:${PORT}`;
        sendJson(res, 200, {
          device_id: result.device.deviceId,
          device_token: result.device.deviceToken,
          user: { id: result.device.userId, display_name: "Mock User" },
          gateway_url: `http://${host}`,
          expires_at: result.device.expiresAt,
        });
        return;
      }
      if (req.method === "POST" && url.pathname === "/v1/companion/devices/token") {
        const body = await readBody(req);
        if (body === null || typeof body !== "object" || Array.isArray(body)) {
          sendJson(res, 400, { error: "The pairing request was invalid." });
          return;
        }
        const { device_id, device_token } = body as Record<string, unknown>;
        if (typeof device_id !== "string" || typeof device_token !== "string") {
          sendJson(res, 400, { error: "The pairing request was invalid." });
          return;
        }
        const result = store.refresh(device_id, device_token);
        if (result.status !== 200) {
          closeDeviceSockets(device_id);
          sendJson(res, result.status, { error: result.error });
          return;
        }
        sendJson(res, 200, {
          access_token: result.rotation.accessToken,
          access_expires_at: result.rotation.accessExpiresAt,
          device_token: result.rotation.deviceToken,
          expires_at: result.rotation.expiresAt,
        });
        return;
      }
      if (req.method === "GET" && url.pathname === "/v1/companion/devices") {
        const devices = store.listDevices(bearer(req) ?? "");
        if (!devices) {
          sendJson(res, 401, { error: "This phone's pairing is no longer valid." });
          return;
        }
        sendJson(res, 200, {
          devices: devices.map((device) => ({
            device_id: device.deviceId,
            name: device.name,
            platform: device.platform,
            created_at: device.createdAt,
            last_seen_at: device.lastSeenAt,
          })),
        });
        return;
      }
      if (req.method === "DELETE" && url.pathname.startsWith("/v1/companion/devices/")) {
        const deviceId = decodeURIComponent(url.pathname.slice("/v1/companion/devices/".length));
        const outcome = store.revokeDevice(bearer(req) ?? "", deviceId);
        if (outcome === "unauthorized") {
          sendJson(res, 401, { error: "This phone's pairing is no longer valid." });
          return;
        }
        if (outcome === "missing") {
          sendJson(res, 404, { error: "No paired phone was found." });
          return;
        }
        closeDeviceSockets(deviceId);
        sendJson(res, 200, {});
        return;
      }
      if (req.method === "GET" && url.pathname === "/") {
        sendJson(res, 200, {
          ok: true,
          mock: "dragent-pairing-dev-mock",
          note: "Development fixture only on loopback: crypto-random fixtures, no real desktop, no E2EE.",
        });
        return;
      }
      sendJson(res, 404, { error: "Not found." });
    } catch {
      sendJson(res, 500, { error: "The mock request failed." });
    }
  });

  server.on("connection", (connection) => {
    connections.add(connection as Socket);
    trackedConnections.set(server, connections);
    connection.on("close", () => connections.delete(connection as Socket));
  });
  server.on("upgrade", (req, socket, head) => {
    const key = req.headers["sec-websocket-key"];
    const url = new URL(req.url ?? "/", "http://mock");
    if (url.pathname !== "/v1/relay/socket" || typeof key !== "string" || req.headers["sec-websocket-version"] !== "13") {
      socket.destroy();
      return;
    }
    handleRelaySocket(socket as Socket, key, head, store, now, track);
  });

  return server;
}

/**
 * Shut a mock server down in tests. Destroys every tracked connection first:
 * relay sockets upgraded from HTTP never release server.close() on their
 * own, and idle keep-alive fetch sockets would stall it as well.
 */
export function closeMockServer(server: Server): Promise<void> {
  return new Promise((resolve) => {
    for (const connection of trackedConnections.get(server) ?? []) {
      try {
        connection.destroy();
      } catch { /* already gone */ }
    }
    try {
      server.closeAllConnections();
    } catch { /* best effort */ }
    server.close(() => resolve());
  });
}

function handleRelaySocket(
  socket: Socket,
  key: string,
  head: Buffer,
  store: MockPairingStore,
  now: () => number,
  track: (deviceId: string, socket: Socket) => void,
): void {
  socket.write(
    [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${acceptKey(key)}`,
      "",
      "",
    ].join("\r\n"),
  );
  socket.on("error", () => socket.destroy());
  let admitted: { deviceId: string; sessionId: string; sessionExpiresAt: number } | null = null;
  let buffered: Buffer = Buffer.alloc(0);

  const closeWith = (code: number) => {
    const body = Buffer.alloc(2);
    body.writeUInt16BE(code);
    socket.end(serverFrame(0x8, body));
  };

  const receive = (chunk: Buffer) => {
    buffered = Buffer.concat([buffered, chunk]);
    for (;;) {
      let frame;
      try {
        frame = parseClientFrame(buffered);
      } catch {
        closeWith(1002);
        return;
      }
      if (!frame && buffered.length > MAX_FRAME_BYTES + 8) {
        socket.destroy();
        return;
      }
      if (!frame) break;
      buffered = frame.rest;
      if (frame.opcode === 0x8) {
        socket.end(serverFrame(0x8, frame.payload));
        return;
      }
      if (frame.opcode === 0x9) {
        socket.write(serverFrame(0xa, frame.payload));
        continue;
      }
      if (frame.opcode === 0xa) continue;
      let msg: Record<string, unknown>;
      try {
        const value: unknown = JSON.parse(frame.payload.toString("utf8"));
        if (value === null || typeof value !== "object" || Array.isArray(value)) continue;
        msg = value as Record<string, unknown>;
      } catch {
        continue;
      }
      if (!admitted) {
        if (msg.type !== "relay-auth" || msg.v !== 1 || typeof msg.access_token !== "string") {
          socket.destroy();
          return;
        }
        const device = store.deviceForAccess(msg.access_token);
        if (!device) {
          closeWith(4401);
          return;
        }
        // One 24h logical lease per device; resume returns the same lease.
        const resume = typeof msg.resume_session_id === "string" ? msg.resume_session_id : null;
        const sessionId = resume && resume.length <= 128 ? resume : `mock-session-${device.deviceId}`;
        const sessionExpiresAt = now() + 24 * 3600_000;
        admitted = { deviceId: device.deviceId, sessionId, sessionExpiresAt };
        track(device.deviceId, socket);
        socket.write(textFrame({
          type: "relay-ready",
          v: 1,
          session_id: sessionId,
          session_expires_at: sessionExpiresAt,
          access_expires_at: now() + 15 * 60_000,
        }));
        continue;
      }
      if (msg.type === "relay-ping" && msg.v === 1) {
        socket.write(textFrame({ type: "relay-pong", v: 1 }));
        continue;
      }
      if (msg.type === "rpc-request" && msg.v === 1 && typeof msg.id === "string" && typeof msg.method === "string") {
        if (msg.method === "machines.list") {
          socket.write(textFrame({
            type: "rpc-reply", id: msg.id, ok: true,
            result: { machines: [sampleMachine(now())] },
          }));
        } else {
          socket.write(textFrame({
            type: "rpc-reply", id: msg.id, ok: false,
            error: { message: "The mock does not implement this method.", loginRequired: false },
          }));
        }
        continue;
      }
      if (msg.type === "rpc-subscribe" && msg.v === 1 && typeof msg.id === "string" && msg.stream === "catalog") {
        socket.write(textFrame({
          type: "rpc-stream", stream: "catalog", seq: 1,
          payload: {
            catalog: { machine: null, projects: [], workspaces: [], tabs: [] },
            machine: sampleMachine(now()),
          },
        }));
      }
    }
  };
  socket.on("data", receive);
  if (head.length > 0) receive(head);
}

if (require.main === module) {
  const store = new MockPairingStore();
  createMockServer(store).listen(PORT, "127.0.0.1", () => {
    console.log(`[dragent-mock] pairing + relay dev mock (loopback only, no E2EE) http://127.0.0.1:${PORT}/ (+ ws /v1/relay/socket)`);
    console.log(`[dragent-mock] create a code: curl -X POST http://127.0.0.1:${PORT}/v1/companion/pairings -H 'Authorization: Bearer ${MOCK_DESKTOP_TOKEN}'`);
  });
}
