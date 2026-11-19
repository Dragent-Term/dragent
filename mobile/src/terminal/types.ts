import type { TerminalSurfaceState, WireAgentBlock, WireColor } from "../transport";

export interface TerminalSpan {
  readonly text: string;
  readonly fg: Readonly<WireColor> | null;
  readonly bg: Readonly<WireColor> | null;
  readonly flags: number;
}

export interface TerminalLine {
  /** Absolute row within its command block; gaps do not allocate empty rows. */
  readonly index: number;
  readonly spans: readonly TerminalSpan[];
  readonly truncated: boolean;
}

export interface TerminalBlock {
  readonly id: string;
  readonly command: string;
  readonly cwd: string | null;
  readonly tsMs: number;
  readonly status: "running" | "completed";
  readonly exitCode: number | null;
  readonly durationMs: number | null;
  readonly failed: boolean;
  readonly lines: readonly TerminalLine[];
  /** Replayed updates below this row cannot resurrect discarded history. */
  readonly discardedBeforeLine: number;
  readonly truncated: boolean;
}

/** A bounded, plain-text projection, never executable agent UI or arbitrary JSON. */
export interface TerminalAgentBlock {
  readonly id: number;
  readonly tsMs: number;
  readonly kind: WireAgentBlock["kind"] | "unknown";
  readonly label: string;
  readonly text: string;
  readonly status: string | null;
  readonly truncated: boolean;
}

export interface TerminalAltScreen {
  readonly rows: number;
  readonly columns: number;
  readonly cursorRow: number;
  readonly cursorColumn: number;
  readonly cursorVisible: boolean;
  readonly lines: readonly TerminalLine[];
  readonly truncated: boolean;
}

export interface TerminalState {
  readonly blocks: readonly TerminalBlock[];
  readonly agentBlocks: readonly TerminalAgentBlock[];
  readonly altScreen: TerminalAltScreen | null;
  readonly surface: TerminalSurfaceState | null;
  readonly styledOutputSeen: boolean;
  /** Raw PTY bytes have arrived, but a styled snapshot has not arrived yet. */
  readonly rawOutputPending: boolean;
  readonly historyTruncated: boolean;
}
