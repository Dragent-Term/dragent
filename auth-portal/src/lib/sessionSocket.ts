// Browser client for the shared-session realtime gateway. It speaks the same
// JSON protocol as the desktop app (see the Rust `dragent_session_protocol`
// crate and the Go `internal/session/types`): it joins as a viewer, tracks the
// participant list and this viewer's role, and can request Edit (control).
//
// Terminal output (`delta` frames) is surfaced via `onDelta`; the join page
// forwards those frames to the WASM-backed terminal surface. This client owns
// the transport and remains independent from the renderer.

export const PROTOCOL_VERSION = 2;

export type Role = "host" | "controller" | "viewer";

/** An RGBA color resolved against the host theme: [r, g, b, a], each 0–255. */
export type WireColor = [number, number, number, number];

/** A styled run of text within a terminal line (mirrors the desktop TextSpan). */
export interface WireSpan {
  text: string;
  fg?: WireColor;
  bg?: WireColor;
  /** SGR attribute bits (bold/italic/underline…) — the desktop Flags::SGR_MASK layout. */
  flags?: number;
}

/** One rendered output line — the wire form of the desktop ColoredLine. */
export interface WireLine {
  spans: WireSpan[];
}

// ── Agent blocks (AI conversation) ───────────────────────────────────────────

export interface WireDiffLine {
  kind: "context" | "add" | "remove";
  text: string;
}
export interface WireFileDiff {
  path: string;
  added: number;
  removed: number;
  lines: WireDiffLine[];
  truncated?: boolean;
}
export interface WireDiff {
  files: WireFileDiff[];
}

export interface WireAgentAuthor {
  display_name: string;
  avatar_url?: string;
}

/** Read-only projection of one agent block (mirrors the Rust `WireAgentBlock`). */
export type WireAgentBlock = { id: number; ts_ms: number } & (
  | { kind: "user_query"; text: string; author?: WireAgentAuthor | null }
  | { kind: "agent_message"; text: string; streaming?: boolean }
  | { kind: "system_note"; text: string }
  | { kind: "thinking"; content: string; complete?: boolean }
  | {
      kind: "tool_step";
      name: string;
      icon: string;
      args: string;
      status: "running" | "done" | "error";
      result?: string | null;
      duration_ms?: number | null;
      diff?: WireDiff | null;
    }
  | { kind: "action_result"; command: string; exit_code?: number | null; output_preview: string }
  | { kind: "approval"; command: string; reason: string; status: string }
  | { kind: "question"; header: string; question: string; options: string[]; status: string }
  | { kind: "plan_review"; content: string; status: string }
  | { kind: "queued"; text: string }
  | { kind: "loading"; message: string }
  | { kind: "error"; message: string }
  | {
      kind: "turn_group";
      children: WireAgentBlock[];
      duration_ms?: number | null;
      status_note?: string | null;
      expanded?: boolean;
    }
  | { kind: "activity_group"; blocks: WireAgentBlock[]; expanded?: boolean }
  | { kind: "empty" }
);

export type TerminalDeltaKind =
  | "surface_state"
  | "block_started"
  | "block_lines"
  | "block_completed"
  | "output_chunk"
  | "alt_screen"
  | "alt_screen_ended"
  | "agent_blocks";

export interface TerminalDelta {
  kind?: TerminalDeltaKind;
  // surface_state
  state?: TerminalSurfaceState;
  // block_started / block_lines / block_completed / output_chunk
  block_id?: string;
  command?: string;
  cwd?: string | null;
  ts_ms?: number;
  // block_lines
  base_line?: number;
  lines?: WireLine[];
  // block_completed
  exit_code?: number | null;
  duration_ms?: number | null;
  failed?: boolean;
  // output_chunk (raw stream — the styled viewer ignores it)
  seq?: number;
  data_base64?: string;
  // alt_screen (styled full-screen grid)
  rows?: number;
  cols?: number;
  cursor_row?: number;
  cursor_col?: number;
  cursor_visible?: boolean;
  // agent_blocks
  blocks?: WireAgentBlock[];
}

export interface TerminalSurfaceState {
  command_input: string;
  command_ghost?: string | null;
  command_cursor: number;
  ai_tool?: string | null;
  ai_input: string;
  ai_ghost?: string | null;
  ai_cursor: number;
  ai_status?: string | null;
  attachments: string[];
  activity?: string | null;
  side_command?: string | null;
  command_running: boolean;
  cwd?: string | null;
}

export interface Participant {
  id: string;
  display_name: string;
  avatar_url?: string;
  role: Role;
  present: boolean;
}

export interface SessionSocketHandlers {
  /** Admitted to the session; `role` is "viewer" or "controller" (Edit). */
  onJoined?: (participant: string, role: Role) => void;
  /** The full participant list changed. */
  onParticipants?: (participants: Participant[]) => void;
  /** This viewer's role changed (e.g. the host granted Edit). */
  onRoleChanged?: (role: Role) => void;
  /** This viewer's Edit request was declined by the host. */
  onAccessDenied?: () => void;
  /** A terminal-output frame consumed by the browser terminal renderer. */
  onDelta?: (delta: TerminalDelta) => void;
  /** The session ended or this viewer was removed. `loginRequired` is true when
   *  the gateway refused an anonymous connection to an Edit link. */
  onClosed?: (reason: string, loginRequired: boolean) => void;
  /** A transport error (socket failed to open, etc.). */
  onError?: (message: string) => void;
}

export interface SessionSocketOptions {
  gateway: string;
  session: string;
  secret: string;
  deviceId: string;
  displayName: string;
  /** Access token for Edit joins; omit for anonymous view. */
  token?: string;
}

const LOGIN_REQUIRED_HINT = "login required";

export class SessionSocket {
  private ws: WebSocket | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private closedByServer = false;
  private manualClose = false;

  constructor(
    private readonly opts: SessionSocketOptions,
    private readonly handlers: SessionSocketHandlers,
  ) {}

  connect() {
    const url = new URL(this.opts.gateway);
    if (this.opts.token) {
      url.searchParams.set("access_token", this.opts.token);
    }

    let ws: WebSocket;
    try {
      ws = new WebSocket(url.toString());
    } catch (e) {
      this.handlers.onError?.(e instanceof Error ? e.message : "socket error");
      return;
    }
    this.ws = ws;

    ws.onopen = () => {
      this.send({
        type: "join",
        protocol_version: PROTOCOL_VERSION,
        session: this.opts.session,
        secret: this.opts.secret,
        device_id: this.opts.deviceId,
        display_name: this.opts.displayName,
      });
      // Keepalive so idle connections aren't reaped by proxies.
      this.pingTimer = setInterval(() => this.send({ type: "ping" }), 25_000);
    };

    ws.onmessage = (ev) => this.handleMessage(ev.data);

    ws.onerror = () => {
      if (!this.closedByServer && !this.manualClose) {
        this.handlers.onError?.("connection error");
      }
    };

    ws.onclose = () => {
      this.stopPing();
      if (this.manualClose || this.closedByServer) return;
      // Closed without an explicit `ended`/`removed` frame.
      this.handlers.onClosed?.("disconnected", false);
    };
  }

  /** Ask the host to upgrade this viewer to Edit (control). */
  requestControl() {
    this.send({ type: "request_control" });
  }

  /** Forward keyboard/input bytes to the host when this participant has Edit. */
  sendInput(bytes: Uint8Array) {
    if (bytes.length === 0) return;
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    this.send({
      type: "input_bytes",
      data_base64: btoa(binary),
    });
  }

  close() {
    this.manualClose = true;
    this.stopPing();
    this.ws?.close();
    this.ws = null;
  }

  private handleMessage(raw: unknown) {
    if (typeof raw !== "string") return;
    let msg: { type?: string; [k: string]: unknown };
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    switch (msg.type) {
      case "joined":
        this.handlers.onJoined?.(
          String(msg.participant ?? ""),
          (msg.role as Role) ?? "viewer",
        );
        break;
      case "participants_updated":
        this.handlers.onParticipants?.(
          Array.isArray(msg.participants) ? (msg.participants as Participant[]) : [],
        );
        break;
      case "role_changed":
        this.handlers.onRoleChanged?.((msg.role as Role) ?? "viewer");
        break;
      case "access_denied":
        this.handlers.onAccessDenied?.();
        break;
      case "delta":
        if (msg.delta && typeof msg.delta === "object") {
          this.handlers.onDelta?.(msg.delta as TerminalDelta);
        }
        break;
      case "removed":
      case "ended": {
        this.closedByServer = true;
        const reason = String(msg.reason ?? "session ended");
        const loginRequired = reason.toLowerCase().includes(LOGIN_REQUIRED_HINT);
        this.handlers.onClosed?.(reason, loginRequired);
        this.stopPing();
        this.ws?.close();
        break;
      }
      case "pong":
        break;
      default:
        break;
    }
  }

  private send(obj: unknown) {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(obj));
    }
  }

  private stopPing() {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/** A stable per-browser device id (persisted), mirroring the desktop app. */
export function browserDeviceId(): string {
  const KEY = "dragent_device_id";
  try {
    let id = localStorage.getItem(KEY);
    if (!id) {
      id = randomId();
      localStorage.setItem(KEY, id);
    }
    return id;
  } catch {
    return randomId();
  }
}

const GUEST_ADJECTIVES = [
  "Swift", "Calm", "Bright", "Bold", "Keen", "Clever", "Quiet", "Brave",
];
const GUEST_ANIMALS = [
  "Falcon", "Otter", "Lynx", "Heron", "Fox", "Wren", "Ibex", "Marten",
];

/** A friendly generic name for an anonymous viewer, persisted per browser. */
export function guestDisplayName(): string {
  const KEY = "dragent_guest_name";
  try {
    const existing = localStorage.getItem(KEY);
    if (existing) return existing;
    const name = `${pick(GUEST_ADJECTIVES)} ${pick(GUEST_ANIMALS)}`;
    localStorage.setItem(KEY, name);
    return name;
  } catch {
    return `${pick(GUEST_ADJECTIVES)} ${pick(GUEST_ANIMALS)}`;
  }
}

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

function randomId(): string {
  if (typeof crypto !== "undefined" && crypto.randomUUID) return crypto.randomUUID();
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}
