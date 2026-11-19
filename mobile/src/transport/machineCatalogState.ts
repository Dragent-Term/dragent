import type { MachineSummary, RelayCatalog } from "./relayCatalog";

/**
 * Platform-independent live catalog state. No React, no native imports:
 * pure domain/store tests can exercise the exact states the hooks publish,
 * without loading the socket lifecycle.
 *
 * Offline, error, empty and loading are distinct: `machine.online === false`
 * renders the offline state (device controls disabled) and never presents
 * stale data as live. Empty means a live machine that shares no projects
 * yet — never sample data.
 */
export type MachineCatalogState =
  | { readonly status: "unavailable"; readonly reason: string; readonly catalog: null; readonly machine: MachineSummary | null }
  | { readonly status: "loading"; readonly reason: null; readonly catalog: null; readonly machine: MachineSummary | null }
  | {
    readonly status: "live"; readonly reason: null; readonly catalog: RelayCatalog; readonly machine: MachineSummary;
    /** §9: a newer blob from the server didn't check out; this is the last good copy. */
    readonly warning?: string | null;
  };

export const unavailableMachineCatalog = (reason: string, machine: MachineSummary | null = null): MachineCatalogState =>
  Object.freeze({ status: "unavailable", reason, catalog: null, machine });
export const loadingMachineCatalog: MachineCatalogState = Object.freeze({
  status: "loading", reason: null, catalog: null, machine: null,
});
export function initialMachineCatalogState(source: {
  gateway: string | null;
  deviceId: string | null | undefined;
}): MachineCatalogState {
  return source.gateway && source.deviceId
    ? loadingMachineCatalog
    : unavailableMachineCatalog("This machine has not shared its live catalog.");
}
