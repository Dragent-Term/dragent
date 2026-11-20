import { NextResponse } from "next/server";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// POST /api/auth/logout with the Bearer token: ends this browser's session on
// the server, so the token stops working even if it was copied.
export async function POST(request: Request) {
  const auth = request.headers.get("authorization") ?? "";
  if (!auth) return NextResponse.json({ ok: true });
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
    return NextResponse.json({ ok }, { status: ok ? 200 : 502 });
  } catch (error: unknown) {
    console.error("Logout proxy failed:", error);
    return NextResponse.json({ ok: false }, { status: 502 });
  }
}
