/**
 * A phone (the real RelaySocketClient + E2EE link over the Rust binding), a
 * scripted desktop (real `dragent_e2ee`) and a scripted, possibly malicious
 * relay between them. The relay sees only what the real one sees: frames.
 */
import type { NativeE2ee } from "../../../../modules/dragent-e2ee/src/types";
import { fromHex, toHex } from "../../e2ee/bytes";
import { PhoneSecurity } from "../../e2ee/security";
import type { RealtimeClock } from "../../clock";
import { RelaySocketClient, type RelaySocketOptions } from "../../RelaySocketClient";
import type { RealtimeSocket } from "../../socket";
import { Desk, Harness, type DeskInbound } from "../harness";

export const ACCOUNT = "0a0b0c0d-0000-4000-8000-000000000001";
export const PHONE = "0a0b0c0d-0000-4000-8000-0000000000a1";
export const DESKTOP = "0a0b0c0d-0000-4000-8000-0000000000d1";
export const DESKTOP_NAME = "Studio Mac";

export class Clock implements RealtimeClock {
  time = 1_700_000_000_000;
  private seq = 0;
  private readonly timers = new Map<number, { at: number; fn: () => void }>();
  now = () => this.time;
  setTimeout = (fn: () => void, ms: number) => { const id = ++this.seq; this.timers.set(id, { at: this.time + ms, fn }); return id; };
  clearTimeout = (id: unknown) => { this.timers.delete(id as number); };
  advance(ms: number): void {
    const end = this.time + ms;
    for (;;) {
      const next = [...this.timers.entries()].sort((a, b) => a[1].at - b[1].at)[0];
      if (!next || next[1].at > end) break;
      this.time = next[1].at;
      this.timers.delete(next[0]);
      next[1].fn();
    }
    this.time = end;
  }
}

export class FakeSocket implements RealtimeSocket {
  readyState = 0;
  binaryType = "blob";
  onopen: RealtimeSocket["onopen"] = null;
  onmessage: RealtimeSocket["onmessage"] = null;
  onerror: RealtimeSocket["onerror"] = null;
  onclose: RealtimeSocket["onclose"] = null;
  readonly text: Record<string, unknown>[] = [];
  /** Every binary frame the phone wrote, in order. */
  readonly binary: Uint8Array[] = [];
  send(data: string | ArrayBuffer): void {
    if (typeof data === "string") {
      const frame = JSON.parse(data) as Record<string, unknown>;
      // The relay answers heartbeats; they aren't part of what tests inspect.
      if (frame.type === "relay-ping") {
        this.receiveText({ type: "relay-pong", v: 1 });
        return;
      }
      this.text.push(frame);
    } else this.binary.push(new Uint8Array(data));
  }
  close(): void { this.readyState = 3; }
  open(): void { this.readyState = 1; this.onopen?.({}); }
  receiveText(frame: unknown): void { this.onmessage?.({ data: JSON.stringify(frame) }); }
  receiveBinary(bytes: Uint8Array): void {
    const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    // Like React Native: an ArrayBuffer only when asked for one, else a Blob.
    this.onmessage?.({ data: this.binaryType === "arraybuffer" ? buffer : { size: buffer.byteLength, type: "" } });
  }
  drop(code = 1006): void { this.readyState = 3; this.onclose?.({ code }); }
}

/** §4.1 header fields the relay parses. */
export function head(frame: Uint8Array): { kind: number; step: number; chan: string; ctr: number } {
  const view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
  return { kind: frame[1]!, step: frame[2]!, chan: toHex(frame.subarray(4, 20)), ctr: Number(view.getBigUint64(36)) };
}

/** The relay rewrites HS1 `peer` from the target to the initiator (§4.1). */
export function rewritePeer(frame: Uint8Array, initiator: string): Uint8Array {
  const out = frame.slice();
  out.set(fromHex(initiator.replace(/-/gu, ""))!, 20);
  return out;
}

export async function flush(): Promise<void> { for (let i = 0; i < 10; i++) await Promise.resolve(); }

export const text = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);
export const enc = (s: string): Uint8Array => new TextEncoder().encode(s);

export interface World {
  hn: Harness;
  native: NativeE2ee;
  security: PhoneSecurity;
  desk: Desk;
  clock: Clock;
  sockets: FakeSocket[];
  client: RelaySocketClient;
  /** Messages the desktop decoded, in order (with their channel). */
  inbox: (DeskInbound & { chan: string })[];
  /** Index into the phone socket's binary frames already relayed. */
  relayed: Map<FakeSocket, number>;
  inputLost: { handles: readonly number[]; message: string }[];
  /** The initiator id the relay writes into HS1 `peer` (a malicious relay may lie). */
  hs1Peer: string;
  socket(): FakeSocket;
  /** Admit the current socket (relay-ready with a lease). */
  admit(sessionId?: string): Promise<FakeSocket>;
  /** Relay phone → desktop. `filter` may drop (false) or replace frames. */
  pump(filter?: (frame: Uint8Array, h: ReturnType<typeof head>) => boolean | Uint8Array): void;
  /** Relay desktop → phone. */
  toPhone(frames: Uint8Array[]): void;
  /** The latest open desktop channel (hex). */
  chan(): string;
  /** Reply to a request the desktop received. */
  reply(id: string, result: unknown, chan?: string): void;
  /** Requests the desktop received (parsed JSON). */
  requests(): Record<string, unknown>[];
  close(): void;
}

let requestSeq = 0;

export function world(opts: Partial<RelaySocketOptions> & { pairedVersion?: number } = {}): World {
  const hn = new Harness();
  const native = hn.native();
  const phoneKey = native.createIdentityKey();
  native.createPushKey();
  const clock = new Clock();
  const security = new PhoneSecurity(native, () => clock.now());
  const desk = new Desk(hn, "d1", ACCOUNT, DESKTOP);
  desk.pin(PHONE, phoneKey);
  security.commitPairing({ desktop: DESKTOP, identityKey: desk.key, desktopName: DESKTOP_NAME, catalogVersion: opts.pairedVersion ?? 1 });
  const sockets: FakeSocket[] = [];
  const inbox: World["inbox"] = [];
  const relayed = new Map<FakeSocket, number>();
  const inputLost: World["inputLost"] = [];
  const chans: string[] = [];
  const leaseExpiry = new Map<string, number>();
  const client = new RelaySocketClient({
    gateway: "wss://relay.example",
    deviceId: DESKTOP,
    clock,
    resolveAccessToken: async () => "account-token",
    createSocket: () => { const s = new FakeSocket(); sockets.push(s); return s; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++requestSeq).padStart(12, "0")}`,
    random: () => 0.5,
    e2ee: { security, account: ACCOUNT, phone: PHONE },
    onInputLost: (handles, message) => inputLost.push({ handles, message }),
    ...opts,
  });
  const w: World = {
    hn, native, security, desk, clock, sockets, client, inbox, relayed, inputLost, hs1Peer: PHONE,
    socket: () => sockets.at(-1)!,
    async admit(sessionId = "lease-1") {
      const expiry = leaseExpiry.get(sessionId) ?? clock.now() + 86_400_000;
      leaseExpiry.set(sessionId, expiry);
      if (sockets.length === 0 || w.socket().readyState === 3) client.connect();
      await flush();
      const s = w.socket();
      s.open();
      s.receiveText({ type: "relay-ready", v: 1, session_id: sessionId, session_expires_at: expiry, access_expires_at: expiry - 400_000 });
      return s;
    },
    pump(filter) {
      const s = w.socket();
      const from = relayed.get(s) ?? 0;
      relayed.set(s, s.binary.length);
      for (const original of s.binary.slice(from)) {
        let frame = original;
        if (filter) {
          const verdict = filter(frame, head(frame));
          if (verdict === false) continue;
          if (verdict instanceof Uint8Array) frame = verdict;
        }
        const h = head(frame);
        if (h.kind === 1 && h.step === 1) {
          const r = desk.listen(rewritePeer(frame, w.hs1Peer), clock.now());
          if (r.hs2) { chans.push(r.chan!); w.toPhone([r.hs2]); }
          else if (r.close) w.toPhone([r.close]);
          continue;
        }
        let got: DeskInbound;
        try {
          got = desk.receive(frame);
        } catch {
          continue;
        }
        inbox.push({ ...got, chan: h.chan });
      }
    },
    toPhone(frames) { for (const f of frames) w.socket().receiveBinary(f); },
    chan: () => chans.at(-1)!,
    reply(id, result, chan) { w.toPhone(desk.sendJson(chan ?? w.chan(), { type: "rpc-reply", v: 1, id, ok: true, result })); },
    requests: () => inbox.filter((m): m is Extract<DeskInbound, { mtype: string }> & { chan: string } => "mtype" in m && m.mtype === "json")
      .map((m) => JSON.parse(m.text) as Record<string, unknown>),
    close() { client.close(); hn.close(); },
  };
  return w;
}
