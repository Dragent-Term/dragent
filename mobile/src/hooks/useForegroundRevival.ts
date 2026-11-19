import { useEffect, useRef } from "react";
import { AppState } from "react-native";
import * as Network from "expo-network";

interface ConnectionNetworkState {
  type?: string;
  isConnected?: boolean;
  isInternetReachable?: boolean;
}

function offline(state: ConnectionNetworkState): boolean {
  return state.isConnected === false || state.isInternetReachable === false;
}

function route(state: ConnectionNetworkState): string | null {
  const value = state.type?.toLowerCase();
  return !value || value === "unknown" || value === "none" ? null : value;
}

/** A live Wi-Fi/cellular handoff can invalidate a socket without an offline event. */
function shouldReviveConnection(previous: ConnectionNetworkState | null, next: ConnectionNetworkState): boolean {
  if (!previous || offline(next)) return false;
  if (offline(previous)) return true;
  const before = route(previous);
  const after = route(next);
  return before !== null && after !== null && before !== after;
}

/**
 * Shared lifecycle adapter for foreground and network recovery. `awayMs` is
 * how long the app was in the background (0 for a network change).
 */
export function useForegroundRevival(revive: (reason: "foreground" | "network", awayMs: number) => void) {
  const latest = useRef(revive);
  latest.current = revive;
  useEffect(() => {
    let previous: ConnectionNetworkState | null = null;
    let leftAt: number | null = AppState.currentState === "active" ? null : Date.now();
    const app = AppState.addEventListener("change", (state) => {
      if (state === "active") {
        const away = leftAt === null ? 0 : Date.now() - leftAt;
        leftAt = null;
        latest.current("foreground", away);
      } else if (state === "background" && leftAt === null) {
        leftAt = Date.now();
      }
    });
    const network = Network.addNetworkStateListener((state) => {
      const revive = shouldReviveConnection(previous, state);
      previous = state;
      if (revive) latest.current("network", 0);
    });
    return () => { app.remove(); network.remove(); };
  }, []);
}
