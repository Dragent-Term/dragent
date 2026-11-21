import { NextRequest, NextResponse } from "next/server";
import { bearerFrom, sameOrigin } from "@/lib/server/session";

// Same-origin gateway for organization APIs. Keeping the Go URL here means the
// browser only ever talks to the portal and the Bearer token is forwarded by a
// server route, matching the existing account and billing proxies.
const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

async function proxy(req: NextRequest, path: string[]) {
  if (req.method !== "GET" && req.method !== "HEAD" && !sameOrigin(req)) {
    return NextResponse.json({ error: "Cross-site request refused" }, { status: 403 });
  }
  const target = `${GO_BACKEND_URL}/api/v1/${path.map(encodeURIComponent).join("/")}${new URL(req.url).search}`;
  const init: RequestInit = {
    method: req.method,
    headers: {
      Authorization: bearerFrom(req),
      "Content-Type": "application/json",
    },
  };
  if (req.method !== "GET" && req.method !== "HEAD") init.body = await req.text();

  try {
    const res = await fetch(target, init);
    // A 204 response is required to have no body. Passing even an empty string
    // into Response/NextResponse makes Next throw "Invalid response status
    // code 204", which was breaking set-active and other successful actions.
    const body = res.status === 204 || res.status === 304 ? null : await res.text();
    return new NextResponse(body, {
      status: res.status,
      headers: { "Content-Type": res.headers.get("content-type") ?? "application/json" },
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : "upstream error";
    return NextResponse.json({ error: message }, { status: 502 });
  }
}

type Context = { params: Promise<{ path: string[] }> };
export async function GET(req: NextRequest, ctx: Context) { return proxy(req, (await ctx.params).path); }
export async function POST(req: NextRequest, ctx: Context) { return proxy(req, (await ctx.params).path); }
export async function PATCH(req: NextRequest, ctx: Context) { return proxy(req, (await ctx.params).path); }
export async function DELETE(req: NextRequest, ctx: Context) { return proxy(req, (await ctx.params).path); }
