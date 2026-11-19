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

    const goRes = await fetch(`${GO_BACKEND_URL}/auth/desktop/session?state=${state}`);
    
    if (goRes.status === 204) {
      return NextResponse.json({ pending: true });
    }

    if (!goRes.ok) {
      return NextResponse.json({ pending: true });
    }

    const data = await goRes.json();
    return NextResponse.json(data);
  } catch (error: unknown) {
    // The detail stays in the server log; the browser gets a plain message.
    console.error("Session status poll failed:", error);
    return NextResponse.json(
      { error: "Failed to check session status" },
      { status: 500 }
    );
  }
}
