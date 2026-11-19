/**
 * App wiring for end-to-end encrypted push (docs/design/e2ee.md §8, §6.6):
 * every desktop socket calls `attachPushSync` (from `useRelaySocket`), which
 * runs `attachPushChannel` with the native module, AsyncStorage and
 * expo-notifications. The logic and its tests live in `e2eePush.ts`.
 */
import { Platform } from "react-native";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { getE2ee } from "../../modules/dragent-e2ee";
import { phoneSecurity } from "../transport/e2ee/native";
import { attachPushChannel, PushKeySync, type PushChannelClient } from "./e2eePush";

const syncs = new Map<string, PushKeySync>();

/** One PushKeySync per account+phone, so all desktops share its queue. */
function pushKeySync(account: string, phone: string): PushKeySync | null {
  const key = `${account}:${phone}`;
  const existing = syncs.get(key);
  if (existing) return existing;
  let sync: PushKeySync;
  try {
    const native = getE2ee();
    const security = phoneSecurity();
    sync = new PushKeySync({
      native,
      prefs: AsyncStorage,
      account,
      phone,
      pairedDesktops: () => security.pairedDesktops(),
    });
  } catch {
    // The web, or no native module: no push keys, nothing to do.
    return null;
  }
  syncs.set(key, sync);
  return sync;
}

/** Remove delivered notifications: iOS by APNs collapse id, Android by tag. */
async function dismiss(identifiers: readonly string[]): Promise<void> {
  const notifications = await import("expo-notifications");
  await Promise.all(identifiers.map((id) => notifications.dismissNotificationAsync(id).catch(() => undefined)));
}

/**
 * Push context, push-key rotation and reconcile for one desktop socket.
 * Returns the cleanup (call it with the socket's).
 */
export function attachPushSync(
  client: PushChannelClient,
  desktop: string,
  account: string | null | undefined,
  phone: string | null | undefined,
): () => void {
  if (!desktop || !account || !phone) return () => {};
  if (Platform.OS !== "ios" && Platform.OS !== "android") return () => {};
  const sync = pushKeySync(account, phone);
  if (!sync) return () => {};
  return attachPushChannel(client, desktop, { sync, platform: Platform.OS, dismiss });
}

/**
 * Write the push context (account and this phone) for the extension and the
 * messaging service as soon as the phone is paired, before any desktop
 * channel opens. Quiet on failure: pushes then show the placeholder.
 */
export function ensurePushContext(account: string | null | undefined, phone: string | null | undefined): void {
  if (!account || !phone) return;
  try {
    pushKeySync(account, phone)?.ensureContext();
  } catch {
    // Placeholder until the next try.
  }
}
