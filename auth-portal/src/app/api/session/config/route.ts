import { NextResponse } from "next/server";

// The Go backend base URL is server-side only (client components can't read
// non-NEXT_PUBLIC env vars), so we expose the derived realtime gateway URL here.
const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

// GET /api/session/config -> { gateway } where gateway is the wss:// (or ws://)
// URL of the shared-session realtime WebSocket the browser connects to directly.
export async function GET() {
  let gateway: string;
  try {
    const u = new URL(GO_BACKEND_URL);
    u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
    u.pathname = "/realtime";
    u.search = "";
    gateway = u.toString();
  } catch {
    gateway = "ws://localhost:8080/realtime";
  }
  return NextResponse.json({ gateway });
}
