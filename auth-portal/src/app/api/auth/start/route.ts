import { NextResponse } from "next/server";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// OAuth popup entry point. The login page opens THIS (same-origin) URL in the
// popup and we redirect to the Go backend's /auth/start. This keeps
// NEXT_BACKEND_URL server-side only: client components cannot read non-
// NEXT_PUBLIC_ env vars (they see `undefined`, which previously sent the popup
// to `<ui-domain>/undefined/auth/start` — the "API call routed to the UI
// domain" bug).
export async function GET(request: Request) {
  const { searchParams } = new URL(request.url);
  const provider = searchParams.get("provider");
  const state = searchParams.get("state");

  if (!provider || !state) {
    return NextResponse.json(
      { error: "provider and state parameters are required" },
      { status: 400 }
    );
  }

  const target = new URL(`${GO_BACKEND_URL}/auth/start`);
  target.searchParams.set("provider", provider);
  target.searchParams.set("state", state);
  return NextResponse.redirect(target, 307);
}
