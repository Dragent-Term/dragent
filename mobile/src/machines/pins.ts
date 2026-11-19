/**
 * Pinned workspaces: the ones the person keeps at the top of a desktop's
 * list. Kept on this phone only, per desktop, by workspace key; never sent
 * anywhere. A pin for a workspace that no longer exists is simply never shown.
 */
import { useEffect, useState } from "react";
import { secureStorage } from "../storage/secure";
import type { KeyValueStore } from "../storage/types";
import { parsePins, withPin, type Pins } from "./pinsModel";

const KEY = "dragent.pins.v1";

const listeners = new Set<(pins: Pins) => void>();
let cached: Pins | undefined;
let loading: Promise<Pins> | null = null;

async function load(store: KeyValueStore): Promise<Pins> {
  if (cached) return cached;
  loading ??= store
    .getItem(KEY)
    .then(parsePins, () => ({}))
    .then((pins) => {
      cached ??= pins;
      loading = null;
      return cached;
    });
  return loading;
}

/** Pin or unpin a workspace; every screen showing the desktop updates. */
export async function setPinned(
  device: string,
  workspace: string,
  pinned: boolean,
  store: KeyValueStore = secureStorage,
): Promise<void> {
  const next = withPin(await load(store), device, workspace, pinned);
  cached = next;
  for (const listener of listeners) listener(next);
  try {
    await store.setItem(KEY, JSON.stringify(next));
  } catch {
    /* Best effort: a pin is a convenience. */
  }
}

/** Forget a desktop's pins (it was unpaired). */
export async function forgetPins(device: string, store: KeyValueStore = secureStorage): Promise<void> {
  const pins = await load(store);
  if (!pins[device]) return;
  const next: Record<string, readonly string[]> = { ...pins };
  delete next[device];
  cached = next;
  for (const listener of listeners) listener(next);
  try {
    await store.setItem(KEY, JSON.stringify(next));
  } catch {
    /* Best effort. */
  }
}

/** The desktop's pinned workspace keys, oldest pin first. */
export function usePinnedWorkspaces(device: string): readonly string[] {
  const [pins, setPins] = useState<Pins>(cached ?? {});
  useEffect(() => {
    let live = true;
    listeners.add(setPins);
    void load(secureStorage).then((loaded) => {
      if (live) setPins(loaded);
    });
    return () => {
      live = false;
      listeners.delete(setPins);
    };
  }, []);
  return pins[device] ?? [];
}

/** Test seam: start from nothing. */
export function resetPinsForTests(): void {
  cached = undefined;
  loading = null;
  listeners.clear();
}
