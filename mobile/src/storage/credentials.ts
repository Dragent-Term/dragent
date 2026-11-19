import type { KeyValueStore } from "./types";

// Controllers can be replaced while native storage is still completing a call.
// Keep their credential transactions ordered by the underlying store instance.
const queues = new WeakMap<KeyValueStore, Promise<void>>();

export function serializeCredentials<T>(storage: KeyValueStore, action: () => Promise<T>): Promise<T> {
  const result = (queues.get(storage) ?? Promise.resolve()).then(action);
  queues.set(storage, result.then(() => {}, () => {}));
  return result;
}

/** Hex uses only native SecureStore's allowed key characters, without collisions. */
export function credentialKey(base: string, origin: string): string {
  let suffix = "";
  for (let i = 0; i < origin.length; i++) suffix += origin.charCodeAt(i).toString(16).padStart(4, "0");
  return `${base}.${suffix}`;
}

function belongsTo(raw: string | null, origin: string): boolean {
  if (!raw) return false;
  try {
    const value: unknown = JSON.parse(raw);
    return value !== null && typeof value === "object" && "portal" in value && value.portal === origin;
  } catch { return false; }
}

/** Read/remove helpers run inside a serialized credential transaction. */
export async function readCredential(storage: KeyValueStore, base: string, origin: string): Promise<string | null> {
  const current = await storage.getItem(credentialKey(base, origin));
  if (current !== null) return current;
  // Existing installs used one shared record containing its portal identity.
  const legacy = await storage.getItem(base);
  return belongsTo(legacy, origin) ? legacy : null;
}

export async function removeCredential(storage: KeyValueStore, base: string, origin: string): Promise<void> {
  await storage.removeItem(credentialKey(base, origin));
  if (belongsTo(await storage.getItem(base), origin)) await storage.removeItem(base);
}
