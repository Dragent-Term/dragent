import { useCallback, useEffect, useState } from "react";
import { router } from "expo-router";
import { useMachine } from "../../machines/MachineContext";
import { machineTabPath } from "../../machines/routes";
import {
  NEW_TAB_APPEAR_TIMEOUT_MS,
  NEW_TAB_FAILED_MESSAGE,
  NEW_TAB_NOT_SUPPORTED_MESSAGE,
  NEW_TAB_TIMEOUT_MESSAGE,
  newTabWaitStatus,
  newTabWorkspaceId,
} from "../../machines/newTab";
import { RelayRpcError } from "../../transport/relayRpcClient";
import { createWorkspaceTab, type RelayNewTabKind } from "../../transport/relayRpcMethods";
import type { RelayWorkspace } from "../../transport/relayCatalog";

export interface UseNewTab {
  /** False when this workspace can't take new tabs from the phone (legacy catalog). */
  readonly available: boolean;
  /** A tab is being opened on the desktop, or is on its way into the list. */
  readonly creating: boolean;
  readonly error: string | null;
  readonly create: (kind: RelayNewTabKind) => void;
  readonly clearError: () => void;
}

/**
 * Open a terminal, browser or Dragent CLI tab in a workspace on the desktop,
 * then go to it as soon as the live catalog carries it. `replace` swaps the
 * current screen for the new tab (from inside another tab) instead of
 * stacking on top of it.
 */
export function useNewTab(deviceId: string, workspace: RelayWorkspace | null, replace = false): UseNewTab {
  const machine = useMachine();
  const { catalog, rpc } = machine;
  const [creating, setCreating] = useState(false);
  const [pending, setPending] = useState<{ id: number; startedAt: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, setTick] = useState(0);
  const targetId = workspace ? newTabWorkspaceId(workspace) : null;

  const create = useCallback((kind: RelayNewTabKind) => {
    setError(null);
    if (!rpc || !targetId) {
      setError(targetId ? NEW_TAB_FAILED_MESSAGE : NEW_TAB_NOT_SUPPORTED_MESSAGE);
      return;
    }
    setCreating(true);
    createWorkspaceTab(rpc, targetId, kind)
      .then(({ id }) => setPending({ id, startedAt: Date.now() }))
      .catch((failure: unknown) => {
        setCreating(false);
        setError(failure instanceof RelayRpcError && failure.message ? failure.message : NEW_TAB_FAILED_MESSAGE);
      });
  }, [rpc, targetId]);

  const liveTabIds = catalog.status === "live" ? catalog.catalog.tabs.map((tab) => tab.id) : [];
  const wait = pending ? newTabWaitStatus(liveTabIds, pending.id, pending.startedAt, Date.now()) : null;
  useEffect(() => {
    if (!pending || !wait) return;
    if (wait === "ready") {
      setPending(null);
      setCreating(false);
      const path = machineTabPath(deviceId, pending.id);
      if (replace) router.replace(path);
      else router.push(path);
      return;
    }
    if (wait === "timeout") {
      setPending(null);
      setCreating(false);
      setError(NEW_TAB_TIMEOUT_MESSAGE);
      return;
    }
    // Check again at the deadline even if the catalog stays quiet.
    const timer = setTimeout(
      () => setTick((tick) => tick + 1),
      Math.max(0, pending.startedAt + NEW_TAB_APPEAR_TIMEOUT_MS - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [pending, wait, deviceId, replace]);

  const clearError = useCallback(() => setError(null), []);
  return { available: targetId !== null, creating, error, create, clearError };
}
