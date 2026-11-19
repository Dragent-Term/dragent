import { NextResponse } from "next/server";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

export async function GET(request: Request) {
  try {
    const { searchParams } = new URL(request.url);
    const state = searchParams.get("state");

    if (!state) {
      return NextResponse.json(
        { error: "State parameter is required" },
        { status: 400 }
      );
    }

    const goRes = await fetch(`${GO_BACKEND_URL}/auth/desktop/session?state=${state}`, {
      method: "GET",
      cache: "no-store",
    });

    if (goRes.status === 204) {
      return NextResponse.json({ pending: true });
    }

    if (!goRes.ok) {
      return NextResponse.json({ pending: true });
    }

    const data = await goRes.json();
    return NextResponse.json({
      pending: false,
      access_token: data.access_token,
      refresh_token: data.refresh_token,
      user_id: data.user_id,
    });
  } catch (error: unknown) {
    console.error("Session status check failed:", error);
    // Return pending to avoid breaking the polling loop in the browser on transient errors
    return NextResponse.json({ pending: true });
  }
}
