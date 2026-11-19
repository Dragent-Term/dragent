// Types of the native E2EE module (docs/design/e2ee.md §3.2, E4). The native
// side is ios/DragentE2EEModule.swift and android/.../DragentE2EEModule.kt;
// tests/module-parity.test.ts keeps the three in step.
//
// Bytes are Uint8Array. Ids are lowercase canonical UUID strings. Counters,
// versions and times are numbers (all below 2^53 by the design's limits).
// Private keys never appear here: the native module holds them.

/** Error codes thrown by the module (`error.code`). */
export type E2eeErrorCode =
  | 'E2EE_INVALID_ARGUMENT'
  | 'E2EE_REJECTED'
  | 'E2EE_MALFORMED'
  | 'E2EE_RANDOM'
  /** §4.3: the channel closed with protocol_error; keys zeroised. */
  | 'E2EE_PROTOCOL_ERROR'
  | 'E2EE_CHANNEL_CLOSED'
  | 'E2EE_NOT_READY'
  | 'E2EE_COUNTER_EXHAUSTED'
  | 'E2EE_TOO_LARGE'
  /** A sealed catalog/push came from a key that isn't pinned. */
  | 'E2EE_WRONG_SENDER'
  | 'E2EE_CATALOG_ROLLBACK'
  | 'E2EE_CATALOG_EQUIVOCATION'
  /** §6.1/§9: security records couldn't be verified; fail closed, pair again. */
  | 'E2EE_STORE_LOST'
  | 'E2EE_IO'
  | 'E2EE_CONSUMED'
  | 'E2EE_INTERNAL'
  /** §6.2 step 4: no device passcode, pairing refused. */
  | 'E2EE_NO_PASSCODE'
  | 'E2EE_NO_IDENTITY'
  | 'E2EE_LOCKED'
  | 'E2EE_NOT_CONFIGURED'
  | 'E2EE_KEYCHAIN'
  | 'E2EE_UNSUPPORTED'
  /** §1.3: E2EE refuses to run on the web platform. */
  | 'E2EE_UNSUPPORTED_PLATFORM';

export interface ChannelIds {
  account: string;
  phone: string;
  desktop: string;
  /** 16 bytes, from randomChannelId(). */
  chan: Uint8Array;
}

export type FrameKind = 'hs' | 'data' | 'close';
export type FramePurpose = 'companion' | 'pairing';

export interface Frame {
  kind: FrameKind;
  step: number;
  purpose: FramePurpose | null;
  chan: Uint8Array;
  peer: string | null;
  ctr: number;
  body: Uint8Array;
}

export type MessageType = 'json' | 'term' | 'ctrl';

export interface Message {
  mtype: MessageType;
  content: Uint8Array;
}

export interface TermOutput {
  handle: number;
  seq: number;
  base: number;
  total: number;
  alive: boolean;
  reset: boolean;
  exitCode: number | null;
  rows: number | null;
  cols: number | null;
  data: Uint8Array;
}

export interface CtrlMessage {
  name: string;
  json: string;
}

export type ChannelState =
  | { state: 'awaiting_confirm' | 'confirming' | 'open' | 'winding_down' }
  | { state: 'closed'; reason: string };

export type Inbound =
  | { kind: 'record'; record: Uint8Array }
  | { kind: 'confirming'; record: Uint8Array }
  /** A replay or duplicate: dropped silently (§4.3). */
  | { kind: 'duplicate' };

export type LocalCloseReason = 'local' | 'peer_closed' | 'revoked' | 'rekeyed';

/** One end of a channel (§4.2, §4.3). Native object; release() frees it. */
export interface Channel {
  readonly chanId: Uint8Array;
  state(): ChannelState;
  /** Pairing: `h` for `pair-confirm {h}` (§6.2 step 7). */
  handshakeHash(): Uint8Array;
  remoteStatic(): Uint8Array;
  hasKeys(): boolean;
  processedThrough(): number | null;
  lastSentCtr(): number | null;
  isQuiesced(): boolean;
  seal(record: Uint8Array): { ctr: number; ciphertext: Uint8Array };
  /** Seal one record and return the whole DATA frame (header + ciphertext). */
  sealFrame(record: Uint8Array): Uint8Array;
  /** Throws E2EE_PROTOCOL_ERROR on any gap or bad tag; the channel is then closed. */
  receive(ctr: number, ciphertext: Uint8Array): Inbound;
  /** §4.6 step 1: no more DATA here; returns the new hello's `resume`. */
  quiesce(): { chan: Uint8Array; lastSentCtr: number | null };
  poll(nowMs: number): string | null;
  rekeyDue(nowMs: number): boolean;
  close(reason: LocalCloseReason): void;
  release(): void;
}

export interface KkHandshake {
  /** The Noise HS1 (frame body). */
  readonly hs1: Uint8Array;
  /** The desktop's caps, after finish(). */
  readonly peerCaps: string[];
  finish(hs2: Uint8Array, nowMs: number): Channel;
  abandon(): void;
  release(): void;
}

export interface PairingHandshake {
  readonly hs1: Uint8Array;
  finish(hs2: Uint8Array, nowMs: number): Channel;
  abandon(): void;
  release(): void;
}

export interface Reassembler {
  /** Throws E2EE_PROTOCOL_ERROR on any §4.4 violation. */
  feed(record: Uint8Array): Message | null;
  buffered(): number;
  clear(): void;
  release(): void;
}

export interface PartInfo {
  partId: string;
  purpose: 'seed' | 'screen' | 'reply';
  requestIdJson: string | null;
  streamHandle: number | null;
  totalBytes: number;
  partCount: number;
}

export interface PartSession {
  info(): PartInfo;
  feed(chunkJson: Uint8Array): void;
  /** All chunks present and the SHA-256 matches → the payload (applied atomically). */
  end(ctrlContent: Uint8Array): Uint8Array;
  abort(): void;
  release(): void;
}

export interface PinnedDesktop {
  id: string;
  identityKey: Uint8Array;
  name: string;
  pinnedAtMs: number;
  rotationKey: Uint8Array | null;
}

export type PinState =
  | { state: 'pinned'; identityKey: Uint8Array }
  | { state: 'unpinned' }
  | { state: 'tombstoned' };

export type CatalogResult =
  | { status: 'accepted'; version: number; json: Uint8Array }
  | { status: 'unchanged'; version: number };

/** The phone pin store (§6.1). Changes are in memory until commit(). */
export interface PinStore {
  generation(): number;
  desktops(): PinnedDesktop[];
  lookup(device: string): PinState;
  candidateKeys(device: string, nowMs: number): Uint8Array[];
  /** §6.2 step 8: pin the key from the QR and set the catalog floor (L6). */
  pinDesktop(device: string, identityKey: Uint8Array, name: string, catalogVersion: number, nowMs: number): void;
  announceRotation(device: string, newKey: Uint8Array, nowMs: number): void;
  confirmRotation(device: string, confirmedKey: Uint8Array): boolean;
  /** §6.4 step 1. */
  revoke(device: string, nowMs: number): boolean;
  /** §6.5: verify against this store's pin and high-water mark, then open. */
  openCatalog(account: string, phone: string, desktop: string, blob: Uint8Array): CatalogResult;
  catalogHighWater(desktop: string): number | null;
  /** Persist (atomic write) and record the new generation in the keychain. */
  commit(): number;
  release(): void;
}

export interface PushPin {
  desktop: string;
  key: Uint8Array;
}

export interface PushNotification {
  desktopId: string;
  id: Uint8Array;
  issuedAtMs: number;
  kind: number;
  dismiss: boolean;
  title: string;
  body: string;
  workspaceId: string;
  tabId: string;
  dismissIds: Uint8Array[];
}

export interface KeyStorageItem {
  item: string;
  /** iOS: whenPasscodeSetThisDeviceOnly | afterFirstUnlockThisDeviceOnly; Android: unlockedDeviceRequired | afterFirstUnlock. */
  keyClass: string;
  accessGroup?: string | null;
  synchronizable: boolean;
}

export interface BenchResult {
  name: string;
  iterations: number;
  microsPerOp: number;
}

/** The raw native module (JSI). Use getE2ee() from the package index. */
export interface NativeE2ee {
  readonly platform: 'ios' | 'android';
  readonly vectorsCompiled: boolean;

  hasDevicePasscode(): boolean;
  identityPublicKey(): Uint8Array | null;
  createIdentityKey(): Uint8Array;
  pushPublicKey(): Uint8Array | null;
  createPushKey(): Uint8Array;
  rotatePushKey(nowMs: number): Uint8Array;
  deleteAllKeys(): void;
  keyStorageInfo(): KeyStorageItem[];

  setPushContext(account: string, phone: string): void;
  listPushPins(): PushPin[];
  setPushPin(desktop: string, key: Uint8Array): void;
  removePushPin(desktop: string): void;
  openPush(sealed: Uint8Array, nowMs: number): PushNotification;

  openPinStore(): PinStore;
  resetPinStore(): void;

  startPairing(ids: ChannelIds, desktopKey: Uint8Array, secretP: Uint8Array, deviceName: string): PairingHandshake;
  startKk(ids: ChannelIds, desktopPin: Uint8Array, caps: string[]): KkHandshake;

  randomChannelId(): Uint8Array;
  randomBytes(n: number): Uint8Array;
  sha256(data: Uint8Array): Uint8Array;
  decodeFrame(bytes: Uint8Array): Frame;
  encodeFrame(frame: Frame): Uint8Array;
  encodeHs1Frame(purpose: FramePurpose, chan: Uint8Array, peer: string, body: Uint8Array): Uint8Array;
  encodeRecords(mtype: MessageType, msgId: number, content: Uint8Array): Uint8Array[];
  encodePadRecord(len: number): Uint8Array;
  decodeTerm(content: Uint8Array): TermOutput;
  decodeCtrl(content: Uint8Array): CtrlMessage;
  encodeCtrl(name: string, json: string): Uint8Array;
  createReassembler(): Reassembler;
  beginPart(ctrlContent: Uint8Array): PartSession;

  nativeBenchmarks(iterations: number): BenchResult[];
}
