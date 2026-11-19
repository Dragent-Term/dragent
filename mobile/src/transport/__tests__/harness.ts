/**
 * Test-only bridge to the Rust harness (./harness): the phone's native E2EE
 * module (E4's binding crate, real cryptography) and a scripted desktop peer.
 *
 * The native module is synchronous (JSI), so the bridge is too: requests go
 * down one FIFO and the reply is read back from another with blocking reads.
 * Nothing here is shipped; `npm run test:e2ee` builds the harness with cargo.
 */
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { closeSync, mkdtempSync, openSync, readSync, rmSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type {
  CatalogResult,
  Channel,
  ChannelIds,
  ChannelState,
  CtrlMessage,
  Frame,
  Inbound,
  KkHandshake,
  Message,
  NativeE2ee,
  PartInfo,
  PartSession,
  PinState,
  PinStore,
  PinnedDesktop,
  PushPin,
  Reassembler,
  TermOutput,
} from "../../../modules/dragent-e2ee/src/types";
import { E2eeError } from "../../../modules/dragent-e2ee/src/api";
import { fromHex, toHex } from "../e2ee/bytes";

const MOBILE = join(__dirname, "../../..");
const MANIFEST = join(__dirname, "harness/Cargo.toml");
const TARGET = join(MOBILE, "modules/dragent-e2ee/rust/target");
const BINARY = join(TARGET, "debug/e2ee-transport-harness");

let built = false;

/** Build the harness (offline, into the gitignored target dir). */
export function buildHarness(): void {
  if (built) return;
  execFileSync("cargo", ["build", "--offline", "--quiet", "--manifest-path", MANIFEST], {
    env: { ...process.env, CARGO_TARGET_DIR: TARGET },
    stdio: ["ignore", "inherit", "inherit"],
  });
  built = true;
}

type J = Record<string, unknown>;
const h = (b: Uint8Array): string => toHex(b);
const b = (x: unknown): Uint8Array => fromHex(x as string)!;

export class Harness {
  private readonly dir: string;
  private readonly child: ChildProcess;
  private readonly req: number;
  private readonly res: number;
  private buffer = "";

  constructor() {
    buildHarness();
    this.dir = mkdtempSync(join(tmpdir(), "e2ee-harness-"));
    const reqPath = join(this.dir, "req");
    const resPath = join(this.dir, "res");
    execFileSync("mkfifo", [reqPath, resPath]);
    this.child = spawn(BINARY, [reqPath, resPath], { stdio: ["ignore", "ignore", "inherit"] });
    // Opening order matches the harness: request (write) first, then response.
    this.req = openSync(reqPath, "w");
    this.res = openSync(resPath, "r");
    this.call({ op: "phone.init", dir: this.dir });
  }

  call(request: J): unknown {
    writeSync(this.req, JSON.stringify(request) + "\n");
    for (;;) {
      const nl = this.buffer.indexOf("\n");
      if (nl >= 0) {
        const line = this.buffer.slice(0, nl);
        this.buffer = this.buffer.slice(nl + 1);
        const reply = JSON.parse(line) as { ok?: unknown; err?: { code: string; message: string } };
        if (reply.err) throw new E2eeError(reply.err.code as never, reply.err.message);
        return reply.ok ?? null;
      }
      const chunk = Buffer.alloc(1 << 20);
      const n = readSync(this.res, chunk, 0, chunk.length, null);
      if (n === 0) throw new Error("harness exited");
      this.buffer += chunk.subarray(0, n).toString("utf8");
    }
  }

  close(): void {
    try { closeSync(this.req); } catch { /* closed */ }
    try { closeSync(this.res); } catch { /* closed */ }
    this.child.kill();
    rmSync(this.dir, { recursive: true, force: true });
  }

  /** The phone's native module, backed by the real Rust binding. */
  native(): NativeE2ee {
    const c = (op: string, args: J = {}) => this.call({ op, ...args });
    const obj = (id: string) => (method: string, args: J = {}) => this.call({ op: "obj.call", obj: id, method, ...args });
    const release = (id: string) => () => { this.call({ op: "obj.release", obj: id }); };

    const channel = (id: string): Channel => {
      const m = obj(id);
      const chanId = b(m("chanId"));
      return {
        chanId,
        state: () => m("state") as ChannelState,
        handshakeHash: () => b(m("handshakeHash")),
        remoteStatic: () => b(m("remoteStatic")),
        hasKeys: () => m("hasKeys") as boolean,
        processedThrough: () => m("processedThrough") as number | null,
        lastSentCtr: () => m("lastSentCtr") as number | null,
        isQuiesced: () => m("isQuiesced") as boolean,
        seal: (record) => {
          const r = m("seal", { record: h(record) }) as J;
          return { ctr: r.ctr as number, ciphertext: b(r.ciphertext) };
        },
        sealFrame: (record) => b(m("sealFrame", { record: h(record) })),
        receive: (ctr, ciphertext) => {
          const r = m("receive", { ctr, ciphertext: h(ciphertext) }) as J;
          if (r.kind === "duplicate") return { kind: "duplicate" };
          return { kind: r.kind as "record" | "confirming", record: b(r.record) } as Inbound;
        },
        quiesce: () => {
          const r = m("quiesce") as J;
          return { chan: b(r.chan), lastSentCtr: r.lastSentCtr as number | null };
        },
        poll: (now) => m("poll", { now }) as string | null,
        rekeyDue: (now) => m("rekeyDue", { now }) as boolean,
        close: (reason) => { m("close", { reason }); },
        release: release(id),
      };
    };

    const kk = (hs1: Uint8Array, id: string): KkHandshake => {
      const m = obj(id);
      let caps: string[] = [];
      return {
        hs1,
        get peerCaps() { return caps; },
        finish: (hs2, now) => {
          const r = m("finish", { hs2: h(hs2), now }) as J;
          caps = r.caps as string[];
          return channel(r.obj as string);
        },
        abandon: () => { m("abandon"); },
        release: release(id),
      };
    };

    const store = (id: string): PinStore => {
      const m = obj(id);
      return {
        generation: () => m("generation") as number,
        desktops: () => (m("desktops") as J[]).map((d): PinnedDesktop => ({
          id: d.id as string,
          identityKey: b(d.identityKey),
          name: d.name as string,
          pinnedAtMs: d.pinnedAtMs as number,
          rotationKey: d.rotationKey ? b(d.rotationKey) : null,
        })),
        lookup: (device) => {
          const r = m("lookup", { device }) as J;
          return (r.state === "pinned" ? { state: "pinned", identityKey: b(r.identityKey) } : { state: r.state }) as PinState;
        },
        candidateKeys: (device, now) => (m("candidateKeys", { device, now }) as string[]).map(b),
        pinDesktop: (device, key, name, catalogVersion, now) => { m("pinDesktop", { device, key: h(key), name, catalogVersion, now }); },
        announceRotation: () => { throw new Error("not in the harness"); },
        confirmRotation: () => { throw new Error("not in the harness"); },
        revoke: (device, now) => m("revoke", { device, now }) as boolean,
        openCatalog: (account, phone, desktop, blob) => {
          const r = m("openCatalog", { account, phone, desktop, blob: h(blob) }) as J;
          return (r.status === "accepted"
            ? { status: "accepted", version: r.version, json: b(r.json) }
            : { status: "unchanged", version: r.version }) as CatalogResult;
        },
        catalogHighWater: (desktop) => m("catalogHighWater", { desktop }) as number | null,
        commit: () => m("commit") as number,
        release: release(id),
      };
    };

    const native: NativeE2ee = {
      platform: "ios",
      vectorsCompiled: false,
      hasDevicePasscode: () => true,
      identityPublicKey: () => { const k = c("phone.identityPublicKey"); return k ? b(k) : null; },
      createIdentityKey: () => b(c("phone.createIdentityKey")),
      pushPublicKey: () => { const k = c("phone.pushPublicKey"); return k ? b(k) : null; },
      createPushKey: () => b(c("phone.createPushKey")),
      rotatePushKey: () => { throw new Error("not in the harness"); },
      deleteAllKeys: () => { c("phone.deleteAllKeys"); },
      keyStorageInfo: () => [],
      setPushContext: () => {},
      listPushPins: () => (c("phone.listPushPins") as J[]).map((p): PushPin => ({ desktop: p.desktop as string, key: b(p.key) })),
      setPushPin: (desktop, key) => { c("phone.setPushPin", { desktop, key: h(key) }); },
      removePushPin: (desktop) => { c("phone.removePushPin", { desktop }); },
      openPush: () => { throw new Error("not in the harness"); },
      openPinStore: () => store(c("phone.openPinStore") as string),
      resetPinStore: () => { c("phone.resetPinStore"); },
      startPairing: () => { throw new Error("not in the harness"); },
      startKk: (ids: ChannelIds, pin, caps) => {
        const r = c("phone.startKk", { account: ids.account, phone: ids.phone, desktop: ids.desktop, chan: h(ids.chan), pin: h(pin), caps }) as J;
        return kk(b(r.hs1), r.obj as string);
      },
      randomChannelId: () => b(c("phone.randomChannelId")),
      randomBytes: (n) => b(c("phone.randomBytes", { n })),
      sha256: (data) => b(c("phone.sha256", { data: h(data) })),
      decodeFrame: (bytes) => {
        const f = c("phone.decodeFrame", { bytes: h(bytes) }) as J;
        return { ...f, chan: b(f.chan), body: b(f.body) } as Frame;
      },
      encodeFrame: (f) => b(c("phone.encodeFrame", { ...f, chan: h(f.chan), body: h(f.body) })),
      encodeHs1Frame: (purpose, chan, peer, body) => b(c("phone.encodeHs1Frame", { purpose, chan: h(chan), peer, body: h(body) })),
      encodeRecords: (mtype, msgId, content) => (c("phone.encodeRecords", { mtype, msgId, content: h(content) }) as string[]).map(b),
      encodePadRecord: (len) => b(c("phone.encodePadRecord", { len })),
      decodeTerm: (content) => {
        const t = c("phone.decodeTerm", { content: h(content) }) as J;
        return { ...t, data: b(t.data) } as TermOutput;
      },
      decodeCtrl: (content) => c("phone.decodeCtrl", { content: h(content) }) as CtrlMessage,
      encodeCtrl: (name, json) => b(c("phone.encodeCtrl", { name, json })),
      createReassembler: (): Reassembler => {
        const id = c("phone.createReassembler") as string;
        const m = obj(id);
        return {
          feed: (record) => {
            const r = m("feed", { record: h(record) }) as J | null;
            return r ? ({ mtype: r.mtype, content: b(r.content) } as Message) : null;
          },
          buffered: () => m("buffered") as number,
          clear: () => { m("clear"); },
          release: release(id),
        };
      },
      beginPart: (content): PartSession => {
        const id = c("phone.beginPart", { content: h(content) }) as string;
        const m = obj(id);
        return {
          info: () => m("info") as PartInfo,
          feed: (chunk) => { m("feed", { chunk: h(chunk) }); },
          end: (ctrl) => b(m("end", { content: h(ctrl) })),
          abort: () => { m("abort"); },
          release: release(id),
        };
      },
      nativeBenchmarks: () => [],
    };
    return native;
  }

  /** Test-only: corrupt the phone's sealed pin store file. */
  tamperPinStore(): void { this.call({ op: "phone.tamperPinStore" }); }
}

/** A scripted desktop on the real `dragent_e2ee` crate. */
export class Desk {
  readonly key: Uint8Array;
  constructor(private readonly hn: Harness, readonly name: string, readonly account: string, readonly desktop: string) {
    this.key = b(hn.call({ op: "desk.new", name, account, desktop }));
  }
  private c(op: string, args: J = {}): unknown { return this.hn.call({ op, desk: this.name, ...args }); }
  pin(phone: string, key: Uint8Array): void { this.c("desk.pin", { phone, key: h(key) }); }
  tombstone(phone: string): void { this.c("desk.tombstone", { phone }); }
  /** HS1 (with the relay-written initiator id in `peer`) → HS2 frame, or a CLOSE. */
  listen(frame: Uint8Array, now = 0, caps: string[] = []): { hs2?: Uint8Array; close?: Uint8Array; chan?: string } {
    const r = this.c("desk.listen", { frame: h(frame), now, caps }) as J;
    if (r.refused) return { close: b(r.close) };
    return { hs2: b(r.hs2), chan: r.chan as string };
  }
  receive(frame: Uint8Array): DeskInbound { return this.c("desk.receive", { frame: h(frame) }) as DeskInbound; }
  send(chan: string, mtype: "json" | "term" | "ctrl", content: Uint8Array): Uint8Array[] {
    return (this.c("desk.send", { chan, mtype, content: h(content) }) as string[]).map(b);
  }
  sendJson(chan: string, value: unknown): Uint8Array[] {
    return this.send(chan, "json", new TextEncoder().encode(JSON.stringify(value)));
  }
  term(fields: { handle: number; seq: number; base: number; total: number; data?: Uint8Array; alive?: boolean; reset?: boolean; rows?: number; cols?: number; exitCode?: number }): Uint8Array {
    return b(this.c("desk.term", { ...fields, data: h(fields.data ?? new Uint8Array()) }));
  }
  ctrl(args: J): Uint8Array { return b(this.c("desk.ctrl", args)); }
  partBegin(args: J & { payload: Uint8Array }): Uint8Array { return b(this.c("desk.partBegin", { ...args, payload: h(args.payload) })); }
  partChunk(partId: string, index: number, bytes: Uint8Array): Uint8Array { return b(this.c("desk.partChunk", { partId, index, bytes: h(bytes) })); }
  partEnd(partId: string): Uint8Array { return b(this.c("desk.partEnd", { partId })); }
  closeFrame(chan: string, reason: number): Uint8Array { return b(this.c("desk.closeFrame", { chan: chan, reason })); }
  chanState(chan: string): { state: string; processedThrough: number | null; lastSentCtr: number | null } {
    return this.c("desk.chanState", { chan }) as never;
  }
  sealCatalog(version: number, json: unknown, phones: { id: string; key: Uint8Array }[]): Uint8Array {
    return b(this.c("desk.sealCatalog", { version, json: h(new TextEncoder().encode(JSON.stringify(json))), phones: phones.map((p) => ({ id: p.id, key: h(p.key) })) }));
  }
}

export type DeskInbound =
  | { protocolError: string }
  | { duplicate: true }
  | { closed: true; reason: number | null }
  | { notConfirmed: true }
  | { partial: true; ctr: number }
  | { ctr: number; mtype: "json" | "term" | "ctrl"; content: string; text: string; ctrlCanonical?: boolean };
