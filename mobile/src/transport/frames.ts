/** Untrusted JSON boundary. No platform APIs or connection state live here. */
import type { Participant, TerminalDelta, TerminalSurfaceState } from "./protocol";

export const MAX_FRAME_CHARS = 8 * 1024 * 1024;
export const MAX_PARTICIPANTS = 512;
/** Agent queries: Go hub limit. Commands/input retain the client's bounded-send policy. */
export const MAX_WRITE_BYTES = 64 * 1024;
export const REALTIME_WRITE_LIMITS = Object.freeze({
  agentQuery: MAX_WRITE_BYTES, command: MAX_WRITE_BYTES, input: MAX_WRITE_BYTES,
});
const MAX_ITEMS = 65_536;
const MAX_AGENT_BLOCKS = 4096;
const MAX_AGENT_DEPTH = 16;

type Check = (value: unknown) => boolean;
type RecordValue = Record<string, unknown>;
export type ViewerRole = "viewer" | "controller";

export type RpcReplyFrame =
  | { type: "rpc-reply"; id: string; ok: true; result: unknown }
  | { type: "rpc-reply"; id: string; ok: false; error: { message: string; loginRequired: boolean; code?: string } };

export type RpcStreamFrame = { type: "rpc-stream"; stream: string; seq: number; payload: unknown };

export type ServerFrame =
  | { type: "joined"; participant: string; role: ViewerRole; is_host_account?: boolean }
  | { type: "participants_updated"; participants: Participant[] }
  | { type: "role_changed"; role: ViewerRole }
  | { type: "delta"; delta: TerminalDelta }
  | { type: "ended" | "removed"; reason: string }
  | { type: "access_denied" | "pong" | "replay_complete" }
  | RpcReplyFrame
  | RpcStreamFrame;

function record(value: unknown): value is RecordValue {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
const text: Check = (value) => typeof value === "string";
const bool: Check = (value) => typeof value === "boolean";
const integer: Check = (value) => Number.isSafeInteger(value);
const unsigned: Check = (value) => integer(value) && (value as number) >= 0;
const shortText: Check = (value) => text(value) && (value as string).length <= 4096;
const id: Check = (value) => shortText(value) && (value as string).length > 0;
const viewerRole: Check = (value) => value === "viewer" || value === "controller";
const optional = (check: Check): Check => (value) => value === undefined || check(value);
const nullable = (check: Check): Check => (value) => value === null || check(value);
const maybe = (check: Check): Check => optional(nullable(check));
const array = (check: Check, limit = MAX_ITEMS): Check => (value) =>
  Array.isArray(value) && value.length <= limit && value.every(check);

function fields(value: unknown, checks: Record<string, Check>): value is RecordValue {
  return record(value) && Object.entries(checks).every(([key, check]) => check(value[key]));
}

const color: Check = (value) => Array.isArray(value) && value.length === 4 &&
  value.every((channel: unknown) => unsigned(channel) && (channel as number) <= 255);
const line: Check = (value) => fields(value, {
  spans: array((span) => fields(span, {
    text, fg: optional(color), bg: optional(color), flags: optional(unsigned),
  })),
});
const participant: Check = (value) => fields(value, {
  id, display_name: shortText, avatar_url: optional(shortText),
  role: (role) => viewerRole(role) || role === "host", present: bool,
});
const diff: Check = (value) => fields(value, {
  files: array((file) => fields(file, {
    path: text, added: unsigned, removed: unsigned, truncated: optional(bool),
    lines: array((entry) => fields(entry, {
      kind: (kind) => kind === "context" || kind === "add" || kind === "remove", text,
    })),
  })),
});

/** Limit both nesting and total blocks, including blocks nested in groups. */
function agentBlocks(value: unknown): boolean {
  let remaining = MAX_AGENT_BLOCKS;
  const block = (entry: unknown, depth: number): boolean => {
    if (--remaining < 0 || depth > MAX_AGENT_DEPTH || !fields(entry, {
      id: unsigned, ts_ms: integer,
    })) return false;
    switch (entry.kind) {
      case "user_query":
        return fields(entry, {
          text, author: maybe((author) => fields(author, {
            display_name: text, avatar_url: optional(text),
          })),
        });
      case "agent_message": return fields(entry, { text, streaming: optional(bool) });
      case "system_note":
      case "queued": return fields(entry, { text });
      case "thinking": return fields(entry, { content: text, complete: optional(bool) });
      case "tool_step": return fields(entry, {
        name: text, icon: text, args: text,
        status: (status) => status === "running" || status === "done" || status === "error",
        result: maybe(text), duration_ms: maybe(integer), diff: maybe(diff),
      });
      case "action_result": return fields(entry, {
        command: text, exit_code: maybe(integer), output_preview: text,
      });
      case "approval": return fields(entry, { command: text, reason: text, status: text });
      case "question": return fields(entry, {
        header: text, question: text, options: array(text), status: text,
      });
      case "plan_review": return fields(entry, { content: text, status: text });
      case "loading":
      case "error": return fields(entry, { message: text });
      case "turn_group": return fields(entry, {
        children: array((child) => block(child, depth + 1), MAX_AGENT_BLOCKS),
        duration_ms: maybe(integer), status_note: maybe(text), expanded: optional(bool),
      });
      case "activity_group": return fields(entry, {
        blocks: array((child) => block(child, depth + 1), MAX_AGENT_BLOCKS),
        expanded: optional(bool),
      });
      case "empty": return true;
      default: return false;
    }
  };
  return array((entry) => block(entry, 0), MAX_AGENT_BLOCKS)(value);
}

function surface(value: unknown): TerminalSurfaceState | null {
  if (!fields(value, {
    command_input: optional(text), command_ghost: maybe(text), command_cursor: optional(unsigned),
    ai_tool: maybe(text), ai_input: optional(text), ai_ghost: maybe(text),
    ai_cursor: optional(unsigned), ai_status: maybe(text), attachments: optional(array(text)),
    activity: maybe(text), side_command: maybe(text), command_running: optional(bool), cwd: maybe(text),
  })) return null;
  // Rust defaults these fields; older hosts can omit them.
  return {
    command_input: "", command_cursor: 0, ai_input: "", ai_cursor: 0,
    attachments: [], command_running: false, ...value,
  } as TerminalSurfaceState;
}

function terminalDelta(value: unknown): TerminalDelta | null {
  if (!record(value)) return null;
  let valid = false;
  switch (value.kind) {
    case "surface_state": {
      const state = surface(value.state);
      return state ? { kind: "surface_state", state } : null;
    }
    case "block_started":
      valid = fields(value, { block_id: id, command: text, cwd: maybe(text), ts_ms: optional(integer) });
      break;
    case "block_lines":
      valid = fields(value, { block_id: id, base_line: unsigned, lines: array(line) });
      break;
    case "block_completed":
      valid = fields(value, {
        block_id: id, exit_code: maybe(integer), duration_ms: maybe(unsigned), failed: optional(bool),
      });
      break;
    case "output_chunk":
      valid = fields(value, { block_id: id, seq: unsigned, data_base64: text });
      break;
    case "alt_screen":
      valid = fields(value, {
        rows: unsigned, cols: unsigned, cursor_row: unsigned, cursor_col: unsigned,
        cursor_visible: bool, lines: array(line),
      });
      break;
    case "alt_screen_ended": valid = true; break;
    case "agent_blocks": valid = agentBlocks(value.blocks); break;
  }
  return valid ? value as TerminalDelta : null;
}

/**
 * Relay RPC envelope (R0): strict hand-written readers in the
 * `relayCatalog.ts` style — null on any bad shape, never throws, never
 * surfaces parser internals. `v` must be 1; reply ids reuse the request-id
 * validity check; stream `seq` is an unsigned integer.
 */
/** The desktop's closed set of in-channel error codes, in plain words. */
export const DESKTOP_ERROR_MESSAGES: Readonly<Record<string, string>> = Object.freeze({
  invalid: "The desktop couldn't read that request.",
  unavailable: "That isn't available on the desktop right now.",
  gone: "The desktop session changed. Try again.",
});

function rpcReply(value: RecordValue): RpcReplyFrame | null {
  if (value.v !== 1 || !isValidRequestId(value.id)) return null;
  const id = value.id as string;
  if (value.ok === true) {
    if (!("result" in value) || value.result === undefined) return null;
    return { type: "rpc-reply", id, ok: true, result: value.result };
  }
  if (value.ok === false) {
    const error = value.error;
    if (!record(error)) return null;
    // Desktop replies inside the end-to-end channel carry a closed error code
    // instead of text (docs/design/e2ee.md §4.4 JSON); map it to plain words.
    if (error.message === undefined && typeof error.code === "string") {
      const message = DESKTOP_ERROR_MESSAGES[error.code];
      if (message === undefined || Object.keys(error).length !== 1) return null;
      return { type: "rpc-reply", id, ok: false, error: { message, loginRequired: false, code: error.code } };
    }
    const message = error.message;
    if (typeof message !== "string" || message.trim() === "" || message.length > 4096) return null;
    if (typeof error.loginRequired !== "boolean") return null;
    // The desktop also says why in a short code (`desktop_in_control`,
    // `gone`, …). Screens act on it, so it must survive alongside the words.
    const code = typeof error.code === "string" && /^[a-z0-9_]{1,64}$/u.test(error.code) ? error.code : undefined;
    return {
      type: "rpc-reply",
      id,
      ok: false,
      error: code === undefined ? { message, loginRequired: error.loginRequired } : { message, loginRequired: error.loginRequired, code },
    };
  }
  return null;
}

function rpcStream(value: RecordValue): RpcStreamFrame | null {
  if (value.v !== 1) return null;
  const stream = value.stream;
  if (!id(stream)) return null;
  if (!unsigned(value.seq)) return null;
  if (!("payload" in value) || value.payload === undefined) return null;
  return { type: "rpc-stream", stream: stream as string, seq: value.seq as number, payload: value.payload };
}

/** Ignore malformed and unknown messages; never expose parser exceptions. */
export function parseServerFrame(raw: unknown): ServerFrame | null {
  if (typeof raw !== "string" || raw.length > MAX_FRAME_CHARS) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (!record(value)) return null;
    switch (value.type) {
      case "joined":
        return fields(value, {
          participant: id, role: viewerRole, is_host_account: optional(bool),
        }) ? value as ServerFrame : null;
      case "participants_updated":
        return fields(value, {
          participants: array(participant, MAX_PARTICIPANTS),
        }) ? value as ServerFrame : null;
      case "role_changed":
        return viewerRole(value.role) ? value as ServerFrame : null;
      case "delta": {
        const delta = terminalDelta(value.delta);
        return delta ? { type: "delta", delta } : null;
      }
      case "ended":
      case "removed":
        return typeof value.reason === "string"
          ? { type: value.type, reason: value.reason } : null;
      case "access_denied":
      case "pong": return { type: value.type };
      case "replay_complete":
        return raw.length <= 128 && Object.keys(value).length === 1 ? { type: "replay_complete" } : null;
      case "rpc-reply": return rpcReply(value);
      case "rpc-stream": return rpcStream(value);
      default: return null;
    }
  } catch {
    return null;
  }
}

/** Match UTF-8 encoders and Go JSON decoding for unpaired UTF-16 surrogates. */
function* utf8Points(text: string): Generator<number> {
  for (const character of text) {
    let point = character.codePointAt(0)!;
    if (point >= 0xd800 && point <= 0xdfff) point = 0xfffd;
    yield point;
  }
}

export function utf8ByteLength(text: string): number {
  let bytes = 0;
  for (const point of utf8Points(text)) bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
  return bytes;
}

/** Byte size only; does not trim, reject blank prompts, or alter terminal input. */
export function fitsWriteLimit(text: string, maxBytes = MAX_WRITE_BYTES): boolean {
  return text.length <= maxBytes && utf8ByteLength(text) <= maxBytes;
}

/** Go strings.TrimSpace uses Unicode White_Space, including NEL but excluding BOM. */
export function normalizeAgentQuery(query: string): string {
  // oxlint-disable-next-line no-control-regex -- Go TrimSpace includes ASCII tab/newline controls.
  return query.replace(/^[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+|[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+$/g, "");
}

export type SessionTextKind = keyof typeof REALTIME_WRITE_LIMITS;
export type SessionTextResult =
  | { ok: true; text: string; byteLength: number }
  | { ok: false; error: "invalid-text" | "empty" | "too-large" };

/** One validation boundary for both UI feedback and the actual outbound send. */
export function prepareSessionText(kind: SessionTextKind, value: unknown): SessionTextResult {
  if (typeof value !== "string") return { ok: false, error: "invalid-text" };
  const text = kind === "agentQuery" ? normalizeAgentQuery(value) : value;
  if (text.length === 0) return { ok: false, error: "empty" };
  const limit = REALTIME_WRITE_LIMITS[kind];
  if (text.length > limit) return { ok: false, error: "too-large" };
  const byteLength = utf8ByteLength(text);
  return byteLength > limit ? { ok: false, error: "too-large" } : { ok: true, text, byteLength };
}

/** The native generator emits canonical UUIDs; nil is explicitly rejected by Go. */
export function isValidRequestId(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value) &&
    value !== "00000000-0000-0000-0000-000000000000";
}

/** UTF-8 + base64 without Node Buffer or a React Native polyfill. */
export function encodeInput(text: string): string {
  const bytes: number[] = [];
  for (const point of utf8Points(text)) {
    if (point < 0x80) bytes.push(point);
    else if (point < 0x800) bytes.push(0xc0 | (point >> 6), 0x80 | (point & 63));
    else if (point < 0x10000) bytes.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
    else bytes.push(0xf0 | (point >> 18), 0x80 | ((point >> 12) & 63), 0x80 | ((point >> 6) & 63), 0x80 | (point & 63));
  }
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const result: string[] = [];
  for (let i = 0; i < bytes.length; i += 3) {
    const first = bytes[i]!;
    const second = bytes[i + 1];
    const third = bytes[i + 2];
    result.push(
      alphabet[first >> 2]!, alphabet[((first & 3) << 4) | ((second ?? 0) >> 4)]!,
      second === undefined ? "=" : alphabet[((second & 15) << 2) | ((third ?? 0) >> 6)]!,
      third === undefined ? "=" : alphabet[third & 63]!,
    );
  }
  return result.join("");
}
