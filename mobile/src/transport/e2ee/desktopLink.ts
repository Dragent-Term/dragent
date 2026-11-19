/**
 * The phone's end-to-end encrypted link to one paired desktop
 * (docs/design/e2ee.md §4.2–§4.6; task E5a). It rides the relay socket's
 * binary frames and carries today's JSON envelopes (`rpc-request`,
 * `rpc-subscribe`, `rpc-unsubscribe` out; `rpc-reply`, `rpc-stream` in), so
 * the RPC client and everything above it keep their shapes.
 *
 * What it guarantees (the cryptography itself is native, §3.2):
 *
 * - **KK initiator, no data before confirmation (§4.2).** HS1 to the pinned
 *   desktop key; on HS2 the first DATA (ctr 0) is CTRL `hello`, then queued
 *   requests ride right behind it (§11: no added round trip).
 * - **§4.3 receiver.** Every DATA frame goes through the native channel's
 *   receiver: replays are dropped silently; a gap, a bad tag or a malformed
 *   record is `protocol_error` — CLOSE, zeroise, discard partial messages,
 *   fail pending requests, report "interrupted", reconnect and re-seed.
 * - **§4.6 rekey and reconnect with resume.** Once HS1 for a new channel is
 *   sent, the old channel sends nothing more (native `quiesce`), input waits
 *   in a queue, and the new `hello` says `{resume: {chan, last_sent_ctr}}`.
 *   On `input_lost` the queue is dropped, the requests the desktop discarded
 *   fail at once (implementer note 3), the terminal hears about it, then
 *   `input_lost_ack` goes out.
 * - **Output tail at a rekey (implementer note 1).** The old channel must end
 *   with `bye{rekeyed}`; otherwise every terminal stream is re-seeded. Every
 *   TERM must also continue its stream (`seq` + 1, `base` = previous
 *   `total`), or that stream is re-seeded: a silently stale screen can't
 *   happen.
 * - **Parts (§4.5).** Seeds, screens and large replies are applied atomically
 *   after the SHA-256 check; failures are `seed_failed` / `reply_failed`,
 *   never a reconnect loop.
 */
import type {
  Channel,
  KkHandshake,
  NativeE2ee,
  PartInfo,
  PartSession,
  Reassembler,
  TermOutput,
} from "../../../modules/dragent-e2ee/src/types";
import { e2eeErrorCode } from "../../../modules/dragent-e2ee/src/api";
import type { RealtimeClock } from "../clock";
import { DESKTOP_IN_CONTROL_CODE, RelayRpcError, RELAY_RPC_CONNECTION_LOST_MESSAGE } from "../relayRpcClient";
import { fromBase64, toBase64, toHex, utf8Decode, utf8Encode } from "./bytes";
import { CtrlError, decodeCtrl, encodeCtrl, type CtrlResume } from "./ctrl";
import type { DesktopChannelOwner, PhoneSecurity } from "./security";

/** The parts of the RPC client the link drives. */
export interface DesktopLinkRpc {
  handleMessage(raw: unknown): boolean;
  pendingRequests(): readonly { readonly id: string; readonly method: string }[];
  failRequest(id: string, error: RelayRpcError): boolean;
  requestMethod(id: string): string | null;
  resubscribe(stream: string): boolean;
  failStream(stream: string, error: RelayRpcError): boolean;
}

export type DesktopLinkState =
  /** A handshake is in flight (first connect, reconnect or rekey). */
  | "connecting"
  | "open"
  /** The channel was lost; a new handshake follows after a backoff. */
  | "retrying"
  /** This desktop isn't paired on this phone (or was unpaired). */
  | "unpaired"
  /** §6.1: the phone's security records failed their check. */
  | "lost"
  /** This build can't run end-to-end encryption (the web, or no native module). */
  | "unavailable"
  | "closed";

/** Why the link is retrying (for the §9 screens). */
export type DesktopLinkProblem = "interrupted" | "handshake" | "refused";

export interface DesktopLinkStatus {
  readonly state: DesktopLinkState;
  readonly problem: DesktopLinkProblem | null;
  /** Plain words for the person (§9), or null when all is well. */
  readonly message: string | null;
}

export interface DesktopLinkOptions {
  readonly native: NativeE2ee;
  readonly security: PhoneSecurity;
  /** Lowercase canonical UUIDs (§5 prologues). */
  readonly account: string;
  readonly phone: string;
  readonly desktop: string;
  readonly clock: RealtimeClock;
  readonly rpc: DesktopLinkRpc;
  /** Write one binary frame to the relay socket; false when it can't. */
  sendBinary(bytes: Uint8Array): boolean;
  onStatus?: ((status: DesktopLinkStatus) => void) | undefined;
  /**
   * The channel was lost without a clean resume: the RPC layer must fail
   * every pending desktop request now and forget stream positions.
   */
  onChannelLost?: (() => void) | undefined;
  /** A channel opened. `afterLoss`: the RPC layer starts a new epoch (re-seed). */
  onChannelOpen?: ((info: { readonly resumed: boolean; readonly afterLoss: boolean }) => void) | undefined;
  /** §4.6 step 4: input on these terminal handles didn't reach the desktop. */
  onInputLost?: ((handles: readonly number[], message: string) => void) | undefined;
  readonly caps?: readonly string[] | undefined;
  readonly random?: (() => number) | undefined;
}

export const DESKTOP_LINK_TIMING = Object.freeze({
  tickMs: 1000,
  handshakeMs: 10_000,
  /** §4.6 step 5: the old channel's tail and `bye{rekeyed}` window. */
  windDownMs: 10_000,
  firstRetryMs: 500,
  maxRetryMs: 30_000,
});

export const DESKTOP_LINK_LIMITS = Object.freeze({
  queuedMessages: 256,
  queuedBytes: 2 * 1024 * 1024,
  /** One JSON message (§4.4: total_len ≤ 1 MiB). */
  messageBytes: 1024 * 1024,
  /** Open part sessions per channel. */
  parts: 4,
});

export const INPUT_LOST_CODE = "input_lost";
export const SEED_FAILED_CODE = "seed_failed";
export const REPLY_FAILED_CODE = "reply_failed";
export const CHANNEL_LOST_CODE = "channel_lost";

/** Development-build diagnostics only: link states and reasons, never content or keys. */
function linkDebug(event: string, detail: Record<string, unknown>): void {
  if ((globalThis as { __DEV__?: boolean }).__DEV__ === true) console.warn(`[e2ee-link] ${event}`, detail);
}
export const SEED_FAILED_MESSAGE = "Couldn't load this terminal's history";
export const REPLY_FAILED_MESSAGE = "The desktop's reply couldn't be loaded. Try again.";

export function inputLostMessage(desktopName: string): string {
  return `Some of what you typed didn't reach ${desktopName}`;
}

/** §9 wording for each problem. */
export function linkProblemMessage(problem: DesktopLinkProblem | "unpaired" | "lost", desktopName: string): string {
  switch (problem) {
    case "interrupted": return `Connection to ${desktopName} was interrupted. Reconnecting…`;
    case "handshake": return `Can't make a secure connection to ${desktopName}. Retrying…`;
    case "refused": return `This phone isn't paired with ${desktopName} any more. Pair again.`;
    case "unpaired": return "Pair this desktop to open it.";
    case "lost": return "Dragent's security records on this device couldn't be verified, so paired devices were reset. Pair again.";
  }
}

const PROTOCOL_ERROR_REASON = 0x01;
const UNKNOWN_PEER_REASON = 0x02;

interface PartEntry {
  readonly session: PartSession;
  readonly info: PartInfo;
}

interface ChanState {
  readonly ch: Channel;
  readonly id: Uint8Array;
  readonly idHex: string;
  readonly reasm: Reassembler;
  readonly parts: Map<string, PartEntry>;
  nextMsgId: number;
  /** §4.6: true after the desktop answered on this channel (resume drained). */
  drained: boolean;
  /** Requests and subscriptions sent here before the desktop drained (note 3). */
  readonly sinceHello: { readonly ids: Set<string>; readonly streams: Set<string> };
  /** Old channel only: when its tail window ends, and whether `bye{rekeyed}` came. */
  windDownAt: number | null;
}

interface Pending {
  readonly kk: KkHandshake;
  readonly id: Uint8Array;
  readonly idHex: string;
  readonly startedAt: number;
  readonly resume: CtrlResume | null;
}

interface TermState {
  /** Waiting for a reset/seed: anything else is the previous subscription's tail. */
  awaitingReset: boolean;
  lastTotal: number;
  lastSeq: number | null;
  outSeq: number;
}

type Queued = { readonly frame: Record<string, unknown>; readonly bytes: Uint8Array };

/**
 * "Nothing was sent yet" from the native channel. The bridge hands an empty
 * optional over as `undefined`, not `null`, and `hello` only accepts a
 * counter or `null`: anything that isn't a usable counter means none.
 */
function sentCounter(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function isInputMethod(method: string): boolean {
  // A prompt or an answer for the built-in agent is input too: never replayed.
  return method === "tabs.input" || method === "terminal.input" || method === "agent.send" || method === "agent.act";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function decodeText(bytes: Uint8Array): string | null {
  const Decoder = (globalThis as { TextDecoder?: new (label: string, opts: { fatal: boolean }) => { decode(b: Uint8Array): string } }).TextDecoder;
  if (Decoder) {
    try {
      return new Decoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      return null;
    }
  }
  return utf8Decode(bytes);
}

function encodeText(text: string): Uint8Array {
  const Encoder = (globalThis as { TextEncoder?: new () => { encode(s: string): Uint8Array } }).TextEncoder;
  return Encoder ? new Encoder().encode(text) : utf8Encode(text);
}

export class E2eeDesktopLink implements DesktopChannelOwner {
  readonly desktop: string;
  private readonly o: DesktopLinkOptions;
  private cur: ChanState | null = null;
  private old: ChanState | null = null;
  private pending: Pending | null = null;
  /** Where a dead channel stopped, for the next `hello` (same lease only). */
  private resumePoint: CtrlResume | null = null;
  private queue: Queued[] = [];
  private queuedBytes = 0;
  private readonly terms = new Map<number, TermState>();
  private attached = false;
  private dead = false;
  private lostSinceOpen = false;
  private failures = 0;
  private retryAt: number | null = null;
  private retryProblem: DesktopLinkProblem | null = null;
  private timer: unknown = null;
  private status: DesktopLinkStatus = { state: "connecting", problem: null, message: null };
  private readonly unregister: () => void;

  constructor(options: DesktopLinkOptions) {
    this.o = options;
    this.desktop = options.desktop;
    this.unregister = options.security.register(this);
  }

  getStatus(): DesktopLinkStatus { return this.status; }

  /** The relay socket was admitted: open a channel (resuming if we can). */
  attach(): void {
    if (this.dead) return;
    this.attached = true;
    this.retryAt = null;
    this.startHandshake(false);
    this.schedule();
  }

  /**
   * The relay socket is gone. Frames of these channels can't arrive any more:
   * remember where the live channel stopped (§4.6 resume), zeroise, and wait
   * for the next admission. Pending requests were failed by the socket layer.
   */
  detach(): void {
    if (!this.attached) return;
    this.attached = false;
    this.abandonPending(true);
    this.retire(this.cur, "peer_closed", true);
    this.retire(this.old, "peer_closed", false);
    this.cur = this.old = null;
    this.dropQueue(null);
    this.terms.clear();
    this.retryAt = null;
    this.cancelTimer();
    if (!this.dead && this.status.state !== "unpaired" && this.status.state !== "lost") this.setStatus("connecting", null);
  }

  /** A new logical session (lease): nothing may be resumed into it (§4.6). */
  resetSession(): void {
    this.resumePoint = null;
  }

  /** §6.4: the desktop was unpaired on this phone. Close and zeroise everything. */
  revoke(): void {
    this.shutdown("revoked");
    this.setStatus("unpaired", null);
  }

  /** The socket owner is done with this link. */
  close(): void {
    this.shutdown("local");
    this.setStatus("closed", null);
    this.unregister();
  }

  private shutdown(reason: "revoked" | "local"): void {
    if (this.dead) return;
    this.dead = true;
    this.attached = false;
    this.cancelTimer();
    this.abandonPending(false);
    this.retire(this.cur, reason, false);
    this.retire(this.old, reason, false);
    this.cur = this.old = null;
    this.resumePoint = null;
    this.dropQueue(null);
    this.terms.clear();
    this.o.onChannelLost?.();
  }

  /**
   * Send one JSON envelope to the desktop. While a handshake is in flight it
   * waits in the queue (§4.6 step 1). False when it can't go at all: the
   * caller fails the request at once.
   */
  sendJson(frame: Record<string, unknown>): boolean {
    if (this.dead || !this.attached) return false;
    if (this.status.state === "unpaired" || this.status.state === "lost") return false;
    let bytes: Uint8Array;
    try {
      bytes = encodeText(JSON.stringify(frame));
    } catch {
      return false;
    }
    if (bytes.length > DESKTOP_LINK_LIMITS.messageBytes) return false;
    this.noteOutgoing(frame);
    const s = this.cur;
    if (s && !this.pending && this.queue.length === 0) return this.sealMessage(s, "json", bytes, frame);
    if (this.queue.length >= DESKTOP_LINK_LIMITS.queuedMessages || this.queuedBytes + bytes.length > DESKTOP_LINK_LIMITS.queuedBytes) return false;
    this.queue.push({ frame, bytes });
    this.queuedBytes += bytes.length;
    return true;
  }

  /**
   * §6.6 push-key rotation (E9a): tell the desktop this phone's new push key
   * with `identity-rotate {new_pub, new_push_pub}` on the open channel.
   * `new_pub` is the phone's CURRENT identity key, so the identity pin never
   * changes. False when no confirmed channel is open (try again on the next
   * open) or the key is malformed.
   */
  announcePushKey(newPushPub: Uint8Array): boolean {
    if (this.dead || !this.attached || this.status.state !== "open") return false;
    const s = this.cur;
    if (!s || this.pending || newPushPub.length !== 32) return false;
    let identity: Uint8Array | null;
    try {
      identity = this.o.native.identityPublicKey();
    } catch {
      return false;
    }
    if (!identity || identity.length !== 32) return false;
    let content: Uint8Array;
    try {
      content = encodeCtrl({ kind: "identity-rotate", newPub: identity, newPushPub });
    } catch {
      return false;
    }
    return this.sealMessage(s, "ctrl", content, null);
  }

  /** One binary frame from the relay socket. */
  onFrame(bytes: Uint8Array): void {
    if (this.dead || !this.attached) return;
    let f;
    try {
      f = this.o.native.decodeFrame(bytes);
    } catch {
      // Unauthenticated junk from the relay: it can only cost us a frame.
      return;
    }
    const hex = toHex(f.chan);
    const p = this.pending;
    if (p && hex === p.idHex) {
      if (f.kind === "hs" && f.step === 2) this.finishHandshake(p, f.body);
      else if (f.kind === "close") this.handshakeFailed(f.body[0] === UNKNOWN_PEER_REASON ? "refused" : "handshake");
      return;
    }
    const s = this.cur && hex === this.cur.idHex ? this.cur : this.old && hex === this.old.idHex ? this.old : null;
    if (!s) return;
    if (f.kind === "close") {
      // Unauthenticated hint (§4.1): it can only cause a reconnect.
      this.channelEnded(s, "peer_closed");
      return;
    }
    if (f.kind !== "data") return;
    this.receiveData(s, f.ctr, f.body);
  }

  /** `relay-chan-closed {chan}`: the relay closed a channel under backpressure. */
  onRelayChannelClosed(chanHex: string): void {
    linkDebug("relay reported channel closed", { pending: this.pending?.idHex === chanHex });
    if (this.dead) return;
    if (this.pending && chanHex === this.pending.idHex) {
      this.handshakeFailed("handshake");
      return;
    }
    const s = this.cur?.idHex === chanHex ? this.cur : this.old?.idHex === chanHex ? this.old : null;
    if (s) this.channelEnded(s, "peer_closed");
  }

  /** Timers: handshake timeout, rekey, the old channel's wind-down, retries. */
  tick(): void {
    if (this.dead || !this.attached) return;
    const now = this.o.clock.now();
    if (this.pending && now - this.pending.startedAt >= DESKTOP_LINK_TIMING.handshakeMs) {
      this.handshakeFailed("handshake");
    }
    const windDownAt = this.old?.windDownAt ?? null;
    if (this.old && windDownAt !== null && now >= windDownAt) {
      // No `bye{rekeyed}` in time: the tail may be missing, so re-seed (note 1).
      this.retire(this.old, "local", false);
      this.old = null;
      this.reseedAll();
    }
    if (this.cur && !this.pending) {
      let due = false;
      try { due = this.cur.ch.rekeyDue(now); } catch { due = false; }
      if (due) this.startHandshake(true);
    }
    if (!this.cur && !this.pending && this.retryAt !== null && now >= this.retryAt) {
      this.retryAt = null;
      this.startHandshake(false);
    }
  }

  // ------------------------------------------------------------- handshake

  private startHandshake(rekey: boolean): void {
    if (this.dead || !this.attached || this.pending) return;
    const pin = this.o.security.pin(this.desktop);
    linkDebug("start handshake", { rekey, pin: pin.state });
    if (pin.state !== "pinned") {
      this.setStatus(pin.state === "lost" ? "lost" : "unpaired", null);
      this.dropQueue(new RelayRpcError(linkProblemMessage(pin.state === "lost" ? "lost" : "unpaired", this.name()), false, CHANNEL_LOST_CODE));
      return;
    }
    let resume: CtrlResume | null = null;
    if (rekey && this.cur) {
      // §4.6 step 1: from HS1 on, nothing more is sent on the old channel.
      const r = this.cur.ch.quiesce();
      resume = { chan: r.chan, lastSentCtr: sentCounter(r.lastSentCtr) };
      this.retire(this.old, "local", false);
      this.old = this.cur;
      this.old.windDownAt = null;
      this.cur = null;
    } else if (this.cur) {
      return;
    } else {
      resume = this.resumePoint;
    }
    const n = this.o.native;
    let kk: KkHandshake | null = null;
    try {
      const id = n.randomChannelId();
      kk = n.startKk({ account: this.o.account, phone: this.o.phone, desktop: this.desktop, chan: id }, pin.key, [...(this.o.caps ?? [])]);
      const frame = n.encodeHs1Frame("companion", id, this.desktop, kk.hs1);
      this.pending = { kk, id, idHex: toHex(id), startedAt: this.o.clock.now(), resume };
      if (this.status.state !== "retrying") this.setStatus("connecting", null);
      if (!this.o.sendBinary(frame)) this.handshakeFailed("interrupted");
    } catch (error) {
      if (kk && !this.pending) { try { kk.release(); } catch { /* freed */ } }
      if (e2eeErrorCode(error) === "E2EE_NO_IDENTITY") {
        this.setStatus("lost", null);
        return;
      }
      this.handshakeFailed("handshake");
    }
  }

  private finishHandshake(p: Pending, hs2: Uint8Array): void {
    let ch: Channel;
    try {
      ch = p.kk.finish(hs2, this.o.clock.now());
    } catch {
      this.handshakeFailed("handshake");
      return;
    }
    this.pending = null;
    try { p.kk.release(); } catch { /* freed */ }
    const s: ChanState = {
      ch,
      id: p.id,
      idHex: p.idHex,
      reasm: this.o.native.createReassembler(),
      parts: new Map(),
      nextMsgId: 1,
      drained: p.resume === null,
      sinceHello: { ids: new Set(), streams: new Set() },
      windDownAt: null,
    };
    this.cur = s;
    // §4.2: DATA ctr 0 is `hello`; §4.6 step 2: it says where the old one stopped.
    if (!this.sealMessage(s, "ctrl", encodeCtrl({ kind: "hello", resume: p.resume }), null)) {
      this.channelEnded(s, "local");
      return;
    }
    this.resumePoint = null;
    if (this.old) this.old.windDownAt = this.o.clock.now() + DESKTOP_LINK_TIMING.windDownMs;
    const afterLoss = this.lostSinceOpen;
    this.lostSinceOpen = false;
    this.failures = 0;
    this.setStatus("open", null);
    this.o.onChannelOpen?.({ resumed: p.resume !== null, afterLoss });
    // Input typed during the handshake goes now, right behind `hello`.
    const queued = this.queue;
    this.queue = [];
    this.queuedBytes = 0;
    for (const q of queued) {
      if (this.cur !== s) break;
      if (!this.sealMessage(s, "json", q.bytes, q.frame)) this.failQueued(q, null);
    }
  }

  private handshakeFailed(problem: DesktopLinkProblem): void {
    linkDebug("handshake failed", { problem });
    const p = this.pending;
    this.abandonPending(false);
    const old = this.old;
    if (old && old.windDownAt === null) {
      // A rekey handshake failed: the old channel is quiesced, so resume from it later.
      this.resumePoint = this.pointOf(old, p?.resume ?? null);
      this.retire(old, "local", false);
      this.old = null;
    } else if (p?.resume) {
      this.resumePoint = p.resume;
    }
    this.loss(problem);
  }

  private abandonPending(keepResume: boolean): void {
    const p = this.pending;
    if (!p) return;
    this.pending = null;
    if (keepResume && p.resume && !this.cur) this.resumePoint = p.resume;
    try { p.kk.abandon(); } catch { /* consumed */ }
    try { p.kk.release(); } catch { /* freed */ }
  }

  // --------------------------------------------------------------- receive

  private receiveData(s: ChanState, ctr: number, body: Uint8Array): void {
    let inbound;
    try {
      inbound = s.ch.receive(ctr, body);
    } catch (error) {
      const code = e2eeErrorCode(error);
      this.protocolError(s, code === "E2EE_PROTOCOL_ERROR");
      return;
    }
    if (inbound.kind === "duplicate") return; // §4.3: replay, dropped silently.
    if (inbound.kind !== "record") {
      this.protocolError(s, true);
      return;
    }
    let msg;
    try {
      msg = s.reasm.feed(inbound.record);
    } catch {
      this.protocolError(s, true);
      return;
    }
    if (!msg) return;
    try {
      if (msg.mtype === "json") this.onJson(s, msg.content);
      else if (msg.mtype === "term") this.onTerm(msg.content);
      else this.onCtrl(s, msg.content);
    } catch {
      // A message we can't make sense of from an authenticated peer is a
      // protocol violation: never act on half of it.
      if (this.cur === s || this.old === s) this.protocolError(s, true);
    }
  }

  private onJson(s: ChanState, content: Uint8Array): void {
    const text = decodeText(content);
    if (text === null) throw new Error("not utf-8");
    const value: unknown = JSON.parse(text);
    if (!isRecord(value)) throw new Error("not an object");
    if (value.type === undefined && typeof value.part_id === "string") {
      this.onPartChunk(s, value.part_id, content);
      return;
    }
    if (value.type === "rpc-reply") {
      // The desktop answers new-channel requests only after the resume drained.
      if (!s.drained) this.markDrained(s);
      this.o.rpc.handleMessage(value);
      return;
    }
    if (value.type === "rpc-stream") {
      this.o.rpc.handleMessage(value);
    }
  }

  private markDrained(s: ChanState): void {
    s.drained = true;
    s.sinceHello.ids.clear();
    s.sinceHello.streams.clear();
  }

  private onCtrl(s: ChanState, content: Uint8Array): void {
    let ctrl;
    try {
      ctrl = decodeCtrl(content);
    } catch (error) {
      if (error instanceof CtrlError && error.kind === "UnknownCommand") return; // newer desktop: ignore
      throw error;
    }
    if (ctrl === null) {
      this.onPartCtrl(s, content);
      return;
    }
    switch (ctrl.kind) {
      case "bye":
        if (s === this.old) {
          // The old channel's tail is complete (§4.6 step 5).
          this.retire(s, ctrl.reason === "rekeyed" ? "rekeyed" : "peer_closed", false);
          this.old = null;
          if (ctrl.reason !== "rekeyed") this.reseedAll();
        } else {
          this.channelEnded(s, "peer_closed");
        }
        return;
      case "input_lost":
        this.onInputLost(s, ctrl.handles);
        return;
      case "control_taken":
        // The desktop took these terminals back: their streams end with a
        // definite reason, so the screen leaves the tab instead of retrying.
        for (const h of ctrl.handles) {
          this.terms.delete(h);
          this.o.rpc.failStream(
            `terminal.${h}`,
            new RelayRpcError("Your desktop is using this terminal.", false, DESKTOP_IN_CONTROL_CODE),
          );
        }
        return;
      case "identity-rotate":
        // §6.6 routine rotation is re-pinned only after a confirming
        // handshake with the new key; this transport doesn't act on the
        // announcement itself (the old pin keeps working meanwhile).
        return;
      default:
        // hello / pair-* / input_lost_ack never come from the desktop on a
        // companion channel.
        throw new Error("unexpected CTRL");
    }
  }

  /**
   * §4.6 step 4 (phone): drop queued input, fail the requests the desktop
   * discarded (at once, note 3), tell the terminal screen, then acknowledge.
   */
  private onInputLost(s: ChanState, handles: readonly number[]): void {
    if (s !== this.cur) return;
    const message = inputLostMessage(this.name());
    for (const id of s.sinceHello.ids) this.failRequest(id, message);
    // Input still waiting for a reply from the old channel never arrived
    // either: its replies would have come before this message (one socket,
    // in order), so it fails now instead of waiting for a timeout.
    for (const p of this.o.rpc.pendingRequests()) {
      if (isInputMethod(p.method)) this.failRequest(p.id, message);
    }
    const streams = [...s.sinceHello.streams];
    s.sinceHello.ids.clear();
    s.sinceHello.streams.clear();
    for (const q of this.queue) this.failQueued(q, message);
    this.queue = [];
    this.queuedBytes = 0;
    try { this.o.onInputLost?.(handles, message); } catch { /* listener errors never stop the ack */ }
    if (!this.sealMessage(s, "ctrl", encodeCtrl({ kind: "input_lost_ack" }), null)) {
      this.channelEnded(s, "local");
      return;
    }
    this.markDrained(s);
    // Subscriptions sent before the ack were discarded too: ask again.
    for (const stream of streams) {
      if (stream.startsWith("terminal.")) this.termFor(stream)?.reset();
      this.o.rpc.resubscribe(stream);
    }
  }

  private failRequest(id: string, inputMessage: string): void {
    const method = this.o.rpc.requestMethod(id);
    if (method === null) return;
    const lostInput = isInputMethod(method);
    this.o.rpc.failRequest(id, lostInput
      ? new RelayRpcError(inputMessage, false, INPUT_LOST_CODE)
      : new RelayRpcError(RELAY_RPC_CONNECTION_LOST_MESSAGE, false, CHANNEL_LOST_CODE));
  }

  private failQueued(q: Queued, inputMessage: string | null): void {
    const f = q.frame;
    if (f.type === "rpc-request" && typeof f.id === "string") {
      this.failRequest(f.id, inputMessage ?? RELAY_RPC_CONNECTION_LOST_MESSAGE);
    }
  }

  // ------------------------------------------------------------------ TERM

  private termFor(stream: string): { reset(): void } | null {
    const h = Number(stream.slice("terminal.".length));
    if (!Number.isSafeInteger(h)) return null;
    return { reset: () => this.terms.set(h, { awaitingReset: true, lastTotal: 0, lastSeq: null, outSeq: 0 }) };
  }

  private onTerm(content: Uint8Array): void {
    const t: TermOutput = this.o.native.decodeTerm(content);
    const st = this.terms.get(t.handle);
    const stream = `terminal.${t.handle}`;
    if (!st) {
      // No live subscription on this phone: nothing to render.
      return;
    }
    if (st.awaitingReset) {
      if (!t.reset) return; // the previous subscription's tail
      st.awaitingReset = false;
      st.outSeq = 0;
    } else if (!t.reset && ((st.lastSeq !== null && t.seq !== st.lastSeq + 1) || t.base !== st.lastTotal)) {
      // A missing piece of output: never render past it (note 1).
      this.reseed(t.handle);
      return;
    } else if (t.reset && st.lastSeq !== null && t.seq !== st.lastSeq + 1) {
      this.reseed(t.handle);
      return;
    }
    st.lastSeq = t.seq;
    st.lastTotal = t.total;
    st.outSeq += 1;
    const payload: Record<string, unknown> = {
      base: t.base,
      total: t.total,
      data: toBase64(t.data),
      alive: t.alive,
      reset: t.reset,
    };
    if (t.exitCode !== null && !t.alive) payload.exit_code = t.exitCode;
    if (t.reset && t.rows !== null && t.cols !== null) {
      payload.rows = t.rows;
      payload.cols = t.cols;
    }
    this.o.rpc.handleMessage({ type: "rpc-stream", v: 1, stream, seq: st.outSeq, payload });
  }

  private reseed(handle: number): void {
    this.terms.set(handle, { awaitingReset: true, lastTotal: 0, lastSeq: null, outSeq: 0 });
    this.o.rpc.resubscribe(`terminal.${handle}`);
  }

  private reseedAll(): void {
    for (const h of Array.from(this.terms.keys())) this.reseed(h);
  }

  private noteOutgoing(frame: Record<string, unknown>): void {
    const stream = typeof frame.stream === "string" ? frame.stream : null;
    if (stream?.startsWith("terminal.")) {
      const h = Number(stream.slice("terminal.".length));
      if (Number.isSafeInteger(h) && h >= 0) {
        if (frame.type === "rpc-subscribe") this.terms.set(h, { awaitingReset: true, lastTotal: 0, lastSeq: null, outSeq: 0 });
        else if (frame.type === "rpc-unsubscribe") this.terms.delete(h);
      }
    }
  }

  // ----------------------------------------------------------------- parts

  private onPartCtrl(s: ChanState, content: Uint8Array): void {
    const text = decodeText(content);
    if (text === null) throw new Error("not utf-8");
    const space = text.indexOf(" ");
    const name = text.slice(0, space);
    const body: unknown = JSON.parse(text.slice(space + 1));
    if (name === "part-begin") {
      if (s.parts.size >= DESKTOP_LINK_LIMITS.parts) throw new Error("too many parts");
      const session = this.o.native.beginPart(content);
      const info = session.info();
      if (s.parts.has(info.partId)) {
        session.release();
        throw new Error("duplicate part");
      }
      s.parts.set(info.partId, { session, info });
      return;
    }
    if (!isRecord(body) || typeof body.part_id !== "string") throw new Error("bad part command");
    const entry = s.parts.get(body.part_id);
    if (!entry) return; // already failed and dropped
    s.parts.delete(body.part_id);
    if (name === "part-abort") {
      this.partFailed(entry);
      return;
    }
    if (name !== "part-end") {
      this.partFailed(entry);
      throw new Error("bad part command");
    }
    let payload: Uint8Array;
    try {
      payload = entry.session.end(content);
    } catch {
      // §4.5: hash mismatch or a missing chunk fails atomically.
      this.partFailed(entry);
      return;
    }
    entry.session.release();
    this.applyPart(entry.info, payload);
  }

  private onPartChunk(s: ChanState, partId: string, content: Uint8Array): void {
    const entry = s.parts.get(partId);
    if (!entry) return;
    try {
      entry.session.feed(content);
    } catch {
      // Over the cap or out of order: this payload fails, the channel lives.
      s.parts.delete(partId);
      this.partFailed(entry);
    }
  }

  private partFailed(entry: PartEntry): void {
    try { entry.session.abort(); } catch { /* done */ }
    try { entry.session.release(); } catch { /* freed */ }
    const info = entry.info;
    if (info.purpose === "seed" && info.streamHandle !== null) {
      this.terms.delete(info.streamHandle);
      this.o.rpc.failStream(`terminal.${info.streamHandle}`, new RelayRpcError(SEED_FAILED_MESSAGE, false, SEED_FAILED_CODE));
      return;
    }
    const id = this.partRequestId(info);
    if (id !== null) this.o.rpc.failRequest(id, new RelayRpcError(REPLY_FAILED_MESSAGE, false, REPLY_FAILED_CODE));
  }

  private partRequestId(info: PartInfo): string | null {
    if (info.requestIdJson === null) return null;
    try {
      const id: unknown = JSON.parse(info.requestIdJson);
      return typeof id === "string" ? id : null;
    } catch {
      return null;
    }
  }

  private applyPart(info: PartInfo, payload: Uint8Array): void {
    const text = decodeText(payload);
    let value: unknown;
    try {
      value = text === null ? null : JSON.parse(text);
    } catch {
      value = null;
    }
    if (info.purpose === "seed" && info.streamHandle !== null) {
      const h = info.streamHandle;
      const seed = isRecord(value) ? value : null;
      const st = this.terms.get(h);
      const data = seed && typeof seed.data === "string" ? fromBase64(seed.data) : null;
      if (!seed || !data || seed.handle !== h || typeof seed.base !== "number" || typeof seed.total !== "number" ||
        typeof seed.rows !== "number" || typeof seed.cols !== "number") {
        this.terms.delete(h);
        this.o.rpc.failStream(`terminal.${h}`, new RelayRpcError(SEED_FAILED_MESSAGE, false, SEED_FAILED_CODE));
        return;
      }
      if (!st) return;
      st.awaitingReset = false;
      st.lastSeq = null;
      st.lastTotal = seed.total;
      st.outSeq = 1;
      const out: Record<string, unknown> = {
        base: seed.base, total: seed.total, data: seed.data, alive: true, reset: true, rows: seed.rows, cols: seed.cols,
      };
      const omitted = seed.scrollback_omitted_lines;
      if (typeof omitted === "number" && Number.isSafeInteger(omitted) && omitted > 0) out.scrollback_omitted_lines = omitted;
      this.o.rpc.handleMessage({ type: "rpc-stream", v: 1, stream: `terminal.${h}`, seq: 1, payload: out });
      return;
    }
    // reply / screen: the payload is the whole rpc-reply envelope.
    const id = this.partRequestId(info);
    if (!isRecord(value) || value.type !== "rpc-reply" || (id !== null && value.id !== id)) {
      if (id !== null) this.o.rpc.failRequest(id, new RelayRpcError(REPLY_FAILED_MESSAGE, false, REPLY_FAILED_CODE));
      return;
    }
    this.o.rpc.handleMessage(value);
  }

  // ------------------------------------------------------------------ send

  private sealMessage(s: ChanState, mtype: "json" | "ctrl", content: Uint8Array, frame: Record<string, unknown> | null): boolean {
    const n = this.o.native;
    try {
      const msgId = s.nextMsgId;
      s.nextMsgId = msgId >= 0xffffffff ? 1 : msgId + 1;
      for (const record of n.encodeRecords(mtype, msgId, content)) {
        if (!this.o.sendBinary(s.ch.sealFrame(record))) return false;
      }
    } catch {
      return false;
    }
    if (frame && !s.drained) {
      if (frame.type === "rpc-request" && typeof frame.id === "string") s.sinceHello.ids.add(frame.id);
      if (frame.type === "rpc-subscribe" && typeof frame.stream === "string") s.sinceHello.streams.add(frame.stream);
    }
    return true;
  }

  // ---------------------------------------------------------------- losses

  /**
   * §4.3 on `protocol_error`: CLOSE, zeroise (the native channel already did),
   * discard partial messages, fail pending requests, report, reconnect.
   */
  private protocolError(s: ChanState, sendClose: boolean): void {
    linkDebug("protocol error", { sendClose });
    if (sendClose) {
      try {
        this.o.sendBinary(this.o.native.encodeFrame({
          kind: "close", step: 0, purpose: null, chan: s.id, peer: null, ctr: 0, body: Uint8Array.of(PROTOCOL_ERROR_REASON),
        }));
      } catch { /* best effort */ }
    }
    this.channelEnded(s, "local");
  }

  private channelEnded(s: ChanState, reason: "peer_closed" | "local"): void {
    linkDebug("channel ended", { reason, wasOld: s === this.old });
    if (s === this.old) {
      this.retire(s, reason, false);
      this.old = null;
      this.reseedAll();
      return;
    }
    if (s !== this.cur) return;
    this.retire(s, reason, true);
    this.cur = null;
    this.loss("interrupted");
  }

  /** Close and zeroise a channel; optionally remember its resume point. */
  private retire(s: ChanState | null, reason: "peer_closed" | "local" | "revoked" | "rekeyed", remember: boolean): void {
    if (!s) return;
    if (remember) this.resumePoint = this.pointOf(s, null);
    for (const entry of s.parts.values()) {
      try { entry.session.abort(); } catch { /* done */ }
      try { entry.session.release(); } catch { /* freed */ }
    }
    s.parts.clear();
    try { s.reasm.clear(); } catch { /* freed */ }
    try { s.ch.close(reason); } catch { /* closed */ }
    try { s.reasm.release(); } catch { /* freed */ }
    try { s.ch.release(); } catch { /* freed */ }
  }

  private pointOf(s: ChanState, fallback: CtrlResume | null): CtrlResume | null {
    try {
      return { chan: s.id, lastSentCtr: sentCounter(s.ch.lastSentCtr()) };
    } catch {
      return fallback;
    }
  }

  private loss(problem: DesktopLinkProblem): void {
    this.lostSinceOpen = true;
    this.terms.clear();
    this.dropQueue(null);
    this.o.onChannelLost?.();
    if (this.dead) return;
    this.failures += 1;
    const random = Math.max(0, Math.min(1, (this.o.random ?? Math.random)()));
    const base = problem === "refused" ? DESKTOP_LINK_TIMING.maxRetryMs : DESKTOP_LINK_TIMING.firstRetryMs * 2 ** Math.min(this.failures - 1, 6);
    this.retryAt = this.o.clock.now() + Math.min(DESKTOP_LINK_TIMING.maxRetryMs, base * (0.8 + 0.4 * random));
    this.retryProblem = problem;
    this.setStatus("retrying", problem);
  }

  private dropQueue(error: RelayRpcError | null): void {
    const queued = this.queue;
    this.queue = [];
    this.queuedBytes = 0;
    for (const q of queued) {
      const f = q.frame;
      if (f.type === "rpc-request" && typeof f.id === "string") {
        this.o.rpc.failRequest(f.id, error ?? new RelayRpcError(RELAY_RPC_CONNECTION_LOST_MESSAGE, false, CHANNEL_LOST_CODE));
      }
    }
  }

  // ---------------------------------------------------------------- status

  private name(): string {
    return this.o.security.desktopName(this.desktop) ?? "your desktop";
  }

  private setStatus(state: DesktopLinkState, problem: DesktopLinkProblem | null): void {
    linkDebug("status", { state, problem });
    let message: string | null = null;
    if (state === "retrying" && problem) message = linkProblemMessage(problem, this.name());
    else if (state === "unpaired" || state === "lost") message = linkProblemMessage(state, this.name());
    const next = { state, problem, message };
    if (this.status.state === next.state && this.status.problem === next.problem && this.status.message === next.message) return;
    this.status = Object.freeze(next);
    if (state !== "retrying") this.retryProblem = null;
    try { this.o.onStatus?.(this.status); } catch { /* listeners never break the link */ }
  }

  private schedule(): void {
    this.cancelTimer();
    if (this.dead || !this.attached) return;
    this.timer = this.o.clock.setTimeout(() => {
      this.timer = null;
      this.tick();
      this.schedule();
    }, DESKTOP_LINK_TIMING.tickMs);
  }

  private cancelTimer(): void {
    if (this.timer !== null) this.o.clock.clearTimeout(this.timer);
    this.timer = null;
  }

  /** Test and diagnostics view (no secrets). */
  debugState(): { cur: string | null; old: string | null; pending: string | null; queued: number; resume: CtrlResume | null; retryProblem: DesktopLinkProblem | null } {
    return {
      cur: this.cur?.idHex ?? null,
      old: this.old?.idHex ?? null,
      pending: this.pending?.idHex ?? null,
      queued: this.queue.length,
      resume: this.resumePoint,
      retryProblem: this.retryProblem,
    };
  }
}
