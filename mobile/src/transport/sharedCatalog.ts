/**
 * One `catalog` subscription per desktop socket, shared by every screen that
 * shows it (the desktop's row on the home screen and its own screens).
 *
 * A socket allows one subscription per stream name: a second screen
 * subscribing threw "This stream is already subscribed" and showed the
 * catalog as unreachable. And a screen re-subscribing right after leaving
 * (a new connection epoch) collided with its own unsubscribe still pending.
 * The hub fans each delivery out, replays the latest snapshot to a screen
 * that joins late, and lingers briefly after the last screen leaves so a
 * re-subscription in the same render rejoins it.
 */
import type { RelaySocketClient } from "./RelaySocketClient";
import { RelayRpcError } from "./relayRpcClient";
import {
  subscribeMachineCatalog,
  type MachineCatalogListener,
  type RelayCatalogSnapshot,
} from "./relayRpcMethods";

const CATALOG_STREAM = "catalog";

/** How long a catalog subscription outlives its last screen. */
const LINGER_MS = 1500;

interface Hub {
  listeners: Set<MachineCatalogListener>;
  subscription: { unsubscribe(): Promise<void> };
  last: RelayCatalogSnapshot | null;
  teardown: ReturnType<typeof setTimeout> | null;
}

const hubs = new WeakMap<RelaySocketClient, Hub>();

function close(client: RelaySocketClient, hub: Hub): void {
  if (hubs.get(client) === hub) hubs.delete(client);
  if (hub.teardown !== null) clearTimeout(hub.teardown);
  hub.teardown = null;
  void hub.subscription.unsubscribe().catch(() => {});
}

/** Subscribe `listener` to `client`'s catalog. Throws like `subscribeMachineCatalog`. */
export function subscribeSharedCatalog(
  client: RelaySocketClient,
  listener: MachineCatalogListener,
  options: { fresh?: boolean } = {},
): { unsubscribe(): Promise<void> } {
  let hub = hubs.get(client);
  // A retry must not rejoin a subscription that never delivered: start a
  // new one, and keep the screens already listening on it.
  const carried = options.fresh && hub ? [...hub.listeners] : [];
  if (options.fresh && hub) {
    const stale = hub;
    stale.listeners.clear();
    close(client, stale);
    // An unsubscribe waits for the snapshot that never came; end it now so
    // the stream name is free for the new subscription.
    client.rpc.failStream(CATALOG_STREAM, new RelayRpcError("Replaced by a new catalog subscription."));
    hub = undefined;
  }
  if (!hub) {
    const fresh: Hub = { listeners: new Set(), subscription: { unsubscribe: async () => {} }, last: null, teardown: null };
    fresh.subscription = subscribeMachineCatalog(
      client.rpc,
      {
        onSnapshot: (snapshot) => {
          fresh.last = snapshot;
          for (const l of [...fresh.listeners]) l.onSnapshot(snapshot);
        },
        onUpdate: (snapshot) => {
          fresh.last = snapshot;
          for (const l of [...fresh.listeners]) l.onUpdate(snapshot);
        },
        onRejected: (message) => {
          for (const l of [...fresh.listeners]) l.onRejected?.(message);
        },
        onError: (error) => {
          // The stream is over: the next screen starts a new one.
          fresh.last = null;
          close(client, fresh);
          for (const l of [...fresh.listeners]) l.onError?.(error);
        },
      },
      client.openCatalog,
    );
    for (const l of carried) fresh.listeners.add(l);
    hubs.set(client, fresh);
    hub = fresh;
  }
  const joined = hub;
  if (joined.teardown !== null) {
    clearTimeout(joined.teardown);
    joined.teardown = null;
  }
  joined.listeners.add(listener);
  if (joined.last) listener.onSnapshot(joined.last);
  let left = false;
  return {
    unsubscribe: async () => {
      if (left) return;
      left = true;
      joined.listeners.delete(listener);
      if (joined.listeners.size === 0 && joined.teardown === null && hubs.get(client) === joined) {
        joined.teardown = setTimeout(() => {
          joined.teardown = null;
          if (joined.listeners.size === 0) close(client, joined);
        }, LINGER_MS);
      }
    },
  };
}
