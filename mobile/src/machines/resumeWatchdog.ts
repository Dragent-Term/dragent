/**
 * Coming back to the app after a while re-makes the desktop connection. It
 * usually takes a moment; when it doesn't, the screens sat on "loading" with
 * no way forward. If the connection isn't back a few seconds after the app
 * returns, the person is taken Home, where every desktop and the "Resume"
 * card are a tap away, instead of waiting on a screen that can't load.
 */
import { useEffect, useRef } from "react";
import { AppState } from "react-native";
import { router } from "expo-router";
import type { RelaySocketSnapshot } from "../transport/RelaySocketClient";
import type { MachineCatalogState } from "../transport/useRelayCatalog";
import { connectionBack } from "./resumeHealth";
import { logConnection } from "../diagnostics/connectionLog";

/** How long a returning screen may wait for its desktop before going Home. */
export const RESUME_GRACE_MS = 5_000;
/** A switch away shorter than this keeps its connection: nothing to watch. */
export const RESUME_AWAY_MS = 3_000;

/**
 * Watch the machine's connection after the app returns from the background;
 * go Home if it isn't back within `RESUME_GRACE_MS`.
 */
export function useResumeWatchdog(socket: RelaySocketSnapshot, catalog: MachineCatalogState): void {
  const back = useRef(false);
  back.current = connectionBack(socket, catalog);
  useEffect(() => {
    let leftAt: number | null = AppState.currentState === "active" ? null : Date.now();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const clear = () => {
      if (timer !== null) clearTimeout(timer);
      timer = null;
    };
    const sub = AppState.addEventListener("change", (state) => {
      if (state === "active") {
        const away = leftAt === null ? 0 : Date.now() - leftAt;
        leftAt = null;
        clear();
        if (away < RESUME_AWAY_MS) return;
        timer = setTimeout(() => {
          timer = null;
          if (back.current || AppState.currentState !== "active") return;
          logConnection("warn", null, `Back after ${Math.round(away / 1000)} s away and the desktop didn't answer in ${RESUME_GRACE_MS / 1000} s: went Home`);
          if (router.canDismiss()) router.dismissTo("/");
          else router.replace("/");
        }, RESUME_GRACE_MS);
      } else if (leftAt === null) {
        leftAt = Date.now();
        clear();
      }
    });
    return () => {
      clear();
      sub.remove();
    };
  }, []);
}
