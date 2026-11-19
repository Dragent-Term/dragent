/**
 * "+ New tab" on a live workspace: the desktop's new-tab menu items and the
 * pure rules around opening one from the phone (which workspaces can, when
 * the new tab has reached the live catalog, when the route asks for the
 * menu). No UI and no transport here.
 */

import type { RelayNewTabKind } from "../transport/relayRpcMethods";

export interface NewTabMenuItem {
  readonly kind: RelayNewTabKind;
  readonly title: string;
  readonly detail: string;
}

/** The desktop header menu's items, word for word (only these three). */
export const NEW_TAB_MENU_ITEMS: readonly NewTabMenuItem[] = Object.freeze([
  Object.freeze({ kind: "terminal", title: "Terminal", detail: "A shell in this workspace" }),
  Object.freeze({ kind: "browser", title: "Browser", detail: "Preview your app or open any site" }),
  Object.freeze({ kind: "dragent_cli", title: "Dragent CLI", detail: "Script workspaces, agents and automations" }),
] as const);

/** How long the phone waits for the new tab to show up in the live catalog. */
export const NEW_TAB_APPEAR_TIMEOUT_MS = 10_000;

export const NEW_TAB_NOT_SUPPORTED_MESSAGE =
  "This desktop shares an older catalog, so tabs can't be opened from the phone.";
export const NEW_TAB_TIMEOUT_MESSAGE =
  "The desktop opened the tab, but it hasn't appeared here yet. Check the tab list in a moment.";
export const NEW_TAB_FAILED_MESSAGE = "The desktop couldn't open a tab. Try again.";

/**
 * The desktop opens tabs only in workspaces it published with a wire id.
 * Legacy path-keyed workspaces (no id) can't be targeted.
 */
export function newTabWorkspaceId(workspace: { readonly id: string | null }): string | null {
  return typeof workspace.id === "string" && /^[A-Za-z0-9]{1,64}$/.test(workspace.id) ? workspace.id : null;
}

export type NewTabWait = "ready" | "waiting" | "timeout";

/**
 * Where a pending new tab stands: in the catalog (open it), still on its way,
 * or late past the deadline (say so).
 */
export function newTabWaitStatus(
  catalogTabIds: readonly number[],
  tabId: number,
  startedAt: number,
  now: number,
  timeoutMs: number = NEW_TAB_APPEAR_TIMEOUT_MS,
): NewTabWait {
  if (catalogTabIds.includes(tabId)) return "ready";
  return now - startedAt >= timeoutMs ? "timeout" : "waiting";
}

/** `newTab=1` on the workspace route opens the menu on arrival, once. */
export function shouldOpenNewTabMenuOnArrival(
  param: string | string[] | undefined,
  alreadyOpened: boolean,
): boolean {
  return !alreadyOpened && param === "1";
}
