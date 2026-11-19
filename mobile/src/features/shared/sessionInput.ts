/**
 * Pure terminal/agent input semantics for the session composer.
 *
 * No React Native imports: this module is unit-tested with tsx/node:test.
 * Text validation runs through the shared transport boundary
 * (`prepareSessionText` in transport/frames), so byte limits and agent
 * trimming stay in one place. This module owns the composer contract on top
 * of it: which keystrokes map to which canonical escape sequences, when a
 * draft may be sent, and how drafts evolve across edits, sends, and session
 * switches.
 */
import {
  MAX_WRITE_BYTES,
  prepareSessionText,
} from "../../transport/frames";

export type TerminalKeyId =
  | "escape" | "tab" | "enter"
  | "up" | "down" | "left" | "right"
  | "backspace" | "ctrlC" | "ctrlD";

export interface TerminalKeystroke {
  id: TerminalKeyId;
  /** Compact visible label for the key button. */
  label: string;
  /** Exact bytes sent via sendInput — canonical VT sequences. */
  bytes: string;
  /** Screen-reader label for the key button. */
  hint: string;
}

/**
 * Canonical escape sequences. Enter is CR (`\r`); arrows are `ESC [` plus the
 * final byte; Backspace is DEL; Ctrl-C / Ctrl-D are ETX / EOT. Text payloads
 * below never gain an extra Enter — only this table's explicit Enter entry
 * carries `\r`.
 */
export const TERMINAL_KEYSTROKES: readonly TerminalKeystroke[] = [
  { id: "escape", label: "Esc", bytes: "\u001b", hint: "Send the Escape key" },
  { id: "tab", label: "Tab", bytes: "\t", hint: "Send the Tab key" },
  { id: "enter", label: "Enter", bytes: "\r", hint: "Send Enter to the running program" },
  { id: "up", label: "\u2191", bytes: "\u001b[A", hint: "Send the Up arrow key" },
  { id: "down", label: "\u2193", bytes: "\u001b[B", hint: "Send the Down arrow key" },
  { id: "left", label: "\u2190", bytes: "\u001b[D", hint: "Send the Left arrow key" },
  { id: "right", label: "\u2192", bytes: "\u001b[C", hint: "Send the Right arrow key" },
  { id: "backspace", label: "\u232b", bytes: "\u007f", hint: "Send the Backspace key" },
  { id: "ctrlC", label: "Ctrl-C", bytes: "\u0003", hint: "Send interrupt (Ctrl-C)" },
  { id: "ctrlD", label: "Ctrl-D", bytes: "\u0004", hint: "Send end of input (Ctrl-D)" },
];

/** Bytes for a key id, or null for an unknown id (nothing is ever sent). */
export function keystrokeBytes(id: TerminalKeyId): string | null {
  return TERMINAL_KEYSTROKES.find((key) => key.id === id)?.bytes ?? null;
}

/** A full command needs visible text; surrounding whitespace alone runs nothing. */
export function isUsableCommand(draft: string): boolean {
  return draft.trim().length > 0;
}

/**
 * Keystroke text (typed or pasted) is sent byte-for-byte: even a lone space
 * or newline is meaningful input to a running program.
 */
export function isUsableKeystrokeText(draft: string): boolean {
  return draft.length > 0;
}

/** Clear, single-sourced byte-limit feedback for an oversized draft. */
export function tooLargeMessage(): string {
  return `That text is too large to send (limit ${MAX_WRITE_BYTES} bytes). Shorten it and try again.`;
}

/**
 * Full commands go to execute_command verbatim. Never append Enter here —
 * submission is the explicit Run action, not a hidden newline. Validation
 * runs through the shared transport boundary.
 */
export function commandToSend(draft: string): string | null {
  if (!isUsableCommand(draft)) return null;
  const prepared = prepareSessionText("command", draft);
  return prepared.ok ? prepared.text : null;
}

/**
 * Buffered text goes to sendInput byte-for-byte: Unicode and line breaks are
 * preserved exactly, no Enter is appended, and pastes never auto-submit.
 */
export function keystrokesToSend(draft: string): string | null {
  if (!isUsableKeystrokeText(draft)) return null;
  const prepared = prepareSessionText("input", draft);
  return prepared.ok ? prepared.text : null;
}

/**
 * Agent questions use the shared boundary's trimming, so a whitespace-only
 * draft never reaches the host. Returns the trimmed question.
 */
export function queryToSend(draft: string): string | null {
  const prepared = prepareAgentDraft(draft);
  return prepared.ok ? prepared.text : null;
}

/** Agent usability, payload and feedback share the gateway's Go-trim boundary. */
export function prepareAgentDraft(draft: unknown) {
  const prepared = prepareSessionText("agentQuery", draft);
  if (prepared.ok) return prepared;
  return {
    ...prepared,
    message: prepared.error === "too-large" ? tooLargeMessage()
      : prepared.error === "empty" ? "Type a question to ask first."
      : "Enter a valid question to ask the agent.",
  };
}

export type DraftField = "command" | "keystrokes" | "query";

export interface ComposerDrafts {
  sessionId: string;
  command: string;
  keystrokes: string;
  query: string;
}

export type DraftAction =
  | { type: "edit"; field: DraftField; value: string }
  | { type: "session"; sessionId: string }
  | { type: "sent"; field: DraftField }
  | { type: "cancel"; field: DraftField };

export function initialDrafts(sessionId: string): ComposerDrafts {
  return { sessionId, command: "", keystrokes: "", query: "" };
}

/**
 * The live draft transitions used by the composer. Edits update one field;
 * a successful send (`sent`) or an explicit user cancel (`cancel`) clears
 * only that field. A failed send dispatches nothing, so the draft survives;
 * switching sessions clears everything so text never leaks rooms. Role
 * flaps dispatch nothing, so drafts survive transient disconnects and
 * downgrades while the composer stays mounted.
 */
export function draftReducer(state: ComposerDrafts, action: DraftAction): ComposerDrafts {
  switch (action.type) {
    case "edit":
      return state[action.field] === action.value ? state : { ...state, [action.field]: action.value };
    case "sent":
    case "cancel":
      return state[action.field] === "" ? state : { ...state, [action.field]: "" };
    case "session":
      return action.sessionId === state.sessionId ? state : initialDrafts(action.sessionId);
  }
}

/** Bounded per-session command history for recall (oldest → newest). */
export const MAX_COMMAND_HISTORY = 100;

export type HistoryDirection = "older" | "newer";

export interface HistoryRecall {
  /** Null means browsing exited and the stashed draft is restored. */
  index: number | null;
  value: string;
}

/**
 * Append a successfully sent command to history. Empty and oversized input
 * is rejected (returns the input unchanged) so history only holds text the
 * transport actually accepted. Consecutive duplicates are collapsed and only
 * the newest `limit` entries are kept.
 */
export function pushCommandHistory(
  history: readonly string[],
  command: string,
  limit: number = MAX_COMMAND_HISTORY,
): string[] {
  if (!isUsableCommand(command)) return history as string[];
  if (commandToSend(command) === null) return history as string[];
  const bounded = Number.isSafeInteger(limit) && limit > 0 ? limit : MAX_COMMAND_HISTORY;
  if (history.length > 0 && history[history.length - 1] === command) return history as string[];
  if (history.length >= bounded) return [...history.slice(history.length - bounded + 1), command];
  return [...history, command];
}

/** Normalize a recall cursor: unknown, fractional, or out-of-range means idle. */
function normalizeHistoryIndex(history: readonly string[], index: number | null): number | null {
  if (index === null) return null;
  if (!Number.isInteger(index) || index < 0 || index >= history.length) return null;
  return index;
}

/**
 * Step through command history without ever discarding the caller's draft.
 * `draft` is the stashed text to restore when stepping past the newest
 * entry. Empty history always stays idle. Older clamps at the oldest entry;
 * newer past the newest entry exits browsing and restores `draft`.
 */
export function stepCommandHistory(
  history: readonly string[],
  currentIndex: number | null,
  direction: HistoryDirection,
  draft: string,
): HistoryRecall {
  if (history.length === 0) return { index: null, value: draft };
  const current = normalizeHistoryIndex(history, currentIndex);
  // A stale cursor (history reset while browsing) restarts from the draft.
  const effective = currentIndex !== null && current === null ? null : current;
  if (direction === "older") {
    if (effective === null) {
      const index = history.length - 1;
      return { index, value: history[index]! };
    }
    if (effective <= 0) return { index: 0, value: history[0]! };
    const index = effective - 1;
    return { index, value: history[index]! };
  }
  if (effective === null) return { index: null, value: draft };
  if (effective >= history.length - 1) return { index: null, value: draft };
  const index = effective + 1;
  return { index, value: history[index]! };
}

/** Whether an older entry exists to recall (history non-empty, not at oldest). */
export function canRecallOlder(history: readonly string[], currentIndex: number | null): boolean {
  if (history.length === 0) return false;
  const current = normalizeHistoryIndex(history, currentIndex);
  if (currentIndex !== null && current === null) return true;
  return current === null || current > 0;
}

/** Whether browsing can step newer (always true while browsing history). */
export function canRecallNewer(history: readonly string[], currentIndex: number | null): boolean {
  if (history.length === 0) return false;
  return normalizeHistoryIndex(history, currentIndex) !== null;
}

export interface SendGate {
  status: string;
  role: string | null;
}

/** Writes are gated by a live controller connection, mirroring the transport. */
export function canSendToSession(gate: SendGate): boolean {
  return gate.status === "connected" && gate.role === "controller";
}

/** Human reason a send is unavailable, or null when writing is allowed. */
export function sendBlockerReason(gate: SendGate): string | null {
  if (gate.status !== "connected") return "Connect the session before sending anything.";
  if (gate.role !== "controller") return "View-only access. Request Edit to send input.";
  return null;
}

/** Structural writers the composer needs; live tab engines satisfy it. */
export interface SessionWriters {
  sendInput(text: string): boolean;
  executeCommand(command: string): boolean;
  sendAgentQuery(query: string): boolean;
  stopAgent(): boolean;
}
