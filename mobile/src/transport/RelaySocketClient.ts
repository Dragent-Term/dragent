import { RelayRpcClient, RelayRpcError, RELAY_RPC_LIMITS } from "./relayRpcClient";
import { utf8ByteLength } from "./frames";
import { createNativeSocket, type RealtimeSocket, type SocketFactory } from "./socket";
import type { RealtimeClock } from "./clock";
import { E2eeDesktopLink, type DesktopLinkStatus } from "./e2ee/desktopLink";
import type { PhoneSecurity } from "./e2ee/security";
import { openSealedCatalogPayload, type SealedCatalogResult } from "./e2ee/sealedCatalog";
import {
  COMPANION_SESSION_LIMITS,
  SESSION_ENDED_MESSAGE,
  SESSION_STORE_UNAVAILABLE_MESSAGE,
  buildAuthSessionFields,
  isLeaseExpired,
  isValidSessionId,
  parseReadySession,
  redialDelayMs,
  type CompanionSessionLease,
  type CompanionSessionStore,
} from "./companionSession";
export interface RelaySocketSnapshot {
  readonly status: "idle" | "connecting" | "ready" | "reconnecting" | "error";
  readonly error: string | null;
  readonly loginRequired: boolean;
  readonly connectionEpoch: number;
  readonly retryAttempt: number;
  /**
   * Logical companion session tracking: none (legacy server without lease
   * fields), active, or ended (4408/expiry — retained, never auto-renewed).
   */
  readonly sessionStatus?: "none" | "active" | "ended";
  readonly sessionEndsAt?: number | null;
  /**
   * The end-to-end encrypted channel to the target desktop
   * (docs/design/e2ee.md §4; null on account discovery sockets). Its
   * `message` is the §9 wording for the person.
   */
  readonly secure?: DesktopLinkStatus | null;
}

/**
 * End-to-end encryption for a targeted desktop socket. Everything the phone
 * and that desktop say to each other rides the encrypted channel; there is
 * no plaintext path (§10). Without it, desktop requests are refused.
 */
export type RelaySocketE2ee =
  | { readonly security: PhoneSecurity; readonly account: string; readonly phone: string }
  /** This build can't run E2EE (web, missing module): the plain-words reason. */
  | { readonly unavailable: string };

/**
 * Requests and streams the relay itself answers (routing only, §2.2). Every
 * other method goes to the desktop inside the encrypted channel.
 */
const SERVER_METHODS: ReadonlySet<string> = new Set(["machines.list", "companion.session.revoke"]);
const SERVER_STREAMS: ReadonlySet<string> = new Set(["catalog"]);

export function isServerRouted(frame: Record<string, unknown>): boolean {
  if (frame.type === "rpc-request") return typeof frame.method === "string" && SERVER_METHODS.has(frame.method);
  if (frame.type === "rpc-subscribe" || frame.type === "rpc-unsubscribe") return typeof frame.stream === "string" && SERVER_STREAMS.has(frame.stream);
  return false;
}
export interface RelaySocketOptions {
  gateway: string;
  /**
   * Target desktop id. Never the phone's installation id.
   * Optional only for account discovery (machines.list): an account socket
   * carries no target and must not inject one. A selected machine always
   * supplies an explicit device, which is forcibly injected on every
   * method/subscription.
   */
  deviceId?: string | undefined;
  resolveAccessToken: () => Promise<string | null>;
  /**
   * Persisted logical-session lease, owner-scoped by the hook wiring (the
   * repository keys by account/gateway/device). Absent in tests and legacy
   * uses: the lease then lives only in memory for the client's lifetime.
   */
  loadSession?: CompanionSessionStore["loadSession"];
  saveSession?: CompanionSessionStore["saveSession"];
  clearSession?: CompanionSessionStore["clearSession"];
  createSocket?: SocketFactory;
  clock?: RealtimeClock;
  createRequestId?: () => string;
  random?: () => number;
  /** Required for desktop requests on a targeted socket; see RelaySocketE2ee. */
  e2ee?: RelaySocketE2ee | undefined;
  /** §4.6 step 4: typed input on these handles didn't reach the desktop. */
  onInputLost?: ((handles: readonly number[], message: string) => void) | undefined;
}
/** Returns reported within this window are one and the same. */
const FOREGROUND_ONCE_MS = 1_000;
export const RELAY_SOCKET_TIMING = Object.freeze({ admissionMs: 15000, heartbeatMs: 25000, staleMs: 60000, firstRetryMs: 500, maxRetryMs: 30000, foregroundProbeMs: 3500, foregroundRedialAfterMs: 5000 });
const initial: RelaySocketSnapshot = Object.freeze({ status: "idle", error: null, loginRequired: false, connectionEpoch: 0, retryAttempt: 0, sessionStatus: "none", sessionEndsAt: null });
const clockDefault: RealtimeClock = { now: Date.now, setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>) };
const LOGIN_MESSAGE = "Pair this phone with your desktop to connect.";
/** §9 wording for the §10 admission refusals. */
export const UPDATE_APP_MESSAGE = "Update the Dragent app on this phone.";
export const PAIR_AGAIN_MESSAGE = "Pair this phone again to keep using it. Phone access is now end-to-end encrypted.";
const SESSION_INVALID_MESSAGE = "The machine session could not be read. Start a new session to continue.";
/** Credentials never belong in the URL. Plain WS is limited to local development. */
export function resolveRelaySocketURL(gateway: string): string | null {
  try {
    const url = new URL(gateway);
    if (url.username || url.password || url.search || url.hash)
      return null;
    if (url.protocol === "https:")
      url.protocol = "wss:";
    if (url.protocol === "http:")
      url.protocol = "ws:";
    if (url.protocol !== "wss:" && url.protocol !== "ws:")
      return null;
    if (url.protocol === "ws:" && !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))
      return null;
    return `${url.origin}/v1/relay/socket`;
  }
  catch {
    return null;
  }
}
/** Authenticated machine data channel. Shared-session grants never admit this socket. */
export class RelaySocketClient {
  readonly rpc: RelayRpcClient;
  private readonly options: RelaySocketOptions;
  private readonly clock: RealtimeClock;
  private readonly createSocket: SocketFactory;
  private snapshot = initial;
  private readonly listeners = new Set<() => void>();
  private socket: RealtimeSocket | null = null;
  private generation = 0;
  private active = false;
  private disposed = false;
  private authenticated = false;
  private authBlocked = false;
  private retry = 0;
  private lastMessageAt = 0;
  private admittedAt = 0;
  private retryTimer: unknown = null;
  private admissionTimer: unknown = null;
  private heartbeatTimer: unknown = null;
  /** Waiting for proof that a socket that looked open survived the background. */
  private probeTimer: unknown = null;
  /** When the last return to the foreground was acted on. */
  private foregroundAt = Number.NEGATIVE_INFINITY;
  private proactiveTimer: unknown = null;
  private sessionTimer: unknown = null;
  /** Held logical-session lease: retained across reconnects, never extended. */
  private session: { id: string; expiresAt: number } | null = null;
  /** Per-socket JWT bound from the latest ready; drives proactive redial. */
  private accessExpiresAt: number | null = null;
  /** Latched when the logical session ends: no automatic new lease. */
  private sessionEnded = false;
  /** Bounded 4401 recovery attempts since the last admission. */
  private authRecovery = 0;
  /** The encrypted channel to the target desktop (targeted sockets only). */
  private link: E2eeDesktopLink | null = null;
  constructor(options: RelaySocketOptions) {
    this.options = { ...options };
    this.clock = options.clock ?? clockDefault;
    this.createSocket = options.createSocket ?? createNativeSocket;
    this.rpc = new RelayRpcClient({
      send: (frame) => this.sendRPC(frame),
      initiallyConnected: false, snapshotSequence: 1, clock: this.clock,
      ...(options.createRequestId ? { createRequestId: options.createRequestId } : {}),
    });
    const target = typeof options.deviceId === "string" ? options.deviceId.trim() : "";
    const e2ee = options.e2ee;
    if (target !== "" && e2ee && "security" in e2ee) {
      this.link = new E2eeDesktopLink({
        native: e2ee.security.native,
        security: e2ee.security,
        account: e2ee.account,
        phone: e2ee.phone,
        desktop: target,
        clock: this.clock,
        rpc: this.rpc,
        ...(options.random ? { random: options.random } : {}),
        sendBinary: (bytes) => this.sendBinary(bytes),
        onStatus: (status) => this.update({ secure: status }),
        onChannelLost: () =>
          this.rpc.failChannelTraffic((kind, name) => (kind === "request" ? SERVER_METHODS : SERVER_STREAMS).has(name)),
        onChannelOpen: ({ afterLoss }) => {
          if (!afterLoss || !this.authenticated) return;
          const epoch = this.snapshot.connectionEpoch + 1;
          this.rpc.setConnectionEpoch(epoch);
          this.update({ connectionEpoch: epoch });
        },
        onInputLost: (handles, message) => options.onInputLost?.(handles, message),
      });
      this.snapshot = Object.freeze({ ...this.snapshot, secure: this.link.getStatus() });
    } else if (target !== "") {
      // Fail closed: no encryption, no desktop traffic (§10: never plaintext).
      const message = e2ee && "unavailable" in e2ee ? e2ee.unavailable : "This build of Dragent can't make an end-to-end encrypted connection.";
      this.snapshot = Object.freeze({ ...this.snapshot, secure: Object.freeze({ state: "unavailable" as const, problem: null, message }) });
    } else {
      this.snapshot = Object.freeze({ ...this.snapshot, secure: null });
    }
  }
  getSnapshot = (): RelaySocketSnapshot => this.snapshot;
  /**
   * Verify and open a sealed `catalog` stream payload for this socket's
   * desktop (§6.5). Without E2EE nothing can be verified, so nothing opens.
   */
  openCatalog = (payload: unknown): SealedCatalogResult => {
    const e2ee = this.options.e2ee;
    const target = typeof this.options.deviceId === "string" ? this.options.deviceId.trim() : "";
    if (!e2ee || !("security" in e2ee) || target === "") return { kind: "unreadable" };
    return openSealedCatalogPayload(e2ee.security, { account: e2ee.account, phone: e2ee.phone, desktop: target }, payload);
  };
  /** §6.6 (E9a): announce a new push key on the open encrypted channel. */
  announcePushKey = (newPushPub: Uint8Array): boolean => this.link?.announcePushKey(newPushPub) ?? false;
  subscribe = (listener: () => void): (() => void) => { if (this.disposed)
    return () => { }; this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  connect = (): void => {
    if (this.disposed)
      return;
    // An ended logical session never mints anew on plain connect: the user
    // action is startNewSession(). Surfacing ended keeps the next mount from
    // silently starting a fresh 24h lease.
    if (this.sessionEnded) {
      this.update({ status: "error", error: SESSION_ENDED_MESSAGE, loginRequired: false });
      return;
    }
    if (this.active && !this.authBlocked && ["connecting", "ready"].includes(this.snapshot.status))
      return;
    this.active = true;
    this.authBlocked = false;
    this.retry = 0;
    this.authRecovery = 0;
    this.dial();
  };
  /**
   * Explicit user action starting a fresh logical session: clears the stored
   * lease, drops the held one, then connects with no resume id so the server
   * mints a new lease. The only path that mints besides first-ever connect.
   */
  startNewSession = (): void => {
    if (this.disposed)
      return;
    const generation = ++this.generation;
    this.teardown();
    this.rpc.disconnect();
    this.session = null;
    this.link?.resetSession();
    this.accessExpiresAt = null;
    this.sessionEnded = false;
    this.authBlocked = false;
    this.retry = 0;
    this.authRecovery = 0;
    this.active = true;
    this.update({ status: "connecting", error: null, loginRequired: false, retryAttempt: 0, sessionStatus: "none", sessionEndsAt: null });
    void (async (): Promise<void> => {
      if (this.options.clearSession) {
        try {
          await this.options.clearSession();
        }
        catch {
          // Fail closed: a clear failure must not proceed fresh — the old
          // lease is retained and no socket is minted.
          if (this.owns(generation)) {
            this.update({ status: "error", error: SESSION_STORE_UNAVAILABLE_MESSAGE, loginRequired: false });
          }
          return;
        }
      }
      if (this.owns(generation)) this.dial();
    })();
  };
  notifyNetworkChange = (): void => { if (this.active && !this.authBlocked && !this.sessionEnded && !this.disposed)
    this.dial(); };
  /**
   * Back in the foreground after `awayMs` in the background. iOS suspends an
   * app a few seconds after it leaves and kills its socket without telling
   * either end; the encrypted channel to the desktop can go the same way. A
   * socket that still looks ready may be dead, and anything waiting on it sat
   * on "loading" until the heartbeat noticed, over a minute later. After a
   * real absence the connection is replaced at once, as reopening the app
   * would: a fresh channel, and every screen re-attaches on the new epoch.
   * After a quick switch away and back a ready socket is pinged and replaced
   * only if the relay doesn't answer within moments.
   */
  notifyForeground = (awayMs = 0): void => {
    if (!this.active || this.authBlocked || this.sessionEnded || this.disposed) return;
    // One socket is shared by every screen showing its desktop, and each
    // reports the same return: act once, or the second redial tears down
    // the connection the first one just started.
    const now = this.clock.now();
    if (now - this.foregroundAt < FOREGROUND_ONCE_MS) return;
    this.foregroundAt = now;
    if (
      awayMs >= RELAY_SOCKET_TIMING.foregroundRedialAfterMs ||
      this.snapshot.status !== "ready" ||
      !this.socket ||
      this.clock.now() - this.lastMessageAt > RELAY_SOCKET_TIMING.staleMs
    ) {
      this.dial();
      return;
    }
    if (this.probeTimer) return;
    const generation = this.generation;
    const sentAt = this.clock.now();
    try {
      this.socket.send(JSON.stringify({ type: "relay-ping", v: 1 }));
    } catch {
      this.dial();
      return;
    }
    this.probeTimer = this.clock.setTimeout(() => {
      this.probeTimer = null;
      if (!this.owns(generation) || this.disposed) return;
      if (this.lastMessageAt < sentAt) this.dial();
    }, RELAY_SOCKET_TIMING.foregroundProbeMs);
  };
  close = (): void => {
    if (this.disposed)
      return;
    this.active = false;
    this.disposed = true;
    ++this.generation;
    this.teardown();
    this.clearSessionTimer();
    this.link?.close();
    this.link = null;
    this.rpc.close();
    this.update({ status: "idle", error: null, loginRequired: false, retryAttempt: 0, sessionStatus: "none", sessionEndsAt: null });
    this.listeners.clear();
  };
  /** Load the persisted lease when none is held; expired leases end, never renew. */
  private async ensureLease(generation: number): Promise<boolean> {
    if (this.session) {
      if (isLeaseExpired(this.session.expiresAt, this.clock.now())) {
        this.enterEnded(SESSION_ENDED_MESSAGE);
        return false;
      }
      return true;
    }
    const load = this.options.loadSession;
    if (!load) return true;
    let stored: CompanionSessionLease | null = null;
    try {
      stored = await load();
    }
    catch {
      // Fail closed: storage unavailable must not silently reset the 24h
      // lease. Retryable error, no socket, no new session.
      if (this.owns(generation)) this.lost(SESSION_STORE_UNAVAILABLE_MESSAGE);
      return false;
    }
    if (!this.owns(generation)) return false;
    if (!stored || !isValidSessionId(stored.sessionId)) return true;
    if (isLeaseExpired(stored.expiresAt, this.clock.now())) {
      // The repository deliberately keeps expired leases: surface ended
      // rather than minting anew on app restart.
      this.session = { id: stored.sessionId, expiresAt: stored.expiresAt };
      this.enterEnded(SESSION_ENDED_MESSAGE);
      return false;
    }
    this.session = { id: stored.sessionId, expiresAt: stored.expiresAt };
    return true;
  }

  /** Resume id for the first frame: held unexpired lease only, never invented. */
  private resumeId(): string | null {
    const held = this.session;
    if (!held || isLeaseExpired(held.expiresAt, this.clock.now())) return null;
    return held.id;
  }

  private dial(): void {
    const generation = ++this.generation;
    this.teardown();
    this.rpc.disconnect();
    const endpoint = resolveRelaySocketURL(this.options.gateway);
    const rawDevice = this.options.deviceId;
    const target = typeof rawDevice === "string" ? rawDevice.trim() : "";
    // Account discovery sockets omit deviceId; an explicitly supplied device
    // must be non-empty. Targeted machine sockets always supply one (the
    // hook enforces it); the client itself only bounds what it was given.
    if (!endpoint) {
      this.authBlocked = true;
      this.update({ status: "error", error: "The machine relay address is invalid.", loginRequired: false });
      return;
    }
    if ((rawDevice !== undefined && target === "") || target.length > 128) {
      this.authBlocked = true;
      this.update({ status: "error", error: "The machine relay address is invalid.", loginRequired: false });
      return;
    }
    this.update({ status: this.retry ? "reconnecting" : "connecting", error: null, loginRequired: false, retryAttempt: this.retry });
    this.admissionTimer = this.clock.setTimeout(() => { if (this.owns(generation))
      this.lost("The machine relay timed out."); }, RELAY_SOCKET_TIMING.admissionMs);
    void (async (): Promise<void> => {
      if (!(await this.ensureLease(generation)))
        return;
      if (!this.owns(generation))
        return;
      let token: string | null;
      try {
        token = await this.options.resolveAccessToken();
      }
      catch {
        if (this.owns(generation))
          this.lost("Could not refresh account access.");
        return;
      }
      if (!this.owns(generation))
        return;
      if (!token || typeof token !== "string" || token.length > 16 * 1024) {
        this.authRejected();
        return;
      }
      const authFields = buildAuthSessionFields(target || null, this.resumeId());
      let socket: RealtimeSocket;
      try {
        socket = this.createSocket(endpoint, {});
      }
      catch {
        this.lost("Could not connect to the machine relay.");
        return;
      }
      this.socket = socket;
      try { socket.binaryType = "arraybuffer"; } catch { /* not settable on this socket */ }
      socket.onopen = () => {
        if (!this.owns(generation) || this.socket !== socket)
          return;
        try {
          // e2e:1 marks an end-to-end encrypting client (§10); the token stays in-band.
          socket.send(JSON.stringify({ type: "relay-auth", v: 1, access_token: token, e2e: 1, ...authFields }));
        }
        catch {
          this.lost("Could not authenticate the machine relay.");
        }
        // The only credential-bearing frame; no token is stored in snapshots.
        token = null;
      };
      socket.onmessage = (event) => {
        if (!this.owns(generation) || this.socket !== socket)
          return;
        this.receive(event.data);
      };
      socket.onerror = () => { if (this.owns(generation) && this.socket === socket)
        this.lost("The machine relay connection was interrupted."); };
      socket.onclose = (event) => {
        if (!this.owns(generation) || this.socket !== socket)
          return;
        // 4408 ends the logical session: retain the lease, never mint anew.
        if (event.code === 4408) {
          this.enterEnded(SESSION_ENDED_MESSAGE);
          return;
        }
        if (event.code === 4401 || event.code === 4403) {
          // Pre-admission close is a rejected credential and latches. A
          // post-admission close is lapsed access: bounded recovery that
          // always resumes the held lease, never mints fresh.
          if (this.authenticated) this.authRecoveryAttempt();
          else this.authRejected();
          return;
        }
        this.lost("The machine relay connection was interrupted.");
      };
    })();
  }
  private receive(raw: unknown): void {
    const binary = raw instanceof ArrayBuffer ? new Uint8Array(raw) : raw instanceof Uint8Array ? raw : null;
    if (binary) {
      // Binary frames are E2EE channel frames, only after admission (§4.1).
      if (!this.authenticated || binary.length > 44 + 65_535) return;
      this.lastMessageAt = this.clock.now();
      this.link?.onFrame(binary);
      return;
    }
    if (typeof raw !== "string" || raw.length > 6 * 1024 * 1024) {
      this.lost("The machine relay response was unreadable.");
      return;
    }
    let frame: Record<string, unknown>;
    try {
      const parsed: unknown = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed))
        return;
      frame = parsed as Record<string, unknown>;
    }
    catch {
      return;
    }
    if (frame.type === "relay-error" && frame.v === 1) {
      // Session end arrives as an error frame BEFORE the 4408 close. It must
      // end immediately: routing it through lost() would tear down the socket
      // first, hiding the 4408 and retry-looping a dead logical session.
      if (frame.sessionEnded === true) {
        this.enterEnded(SESSION_ENDED_MESSAGE);
        return;
      }
      // §10 cut-over refusals (before admission, then close 4403). The text
      // shown is ours, never the server's.
      if (frame.updateRequired === true) {
        this.e2eeRefused(UPDATE_APP_MESSAGE, false);
        return;
      }
      if (frame.repairRequired === true) {
        this.e2eeRefused(PAIR_AGAIN_MESSAGE, true);
        return;
      }
      if (frame.loginRequired === true)
        this.authRejected();
      else
        this.lost("The machine relay could not complete this connection.");
      return;
    }
    if (!this.authenticated) {
      if (frame.type !== "relay-ready" || frame.v !== 1)
        return;
      const ready = parseReadySession(frame);
      if (ready.kind === "invalid") {
        // Malformed session metadata fails closed: never admit as legacy,
        // which would discard the lease and bypass expiry.
        this.sessionInvalid();
        return;
      }
      this.authenticated = true;
      if (this.admissionTimer !== null)
        this.clock.clearTimeout(this.admissionTimer);
      this.admissionTimer = null;
      this.lastMessageAt = this.clock.now();
      this.admittedAt = this.lastMessageAt;
      this.authRecovery = 0;
      const epoch = this.snapshot.connectionEpoch + 1;
      this.rpc.setConnectionEpoch(epoch);
      if (ready.kind === "lease") {
        // Defensive: a brand-new already-expired lease ends, never renews.
        if (isLeaseExpired(ready.lease.sessionExpiresAt, this.clock.now())) {
          this.session = { id: ready.lease.sessionId, expiresAt: ready.lease.sessionExpiresAt };
          this.enterEnded(SESSION_ENDED_MESSAGE);
          return;
        }
        // A held lease must come back identical: same id AND same absolute
        // expiry. The server never replaces or extends on resume; anything
        // else fails closed instead of adopting new metadata.
        if (this.session) {
          if (
            ready.lease.sessionId !== this.session.id ||
            ready.lease.sessionExpiresAt !== this.session.expiresAt
          ) {
            this.sessionInvalid();
            return;
          }
          this.accessExpiresAt = ready.lease.accessExpiresAt;
          this.scheduleRedial(this.session.expiresAt, ready.lease.accessExpiresAt);
          this.scheduleSessionEnd(this.session.id, this.session.expiresAt);
        } else {
          this.adoptLease(ready.lease.sessionId, ready.lease.sessionExpiresAt, ready.lease.accessExpiresAt);
        }
        this.update({ status: "ready", error: null, loginRequired: false, connectionEpoch: epoch, retryAttempt: 0, sessionStatus: "active", sessionEndsAt: this.session!.expiresAt });
      } else {
        this.accessExpiresAt = null;
        if (!this.session) {
          this.update({ status: "ready", error: null, loginRequired: false, connectionEpoch: epoch, retryAttempt: 0, sessionStatus: "none", sessionEndsAt: null });
        } else {
          this.update({ status: "ready", error: null, loginRequired: false, connectionEpoch: epoch, retryAttempt: 0, sessionStatus: "active", sessionEndsAt: this.session.expiresAt });
        }
      }
      this.heartbeat();
      this.link?.attach();
      return;
    }
    this.lastMessageAt = this.clock.now();
    if (frame.type === "relay-pong" && frame.v === 1)
      return;
    if (frame.type === "relay-chan-closed") {
      if ((globalThis as { __DEV__?: boolean }).__DEV__ === true) {
        console.warn("[e2ee-link] relay closed a channel", { reason: (frame as { reason?: unknown }).reason ?? null });
      }
      if (typeof frame.chan === "string") this.link?.onRelayChannelClosed(frame.chan);
      return;
    }
    this.rpc.handleMessage(raw);
  }
  private sendRPC(frame: Record<string, unknown>): boolean {
    if (!this.authenticated || !this.socket || this.socket.readyState !== 1)
      return false;
    if (!isServerRouted(frame)) {
      // Desktop traffic goes only inside the encrypted channel, never as text.
      if (!this.link) return false;
      if ((frame.type === "rpc-request" || frame.type === "rpc-subscribe") &&
        utf8ByteLength(JSON.stringify(frame.params ?? {})) > RELAY_RPC_LIMITS.paramsBytes) return false;
      return this.link.sendJson(frame);
    }
    const outgoing = { ...frame };
    if (frame.type === "rpc-request" || frame.type === "rpc-subscribe") {
      const rawDevice = this.options.deviceId;
      const target = typeof rawDevice === "string" ? rawDevice.trim() : "";
      if (target !== "") {
        // A selected machine forcibly owns its target: a caller-supplied
        // device_id can never reroute it.
        outgoing.params = { ...(frame.params as Record<string, unknown>), device_id: target };
      } else {
        // Account discovery carries no target: never inject an empty one and
        // never erase a per-call device_id the caller explicitly supplied.
        outgoing.params = { ...(frame.params as Record<string, unknown>) };
        if (!("device_id" in (outgoing.params as Record<string, unknown>))) {
          // machines.list and other account-scoped calls send bare params.
        }
      }
      if (utf8ByteLength(JSON.stringify(outgoing.params)) > RELAY_RPC_LIMITS.paramsBytes)
        return false;
    }
    try {
      this.socket.send(JSON.stringify(outgoing));
      return true;
    }
    catch {
      return false;
    }
  }
  private sendBinary(bytes: Uint8Array): boolean {
    const socket = this.socket;
    if (!this.authenticated || !socket || socket.readyState !== 1) return false;
    try {
      socket.send(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer);
      return true;
    } catch {
      return false;
    }
  }
  private heartbeat(): void {
    this.heartbeatTimer = this.clock.setTimeout(() => {
      this.heartbeatTimer = null;
      if (!this.authenticated || !this.socket)
        return;
      if (this.clock.now() - this.lastMessageAt >= RELAY_SOCKET_TIMING.staleMs) {
        this.lost("The machine relay stopped responding.");
        return;
      }
      try {
        this.socket.send(JSON.stringify({ type: "relay-ping", v: 1 }));
      }
      catch {
        this.lost("The machine relay connection was interrupted.");
        return;
      }
      this.heartbeat();
    }, RELAY_SOCKET_TIMING.heartbeatMs);
  }
  private authRejected(): void {
    ++this.generation;
    this.authBlocked = true;
    this.teardown();
    this.rpc.disconnect(true);
    this.update({ status: "error", error: LOGIN_MESSAGE, loginRequired: true });
  }

  /** §10: refused before admission (old app, or paired before the cut-over). Latches. */
  private e2eeRefused(message: string, loginRequired: boolean): void {
    ++this.generation;
    this.authBlocked = true;
    this.teardown();
    this.rpc.disconnect(loginRequired);
    this.update({ status: "error", error: message, loginRequired });
  }

  /**
   * Adopt a fresh ready lease (no held lease — the ready branch reaffirms
   * held leases by strict id+expiry equality instead of merging).
   */
  private adoptLease(sessionId: string, sessionExpiresAt: number, accessExpiresAt: number): void {
    const expiresAt = sessionExpiresAt;
    // A new logical session: handles from the old one never carry over (§4.6).
    this.link?.resetSession();
    this.session = { id: sessionId, expiresAt };
    this.accessExpiresAt = accessExpiresAt;
    const save = this.options.saveSession;
    if (save) {
      const lease: CompanionSessionLease = Object.freeze({ sessionId, expiresAt });
      void Promise.resolve().then(() => save(lease)).catch(() => {});
    }
    this.scheduleRedial(expiresAt, accessExpiresAt);
    this.scheduleSessionEnd(sessionId, expiresAt);
  }

  /** Proactive redial at min(JWT margin, logical expiry); null lapses to 4401 recovery. */
  private scheduleRedial(sessionExpiresAt: number, accessExpiresAt: number): void {
    if (this.proactiveTimer !== null) {
      this.clock.clearTimeout(this.proactiveTimer);
      this.proactiveTimer = null;
    }
    const delay = redialDelayMs(sessionExpiresAt, accessExpiresAt, this.clock.now());
    if (delay === null) return;
    this.proactiveTimer = this.clock.setTimeout(() => {
      this.proactiveTimer = null;
      if (!this.active || this.disposed || this.authBlocked || this.sessionEnded) return;
      if (!this.authenticated) return;
      this.dial();
    }, delay);
  }

  /** Absolute logical expiry ends mobile access even without traffic. */
  private scheduleSessionEnd(sessionId: string, expiresAt: number): void {
    this.clearSessionTimer();
    const delay = expiresAt - this.clock.now();
    if (!(delay > 0)) {
      if (this.session?.id === sessionId) this.enterEnded(SESSION_ENDED_MESSAGE);
      return;
    }
    this.sessionTimer = this.clock.setTimeout(() => {
      this.sessionTimer = null;
      if (this.session?.id !== sessionId) return;
      this.enterEnded(SESSION_ENDED_MESSAGE);
    }, delay);
  }

  private clearSessionTimer(): void {
    if (this.sessionTimer !== null) {
      this.clock.clearTimeout(this.sessionTimer);
      this.sessionTimer = null;
    }
  }

  /**
   * Logical session ended (4408, absolute expiry, expired lease on load):
   * stop everything, surface the specific end, retain the stored lease so
   * the next mount surfaces ended instead of silently minting anew. Only
   * startNewSession or signout cleanup clears it.
   */
  private enterEnded(message: string): void {
    ++this.generation;
    this.sessionEnded = true;
    this.teardown();
    this.clearSessionTimer();
    this.rpc.disconnect();
    this.update({ status: "error", error: message, loginRequired: false, sessionStatus: "ended", sessionEndsAt: this.session?.expiresAt ?? null });
  }

  /** Malformed session metadata: fail closed, never admit as legacy. */
  private sessionInvalid(): void {
    ++this.generation;
    this.authBlocked = true;
    this.teardown();
    this.rpc.disconnect(true);
    this.update({ status: "error", error: SESSION_INVALID_MESSAGE, loginRequired: false });
  }

  /**
   * Bounded 4401 recovery after admission: fresh account token, same held
   * resume — never a fresh lease. Revoked credentials surface at admission
   * and latch there; the cap stops anything else from storming.
   */
  private authRecoveryAttempt(): void {
    if (this.authRecovery >= COMPANION_SESSION_LIMITS.authRecoveryMax) {
      this.authRejected();
      return;
    }
    this.authRecovery++;
    this.lost("The machine relay connection was interrupted.");
  }
  private lost(message: string): void {
    // A flapping connection must not reset backoff every time it briefly admits.
    if (this.authenticated && this.clock.now() - this.admittedAt >= RELAY_SOCKET_TIMING.staleMs)
      this.retry = 0;
    ++this.generation;
    this.teardown();
    this.rpc.disconnect();
    if (!this.active || this.disposed || this.authBlocked)
      return;
    this.retry++;
    this.update({ status: "reconnecting", error: message, retryAttempt: this.retry });
    const random = Math.max(0, Math.min(1, (this.options.random ?? Math.random)()));
    const delay = Math.min(RELAY_SOCKET_TIMING.maxRetryMs, RELAY_SOCKET_TIMING.firstRetryMs * 2 ** Math.min(this.retry - 1, 6) * (0.8 + 0.4 * random));
    this.retryTimer = this.clock.setTimeout(() => { this.retryTimer = null; if (this.active && !this.disposed && !this.authBlocked)
      this.dial(); }, delay);
  }
  private owns(generation: number): boolean { return this.active && !this.disposed && this.generation === generation; }
  private teardown(): void {
    for (const timer of [this.retryTimer, this.admissionTimer, this.heartbeatTimer, this.proactiveTimer, this.probeTimer])
      if (timer !== null)
        this.clock.clearTimeout(timer);
    this.retryTimer = this.admissionTimer = this.heartbeatTimer = this.proactiveTimer = this.probeTimer = null;
    this.authenticated = false;
    this.link?.detach();
    const socket = this.socket;
    this.socket = null;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
      try {
        socket.close();
      }
      catch { /* Closed already. */ }
    }
  }
  private update(patch: Partial<RelaySocketSnapshot>): void { this.snapshot = Object.freeze({ ...this.snapshot, ...patch }); for (const listener of this.listeners)
    listener(); }
}
/** Typed catalog validation is kept with the existing method readers. */
export { RelayRpcError };
