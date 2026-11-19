/**
 * The phone transport's security state (docs/design/e2ee.md §6.1, §6.2 step 8,
 * §6.4, §6.5): desktop pins, catalog verification with persisted high-water
 * marks, and the atomic phone-side unpair. One instance per app process.
 *
 * Every private key and every check lives in the native module; this file
 * only sequences calls under the transport's state lock and keeps the
 * process-lifetime "last good" catalog copies (§9 "Showing the last good
 * copy").
 *
 * The lock: JS runs one task at a time and native calls are synchronous, so a
 * critical section is any run of code with no `await` inside. `locked()`
 * additionally refuses re-entry, so a listener fired from inside a critical
 * section can't interleave another unpair or pin change.
 */
import type { NativeE2ee, PinStore } from "../../../modules/dragent-e2ee/src/types";
import { e2eeErrorCode } from "../../../modules/dragent-e2ee/src/api";
import { utf8Decode } from "./bytes";

export type PinStatus =
  | { readonly state: "pinned"; readonly key: Uint8Array; readonly name: string }
  | { readonly state: "unpinned" }
  | { readonly state: "tombstoned" }
  /** §6.1/§9: the pin store failed its check. Fail closed: pair again. */
  | { readonly state: "lost" };

export type CatalogCheck =
  | { readonly status: "accepted"; readonly version: number; readonly json: unknown }
  /** Same version and body as the last accepted blob: keep the last good copy. */
  | { readonly status: "unchanged"; readonly version: number; readonly json: unknown | null }
  /** §6.5: older than the high-water mark, or the same version with a different body. */
  | { readonly status: "rollback" | "equivocation"; readonly lastGood: unknown | null }
  /** Not from the pinned desktop, not for this phone, or malformed. */
  | { readonly status: "rejected"; readonly lastGood: unknown | null }
  | { readonly status: "unpaired" }
  | { readonly status: "lost" };

/** Anything that holds live channels to a desktop (an E2EE desktop link). */
export interface DesktopChannelOwner {
  readonly desktop: string;
  /** §6.4 step 2: close and zeroise everything for this desktop, now. */
  revoke(): void;
}

export interface UnpairOptions {
  /** §6.4 step 6: the server call, availability only. Errors are ignored. */
  readonly deleteOnServer?: (() => Promise<void>) | undefined;
  /** §6.4 step 5: forget the device token when the last desktop goes. */
  readonly forgetDeviceToken?: (() => Promise<void>) | undefined;
  readonly now?: number | undefined;
}

export interface UnpairResult {
  /** Whether a pin was removed. */
  readonly wasPinned: boolean;
  /** True when this was the last desktop and every key was deleted. */
  readonly keysDeleted: boolean;
}

export const STORE_LOST_MESSAGE =
  "Dragent's security records on this device couldn't be verified, so paired devices were reset. Pair again.";

export class PhoneSecurity {
  private store: PinStore | null = null;
  private lost = false;
  private busy = false;
  private readonly lastGood = new Map<string, { version: number; json: unknown }>();
  private readonly owners = new Set<DesktopChannelOwner>();
  private readonly listeners = new Set<(desktop: string) => void>();

  constructor(readonly native: NativeE2ee, private readonly clock: () => number = Date.now) {}

  /** Run `fn` under the transport state lock (no re-entry). */
  locked<T>(fn: () => T): T {
    if (this.busy) throw new Error("E2EE state is busy");
    this.busy = true;
    try {
      return fn();
    } finally {
      this.busy = false;
    }
  }

  /** The opened pin store, or null when it failed its check (fail closed). */
  private pins(): PinStore | null {
    if (this.lost) return null;
    if (this.store) return this.store;
    try {
      this.store = this.native.openPinStore();
      return this.store;
    } catch (error) {
      if (e2eeErrorCode(error) === "E2EE_STORE_LOST") this.lost = true;
      return null;
    }
  }

  private commit(store: PinStore): void {
    try {
      store.commit();
    } catch (error) {
      if (e2eeErrorCode(error) === "E2EE_STORE_LOST") this.lost = true;
      throw error;
    }
  }

  /** §9: true once the pin store failed its check; every desktop must pair again. */
  isLost(): boolean {
    this.pins();
    return this.lost;
  }

  /** What this phone knows about `desktop`. */
  pin(desktop: string): PinStatus {
    const store = this.pins();
    if (!store) return { state: "lost" };
    const s = store.lookup(desktop);
    if (s.state !== "pinned") return s;
    const name = store.desktops().find((d) => d.id === desktop)?.name ?? "";
    return { state: "pinned", key: s.identityKey, name };
  }

  /** The desktop's name from pairing (`pair-done`), or null. */
  desktopName(desktop: string): string | null {
    const p = this.pin(desktop);
    return p.state === "pinned" && p.name ? p.name : null;
  }

  /** Every paired desktop id. */
  pairedDesktops(): string[] {
    const store = this.pins();
    return store ? store.desktops().map((d) => d.id) : [];
  }

  /**
   * §6.2 step 8, phone side: commit the pin when `pair-done` arrives. The key
   * is the one from the QR (never from the server). The catalog high-water is
   * floored at `catalogVersion − 1`, so no older blob (even one still wrapped
   * to this phone from an earlier pairing) is ever accepted (L6). The push pin
   * lets the notification extension recognise this desktop (§8).
   */
  commitPairing(input: { desktop: string; identityKey: Uint8Array; desktopName: string; catalogVersion: number }): void {
    this.locked(() => {
      const store = this.pins();
      if (!store) throw new Error(STORE_LOST_MESSAGE);
      store.pinDesktop(input.desktop, input.identityKey, input.desktopName, input.catalogVersion, this.clock());
      this.commit(store);
      this.native.setPushPin(input.desktop, input.identityKey);
      this.lastGood.delete(input.desktop);
    });
    for (const l of this.listeners) l(input.desktop);
  }

  /**
   * §6.5: verify a sealed catalog blob from the server against the pin and the
   * persisted high-water mark, then open it. Nothing from an unverified blob
   * is returned; a rejected blob leaves the last good copy in place.
   */
  openCatalog(ids: { account: string; phone: string; desktop: string }, blob: Uint8Array): CatalogCheck {
    return this.locked((): CatalogCheck => {
      const store = this.pins();
      if (!store) return { status: "lost" };
      if (store.lookup(ids.desktop).state !== "pinned") return { status: "unpaired" };
      const last = this.lastGood.get(ids.desktop) ?? null;
      let result;
      try {
        result = store.openCatalog(ids.account, ids.phone, ids.desktop, blob);
      } catch (error) {
        const code = e2eeErrorCode(error);
        if (code === "E2EE_CATALOG_ROLLBACK") return { status: "rollback", lastGood: last?.json ?? null };
        if (code === "E2EE_CATALOG_EQUIVOCATION") return { status: "equivocation", lastGood: last?.json ?? null };
        if (code === "E2EE_STORE_LOST") {
          this.lost = true;
          return { status: "lost" };
        }
        return { status: "rejected", lastGood: last?.json ?? null };
      }
      if (result.status === "unchanged") {
        return { status: "unchanged", version: result.version, json: last && last.version === result.version ? last.json : null };
      }
      const text = utf8Decode(result.json);
      let json: unknown;
      try {
        json = text === null ? undefined : JSON.parse(text);
      } catch {
        json = undefined;
      }
      // Commit the advanced high-water mark before anything is shown (§6.5:
      // "the high-water mark is persisted").
      this.commit(store);
      if (json === undefined) return { status: "rejected", lastGood: last?.json ?? null };
      this.lastGood.set(ids.desktop, { version: result.version, json });
      return { status: "accepted", version: result.version, json };
    });
  }

  /** The persisted catalog high-water mark for `desktop` (tests, diagnostics). */
  catalogHighWater(desktop: string): number | null {
    const store = this.pins();
    return store ? store.catalogHighWater(desktop) : null;
  }

  /** Register a live channel owner so unpair can close it (§6.4 step 2). */
  register(owner: DesktopChannelOwner): () => void {
    this.owners.add(owner);
    return () => { this.owners.delete(owner); };
  }

  /** Notified after a desktop is paired or unpaired. */
  subscribe(listener: (desktop: string) => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  /**
   * §6.4 "A phone unpairs a desktop": local, total, immediate. Steps 1–5 run
   * under the state lock with no await; only the server call (availability
   * only) runs afterwards.
   */
  async unpairDesktop(desktop: string, options: UnpairOptions = {}): Promise<UnpairResult> {
    const now = options.now ?? this.clock();
    const result = this.locked((): UnpairResult => {
      const store = this.pins();
      // 1. Remove the pin and write a tombstone (persisted before anything else).
      let wasPinned = false;
      if (store) {
        wasPinned = store.revoke(desktop, now);
        this.commit(store);
      }
      // 2. Close and zeroise every channel, abort handshakes, fail pending
      //    requests, drop subscriptions/stream state, discard queued input.
      for (const owner of Array.from(this.owners)) if (owner.desktop === desktop) owner.revoke();
      // 3. Its notifications now fail the sender check (generic text only).
      this.native.removePushPin(desktop);
      // 4. The cached catalog and its high-water mark (revoke dropped the mark).
      this.lastGood.delete(desktop);
      // 5. The last desktop: every key goes (identity, push, pin store).
      const remaining = store ? store.desktops().length : 0;
      let keysDeleted = false;
      if (remaining === 0) {
        this.native.deleteAllKeys();
        this.store = null;
        this.lastGood.clear();
        keysDeleted = true;
      }
      return { wasPinned, keysDeleted };
    });
    for (const l of this.listeners) l(desktop);
    if (result.keysDeleted && options.forgetDeviceToken) {
      try { await options.forgetDeviceToken(); } catch { /* availability only */ }
    }
    // 6. Then the server: availability only.
    if (options.deleteOnServer) {
      try { await options.deleteOnServer(); } catch { /* availability only */ }
    }
    return result;
  }
}
