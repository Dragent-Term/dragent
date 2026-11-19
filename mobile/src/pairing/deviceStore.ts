import { credentialKey } from "../storage/credentials";
import type { KeyValueStore } from "../storage/types";

/**
 * This phone's pairing credentials. The device token is opaque 256-bit
 * server state: it lives in SecureStore only, never in ordinary prefs.
 * Access tokens stay in memory; the rotating device token is persisted.
 *
 * Transaction boundaries belong to the controller: every method below runs
 * inside the caller's serialized credential transaction, never its own.
 */
export interface DeviceCredential {
  readonly gateway: string;
  readonly deviceId: string;
  readonly deviceToken: string;
  readonly deviceName: string;
  readonly userId: string;
  readonly displayName: string;
  readonly pairedAt: number;
}

const DEVICE_KEY = "dragent.device.v1";

function valid(value: unknown): value is DeviceCredential {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<DeviceCredential>;
  return typeof record.gateway === "string" && !!record.gateway &&
    typeof record.deviceId === "string" && !!record.deviceId &&
    typeof record.deviceToken === "string" && !!record.deviceToken &&
    typeof record.deviceName === "string" &&
    typeof record.userId === "string" && !!record.userId &&
    typeof record.displayName === "string" &&
    typeof record.pairedAt === "number" && Number.isSafeInteger(record.pairedAt) && record.pairedAt > 0;
}

/**
 * Single active pairing, keyed by gateway so a second desktop never
 * overwrites the first silently — callers load/save per gateway inside a
 * serialized transaction.
 */
export function deviceCredentialStore(storage: KeyValueStore, gateway: string) {
  const key = credentialKey(DEVICE_KEY, gateway);
  return {
    load: async (): Promise<DeviceCredential | null> => {
      const raw = await storage.getItem(key);
      if (raw === null) return null;
      let value: unknown;
      try {
        value = JSON.parse(raw);
      } catch {
        value = null;
      }
      if (valid(value) && value.gateway === gateway) {
        return Object.freeze({ ...value });
      }
      await storage.removeItem(key);
      return null;
    },
    save: async (credential: DeviceCredential) => {
      if (!valid(credential) || credential.gateway !== gateway) {
        throw new Error("Invalid device credential.");
      }
      await storage.setItem(key, JSON.stringify({ ...credential }));
    },
    clear: () => storage.removeItem(key),
  };
}
