/** The alert history store: shared app-wide, saved on this phone only. */
import { useSyncExternalStore } from "react";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { addEntry, readEntries, unreadCount, type NotificationEntry } from "./historyModel";

const KEY = "dragent.notifications.history.v1";
let entries: readonly NotificationEntry[] = [];
let loaded = false;
const listeners = new Set<() => void>();

function publish(next: readonly NotificationEntry[]) {
  entries = Object.freeze([...next]);
  for (const listener of listeners) listener();
  void AsyncStorage.setItem(KEY, JSON.stringify(entries)).catch(() => {});
}

/** Read what was saved (once); later records merge into it. */
export async function loadNotificationHistory(): Promise<void> {
  if (loaded) return;
  loaded = true;
  const saved = readEntries(await AsyncStorage.getItem(KEY).catch(() => null));
  let merged = saved;
  for (const entry of entries) merged = addEntry(merged, entry, entry.read);
  publish(merged);
}

export function recordNotification(
  input: { id?: string | null; title: string; body: string; desktop: string | null; at?: number },
  read = false,
): void {
  const at = input.at ?? Date.now();
  const id = input.id || `${at}:${input.title}:${input.body}`.slice(0, 300);
  publish(addEntry(entries, { id, title: input.title, body: input.body, desktop: input.desktop, at }, read));
}

export function markAllNotificationsRead(): void {
  if (entries.some((entry) => !entry.read)) publish(entries.map((entry) => ({ ...entry, read: true })));
}

export function clearNotificationHistory(): void {
  publish([]);
}

export function useNotificationHistory(): { entries: readonly NotificationEntry[]; unread: number } {
  const list = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => entries,
    () => entries,
  );
  return { entries: list, unread: unreadCount(list) };
}
