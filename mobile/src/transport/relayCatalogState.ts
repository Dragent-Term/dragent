import { parseRelayCatalog, type RelayCatalog } from "./relayCatalog";
export type RelayCatalogState = {
  readonly status: "unavailable";
  readonly reason: string;
  readonly catalog: null;
} | {
  readonly status: "loading";
  readonly reason: null;
  readonly catalog: null;
} | {
  readonly status: "live";
  readonly reason: null;
  readonly catalog: RelayCatalog;
};
export const unavailableRelayCatalog = (reason: string): RelayCatalogState => Object.freeze({ status: "unavailable", reason, catalog: null });
export const loadingRelayCatalog: RelayCatalogState = Object.freeze({ status: "loading", reason: null, catalog: null });
export function initialRelayCatalogState(source: {
  gateway: string | null;
  deviceId: string | null;
}): RelayCatalogState {
  return source.gateway && source.deviceId ? loadingRelayCatalog : unavailableRelayCatalog("This machine has not shared its live catalog.");
}
export function parseRelayCatalogSnapshot(raw: unknown): RelayCatalog | null { return parseRelayCatalog(raw); }
