import { NextRequest } from "next/server";
import { proxyForward } from "@/lib/forwardProxy";

// Browser-facing proxy for a host-approved session forward. The capability is
// the session share secret; it is passed only to the Go gateway and is never
// forwarded to the desktop service.
type Params = { params: Promise<{ forwardID: string; path?: string[] }> };

export const GET = (req: NextRequest, ctx: Params) =>
  proxyForward(req, ctx, "/forward");
export const POST = (req: NextRequest, ctx: Params) =>
  proxyForward(req, ctx, "/forward");
export const PUT = (req: NextRequest, ctx: Params) =>
  proxyForward(req, ctx, "/forward");
export const PATCH = (req: NextRequest, ctx: Params) =>
  proxyForward(req, ctx, "/forward");
export const DELETE = (req: NextRequest, ctx: Params) =>
  proxyForward(req, ctx, "/forward");
