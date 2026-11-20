import { NextResponse } from "next/server";

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// POST /api/auth/desktop/approve {state} with the signed-in Bearer token:
// lets the Dragent app waiting on this sign-in collect it.
export async function POST(request: Request) {
  const auth = request.headers.get("authorization") ?? "";
  let state = "";
  try {
    const body = await request.json();
    state = typeof body?.state === "string" ? body.state : "";
  } catch {
    /* falls through to the 400 */
  }
  if (!auth || !state || state.length > 256) {
    return NextResponse.json({ error: "Sign in and try again." }, { status: 400 });
  }
  try {
    const res = await fetch(`${GO_BACKEND_URL}/auth/desktop/approve`, {
      method: "POST",
      cache: "no-store",
      headers: { Authorization: auth, "Content-Type": "application/json" },
      body: JSON.stringify({ state }),
    });
    if (res.ok) return NextResponse.json({ approved: true });
    return NextResponse.json(
      { error: res.status === 404 ? "This sign-in has expired. Start again from the app." : "Couldn't approve. Try again." },
      { status: res.status === 404 ? 404 : 502 }
    );
  } catch (error: unknown) {
    console.error("Desktop approval proxy failed:", error);
    return NextResponse.json({ error: "Couldn't approve. Try again." }, { status: 502 });
  }
}
