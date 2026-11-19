/**
 * Client-side handle registry: caps concurrent attached handles without
 * ever stealing another tab's handle. The server enforces max 4 per
 * companion; this registry mirrors that bound locally so a 5th attach
 * fails explicitly instead of evicting live state.
 */
import { COMPANION_TERMINAL_MAX_HANDLES } from "./types";

export interface CompanionHandleRegistry {
  readonly size: () => number;
  readonly has: (handle: string) => boolean;
  /** False when full or when the handle is empty/duplicate. */
  readonly tryAdd: (handle: string) => boolean;
  readonly remove: (handle: string) => void;
  readonly clear: () => void;
}

export function createCompanionHandleRegistry(limit: number = COMPANION_TERMINAL_MAX_HANDLES): CompanionHandleRegistry {
  const bounded = Number.isSafeInteger(limit) && limit > 0 ? limit : COMPANION_TERMINAL_MAX_HANDLES;
  const active = new Set<string>();
  return Object.freeze({
    size: () => active.size,
    has: (handle: string) => (typeof handle === "string" && handle !== "" ? active.has(handle) : false),
    tryAdd: (handle: string) => {
      if (typeof handle !== "string" || handle === "" || active.has(handle)) return false;
      if (active.size >= bounded) return false;
      active.add(handle);
      return true;
    },
    remove: (handle: string) => {
      if (typeof handle === "string" && handle !== "") active.delete(handle);
    },
    clear: () => {
      active.clear();
    },
  });
}

/** Shared registry for the live machine route (one hook per selected tab). */
export const sharedCompanionHandleRegistry: CompanionHandleRegistry = createCompanionHandleRegistry();
