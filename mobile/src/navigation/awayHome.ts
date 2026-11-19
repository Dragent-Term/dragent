/**
 * Away for five minutes or more (the app in the background, or the phone
 * locked with it open) → coming back starts at Home, with every desktop and
 * the Resume card a tap away, instead of a screen whose connection went
 * stale. Shorter switches come back to where you were.
 *
 * Opening the app by tapping a notification wins: it goes to that desktop.
 */
import { useEffect, useRef } from "react";
import { AppState, type AppStateStatus } from "react-native";
import { router, usePathname } from "expo-router";
import { AWAY_HOME_MS, SETTLE_MS, shouldGoHome } from "./awayHomeRules";

export { AWAY_HOME_MS } from "./awayHomeRules";

let tappedAt = 0;
/** Called when a notification tap opens a screen. */
export function markOpenedFromNotification(now = Date.now()): void {
  tappedAt = now;
}

export function useAwayGoesHome(): void {
  const pathname = usePathname();
  const where = useRef(pathname);
  where.current = pathname;
  useEffect(() => {
    let leftAt: number | null = AppState.currentState === "active" ? null : Date.now();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const onChange = (state: AppStateStatus) => {
      if (state === "active") {
        if (leftAt === null) return;
        const away = Date.now() - leftAt;
        leftAt = null;
        if (away < AWAY_HOME_MS) return;
        if (timer !== null) clearTimeout(timer);
        timer = setTimeout(() => {
          timer = null;
          if (AppState.currentState !== "active") return;
          if (!shouldGoHome(away, where.current, tappedAt, Date.now())) return;
          if (router.canDismiss()) router.dismissTo("/");
          else router.replace("/");
        }, SETTLE_MS);
      } else if (leftAt === null) {
        leftAt = Date.now();
      }
    };
    const subscription = AppState.addEventListener("change", onChange);
    return () => {
      if (timer !== null) clearTimeout(timer);
      subscription.remove();
    };
  }, []);
}
