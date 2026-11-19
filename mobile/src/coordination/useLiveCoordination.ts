/**
 * Live coordination polling. Owns no sockets: both resources reuse the
 * machine route's account-scoped rpc/socket from `MachineContext`.
 *
 * One small typed resource hook (`useCoordinationResource`) drives a
 * `CoordinationResourceController` sequentially on a bounded 5s cadence;
 * `useLiveCoordinationList` and `useLiveCoordinationDetail` are thin
 * wrappers differing only in scope and fetch. No general app framework.
 *
 * - Polls only while foreground + socket ready + session active + machine
 *   online; never with an ended/missing lease.
 * - Generation-guarded: owner/eligibility change disposes the controller and
 *   late results never publish; render masks stale records synchronously via
 *   the owner key (same pattern as the companion terminal hook).
 * - Ineligibility synchronously hides previous live results and shows a
 *   meaningful hold (offline / ended / reconnecting / background) instead of
 *   stale output or permanent loading.
 * - Fetch failures surface retryable errors; empty renders only on a
 *   successful empty reply — never on error. Truncation is explicit.
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { AppState } from "react-native";
import { useDevice } from "../pairing/DeviceContext";
import { useMachine } from "../machines/MachineContext";
import type { RelayRpcClient } from "../transport/relayRpcClient";
import type { RelaySocketSnapshot } from "../transport/RelaySocketClient";
import { getCoordination, listCoordinations } from "./rpc";
import {
  coordinationClientTokenFor,
  coordinationOwnerKey,
  COORDINATION_POLL_MS,
  isCoordinationEligible,
  selectOwnedCoordination,
} from "./owner";
import {
  CoordinationResourceController,
  COORDINATION_SIGN_IN_MESSAGE,
  COORDINATION_UNREACHABLE_MESSAGE,
  type CoordinationResourceState,
} from "./controller";
import type { CoordinationDetailResult, CoordinationListResult } from "./types";

export { COORDINATION_POLL_MS };

export type CoordinationListState = CoordinationResourceState<CoordinationListResult>;
export type CoordinationDetailState = CoordinationResourceState<CoordinationDetailResult>;

type CoordinationHold = Extract<CoordinationListState, { status: "unavailable" }>;

const OFFLINE_MESSAGE = "This machine is offline. Coordination resumes when it reconnects.";
const ENDED_MESSAGE = "This session has ended. Start a new session to view live coordination.";
const CONNECTING_MESSAGE = "Connecting… Live coordination resumes when the connection is ready.";
const RECONNECTING_MESSAGE = "Reconnecting… Live coordination resumes when the connection is ready.";
const BACKGROUND_MESSAGE = "Paused while the app is in the background. It resumes when you return.";
const WAITING_MESSAGE = "Waiting for the live session…";
const INVALID_MESSAGE = "This coordination is unavailable.";

function useForeground(): boolean {
  const [foreground, setForeground] = useState(() => AppState.currentState === "active");
  useEffect(() => {
    setForeground(AppState.currentState === "active");
    const sub = AppState.addEventListener("change", (state) => {
      setForeground(state === "active");
    });
    return () => {
      sub.remove();
    };
  }, []);
  return foreground;
}

const LOADING: { readonly status: "loading" } = Object.freeze({ status: "loading" });

/** Meaningful hold for an ineligible moment: never stale output, never a spinner. */
function holdStateOf(args: {
  deviceValid: boolean;
  signedIn: boolean;
  socket: RelaySocketSnapshot;
  online: boolean;
  foreground: boolean;
  rpcAvailable: boolean;
}): CoordinationHold {
  if (!args.deviceValid) {
    return { status: "unavailable", reason: INVALID_MESSAGE, action: "retry" };
  }
  if (!args.signedIn || args.socket.loginRequired) {
    return { status: "unavailable", reason: COORDINATION_SIGN_IN_MESSAGE, action: "retry" };
  }
  if (args.socket.sessionStatus === "ended") {
    return { status: "unavailable", reason: ENDED_MESSAGE, action: "reconnect" };
  }
  if (args.socket.status === "error") {
    return {
      status: "unavailable",
      reason: args.socket.error || COORDINATION_UNREACHABLE_MESSAGE,
      action: "retry",
    };
  }
  if (args.socket.status === "connecting" || args.socket.status === "idle") {
    return { status: "unavailable", reason: CONNECTING_MESSAGE, action: "retry" };
  }
  if (args.socket.status === "reconnecting") {
    return { status: "unavailable", reason: RECONNECTING_MESSAGE, action: "retry" };
  }
  if (!args.online) {
    return { status: "unavailable", reason: OFFLINE_MESSAGE, action: "retry" };
  }
  if (!args.foreground) {
    return { status: "unavailable", reason: BACKGROUND_MESSAGE, action: "retry" };
  }
  if (args.socket.sessionStatus !== "active" || !args.rpcAvailable) {
    return { status: "unavailable", reason: WAITING_MESSAGE, action: "retry" };
  }
  return { status: "unavailable", reason: COORDINATION_UNREACHABLE_MESSAGE, action: "retry" };
}

/**
 * Single typed polling resource. The wrappers below supply scope + fetch;
 * everything else (owner key, eligibility, hold masking, cadence, retry and
 * session reconnect) lives here exactly once.
 */
function useCoordinationResource<T>(input: {
  projectId: string;
  teamId: string;
  scopeValid: boolean;
  fetch: (client: RelayRpcClient) => Promise<T>;
  /** Poll cadence; the coordination default when absent. */
  pollMs?: number | undefined;
}): {
  readonly state: CoordinationResourceState<T>;
  readonly eligible: boolean;
  readonly retry: () => void;
  readonly reconnect: () => void;
} {
  const machine = useMachine();
  const { status: deviceStatus, device } = useDevice();
  const accountId = device?.userId ?? null;
  const authStatus = deviceStatus === "paired" ? "signed-in" : "signed-out";
  const signedIn = authStatus === "signed-in";
  const foreground = useForeground();
  const [retryCounter, setRetryCounter] = useState(0);

  const deviceId = machine.deviceId;
  const rpc = machine.rpc;
  const socket = machine.socket;
  const online = machine.catalog.status === "live" && machine.catalog.machine.online;

  const deviceValid = deviceId.trim() !== "" && input.scopeValid;
  const clientToken = useMemo(() => coordinationClientTokenFor(rpc), [rpc]);
  const ownerKey = useMemo(
    () =>
      coordinationOwnerKey({
        accountId,
        deviceId,
        epoch: socket.connectionEpoch,
        retry: retryCounter,
        clientToken,
        projectId: input.projectId,
        teamId: input.teamId,
      }),
    [accountId, deviceId, socket.connectionEpoch, retryCounter, clientToken, input.projectId, input.teamId],
  );
  const eligible = isCoordinationEligible({
    foreground,
    socketReady: socket.status === "ready",
    sessionActive: socket.sessionStatus === "active",
    online,
    rpcAvailable: rpc !== null,
    deviceValid,
  });

  const [record, setRecord] = useState<{ key: string; state: CoordinationResourceState<T> }>(() => ({
    key: ownerKey,
    state: LOADING,
  }));
  const hold = holdStateOf({
    deviceValid,
    signedIn,
    socket,
    online,
    foreground,
    rpcAvailable: rpc !== null,
  });
  const shown = eligible
    ? selectOwnedCoordination(record.key, ownerKey, record.state, LOADING)
    : hold;

  const pollMs = input.pollMs ?? COORDINATION_POLL_MS;
  useEffect(() => {
    if (!eligible || rpc === null) return;
    const key = ownerKey;
    const client = rpc;
    const run = input.fetch;
    setRecord((prev) => (prev.key === key ? prev : { key, state: LOADING }));
    const controller = new CoordinationResourceController<T>(
      () => run(client),
      (next) => setRecord({ key, state: next }),
    );
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    void controller.pollOnce();
    const tick = async () => {
      if (cancelled) return;
      await controller.pollOnce();
      if (!cancelled) {
        timer = setTimeout(() => {
          void tick();
        }, pollMs);
      }
    };
    timer = setTimeout(() => {
      void tick();
    }, pollMs);
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
      controller.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ownerKey, eligible, rpc, input.fetch, pollMs]);

  const retry = useCallback(() => {
    if (!eligible) machine.reconnect();
    setRetryCounter((value) => value + 1);
  }, [eligible, machine.reconnect]);

  const reconnect = useCallback(() => {
    machine.reconnect();
    setRetryCounter((value) => value + 1);
  }, [machine.reconnect]);

  return { state: shown, eligible, retry, reconnect };
}

/** Live team summaries for the machine coordination section (readonly). */
export function useLiveCoordinationList(projectId?: string): {
  readonly state: CoordinationListState;
  readonly eligible: boolean;
  readonly retry: () => void;
  readonly reconnect: () => void;
} {
  const scopeProject = typeof projectId === "string" ? projectId : "";
  const fetch = useCallback(
    (client: RelayRpcClient) =>
      listCoordinations(client, scopeProject === "" ? {} : { project_id: scopeProject }),
    [scopeProject],
  );
  return useCoordinationResource<CoordinationListResult>({
    projectId: scopeProject,
    teamId: "",
    scopeValid: true,
    fetch,
  });
}

/** Live team detail with tasks and messages (readonly). */
export function useLiveCoordinationDetail(teamId: string): {
  readonly state: CoordinationDetailState;
  readonly eligible: boolean;
  readonly retry: () => void;
  readonly reconnect: () => void;
} {
  const scopeTeam = typeof teamId === "string" ? teamId : "";
  const fetch = useCallback(
    (client: RelayRpcClient) => getCoordination(client, scopeTeam),
    [scopeTeam],
  );
  return useCoordinationResource<CoordinationDetailResult>({
    projectId: "",
    teamId: scopeTeam,
    scopeValid: scopeTeam.trim() !== "",
    fetch,
  });
}

/**
 * The same machine resource for other live areas (Tasks): one polled read
 * scoped by `scope`, with the same eligibility, holds and retry.
 */
export const useMachineResource = <T,>(input: {
  scope: string;
  fetch: (client: RelayRpcClient) => Promise<T>;
  pollMs?: number | undefined;
}) =>
  useCoordinationResource<T>({
    projectId: input.scope,
    teamId: "",
    scopeValid: true,
    fetch: input.fetch,
    pollMs: input.pollMs,
  });
