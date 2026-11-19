import type { RelaySocketSnapshot } from "../transport/RelaySocketClient";
import type { MachineCatalogState } from "../transport/machineCatalogState";

/** The desktop is reachable: relay admitted, catalog live, encrypted channel open. */
export function connectionBack(socket: RelaySocketSnapshot, catalog: MachineCatalogState): boolean {
  if (socket.status !== "ready" || catalog.status !== "live") return false;
  const secure = socket.secure;
  return secure === null || secure === undefined || secure.state === "open";
}
