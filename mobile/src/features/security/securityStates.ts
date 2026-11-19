/**
 * Phone-side §9 failure screens (docs/design/e2ee.md §9), pure mapping.
 *
 * No React, no native imports: `securityStateFor` turns transport state into
 * a screen descriptor, so node tests can assert every §9 phone state and its
 * exact wording without loading the socket lifecycle. Rendering lives in
 * `SecurityScreen.tsx`.
 *
 * Wording is the design's, verbatim except for the `<desktop>` slot, which
 * takes the desktop's display name.
 */

export type SecurityScreenKind =
  /** §9: the desktop key differs from the phone's pin. Pair again / Remove. */
  | "key-changed"
  /** §9: the phone app predates the cut-over (relay-error updateRequired). */
  | "update-app"
  /** §9: the desktop predates E2EE. Set when the server says so (E2a flag). */
  | "update-desktop"
  /** §9: the phone is unknown/revoked, or paired before the cut-over. */
  | "pair-again"
  /** §6.3/§9: a same-account desktop the phone has no pin for. No name. */
  | "pair-desktop"
  /** §9: the handshake keeps failing. Backoff, never a downgrade. */
  | "cannot-connect"
  /** §9: the pin store failed its check. Fail closed, pair again. */
  | "store-lost"
  /** §9: frames went missing; the channel closed and reconnects. Transient. */
  | "interrupted";

export type SecurityScreenAction = "pair-again" | "remove-desktop" | "retry";

export interface SecurityScreenState {
  readonly kind: SecurityScreenKind;
  readonly title: string;
  readonly body: string;
  readonly actions: readonly SecurityScreenAction[];
  /** Transient notices auto-retry; they render inline, not as a screen. */
  readonly transient: boolean;
}

export interface SecurityLinkStatus {
  readonly state: "connecting" | "open" | "retrying" | "unpaired" | "lost" | "unavailable" | "closed";
  readonly problem: "interrupted" | "handshake" | "refused" | null;
}

export interface SecurityInputs {
  /** The E2EE link status for the targeted desktop, if any. */
  readonly secure?: SecurityLinkStatus | null | undefined;
  /** The relay socket error, if any (carries the §10 refusal wording). */
  readonly socketError?: string | null | undefined;
  /** True when this desktop previously held an open channel (§9 key change). */
  readonly wasOpen?: boolean | undefined;
  /** True when the server reports this desktop predates E2EE (E2a flag). */
  readonly desktopLegacy?: boolean | undefined;
  /** Display name for the `<desktop>` slot. */
  readonly desktopName?: string | undefined;
}

/** §10 refusal wording, kept next to the transport constants they mirror. */
export const UPDATE_APP_MESSAGE = "Update the Dragent app on this phone.";
export const PAIR_AGAIN_MESSAGE =
  "Pair this phone again to keep using it. Phone access is now end-to-end encrypted.";

export const STORE_LOST_MESSAGE =
  "Dragent's security records on this device couldn't be verified, so paired devices were reset. Pair again.";

function screen(
  kind: SecurityScreenKind,
  title: string,
  body: string,
  actions: readonly SecurityScreenAction[],
  transient = false,
): SecurityScreenState {
  return Object.freeze({ kind, title, body, actions: Object.freeze(actions), transient });
}

/**
 * Which §9 phone screen (if any) the person should see. Order matters: the
 * pin-store check comes first (fail closed), then the admission refusals,
 * then the link state.
 */
export function securityStateFor(input: SecurityInputs): SecurityScreenState | null {
  const name = (input.desktopName ?? "").trim() || "this desktop";
  const secure = input.secure ?? null;

  if (secure?.state === "lost") {
    return screen("store-lost", "Pair again", STORE_LOST_MESSAGE, ["pair-again"]);
  }
  if (input.socketError === UPDATE_APP_MESSAGE) {
    return screen("update-app", UPDATE_APP_MESSAGE, "Phone access is now end-to-end encrypted.", ["retry"]);
  }
  if (input.socketError === PAIR_AGAIN_MESSAGE) {
    return screen("pair-again", PAIR_AGAIN_MESSAGE, "", ["pair-again"]);
  }
  if (secure?.state === "unpaired") {
    // §6.3: a same-account desktop that isn't paired shows with no name.
    return screen("pair-desktop", "Pair this desktop to open it.", "", ["pair-again"]);
  }
  if (secure?.problem === "refused") {
    return screen(
      "pair-again",
      `This phone isn't paired with ${name} any more.`,
      "Pair again.",
      ["pair-again"],
    );
  }
  if (input.desktopLegacy === true) {
    return screen("update-desktop", `Update Dragent on ${name} to connect.`, "", ["retry"]);
  }
  if (secure?.problem === "handshake") {
    if (input.wasOpen === true) {
      return screen(
        "key-changed",
        "This desktop's security key changed.",
        "If you reinstalled Dragent or reset its keys, pair again from Settings → Mobile on that desktop. If you didn't, don't continue.",
        ["pair-again", "remove-desktop"],
      );
    }
    return screen(
      "cannot-connect",
      `Can't make a secure connection to ${name}.`,
      "Retrying…",
      ["retry"],
    );
  }
  if (secure?.problem === "interrupted") {
    return screen(
      "interrupted",
      `Connection to ${name} was interrupted.`,
      "Reconnecting…",
      ["retry"],
      true,
    );
  }
  return null;
}

/** Every §9 phone screen kind, for the preview harness and wording tests. */
export const SECURITY_SCREEN_KINDS: readonly SecurityScreenKind[] = Object.freeze([
  "key-changed",
  "update-app",
  "update-desktop",
  "pair-again",
  "pair-desktop",
  "cannot-connect",
  "store-lost",
  "interrupted",
]);
