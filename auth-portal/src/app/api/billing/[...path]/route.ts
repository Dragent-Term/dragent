import { NextRequest, NextResponse } from "next/server";
import { bearerFrom, sameOrigin } from "@/lib/server/session";

// Server-side only. Proxies /api/billing/* → Go backend /api/v1/billing/*,
// forwarding the session cookie as a Bearer token (the billing endpoints require it). The
// Go backend URL is never exposed to the browser.
const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

async function proxy(req: NextRequest, path: string[]) {
  const search = new URL(req.url).search;
  const target = `${GO_BACKEND_URL}/api/v1/billing/${path.join("/")}${search}`;
  const auth = bearerFrom(req);
  if (req.method !== "GET" && req.method !== "HEAD" && !sameOrigin(req)) {
    return NextResponse.json({ error: "Cross-site request refused" }, { status: 403 });
  }

  const init: RequestInit = {
    method: req.method,
    headers: { Authorization: auth, "Content-Type": "application/json" },
  };
  if (req.method !== "GET" && req.method !== "HEAD") {
    init.body = await req.text();
  }

  try {
    const res = await fetch(target, init);
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

export async function GET(req: NextRequest, ctx: { params: Promise<{ path: string[] }> }) {
  return proxy(req, (await ctx.params).path);
}

export async function POST(req: NextRequest, ctx: { params: Promise<{ path: string[] }> }) {
  return proxy(req, (await ctx.params).path);
}
