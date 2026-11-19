import { NextRequest, NextResponse } from "next/server";

// Proxies /api/account → Go backend /api/v1/account (balances, tier, usage),
// forwarding the caller's Bearer token.
const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

export async function GET(req: NextRequest) {
  const auth = req.headers.get("authorization") ?? "";
  try {
    const res = await fetch(`${GO_BACKEND_URL}/api/v1/account`, {
      headers: { Authorization: auth },
    });

    const body = await res.text();
    return new NextResponse(body, {
      status: res.status,
      headers: { "Content-Type": res.headers.get("content-type") ?? "application/json" },
    });
  } catch (e) {
    const msg = e instanceof Error ? e.message : "upstream error";
    return NextResponse.json({ error: msg }, { status: 502 });
  }
}
