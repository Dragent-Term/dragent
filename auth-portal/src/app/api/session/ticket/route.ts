import { NextResponse } from "next/server";
import { bearerFrom, sameOrigin } from "@/lib/server/session";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// POST /api/session/ticket → {ticket, expires_in}: a one-minute ticket the
// browser viewer joins /realtime with, so the access token (in the httpOnly
// session cookie) never goes into a WebSocket URL. 401 when signed out; the
// viewer then joins anonymously, authorised by the link's secret.
export async function POST(request: Request) {
  if (!sameOrigin(request)) {
    return NextResponse.json({ error: "Cross-site request refused" }, { status: 403 });
  }
  const auth = bearerFrom(request);
  if (!auth) return NextResponse.json({ error: "Not signed in" }, { status: 401 });
  try {
    const res = await fetch(`${GO_BACKEND_URL}/realtime/ticket`, {
      method: "POST",
      cache: "no-store",
      signal: AbortSignal.timeout(10_000),
      headers: { Authorization: auth },
    });
    if (!res.ok) return NextResponse.json({ error: "No ticket" }, { status: res.status === 401 ? 401 : 502 });
    const body = await res.json();
    return NextResponse.json(
      { ticket: body.ticket, expires_in: body.expires_in },
      { headers: { "Cache-Control": "no-store" } }
    );
  } catch (error: unknown) {
    console.error("Realtime ticket proxy failed:", error);
    return NextResponse.json({ error: "No ticket" }, { status: 502 });
  }
}
