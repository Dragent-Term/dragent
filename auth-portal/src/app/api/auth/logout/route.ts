import { NextResponse } from "next/server";
import { bearerFrom, clearSessionCookie, sameOrigin } from "@/lib/server/session";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// POST /api/auth/logout: ends this browser's session on the server (so the
// token stops working even if it was copied) and drops the session cookie.
export async function POST(request: Request) {
  if (!sameOrigin(request)) {
    return NextResponse.json({ error: "Cross-site request refused" }, { status: 403 });
  }
  const auth = bearerFrom(request);
  if (!auth) {
    const res = NextResponse.json({ ok: true });
    clearSessionCookie(res);
    return res;
  }
  try {
    const res = await fetch(new URL("/query", GO_BACKEND_URL), {
      method: "POST",
      cache: "no-store",
      signal: AbortSignal.timeout(10_000),
      headers: { Authorization: auth, "Content-Type": "application/json" },
      body: JSON.stringify({ query: "mutation { logout }" }),
    });
    const body = await res.json().catch(() => null);
    const ok = res.ok && body?.data?.logout === true;
    // The cookie goes either way: this browser is signed out.
    const out = NextResponse.json({ ok }, { status: ok ? 200 : 502 });
    clearSessionCookie(out);
    return out;
  } catch (error: unknown) {
    console.error("Logout proxy failed:", error);
    const out = NextResponse.json({ ok: false }, { status: 502 });
    clearSessionCookie(out);
    return out;
  }
}
