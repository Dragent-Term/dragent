/**
 * Checks the public release file on start and when the app comes back to the
 * front (at most every 30 minutes), and shares the result app-wide.
 */
import { useEffect, useSyncExternalStore } from "react";
import { AppState, Linking, Platform } from "react-native";
import * as Application from "expo-application";
import { buildNumber, noticeFor, parseRelease, type VersionNotice } from "./notice";

/** Public, numbers-and-links only (see notice.ts). */
export const RELEASES_URL = "https://github.com/Dragent-Term/dragent/releases/download/mobile-latest/latest.json";
const MIN_GAP_MS = 30 * 60 * 1000;
const TIMEOUT_MS = 10_000;

export interface VersionState {
  readonly notice: VersionNotice;
  readonly checking: boolean;
  readonly checkedAt: number | null;
  readonly failed: boolean;
  /** "Later" on the card, for this run of the app. */
  readonly dismissed: boolean;
}

let state: VersionState = Object.freeze<VersionState>({ notice: { kind: "current" }, checking: false, checkedAt: null, failed: false, dismissed: false });
const listeners = new Set<() => void>();
let running: Promise<void> | null = null;

function set(next: Partial<VersionState>) {
  state = Object.freeze({ ...state, ...next });
  for (const listener of listeners) listener();
}

export function thisBuild(): number | null {
  return buildNumber(Application.nativeBuildVersion);
}

export function checkForNewVersion(force = false): Promise<void> {
  if (Platform.OS !== "ios" && Platform.OS !== "android") return Promise.resolve();
  if (running) return running;
  if (!force && state.checkedAt !== null && Date.now() - state.checkedAt < MIN_GAP_MS) return Promise.resolve();
  running = (async () => {
    set({ checking: true });
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), TIMEOUT_MS);
    try {
      const response = await fetch(RELEASES_URL, { signal: abort.signal, headers: { Accept: "application/json" }, cache: "no-store" });
      if (!response.ok) throw new Error(String(response.status));
      const release = parseRelease(await response.json(), Platform.OS as "ios" | "android");
      const notice = noticeFor(release, thisBuild());
      const changed = notice.kind !== state.notice.kind || ("release" in notice && "release" in state.notice && notice.release.latest !== state.notice.release.latest);
      set({ notice, failed: false, checkedAt: Date.now(), ...(changed ? { dismissed: false } : {}) });
    } catch {
      // Offline or the file is unreachable: keep what we knew.
      set({ failed: true, checkedAt: Date.now() });
    } finally {
      clearTimeout(timer);
      set({ checking: false });
      running = null;
    }
  })();
  return running;
}

export function dismissNewVersion(): void {
  set({ dismissed: true });
}

export async function openUpdate(url: string): Promise<boolean> {
  try {
    await Linking.openURL(url);
    return true;
  } catch {
    return false;
  }
}

export function useVersionState(): VersionState {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => state,
    () => state,
  );
}

/** Mounted once at the root. */
export function useVersionCheck(): void {
  useEffect(() => {
    void checkForNewVersion(true);
    const subscription = AppState.addEventListener("change", (next) => {
      if (next === "active") void checkForNewVersion();
    });
    return () => subscription.remove();
  }, []);
}
