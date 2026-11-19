import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AppState } from "react-native";
import { useDevice } from "../../pairing/DeviceContext";
import { useMachine } from "../../machines/MachineContext";
import {
  attachCompanionTab,
  detachCompanionTab,
  inputCompanionTab,
  readCompanionTab,
  resizeCompanionTab,
  screenCompanionTab,
  subscribeCompanionTerminal,
} from "../../transport/relayRpcMethods";
import { TerminalRuntimeStore } from "../runtime/TerminalRuntimeStore";
import type { TerminalDelta } from "../../transport/protocol";
import {
  CompanionTerminalEngine,
  initialCompanionStatus,
  type CompanionResizeResult,
  type CompanionTerminalRpc,
} from "./engine";
import { sharedCompanionHandleRegistry } from "./registry";
import {
  COMPANION_TERMINAL_MESSAGES,
  COMPANION_TERMINAL_POLL_MS,
  type CompanionTerminalEngineStatus,
} from "./types";
import {
  clientTokenFor,
  companionOwnerKey,
  isCompanionEligible,
  selectOwnedCompanion,
} from "./owner";

export interface UseCompanionTerminalResult {
  readonly store: TerminalRuntimeStore;
  readonly status: CompanionTerminalEngineStatus;
  readonly eligible: boolean;
  readonly sendInput: (text: string) => Promise<{ ok: true } | { ok: false; message: string }>;
  readonly submitDraft: (draft: string) => Promise<{ ok: true } | { ok: false; message: string }>;
  readonly sendResize: (rows: number, cols: number) => Promise<CompanionResizeResult>;
  readonly retry: () => void;
  /**
   * §4.6 step 4: typed input that didn't reach the desktop, from the
   * machine context. Shown until dismissed or the channel is replaced.
   */
  readonly inputLost: { readonly handles: readonly number[]; readonly message: string } | null;
  readonly clearInputLost: () => void;
}

function machineOnlineOf(catalog: unknown): boolean {
  return (
    typeof catalog === "object" &&
    catalog !== null &&
    (catalog as { status?: unknown }).status === "live" &&
    typeof (catalog as { machine?: unknown }).machine === "object" &&
    (catalog as { machine: { online?: unknown } }).machine !== null &&
    (catalog as { machine: { online?: unknown } }).machine.online === true
  );
}

/**
 * One lifecycle per selected tab, keyed by full owner identity
 * (account/device/tab/client/epoch/retry). Reuses the machine route's
 * rpc/socket — never dials its own.
 *
 * Render masking is synchronous: the store instance is memoized per owner
 * key so a new owner renders a fresh blank runtime immediately, and status
 * falls back to blank until the new owner's engine publishes. The single
 * effect depends on owner key + eligibility: it disposes/detaches when
 * ineligible (no RPC in background) and creates a fresh engine + attach on
 * resume. Output arrives on the engine's push stream; the 200ms tick then
 * issues no request (it only watches for a renderer resync). Without
 * streaming support, polling is sequential, one RPC per tick, only while
 * eligible. Writes capture owner + eligibility and require the live engine
 * record to match before delegating; anything else reports not-ready
 * without touching the wire.
 */
export function useCompanionTerminal(tabId: number): UseCompanionTerminalResult {
  const machine = useMachine();
  const { device } = useDevice();
  const accountId = device?.userId ?? null;
  const deviceId = machine.deviceId;
  const rpc = machine.rpc;
  const socket = machine.socket;
  const catalog = machine.catalog;

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

  const [retryCounter, setRetryCounter] = useState(0);

  const tabValid = Number.isSafeInteger(tabId) && tabId >= 0;
  const clientToken = useMemo(() => clientTokenFor(rpc), [rpc]);
  // History-part failures (§4.5) survive one manual Retry: the second
  // failure keeps the live stream without history. Keyed per tab epoch so
  // a reconnect (new epoch) starts the count over.
  const partFailBaseKey = useMemo(
    () =>
      companionOwnerKey({
        accountId,
        deviceId,
        tabId,
        epoch: socket.connectionEpoch,
        retry: 0,
        clientToken,
      }),
    [accountId, deviceId, tabId, socket.connectionEpoch, clientToken],
  );
  const partFailuresRef = useRef<{ key: string; count: number } | null>(null);
  const ownerKey = useMemo(
    () =>
      companionOwnerKey({
        accountId,
        deviceId,
        tabId,
        epoch: socket.connectionEpoch,
        retry: retryCounter,
        clientToken,
      }),
    [accountId, deviceId, tabId, socket.connectionEpoch, retryCounter, clientToken],
  );

  const eligible = isCompanionEligible({
    foreground,
    socketReady: socket.status === "ready",
    leaseActive: socket.sessionStatus !== "ended",
    online: machineOnlineOf(catalog),
    rpcAvailable: rpc !== null,
    tabValid,
  });

  // Fresh blank runtime per owner, synchronously: a new owner never renders
  // the previous owner's bytes. The renderer remounts on store identity.
  const record = useMemo(() => ({ key: ownerKey, store: new TerminalRuntimeStore() }), [ownerKey]);
  useEffect(() => () => record.store.dispose(), [record]);

  const [statusRecord, setStatusRecord] = useState(() => ({
    key: ownerKey,
    status: initialCompanionStatus(),
  }));
  const status = selectOwnedCompanion(statusRecord.key, ownerKey, statusRecord.status, initialCompanionStatus());
  const store = selectOwnedCompanion(record.key, ownerKey, record.store, record.store);

  const engineRef = useRef<{ key: string; engine: CompanionTerminalEngine } | null>(null);

  // Single owner+eligibility effect: ineligible disposes/detaches with no
  // RPC; eligible creates a fresh engine, attaches once, then polls
  // sequentially until cleanup or ineligibility.
  useEffect(() => {
    if (!eligible || !tabValid || rpc === null) {
      engineRef.current?.engine.dispose();
      engineRef.current = null;
      return;
    }
    const client = rpc;
    const currentOwner = {
      accountId,
      deviceId,
      tabId,
      client,
      epoch: socket.connectionEpoch,
    };
    const companionRpc: CompanionTerminalRpc = {
      attach: (id) => attachCompanionTab(client, id),
      read: (handle, offset) => readCompanionTab(client, handle, offset),
      screen: (handle) => screenCompanionTab(client, handle),
      input: (handle, text) => inputCompanionTab(client, handle, text),
      resize: (handle, rows, cols) => resizeCompanionTab(client, handle, rows, cols),
      detach: (handle) => detachCompanionTab(client, handle),
      subscribe: (handle, offset, listener) => subscribeCompanionTerminal(client, handle, offset, listener),
    };
    let receive: (delta: TerminalDelta) => void = record.store.reset();
    const priorPartFailures =
      partFailuresRef.current?.key === partFailBaseKey ? partFailuresRef.current.count : 0;
    partFailuresRef.current = { key: partFailBaseKey, count: priorPartFailures };
    const engine = new CompanionTerminalEngine(
      currentOwner,
      companionRpc,
      {
        setViewport: (rows, cols) => record.store.setViewport(rows, cols),
        setScreenModes: (modes) => {
          if (engineRef.current?.key === ownerKey) record.store.setScreenModes(modes);
        },
        receive: (delta) => {
          if (engineRef.current?.key === ownerKey) receive(delta);
        },
        reset: () => {
          if (engineRef.current?.key === ownerKey) receive = record.store.reset();
        },
        notePartFailure: () => {
          if (engineRef.current?.key !== ownerKey) return;
          const current = partFailuresRef.current;
          partFailuresRef.current = {
            key: partFailBaseKey,
            count: (current?.key === partFailBaseKey ? current.count : 0) + 1,
          };
        },
      },
      sharedCompanionHandleRegistry,
      undefined,
      undefined,
      priorPartFailures,
    );
    engineRef.current?.engine.dispose();
    engineRef.current = { key: ownerKey, engine };
    const unsubscribe = engine.subscribe((next) => {
      setStatusRecord({ key: ownerKey, status: next });
    });
    // A renderer (e.g. a restarted WebView) needs raw bytes the store's window
    // dropped: the engine redraws from a seed instead of partial VT.
    const stopReplayWatch = record.store.subscribeReplayUnavailable(() => {
      if (engineRef.current?.key === ownerKey) engine.requestReseed();
    });
    setStatusRecord({ key: ownerKey, status: engine.getStatus() });
    let cancelled = false;
    void engine.attach();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = async () => {
      if (cancelled) return;
      if (engineRef.current?.key !== ownerKey) return;
      try {
        await engine.pollOnce();
      } catch {
        /* Engine never throws; belt and suspenders. */
      }
      try {
        const snapshot = record.store.getSnapshot();
        if (snapshot.presentation === "resync" && engine.getStatus().mode === "raw") {
          // Streaming: a fresh seed. Polling fallback: screen snapshots.
          engine.requestReseed();
        }
      } catch {
        /* Store faults stay local. */
      }
      if (!cancelled && engineRef.current?.key === ownerKey) {
        timer = setTimeout(() => {
          void tick();
        }, COMPANION_TERMINAL_POLL_MS);
      }
    };
    timer = setTimeout(() => {
      void tick();
    }, COMPANION_TERMINAL_POLL_MS);
    return () => {
      cancelled = true;
      if (timer !== null) clearTimeout(timer);
      unsubscribe();
      stopReplayWatch();
      if (engineRef.current?.key === ownerKey) engineRef.current = null;
      // Detach releases transport only — never kills the host PTY. Late
      // attach after this point detaches its handle immediately.
      engine.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ownerKey, eligible, tabValid, rpc, record, accountId, deviceId, tabId, socket.connectionEpoch]);

  const sendInput = useCallback(
    async (text: string) => {
      if (!eligible) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      const rec = engineRef.current;
      if (!rec || rec.key !== ownerKey) {
        return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      }
      return rec.engine.sendInput(text);
    },
    [ownerKey, eligible],
  );

  const submitDraft = useCallback(
    async (draft: string) => {
      if (!eligible) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      const rec = engineRef.current;
      if (!rec || rec.key !== ownerKey) {
        return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.sendNotReady };
      }
      return rec.engine.submitDraft(draft);
    },
    [ownerKey, eligible],
  );

  const sendResize = useCallback(
    async (rows: number, cols: number): Promise<CompanionResizeResult> => {
      if (!eligible) return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeNotReady };
      const rec = engineRef.current;
      if (!rec || rec.key !== ownerKey) {
        return { ok: false as const, message: COMPANION_TERMINAL_MESSAGES.resizeNotReady };
      }
      return rec.engine.sendResize(rows, cols);
    },
    [ownerKey, eligible],
  );

  const retry = useCallback(() => {
    setRetryCounter((value) => value + 1);
  }, []);

  return { store, status, eligible, sendInput, submitDraft, sendResize, retry, inputLost: machine.inputLost, clearInputLost: machine.clearInputLost };
}
