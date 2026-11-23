/**
 * The pending "Open in Dragent" hand-off.
 *
 * The desktop app opens /login?scheme=dragent&state=<uuid> (a Dev build:
 * scheme=dragent-dev). Sign-in now always
 * finishes on /dashboard, so the deep-link payload has to survive one
 * navigation — that is all this is.
 *
 * sessionStorage on purpose, unlike the access token: this is a one-shot,
 * tab-scoped value. Persisting it would re-prompt "Open in Dragent" on a plain
 * dashboard visit days later, and it carries the app's sign-in code (or, for
 * older desktop builds, its refresh token), which should not outlive the
 * hand-off.
 */

const KEY = "dragent_desktop_handoff";

export type DesktopHandoff = {
  scheme: string;
  state: string;
  /** One-time code the app redeems with its PKCE verifier (current builds). */
  code?: string;
  /** The app's refresh token itself (desktop builds from before PKCE). */
  refresh_token?: string;
  user_id: string;
};

/** The app's half of a sign-in response: its one-time code, or (older
 * builds) its refresh token. Never both. */
export function appPart(data: { handoff_code?: unknown; refresh_token?: unknown }): {
  code?: string;
  refresh_token?: string;
} {
  if (typeof data.handoff_code === "string" && data.handoff_code) return { code: data.handoff_code };
  if (typeof data.refresh_token === "string" && data.refresh_token) return { refresh_token: data.refresh_token };
  return {};
}

/**
 * The app's URL scheme when none (or an unknown one) is given: the installed
 * Dragent.
 */
export const APP_SCHEME = "dragent";

/**
 * The only schemes a sign-in may be handed to: the installed Dragent and a
 * Dragent Dev build. The hand-off carries a sign-in code, so `?scheme=` is
 * checked against this exact list: anything else (`evilapp`, `javascript`,
 * `Dragent-Dev`, `dragent-dev:`) falls back to `dragent`.
 */
export const APP_SCHEMES = ["dragent", "dragent-dev"] as const;
export type AppScheme = (typeof APP_SCHEMES)[number];

/** `raw` if it is exactly an allowed scheme, else `dragent`. */
export function appScheme(raw: unknown): AppScheme {
  return (APP_SCHEMES as readonly unknown[]).includes(raw) ? (raw as AppScheme) : APP_SCHEME;
}

/** The dragent:// (or dragent-dev://) link that hands a finished sign-in to the app. */
export function appDeepLink(h: {
  scheme?: string;
  state: string;
  code?: string;
  refresh_token?: string;
  user_id?: string;
}): string {
  const part = h.code
    ? `code=${encodeURIComponent(h.code)}`
    : `refresh_token=${encodeURIComponent(h.refresh_token || "")}`;
  return `${appScheme(h.scheme)}://auth/callback?${part}&state=${encodeURIComponent(h.state)}&user_id=${encodeURIComponent(h.user_id || "")}`;
}

const SCHEME_KEY_PREFIX = "dragent_handoff_scheme:";

/**
 * Remember which app started the sign-in with this state, so the scheme
 * survives the round trips that leave /login (a provider taking over the
 * whole tab, a magic link opened in another tab of this browser). localStorage
 * because those land in a new tab; it holds only the scheme name, never a
 * code or token.
 */
export function rememberScheme(state: string, scheme: unknown): void {
  if (typeof window === "undefined" || !state) return;
  try {
    const s = appScheme(scheme);
    if (s === APP_SCHEME) window.localStorage.removeItem(SCHEME_KEY_PREFIX + state);
    else window.localStorage.setItem(SCHEME_KEY_PREFIX + state, s);
  } catch {
    /* storage unavailable — the hand-off falls back to dragent:// */
  }
}

/**
 * The scheme remembered for this state (checked against the allow-list again),
 * or `dragent`. Forgets it: one sign-in, one hand-off.
 */
export function schemeForState(state: string | null | undefined): AppScheme {
  if (typeof window === "undefined" || !state) return APP_SCHEME;
  try {
    const key = SCHEME_KEY_PREFIX + state;
    const s = appScheme(window.localStorage.getItem(key));
    window.localStorage.removeItem(key);
    return s;
  } catch {
    return APP_SCHEME;
  }
}

export function stashHandoff(h: DesktopHandoff): void {
  if (typeof window === "undefined") return;
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify({ ...h, scheme: appScheme(h.scheme) }));
  } catch {
    /* storage unavailable — the dashboard simply will not offer the deep link */
  }
}

/** Read it. Does not clear: the dashboard shows the prompt until dismissed. */
export function readHandoff(): DesktopHandoff | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.sessionStorage.getItem(KEY);
    if (!raw) return null;
    const h = JSON.parse(raw) as DesktopHandoff;
    return h && h.state && (h.code || h.refresh_token) ? { ...h, scheme: appScheme(h.scheme) } : null;
  } catch {
    return null;
  }
}

export function clearHandoff(): void {
  if (typeof window === "undefined") return;
  try {
    window.sessionStorage.removeItem(KEY);
  } catch {
    /* nothing to clear */
  }
}
