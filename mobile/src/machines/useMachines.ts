import { useCallback, useEffect, useMemo, useState } from "react";
import { useDevice } from "../pairing/DeviceContext";
import { useForegroundRevival } from "../hooks/useForegroundRevival";
import { listMachines } from "../transport/relayRpcMethods";
import { RelayRpcError } from "../transport/relayRpcClient";
import type { MachineSummary } from "../transport/relayCatalog";
import { selectOwnedState, useRelaySocket } from "../transport/useRelaySocket";
import { createMachineSocketSource } from "./socketSource";

export type MachinesState =
  | { readonly status: "unavailable"; readonly reason: string; readonly machines: null }
  | { readonly status: "loading"; readonly reason: null; readonly machines: null }
  | {
      readonly status: "live";
      readonly reason: null;
      readonly machines: readonly MachineSummary[];
    };

export const MACHINES_POLL_MS = 15000;

const SIGN_IN_MESSAGE = "Pair this phone to view your desktops.";
const UNREACHABLE_MESSAGE = "Your desktops are unreachable right now.";

/**
 * Pairing-scoped desktop discovery over a socket with no target device:
 * `machines.list` with foreground refresh and polling. Stale
 * owner/generation results are masked — unpairing or re-pairing clears the
 * list instead of showing the previous pairing's desktops — and discovery
 * failure never falls back to sample data.
 */
export function useMachines(): MachinesState & {
  reconnect: () => void;
  refreshing: boolean;
  sessionEnded?: boolean;
} {
  const { status: deviceStatus, getAccessToken, device, gateway: deviceGateway } = useDevice();
  const signedIn = deviceStatus === "paired";
  const accountId = device?.userId ?? null;

  // Per-pairing source: a paired user.id change forces a NEW authenticated
  // socket instead of relabeling the old pairing's socket/response. Display
  // masking alone is not enough — the old socket was admitted as the old
  // pairing.
  const source = useMemo(
    () =>
      createMachineSocketSource({
        gateway: deviceGateway,
        deviceId: null,
        requireDevice: false,
        accountId,
        // Account discovery carries no desktop traffic, so no E2EE identity.
        phoneDeviceId: null,
        signedIn,
        getAccessToken,
      }),
    [deviceGateway, accountId, signedIn, getAccessToken],
  );
  const { client, state: socket, reconnect, available } = useRelaySocket(source);
  const [poll, setPoll] = useState(0);
  const [result, setResult] = useState<{
    client: typeof client;
    epoch: number;
    state: MachinesState;
  } | null>(null);

  // Foreground and network recovery refresh the listing, not just the socket.
  useForegroundRevival(
    useCallback(() => {
      setPoll((value) => value + 1);
    }, []),
  );

  useEffect(() => {
    if (!available || !client || socket.status !== "ready") return;
    // Generation identity is the client plus its epoch: the client is
    // already per-account/gateway (source owner), so a stale async result
    // can never be relabeled onto a new account's generation.
    const epoch = socket.connectionEpoch;
    let active = true;
    setResult((prev) =>
      prev?.client === client && prev.epoch === epoch
        ? prev
        : {
            client,
            epoch,
            state: Object.freeze({ status: "loading", reason: null, machines: null }),
          },
    );
    void listMachines(client.rpc)
      .then((machines) => {
        if (!active) return;
        setResult({
          client,
          epoch,
          state: Object.freeze({ status: "live", reason: null, machines }),
        });
      })
      .catch((error: unknown) => {
        if (!active) return;
        const message =
          error instanceof RelayRpcError && error.loginRequired
            ? SIGN_IN_MESSAGE
            : error instanceof RelayRpcError
              ? error.message || UNREACHABLE_MESSAGE
              : UNREACHABLE_MESSAGE;
        setResult({
          client,
          epoch,
          state: Object.freeze({ status: "unavailable", reason: message, machines: null }),
        });
      });
    return () => {
      active = false;
    };
  }, [available, client, socket.status, socket.connectionEpoch, accountId, deviceGateway, poll]);

  useEffect(() => {
    if (socket.status !== "ready") return;
    const timer = setTimeout(() => setPoll((value) => value + 1), MACHINES_POLL_MS);
    return () => clearTimeout(timer);
  }, [socket.status, socket.connectionEpoch, poll]);

  const refresh = useCallback(() => {
    reconnect();
    setPoll((value) => value + 1);
  }, [reconnect]);

  if (!signedIn || !getAccessToken) {
    return {
      status: "unavailable",
      reason: SIGN_IN_MESSAGE,
      machines: null,
      reconnect: refresh,
      refreshing: false,
    };
  }
  if (!deviceGateway) {
    return {
      status: "unavailable",
      reason: "Pair this phone with your desktop to view its live catalog.",
      machines: null,
      reconnect: refresh,
      refreshing: false,
    };
  }
  if (!available) {
    return {
      status: "unavailable",
      reason: SIGN_IN_MESSAGE,
      machines: null,
      reconnect: refresh,
      refreshing: false,
    };
  }
  if (socket.status === "error" || socket.status === "reconnecting") {
    return {
      sessionEnded: socket.sessionStatus === "ended",
      status: "unavailable",
      reason: socket.loginRequired
        ? SIGN_IN_MESSAGE
        : (socket.error ?? UNREACHABLE_MESSAGE),
      machines: null,
      reconnect: refresh,
      refreshing: false,
    };
  }
  // Same-render masking by client identity AND epoch: the previous
  // pairing's list never renders under the new pairing even when both
  // generations share an epoch number.
  const current = selectOwnedState(result, client, socket.connectionEpoch);
  if (socket.status !== "ready" || !current) {
    return {
      status: "loading",
      reason: null,
      machines: null,
      reconnect: refresh,
      refreshing: true,
    };
  }
  return { ...current, reconnect: refresh, refreshing: false };
}
