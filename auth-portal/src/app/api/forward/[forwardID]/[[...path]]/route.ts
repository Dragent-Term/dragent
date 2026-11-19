import { NextRequest } from "next/server";
import { proxyForward } from "@/lib/forwardProxy";
type Params = { params: Promise<{ forwardID: string; path?: string[] }> };

export const GET = (req: NextRequest, ctx: Params) => proxy(req, ctx);
export const POST = (req: NextRequest, ctx: Params) => proxy(req, ctx);
export const PUT = (req: NextRequest, ctx: Params) => proxy(req, ctx);
export const PATCH = (req: NextRequest, ctx: Params) => proxy(req, ctx);
export const DELETE = (req: NextRequest, ctx: Params) => proxy(req, ctx);

async function proxy(req: NextRequest, ctx: Params) {
  return proxyForward(req, ctx, "/api/forward");
}
