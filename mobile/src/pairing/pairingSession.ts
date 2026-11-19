/**
 * The phone side of pairing (docs/design/e2ee.md §6.2): redeem without a
 * device name, then the IKpsk1 handshake to the desktop key **from the QR**
 * (`k`), confirmed with `pair-confirm {h}`, committed on `pair-done`.
 *
 * - Redeem sends only `{code, platform}` (F11): the device name travels
 *   inside the encrypted HS1 payload, never to the server.
 * - Pairing is refused without a device passcode (§6.7, §9).
 * - The QR secret `P` is wiped when pairing finishes or is abandoned (§6.2
 *   step 3), whatever the outcome.
 * - The pin is committed only on a valid `pair-done` over the confirmed
 *   channel, and only to the QR key: a substituted key, a wrong confirmation
 *   hash or a replayed HS1 without confirmation commits nothing (§12.5).
 *
 * The handshake rides a short-lived pairing socket (text `relay-auth` with
 * the in-band token and `e2e:1`, then binary pairing frames). Frame and
 * crypto work goes through {@link PairingCrypto} so tests can drive the state
 * machine without the native module; production passes
 * {@link nativePairingCrypto}.
 */

import type {
  Channel,
  ChannelIds,
  Frame,
  NativeE2ee,
  PairingHandshake,
  Reassembler,
} from "../../modules/dragent-e2ee/src/types";
import { bytesEqual } from "../transport/e2ee/bytes";
import { decodeCtrl, encodeCtrl } from "../transport/e2ee/ctrl";
import { resolveRelaySocketURL } from "../transport/RelaySocketClient";
import type { PhoneSecurity } from "../transport/e2ee/security";
import type { SocketFactory } from "../transport/socket";
import {
  PairingError,
  redeemPairingCode,
  refreshDeviceToken,
  type PairingFetchFn,
  type PairingPlatform,
} from "./api";
import { normalizeDeviceName, wipeBytes } from "./pairing";

/** §9: no passcode, no pairing. */
export const NO_PASSCODE_MESSAGE = "Set a passcode on this phone to pair it.";
/** §9: the desktop key differs from the QR pin. */
export const KEY_CHANGED_MESSAGE =
  "This desktop's security key changed. If you reinstalled Dragent or reset its keys, pair again from Settings → Mobile on that desktop. If you didn't, don't continue.";
/** The code was single-use or stale, or the handshake failed. */
export const PAIRING_FAILED_MESSAGE =
  "Pairing didn't complete. On your desktop, open Settings → Mobile and try again.";
const UNREACHABLE_MESSAGE = "Could not reach your desktop. Check your connection and try again.";

/** The failure message with the step it happened at, so a report says where. */
function failedAt(step: string, kind: "invalid" | "offline" = "invalid"): PairingError {
  return new PairingError(`${PAIRING_FAILED_MESSAGE} (${step})`, kind);
}

/** The narrow crypto surface pairing needs (production: the native module). */
export interface PairingCrypto {
  hasDevicePasscode(): boolean;
  randomChannelId(): Uint8Array;
  /** Create the identity and push keys when missing (refuses without a passcode). */
  ensureKeys(): void;
  startPairing(ids: ChannelIds, desktopKey: Uint8Array, secretP: Uint8Array, deviceName: string): PairingHandshake;
  encodeHs1Frame(chan: Uint8Array, peer: string, body: Uint8Array): Uint8Array;
  decodeFrame(bytes: Uint8Array): Frame;
  encodeCtrlRecords(msgId: number, content: Uint8Array): Uint8Array[];
  createReassembler(): Reassembler;
}

/** {@link PairingCrypto} over the native E2EE module (no JS crypto, §3.2). */
export function nativePairingCrypto(native: NativeE2ee): PairingCrypto {
  return {
    hasDevicePasscode: () => native.hasDevicePasscode(),
    randomChannelId: () => native.randomChannelId(),
    ensureKeys: () => {
      native.createIdentityKey();
      native.createPushKey();
    },
    startPairing: (ids, desktopKey, secretP, deviceName) => native.startPairing(ids, desktopKey, secretP, deviceName),
    encodeHs1Frame: (chan, peer, body) => native.encodeHs1Frame("pairing", chan, peer, body),
    decodeFrame: (bytes) => native.decodeFrame(bytes),
    encodeCtrlRecords: (msgId, content) => native.encodeRecords("ctrl", msgId, content),
    createReassembler: () => native.createReassembler(),
  };
}

export interface PairingInput {
  readonly code: string;
  readonly gateway: string;
  readonly desktop: string;
  /** The desktop key from the QR. The server never supplies this. */
  readonly desktopKey: Uint8Array;
  /** The QR secret. Wiped on finish or abandon, whatever the outcome. */
  readonly secretP: Uint8Array;
  /** Goes into the encrypted HS1 payload only, never to the server. */
  readonly deviceName: string;
  readonly platform: PairingPlatform;
}

export interface PairingDeps {
  readonly crypto: PairingCrypto;
  readonly fetchFn: PairingFetchFn;
  readonly createSocket: SocketFactory;
  readonly security: PhoneSecurity;
  readonly now?: () => number;
  /** Overall pairing deadline (§6.2 step 9: 30 s). */
  readonly timeoutMs?: number;
}

export interface PairingResult {
  /** Persist these credentials (the device token is already rotated). */
  readonly deviceId: string;
  readonly deviceToken: string;
  readonly userId: string;
  readonly displayName: string;
  readonly gateway: string;
  readonly expiresAt: number;
  readonly desktop: string;
  readonly desktopName: string;
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return copy.buffer;
}

function fromIncoming(data: unknown): Uint8Array | null {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) {
    const view = data as unknown as Uint8Array;
    const copy = new Uint8Array(view.byteLength);
    copy.set(view);
    return copy;
  }
  return null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hexOf(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += b.toString(16).padStart(2, "0");
  return out;
}

/**
 * Run the full pairing flow. Resolves with the credentials to persist and
 * the pinned desktop; rejects with a {@link PairingError} carrying plain
 * words for the person. `secretP` is wiped before this returns.
 */
export async function runPairing(deps: PairingDeps, input: PairingInput): Promise<PairingResult> {
  const name = normalizeDeviceName(input.deviceName);
  try {
    if (!deps.crypto.hasDevicePasscode()) {
      throw new PairingError(NO_PASSCODE_MESSAGE, "passcode");
    }
    deps.crypto.ensureKeys();
    const redeemed = await redeemPairingCode(deps.fetchFn, input.gateway, {
      code: input.code,
      platform: input.platform,
    });
    // The pairing socket needs a short-lived access token; the device token
    // rotates on every refresh, so persist the rotated one (reusing the old
    // one would revoke the device server-side).
    const refreshed = await refreshDeviceToken(deps.fetchFn, input.gateway, {
      deviceId: redeemed.deviceId,
      deviceToken: redeemed.deviceToken,
    });
    const url = resolveRelaySocketURL(input.gateway);
    if (!url) throw new PairingError(UNREACHABLE_MESSAGE, "offline");
    const pairDone = await pairOverSocket(deps, input, name, {
      account: redeemed.user.id,
      phone: redeemed.deviceId,
      accessToken: refreshed.accessToken,
      socketUrl: url,
    });
    deps.security.commitPairing({
      desktop: input.desktop,
      identityKey: input.desktopKey,
      desktopName: pairDone.name,
      catalogVersion: pairDone.catalogVersion,
    });
    return {
      deviceId: redeemed.deviceId,
      deviceToken: refreshed.deviceToken,
      userId: redeemed.user.id,
      displayName: redeemed.user.displayName,
      gateway: redeemed.gatewayUrl,
      expiresAt: redeemed.expiresAt,
      desktop: input.desktop,
      desktopName: pairDone.name,
    };
  } finally {
    wipeBytes(input.secretP);
  }
}

interface SocketAuth {
  readonly account: string;
  readonly phone: string;
  readonly accessToken: string;
  readonly socketUrl: string;
}

interface PairDone {
  readonly name: string;
  readonly catalogVersion: number;
}

/**
 * HS1 → HS2 → `pair-confirm {h}` → `pair-done` over one pairing socket.
 * Commits nothing: the caller pins on success. Abandons the handshake and
 * closes the channel and socket on every exit.
 */
function pairOverSocket(
  deps: PairingDeps,
  input: PairingInput,
  deviceName: string,
  auth: SocketAuth,
): Promise<PairDone> {
  const now = deps.now ?? Date.now;
  const timeoutMs = deps.timeoutMs ?? 30_000;
  const socket = deps.createSocket(auth.socketUrl, {});
  // React Native hands binary frames over as Blobs unless asked for
  // ArrayBuffers; a Blob is unreadable here, so the desktop's handshake reply
  // was dropped and pairing timed out (seen on Android).
  try {
    socket.binaryType = "arraybuffer";
  } catch {
    /* not settable on this socket */
  }

  return new Promise<PairDone>((resolve, reject) => {
    let handshake: PairingHandshake | null = null;
    let channel: Channel | null = null;
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let admitted = false;
    let hsSent = false;
    let confirmSent = false;
    let chanHex = "";
    const reasm = deps.crypto.createReassembler();

    const cleanup = () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      try {
        reasm.release();
      } catch {
        /* freed */
      }
      if (handshake) {
        try {
          handshake.abandon();
        } catch {
          /* consumed */
        }
        try {
          handshake.release();
        } catch {
          /* freed */
        }
        handshake = null;
      }
      if (channel) {
        try {
          channel.close("local");
        } catch {
          /* closed */
        }
        try {
          channel.release();
        } catch {
          /* freed */
        }
        channel = null;
      }
      try {
        socket.close();
      } catch {
        /* gone */
      }
    };
    const failed = (error: unknown) => {
      if (settled) return;
      console.warn(`[pairing] failed: ${error instanceof Error ? error.message : String(error)}`);
      settled = true;
      cleanup();
      if (error instanceof PairingError) {
        reject(error);
        return;
      }
      // A native error's own words say which check failed.
      const why = error instanceof Error && error.message ? error.message : String(error);
      reject(failedAt(why.slice(0, 120)));
    };
    const done = (value: PairDone) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(value);
    };

    timer = setTimeout(() => {
      const step = confirmSent
        ? "your desktop didn't finish"
        : hsSent
          ? "no reply from your desktop"
          : admitted
            ? "couldn't start the handshake"
            : "the relay didn't let this phone in";
      failed(failedAt(step, "offline"));
    }, timeoutMs);

    const sendHs1 = () => {
      const chan = deps.crypto.randomChannelId();
      chanHex = hexOf(chan);
      try {
        handshake = deps.crypto.startPairing(
          { account: auth.account, phone: auth.phone, desktop: input.desktop, chan },
          input.desktopKey,
          input.secretP,
          deviceName,
        );
      } catch (error) {
        failed(error);
        return;
      }
      try {
        socket.send(toArrayBuffer(deps.crypto.encodeHs1Frame(chan, input.desktop, handshake.hs1)));
      } catch {
        failed(new PairingError(UNREACHABLE_MESSAGE, "offline"));
        return;
      }
      hsSent = true;
    };

    const onHs2 = (body: Uint8Array) => {
      if (!handshake || channel) {
        failed(failedAt("unexpected reply"));
        return;
      }
      const hs = handshake;
      handshake = null;
      let ch: Channel;
      try {
        ch = hs.finish(body, now());
      } catch {
        try {
          hs.abandon();
        } catch {
          /* consumed */
        }
        failed(failedAt("the desktop's reply didn't verify"));
        return;
      }
      try {
        hs.release();
      } catch {
        /* freed */
      }
      // The channel is authenticated by the QR key and `P`: a substituted
      // key fails here and pins nothing (§12.5).
      if (!bytesEqual(ch.remoteStatic(), input.desktopKey)) {
        try {
          ch.close("local");
        } catch {
          /* closed */
        }
        try {
          ch.release();
        } catch {
          /* freed */
        }
        failed(new PairingError(KEY_CHANGED_MESSAGE, "invalid"));
        return;
      }
      channel = ch;
      let records: Uint8Array[];
      try {
        records = deps.crypto.encodeCtrlRecords(1, encodeCtrl({ kind: "pair-confirm", h: ch.handshakeHash() }));
      } catch {
        failed(failedAt("couldn't write the confirmation"));
        return;
      }
      try {
        for (const record of records) socket.send(toArrayBuffer(ch.sealFrame(record)));
      } catch {
        failed(new PairingError(UNREACHABLE_MESSAGE, "offline"));
        return;
      }
      confirmSent = true;
    };

    const onData = (ctr: number, body: Uint8Array) => {
      if (!channel || !confirmSent) {
        failed(failedAt("data before confirmation"));
        return;
      }
      let inbound;
      try {
        inbound = channel.receive(ctr, body);
      } catch {
        failed(failedAt("couldn't open the desktop's message"));
        return;
      }
      if (inbound.kind === "duplicate") return;
      if (inbound.kind !== "record") {
        failed(failedAt(`unexpected ${inbound.kind} message`));
        return;
      }
      let message;
      try {
        message = reasm.feed(inbound.record);
      } catch {
        failed(failedAt("couldn't reassemble the desktop's message"));
        return;
      }
      if (!message) return;
      if (message.mtype !== "ctrl") {
        failed(failedAt(`unexpected ${message.mtype} message`));
        return;
      }
      let ctrl;
      try {
        ctrl = decodeCtrl(message.content);
      } catch {
        failed(failedAt("couldn't read pair-done"));
        return;
      }
      if (!ctrl || ctrl.kind !== "pair-done") {
        failed(failedAt(`got ${ctrl ? ctrl.kind : "an unknown message"}, not pair-done`));
        return;
      }
      done({ name: ctrl.desktopName, catalogVersion: ctrl.catalogVersion });
    };

    socket.onopen = () => {
      try {
        socket.send(JSON.stringify({
          type: "relay-auth",
          v: 1,
          access_token: auth.accessToken,
          e2e: 1,
          device_id: input.desktop,
        }));
      } catch {
        failed(new PairingError(UNREACHABLE_MESSAGE, "offline"));
      }
    };
    socket.onmessage = (event: { data: unknown }) => {
      const binary = fromIncoming(event.data);
      if (binary) {
        let frame: Frame;
        try {
          frame = deps.crypto.decodeFrame(binary);
        } catch {
          return; // Unauthenticated junk: it can only cost us a frame.
        }
        // Frames for another channel (an older one being closed) aren't ours.
        if (hexOf(frame.chan) !== chanHex) return;
        if (frame.kind === "close") {
          failed(failedAt(hsSent && !channel ? "your desktop refused this phone" : "your desktop closed the pairing"));
          return;
        }
        if (frame.kind === "hs" && frame.step === 2 && hsSent && !channel) {
          console.info("[pairing] desktop answered the handshake");
          onHs2(frame.body);
          return;
        }
        if (frame.kind === "data" && channel) {
          onData(frame.ctr, frame.body);
        }
        return;
      }
      if (typeof event.data !== "string") return;
      let text: unknown;
      try {
        text = JSON.parse(event.data);
      } catch {
        return;
      }
      if (!isRecord(text)) return;
      if (text.type === "relay-ready" && !admitted) {
        console.info("[pairing] relay admitted the pairing socket; sending the handshake");
        admitted = true;
        sendHs1();
        return;
      }
      // The relay refused or dropped our handshake channel, and says why
      // (unavailable, refused, bad target…). Waiting would only time out.
      if (text.type === "relay-chan-closed" && (typeof text.chan !== "string" || text.chan === chanHex)) {
        const reason = typeof text.reason === "string" && text.reason ? text.reason : "unknown reason";
        console.warn(`[pairing] relay closed the pairing channel: ${reason}`);
        failed(failedAt(`the relay closed the pairing: ${reason}`));
        return;
      }
      if (text.type === "relay-error") {
        const message = typeof text.message === "string" && text.message.trim()
          ? text.message
          : PAIRING_FAILED_MESSAGE;
        failed(new PairingError(message, "invalid"));
      }
    };
    socket.onerror = () => {
      if (!admitted && !hsSent) failed(new PairingError(UNREACHABLE_MESSAGE, "offline"));
    };
    socket.onclose = (event: { code: number }) => {
      failed(admitted
        ? failedAt(`connection closed, code ${event?.code ?? "unknown"}`)
        : new PairingError(UNREACHABLE_MESSAGE, "offline"));
    };
  });
}
