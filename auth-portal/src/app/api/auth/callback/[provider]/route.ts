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

    // Call the Go backend with redirect: "manual" to catch the 302 redirect
    const goRes = await fetch(
      `${GO_BACKEND_URL}/auth/callback/${provider}?code=${code}&state=${state}`,
      {
        method: "GET",
        redirect: "manual",
      }
    );

    const location = goRes.headers.get("location");
    console.log(`Go backend OAuth callback status for ${provider}:`, goRes.status, "location:", location);

    if (location && (location.startsWith("dragent://") || location.includes("/callback"))) {
      // Parse query params from the redirect URL
      const callbackUrl = new URL(location.replace("dragent://", "http://temp-placeholder.local/"));
      const refresh_token = callbackUrl.searchParams.get("refresh_token");
      const redirect_state = callbackUrl.searchParams.get("state");
      const user_id = callbackUrl.searchParams.get("user_id");

      if (refresh_token) {
        return NextResponse.json({
          success: true,
          refresh_token,
          state: redirect_state || state,
          user_id,
        });
      }
    }

    // Read details if error occurred
    const errorText = await goRes.text();
    return NextResponse.json(
      { error: `Go backend authentication failed: ${errorText || "Invalid credentials"}` },
      { status: 401 }
    );
  } catch (error: unknown) {
    // The detail stays in the server log; the browser gets a plain message.
    console.error("OAuth callback proxy failed:", error);
    return NextResponse.json(
      { error: "Failed to verify social login" },
      { status: 500 }
    );
  }
}
