/** When coming back to the app starts at Home (see awayHome.ts). */
export const AWAY_HOME_MS = 5 * 60 * 1000;
/** A notification tap this recent means the person chose where to go. */
const TAP_WINS_MS = 3_000;
/** Notification taps arrive just after the app becomes active. */
export const SETTLE_MS = 400;

/** Screens that are themselves a start, or a flow that must not be cut. */
export function staysPut(pathname: string): boolean {
  return pathname === "/" || pathname === "/settings" || pathname.startsWith("/pair-");
}

export function shouldGoHome(awayMs: number, pathname: string, lastTapAt: number, now: number): boolean {
  if (awayMs < AWAY_HOME_MS) return false;
  if (staysPut(pathname)) return false;
  return now - lastTapAt > TAP_WINS_MS;
}

