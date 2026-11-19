/**
 * CTRL control messages (docs/design/e2ee.md §4.4, §4.6, §6.2), phone side.
 *
 * Wire form, fixed by `dragent_e2ee::codec::ctrl` and pinned by
 * `vectors/codec/ctrl.json`: `<name> <compact json>`, one encoding per
 * message (fields in a fixed order, no whitespace, minimal escapes). The
 * phone encodes exactly those bytes and decodes strictly: anything that is
 * not the canonical encoding is rejected, so both ends agree byte for byte.
 * The part commands (`part-begin`, `part`, `part-end`, `part-abort`) pass
 * through as `null`; the native part session parses them.
 */
import { bytesEqual, toHex, utf8Decode, utf8Encode } from "./bytes";

export type Ctrl =
  | { readonly kind: "hello"; readonly resume: CtrlResume | null }
  | { readonly kind: "bye"; readonly reason: string }
  | { readonly kind: "pair-confirm"; readonly h: Uint8Array }
  | { readonly kind: "pair-done"; readonly desktopName: string; readonly catalogVersion: number }
  | { readonly kind: "identity-rotate"; readonly newPub: Uint8Array; readonly newPushPub: Uint8Array | null }
  | { readonly kind: "input_lost"; readonly handles: readonly number[] }
  | { readonly kind: "input_lost_ack" }
  /** The desktop took these terminals back; their handles are released. */
  | { readonly kind: "control_taken"; readonly handles: readonly number[] };

export interface CtrlResume {
  /** The old channel id (16 bytes). */
  readonly chan: Uint8Array;
  /** The last DATA counter sent on it; null when nothing was sent. */
  readonly lastSentCtr: number | null;
}

export type CtrlErrorKind = "BadFraming" | "UnknownCommand" | "NotJson" | "BadField" | "NotCanonical";

export class CtrlError extends Error {
  constructor(readonly kind: CtrlErrorKind) {
    super(`CTRL rejected: ${kind}`);
    this.name = "CtrlError";
  }
}

export const CTRL_LIMITS = Object.freeze({
  byeReasonBytes: 64,
  desktopNameBytes: 128,
  inputLostHandles: 4096,
  counterLimit: 2 ** 32,
  maxCatalogVersion: 2 ** 53 - 1,
});

const PART_COMMANDS = new Set(["part-begin", "part", "part-end", "part-abort"]);
const COMMANDS = new Set(["hello", "bye", "pair-confirm", "pair-done", "identity-rotate", "input_lost", "input_lost_ack", "control_taken"]);

const B64URL = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/** Unpadded base64url (the crate's `b64`). */
export function toBase64Url(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i]!;
    const b = bytes[i + 1];
    const c = bytes[i + 2];
    out += B64URL[a >> 2]! + B64URL[((a & 3) << 4) | ((b ?? 0) >> 4)]!;
    if (b !== undefined) out += B64URL[((b & 15) << 2) | ((c ?? 0) >> 6)]!;
    if (c !== undefined) out += B64URL[c & 63]!;
  }
  return out;
}

/** Strict, canonical unpadded base64url decode (null otherwise). */
export function fromBase64Url(text: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]*$/u.test(text) || text.length % 4 === 1) return null;
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const ch of text) {
    acc = (acc << 6) | B64URL.indexOf(ch);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 255);
    }
    acc &= (1 << bits) - 1;
  }
  const bytes = Uint8Array.from(out);
  return toBase64Url(bytes) === text ? bytes : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, required: readonly string[], optional: readonly string[] = []): boolean {
  const keys = Object.keys(value);
  return required.every((k) => k in value) && keys.every((k) => required.includes(k) || optional.includes(k));
}

const u = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const str = (v: unknown): v is string => typeof v === "string";

function byteLen(text: string): number {
  return utf8Encode(text).length;
}

function body(ctrl: Ctrl): string {
  switch (ctrl.kind) {
    case "hello":
      return ctrl.resume === null
        ? "{}"
        : JSON.stringify({ resume: { chan: toHex(ctrl.resume.chan), last_sent_ctr: ctrl.resume.lastSentCtr } });
    case "bye": return JSON.stringify({ reason: ctrl.reason });
    case "pair-confirm": return JSON.stringify({ h: toBase64Url(ctrl.h) });
    case "pair-done": return JSON.stringify({ desktop_name: ctrl.desktopName, catalog_version: ctrl.catalogVersion });
    case "identity-rotate":
      return ctrl.newPushPub === null
        ? JSON.stringify({ new_pub: toBase64Url(ctrl.newPub) })
        : JSON.stringify({ new_pub: toBase64Url(ctrl.newPub), new_push_pub: toBase64Url(ctrl.newPushPub) });
    case "input_lost":
    case "control_taken": return JSON.stringify({ handles: ctrl.handles });
    case "input_lost_ack": return "{}";
  }
}

function validate(ctrl: Ctrl): void {
  const bad = () => { throw new CtrlError("BadField"); };
  switch (ctrl.kind) {
    case "hello":
      if (ctrl.resume) {
        if (ctrl.resume.chan.length !== 16) bad();
        const c = ctrl.resume.lastSentCtr;
        if (c !== null && (!u(c) || c >= CTRL_LIMITS.counterLimit)) bad();
      }
      return;
    case "bye": {
      const n = byteLen(ctrl.reason);
      if (n < 1 || n > CTRL_LIMITS.byeReasonBytes) bad();
      return;
    }
    case "pair-confirm": if (ctrl.h.length !== 32) bad(); return;
    case "pair-done": {
      const n = byteLen(ctrl.desktopName);
      if (n < 1 || n > CTRL_LIMITS.desktopNameBytes) bad();
      if (!u(ctrl.catalogVersion) || ctrl.catalogVersion < 1 || ctrl.catalogVersion > CTRL_LIMITS.maxCatalogVersion) bad();
      return;
    }
    case "identity-rotate":
      if (ctrl.newPub.length !== 32 || (ctrl.newPushPub !== null && ctrl.newPushPub.length !== 32)) bad();
      return;
    case "input_lost":
    case "control_taken": {
      if (ctrl.handles.length > CTRL_LIMITS.inputLostHandles) bad();
      let prev = -1;
      for (const h of ctrl.handles) {
        if (!u(h) || h >= CTRL_LIMITS.counterLimit || h <= prev) bad();
        prev = h;
      }
      return;
    }
    case "input_lost_ack": return;
  }
}

/** The canonical CTRL content bytes. Throws CtrlError("BadField") when out of range. */
export function encodeCtrl(ctrl: Ctrl): Uint8Array {
  validate(ctrl);
  return utf8Encode(`${ctrl.kind} ${body(ctrl)}`);
}

function key32(value: unknown): Uint8Array {
  if (!str(value)) throw new CtrlError("NotJson");
  const bytes = fromBase64Url(value);
  if (!bytes || bytes.length !== 32) throw new CtrlError("BadField");
  return bytes;
}

function shape(name: string, json: unknown): Ctrl {
  const notJson = (): never => { throw new CtrlError("NotJson"); };
  if (!isRecord(json)) return notJson();
  switch (name) {
    case "hello": {
      if (!exactKeys(json, [], ["resume"])) return notJson();
      if (!("resume" in json)) return { kind: "hello", resume: null };
      const r = json.resume;
      if (!isRecord(r) || !exactKeys(r, ["chan", "last_sent_ctr"]) || !str(r.chan)) return notJson();
      if (r.last_sent_ctr !== null && !u(r.last_sent_ctr)) return notJson();
      if (!/^[0-9a-f]{32}$/u.test(r.chan)) throw new CtrlError("BadField");
      const chan = Uint8Array.from(r.chan.match(/../gu)!.map((p) => parseInt(p, 16)));
      return { kind: "hello", resume: { chan, lastSentCtr: r.last_sent_ctr as number | null } };
    }
    case "bye":
      if (!exactKeys(json, ["reason"]) || !str(json.reason)) return notJson();
      return { kind: "bye", reason: json.reason };
    case "pair-confirm":
      if (!exactKeys(json, ["h"])) return notJson();
      return { kind: "pair-confirm", h: key32(json.h) };
    case "pair-done":
      if (!exactKeys(json, ["desktop_name", "catalog_version"]) || !str(json.desktop_name) || !u(json.catalog_version)) return notJson();
      return { kind: "pair-done", desktopName: json.desktop_name, catalogVersion: json.catalog_version };
    case "identity-rotate":
      if (!exactKeys(json, ["new_pub"], ["new_push_pub"])) return notJson();
      return {
        kind: "identity-rotate",
        newPub: key32(json.new_pub),
        newPushPub: "new_push_pub" in json ? key32(json.new_push_pub) : null,
      };
    case "input_lost":
      if (!exactKeys(json, ["handles"]) || !Array.isArray(json.handles)) return notJson();
      if (!json.handles.every((h) => u(h) && h < CTRL_LIMITS.counterLimit)) return notJson();
      return { kind: "input_lost", handles: json.handles as number[] };
    case "control_taken":
      if (!exactKeys(json, ["handles"]) || !Array.isArray(json.handles)) return notJson();
      if (!json.handles.every((h) => u(h) && h < CTRL_LIMITS.counterLimit)) return notJson();
      return { kind: "control_taken", handles: json.handles as number[] };
    case "input_lost_ack":
      if (!exactKeys(json, [])) return notJson();
      return { kind: "input_lost_ack" };
    default:
      throw new CtrlError("UnknownCommand");
  }
}

/** Split `<name> <json>` (the native split_ctrl framing). */
export function splitCtrl(content: Uint8Array): { name: string; json: string } {
  const text = utf8Decode(content);
  if (text === null) throw new CtrlError("BadFraming");
  const space = text.indexOf(" ");
  if (space <= 0 || space === text.length - 1) throw new CtrlError("BadFraming");
  return { name: text.slice(0, space), json: text.slice(space + 1) };
}

/**
 * Strictly decode CTRL content. `null` for a part command (handled by the
 * part protocol). Throws CtrlError otherwise.
 */
export function decodeCtrl(content: Uint8Array): Ctrl | null {
  const { name, json } = splitCtrl(content);
  if (PART_COMMANDS.has(name)) return null;
  if (!COMMANDS.has(name)) throw new CtrlError("UnknownCommand");
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new CtrlError("NotJson");
  }
  const ctrl = shape(name, parsed);
  validate(ctrl);
  if (!bytesEqual(encodeCtrl(ctrl), content)) throw new CtrlError("NotCanonical");
  return ctrl;
}

/** The command word of CTRL content, or null if it isn't framed as CTRL. */
export function ctrlName(content: Uint8Array): string | null {
  try {
    return splitCtrl(content).name;
  } catch {
    return null;
  }
}
