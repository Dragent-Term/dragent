import { useHeldCatalog } from "./heldCatalog";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useDevice } from "../pairing/DeviceContext";
import type { RelayRpcClient } from "../transport/relayRpcClient";
import { subscribeSharedCatalog } from "../transport/sharedCatalog";
import type { MachineSummary } from "../transport/relayCatalog";
import type { MachineCatalogState } from "../transport/useRelayCatalog";
import {
  loadingMachineCatalog,
  unavailableMachineCatalog,
} from "../transport/useRelayCatalog";
import { selectOwnedState, useRelaySocket } from "../transport/useRelaySocket";
import type { RelaySocketSnapshot } from "../transport/RelaySocketClient";
import { createMachineSocketSource } from "./socketSource";
import { useResumeWatchdog } from "./resumeWatchdog";
import { useConnectionEvents } from "../diagnostics/useConnectionEvents";
import { publishDesktop } from "../agentCard/store";

export interface MachineContextValue {
  readonly deviceId: string;
  /** Targeted rpc client, or null until the machine socket admits. */
  readonly rpc: RelayRpcClient | null;
  readonly socket: RelaySocketSnapshot;
  readonly catalog: MachineCatalogState;
  readonly reconnect: () => void;
  readonly available: boolean;
  /**
   * §4.6 step 4: typed input that didn't reach the desktop
   * ("Some of what you typed didn't reach `<desktop>`"). Shown on the
   * terminal views until dismissed or the channel is replaced.
   */
  readonly inputLost: { readonly handles: readonly number[]; readonly message: string } | null;
  readonly clearInputLost: () => void;
}

const MachineContext = createContext<MachineContextValue | null>(null);

/** Automatic catalog retries on a ready connection before waiting for Retry. */
const CATALOG_AUTO_RETRIES = 3;
const SIGN_IN_MESSAGE = "Pair this phone to view this desktop's live catalog.";
const UNREACHABLE_MESSAGE = "The live machine catalog is unreachable right now.";

/**
 * Owns ONE targeted socket at the machine route boundary (`app/machine/
 * [deviceId]/_layout`) and shares its rpc client plus the live catalog
 * subscription with project/workspace/tab children. Home discovery owns a
 * separate account-scoped client; the two never share state.
 */
export function MachineProvider({
  deviceId,
  children,
}: {
  deviceId: string;
  children: ReactNode;
}) {
  const { status: deviceStatus, getAccessToken, device, gateway: deviceGateway } = useDevice();
  const signedIn = deviceStatus === "paired";
  const accountId = device?.userId ?? null;
  // This phone's own device id (the desktop's is `deviceId`).
  const phoneDeviceId = device?.deviceId ?? null;
  const [inputLost, setInputLost] = useState<{
    handles: readonly number[];
    message: string;
  } | null>(null);
  // Stable identity: the socket hook reads it when the client is built and
  // a new callback must never tear down a live connection.
  const onInputLost = useCallback((handles: readonly number[], message: string) => {
    setInputLost({ handles, message });
  }, []);
  const clearInputLost = useCallback(() => {
    setInputLost(null);
  }, []);
  // Per-pairing source: a new paired user id forces a new resolver identity and
  // client even when the controller method stays stable. The render below
  // additionally masks by client identity, so the previous pairing's catalog
  // can never leak through an epoch collision (both clients start at epoch 1).
  const source = useMemo(
    () =>
      createMachineSocketSource({
        gateway: deviceGateway,
        deviceId,
        accountId,
        phoneDeviceId,
        signedIn,
        getAccessToken,
        onInputLost,
      }),
    [deviceGateway, deviceId, accountId, phoneDeviceId, signedIn, getAccessToken, onInputLost],
  );
  const { client, state: socket, reconnect: reconnectSocket, available } = useRelaySocket(source);
  // A new catalog subscription on the same connection: Retry, and the
  // automatic retries below. `fresh` drops one that never delivered.
  const [catalogAttempt, setCatalogAttempt] = useState(0);
  const autoRetries = useRef(0);
  const [result, setResult] = useState<{
    client: typeof client;
    epoch: number;
    state: MachineCatalogState;
  } | null>(null);

  // A replaced channel restarts the §4.3 prefix guarantee, so a previous
  // input-loss notice no longer describes this channel.
  useEffect(() => {
    setInputLost(null);
  }, [client, socket.connectionEpoch]);

  useEffect(() => {
    if (!client || socket.status !== "ready") return;
    let active = true;
    const epoch = socket.connectionEpoch;
    setResult({ client, epoch, state: loadingMachineCatalog });
    const publish = (state: MachineCatalogState) => {
      if (active) setResult({ client, epoch, state });
    };
    let subscription: { unsubscribe(): Promise<void> } | null = null;
    try {
      // A rejected blob belongs to the one delivery that follows it: the
      // last good copy stays on screen with its "didn't check out" warning.
      let warning: string | null = null;
      subscription = subscribeSharedCatalog(client, {
        onSnapshot: (snapshot) =>
          publish(
            Object.freeze({
              status: "live",
              reason: null,
              catalog: snapshot.catalog,
              machine: snapshot.machine,
              warning,
            }),
          ),
        onUpdate: (snapshot) =>
          publish(
            Object.freeze({
              status: "live",
              reason: null,
              catalog: snapshot.catalog,
              machine: snapshot.machine,
              warning,
            }),
          ),
        onRejected: (message) => {
          warning = message;
        },
        // Subscription failure (initial or midstream) leaves loading at
        // once — and drops any stale live state — instead of waiting out
        // the snapshot deadline.
        onError: (error) =>
          publish(
            unavailableMachineCatalog(
              error.loginRequired ? SIGN_IN_MESSAGE : error.message || UNREACHABLE_MESSAGE,
            ),
          ),
        // §6.5: every catalog from the relay is sealed; the pin store opens
        // and verifies it. Without the opener nothing could be shown.
      }, { fresh: catalogAttempt > 0 });
    } catch {
      publish(unavailableMachineCatalog(UNREACHABLE_MESSAGE));
      return () => {
        active = false;
      };
    }
    const deadline = setTimeout(() => {
      if (!active) return;
      setResult((prev) => {
        if (!prev || prev.client !== client || prev.epoch !== epoch) return prev;
        if (prev.state.status !== "loading") return prev;
        return { client, epoch, state: unavailableMachineCatalog(UNREACHABLE_MESSAGE) };
      });
    }, 15000);
    return () => {
      active = false;
      clearTimeout(deadline);
      const sub = subscription;
      subscription = null;
      if (sub) void sub.unsubscribe().catch(() => {});
    };
  }, [client, socket.status, socket.connectionEpoch, catalogAttempt]);

  // Retry on the connection that is up, rather than wait for the person: a
  // catalog that didn't arrive (or broke) while the socket is ready used to
  // stay "unreachable" until they left the desktop's screens.
  const owned = selectOwnedState(result, client, socket.connectionEpoch);
  const catalogFailed = socket.status === "ready" && owned !== null && owned.status === "unavailable";
  useEffect(() => {
    if (owned?.status === "live") autoRetries.current = 0;
  }, [owned]);
  useEffect(() => {
    if (!catalogFailed || autoRetries.current >= CATALOG_AUTO_RETRIES) return;
    const timer = setTimeout(() => {
      autoRetries.current += 1;
      setCatalogAttempt((value) => value + 1);
    }, 1500 * (autoRetries.current + 1));
    return () => clearTimeout(timer);
  }, [catalogFailed]);
  const reconnect = useCallback(() => {
    autoRetries.current = 0;
    // A ready socket ignores connect(): renew the catalog instead.
    if (socket.status === "ready") setCatalogAttempt((value) => value + 1);
    else reconnectSocket();
  }, [socket.status, reconnectSocket]);

  const bare = useMemo<Omit<MachineContextValue, "inputLost" | "clearInputLost">>(() => {
    const trimmed = deviceId.trim();
    if (!trimmed) {
      return {
        deviceId: "",
        rpc: null,
        socket,
        catalog: unavailableMachineCatalog(
          "This machine has not shared its live catalog.",
        ),
        reconnect,
        available: false,
      };
    }
    if (!signedIn) {
      return {
        deviceId: trimmed,
        rpc: null,
        socket,
        catalog: unavailableMachineCatalog(SIGN_IN_MESSAGE),
        reconnect,
        available: false,
      };
    }
    if (!deviceGateway) {
      return {
        deviceId: trimmed,
        rpc: null,
        socket,
        catalog: unavailableMachineCatalog(
          "Pair this phone with your desktop to view its live catalog.",
        ),
        reconnect,
        available: false,
      };
    }
    if (socket.status === "error" || socket.status === "reconnecting") {
      return {
        deviceId: trimmed,
        rpc: null,
        socket,
        catalog: unavailableMachineCatalog(
          socket.loginRequired ? SIGN_IN_MESSAGE : (socket.error ?? UNREACHABLE_MESSAGE),
        ),
        reconnect,
        available,
      };
    }
    // Same-render masking by client identity AND epoch: a new account or
    // device owns a new client whose epoch restarts at 1, so the previous
    // generation's catalog must not render even on an epoch collision.
    if (socket.status !== "ready" || !owned) {
      return {
        deviceId: trimmed,
        rpc: client?.rpc ?? null,
        socket,
        catalog: loadingMachineCatalog,
        reconnect,
        available,
      };
    }
    return {
      deviceId: trimmed,
      rpc: client?.rpc ?? null,
      socket,
      catalog: owned,
      reconnect,
      available,
    };
  }, [deviceId, signedIn, deviceGateway, socket, owned, client, reconnect, available]);

  // While the connection is re-made (coming back to the app, a network
  // change) the last catalog stays on screen instead of an "unreachable" page.
  const catalog = useHeldCatalog(client, socket.status, bare.catalog);
  // The live agent card (lock screen / ongoing notification) reads every
  // connected desktop's agents from here.
  useEffect(() => {
    if (bare.catalog.status !== "live") return;
    const live = bare.catalog;
    publishDesktop(
      deviceId,
      live.catalog.machine?.name || live.machine.name || "Desktop",
      live.machine.online ? live.catalog : null,
    );
  }, [deviceId, bare.catalog]);
  // Back from the background and the desktop isn't answering: go Home
  // rather than leave a screen loading. Judged on the real catalog, not the
  // held copy that stands in while the connection is re-made.
  useResumeWatchdog(socket, bare.catalog);
  // The connection log (Settings → Connection log).
  useConnectionEvents(deviceId, socket, bare.catalog);
  const value = useMemo<MachineContextValue>(
    () => ({ ...bare, catalog, inputLost, clearInputLost }),
    [bare, catalog, inputLost, clearInputLost],
  );

  return <MachineContext value={value}>{children}</MachineContext>;
}

export function useMachine(): MachineContextValue {
  const value = useContext(MachineContext);
  if (!value) throw new Error("useMachine requires MachineProvider");
  return value;
}

export type { MachineSummary };
