/**
 * Remote push opt-in controller. Pure logic (no React Native imports):
 * permission is requested ONLY inside `enable()`, renewal/rotation never
 * invent local completion alerts, and unmount cleanup never unregisters.
 *
 * - Opt-in preference is pairing+gateway scoped (`prefs`, ordinary storage).
 *   The native push token lives in memory only and is never persisted.
 * - Pairing/gateway are captured per call (generation mask): stale async
 *   results can never enable or report for the wrong pairing.
 * - Forget drain uses `revoke()` (best-effort unregister) + `close()`
 *   (timers only) via the existing companion-connections registry.
 */

import type { KeyValueStore } from "../storage/types";
import { serializeCredentials } from "../storage/credentials";
import { PushHttpError, registerPush, unregisterPush, type PushFetchFn, type PushPlatform } from "./pushApi";

export type PushOptStatus = "disabled" | "enabling" | "enabled" | "error" | "unavailable";

export interface PushOptSnapshot {
  readonly status: PushOptStatus;
  /** User-safe reason when disabled by the platform, or the last error. */
  readonly reason: string | null;
  /** True while an enable/disable round-trip is in flight. */
  readonly pending: boolean;
}

export interface PushNativeDriver {
  readonly available: boolean;
  readonly unavailableReason: string | null;
  readonly platform: PushPlatform;
  requestPermission(): Promise<boolean>;
  getDevicePushToken(): Promise<string>;
  ensureAndroidChannel?(): Promise<void>;
  onTokenRotation?(listener: () => void): () => void;
}

export interface PushControllerDeps {
  readonly prefs: KeyValueStore;
  readonly getAccountId: () => string | null;
  readonly getGateway: () => string | null;
  readonly getAccessToken: () => Promise<string | null>;
  readonly driver: PushNativeDriver;
  readonly fetchFn: PushFetchFn;
  readonly now?: () => number;
  /** Foreground renewal cadence; default 10 minutes (must stay ≤ 15 min). */
  readonly renewalMs?: number;
  readonly isForeground?: () => boolean;
}

export const PUSH_RENEWAL_MS = 10 * 60_000;
const OPTIN_PREFIX = "dragent.push.optin.v1";

function optinKey(accountId: string, gateway: string): string {
  return `${OPTIN_PREFIX}:${accountId}:${gateway}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parsePushOptin(raw: string | null): boolean {
  if (!raw) return false;
  try {
    const value: unknown = JSON.parse(raw);
    return isRecord(value) && value.optin === true;
  } catch {
    return raw.trim() === "1";
  }
}

export class PushController {
  private readonly deps: PushControllerDeps;
  private readonly listeners = new Set<() => void>();
  private snapshot: PushOptSnapshot = Object.freeze({ status: "disabled", reason: null, pending: false });
  private generation = 0;
  private lastToken: string | null = null;
  private lastRegisteredAt = 0;
  private rotationDetach: (() => void) | null = null;
  private closed = false;
  private renewing = false;
  private tokenDirty = true;

  constructor(deps: PushControllerDeps) {
    this.deps = deps;
  }

  getSnapshot = (): PushOptSnapshot => this.snapshot;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private publish(next: PushOptSnapshot): void {
    if (this.closed) return;
    this.snapshot = Object.freeze(next);
    for (const listener of this.listeners) listener();
  }

  private scope(): { accountId: string; gateway: string } | null {
    if (this.closed) return null;
    const accountId = this.deps.getAccountId();
    const gateway = this.deps.getGateway();
    if (!accountId || !gateway) return null;
    return { accountId, gateway };
  }

  private current(generation: number, key: string): boolean {
    return !this.closed && this.generation === generation && this.scopeKey() === key;
  }

  private accessFor(generation: number, key: string): () => Promise<string | null> {
    return async () => {
      if (!this.current(generation, key)) return null;
      const token = await this.deps.getAccessToken();
      return this.current(generation, key) ? token : null;
    };
  }

  private attachRotation(): void {
    if (this.closed || this.rotationDetach || !this.deps.driver.onTokenRotation) return;
    this.rotationDetach = this.deps.driver.onTokenRotation(() => {
      this.tokenDirty = true;
      void this.handleForeground();
    });
  }

  /** Load persisted opt-in (flag only) without touching native or network. */
  async restore(): Promise<void> {
    if (!this.deps.driver.available) {
      this.publish({ status: "unavailable", reason: this.deps.driver.unavailableReason ?? "Push notifications are unavailable on this device.", pending: false });
      return;
    }
    const scoped = this.scope();
    if (!scoped) {
      this.publish({ status: "disabled", reason: null, pending: false });
      return;
    }
    let opted = false;
    try {
      opted = parsePushOptin(await this.deps.prefs.getItem(optinKey(scoped.accountId, scoped.gateway)));
    } catch {
      opted = false;
    }
    if (!opted) {
      this.publish({ status: "disabled", reason: null, pending: false });
      return;
    }
    if (!this.deps.driver.available) {
      this.publish({ status: "unavailable", reason: this.deps.driver.unavailableReason ?? "Push notifications are unavailable on this device.", pending: false });
      return;
    }
    // Opted in before: report enabled; the next foreground tick renews the
    // lease and re-reads the native token (rotation covered without storage).
    this.lastRegisteredAt = 0;
    this.publish({ status: "enabled", reason: null, pending: false });
    this.attachRotation();
  }

  /** Explicit Enable button ONLY: the one place permission is requested. */
  async enable(): Promise<void> {
    if (this.closed) return;
    const scoped = this.scope();
    if (!scoped) {
      this.publish({ status: "error", reason: "Pair this phone with your desktop first.", pending: false });
      return;
    }
    const generation = ++this.generation;
    const key = this.scopeKey();
    if (!this.deps.driver.available) {
      this.publish({ status: "unavailable", reason: this.deps.driver.unavailableReason ?? "Push notifications are unavailable on this device.", pending: false });
      return;
    }
    this.publish({ status: "enabling", reason: null, pending: true });
    try {
      const granted = await this.deps.driver.requestPermission();
      if (!this.current(generation, key)) return;
      if (!granted) throw new Error("Allow notifications to enable push. No permission was requested elsewhere.");
      if (this.deps.driver.ensureAndroidChannel) await this.deps.driver.ensureAndroidChannel();
      if (!this.current(generation, key)) return;
      const pushToken = await this.deps.driver.getDevicePushToken();
      if (!this.current(generation, key)) return;
      if (!pushToken.trim()) throw new Error("The device token is missing.");
      await registerPush(this.deps.fetchFn, scoped.gateway, this.accessFor(generation, key), {
        platform: this.deps.driver.platform,
        pushToken,
        foreground: true,
      });
      if (!this.current(generation, key)) return;
      try {
        await serializeCredentials(this.deps.prefs, () => this.deps.prefs.setItem(optinKey(scoped.accountId, scoped.gateway), JSON.stringify({ optin: true })));
      } catch {
        // Persist best-effort; the lease itself is server-side.
      }
      this.lastToken = pushToken;
      this.lastRegisteredAt = (this.deps.now ?? Date.now)();
      if (!this.current(generation, key)) return;
      this.tokenDirty = false;
      this.publish({ status: "enabled", reason: null, pending: false });
      this.attachRotation();
    } catch (error) {
      if (this.generation !== generation) return;
      const message = error instanceof PushHttpError && error.loginRequired
        ? "Pair this phone to manage notifications."
        : error instanceof Error
          ? error.message || "Push could not be enabled. Try again."
          : "Push could not be enabled. Try again.";
      this.publish({ status: "error", reason: message, pending: false });
    }
  }

  /** Explicit Disable button or sign-out revoke: unregister best-effort. */
  async disable(): Promise<void> {
    const scoped = this.scope();
    const generation = ++this.generation;
    const key = this.scopeKey();
    this.detachRotation();
    this.publish({ status: this.snapshot.status === "enabled" ? "enabled" : this.snapshot.status, reason: this.snapshot.reason, pending: true });
    try {
      if (scoped) {
        try {
          await serializeCredentials(this.deps.prefs, () => this.deps.prefs.removeItem(optinKey(scoped.accountId, scoped.gateway)));
        } catch {
          // Best-effort: local state always clears.
        }
        try {
          await unregisterPush(this.deps.fetchFn, scoped.gateway, this.accessFor(generation, key));
        } catch {
          // Best-effort.
        }
      }
      if (!this.current(generation, key)) return;
      this.lastToken = null;
      this.lastRegisteredAt = 0;
      if (!this.deps.driver.available) {
        this.publish({ status: "unavailable", reason: this.deps.driver.unavailableReason ?? "Push notifications are unavailable on this device.", pending: false });
        return;
      }
      this.publish({ status: "disabled", reason: null, pending: false });
    } catch {
      this.lastToken = null;
      this.publish({ status: "disabled", reason: null, pending: false });
    }
  }

  /** Sign-out drain: revoke server lease quickly, then drop local state. */
  async revoke(): Promise<void> {
    const generation = ++this.generation;
    const key = this.scopeKey();
    try {
      const scoped = this.scope();
      if (scoped) {
        try {
          await serializeCredentials(this.deps.prefs, () => this.deps.prefs.removeItem(optinKey(scoped.accountId, scoped.gateway)));
        } catch {
          // Close even if revocation fails (drain has its own timeout).
        }
        try {
          await unregisterPush(this.deps.fetchFn, scoped.gateway, this.accessFor(generation, key));
        } catch {
          // Best-effort.
        }
      }
    } finally {
      this.lastToken = null;
      this.lastRegisteredAt = 0;
      this.detachRotation();
    }
  }

  /** UI unmount / account switch: timers and listeners only, never network. */
  close(): void {
    this.closed = true;
    this.generation++;
    this.detachRotation();
    this.listeners.clear();
  }

  /** Detach rotation listener without network (called from close/revoke). */
  detachRotation(): void {
    if (this.rotationDetach) {
      try {
        this.rotationDetach();
      } catch {
        // Listener cleanup never throws.
      }
      this.rotationDetach = null;
    }
  }

  /** Foreground tick: renew the lease at most every renewal window. */
  async handleForeground(): Promise<void> {
    if (this.closed || this.renewing || this.snapshot.status !== "enabled" || this.deps.isForeground?.() === false) return;
    const scoped = this.scope();
    if (!scoped) return;
    const now = (this.deps.now ?? Date.now)();
    const renewalMs = this.deps.renewalMs ?? PUSH_RENEWAL_MS;
    if (!this.tokenDirty && now - this.lastRegisteredAt < renewalMs) return;
    this.renewing = true;
    const generation = ++this.generation;
    const key = this.scopeKey();
    try {
      const pushToken = await this.deps.driver.getDevicePushToken();
      if (!this.current(generation, key)) return;
      if (!pushToken.trim()) return;
      const rotated = this.lastToken !== null && this.lastToken !== pushToken;
      await registerPush(this.deps.fetchFn, scoped.gateway, this.accessFor(generation, key), {
        platform: this.deps.driver.platform,
        pushToken,
        foreground: true,
      });
      if (!this.current(generation, key)) return;
      void rotated;
      this.lastToken = pushToken;
      this.lastRegisteredAt = (this.deps.now ?? Date.now)();
      this.tokenDirty = false;
    } catch {
      // Renewal is best-effort; the persisted opt-in stays so the next tick
      // retries. Never surface renewal noise as a blocking error here.
    } finally { this.renewing = false; }
  }

  private scopeKey(): string {
    return JSON.stringify([this.deps.getAccountId(), this.deps.getGateway()]);
  }
}
