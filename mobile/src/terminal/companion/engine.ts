/**
 * Pure per-tab companion terminal engine.
 *
 * No React, no sockets, and no timers beyond the injected gap inside a draft
 * submit: the hook drives `attach` once and then
 * `pollOnce` sequentially every 200ms while foreground + socket ready +
 * machine online + lease active. The engine owns exact byte-offset continuity,
 * gap → snapshot fallback, base64 caps (via the strict RPC readers), stale
 * owner masking and detach-without-kill. It feeds the existing
 * `TerminalRuntimeStore` through `TerminalDelta` only — never a new parser,
 * never partial VT after a grid.
 *
 * Streaming: after attach the engine subscribes to `terminal.<handle>` and
 * stops polling. A primary screen subscribes from its raw offset (0 on a fresh
 * attach, which replays the desktop's ring); an alternate screen subscribes
 * without one, so its first frame is a VT seed that redraws the screen in a
 * fresh runtime epoch — no snapshot polling. Raw frames obey the same
 * continuity rule as `tabs.read`; a base that does not match means asking for
 * a new seed, never concatenating. A stream that fails after working is
 * resubscribed from the current offset (bounded, with backoff), then
 * reattached. A subscribe that fails before any frame ever arrived means the
 * relay or desktop cannot stream: the engine falls back to today's polling.
 *
 * Long sessions: the runtime store keeps a rolling raw window. When a
 * renderer needs bytes it dropped, the store says so and the engine asks for
 * a seed (streaming) or switches to screen snapshots (polling fallback only).
 *
 * Screen modes: raw frames carry none. The renderer reads mouse/alternate
 * screen from its own VT parser fed by the seed and every byte after, and Send
 * fetches fresh modes before each draft, so the engine refreshes its copy from
 * `tabs.screen` only after a seed and when output resumes after a quiet spell
 * — one request at a time, off the stream's path, never per frame.
 *
 * Host generation: a desktop JWT refresh replaces the host generation and
 * invalidates old handles while the phone socket may stay ready. Non-login,
 * non-malformed read/screen failures therefore trigger a bounded reattach
 * (3 attempts) with the 200ms poll cadence as backoff — never a permanent
 * dead view requiring logout. Reattach is metadata-only (attach), resets the
 * WASM epoch on the new handle, cleans the old handle up best-effort, and
 * never replays input/resize as if acknowledged.
 */
import type { TerminalDelta } from "../../transport/protocol";
import { RELAY_RPC_UNREADABLE_MESSAGE } from "../../transport/relayRpcMethods";
import type {
  CompanionTerminalStream,
  CompanionTerminalStreamListener,
  RelayRpcTabsAttachResult,
  RelayRpcTabsReadResult,
  RelayRpcTabsScreenResult,
  RelayRpcTerminalStreamPayload,
} from "../../transport/relayRpcMethods";
import { COMPANION_TERMINAL_LIMITS, DESKTOP_IN_CONTROL_CODE } from "../../transport/relayRpcMethods";
import { utf8ByteLength } from "../../transport/frames";
import { SEED_FAILED_CODE, REPLY_FAILED_CODE, SEED_FAILED_MESSAGE, CHANNEL_LOST_CODE } from "../../transport/e2ee/desktopLink";
import { RELAY_RPC_CONNECTION_LOST_MESSAGE, RELAY_RPC_TIMEOUT_MESSAGE } from "../../transport/relayRpcClient";
import {
  COMPANION_TERMINAL_MESSAGES,
  type CompanionTerminalEngineStatus,
  type CompanionTerminalMode,
  type CompanionTerminalOwner,
  type CompanionTerminalPhase,
} from "./types";
import type { CompanionHandleRegistry } from "./registry";
import { SUBMIT_CR_DELAY_MS, SUBMIT_ENTER, draftRefusal, encodeDraft } from "./submitDraft";

export const COMPANION_TERMINAL_MAX_REATTACH_ATTEMPTS = 3;

/** Stream recovery: resubscribes after a working stream fails, before reattaching. */
export const COMPANION_STREAM_MAX_RESUBSCRIBES = 3;
export const COMPANION_STREAM_BACKOFF_MS = 250;
/** No frame at all this long after the first subscribe means streaming is not available. */
export const COMPANION_STREAM_FIRST_FRAME_TIMEOUT_MS = 8000;
/** Output after this much silence refreshes screen modes once, after a short settle. */
export const COMPANION_MODES_QUIET_MS = 1500;
export const COMPANION_MODES_SETTLE_MS = 300;

export interface CompanionTimers {
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(id: unknown): void;
}

const defaultTimers: CompanionTimers = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>),
};

type InputResult = { ok: true } | { ok: false; message: string };

/** A resize outcome; `desktopInControl` marks the desktop's refusal. */
export type CompanionResizeResult =
  | { ok: true }
  | { ok: false; message: string; desktopInControl?: true };

export interface CompanionTerminalRpc {
  attach(tabId: number): Promise<RelayRpcTabsAttachResult>;
  read(handle: string, offset: number): Promise<RelayRpcTabsReadResult>;
  screen(handle: string): Promise<RelayRpcTabsScreenResult>;
  input(handle: string, text: string): Promise<{ readonly sent: true }>;
  resize(handle: string, rows: number, cols: number): Promise<{ readonly resized: true }>;
  detach(handle: string): Promise<{ readonly detached: true }>;
  /** Push stream of the handle's output. Absent means polling only. */
  subscribe?(handle: string, offset: number | undefined, listener: CompanionTerminalStreamListener): CompanionTerminalStream;
}

interface StreamRecord {
  readonly token: number;
  readonly handle: string;
  /** Undefined asked for a seed first. */
  readonly offset: number | undefined;
  sub: CompanionTerminalStream | null;
  frames: number;
}

export interface CompanionScreenModes {
  readonly alternateScreen: boolean;
  readonly mouseReporting: boolean;
  readonly sgrMouse: boolean;
  readonly bracketedPaste: boolean;
}

export interface CompanionTerminalSink {
  setViewport?(rows: number, cols: number): void;
  setScreenModes?(modes: CompanionScreenModes): void;
  receive(delta: TerminalDelta): void;
  /** Reset the WASM epoch for a new handle (store.reset). Best-effort. */
  reset(): void;
  /** A history part failed (seed_failed/reply_failed): count manual retries. */
  notePartFailure?(): void;
}

/**
 * A part-payload failure (docs/design/e2ee.md §4.5): hash mismatch, cap
 * overrun or abort while fetching history. Never an automatic reconnect
 * loop — the first failure is a manual-Retry error, the second keeps the
 * live stream running without history. Duck-typed like the other error
 * helpers here: the engine never imports the error class.
 */
export interface PartFailure {
  readonly code: string;
  readonly message: string;
}

export function partFailureOf(error: unknown): PartFailure | null {
  if (typeof error !== "object" || error === null) return null;
  const record = error as { code?: unknown; message?: unknown };
  if (record.code !== SEED_FAILED_CODE && record.code !== REPLY_FAILED_CODE) return null;
  return {
    code: record.code,
    message: typeof record.message === "string" ? record.message : "",
  };
}

function loginRequiredOf(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "loginRequired" in error &&
    (error as { loginRequired?: unknown }).loginRequired === true
  );
}

function isUnreadableOf(error: unknown): boolean {
  // Malformed payloads fail closed via the strict reader. Compare against the
  // single exported constant — never parse human message text or R0 internals.
  return (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    (error as { message?: unknown }).message === RELAY_RPC_UNREADABLE_MESSAGE
  );
}

function decodedLength(data: string): number {
  if (data === "") return 0;
  const padding = data.endsWith("==") ? 2 : data.endsWith("=") ? 1 : 0;
  return (data.length / 4) * 3 - padding;
}

/** Client-side input gate: non-empty, max 16KiB UTF-8, no fabrication. */
export function isUsableCompanionInput(text: unknown): boolean {
  if (typeof text !== "string" || text.length === 0) return false;
  return utf8ByteLength(text) <= COMPANION_TERMINAL_LIMITS.inputUtf8Bytes;
}

/** Client-side resize gate: rows/cols/cells bounds from the contract. */
export function isUsableCompanionResize(rows: unknown, cols: unknown): boolean {
  if (typeof rows !== "number" || typeof cols !== "number") return false;
  if (!Number.isSafeInteger(rows) || !Number.isSafeInteger(cols)) return false;
  if (rows < 1 || cols < 1 || rows > 240 || cols > 512 || rows * cols > 65_536) return false;
  return true;
}

/**
 * Pure read continuity: base must equal the requested offset exactly.
 * `gap` means the ring evicted bytes we never saw — the caller must switch
 * to snapshot mode and never concatenate the missing VT. Offset advances by
 * base + decoded length only, never by total.
 */
export type CompanionReadDecision =
  | { readonly kind: "empty" }
  | { readonly kind: "data"; readonly nextOffset: number; readonly decoded: number }
  | { readonly kind: "gap"; readonly base: number; readonly expected: number };

export function decideCompanionRead(expectedOffset: number, result: RelayRpcTabsReadResult): CompanionReadDecision {
  if (result.base !== expectedOffset) {
    return Object.freeze({ kind: "gap" as const, base: result.base, expected: expectedOffset });
  }
  const decoded = decodedLength(result.data);
  if (decoded === 0) return Object.freeze({ kind: "empty" as const });
  return Object.freeze({ kind: "data" as const, nextOffset: result.base + decoded, decoded });
}

/** Pure screen mapping: complete grids feed alt_screen; incomplete is resync. */
export type CompanionScreenDecision =
  | { readonly kind: "grid"; readonly delta: TerminalDelta }
  | { readonly kind: "resync" };

export function decideCompanionScreen(result: RelayRpcTabsScreenResult): CompanionScreenDecision {
  if (!result.complete || !result.delta) return Object.freeze({ kind: "resync" as const });
  const delta = result.delta;
  return Object.freeze({
    kind: "grid" as const,
    delta: Object.freeze({
      kind: "alt_screen",
      rows: delta.rows,
      cols: delta.cols,
      cursor_row: delta.cursor_row,
      cursor_col: delta.cursor_col,
      cursor_visible: delta.cursor_visible,
      lines: delta.lines,
    }) as TerminalDelta,
  });
}

export function initialCompanionStatus(): CompanionTerminalEngineStatus {
  return Object.freeze({
    phase: "attaching" as const,
    message: COMPANION_TERMINAL_MESSAGES.attaching,
    mode: "raw" as const,
    handle: null,
    offset: 0,
    alive: true,
    exitCode: null,
    canWrite: false,
    canResize: false,
    takenByDesktop: false,
  });
}

/**
 * Whether a failed attach is worth trying again, and what to show meanwhile;
 * `null`: it isn't. Right after the phone reconnects (a Wi-Fi switch, the app
 * back in front) the attach can run before the end-to-end channel has
 * reopened, or before the desktop has re-registered its tabs. That failure
 * used to stick: "This live tab is unavailable on that machine" until the
 * network changed again.
 */
export function attachRetryOf(error: unknown, attempt: number): string | null {
  const e = error as { code?: unknown; message?: unknown } | null;
  if (e?.code === WAKING_CODE) return attempt < WAKE_ATTEMPTS ? COMPANION_TERMINAL_MESSAGES.waking : null;
  const transient =
    e?.code === CHANNEL_LOST_CODE ||
    e?.message === RELAY_RPC_CONNECTION_LOST_MESSAGE ||
    e?.message === RELAY_RPC_TIMEOUT_MESSAGE;
  if (transient) return attempt < RECONNECT_ATTEMPTS ? COMPANION_TERMINAL_MESSAGES.reconnecting : null;
  // The desktop has no terminal for it yet: a few tries, then believe it.
  if (e?.code === "unavailable") return attempt < UNAVAILABLE_ATTEMPTS ? COMPANION_TERMINAL_MESSAGES.reconnecting : null;
  return null;
}
/** Tries while the connection comes back: about 30 s. */
const RECONNECT_ATTEMPTS = 15;
/** Tries for a tab the desktop doesn't route yet: about 6 s. */
const UNAVAILABLE_ATTEMPTS = 4;

/** The desktop's code while it wakes a sleeping agent's tab. */
const WAKING_CODE = "waking";
/** Retries while waking: about 40 s in all, for a slow agent start. */
const WAKE_ATTEMPTS = 20;
const WAKE_RETRY_MS = 2000;

function desktopTookOf(error: unknown): boolean {
  return (error as { code?: unknown } | null)?.code === DESKTOP_IN_CONTROL_CODE;
}

function statusFor(
  base: CompanionTerminalEngineStatus,
  patch: Partial<CompanionTerminalEngineStatus>,
): CompanionTerminalEngineStatus {
  return Object.freeze({ ...base, ...patch });
}

/**
 * One engine per selected tab. The hook recreates it whenever the owner
 * (account/device/tab/client/epoch) changes; the old engine is disposed so
 * its late callbacks detach without feeding the new store.
 */
export class CompanionTerminalEngine {
  private disposed = false;
  private attachGeneration = 0;
  private status: CompanionTerminalEngineStatus = initialCompanionStatus();
  private listeners = new Set<(status: CompanionTerminalEngineStatus) => void>();
  private rawSeq = 0;
  private rawBytesTotal = 0;
  private rawChunkCount = 0;
  private reattachAttempts = 0;
  private reattaching = false;
  // Streaming state. `streamEngaged` holds polling off from the first
  // subscribe until the engine falls back to polling or the handle ends,
  // including the backoff between resubscribes.
  private streamEngaged = false;
  private streamSupport: "unknown" | "works" | "unsupported" = "unknown";
  private stream: StreamRecord | null = null;
  private streamToken = 0;
  private streamResubscribes = 0;
  private streamTimer: unknown = null;
  private firstFrameTimer: unknown = null;
  private reseedWanted = false;
  private altAttach = false;
  private lastOutputAt = -Infinity;
  private modesTimer: unknown = null;
  private modesInFlight = false;
  /** Consecutive history-part failures across manual retries (hook-owned). */
  private partFailures = 0;
  /** Set after the second part failure: live without history, message kept. */
  private historyMessage: string | null = null;
  private screenModes: CompanionScreenModes = Object.freeze({
    alternateScreen: false,
    mouseReporting: false,
    sgrMouse: false,
    bracketedPaste: false,
  });

  constructor(
    private readonly owner: CompanionTerminalOwner,
    private readonly rpc: CompanionTerminalRpc,
    private readonly sink: CompanionTerminalSink,
    private readonly registry: CompanionHandleRegistry,
    private readonly delay: (ms: number) => Promise<void> = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    private readonly timers: CompanionTimers = defaultTimers,
    priorPartFailures = 0,
  ) {
    this.partFailures = priorPartFailures;
  }

  getOwner(): CompanionTerminalOwner {
    return this.owner;
  }

  getStatus(): CompanionTerminalEngineStatus {
    return this.status;
  }

  getScreenModes(): CompanionScreenModes {
    return this.screenModes;
  }

  private modesFrom(
    result: Pick<
      RelayRpcTabsAttachResult & RelayRpcTabsScreenResult,
      "alternateScreen" | "mouseReporting" | "sgrMouse" | "bracketedPaste"
    >,
  ): CompanionScreenModes {
    // Absent means false: old hosts omit modes the phone never knew.
    return Object.freeze({
      alternateScreen: result.alternateScreen ?? false,
      mouseReporting: result.mouseReporting ?? false,
      sgrMouse: result.sgrMouse ?? false,
      bracketedPaste: result.bracketedPaste ?? false,
    });
  }

  private publishModes(modes: CompanionScreenModes): void {
    this.screenModes = modes;
    try {
      this.sink.setScreenModes?.(modes);
    } catch {
      /* Mode forwarding is best-effort; polling still advances. */
    }
  }

  /**
   * One-shot mode refresh without disturbing raw offsets or snapshot grids.
   * Returns fresh modes; on failure it rejects and leaves old values alone.
   */
  async refreshScreenModes(): Promise<CompanionScreenModes> {
    const handle = this.status.handle;
    if (this.disposed || handle === null) {
      return Promise.reject(new Error("not attached"));
    }
    const currentHandle = handle;
    const result = await this.rpc.screen(currentHandle);
    if (this.disposed || this.status.handle !== currentHandle) {
      throw new Error("stale handle");
    }
    const modes = this.modesFrom(result);
    this.publishModes(modes);
    return modes;
  }

  /** For tests: how much raw history this handle has accumulated. */
  getRawUsage(): { readonly bytes: number; readonly chunks: number; readonly reattachAttempts: number } {
    return Object.freeze({ bytes: this.rawBytesTotal, chunks: this.rawChunkCount, reattachAttempts: this.reattachAttempts });
  }

  subscribe(listener: (status: CompanionTerminalEngineStatus) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** Stale results (old account/device/tab/client/epoch) must be masked. */
  isCurrent(candidate: CompanionTerminalOwner): boolean {
    if (this.disposed) return false;
    return (
      candidate.accountId === this.owner.accountId &&
      candidate.deviceId === this.owner.deviceId &&
      candidate.tabId === this.owner.tabId &&
      candidate.client === this.owner.client &&
      candidate.epoch === this.owner.epoch
    );
  }

  private publish(next: CompanionTerminalEngineStatus): void {
    this.status = next;
    for (const listener of Array.from(this.listeners)) {
      try {
        listener(next);
      } catch {
        /* Diagnostics never break polling. */
      }
    }
  }

  private attachedReady(): boolean {
    return (
      !this.disposed &&
      this.status.handle !== null &&
      (this.status.phase === "live-raw" || this.status.phase === "live-snapshot") &&
      this.status.alive
    );
  }

  private resetSinkEpoch(): void {
    try {
      this.sink.reset();
    } catch {
      /* Reset is best-effort; the next attach still starts from offset 0. */
    }
  }

  /**
   * `tabs.attach`, waiting while the desktop wakes the tab's agent: an idle
   * agent is put to sleep to save memory and has no terminal until opening
   * its tab starts it again (a few seconds).
   */
  private async attachWakingSleeper(generation: number): Promise<RelayRpcTabsAttachResult> {
    for (let attempt = 1; ; attempt += 1) {
      try {
        return await this.rpc.attach(this.owner.tabId);
      } catch (error) {
        const retry = attachRetryOf(error, attempt);
        if (retry === null || this.disposed || this.attachGeneration !== generation) throw error;
        this.publish(statusFor(this.status, { message: retry }));
        await this.delay(WAKE_RETRY_MS);
        if (this.disposed || this.attachGeneration !== generation) throw error;
      }
    }
  }

  /** Attach once per engine. Late success after dispose detaches immediately. */
  async attach(): Promise<void> {
    if (this.disposed) return;
    const generation = ++this.attachGeneration;
    this.publish(
      statusFor(this.status, {
        phase: "attaching",
        message: COMPANION_TERMINAL_MESSAGES.attaching,
        handle: null,
        offset: 0,
        mode: "raw",
        alive: true,
        exitCode: null,
        canWrite: false,
        canResize: false,
      }),
    );
    // Local bound first: never steal another tab's handle.
    if (this.registry.size() >= 4) {
      if (!this.disposed && this.attachGeneration === generation) {
        this.publish(
          statusFor(this.status, { phase: "rejected", message: COMPANION_TERMINAL_MESSAGES.rejected }),
        );
      }
      return;
    }
    let result: RelayRpcTabsAttachResult;
    try {
      result = await this.attachWakingSleeper(generation);
    } catch (error) {
      // Development builds only: the reason behind "unavailable" (no content).
      if ((globalThis as { __DEV__?: boolean }).__DEV__ === true) {
        const e = error as { message?: unknown; code?: unknown; loginRequired?: unknown; name?: unknown };
        console.warn("[terminal] attach failed", { tab: this.owner.tabId, name: e?.name, code: e?.code, message: e?.message, loginRequired: e?.loginRequired });
      }
      if (this.disposed || this.attachGeneration !== generation) return;
      if (desktopTookOf(error)) {
        this.takenByDesktop();
        return;
      }
      if (loginRequiredOf(error)) {
        this.publish(statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.signIn }));
      } else {
        this.publish(
          statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.unavailable }),
        );
      }
      return;
    }
    if (this.disposed || this.attachGeneration !== generation) {
      // Late attach after unmount/tab switch: release transport at once.
      // Never kills the host PTY — detach only.
      try {
        await this.rpc.detach(result.handle);
      } catch {
        /* Best effort cleanup. */
      }
      return;
    }
    if (!this.registry.tryAdd(result.handle)) {
      try {
        await this.rpc.detach(result.handle);
      } catch {
        /* Best effort cleanup. */
      }
      if (!this.disposed && this.attachGeneration === generation) {
        this.publish(
          statusFor(this.status, { phase: "rejected", message: COMPANION_TERMINAL_MESSAGES.rejected }),
        );
      }
      return;
    }
    this.reattachAttempts = 0;
    await this.beginLive(result, generation);
  }

  /**
   * A handle is ready (fresh attach or reattach): new runtime epoch, host
   * grid and modes, then the stream — or, when streaming is unavailable,
   * today's polling (alternate screen as snapshots, shown at once).
   */
  private async beginLive(result: RelayRpcTabsAttachResult, generation: number): Promise<void> {
    this.stopStream();
    this.rawSeq = 0;
    this.rawBytesTotal = 0;
    this.rawChunkCount = 0;
    this.reattaching = false;
    this.streamEngaged = false;
    this.streamResubscribes = 0;
    this.reseedWanted = false;
    // A fresh attach makes a fresh seed attempt, so a previous history loss
    // no longer describes this epoch. A failing seed re-sets it (failPart).
    this.historyMessage = null;
    this.resetSinkEpoch();
    this.sink.setViewport?.(result.rows, result.cols);
    this.publishModes(this.modesFrom(result));
    const alternate = result.alternateScreen ?? false;
    this.altAttach = alternate;
    const streaming = this.rpc.subscribe !== undefined && this.streamSupport !== "unsupported";
    const snapshotAttach = alternate && !streaming;
    this.publish(
      statusFor(this.status, {
        phase: snapshotAttach ? "live-snapshot" : "live-raw",
        message: null,
        mode: snapshotAttach ? "snapshot" : "raw",
        handle: result.handle,
        offset: 0,
        alive: true,
        exitCode: null,
        canWrite: true,
        canResize: true,
      }),
    );
    if (streaming) {
      // An alternate screen starts from a seed; a primary one replays the
      // desktop's ring from 0, as polling did.
      this.streamEngaged = true;
      this.startStream(result.handle, alternate ? undefined : 0);
      return;
    }
    // Alt-screen attach shows the current screen at once instead of waiting
    // for the next 200ms poll.
    if (snapshotAttach && !this.disposed && this.attachGeneration === generation) {
      await this.fetchSnapshotNow(result.handle, generation);
    }
  }

  // ---- Streaming ---------------------------------------------------------

  private startStream(handle: string, offset: number | undefined): void {
    this.clearStreamTimers();
    const subscribe = this.rpc.subscribe;
    if (this.disposed || subscribe === undefined) return;
    const record: StreamRecord = { token: ++this.streamToken, handle, offset, sub: null, frames: 0 };
    const token = record.token;
    this.stream = record;
    let sub: CompanionTerminalStream;
    try {
      sub = subscribe.call(this.rpc, handle, offset, {
        onFrame: (payload) => this.onStreamFrame(token, payload),
        onError: (error) => this.onStreamError(token, error),
      });
    } catch (error) {
      this.onStreamError(token, error);
      return;
    }
    if (this.stream !== record) {
      // It already failed or was replaced while subscribing.
      try { sub.close(); } catch { /* Best effort. */ }
      return;
    }
    record.sub = sub;
    if (this.streamSupport === "unknown" && record.frames === 0) {
      this.firstFrameTimer = this.timers.setTimeout(() => {
        this.firstFrameTimer = null;
        if (this.disposed || this.stream !== record || record.frames > 0 || this.streamSupport !== "unknown") return;
        this.fallBackToPolling();
      }, COMPANION_STREAM_FIRST_FRAME_TIMEOUT_MS);
    }
  }

  private clearStreamTimers(): void {
    if (this.streamTimer !== null) this.timers.clearTimeout(this.streamTimer);
    if (this.firstFrameTimer !== null) this.timers.clearTimeout(this.firstFrameTimer);
    this.streamTimer = null;
    this.firstFrameTimer = null;
  }

  /** Unsubscribe and forget; no frame of it reaches the store afterwards. */
  private stopStream(): void {
    const record = this.stream;
    this.stream = null;
    this.clearStreamTimers();
    try {
      record?.sub?.close();
    } catch {
      /* Unsubscribe is best effort. */
    }
  }

  /** For tests and diagnostics. */
  getStreamState(): { readonly engaged: boolean; readonly support: "unknown" | "works" | "unsupported"; readonly offset: number | undefined | null } {
    return Object.freeze({ engaged: this.streamEngaged, support: this.streamSupport, offset: this.stream ? this.stream.offset : null });
  }

  /** Streaming never worked here (old relay or desktop): poll exactly as before. */
  private fallBackToPolling(): void {
    this.streamSupport = "unsupported";
    this.streamEngaged = false;
    this.stopStream();
    const handle = this.status.handle;
    if (this.disposed || handle === null || !this.attachedReady()) return;
    if (this.altAttach && this.rawSeq === 0) {
      this.publish(statusFor(this.status, { phase: "live-snapshot", mode: "snapshot" as CompanionTerminalMode, message: null }));
      void this.fetchSnapshotNow(handle, this.attachGeneration);
    }
    // A primary screen keeps raw mode; the next poll reads from the offset.
  }

  private onStreamFrame(token: number, frame: RelayRpcTerminalStreamPayload): void {
    const record = this.stream;
    if (this.disposed || record === null || record.token !== token || this.status.handle !== record.handle) return;
    if (record.frames === 0 && this.firstFrameTimer !== null) {
      this.timers.clearTimeout(this.firstFrameTimer);
      this.firstFrameTimer = null;
    }
    record.frames += 1;
    this.streamSupport = "works";
    this.streamResubscribes = 0;
    let offset = this.status.offset;
    if (frame.reset) {
      this.applySeed(frame);
      offset = frame.base;
    } else {
      const decision = decideCompanionRead(this.status.offset, frame);
      if (decision.kind === "gap") {
        // Bytes are missing between what we hold and this frame: never
        // concatenate. Ask for a seed; an exited terminal just closes.
        if (frame.alive) {
          this.reseed(record.handle);
          return;
        }
      } else if (decision.kind === "data") {
        this.feedRaw(frame.data, decision.decoded);
        offset = decision.nextOffset;
        this.noteOutput();
      }
    }
    if (this.disposed || this.stream !== record) return;
    if (!frame.alive) {
      this.stopStream();
      this.streamEngaged = false;
      this.publish(
        statusFor(this.status, {
          phase: "closed",
          message:
            frame.exitCode !== null
              ? `${COMPANION_TERMINAL_MESSAGES.closed} (exit ${frame.exitCode})`
              : COMPANION_TERMINAL_MESSAGES.closed,
          alive: false,
          exitCode: frame.exitCode,
          offset,
          canWrite: false,
          canResize: false,
        }),
      );
      return;
    }
    this.advance(offset);
    if (this.reseedWanted && !frame.reset) this.reseed(record.handle);
  }

  private applySeed(frame: RelayRpcTerminalStreamPayload): void {
    // A seed redraws a BLANK terminal: fresh parser (epoch), its grid, then it.
    this.rawSeq = 0;
    this.rawBytesTotal = 0;
    this.rawChunkCount = 0;
    this.reseedWanted = false;
    this.resetSinkEpoch();
    if (frame.rows !== null && frame.cols !== null) this.sink.setViewport?.(frame.rows, frame.cols);
    // The store's reset forgot its modes; keep the last known ones until the refresh.
    this.publishModes(this.screenModes);
    if (frame.decoded > 0) this.feedRaw(frame.data, frame.decoded);
    this.lastOutputAt = this.timers.now();
    this.scheduleModesRefresh();
  }

  private feedRaw(data: string, decoded: number): void {
    this.rawSeq += 1;
    this.rawBytesTotal += decoded;
    this.rawChunkCount += 1;
    try {
      this.sink.receive(Object.freeze({ kind: "output_chunk", seq: this.rawSeq, data_base64: data }) as TerminalDelta);
    } catch {
      // A hostile store must not break offset continuity accounting.
    }
  }

  /**
   * The offset moves on every frame, but listeners only hear about changes
   * they show (phase, message, permissions): no re-render per frame.
   */
  private advance(offset: number): void {
    const current = this.status;
    if (current.phase === "live-raw" && current.mode === "raw" && current.message === this.historyMessage && current.canWrite && current.canResize && current.alive) {
      this.status = statusFor(current, { offset });
      return;
    }
    this.publish(
      statusFor(current, { phase: "live-raw", mode: "raw", message: this.historyMessage, offset, alive: true, canWrite: true, canResize: true }),
    );
  }

  private reseed(handle: string): void {
    this.stopStream();
    this.startStream(handle, undefined);
  }

  private onStreamError(token: number, error: unknown): void {
    const record = this.stream;
    if (this.disposed || record === null || record.token !== token) return;
    this.stream = null;
    this.clearStreamTimers();
    if (desktopTookOf(error)) {
      this.takenByDesktop();
      return;
    }
    if (loginRequiredOf(error)) {
      this.streamEngaged = false;
      this.publish(statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.signIn, canWrite: false, canResize: false }));
      return;
    }
    // §4.5: a failed history part is a manual-Retry error (then live
    // without history), never a resubscribe or reattach loop.
    const part = partFailureOf(error);
    if (part !== null) {
      this.failPart(part);
      return;
    }
    if (this.streamSupport !== "works") {
      // Failed before any frame ever arrived: an old relay or desktop.
      this.fallBackToPolling();
      return;
    }
    const handle = record.handle;
    if (this.streamResubscribes < COMPANION_STREAM_MAX_RESUBSCRIBES) {
      this.streamResubscribes += 1;
      const wait = COMPANION_STREAM_BACKOFF_MS * 2 ** (this.streamResubscribes - 1);
      this.streamTimer = this.timers.setTimeout(() => {
        this.streamTimer = null;
        if (this.disposed || this.stream !== null || !this.streamEngaged || this.status.handle !== handle || !this.attachedReady()) return;
        this.startStream(handle, this.reseedWanted ? undefined : this.status.offset);
      }, wait);
      return;
    }
    // Still failing: the handle itself is likely stale. Polling stays off.
    void this.reattachBounded(handle);
  }

  /**
   * The renderer needs bytes the runtime window no longer holds (or its raw
   * stream went out of sync). Streaming: ask for a seed. Polling fallback:
   * screen snapshots, as the old cap did.
   */
  requestReseed(): void {
    if (this.disposed || !this.attachedReady()) return;
    const handle = this.status.handle as string;
    if (this.streamEngaged) {
      const record = this.stream;
      if (record === null) {
        this.reseedWanted = true;
        return;
      }
      if (record.frames === 0) {
        // A seed is already on its way, or will be asked for after the first frame.
        if (record.offset !== undefined) this.reseedWanted = true;
        return;
      }
      this.reseed(handle);
      return;
    }
    if (this.status.mode === "raw") this.switchToSnapshotForCap();
  }

  /** Output after a quiet spell (a program starting) refreshes modes once. */
  private noteOutput(): void {
    const now = this.timers.now();
    const quiet = now - this.lastOutputAt >= COMPANION_MODES_QUIET_MS;
    this.lastOutputAt = now;
    if (quiet) this.scheduleModesRefresh();
  }

  private scheduleModesRefresh(): void {
    if (this.disposed || this.modesTimer !== null) return;
    this.modesTimer = this.timers.setTimeout(() => {
      this.modesTimer = null;
      if (this.disposed || this.modesInFlight || !this.attachedReady()) return;
      this.modesInFlight = true;
      this.refreshScreenModes()
        .catch(() => undefined)
        .finally(() => {
          this.modesInFlight = false;
        });
    }, COMPANION_MODES_SETTLE_MS);
  }

  private async fetchSnapshotNow(handle: string, generation: number): Promise<void> {
    let screen: RelayRpcTabsScreenResult;
    try {
      screen = await this.rpc.screen(handle);
    } catch {
      // Next poll retries; attach itself already succeeded.
      return;
    }
    if (this.disposed || this.attachGeneration !== generation || this.status.handle !== handle) return;
    if (this.status.mode !== "snapshot") return;
    this.publishModes(this.modesFrom(screen));
    const decision = decideCompanionScreen(screen);
    if (decision.kind !== "grid") return;
    try {
      this.sink.receive(decision.delta);
    } catch {
      /* Store faults stay local. */
    }
  }

  /**
   * One sequential poll step. The hook calls this at most every 200ms and
   * only while foreground + socket ready + machine online + lease active.
   * Raw mode reads exact offsets; any gap switches permanently to snapshot
   * mode for this handle lifetime and never mixes partial VT after a grid.
   * Only used when streaming is unavailable.
   */
  async pollOnce(): Promise<void> {
    // A stream (or its recovery) is live: polling issues no request at all.
    if (this.disposed || this.streamEngaged || this.status.handle === null) return;
    if (this.status.phase !== "live-raw" && this.status.phase !== "live-snapshot") return;
    if (!this.status.alive) return;
    const handle = this.status.handle;
    if (this.status.mode === "raw") {
      await this.pollRaw(handle);
    } else {
      await this.pollSnapshot(handle);
    }
  }

  /**
   * Polling fallback only: the renderer cannot be given the raw bytes it
   * needs (store resync, or replay older than the window) — switch to
   * authoritative screen mode without feeding further raw VT.
   */
  switchToSnapshotForCap(): void {
    if (this.disposed || this.status.handle === null) return;
    if (this.status.phase !== "live-raw" || this.status.mode !== "raw") return;
    this.publish(
      statusFor(this.status, {
        phase: "live-snapshot",
        message: COMPANION_TERMINAL_MESSAGES.snapshotCapped,
        mode: "snapshot" as CompanionTerminalMode,
        canWrite: true,
        canResize: true,
      }),
    );
  }

  private async pollRaw(handle: string): Promise<void> {
    const expected = this.status.offset;
    let result: RelayRpcTabsReadResult;
    try {
      result = await this.rpc.read(handle, expected);
    } catch (error) {
      if (this.disposed) return;
      if (desktopTookOf(error)) {
        this.takenByDesktop();
        return;
      }
      if (loginRequiredOf(error)) {
        // Login stops: no reattach, no replay.
        this.publish(statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.signIn, canWrite: false, canResize: false }));
        return;
      }
      // §4.5: a failed history part is a manual-Retry error (then live
      // without history), never a reattach loop.
      const part = partFailureOf(error);
      if (part !== null) {
        this.failPart(part);
        return;
      }
      if (isUnreadableOf(error)) {
        // Malformed payload fails closed: never advance, never reattach blindly.
        this.publish(
          statusFor(this.status, {
            phase: "error",
            message: COMPANION_TERMINAL_MESSAGES.invalid,
            canWrite: false,
            canResize: false,
          }),
        );
        return;
      }
      // Unknown transport failure (including stale handle after a host
      // generation refresh while the phone socket stays ready): bounded
      // reattach, never a permanent dead view.
      await this.reattachBounded(handle);
      return;
    }
    if (this.disposed) return;
    // Remote close is explicit and visible; stop polling raw bytes.
    if (!result.alive) {
      this.publish(
        statusFor(this.status, {
          phase: "closed",
          message:
            result.exitCode !== null
              ? `${COMPANION_TERMINAL_MESSAGES.closed} (exit ${result.exitCode})`
              : COMPANION_TERMINAL_MESSAGES.closed,
          alive: false,
          exitCode: result.exitCode,
          offset: result.base + decodedLength(result.data),
          canWrite: false,
          canResize: false,
        }),
      );
      return;
    }
    const decision = decideCompanionRead(expected, result);
    if (decision.kind === "gap") {
      // Ring eviction: switch to snapshot grid mode, never concatenate.
      this.publish(
        statusFor(this.status, {
          phase: "live-snapshot",
          message: COMPANION_TERMINAL_MESSAGES.snapshot,
          mode: "snapshot" as CompanionTerminalMode,
          canWrite: true,
          canResize: true,
        }),
      );
      return;
    }
    if (decision.kind === "empty") {
      // No new bytes: stay live, keep exact offset continuity. A lost
      // history keeps its notice; it clears only on a fresh seed.
      if (this.status.phase !== "live-raw") return;
      this.publish(statusFor(this.status, { message: this.historyMessage, canWrite: true, canResize: true }));
      return;
    }
    // History length is the runtime store's business (rolling window); a
    // renderer that needs dropped bytes is handled by requestReseed.
    const nextOffset = decision.nextOffset;
    this.feedRaw(result.data, decision.decoded);
    if (this.disposed) return;
    this.publish(
      statusFor(this.status, {
        phase: "live-raw" as CompanionTerminalPhase,
        message: this.historyMessage,
        offset: nextOffset,
        alive: true,
        canWrite: true,
        canResize: true,
      }),
    );
  }

  private async pollSnapshot(handle: string): Promise<void> {
    let result: RelayRpcTabsScreenResult;
    try {
      result = await this.rpc.screen(handle);
    } catch (error) {
      if (this.disposed) return;
      if (desktopTookOf(error)) {
        this.takenByDesktop();
        return;
      }
      if (loginRequiredOf(error)) {
        this.publish(statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.signIn, canWrite: false, canResize: false }));
        return;
      }
      // §4.5: a failed history part is a manual-Retry error (then live
      // without history), never a reattach loop.
      const part = partFailureOf(error);
      if (part !== null) {
        this.failPart(part);
        return;
      }
      if (isUnreadableOf(error)) {
        this.publish(
          statusFor(this.status, {
            phase: "error",
            message: COMPANION_TERMINAL_MESSAGES.invalid,
            canWrite: false,
            canResize: false,
          }),
        );
        return;
      }
      await this.reattachBounded(handle);
      return;
    }
    if (this.disposed) return;
    // Every screen result refreshes modes, even resync: the phone needs them
    // without a grid, and leaving alt-screen must not resume raw parsing.
    this.publishModes(this.modesFrom(result));
    const decision = decideCompanionScreen(result);
    if (decision.kind === "resync") {
      this.publish(
        statusFor(this.status, {
          phase: "live-snapshot",
          message: COMPANION_TERMINAL_MESSAGES.screenUnavailable,
          canWrite: this.status.alive,
          canResize: this.status.alive,
        }),
      );
      return;
    }
    try {
      this.sink.receive(decision.delta);
    } catch {
      /* Store faults stay local. */
    }
    if (this.disposed) return;
    this.publish(
      statusFor(this.status, {
        phase: "live-snapshot",
        message: this.historyMessage,
        canWrite: true,
        canResize: true,
      }),
    );
  }

  /**
   * §4.5 history-part failure (seed_failed/reply_failed): never an
   * automatic reconnect loop. The first failure is a manual-Retry error;
   * after a second failure the live stream keeps running without history.
   */
  private failPart(error: PartFailure): void {
    if (this.disposed) return;
    this.partFailures += 1;
    try {
      this.sink.notePartFailure?.();
    } catch {
      /* Diagnostics never break polling. */
    }
    const message = error.message || SEED_FAILED_MESSAGE;
    if (this.partFailures >= 2) {
      // Second failure: stay live without history. Streaming resubscribes
      // for live deltas from the current offset; polling just keeps reading.
      this.historyMessage = message;
      const handle = this.status.handle;
      if (this.streamEngaged && handle !== null) {
        this.startStream(handle, this.status.offset);
      } else {
        this.publish(
          statusFor(this.status, { message, canWrite: true, canResize: true }),
        );
      }
      return;
    }
    this.stopStream();
    this.streamEngaged = false;
    this.publish(
      statusFor(this.status, {
        phase: "error",
        message,
        canWrite: false,
        canResize: false,
      }),
    );
  }

  /**
   * Bounded recovery for a stale handle (host generation refresh while the
   * phone socket stays ready). Metadata-only reattach: detach the old handle
   * best-effort, reset the WASM epoch, attach a fresh handle. Never replays
   * input/resize. At most 3 attempts; afterwards surfaces error for manual
   * retry. Concurrent callers collapse into one attempt.
   */
  private async reattachBounded(staleHandle: string): Promise<void> {
    if (this.disposed || this.reattaching) return;
    if (this.reattachAttempts >= COMPANION_TERMINAL_MAX_REATTACH_ATTEMPTS) {
      this.publish(
        statusFor(this.status, {
          phase: "error",
          message: COMPANION_TERMINAL_MESSAGES.reattachFailed,
          canWrite: false,
          canResize: false,
        }),
      );
      return;
    }
    this.reattaching = true;
    this.stopStream();
    this.reattachAttempts += 1;
    const attempt = this.reattachAttempts;
    // Show recovery while the bounded attempts run (200ms cadence = backoff).
    if (!this.disposed) {
      this.publish(
        statusFor(this.status, {
          message: COMPANION_TERMINAL_MESSAGES.reattaching,
          canWrite: false,
          canResize: false,
        }),
      );
    }
    // Best-effort cleanup of the stale handle; never kills the host PTY.
    this.registry.remove(staleHandle);
    try {
      await this.rpc.detach(staleHandle);
    } catch {
      /* Best effort cleanup. */
    }
    if (this.disposed) {
      this.reattaching = false;
      return;
    }
    let result: RelayRpcTabsAttachResult;
    try {
      result = await this.rpc.attach(this.owner.tabId);
    } catch (error) {
      this.reattaching = false;
      if (this.disposed) return;
      if (desktopTookOf(error)) {
        this.takenByDesktop();
        return;
      }
      if (loginRequiredOf(error)) {
        this.publish(statusFor(this.status, { phase: "unavailable", message: COMPANION_TERMINAL_MESSAGES.signIn, canWrite: false, canResize: false }));
        return;
      }
      // Malformed attach fails closed; unknown failures retry until the bound.
      if (isUnreadableOf(error) || attempt >= COMPANION_TERMINAL_MAX_REATTACH_ATTEMPTS) {
        this.publish(
          statusFor(this.status, {
            phase: "error",
            message:
              attempt >= COMPANION_TERMINAL_MAX_REATTACH_ATTEMPTS
                ? COMPANION_TERMINAL_MESSAGES.reattachFailed
                : COMPANION_TERMINAL_MESSAGES.invalid,
            canWrite: false,
            canResize: false,
          }),
        );
        return;
      }
      this.publish(
        statusFor(this.status, {
          message: COMPANION_TERMINAL_MESSAGES.reattaching,
          canWrite: false,
          canResize: false,
        }),
      );
      return;
    }
    if (this.disposed) {
      this.reattaching = false;
      try {
        await this.rpc.detach(result.handle);
      } catch {
        /* Best effort cleanup. */
      }
      return;
    }
    if (!this.registry.tryAdd(result.handle)) {
      this.reattaching = false;
      try {
        await this.rpc.detach(result.handle);
      } catch {
        /* Best effort cleanup. */
      }
      this.publish(
        statusFor(this.status, { phase: "rejected", message: COMPANION_TERMINAL_MESSAGES.rejected }),
      );
      return;
    }
    await this.beginLive(result, this.attachGeneration);
  }

  /**
   * One-shot write while attached ready; never retried or replayed.
   * Pre-wire gate failures truthfully report not-sent. Once the request
   * reaches the wire, a transport failure leaves delivery unknown, so the
   * message reports unconfirmed — never claims unsent, never replays.
   */
  private inputTail: Promise<unknown> = Promise.resolve();
  private queuedInputs = 0;
  // Bumped by a failed write. Only input already queued behind that write is
  // dropped (its context is lost: "hello" failed, so its "\r" must not run).
  // Input made afterwards goes through: one lost acknowledgement must not lock
  // typing on a session that is still live.
  private inputEpoch = 0;
  // Keystrokes typed while an input request is in flight wait here and go out
  // together as the next request. Open until that request starts; never across
  // a submitDraft job (which closes it), so order is always kept.
  private keyBatch: { text: string; readonly handle: string | null; readonly epoch: number; result: Promise<InputResult> | null } | null = null;

  private enqueueInput(job: () => Promise<InputResult>): Promise<InputResult> {
    const handle = this.status.handle;
    const epoch = this.inputEpoch;
    this.keyBatch = null;
    this.queuedInputs++;
    const result = this.inputTail.then(async () => {
      if (epoch !== this.inputEpoch || this.status.handle !== handle) {
        return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      }
      const sent = await job();
      if (!sent.ok) this.inputEpoch++;
      return sent;
    }).finally(() => { this.queuedInputs--; });
    this.inputTail = result;
    return result;
  }

  /**
   * One key (or paste) from the renderer. With nothing in flight it goes out
   * at once; otherwise it joins the keys already waiting, so a burst of typing
   * costs one round trip rather than one per key. A merged request that fails
   * fails for every key in it, and the usual rule drops what queued behind.
   */
  sendInput(text: string): Promise<InputResult> {
    if (!this.attachedReady() || !isUsableCompanionInput(text)) {
      return Promise.resolve({ ok: false, message: COMPANION_TERMINAL_MESSAGES.sendNotReady });
    }
    const open = this.keyBatch;
    if (
      open !== null &&
      open.result !== null &&
      open.handle === this.status.handle &&
      open.epoch === this.inputEpoch &&
      utf8ByteLength(open.text) + utf8ByteLength(text) <= COMPANION_TERMINAL_LIMITS.inputUtf8Bytes
    ) {
      open.text += text;
      return open.result;
    }
    if (this.queuedInputs >= 64) {
      return Promise.resolve({ ok: false, message: COMPANION_TERMINAL_MESSAGES.sendNotReady });
    }
    const busy = this.queuedInputs > 0;
    const batch = { text, handle: this.status.handle, epoch: this.inputEpoch, result: null as Promise<InputResult> | null };
    const result = this.enqueueInput(() => {
      if (this.keyBatch === batch) this.keyBatch = null;
      return this.deliverInput(batch.text);
    });
    // Only keys that must wait anyway are held open for merging.
    if (busy) {
      batch.result = result;
      this.keyBatch = batch;
    }
    return result;
  }

  /**
   * Text, a gap, then Enter alone (see submitDraft.ts). The whole sequence is
   * one queue job, so a key pressed meanwhile lands after the Enter. Once the
   * text is out, any later failure reports unconfirmed: the words may already
   * sit in the program's input, and resending blindly would double them.
   */
  submitDraft(draft: string): Promise<InputResult> {
    if (!this.attachedReady() || this.queuedInputs >= 64) {
      return Promise.resolve({ ok: false, message: COMPANION_TERMINAL_MESSAGES.sendNotReady });
    }
    const refusal = draftRefusal(draft);
    if (refusal !== null) return Promise.resolve({ ok: false, message: refusal });
    const handle = this.status.handle as string;
    return this.enqueueInput(async () => {
      const modes = await this.refreshScreenModes().catch(() => this.screenModes);
      if (!this.liveOn(handle)) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      const written = await this.deliverInput(encodeDraft(draft, modes));
      if (!written.ok) return written;
      await this.delay(SUBMIT_CR_DELAY_MS);
      // Never to a different session: a reattach or detach in the gap ends it here.
      if (!this.liveOn(handle)) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed };
      const entered = await this.deliverInput(SUBMIT_ENTER);
      return entered.ok ? entered : { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed };
    });
  }

  private liveOn(handle: string): boolean {
    return this.attachedReady() && this.status.handle === handle && !this.reattaching;
  }

  private async deliverInput(text: string): Promise<InputResult> {
    if (!this.attachedReady() || !isUsableCompanionInput(text)) {
      return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
    }
    const handle = this.status.handle as string;
    try {
      await this.rpc.input(handle, text);
    } catch {
      // Delivery unknown after the wire: do not claim unsent, do not replay.
      return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed };
    }
    // The host acked before teardown: the bytes did go out, so "not sent" would
    // be false. Report unconfirmed so the person checks the terminal.
    if (!this.attachedReady()) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendUnconfirmed };
    return { ok: true as const };
  }

  /**
   * Bounded resize while attached ready; never retried. Same delivery
   * semantics as input: gate reports not-sent, post-wire loss reports
   * unconfirmed.
   */
  async sendResize(rows: number, cols: number): Promise<CompanionResizeResult> {
    if (!this.attachedReady() || !isUsableCompanionResize(rows, cols)) {
      return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeNotReady };
    }
    const handle = this.status.handle as string;
    try {
      await this.rpc.resize(handle, rows, cols);
    } catch (error) {
      // A definite refusal, not a lost delivery: the desktop holds the size.
      if (desktopTookOf(error)) {
        this.takenByDesktop();
        return {
          ok: false as const,
          message: COMPANION_TERMINAL_MESSAGES.desktopInControl,
          desktopInControl: true as const,
        };
      }
      return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeUnconfirmed };
    }
    if (this.disposed || !this.attachedReady()) {
      if (this.disposed) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeUnconfirmed };
      return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeNotReady };
    }
    this.sink.setViewport?.(rows, cols);
    return { ok: true as const };
  }

  /**
   * The desktop took the terminal back: the handle is gone for good, so
   * nothing retries or reattaches (that would take it straight back).
   */
  private takenByDesktop(): void {
    if (this.disposed || this.status.takenByDesktop) return;
    this.streamEngaged = false;
    this.stopStream();
    this.publish(
      statusFor(this.status, {
        phase: "detached",
        message: COMPANION_TERMINAL_MESSAGES.desktopInControl,
        canWrite: false,
        canResize: false,
        takenByDesktop: true,
      }),
    );
  }

  /** Release transport only. Never kills the host PTY. Idempotent. */
  async detach(): Promise<void> {
    const handle = this.status.handle;
    this.streamEngaged = false;
    this.stopStream();
    this.publish(
      statusFor(this.status, {
        phase: "detached",
        message: COMPANION_TERMINAL_MESSAGES.detached,
        canWrite: false,
        canResize: false,
      }),
    );
    if (handle !== null) {
      this.registry.remove(handle);
      try {
        await this.rpc.detach(handle);
      } catch {
        /* Detach is best effort; the handle is already released locally. */
      }
    }
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.attachGeneration += 1;
    this.streamEngaged = false;
    this.stopStream();
    if (this.modesTimer !== null) this.timers.clearTimeout(this.modesTimer);
    this.modesTimer = null;
    const handle = this.status.handle;
    if (handle !== null) {
      this.registry.remove(handle);
      // Fire-and-forget: unmount must not wait on the network, and the
      // host PTY keeps running regardless.
      void (async () => {
        try {
          await this.rpc.detach(handle);
        } catch {
          /* Best effort cleanup. */
        }
      })();
    }
    this.listeners.clear();
    this.publish(
      statusFor(this.status, { phase: "detached", message: COMPANION_TERMINAL_MESSAGES.detached, canWrite: false, canResize: false }),
    );
  }
}
