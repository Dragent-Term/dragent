import { isValidRequestId, parseServerFrame, utf8ByteLength } from "./frames";
import { createRequestId } from "./socket";
import type { RealtimeClock } from "./clock";
export const RELAY_RPC_DEFAULT_TIMEOUT_MS = 15000;
export const RELAY_RPC_TIMEOUT_MESSAGE = "request timed out";
export const RELAY_RPC_CONNECTION_LOST_MESSAGE = "Connection lost. Try again.";
export const RELAY_RPC_SEND_FAILED_MESSAGE = "Could not send the request.";
export const RELAY_RPC_FAILED_MESSAGE = "The request failed.";
export const RELAY_RPC_LIMITS = Object.freeze({ paramsBytes: 64 * 1024, pending: 128, streams: 16, idsPerEpoch: 8192 });
export class RelayRpcError extends Error {
  /**
   * A machine-readable reason when one applies: the desktop's error code, or
   * a transport reason such as `input_lost`, `seed_failed`, `reply_failed`,
   * `channel_lost` (docs/design/e2ee.md §4.5, §4.6, §9).
   */
  readonly code: string | null;
  constructor(message: string, readonly loginRequired = false, code: string | null = null) { super(message); this.name = "RelayRpcError"; this.code = code; }
}
/** The desktop's code when it took a terminal back from the phone. */
export const DESKTOP_IN_CONTROL_CODE = "desktop_in_control";
export type RelayStreamHandler = (payload: unknown, info: {
  readonly snapshot: boolean;
  readonly seq: number;
}) => void;
export interface RelaySubscription {
  readonly stream: string;
  readonly active: boolean;
  unsubscribe(): Promise<void>;
}
export interface RelayRpcClientOptions {
  send: (frame: Record<string, unknown>) => boolean;
  createRequestId?: () => string;
  clock?: RealtimeClock;
  requestTimeoutMs?: number;
  log?: (message: string) => void;
  /** Socket adapters start disconnected and admit only after authenticated ready. */
  initiallyConnected?: boolean;
  /** Known servers restart every subscribed stream at sequence 1. */
  snapshotSequence?: number;
}
interface PendingRequest {
  method: string;
  resolve(value: unknown): void;
  reject(error: RelayRpcError): void;
  timer: unknown;
}
interface StreamEntry {
  name: string;
  params: Record<string, unknown>;
  handler: RelayStreamHandler;
  onError: ((error: RelayRpcError) => void) | undefined;
  subscribeId: string;
  lastSeq: number | null;
  closed: boolean;
  unsubscribePromise: Promise<void> | null;
  resolveUnsubscribe: (() => void) | null;
  unsubscribeTimer: unknown;
}
function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function paramsObject(value: unknown): Record<string, unknown> | null {
  if (value === undefined)
    return {};
  if (!record(value))
    return null;
  try {
    const encoded = JSON.stringify(value);
    if (encoded.length > RELAY_RPC_LIMITS.paramsBytes || utf8ByteLength(encoded) > RELAY_RPC_LIMITS.paramsBytes)
      return null;
    const copy: unknown = JSON.parse(encoded);
    return record(copy) ? copy : null;
  }
  catch {
    return null;
  }
}
const defaultClock: RealtimeClock = { now: Date.now, setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>) };
/** One bounded correlation/stream registry. It owns neither sockets nor credentials. */
export class RelayRpcClient {
  private readonly clock: RealtimeClock;
  private readonly newId: () => string;
  private readonly timeoutMs: number;
  private readonly pending = new Map<string, PendingRequest>();
  private readonly streams = new Map<string, StreamEntry>();
  private readonly usedIds = new Set<string>();
  private epoch: number | null = null;
  private closed = false;
  private connected: boolean;
  constructor(private readonly options: RelayRpcClientOptions) {
    this.clock = options.clock ?? defaultClock;
    this.newId = options.createRequestId ?? createRequestId;
    this.timeoutMs = this.deadline(options.requestTimeoutMs);
    this.connected = options.initiallyConnected ?? true;
  }
  pendingCount(): number { return this.pending.size; }
  streamNames(): string[] { return [...this.streams.keys()]; }
  /** Called only after admission. Requests are never replayed; streams get a new snapshot. */
  setConnectionEpoch(epoch: number): void {
    if (this.closed || !Number.isSafeInteger(epoch) || epoch < 0 || (this.connected && this.epoch === epoch))
      return;
    const resubscribe = !this.connected || this.epoch !== null;
    this.disconnect();
    this.epoch = epoch;
    this.usedIds.clear();
    this.connected = true;
    if (resubscribe)
      for (const entry of this.streams.values())
        this.sendSubscription(entry);
  }
  /** Reject work immediately on loss, retaining only active subscription intent. */
  disconnect(loginRequired = false): void {
    this.connected = false;
    const error = new RelayRpcError(RELAY_RPC_CONNECTION_LOST_MESSAGE, loginRequired);
    for (const request of this.pending.values()) {
      this.clock.clearTimeout(request.timer);
      request.reject(error);
    }
    this.pending.clear();
    for (const entry of this.streams.values()) {
      entry.lastSeq = null;
      if (entry.unsubscribePromise)
        this.finishStream(entry);
    }
  }
  /**
   * The encrypted channel to the desktop dropped while the relay socket is
   * fine: fail only what went through the channel. What the relay itself
   * serves (`relayServed` says which: the catalog, the machine list) keeps
   * working. Failing everything here blanked the machine's catalog whenever
   * the channel had a hiccup, e.g. on opening a tab.
   */
  failChannelTraffic(relayServed: (kind: "request" | "stream", name: string) => boolean): void {
    const error = new RelayRpcError(RELAY_RPC_CONNECTION_LOST_MESSAGE);
    for (const [id, request] of [...this.pending]) {
      if (relayServed("request", request.method)) continue;
      this.clock.clearTimeout(request.timer);
      this.pending.delete(id);
      request.reject(error);
    }
    for (const entry of this.streams.values()) {
      if (relayServed("stream", entry.name)) continue;
      entry.lastSeq = null;
      if (entry.unsubscribePromise)
        this.finishStream(entry);
    }
  }
  request<T = unknown>(method: string, params?: unknown, opts?: {
    timeoutMs?: number;
  }): Promise<T> {
    if (this.closed || !this.connected)
      return Promise.reject(new RelayRpcError(RELAY_RPC_CONNECTION_LOST_MESSAGE));
    const body = paramsObject(params);
    if (!/^[A-Za-z0-9_.-]{1,128}$/u.test(method) || !body)
      return Promise.reject(new RelayRpcError("The relay request is invalid."));
    if (this.pending.size >= RELAY_RPC_LIMITS.pending)
      return Promise.reject(new RelayRpcError("Too many requests are waiting."));
    const id = this.nextId();
    if (!id)
      return Promise.reject(new RelayRpcError(RELAY_RPC_SEND_FAILED_MESSAGE));
    return new Promise<T>((resolve, reject) => {
      const timer = this.clock.setTimeout(() => {
        if (this.pending.delete(id))
          reject(new RelayRpcError(RELAY_RPC_TIMEOUT_MESSAGE));
      }, this.deadline(opts?.timeoutMs ?? this.timeoutMs));
      // Install BEFORE send: injected/native transports may synchronously reenter.
      this.pending.set(id, { method, resolve: (value) => resolve(value as T), reject, timer });
      if (!this.trySend({ type: "rpc-request", v: 1, id, method, params: body })) {
        const waiting = this.pending.get(id);
        this.pending.delete(id);
        this.clock.clearTimeout(timer);
        waiting?.reject(new RelayRpcError(RELAY_RPC_SEND_FAILED_MESSAGE));
      }
    });
  }
  subscribe(stream: string, params: unknown, handler: RelayStreamHandler, onError?: (error: RelayRpcError) => void): RelaySubscription {
    const body = paramsObject(params);
    if (this.closed || !/^[A-Za-z0-9_.-]{1,128}$/u.test(stream) || !body || typeof handler !== "function")
      throw new RelayRpcError(RELAY_RPC_FAILED_MESSAGE);
    if (this.streams.has(stream))
      throw new RelayRpcError("This stream is already subscribed.");
    if (this.streams.size >= RELAY_RPC_LIMITS.streams)
      throw new RelayRpcError("Too many streams are open.");
    const entry: StreamEntry = { name: stream, params: body, handler, onError, subscribeId: "", lastSeq: null, closed: false, unsubscribePromise: null, resolveUnsubscribe: null, unsubscribeTimer: null };
    this.streams.set(stream, entry);
    if (this.connected)
      this.sendSubscription(entry);
    return { stream, get active() { return !entry.closed; }, unsubscribe: () => this.unsubscribeEntry(entry) };
  }
  unsubscribe(stream: string): Promise<void> { const entry = this.streams.get(stream); return entry ? this.unsubscribeEntry(entry) : Promise.resolve(); }
  handleMessage(raw: unknown): boolean {
    if (this.closed || !this.connected)
      return false;
    let value: unknown = raw;
    if (typeof raw === "string") {
      try {
        value = JSON.parse(raw);
      }
      catch {
        return false;
      }
    }
    if (!record(value) || typeof value.type !== "string" || !value.type.startsWith("rpc-"))
      return false;
    let frame;
    try {
      frame = parseServerFrame(JSON.stringify(value));
    }
    catch {
      this.drop("Dropped malformed rpc frame");
      return true;
    }
    if (!frame || (frame.type !== "rpc-reply" && frame.type !== "rpc-stream")) {
      this.drop("Dropped malformed rpc frame");
      return true;
    }
    if (frame.type === "rpc-reply") {
      const request = this.pending.get(frame.id);
      if (!request) {
        const stream = [...this.streams.values()].find((entry) => entry.subscribeId === frame.id);
        if (stream && !frame.ok) {
          this.finishStream(stream);
          try { stream.onError?.(new RelayRpcError(frame.error.message, frame.error.loginRequired, frame.error.code ?? null)); }
          catch { this.drop("Dropped relay stream error handler failure"); }
        }
        else
          this.drop("Dropped rpc reply for unknown request");
      }
      else {
        this.pending.delete(frame.id);
        this.clock.clearTimeout(request.timer);
        if (frame.ok)
          request.resolve(frame.result);
        else
          request.reject(new RelayRpcError(frame.error.message, frame.error.loginRequired, frame.error.code ?? null));
      }
      return true;
    }
    const entry = this.streams.get(frame.stream);
    if (!entry || entry.closed) {
      this.drop("Dropped rpc frame for unknown stream");
      return true;
    }
    if (entry.lastSeq === null) {
      if (this.options.snapshotSequence !== undefined && frame.seq !== this.options.snapshotSequence)
        return true;
      entry.lastSeq = frame.seq;
      this.deliver(entry, frame.payload, true, frame.seq);
      if (entry.unsubscribePromise) {
        this.sendUnsubscribe(entry);
        this.finishStream(entry);
      }
    }
    else if (frame.seq === entry.lastSeq + 1) {
      entry.lastSeq = frame.seq;
      this.deliver(entry, frame.payload, false, frame.seq);
    }
    else if (frame.seq > entry.lastSeq) {
      this.drop("Dropped rpc stream gap: resubscribing");
      entry.lastSeq = null;
      this.sendSubscription(entry);
    }
    else
      this.drop("Dropped stale rpc stream frame");
    return true;
  }
  /** Ids and methods of every pending request. */
  pendingRequests(): { readonly id: string; readonly method: string }[] {
    return [...this.pending.entries()].map(([id, request]) => ({ id, method: request.method }));
  }
  /** The method of a pending request, or null when it isn't pending. */
  requestMethod(id: string): string | null { return this.pending.get(id)?.method ?? null; }
  /** Fail one pending request now (no timeout wait). */
  failRequest(id: string, error: RelayRpcError): boolean {
    const request = this.pending.get(id);
    if (!request)
      return false;
    this.pending.delete(id);
    this.clock.clearTimeout(request.timer);
    request.reject(error);
    return true;
  }
  /** Ask for a fresh snapshot of a live stream (a re-seed), keeping its handler. */
  resubscribe(stream: string): boolean {
    const entry = this.streams.get(stream);
    if (!entry || entry.closed || !this.connected)
      return false;
    entry.lastSeq = null;
    this.sendSubscription(entry);
    return true;
  }
  /** End one stream with an error (its handler sees onError once). */
  failStream(stream: string, error: RelayRpcError): boolean {
    const entry = this.streams.get(stream);
    if (!entry || entry.closed)
      return false;
    this.finishStream(entry);
    try { entry.onError?.(error); }
    catch { this.drop("Dropped relay stream error handler failure"); }
    return true;
  }
  close(): void {
    if (this.closed)
      return;
    this.disconnect();
    this.closed = true;
    for (const entry of this.streams.values())
      this.finishStream(entry);
    this.usedIds.clear();
  }
  private deadline(value?: number): number { return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.min(value, 60000) : RELAY_RPC_DEFAULT_TIMEOUT_MS; }
  private unsubscribeEntry(entry: StreamEntry): Promise<void> {
    if (entry.closed)
      return Promise.resolve();
    if (!this.connected || entry.lastSeq !== null) {
      if (this.connected)
        this.sendUnsubscribe(entry);
      this.finishStream(entry);
      return Promise.resolve();
    }
    if (!entry.unsubscribePromise) {
      entry.unsubscribePromise = new Promise<void>((resolve) => { entry.resolveUnsubscribe = resolve; });
      entry.unsubscribeTimer = this.clock.setTimeout(() => { this.sendUnsubscribe(entry); this.finishStream(entry); }, this.timeoutMs);
    }
    return entry.unsubscribePromise;
  }
  private finishStream(entry: StreamEntry): void {
    entry.closed = true;
    if (this.streams.get(entry.name) === entry)
      this.streams.delete(entry.name);
    if (entry.unsubscribeTimer !== null)
      this.clock.clearTimeout(entry.unsubscribeTimer);
    entry.resolveUnsubscribe?.();
    entry.resolveUnsubscribe = null;
  }
  private sendSubscription(entry: StreamEntry): void {
    const id = this.nextId();
    if (!id) {
      this.finishStream(entry);
      return;
    }
    entry.subscribeId = id;
    if (!this.trySend({ type: "rpc-subscribe", v: 1, id, stream: entry.name, params: entry.params }))
      this.drop("Could not subscribe to relay stream");
  }
  private sendUnsubscribe(entry: StreamEntry): void {
    const id = this.nextId();
    if (id)
      this.trySend({ type: "rpc-unsubscribe", v: 1, id, stream: entry.name });
  }
  /**
   * The duplicate-id guard is a rolling window, not a budget: once it holds
   * `idsPerEpoch` ids the oldest settled one is forgotten, so a long session
   * never runs out of requests. Ids still awaiting a reply or naming a live
   * subscription are never forgotten.
   */
  private nextId(): string | null {
    if (this.usedIds.size >= RELAY_RPC_LIMITS.idsPerEpoch)
      this.forgetOldestIds();
    for (let attempt = 0; attempt < 3; attempt++) {
      let id: string;
      try {
        id = this.newId();
      }
      catch {
        return null;
      }
      if (!isValidRequestId(id) || this.usedIds.has(id))
        continue;
      this.usedIds.add(id);
      return id;
    }
    return null;
  }
  private forgetOldestIds(): void {
    const live = new Set<string>(this.pending.keys());
    for (const entry of this.streams.values())
      live.add(entry.subscribeId);
    for (const id of this.usedIds) {
      if (this.usedIds.size < RELAY_RPC_LIMITS.idsPerEpoch)
        return;
      if (!live.has(id))
        this.usedIds.delete(id);
    }
  }
  private trySend(frame: Record<string, unknown>): boolean { if (this.closed || !this.connected)
    return false; try {
    return this.options.send(frame) === true;
  }
  catch {
    return false;
  } }
  private deliver(entry: StreamEntry, payload: unknown, snapshot: boolean, seq: number): void { try {
    entry.handler(payload, { snapshot, seq });
  }
  catch {
    this.drop("Dropped relay stream handler error");
  } }
  private drop(message: string): void { try {
    this.options.log?.(message);
  }
  catch { /* Diagnostics never interrupt transport. */ } }
}
