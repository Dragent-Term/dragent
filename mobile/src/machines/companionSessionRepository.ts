import type { KeyValueStore } from "../storage/types";
import { credentialKey, serializeCredentials } from "../storage/credentials";
import type { CompanionSessionLease } from "../transport/companionSession";
export type { CompanionSessionLease } from "../transport/companionSession";

export interface CompanionSessionOwner {
  readonly accountId: string;
  readonly gateway: string;
  readonly deviceId: string | null;
}

/** Storage is injected: native uses SecureStore; tests and web use memory. */
export function companionSessionRepository(
  storage: KeyValueStore,
  owner: CompanionSessionOwner,
) {
  const identity = JSON.stringify([owner.accountId, owner.gateway, owner.deviceId]);
  const key = credentialKey("dragent.companion-session.v1", identity);
  const valid = (value: unknown): value is CompanionSessionLease => {
    if (!value || typeof value !== "object") return false;
    const lease = value as Partial<CompanionSessionLease>;
    return typeof lease.sessionId === "string" && /^[a-zA-Z0-9-]{1,128}$/.test(lease.sessionId)
      && typeof lease.expiresAt === "number" && Number.isSafeInteger(lease.expiresAt)
      && lease.expiresAt > 0;
  };
  return {
    loadSession: () => serializeCredentials(storage, async (): Promise<CompanionSessionLease | null> => {
      const raw = await storage.getItem(key);
      if (raw === null) return null;
      let value: unknown;
      try { value = JSON.parse(raw); } catch { value = null; }
      if (valid(value)) return Object.freeze({ sessionId: value.sessionId, expiresAt: value.expiresAt });
      await storage.removeItem(key);
      return null;
    }),
    saveSession: (lease: CompanionSessionLease) => serializeCredentials(storage, async () => {
      if (!valid(lease)) throw new Error("Invalid companion session lease.");
      await storage.setItem(key, JSON.stringify({ sessionId: lease.sessionId, expiresAt: lease.expiresAt }));
    }),
    clearSession: () => serializeCredentials(storage, () => storage.removeItem(key)),
  };
}
