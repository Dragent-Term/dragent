// Same-origin avatar proxy for the wasm session viewer.
//
// Participant avatars are OAuth-provider CDN URLs (Google / GitHub / Gravatar).
// The web viewer draws them on a wgpu canvas, which means it must `fetch()` the
// raw bytes — and cross-origin CDNs don't reliably send CORS headers, so a
// direct fetch is blocked and the avatar falls back to initials. Fetching them
// here (server-side, no CORS) and serving from our own origin fixes that.
//
// GET /api/avatar?url=<https-cdn-url> -> the image bytes, same-origin.

// Allowlisted avatar hosts — an open proxy would be an SSRF hole, so only the
// providers we actually issue avatar URLs from are permitted.
const ALLOWED_HOSTS = [
  "googleusercontent.com",
  "githubusercontent.com",
  "avatars.githubusercontent.com",
  "gravatar.com",
  "secure.gravatar.com",
];

function hostAllowed(hostname: string): boolean {
  const h = hostname.toLowerCase();
  return ALLOWED_HOSTS.some((a) => h === a || h.endsWith(`.${a}`));
}

export async function GET(request: Request) {
  const raw = new URL(request.url).searchParams.get("url");
  if (!raw) {
    return new Response("missing url", { status: 400 });
  }

  let target: URL;
  try {
    target = new URL(raw);
  } catch {
    return new Response("bad url", { status: 400 });
  }
  if (target.protocol !== "https:" || !hostAllowed(target.hostname)) {
    return new Response("host not allowed", { status: 400 });
  }

  let upstream: Response;
  try {
    upstream = await fetch(target, {
      headers: { Accept: "image/*" },
      redirect: "follow",
    });
  } catch {
    return new Response("upstream fetch failed", { status: 502 });
  }
  if (!upstream.ok) {
    return new Response("upstream error", { status: upstream.status });
  }

  const contentType = upstream.headers.get("content-type") ?? "image/jpeg";
  if (!contentType.startsWith("image/")) {
    return new Response("not an image", { status: 415 });
  }

  const body = await upstream.arrayBuffer();
  return new Response(body, {
    headers: {
      "Content-Type": contentType,
      // Avatars change rarely; let the browser cache them for a day.
      "Cache-Control": "public, max-age=86400",
    },
  });
}
