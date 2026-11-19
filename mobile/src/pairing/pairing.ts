/**
 * Phone-pairing link parsing (docs/design/e2ee.md §6.2, no network, no storage).
 *
 * The signed-in desktop shows a QR encoding the v2 link
 * `dragent://pair?v=2&c=<code>&g=<gateway>&d=<desktop_id>&k=<b64url identity pub>&p=<b64url P>`
 * plus the full link as text for pasting. The phone never signs in: it
 * redeems the single-use code for its own device credentials, then proves
 * possession of the QR secret `P` inside the IKpsk1 handshake (never to the
 * server).
 *
 * There is no bare-code entry: without a camera, the person pastes the full
 * link. `P` is never persisted: the link handler and the confirm screen pass
 * `d`, `k` and `p` in memory (see `pendingLink.ts`), never as route
 * parameters that navigation state or history keeps, and `p` is wiped when
 * pairing finishes or is abandoned.
 */

import { fromBase64Url } from "../transport/e2ee/ctrl";

export interface PairLink {
  /** 20-char pairing code, uppercased. */
  readonly code: string;
  /** Canonical pairing gateway origin (https, or http loopback in dev). */
  readonly gateway: string;
  /** The desktop's device id (lowercase canonical UUID, §5). */
  readonly desktop: string;
  /** The desktop's identity public key from the QR (32 bytes). Never from the server. */
  readonly desktopKey: Uint8Array;
  /**
   * The QR secret `P` (32 bytes). Mutable and owned by the holder: wipe with
   * {@link wipeBytes} when pairing finishes or is abandoned (§6.2 step 3).
   */
  readonly secretP: Uint8Array;
}

/** Overwrite every byte (pairing secrets, §6.2 step 3). Idempotent. */
export function wipeBytes(bytes: Uint8Array): void {
  bytes.fill(0);
}

/** Loopback hosts where cleartext http is a development override, never production. */
function isLoopback(hostname: string): boolean {
  return hostname === "localhost" || hostname === "127.0.0.1" || hostname === "[::1]";
}

/**
 * Canonical pairing gateway: an https origin (wss mapped to https), or a
 * loopback http origin for development. Credentials, paths, queries and
 * fragments are rejected; tokens never belong in a URL.
 */
export function normalizePairingGateway(raw: string, allowInsecure = false): string {
  const value = raw.trim();
  if (!value || value.length > 2048) throw new Error("The pairing code has no usable gateway. Scan the QR code again.");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error("The pairing code has no usable gateway. Scan the QR code again.");
  }
  if (!url.hostname || url.username || url.password || url.search || url.hash) {
    throw new Error("The pairing code has no usable gateway. Scan the QR code again.");
  }
  if (url.protocol === "wss:") url.protocol = "https:";
  else if (url.protocol === "ws:") url.protocol = "http:";
  const loopback = isLoopback(url.hostname);
  if (url.protocol !== "https:" && !(allowInsecure && loopback && url.protocol === "http:")) {
    throw new Error("The pairing gateway must use HTTPS.");
  }
  return url.origin;
}

function requiredParam(params: URLSearchParams, key: string): string {
  const values = params.getAll(key);
  if (values.length !== 1 || !values[0]) {
    throw new Error("This link is missing its security details. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  return values[0]!;
}

function isDesktopId(value: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
}

function keyParam(params: URLSearchParams, key: string): Uint8Array {
  const raw = requiredParam(params, key);
  if (raw.length > 64) {
    throw new Error("This link is missing its security details. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  const bytes = fromBase64Url(raw);
  if (!bytes || bytes.length !== 32) {
    throw new Error("This link is missing its security details. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  return bytes;
}

/**
 * Parse a scanned, pasted or deep-linked v2 pairing payload
 * (`dragent://pair?v=2&c=…&g=…&d=…&k=…&p=…`). Bare codes are refused (§6.2:
 * without a camera, the person pastes the full link).
 */
export function parsePairLink(raw: string, allowInsecure = false): PairLink {
  const trimmed = raw.trim();
  if (!trimmed || trimmed.length > 8192) {
    throw new Error("This is not a Dragent pairing code. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  // Bare code paste: refused. A typed code carries no desktop key and no
  // secret, so it can never pair (§6.2, §12.5).
  if (!trimmed.includes(":")) {
    throw new Error("A typed code can't pair this phone. Paste the full link shown under the QR in Settings → Mobile on your desktop.");
  }
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new Error("This is not a Dragent pairing code. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  if (url.protocol !== "dragent:" || url.hostname !== "pair" || url.username || url.password || url.hash) {
    throw new Error("This is not a Dragent pairing code. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  if (url.searchParams.get("v") !== "2") {
    throw new Error("This pairing link is from an older Dragent. Update Dragent on your desktop and scan the new QR.");
  }
  const code = normalizePairingCode(requiredParam(url.searchParams, "c"));
  const gateway = normalizePairingGateway(requiredParam(url.searchParams, "g"), allowInsecure);
  const desktop = requiredParam(url.searchParams, "d");
  if (!isDesktopId(desktop)) {
    throw new Error("This link is missing its security details. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  const desktopKey = keyParam(url.searchParams, "k");
  const secretP = keyParam(url.searchParams, "p");
  return { code, gateway, desktop, desktopKey, secretP };
}

/** 20 chars of base32, case-insensitive on entry, stored uppercase. */
export function normalizePairingCode(raw: string): string {
  const code = raw.trim().toUpperCase();
  if (!/^[A-Z2-7]{20}$/.test(code)) {
    throw new Error("This is not a Dragent pairing code. Scan the QR shown in Settings → Mobile on your desktop.");
  }
  return code;
}

/**
 * Device names are bounded at 64 chars. The name never goes to the server
 * (F11): it travels inside the encrypted pairing payload (§6.2 step 5) and
 * labels the local credential.
 */
export function normalizeDeviceName(raw: string): string {
  const name = raw.trim().replace(/\s+/g, " ");
  if (!name) throw new Error("Name this phone so you can recognize it on your desktop.");
  if (name.length > 64) throw new Error("Keep the device name to 64 characters or fewer.");
  return name;
}
