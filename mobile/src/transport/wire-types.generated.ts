// Generated from auth-portal/src/lib/sessionSocket.ts. Do not edit by hand.
// Regenerate: node mobile/src/transport/generate-wire-types.mjs

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
