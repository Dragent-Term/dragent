import { NextResponse } from "next/server";
import { sameOrigin, setSessionCookie } from "@/lib/server/session";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// POST /api/auth/adopt {token}: one-time move for browsers signed in before
// the session moved to an httpOnly cookie. The token they kept in
// localStorage is checked with the server, put in the cookie, and the page
// deletes its copy. Remove once those sessions have expired (24h after
// this ships).
export async function POST(request: Request) {
  if (!sameOrigin(request)) {
    return NextResponse.json({ error: "Cross-site request refused" }, { status: 403 });
  }
  let token = "";
  try {
    const body = await request.json();
    token = typeof body?.token === "string" ? body.token : "";
  } catch {
    /* falls through */
  }
  if (!token || token.length > 4096) return NextResponse.json({ ok: false }, { status: 400 });
  try {
    const check = await fetch(`${GO_BACKEND_URL}/api/v1/account`, {
      cache: "no-store",
      signal: AbortSignal.timeout(10_000),
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!check.ok) return NextResponse.json({ ok: false }, { status: 401 });
    const res = NextResponse.json({ ok: true });
    setSessionCookie(res, token);
    return res;
  } catch {
    return NextResponse.json({ ok: false }, { status: 502 });
  }
}
