import { useHeldCatalog } from "../machines/heldCatalog";
import { useEffect, useState } from "react";
import type { RelayCatalogFetch } from "./relayCatalog";
import { subscribeMachineCatalog } from "./relayRpcMethods";
import {
  initialMachineCatalogState,
  loadingMachineCatalog,
  unavailableMachineCatalog,
  type MachineCatalogState,
} from "./machineCatalogState";
export { initialRelayCatalogState, parseRelayCatalogSnapshot } from "./relayCatalogState";
export type { RelayCatalogState } from "./relayCatalogState";
export {
  initialMachineCatalogState,
  loadingMachineCatalog,
  unavailableMachineCatalog,
  type MachineCatalogState,
} from "./machineCatalogState";
import { useRelaySocket, type RelaySocketSource } from "./useRelaySocket";
import { selectOwnedState } from "./ownedResult";
export interface RelayCatalogSource extends RelaySocketSource {
  /** Retained for source compatibility; catalog reads now use the relay socket. */
  readonly fetchFn?: RelayCatalogFetch | undefined;
}

const LIVE_UNREACHABLE = "The live machine catalog is unreachable right now.";

/**
 * Authenticated live catalog over the targeted machine socket. Subscribes to
 * the `catalog` stream (seq1 snapshot, then full replacement payloads on
 * changes). Preview data is never substituted for a failed live read. Stale
 * owner/generation results are masked: only the current client and
 * connection epoch can publish.
 */
export function useRelayCatalog(source: RelayCatalogSource): MachineCatalogState {
  const { client, state: socket, available } = useRelaySocket(source);
  const [result, setResult] = useState<{
    client: typeof client;
    epoch: number;
    state: MachineCatalogState;
  } | null>(null);
  useEffect(() => {
    if (!client || socket.status !== "ready")
      return;
    let active = true;
    const epoch = socket.connectionEpoch;
    setResult({ client, epoch, state: loadingMachineCatalog });
    const publish = (state: MachineCatalogState) => {
      if (active) setResult({ client, epoch, state });
    };
    let subscription: { unsubscribe(): Promise<void> } | null = null;
    let warning: string | null = null;
    try {
      subscription = subscribeMachineCatalog(client.rpc, {
        // A warning belongs to the one delivery that follows a rejected blob.
        onSnapshot: (snapshot) => {
          publish(Object.freeze({ status: "live", reason: null, catalog: snapshot.catalog, machine: snapshot.machine, warning }));
          warning = null;
        },
        onUpdate: (snapshot) => {
          publish(Object.freeze({ status: "live", reason: null, catalog: snapshot.catalog, machine: snapshot.machine, warning }));
          warning = null;
        },
        // The last good copy (if any) is re-delivered right after this.
        onRejected: (message) => { warning = message; },
        // Subscription failure (initial or midstream) leaves loading at
        // once — and drops any stale live state — instead of waiting out
        // the snapshot deadline.
        onError: (error) => publish(unavailableMachineCatalog(
          error.loginRequired ? "Pair this phone to view this desktop's live catalog." : error.message || LIVE_UNREACHABLE,
        )),
      }, client.openCatalog);
    } catch {
      publish(unavailableMachineCatalog(LIVE_UNREACHABLE));
      return () => { active = false; };
    }
    // A snapshot that never arrives (server without catalog support) must
    // not hang the UI in loading forever.
    const deadline = setTimeout(() => {
      if (!active) return;
      setResult((prev) => {
        if (prev?.client !== client || prev.epoch !== epoch) return prev;
        if (prev.state.status !== "loading") return prev;
        return { client, epoch, state: unavailableMachineCatalog(LIVE_UNREACHABLE) };
      });
    }, 15000);
    return () => {
      active = false;
      clearTimeout(deadline);
      const sub = subscription;
      subscription = null;
      if (sub) void sub.unsubscribe().catch(() => {});
    };
  }, [client, socket.status, socket.connectionEpoch]);
  const current = ((): MachineCatalogState => {
    if (!source.gateway || !source.deviceId)
      return initialMachineCatalogState(source);
    if (!available)
      return unavailableMachineCatalog("Pair this phone to view this desktop's live catalog.");
    if (socket.status === "error" || socket.status === "reconnecting")
      return unavailableMachineCatalog(socket.error ?? LIVE_UNREACHABLE);
    // Masked by client identity AND epoch: a new account/device owns a new
    // client whose epoch restarts, so the previous generation never renders.
    // Callers must build `source` per account (accountId + fresh resolver);
    // see createMachineSocketSource in machines/socketSource.
    const owned = selectOwnedState(result, client, socket.connectionEpoch);
    if (socket.status !== "ready" || !owned) return loadingMachineCatalog;
    return owned;
  })();
  // The last catalog stays up while the connection is re-made.
  return useHeldCatalog(client, socket.status, current);
}
