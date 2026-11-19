/** The connection log store: app-wide, saved on this phone only. */
import { useSyncExternalStore } from "react";
import AsyncStorage from "@react-native-async-storage/async-storage";
import { addLog, readLog, type LogEntry, type LogLevel } from "./connectionLogModel";

const KEY = "dragent.connection-log.v1";
let entries: readonly LogEntry[] = [];
let loaded = false;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<() => void>();

function publish(next: readonly LogEntry[]) {
  entries = Object.freeze([...next]);
  for (const listener of listeners) listener();
  // Saved a moment later, so a burst of events is one write.
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    void AsyncStorage.setItem(KEY, JSON.stringify(entries)).catch(() => {});
  }, 1000);
}

export async function loadConnectionLog(): Promise<void> {
  if (loaded) return;
  loaded = true;
  const saved = readLog(await AsyncStorage.getItem(KEY).catch(() => null));
  // What was logged before the saved copy loaded stays on top.
  publish([...entries, ...saved].slice(0, 300));
}

export function logConnection(level: LogLevel, desktop: string | null, text: string): void {
  publish(addLog(entries, { level, desktop, text, at: Date.now() }));
}

export function clearConnectionLog(): void {
  publish([]);
}

export function useConnectionLog(): readonly LogEntry[] {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => entries,
    () => entries,
  );
}
