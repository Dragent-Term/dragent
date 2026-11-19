/**
 * The last tab the person had open, so home can take them straight back to
 * it. Kept on the phone only (the keychain-backed store), never sent anywhere.
 */
import { useEffect, useState } from "react";
import { secureStorage } from "../storage/secure";
import type { KeyValueStore } from "../storage/types";

export interface ResumePoint {
  readonly deviceId: string;
  readonly tabId: number;
  readonly title: string;
  /** "Project · workspace" when known. */
  readonly context: string;
  readonly desktopName: string;
  readonly at: number;
  /** What the tab runs: an agent's key ("claude", "dragent"…), "terminal" or "browser". */
  readonly mark?: string;
}

const KEY = "dragent.resume.v1";
const listeners = new Set<(point: ResumePoint | null) => void>();
let cached: ResumePoint | null | undefined;

function parse(raw: string | null): ResumePoint | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as Partial<ResumePoint>;
    if (typeof value.deviceId !== "string" || typeof value.tabId !== "number" || !Number.isSafeInteger(value.tabId)) return null;
    return {
      deviceId: value.deviceId,
      tabId: value.tabId,
      title: typeof value.title === "string" ? value.title : "Tab",
      context: typeof value.context === "string" ? value.context : "",
      desktopName: typeof value.desktopName === "string" ? value.desktopName : "",
      at: typeof value.at === "number" ? value.at : 0,
      ...(typeof value.mark === "string" && value.mark ? { mark: value.mark } : {}),
    };
  } catch {
    return null;
  }
}

/** Remember `point` as the place to resume. */
export async function rememberResume(point: ResumePoint, store: KeyValueStore = secureStorage): Promise<void> {
  cached = point;
  for (const listener of listeners) listener(point);
  try {
    await store.setItem(KEY, JSON.stringify(point));
  } catch {
    /* Best effort: resuming is a convenience. */
  }
}

/** Forget it (the desktop was unpaired, or the tab is gone). */
export async function forgetResume(store: KeyValueStore = secureStorage): Promise<void> {
  cached = null;
  for (const listener of listeners) listener(null);
  try {
    await store.removeItem(KEY);
  } catch {
    /* Best effort. */
  }
}

export async function loadResume(store: KeyValueStore = secureStorage): Promise<ResumePoint | null> {
  if (cached !== undefined) return cached;
  try {
    cached = parse(await store.getItem(KEY));
  } catch {
    cached = null;
  }
  return cached;
}

/** The current resume point, kept up to date. */
export function useResumePoint(): ResumePoint | null {
  const [point, setPoint] = useState<ResumePoint | null>(cached ?? null);
  useEffect(() => {
    let active = true;
    void loadResume().then((loaded) => {
      if (active) setPoint(loaded);
    });
    listeners.add(setPoint);
    return () => {
      active = false;
      listeners.delete(setPoint);
    };
  }, []);
  return point;
}

/** "just now", "5m ago", "2h ago", "3d ago". */
export function ageLabel(at: number, now = Date.now()): string {
  const minutes = Math.floor((now - at) / 60_000);
  if (!Number.isFinite(minutes) || minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}
