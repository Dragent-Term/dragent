/**
 * The pending "Open in Dragent" hand-off.
 *
 * The desktop app opens /login?scheme=dragent&state=<uuid>. Sign-in now always
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

/** The dragent:// link that hands a finished sign-in to the app. */
export function appDeepLink(h: { scheme: string; state: string; code?: string; refresh_token?: string; user_id?: string }): string {
  const part = h.code
    ? `code=${encodeURIComponent(h.code)}`
    : `refresh_token=${encodeURIComponent(h.refresh_token || "")}`;
  return `${h.scheme}://auth/callback?${part}&state=${encodeURIComponent(h.state)}&user_id=${encodeURIComponent(h.user_id || "")}`;
}

export function stashHandoff(h: DesktopHandoff): void {
  if (typeof window === "undefined") return;
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify(h));
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
    return h && h.state && (h.code || h.refresh_token) ? h : null;
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
