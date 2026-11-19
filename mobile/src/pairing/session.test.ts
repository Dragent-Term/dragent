/**
 * §12.5 phone cases for the IKpsk1 pairing flow (docs/design/e2ee.md §6.2):
 * redeem carries no device name, the QR key (never a server key) drives the
 * handshake, nothing commits on a substituted key, a bad confirmation or a
 * substituted peer, pairing without a passcode is refused, and `P` is wiped
 * on finish or abandon.
 *
 * The state machine is driven through fakes (no native module under tsx);
 * the CTRL codec is the real one, so `pair-confirm {h}` / `pair-done` bytes
 * are exactly what the desktop checks and sends.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  KEY_CHANGED_MESSAGE,
  NO_PASSCODE_MESSAGE,
  runPairing,
  type PairingCrypto,
  type PairingDeps,
  type PairingInput,
} from "./pairingSession";
import { PairingError, type PairingFetchFn } from "./api";
import { decodeCtrl, encodeCtrl, toBase64Url } from "../transport/e2ee/ctrl";
import type {
  Channel,
  ChannelIds,
  Frame,
  PairingHandshake,
  Reassembler,
} from "../../modules/dragent-e2ee/src/types";
import type { PhoneSecurity } from "../transport/e2ee/security";
import type { RealtimeSocket, SocketFactory } from "../transport/socket";

const CODE = "ABCDEFGH234567ABCDEF";
const GATEWAY = "https://api.example.test";
const ACCOUNT = "0a0b0c0d-0000-4000-8000-000000000001";
const PHONE = "0a0b0c0d-0000-4000-8000-0000000000a1";
const DESKTOP = "0a0b0c0d-0000-4000-8000-0000000000d1";

const qrKey = Uint8Array.from({ length: 32 }, (_, i) => (7 + i) & 255);
const secret = (): Uint8Array => Uint8Array.from({ length: 32 }, (_, i) => (101 + i) & 255);
const CHAN = Uint8Array.from({ length: 16 }, (_, i) => i + 1);
const HS_HASH = Uint8Array.from({ length: 32 }, (_, i) => (200 + i) & 255);

const hexOf = (b: Uint8Array): string => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
const unhex = (s: string): Uint8Array => Uint8Array.from(s.match(/../g)!.map((p) => parseInt(p, 16)));
const utf8 = (s: string): Uint8Array => new TextEncoder().encode(s);
const ZERO32 = new Uint8Array(32);

// --- fake frame codec (test-only envelope; the crypto is faked, the CTRL is real)

function encFrame(f: { kind: string; step: number; chan: string; ctr: number; body: string; peer?: string }): Uint8Array {
  return utf8(JSON.stringify({ ...f, purpose: "pairing" }));
}

function decFrame(bytes: Uint8Array): Frame {
  const f = JSON.parse(new TextDecoder().decode(bytes)) as {
    kind: string; step: number; chan: string; ctr: number; body: string; peer?: string;
  };
  return {
    kind: f.kind as Frame["kind"],
    step: f.step,
    purpose: "pairing",
    chan: unhex(f.chan),
    peer: f.peer ?? null,
    ctr: f.ctr,
    body: unhex(f.body),
  };
}

// --- fakes

class FakeSocket implements RealtimeSocket {
  readyState = 1;
  /** React Native's default: binary frames arrive as Blobs. */
  binaryType = "blob";
  onopen: RealtimeSocket["onopen"] = null;
  onmessage: RealtimeSocket["onmessage"] = null;
  onerror: RealtimeSocket["onerror"] = null;
  onclose: RealtimeSocket["onclose"] = null;
  readonly sent: unknown[] = [];
  closed = false;
  send(data: string | ArrayBuffer): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
  }
  open(): void {
    this.onopen?.({});
  }
  receiveText(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) });
  }
  receiveBinary(bytes: Uint8Array): void {
    const copy = new Uint8Array(bytes.byteLength);
    copy.set(bytes);
    // Like React Native: an ArrayBuffer only when asked for one.
    this.onmessage?.({ data: this.binaryType === "arraybuffer" ? copy.buffer : { size: copy.byteLength, type: "" } });
  }
}

interface StartedCall {
  readonly ids: ChannelIds;
  readonly desktopKey: Uint8Array;
  readonly secretP: Uint8Array;
  readonly deviceName: string;
}

class FakeHandshake {
  abandoned = false;
  released = false;
  constructor(
    private readonly crypto: FakeCrypto,
    readonly hs1: Uint8Array,
  ) {}
  finish(_hs2: Uint8Array, _nowMs: number): Channel {
    return this.crypto.openChannel();
  }
  abandon(): void {
    this.abandoned = true;
  }
  release(): void {
    this.released = true;
  }
}

class FakeChannel {
  private ctr = 0;
  constructor(private readonly crypto: FakeCrypto) {}
  get chanId(): Uint8Array {
    return CHAN;
  }
  handshakeHash(): Uint8Array {
    return HS_HASH;
  }
  remoteStatic(): Uint8Array {
    return this.crypto.remoteStatic;
  }
  sealFrame(record: Uint8Array): Uint8Array {
    const frame = encFrame({ kind: "data", step: 0, chan: hexOf(CHAN), ctr: this.ctr++, body: hexOf(record) });
    const copy = new Uint8Array(frame.byteLength);
    copy.set(frame);
    return copy;
  }
  receive(ctr: number, ciphertext: Uint8Array): { kind: "record"; record: Uint8Array } {
    assert.equal(ctr, 0);
    return { kind: "record", record: ciphertext };
  }
  close(_reason: unknown): void {}
  release(): void {}
}

class FakeCrypto implements PairingCrypto {
  passcode = true;
  ensureKeysCalled = false;
  started: StartedCall[] = [];
  handshakes: FakeHandshake[] = [];
  remoteStatic: Uint8Array = qrKey;
  hasDevicePasscode(): boolean {
    return this.passcode;
  }
  randomChannelId(): Uint8Array {
    return CHAN.slice();
  }
  ensureKeys(): void {
    this.ensureKeysCalled = true;
  }
  startPairing(ids: ChannelIds, desktopKey: Uint8Array, secretP: Uint8Array, deviceName: string): PairingHandshake {
    this.started.push({ ids, desktopKey: desktopKey.slice(), secretP: secretP.slice(), deviceName });
    const hs = new FakeHandshake(this, utf8("fake-hs1"));
    this.handshakes.push(hs);
    return hs as unknown as PairingHandshake;
  }
  openChannel(): Channel {
    return new FakeChannel(this) as unknown as Channel;
  }
  encodeHs1Frame(chan: Uint8Array, peer: string, body: Uint8Array): Uint8Array {
    return encFrame({ kind: "hs", step: 1, chan: hexOf(chan), ctr: 0, body: hexOf(body), peer });
  }
  decodeFrame(bytes: Uint8Array): Frame {
    return decFrame(bytes);
  }
  encodeCtrlRecords(_msgId: number, content: Uint8Array): Uint8Array[] {
    return [content.slice()];
  }
  createReassembler(): Reassembler {
    return {
      feed: (record: Uint8Array) => ({ mtype: "ctrl", content: record }),
      buffered: () => 0,
      clear: () => {},
      release: () => {},
    } as unknown as Reassembler;
  }
}

interface FetchLog {
  readonly redeem: Record<string, unknown>[];
  readonly token: Record<string, unknown>[];
}

function fetchFn(log: FetchLog, mode: "ok" | "gone" = "ok"): PairingFetchFn {
  const json = (status: number, body: unknown) => ({ ok: status >= 200 && status < 300, status, text: async () => JSON.stringify(body) });
  return async (url, init) => {
    const body = JSON.parse(init?.body ?? "{}") as Record<string, unknown>;
    if (url.endsWith("/v1/companion/pairings/redeem")) {
      log.redeem.push(body);
      if (mode === "gone") return json(410, { error: "used" });
      return json(200, {
        device_id: PHONE,
        device_token: "tok-1",
        user: { id: ACCOUNT, display_name: "Ada" },
        gateway_url: GATEWAY,
        expires_at: 999,
      });
    }
    log.token.push(body);
    return json(200, {
      access_token: "a-2",
      access_expires_at: 200,
      device_token: "tok-2",
      expires_at: 300,
    });
  };
}

interface Harness {
  readonly crypto: FakeCrypto;
  readonly socket: FakeSocket;
  readonly commits: Record<string, unknown>[];
  readonly log: FetchLog;
  readonly deps: PairingDeps;
  readonly input: PairingInput;
}

function harness(mode: "ok" | "gone" = "ok", timeoutMs = 30_000): Harness {
  const crypto = new FakeCrypto();
  const socket = new FakeSocket();
  const commits: Record<string, unknown>[] = [];
  const log: FetchLog = { redeem: [], token: [] };
  const security = {
    commitPairing: (value: unknown) => {
      commits.push(value as Record<string, unknown>);
    },
  } as unknown as PhoneSecurity;
  const createSocket: SocketFactory = () => socket;
  const secretP = secret();
  const deps: PairingDeps = { crypto, fetchFn: fetchFn(log, mode), createSocket, security, timeoutMs };
  const input: PairingInput = {
    code: CODE,
    gateway: GATEWAY,
    desktop: DESKTOP,
    desktopKey: qrKey.slice(),
    secretP,
    deviceName: "My phone",
    platform: "ios",
  };
  return { crypto, socket, commits, log, deps, input };
}

const flush = async (): Promise<void> => {
  for (let i = 0; i < 20; i++) await Promise.resolve();
};

function texts(socket: FakeSocket): Record<string, unknown>[] {
  return socket.sent.filter((s): s is string => typeof s === "string").map((s) => JSON.parse(s) as Record<string, unknown>);
}

function binaries(socket: FakeSocket): Frame[] {
  const crypto = new FakeCrypto();
  return socket.sent
    .filter((s): s is ArrayBuffer => s instanceof ArrayBuffer)
    .map((s) => crypto.decodeFrame(new Uint8Array(s)));
}

/** Drive the socket to just before `pair-done`; returns the DATA expectation. */

test("happy path pins the QR key on pair-done and wipes P", async () => {
  const h = harness();
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();

  // relay-auth carries the in-band token and e2e:1, never a device name.
  const auth = texts(h.socket).find((t) => t.type === "relay-auth");
  assert.deepEqual(auth, { type: "relay-auth", v: 1, access_token: "a-2", e2e: 1, device_id: DESKTOP });
  for (const t of texts(h.socket)) {
    assert.ok(!("device_name" in t), "device name on the wire");
  }

  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();

  // HS1 goes to the desktop with the QR key behind it (never a server key).
  assert.equal(h.crypto.started.length, 1);
  const started = h.crypto.started[0]!;
  assert.deepEqual(started.desktopKey, qrKey);
  assert.equal(started.deviceName, "My phone");
  assert.equal(started.ids.account, ACCOUNT);
  assert.equal(started.ids.phone, PHONE);
  assert.equal(started.ids.desktop, DESKTOP);
  assert.deepEqual(started.ids.chan, CHAN);
  const hs1 = binaries(h.socket).find((f) => f.kind === "hs");
  assert.ok(hs1);
  assert.equal(hs1.step, 1);
  assert.equal(hexOf(hs1.chan), hexOf(CHAN));

  // HS2 → pair-confirm {h} as DATA ctr 0.
  h.socket.receiveBinary(encFrame({ kind: "hs", step: 2, chan: hexOf(CHAN), ctr: 0, body: hexOf(utf8("fake-hs2")) }));
  await flush();
  const confirm = binaries(h.socket).find((f) => f.kind === "data");
  assert.ok(confirm);
  assert.equal(confirm.ctr, 0);
  const ctrl = decodeCtrl(confirm.body);
  assert.ok(ctrl && ctrl.kind === "pair-confirm");
  assert.deepEqual(ctrl.h, HS_HASH);

  // pair-done commits the pin to the QR key with the L6 floor inputs.
  const doneBytes = encodeCtrl({ kind: "pair-done", desktopName: "Studio Mac", catalogVersion: 9 });
  h.socket.receiveBinary(encFrame({ kind: "data", step: 0, chan: hexOf(CHAN), ctr: 0, body: hexOf(doneBytes) }));
  const result = await run;
  assert.equal(h.commits.length, 1);
  const commit = h.commits[0]! as { desktop: string; identityKey: Uint8Array; desktopName: string; catalogVersion: number };
  assert.equal(commit.desktop, DESKTOP);
  assert.deepEqual(commit.identityKey, qrKey);
  assert.equal(commit.desktopName, "Studio Mac");
  assert.equal(commit.catalogVersion, 9);
  // The rotated device token is what the caller persists (reusing tok-1 would revoke).
  assert.equal(result.deviceId, PHONE);
  assert.equal(result.deviceToken, "tok-2");
  assert.equal(result.userId, ACCOUNT);
  assert.equal(result.gateway, GATEWAY);
  assert.equal(result.desktopName, "Studio Mac");
  // P is wiped on finish.
  assert.deepEqual(h.input.secretP, ZERO32);
  assert.ok(h.socket.closed);
});

test("redeem sends no device name, only code and platform", async () => {
  const h = harness();
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  h.socket.receiveBinary(encFrame({ kind: "hs", step: 2, chan: hexOf(CHAN), ctr: 0, body: hexOf(utf8("fake-hs2")) }));
  await flush();
  h.socket.receiveBinary(encFrame({
    kind: "data", step: 0, chan: hexOf(CHAN), ctr: 0,
    body: hexOf(encodeCtrl({ kind: "pair-done", desktopName: "D", catalogVersion: 1 })),
  }));
  await run;
  assert.equal(h.log.redeem.length, 1);
  assert.deepEqual(Object.keys(h.log.redeem[0]!).sort(), ["code", "platform"]);
  assert.deepEqual(h.log.redeem[0], { code: CODE, platform: "ios" });
});

test("pairing without a passcode is refused before any network", async () => {
  const h = harness();
  h.crypto.passcode = false;
  await assert.rejects(runPairing(h.deps, h.input), (error: unknown) =>
    error instanceof PairingError && error.kind === "passcode" && error.message === NO_PASSCODE_MESSAGE);
  assert.equal(h.log.redeem.length, 0);
  assert.equal(h.crypto.ensureKeysCalled, false);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("a substituted desktop key pins nothing", async () => {
  const h = harness();
  h.crypto.remoteStatic = Uint8Array.from({ length: 32 }, () => 9);
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  h.socket.receiveBinary(encFrame({ kind: "hs", step: 2, chan: hexOf(CHAN), ctr: 0, body: hexOf(utf8("fake-hs2")) }));
  await assert.rejects(run, (error: unknown) =>
    error instanceof PairingError && error.message === KEY_CHANGED_MESSAGE);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("a wrong confirmation shape commits nothing", async () => {
  const h = harness();
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  h.socket.receiveBinary(encFrame({ kind: "hs", step: 2, chan: hexOf(CHAN), ctr: 0, body: hexOf(utf8("fake-hs2")) }));
  await flush();
  // Anything but pair-done after pair-confirm fails the pairing.
  h.socket.receiveBinary(encFrame({
    kind: "data", step: 0, chan: hexOf(CHAN), ctr: 0,
    body: hexOf(encodeCtrl({ kind: "bye", reason: "nope" })),
  }));
  await assert.rejects(run, (error: unknown) => error instanceof PairingError);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("an HS2 on a substituted channel is ignored and the pairing times out", async () => {
  const h = harness("ok", 200);
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  const other = hexOf(Uint8Array.from({ length: 16 }, () => 7));
  h.socket.receiveBinary(encFrame({ kind: "hs", step: 2, chan: other, ctr: 0, body: hexOf(utf8("fake-hs2")) }));
  await assert.rejects(run, (error: unknown) => error instanceof PairingError);
  assert.equal(h.commits.length, 0);
  assert.equal(h.crypto.handshakes[0]?.abandoned, true);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("a dead socket abandons the handshake and wipes P", async () => {
  const h = harness();
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  assert.equal(h.crypto.handshakes.length, 1);
  h.socket.onclose?.({ code: 1006 });
  await assert.rejects(run, (error: unknown) => error instanceof PairingError);
  assert.equal(h.crypto.handshakes[0]?.abandoned, true);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("an expired code fails with gone and wipes P", async () => {
  const h = harness("gone");
  await assert.rejects(runPairing(h.deps, h.input), (error: unknown) =>
    error instanceof PairingError && error.kind === "gone");
  assert.equal(h.crypto.started.length, 0);
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("a relay-error carries the server message and commits nothing", async () => {
  const h = harness();
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-error", v: 1, message: "try later" });
  await assert.rejects(run, (error: unknown) =>
    error instanceof PairingError && (error as Error).message === "try later");
  assert.equal(h.commits.length, 0);
  assert.deepEqual(h.input.secretP, ZERO32);
});

test("the QR key in b64url round-trips through the link", () => {
  assert.equal(toBase64Url(qrKey).length, 43);
});

test("the relay refusing the handshake fails at once with its reason and wipes P", async () => {
  // A long deadline: the refusal must end pairing, not the timer.
  const h = harness("ok", 60_000);
  const run = runPairing(h.deps, h.input);
  await flush();
  h.socket.open();
  await flush();
  h.socket.receiveText({ type: "relay-ready", v: 1 });
  await flush();
  h.socket.receiveText({ type: "relay-chan-closed", v: 1, chan: hexOf(CHAN), reason: "unavailable" });
  await assert.rejects(run, (error: unknown) =>
    error instanceof PairingError && error.message.includes("the relay closed the pairing: unavailable"));
  assert.equal(h.commits.length, 0);
  assert.equal(h.crypto.handshakes[0]?.abandoned, true);
  assert.deepEqual(h.input.secretP, ZERO32);
});
