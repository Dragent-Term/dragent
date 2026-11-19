import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useForegroundRevival } from "../hooks/useForegroundRevival";
import { companionSessionRepository } from "../machines/companionSessionRepository";
import { secureStorage } from "../storage/secure";
import { registerCompanionConnection } from "../machines/companionConnections";
import { revokeCompanionSession } from "./companionSession";
import { RelaySocketClient, type RelaySocketOptions, type RelaySocketSnapshot } from "./RelaySocketClient";
import { socketE2ee } from "./e2ee/native";
import { attachPushSync } from "../notifications/pushSync";
import { createSocketPool } from "./sharedSocketPool";
export type { OwnedResult } from "./ownedResult";
export { selectOwnedState } from "./ownedResult";
export interface RelaySocketSource {
  gateway: string | null;
  /**
   * Target desktop id, or null for account discovery (machines.list).
   * Targeted machine sockets always supply one; the account discovery hook
   * owns a client with no device.
   */
  deviceId: string | null | undefined;
  /** Obtain this gateway from the paired desktop before supplying auth. */
  resolveAccessToken?: (() => Promise<string | null>) | null | undefined;
  createSocket?: RelaySocketOptions["createSocket"] | undefined;
  /**
   * Set false for account discovery sockets (machines.list) that intentionally
   * carry no target. Defaults to true: targeted machine sockets require one.
   */
  requireDevice?: boolean | undefined;
  /**
   * Pairing identity (paired user id) owning this socket. The controller's
   * getAccessToken can stay stable across pairings and paired can persist
   * across batched updates, so the pairing id forces a new resolver identity
   * and client per pairing — never showing the previous pairing's data
   * during the render before passive cleanup.
   */
  accountId?: string | null | undefined;
  /**
   * This phone's paired device id (the §5 prologue `phone`). Required for a
   * targeted desktop socket: without it nothing is sent to the desktop.
   */
  phoneDeviceId?: string | null | undefined;
  /** §4.6 step 4: typed input on these terminal handles didn't reach the desktop. */
  onInputLost?: ((handles: readonly number[], message: string) => void) | undefined;
}

const idle: RelaySocketSnapshot = Object.freeze({ status: "idle", error: null, loginRequired: false, connectionEpoch: 0, retryAttempt: 0 });
const noSubscribe = () => () => { };
const idleSnapshot = () => idle;
// Long enough to cover moving between screens; short enough that a
// desktop nobody is looking at stops holding a channel.
const sharedSockets = createSocketPool<RelaySocketClient>({ lingerMs: 15_000 });

function sharedSocketKey(gateway: string, deviceId: string, accountId: string, phoneDeviceId: string | null): string {
  return JSON.stringify([gateway, deviceId, accountId, phoneDeviceId]);
}

/** Own once at the machine boundary, then pass its rpc client to child screens. */
export function useRelaySocket(source: RelaySocketSource) {
  const { gateway, deviceId, resolveAccessToken, createSocket, requireDevice, accountId, phoneDeviceId, onInputLost } = source;
  const owner = useMemo(
    () => ({ gateway, deviceId, resolveAccessToken, createSocket, requireDevice, accountId, phoneDeviceId }),
    [gateway, deviceId, resolveAccessToken, createSocket, requireDevice, accountId, phoneDeviceId],
  );
  const [owned, setOwned] = useState<{
    owner: typeof owner;
    client: RelaySocketClient;
  } | null>(null);
  // Never expose the previous account/machine during the render before cleanup.
  const client = owned?.owner === owner ? owned.client : null;
  const state = useSyncExternalStore(client?.subscribe ?? noSubscribe, client?.getSnapshot ?? idleSnapshot, idleSnapshot);
  // Pairing discovery sockets omit the device; targeted desktop sockets
  // require one. The hook never exposes the previous pairing/desktop during
  // the render before cleanup (owner masking).
  const requiresDevice = source.requireDevice !== false;
  useEffect(() => {
    if (!gateway || !resolveAccessToken)
      return;
    const trimmed = typeof deviceId === "string" ? deviceId.trim() : "";
    if (requiresDevice && !trimmed)
      return;
    const build = (inputLost: RelaySocketSource["onInputLost"]) => {
      const current = new RelaySocketClient({
        gateway,
        ...(trimmed ? { deviceId: trimmed } : {}),
        resolveAccessToken,
        ...(accountId ? companionSessionRepository(secureStorage, {
          accountId, gateway, deviceId: trimmed || null,
        }) : {}),
        ...(createSocket ? { createSocket } : {}),
        // Every desktop socket is end-to-end encrypted; account discovery
        // sockets carry no desktop traffic at all.
        ...(trimmed ? { e2ee: socketE2ee(accountId, phoneDeviceId) } : {}),
        ...(inputLost ? { onInputLost: inputLost } : {}),
      });
      const unregister = accountId ? registerCompanionConnection(accountId, {
        revoke: () => revokeCompanionSession(current.rpc, { timeoutMs: 1800 }),
        close: current.close,
      }) : () => {};
      current.connect();
      // §8 / §6.6 (E9a): push context, push-key rotation and reconcile on open.
      const stopPush = trimmed ? attachPushSync(current, trimmed, accountId, phoneDeviceId) : () => {};
      return { client: current, teardown: () => { stopPush(); unregister(); current.close(); } };
    };
    // One socket per desktop, shared by everything showing it (see
    // sharedSocketPool).
    const key = trimmed && accountId && !createSocket
      ? sharedSocketKey(gateway, trimmed, accountId, phoneDeviceId ?? null)
      : null;
    if (key === null) {
      const own = build(onInputLost);
      setOwned({ owner, client: own.client });
      return own.teardown;
    }
    const release = sharedSockets.acquire(key, onInputLost, build);
    setOwned({ owner, client: release.client });
    return release.release;
    // onInputLost is read when the client is built: a new callback identity
    // must not tear down a live connection, so it isn't a dependency.
  }, [owner, gateway, deviceId, resolveAccessToken, createSocket, requiresDevice, accountId, phoneDeviceId]);
  useForegroundRevival(useCallback((reason, awayMs) => {
    if (reason === "network")
      client?.notifyNetworkChange();
    else
      client?.notifyForeground(awayMs);
  }, [client]));
  // Only a deliberate user retry can replace an ended logical session.
  // Foreground/network recovery above always resumes the existing lease.
  const reconnect = useCallback(() => {
    if (client?.getSnapshot().sessionStatus === "ended") client.startNewSession();
    else client?.connect();
  }, [client]);
  const trimmedDevice = typeof deviceId === "string" ? deviceId.trim() : "";
  const available = Boolean(gateway && resolveAccessToken && (requiresDevice ? trimmedDevice : true));
  return { client, state, reconnect, available };
}
