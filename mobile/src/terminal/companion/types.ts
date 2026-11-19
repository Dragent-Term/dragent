/**
 * Companion per-tab terminal bridge types.
 *
 * Pure domain state only: no React, no native imports, no sockets. The hook
 * owns timers/foreground gating; the engine owns offsets/modes/masking.
 * Rendering stays in `WasmTerminalView` + `TerminalRuntimeStore` — this
 * module never parses VT and never fabricates scrollback.
 */

/** Poll cadence: sequential, bounded, foreground + ready + online only. */
export const COMPANION_TERMINAL_POLL_MS = 200;

/** Client-side cap: at most 4 active handles, never evict another tab. */
export const COMPANION_TERMINAL_MAX_HANDLES = 4;

export type CompanionTerminalMode = "raw" | "snapshot";

export type CompanionTerminalPhase =
  | "attaching"
  | "live-raw"
  | "live-snapshot"
  | "closed"
  | "unavailable"
  | "rejected"
  | "error"
  | "detached";

/**
 * Owner binding for stale-result masking. A result applies only when every
 * field matches the current owner: account, device, tab, rpc client identity
 * and socket epoch. A new account or epoch restarts at 1, so epoch alone
 * would leak the previous generation without the client check.
 */
export interface CompanionTerminalOwner {
  readonly accountId: string | null;
  readonly deviceId: string;
  readonly tabId: number;
  readonly client: unknown;
  readonly epoch: number;
}

export interface CompanionTerminalEngineStatus {
  readonly phase: CompanionTerminalPhase;
  /** User-safe, no tokens or raw daemon text. */
  readonly message: string | null;
  readonly mode: CompanionTerminalMode;
  readonly handle: string | null;
  readonly offset: number;
  readonly alive: boolean;
  readonly exitCode: number | null;
  readonly canWrite: boolean;
  readonly canResize: boolean;
  /**
   * The desktop took this terminal back and released the handle. The screen
   * leaves the tab; opening it again takes the terminal over.
   */
  readonly takenByDesktop: boolean;
}

export const COMPANION_TERMINAL_MESSAGES = Object.freeze({
  attaching: "Attaching to the live terminal…",
  waking: "Waking the agent on your desktop…",
  reconnecting: "Reconnecting to your desktop…",
  waiting: "Waiting for terminal output…",
  snapshot: "Showing the live screen. Earlier scrollback is unavailable.",
  snapshotCapped:
    "Terminal history grew beyond the local buffer. Showing the live screen instead; no input was replayed.",
  screenUnavailable: "The live screen is temporarily unavailable. Retrying…",
  closed: "The terminal has closed on the desktop.",
  detached: "Detached. The desktop terminal keeps running.",
  offline: "Paused while the machine is offline or the connection drops. Nothing is replayed.",
  rejected: "Too many live terminals are already attached. Close another tab and try again.",
  unavailable: "This live tab is unavailable on that machine.",
  signIn: "Pair this phone to view this live terminal.",
  invalid: "The relay response was unreadable. Retrying…",
  reattaching: "The desktop restarted its terminal session. Reattaching…",
  reattachFailed: "The terminal session could not be reattached. Retry to try again.",
  // Pre-wire sync gate: truthfully nothing reached the wire.
  sendNotReady: "The terminal is not ready. Input was not sent.",
  resizeNotReady: "The terminal is not ready. Resize was not sent.",
  // Post-wire transport failure: delivery is unknown, never claim unsent.
  // No replay: check the terminal before resending.
  sendUnconfirmed:
    "Input delivery could not be confirmed. It was not replayed — check the terminal before resending.",
  // The desktop took the terminal back: it keeps the desktop's size until
  // the person chooses to use it on the phone again.
  desktopInControl: "Your desktop took this terminal back. Open the tab again to use it here.",
  resizeUnconfirmed:
    "Resize delivery could not be confirmed. It was not replayed — check the terminal before resizing again.",
});

export function isSameCompanionOwner(a: CompanionTerminalOwner, b: CompanionTerminalOwner): boolean {
  return (
    a.accountId === b.accountId &&
    a.deviceId === b.deviceId &&
    a.tabId === b.tabId &&
    a.client === b.client &&
    a.epoch === b.epoch
  );
}
