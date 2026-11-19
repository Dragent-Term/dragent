import { NextResponse } from "next/server";
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
    const { email, state, first_name, last_name, is_registration } = body;

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
      body: JSON.stringify({ email, state, first_name, last_name, is_registration }),
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

    // Call the Go backend with redirect: "manual" to catch the 302 redirect
    const goRes = await fetch(
      `${GO_BACKEND_URL}/auth/magic?token=${token}&state=${state}`,
      {
        method: "GET",
        redirect: "manual",
      }
    );

    const location = goRes.headers.get("location");
    console.log("Go backend magic link redirect status:", goRes.status, "location:", location);

    if (location && (location.startsWith("dragent://") || location.includes("/callback"))) {
      // Parse query params from the redirect URL
      const callbackUrl = new URL(location.replace("dragent://", "http://temp-placeholder.local/"));
      const refresh_token = callbackUrl.searchParams.get("refresh_token");
      const redirect_state = callbackUrl.searchParams.get("state");
      const user_id = callbackUrl.searchParams.get("user_id");

      if (refresh_token) {
        // The backend's redirect only carries a REFRESH token, but the dashboard
        // authenticates with a Bearer ACCESS token — so verifying a magic link
        // was not by itself enough to browse signed in.
        //
        // VerifyMagicLink has already called AuthorizeState by the time it
        // redirects, so the access token is sitting under `poll:<state>`. Read
        // it the same way the desktop app and the polling /login tab do.
        // GetAuthorizedState is a plain GET with a 10-minute TTL, not a
        // consuming read, so this does not steal the session from either of
        // them (see internal/identity/model/redis.go).
        let access_token: string | null = null;
        try {
          const sessionRes = await fetch(
            `${GO_BACKEND_URL}/auth/desktop/session?state=${encodeURIComponent(
              redirect_state || state
            )}`,
            { cache: "no-store" }
          );
          if (sessionRes.ok) {
            access_token = (await sessionRes.json()).access_token ?? null;
          }
        } catch (err) {
          // Non-fatal: the link itself verified. The client reports that it
          // cannot open the dashboard rather than pretending sign-in failed.
          console.error("Magic link verified but access-token lookup failed:", err);
        }

        return NextResponse.json({
          success: true,
          access_token,
          refresh_token,
          state: redirect_state || state,
          user_id,
        });
      }
    }

    // If redirect didn't have the token, return error
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
