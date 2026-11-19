/**
 * End-to-end encrypted push, the phone's app side (docs/design/e2ee.md §8,
 * §6.6). Pure logic: no React Native imports, so `tsx --test` runs it.
 *
 * - **Push context.** The Notification Service Extension (iOS) and the
 *   messaging service (Android) open a sealed push with the push prologue
 *   `account|phone`; the app writes both ids for them with
 *   `setPushContext` whenever this phone is paired.
 * - **Push-key rotation (§6.6).** Every 180 days the push key is replaced
 *   (`rotatePushKey` keeps the old private key 7 days, so pushes sealed
 *   before a desktop learns the new key still open). Each paired desktop is
 *   told inside its open KK channel with `identity-rotate {new_pub: this
 *   phone's identity key, new_push_pub}`; desktops not reached yet stay
 *   pending and are told the next time their channel opens.
 * - **Reconcile on open (§8 step 6).** When a desktop's channel opens the
 *   phone asks `notifications.done` and removes what was handled on the
 *   desktop: on iOS by collapse id (the identifier APNs gives the
 *   notification), on Android by tag (the hex notification id).
 *
 * Never a plaintext fallback: nothing here sees push content.
 */

/** §6.6: routine push-key rotation. */
export const PUSH_ROTATION_MS = 180 * 24 * 60 * 60 * 1000;
/** Most handled ids accepted in one `notifications.done` reply. */
export const MAX_DONE_IDS = 256;
const RECORD_PREFIX = "dragent.push.rotation.v1";

export type PushPlatformName = "ios" | "android";

/** The native module calls this file uses (a subset of NativeE2ee). */
export interface PushKeyNative {
  pushPublicKey(): Uint8Array | null;
  rotatePushKey(nowMs: number): Uint8Array;
  setPushContext(account: string, phone: string): void;
}

export interface PushPrefs {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  removeItem(key: string): Promise<void>;
}

/** Stored per account+phone (not secret: dates and desktop ids only). */
export interface PushRotationRecord {
  readonly v: 1;
  /** When the current push key was created or last rotated. */
  readonly rotatedAt: number;
  /** Paired desktops not yet told the current push key. */
  readonly pending: readonly string[];
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function rotationKey(account: string, phone: string): string {
  return `${RECORD_PREFIX}:${account}:${phone}`;
}

export function parseRotationRecord(raw: string | null): PushRotationRecord | null {
  if (!raw) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) return null;
    const r = value as { v?: unknown; rotatedAt?: unknown; pending?: unknown };
    if (r.v !== 1 || typeof r.rotatedAt !== "number" || !Number.isFinite(r.rotatedAt) || r.rotatedAt < 0) return null;
    if (!Array.isArray(r.pending) || !r.pending.every((d) => typeof d === "string" && UUID.test(d))) return null;
    return Object.freeze({ v: 1, rotatedAt: r.rotatedAt, pending: Object.freeze([...new Set(r.pending as string[])]) });
  } catch {
    return null;
  }
}

export interface PushKeySyncDeps {
  readonly native: PushKeyNative;
  readonly prefs: PushPrefs;
  readonly account: string;
  readonly phone: string;
  /** Desktops currently paired on this phone. */
  readonly pairedDesktops: () => readonly string[];
  readonly now?: () => number;
}

/**
 * Push context and §6.6 push-key rotation for one account+phone. Calls are
 * serialised, so two desktops' channels opening together can't race.
 */
export class PushKeySync {
  private readonly deps: PushKeySyncDeps;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(deps: PushKeySyncDeps) {
    this.deps = deps;
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  private serial<T>(work: () => Promise<T>): Promise<T> {
    const next = this.queue.then(work, work);
    this.queue = next.catch(() => undefined);
    return next;
  }

  private async load(): Promise<PushRotationRecord | null> {
    try {
      return parseRotationRecord(await this.deps.prefs.getItem(rotationKey(this.deps.account, this.deps.phone)));
    } catch {
      return null;
    }
  }

  private async save(record: PushRotationRecord): Promise<void> {
    await this.deps.prefs.setItem(rotationKey(this.deps.account, this.deps.phone), JSON.stringify(record));
  }

  /** Write the push context the extension / messaging service read. */
  ensureContext(): void {
    this.deps.native.setPushContext(this.deps.account, this.deps.phone);
  }

  /**
   * Rotate the push key if it is 180 days old. A phone with no push key
   * (never paired) does nothing. The first call only records today.
   * Returns true when a new key was made.
   */
  maybeRotate(): Promise<boolean> {
    return this.serial(async () => {
      if (!this.deps.native.pushPublicKey()) return false;
      const now = this.now();
      const record = await this.load();
      if (!record) {
        await this.save({ v: 1, rotatedAt: now, pending: [] });
        return false;
      }
      if (now - record.rotatedAt < PUSH_ROTATION_MS) return false;
      // Record the desktops to tell before the key changes: a crash in
      // between then re-announces at worst (harmless), never forgets one.
      const pending = [...new Set([...record.pending, ...this.deps.pairedDesktops()])];
      await this.save({ v: 1, rotatedAt: record.rotatedAt, pending });
      this.deps.native.rotatePushKey(now);
      await this.save({ v: 1, rotatedAt: now, pending });
      return true;
    });
  }

  /**
   * A desktop's channel is open: tell it the current push key if it hasn't
   * heard it. `send` sends `identity-rotate` on that channel and returns
   * false when it couldn't. Returns true when an announcement was sent.
   */
  announce(desktop: string, send: (newPushPub: Uint8Array) => boolean): Promise<boolean> {
    return this.serial(async () => {
      const record = await this.load();
      if (!record || !record.pending.includes(desktop)) return false;
      const key = this.deps.native.pushPublicKey();
      if (!key) return false;
      if (!send(key)) return false;
      const paired = new Set(this.deps.pairedDesktops());
      await this.save({ v: 1, rotatedAt: record.rotatedAt, pending: record.pending.filter((d) => d !== desktop && paired.has(d)) });
      return true;
    });
  }

  /** Is `desktop` still waiting to hear the current push key? */
  async isPending(desktop: string): Promise<boolean> {
    return (await this.load())?.pending.includes(desktop) ?? false;
  }
}

/** A validated `notifications.done` reply. */
export interface DoneReply {
  /** iOS: APNs collapse ids (22 base64url characters). */
  readonly ids: readonly string[];
  /** Android: notification tags (32 lowercase hex). */
  readonly tags: readonly string[];
}

const COLLAPSE = /^[A-Za-z0-9_-]{22}$/;
const TAG = /^[0-9a-f]{32}$/;

/** Strict: anything unexpected is null (nothing is removed). */
export function parseDoneReply(value: unknown): DoneReply | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const r = value as Record<string, unknown>;
  if (Object.keys(r).some((k) => k !== "ids" && k !== "tags")) return null;
  const ids = r.ids;
  const tags = r.tags;
  if (!Array.isArray(ids) || !Array.isArray(tags)) return null;
  if (ids.length > MAX_DONE_IDS || tags.length > MAX_DONE_IDS) return null;
  if (!ids.every((i) => typeof i === "string" && COLLAPSE.test(i))) return null;
  if (!tags.every((t) => typeof t === "string" && TAG.test(t))) return null;
  return Object.freeze({ ids: Object.freeze([...ids] as string[]), tags: Object.freeze([...tags] as string[]) });
}

export interface DoneRpc {
  request<T = unknown>(method: string, params?: unknown): Promise<T>;
}

/**
 * §8 step 6: ask the desktop which notifications were handled there and
 * remove them. Returns how many identifiers were passed to `dismiss`.
 * Failures are quiet: a notification left showing is the only cost.
 */
export async function reconcileDone(
  rpc: DoneRpc,
  platform: PushPlatformName,
  dismiss: (identifiers: readonly string[]) => Promise<void>,
): Promise<number> {
  let reply: DoneReply | null;
  try {
    reply = parseDoneReply(await rpc.request("notifications.done", { platform }));
  } catch {
    return 0;
  }
  if (!reply) return 0;
  const identifiers = platform === "ios" ? reply.ids : reply.tags;
  if (identifiers.length === 0) return 0;
  try {
    await dismiss(identifiers);
  } catch {
    return 0;
  }
  return identifiers.length;
}

/** What `onChannelOpen` watches: a desktop socket's snapshot. */
export interface ChannelOpenSource {
  subscribe(listener: () => void): () => void;
  getSnapshot(): { readonly secure?: { readonly state: string } | null };
}

/**
 * Call `run` each time the desktop's encrypted channel becomes open (and at
 * once if it already is). Returns the unsubscribe.
 */
export function onChannelOpen(source: ChannelOpenSource, run: () => void): () => void {
  let wasOpen = false;
  const check = (): void => {
    const open = source.getSnapshot().secure?.state === "open";
    if (open && !wasOpen) {
      try {
        run();
      } catch {
        // A failing listener never breaks the socket's other subscribers.
      }
    }
    wasOpen = open;
  };
  const unsubscribe = source.subscribe(check);
  check();
  return unsubscribe;
}

export interface PushChannelClient extends ChannelOpenSource {
  readonly rpc: DoneRpc;
  announcePushKey(newPushPub: Uint8Array): boolean;
}

export interface PushChannelDeps {
  readonly sync: PushKeySync;
  readonly platform: PushPlatformName;
  readonly dismiss: (identifiers: readonly string[]) => Promise<void>;
  readonly now?: () => number;
  /** Reconcile at most this often per desktop (rekeys reopen the channel). */
  readonly minIntervalMs?: number;
}

/** Default spacing between reconciles of one desktop. */
export const RECONCILE_MIN_INTERVAL_MS = 30_000;

/**
 * Everything the phone does when a desktop's channel opens: write the push
 * context, rotate the push key if due, announce it to this desktop if it
 * hasn't heard it, then reconcile handled notifications. Returns the
 * unsubscribe. Errors are quiet (a notification left showing, or an
 * announcement retried on the next open).
 */
export function attachPushChannel(client: PushChannelClient, desktop: string, deps: PushChannelDeps): () => void {
  const now = deps.now ?? Date.now;
  const minInterval = deps.minIntervalMs ?? RECONCILE_MIN_INTERVAL_MS;
  let last = -Infinity;
  let running = false;
  let closed = false;
  const run = async (): Promise<void> => {
    if (running || closed) return;
    running = true;
    try {
      try {
        deps.sync.ensureContext();
        await deps.sync.maybeRotate();
        if (!closed) await deps.sync.announce(desktop, (key) => client.announcePushKey(key));
      } catch {
        // Retried on the next open.
      }
      if (closed || now() - last < minInterval) return;
      last = now();
      await reconcileDone(client.rpc, deps.platform, deps.dismiss);
    } finally {
      running = false;
    }
  };
  const stop = onChannelOpen(client, () => { void run(); });
  return () => {
    closed = true;
    stop();
  };
}
