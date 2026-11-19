import { NextRequest, NextResponse } from "next/server";
import {
  FORWARD_SANDBOX_POLICY,
  dedicatedForwardURL,
  forwardDomain,
  sandboxedForwardResponseHeaders,
  upstreamForwardHeaders,
} from "@/lib/forwardSandbox";

type ForwardParams = {
  params: Promise<{ forwardID: string; path?: string[] }>;
};

const GO_BACKEND_URL = process.env.NEXT_BACKEND_URL || "http://localhost:8080";

export async function proxyForward(
  req: NextRequest,
  ctx: ForwardParams,
  publicPrefix: string,
) {
  const { forwardID, path = [] } = await ctx.params;
  const incoming = new URL(req.url);

  // Forwards are user-controlled scripts. When a dedicated forward domain is
  // configured, serve them only there; the proxy below is the fallback.
  const dedicated = dedicatedForwardURL(
    forwardDomain(process.env),
    forwardID,
    path,
    incoming.search,
  );
  if (dedicated) {
    const response = NextResponse.redirect(dedicated, 307);
    response.headers.set("content-security-policy", FORWARD_SANDBOX_POLICY);
    return response;
  }

  const secret = incoming.searchParams.get("secret");
  const upstream = new URL(
    `${GO_BACKEND_URL}/forward/${encodeURIComponent(forwardID)}/${path
      .map((segment) => encodeURIComponent(segment).replace(/%40/gi, "@"))
      .join("/")}`,
  );
  upstream.search = incoming.search;
  upstream.searchParams.delete("secret");
  const headers = upstreamForwardHeaders(req.headers, secret);

  try {
    const response = await fetch(upstream, {
      method: req.method,
      headers,
      body: req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer(),
      redirect: "manual",
    });
    // Served on the portal's own origin: the sandbox (no allow-same-origin)
    // keeps the forward's scripts away from the portal's token and cookies.
    const out = sandboxedForwardResponseHeaders(response.headers);

    const contentType = out.get("content-type") || "";
    const lowerContentType = contentType.toLowerCase();
    if (
      lowerContentType.includes("text/html") ||
      lowerContentType.includes("javascript") ||
      lowerContentType.includes("text/css")
    ) {
      const source = await response.text();
      const body = rewriteRootAssetUrls(
        source,
        `${publicPrefix}/${forwardID}`,
        secret,
        lowerContentType.includes("text/html"),
      );
      return new NextResponse(body, { status: response.status, headers: out });
    }

    return new NextResponse(response.body, { status: response.status, headers: out });
  } catch (error) {
    const message = error instanceof Error ? error.message : "forward proxy failed";
    const failed = NextResponse.json({ error: message }, { status: 502 });
    failed.headers.set("content-security-policy", FORWARD_SANDBOX_POLICY);
    return failed;
  }
}

// Dev servers commonly emit <script src="/src/main.tsx"> and
// <script src="/@react-refresh">. Those paths must stay inside this forward;
// otherwise the browser requests them from the portal origin.
function rewriteRootAssetUrls(
  source: string,
  prefix: string,
  secret: string | null,
  html: boolean,
) {
  const pattern = html
    ? /(\b(?:src|href|action)\s*=\s*["'])\/(?![\/"'#])([^"']*)(["'])/gi
    : /(["'`])\/(?![\/"'#])([^"'`\s]+)(["'`])/g;

  const rewrittenAttributes = source.replace(pattern, (_match, start: string, path: string, end: string) => {
    const target = new URL(path, "http://forward.invalid");
    if (!target.pathname || target.pathname === "/") return _match;
    if (secret) target.searchParams.set("secret", secret);
    return `${start}${prefix}${target.pathname}${target.search}${target.hash}${end}`;
  });

  // Vite also injects root-absolute imports inside inline module scripts,
  // notably: import ... from "/@react-refresh". Rewrite those as well.
  if (!html) return rewrittenAttributes;
  return rewrittenAttributes.replace(
    /(["'`])\/(?![\/"'#])([^"'`\s]+)(["'`])/g,
    (_match, start: string, path: string, end: string) => {
      const target = new URL(path, "http://forward.invalid");
      if (!target.pathname || target.pathname === "/") return _match;
      if (secret) target.searchParams.set("secret", secret);
      return `${start}${prefix}${target.pathname}${target.search}${target.hash}${end}`;
    },
  );
}
