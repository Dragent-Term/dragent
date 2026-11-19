/**
 * What to show while a desktop's connection is being re-made.
 *
 * Coming back to the app replaces the connection, which takes a moment. The
 * screens used to drop everything for that moment and say the catalog was
 * unreachable. Instead the last catalog that arrived stays on screen while
 * the connection is in flight, for a bounded time; only a connection that
 * stays down, or a real failure, shows as unavailable.
 */
import { useEffect, useRef, useState } from "react";
import type { MachineCatalogState } from "../transport/machineCatalogState";

/** How long the last catalog stands in for a connection that isn't back yet. */
export const HOLD_CATALOG_MS = 20_000;

export type SocketPhase = "idle" | "connecting" | "reconnecting" | "ready" | "error";

/** The connection is on its way (back): not live, not failed for good. */
export function inFlight(phase: SocketPhase, current: MachineCatalogState): boolean {
  if (phase === "connecting" || phase === "reconnecting") return true;
  return phase === "ready" && current.status === "loading";
}

/** Pure choice: the current state, or the held one while the connection is in flight. */
export function chooseCatalog(
  current: MachineCatalogState,
  held: MachineCatalogState | null,
  phase: SocketPhase,
  expired: boolean,
): MachineCatalogState {
  if (current.status === "live" || held === null || expired) return current;
  return inFlight(phase, current) ? held : current;
}

/**
 * `current`, except that while `owner`'s connection is in flight the last
 * live catalog from the same owner is kept for up to `HOLD_CATALOG_MS`.
 */
export function useHeldCatalog(owner: unknown, phase: SocketPhase, current: MachineCatalogState): MachineCatalogState {
  const last = useRef<{ owner: unknown; state: MachineCatalogState } | null>(null);
  if (current.status === "live") last.current = { owner, state: current };
  const held = last.current !== null && last.current.owner === owner ? last.current.state : null;
  const waiting = current.status !== "live" && held !== null && inFlight(phase, current);
  const [expired, setExpired] = useState(false);
  useEffect(() => {
    if (!waiting) {
      setExpired(false);
      return;
    }
    const timer = setTimeout(() => setExpired(true), HOLD_CATALOG_MS);
    return () => clearTimeout(timer);
  }, [waiting]);
  return chooseCatalog(current, held, phase, expired);
}
