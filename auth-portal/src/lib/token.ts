/**
 * The browser session, as page scripts see it: not at all.
 *
 * The access token lives in an httpOnly cookie that only this app's server
 * reads (src/lib/server/session.ts); the API routes forward it to the Go
 * backend. Scripts on the page, an injected one included, can't read it.
 *
 * What's left here is a harmless "signed in" hint in localStorage, so other
 * tabs notice a sign-in (the `storage` event) and the UI knows whether to
 * try, plus a one-time move of tokens that older builds kept in storage.
 */

const HINT = "dragent_signed_in";
// Where builds before the cookie kept the token itself.
const LEGACY = "dragent_token";

/** An opaque marker meaning "this browser has a session cookie". */
export const SESSION = "cookie";

export function markSignedIn(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(HINT, String(Date.now()));
  } catch {
    /* storage unavailable: the cookie still works */
  }
}

export function markSignedOut(): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(HINT);
    window.localStorage.removeItem(LEGACY);
    window.sessionStorage.removeItem(LEGACY);
  } catch {
    /* nothing to clear */
  }
}

/** The storage key other tabs watch for a sign-in. */
export const SIGNED_IN_KEY = HINT;

/**
 * Move a token an older build left in storage into the session cookie, then
 * delete it from storage. Resolves once done (or when there was nothing).
 */
export async function adoptLegacyToken(): Promise<void> {
  if (typeof window === "undefined") return;
  let token: string | null = null;
  try {
    token = window.localStorage.getItem(LEGACY) ?? window.sessionStorage.getItem(LEGACY);
  } catch {
    return;
  }
  if (!token) return;
  try {
    const res = await fetch("/api/auth/adopt", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ token }),
    });
    if (res.ok) markSignedIn();
  } catch {
    /* offline: the old token is dropped anyway, signing in again fixes it */
  } finally {
    try {
      window.localStorage.removeItem(LEGACY);
      window.sessionStorage.removeItem(LEGACY);
    } catch {
      /* nothing to clear */
    }
  }
}
