/**
 * The pending "Open in Dragent" hand-off.
 *
 * The desktop app opens /login?scheme=dragent&state=<uuid>. Sign-in now always
 * finishes on /dashboard, so the deep-link payload has to survive one
 * navigation — that is all this is.
 *
 * sessionStorage on purpose, unlike the access token: this is a one-shot,
 * tab-scoped value. Persisting it would re-prompt "Open in Dragent" on a plain
 * dashboard visit days later, and it carries a refresh token, which should not
 * outlive the hand-off.
 */

const KEY = "dragent_desktop_handoff";

export type DesktopHandoff = {
  scheme: string;
  state: string;
  refresh_token: string;
  user_id: string;
};

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
    return h && h.state && h.refresh_token ? h : null;
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
