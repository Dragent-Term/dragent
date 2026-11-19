// Isolation rules for session forwards served through the portal. Forwards are
// arbitrary HTML and JavaScript from a user's local dev server; the portal
// keeps the signed-in user's access token in localStorage. Forwarded content
// must therefore never run with the portal's origin. Kept free of Next imports
// so it can be unit tested with plain Node.

// The sandbox deliberately omits allow-same-origin: the forwarded document gets
// an opaque origin and cannot read the portal's localStorage, cookies or
// same-origin responses.
export const FORWARD_SANDBOX_POLICY =
  "sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads";

export const FORWARD_COOKIE = "__Host-dragent_forward";

// Same slug format the Go gateway accepts (components.IsForwardID), restricted
// to characters that are safe as a DNS label before it is put in a hostname.
const FORWARD_ID = /^[a-z0-9]{1,50}-[0-9a-f]{6}$/;

export function isForwardID(id: string): boolean {
  return FORWARD_ID.test(id);
}

// The dedicated forward domain, e.g. "dragent-preview.dev", or "" when the
// portal proxy is the only way to reach a forward. It reads the backend's
// DRAGENT_FORWARD_DOMAIN, with NEXT_PUBLIC_FORWARD_DOMAIN as an alternative.
export function forwardDomain(env: Record<string, string | undefined>): string {
  const raw = env.DRAGENT_FORWARD_DOMAIN || env.NEXT_PUBLIC_FORWARD_DOMAIN || "";
  const domain = raw.trim().toLowerCase().replace(/\.$/, "");
  // A bare hostname only: anything else would make the redirect target
  // attacker-shaped or simply wrong.
  return /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(domain)
    ? domain
    : "";
}

// Where a portal forward URL lives on the dedicated forward host, or null when
// it has to fall back to the sandboxed portal proxy.
export function dedicatedForwardURL(
  domain: string,
  forwardID: string,
  path: string[],
  search: string,
): string | null {
  if (!domain || !isForwardID(forwardID)) return null;
  // Keep "@" literal: Vite serves /@vite/client and /@react-refresh.
  const encodedPath = path
    .map((segment) => encodeURIComponent(segment).replace(/%40/gi, "@"))
    .join("/");
  return `https://${forwardID}.${domain}/${encodedPath}${search}`;
}

// Headers sent to the Go gateway. The visitor's portal cookies and credentials
// must not reach the forward (the gateway relays headers to the user's local
// service). The share secret travels as the gateway's forward cookie, so the
// gateway authorizes the request without redirecting to set a cookie on the
// portal origin.
export function upstreamForwardHeaders(incoming: Headers, secret: string | null): Headers {
  const headers = new Headers(incoming);
  for (const name of ["host", "connection", "content-length", "cookie", "authorization"]) {
    headers.delete(name);
  }
  // Next's fetch transparently decompresses gzip/br/deflate responses but
  // retains the upstream Content-Encoding and Content-Length headers. Asking
  // the loopback app for an identity response avoids handing the browser a
  // decoded body that it attempts to decode for a second time.
  headers.set("accept-encoding", "identity");
  if (secret) headers.set("cookie", `${FORWARD_COOKIE}=${secret}`);
  return headers;
}

// Headers returned to the browser for a forwarded response. Upstream headers
// that could weaken the sandbox or plant state on the portal origin are
// removed, and the sandbox policy is applied.
export function sandboxedForwardResponseHeaders(upstream: Headers): Headers {
  const out = new Headers(upstream);
  for (const name of [
    "connection",
    "transfer-encoding",
    // fetch has already decoded the body, so neither remains valid for the
    // response streamed to the browser.
    "content-encoding",
    "content-length",
    // A forward must not set or overwrite cookies on the portal origin.
    "set-cookie",
    // Replaced by the sandbox policy below.
    "content-security-policy",
    "content-security-policy-report-only",
    "service-worker-allowed",
    "access-control-allow-origin",
    "access-control-allow-credentials",
  ]) {
    out.delete(name);
  }
  out.set("content-security-policy", FORWARD_SANDBOX_POLICY);
  // The sandboxed page has an opaque origin, so its module scripts and fetches
  // back into this forward are cross-origin (Origin: null). They carry no
  // credentials; the share secret is in each rewritten URL.
  out.set("access-control-allow-origin", "*");
  out.set("x-content-type-options", "nosniff");
  return out;
}
