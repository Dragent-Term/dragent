/**
 * Writes a desktop connection's changes into the connection log: socket
 * states, the encrypted link, the session, sign-in, and whether the desktop
 * is online. Only states and error words, never anything a desktop sent.
 */
import { useEffect, useRef } from "react";
import { AppState } from "react-native";
import type { RelaySocketSnapshot } from "../transport/RelaySocketClient";
import type { MachineCatalogState } from "../transport/machineCatalogState";
import { logConnection } from "./connectionLog";

const LINK_WORDS: Record<string, string> = {
  connecting: "Encrypted link: setting up",
  open: "Encrypted link: open",
  retrying: "Encrypted link lost; setting it up again",
  unpaired: "Encrypted link: this desktop isn't paired on this phone",
  lost: "Encrypted link: this phone's security records failed their check",
  unavailable: "Encrypted link: not available in this build",
  closed: "Encrypted link closed",
};

/**
 * The last line written per desktop and topic. Every screen showing a
 * desktop runs this hook on the same shared connection, so the record is
 * per desktop, not per screen: each change is written once, however many
 * screens are open.
 */
const lastWritten = new Map<string, string>();

/** Write `words` for `desktop`'s `topic` unless it's what was last written. */
export function logOnce(desktop: string, topic: string, level: "info" | "warn" | "error", words: string): boolean {
  const key = `${desktop}\u0000${topic}`;
  if (lastWritten.get(key) === words) return false;
  lastWritten.set(key, words);
  logConnection(level, desktop, words);
  return true;
}

/** The topic is back to normal: its next problem is written again. */
function settle(desktop: string, topic: string): void {
  lastWritten.delete(`${desktop}\u0000${topic}`);
}

/** What a socket state means for the log; null when there's nothing to say. */
export function socketWords(socket: Pick<RelaySocketSnapshot, "status" | "retryAttempt" | "error">): { level: "info" | "warn" | "error"; words: string } | null {
  switch (socket.status) {
    case "connecting": return { level: "info", words: "Connecting…" };
    case "ready": return { level: "info", words: "Connected" };
    case "reconnecting": return { level: "warn", words: `Connection lost; retrying (attempt ${socket.retryAttempt})` };
    case "error": return { level: "error", words: `Couldn't connect${socket.error ? `: ${socket.error}` : ""}` };
    // A screen's connection before it's joined, or one closed because no
    // screen needs it any more: neither is a drop.
    default: return null;
  }
}

/** What the desktop's catalog state means for the log; null for loading states. */
export function catalogWords(catalog: MachineCatalogState): { level: "info" | "warn"; words: string } | null {
  if (catalog.status === "live") {
    return catalog.machine.online
      ? { level: "info", words: "Desktop online" }
      : { level: "warn", words: "Desktop offline (asleep, closed or without internet)" };
  }
  if (catalog.status === "unavailable") return { level: "warn", words: `Desktop's workspace list unavailable: ${catalog.reason}` };
  return null;
}

export function useConnectionEvents(desktop: string, socket: RelaySocketSnapshot, catalog: MachineCatalogState): void {
  useEffect(() => {
    const said = socketWords(socket);
    if (said) logOnce(desktop, "socket", said.level, said.words);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop, socket.status, socket.retryAttempt, socket.error]);

  useEffect(() => {
    const secure = socket.secure;
    if (!secure) return;
    const words = LINK_WORDS[secure.state] ?? `Encrypted link: ${secure.state}`;
    const level = secure.state === "open" || secure.state === "connecting" ? "info" : secure.state === "retrying" ? "warn" : "error";
    logOnce(desktop, "link", level, secure.message ? `${words} — ${secure.message}` : words);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop, socket.secure?.state, socket.secure?.problem]);

  useEffect(() => {
    if (socket.sessionStatus === "ended") logOnce(desktop, "session", "warn", "The connection's session ended; a new one is needed");
    else settle(desktop, "session");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop, socket.sessionStatus]);

  useEffect(() => {
    if (socket.loginRequired) logOnce(desktop, "login", "error", "Sign-in needed before this desktop can be reached");
    else settle(desktop, "login");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [desktop, socket.loginRequired]);

  useEffect(() => {
    const said = catalogWords(catalog);
    if (said) logOnce(desktop, "catalog", said.level, said.words);
  }, [desktop, catalog]);
}

/** The app going to the background and coming back (mounted once). */
export function useAppStateEvents(): void {
  useEffect(() => {
    let last = AppState.currentState;
    const subscription = AppState.addEventListener("change", (next) => {
      if (next === last) return;
      if (next === "active") logConnection("info", null, "App opened");
      else if (next === "background") logConnection("info", null, "App went to the background");
      last = next;
    });
    return () => subscription.remove();
  }, []);
}

/** Push registration results (mounted once, inside the push provider). */
export function usePushEvents(status: string, reason: string | null): void {
  const last = useRef<string | null>(null);
  useEffect(() => {
    const key = `${status}:${reason ?? ""}`;
    if (last.current === key) return;
    last.current = key;
    if (status === "enabled") logConnection("info", null, "Notifications on: this phone is registered for push");
    else if (status === "error") logConnection("error", null, `Push registration failed${reason ? `: ${reason}` : ""}`);
    else if (status === "unavailable" && reason) logConnection("info", null, `Push unavailable: ${reason}`);
  }, [status, reason]);
}
