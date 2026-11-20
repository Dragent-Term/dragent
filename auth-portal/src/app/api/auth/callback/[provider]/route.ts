import { NextResponse } from "next/server";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

export async function GET(
  request: Request,
  { params }: { params: Promise<{ provider: string }> }
) {
  try {
    const { provider } = await params;
    const { searchParams } = new URL(request.url);
    const code = searchParams.get("code");
    const state = searchParams.get("state");

    if (!code || !state) {
      return NextResponse.json(
        { error: "Code and state parameters are required" },
        { status: 400 }
      );
    }

    if (provider !== "github" && provider !== "google") {
      return NextResponse.json(
        { error: `Unsupported auth provider: ${provider}` },
        { status: 400 }
      );
    }

    // Asked for JSON, the Go backend answers with the session itself
    // instead of a redirect to the app.
    const goRes = await fetch(
      `${GO_BACKEND_URL}/auth/callback/${provider}?code=${encodeURIComponent(code)}&state=${encodeURIComponent(state)}`,
      { method: "GET", redirect: "manual", cache: "no-store", headers: { Accept: "application/json" } }
    );

    if (goRes.ok) {
      const data = await goRes.json();
      if (data?.access_token && data?.refresh_token) {
        return NextResponse.json({
          success: true,
          access_token: data.access_token,
          refresh_token: data.refresh_token,
          state: data.state || state,
          user_id: data.user_id,
        });
      }
    }

    return NextResponse.json({ error: "Sign-in failed. Please try again." }, { status: 401 });
  } catch (error: unknown) {
    // The detail stays in the server log; the browser gets a plain message.
    console.error("OAuth callback proxy failed:", error);
    return NextResponse.json(
      { error: "Failed to verify social login" },
      { status: 500 }
    );
  }
}
