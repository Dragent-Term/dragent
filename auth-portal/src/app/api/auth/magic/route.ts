import { NextResponse } from "next/server";
import { setSessionCookie } from "@/lib/server/session";
import { visitorIP } from "@/lib/visitorIp";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// True when NEXT_BACKEND_URL was missing at runtime — the usual cause of
// "fetch failed" when a host only exposes env vars at build time (persist
// them into .env.production at build).
const usingFallback = !process.env.NEXT_BACKEND_URL;

// Shared with the API (AUTH_PORTAL_PROXY_SECRET there too). This route calls
// the API server-to-server, so the API would otherwise see every visitor as
// this server's IP; the secret lets it trust the visitor IP we pass along
// for its per-IP sign-in rate limit.
const PORTAL_PROXY_SECRET = process.env.AUTH_PORTAL_PROXY_SECRET || "";

// POST /api/auth/magic -> Request a magic link from Go backend
export async function POST(request: Request) {
  try {
    const body = await request.json();
    const { email, state, first_name, last_name, is_registration, challenge } = body;

    if (!email || !state) {
      return NextResponse.json(
        { error: "Email and state parameters are required" },
        { status: 400 }
      );
    }

    const goRes = await fetch(`${GO_BACKEND_URL}/auth/magic/request`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(PORTAL_PROXY_SECRET
          ? {
              "X-Dragent-Portal-Secret": PORTAL_PROXY_SECRET,
              "X-Dragent-Client-IP": visitorIP(request.headers),
            }
          : {}),
      },
      body: JSON.stringify({ email, state, first_name, last_name, is_registration, challenge }),
    });

    if (goRes.status === 429) {
      const retryAfter = goRes.headers.get("retry-after");
      return NextResponse.json(
        { error: "Too many sign-in attempts. Please wait a few minutes and try again." },
        { status: 429, headers: retryAfter ? { "Retry-After": retryAfter } : undefined }
      );
    }

    if (!goRes.ok) {
      const errorBody = await goRes.text();
      let errorJson: { error?: string } = {};
      try {
        errorJson = JSON.parse(errorBody);
      } catch {
        errorJson = { error: errorBody };
      }
      return NextResponse.json(
        { error: errorJson.error || errorBody },
        { status: goRes.status }
      );
    }

    return NextResponse.json({ success: true });
  } catch (error: unknown) {
    // The detail (and how the server is configured) stays in the server
    // log; the browser gets a plain message.
    console.error(
      `Magic link request proxy failed (backend=${GO_BACKEND_URL}, env_set=${!usingFallback}):`,
      error
    );
    return NextResponse.json(
      { error: "Failed to submit magic link request" },
      { status: 500 }
    );
  }
}

// GET /api/auth/magic -> Verify a magic link token with Go backend
export async function GET(request: Request) {
  try {
    const { searchParams } = new URL(request.url);
    const token = searchParams.get("token");
    const state = searchParams.get("state");

    if (!token || !state) {
      return NextResponse.json(
        { error: "Token and state parameters are required" },
        { status: 400 }
      );
    }

    // Asked for JSON, the Go backend answers with the session itself: an
    // access token for this browser and a separate refresh token for the
    // Dragent app. (The app's copy is also parked for it to collect by
    // polling, once the user approves it on the dashboard.)
    const goRes = await fetch(
      `${GO_BACKEND_URL}/auth/magic?token=${encodeURIComponent(token)}&state=${encodeURIComponent(state)}`,
      { method: "GET", redirect: "manual", cache: "no-store", headers: { Accept: "application/json" } }
    );

    if (goRes.ok) {
      const data = await goRes.json();
      // The app's part: a one-time code when the desktop started with a
      // PKCE challenge, or (older desktop builds) its refresh token.
      if (data?.access_token && (data?.handoff_code || data?.refresh_token)) {
        // This browser's session goes into the httpOnly cookie, not the
        // page: scripts on the page never see the access token.
        const res = NextResponse.json({
          success: true,
          ...(data.handoff_code
            ? { handoff_code: data.handoff_code }
            : { refresh_token: data.refresh_token }),
          state: data.state || state,
          user_id: data.user_id,
        });
        setSessionCookie(res, data.access_token);
        return res;
      }
    }

    return NextResponse.json(
      { error: "Failed to verify magic link token. Link might be expired or invalid." },
      { status: 401 }
    );
  } catch (error: unknown) {
    // The detail stays in the server log; the browser gets a plain message.
    console.error("Magic link verification proxy failed:", error);
    return NextResponse.json(
      { error: "Failed to verify magic link" },
      { status: 500 }
    );
  }
}
