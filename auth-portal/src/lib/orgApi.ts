"use client";

/**
 * Authenticated fetch against this app's own /api/organization/* gateway
 * (never the Go backend directly). Every call site passes the FULL
 * remaining backend path, e.g. api("/api/organization/organizations") or
 * api("/api/organization/me/active-organization") — the route handler at
 * src/app/api/organization/[...path]/route.ts forwards everything after
 * /api/organization/ straight onto /api/v1/*.
 */
export class ApiError extends Error {}

// The session travels as the httpOnly cookie; `session` only marks that
// there is one (callers re-create the client when it changes).
export function apiFetch(session: string | null | undefined) {
  void session;
  return async (path: string, init?: RequestInit) => {
    const res = await fetch(path, {
      ...init,
      headers: {
        ...(init?.headers ?? {}),
        "Content-Type": "application/json",
      },
    });
    if (res.status === 204) return null;
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new ApiError(data?.error ?? `request failed (${res.status})`);
    return data;
  };
}
