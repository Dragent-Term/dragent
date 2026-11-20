import type { NextRequest, NextResponse } from "next/server";

/**
 * The browser's session lives in an httpOnly cookie that only this app's
 * server reads: page scripts never see the access token, so an XSS can't
 * steal it. The API routes here forward it to the Go backend as a Bearer
 * header. SameSite=Lax keeps other sites from sending it on their POSTs.
 */
export const SESSION_COOKIE = "dragent_session";

// The access token's own lifetime (24h); the server refuses it after that
// anyway, this just stops the browser keeping it longer.
const MAX_AGE_SECONDS = 24 * 60 * 60;

export function setSessionCookie(res: NextResponse, accessToken: string): void {
  res.cookies.set(SESSION_COOKIE, accessToken, {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    path: "/",
    maxAge: MAX_AGE_SECONDS,
  });
}

export function clearSessionCookie(res: NextResponse): void {
  res.cookies.set(SESSION_COOKIE, "", {
    httpOnly: true,
    secure: process.env.NODE_ENV === "production",
    sameSite: "lax",
    path: "/",
    maxAge: 0,
  });
}

/** The Authorization header to send upstream, from the session cookie. */
export function bearerFrom(req: Request | NextRequest): string {
  const cookie = "cookies" in req && typeof req.cookies?.get === "function"
    ? req.cookies.get(SESSION_COOKIE)?.value
    : readCookie(req.headers.get("cookie") ?? "", SESSION_COOKIE);
  return cookie ? `Bearer ${cookie}` : "";
}

function readCookie(header: string, name: string): string | undefined {
  for (const part of header.split(";")) {
    const [k, ...v] = part.trim().split("=");
    if (k === name) return decodeURIComponent(v.join("="));
  }
  return undefined;
}

/**
 * Refuse state-changing requests from other sites. SameSite=Lax already keeps
 * the cookie off cross-site POSTs; this is the second lock.
 */
export function sameOrigin(req: Request): boolean {
  const origin = req.headers.get("origin");
  if (!origin) return true; // same-origin fetches from older browsers, server calls
  let host: string;
  try {
    host = new URL(origin).host;
  } catch {
    return false;
  }
  // Behind a proxy the public host may be in either header.
  return [req.headers.get("x-forwarded-host"), req.headers.get("host")]
    .flatMap((h) => (h ? h.split(",").map((v) => v.trim()) : []))
    .includes(host);
}
