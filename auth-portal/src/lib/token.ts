/**
 * Where the access token lives in the browser.
 *
 * `localStorage`, not `sessionStorage`: sessionStorage is scoped to the tab and
 * is discarded when it closes, which is why signing in again was needed on
 * every visit. localStorage survives restarts, so a returning user stays signed
 * in until the token expires or they sign out.
 *
 * Trade-off worth knowing: localStorage is readable by any script on this
 * origin, so an XSS becomes a token theft. An httpOnly cookie would remove that
 * class of attack, but every server route here forwards an `Authorization:
 * Bearer` header the client supplies, so moving to cookies is a larger change.
 * This is the deliberate interim.
 *
 * Read/write only through this module — the token used to be touched directly
 * in three separate components, which is how the dashboard and the WASM viewer
 * ended up able to disagree about whether anyone was signed in.
 */

const KEY = "dragent_token";

/** The stored token, or null. Safe to call during SSR (returns null). */
export function getToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(KEY);
  } catch {
    // Private-browsing modes can throw on access rather than return null.
    return null;
  }
}

export function setToken(token: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(KEY, token);
  } catch {
    /* storage unavailable — the session stays in memory for this page only */
  }
}

export function clearToken(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(KEY);
    // Also clear the old location so a user upgrading from a build that used
    // sessionStorage does not keep a stale token in the tab.
    window.sessionStorage.removeItem(KEY);
  } catch {
    /* nothing to clear */
  }
}

/**
 * One-time migration for anyone mid-session when this shipped: promote a token
 * left in sessionStorage into localStorage so they are not signed out by the
 * upgrade itself.
 */
export function adoptLegacySessionToken(): string | null {
  if (typeof window === "undefined") return null;
  try {
    const legacy = window.sessionStorage.getItem(KEY);
    if (legacy && !window.localStorage.getItem(KEY)) {
      window.localStorage.setItem(KEY, legacy);
      window.sessionStorage.removeItem(KEY);
      return legacy;
    }
  } catch {
    /* ignore */
  }
  return null;
}
