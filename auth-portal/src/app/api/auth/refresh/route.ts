const BACKEND = process.env.NEXT_BACKEND_URL || "http://localhost:8080";
const headers = { "Cache-Control": "no-store" };
const mutation = `mutation MobileRefresh($token: String!) {
  refreshToken(refreshToken: $token) {
    accessToken refreshToken expiresIn user { id email displayName }
  }
}`;

/** Mobile refresh uses the existing Go rotation flow, with no tokens in URLs. */
export async function POST(request: Request) {
  let body: unknown;
  try { body = await request.json(); } catch {
    return Response.json({ error: "Invalid request" }, { status: 400, headers });
  }
  if (!body || typeof body !== "object" || !("refreshToken" in body) ||
      typeof body.refreshToken !== "string" || !body.refreshToken || body.refreshToken.length > 4096) {
    return Response.json({ error: "A refresh token is required" }, { status: 400, headers });
  }
  try {
    const response = await fetch(new URL("/query", BACKEND), {
      method: "POST", cache: "no-store", signal: AbortSignal.timeout(15_000),
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ query: mutation, variables: { token: body.refreshToken } }),
    });
    if (!response.ok) {
      return Response.json({ error: "Account service unavailable" }, { status: 502, headers });
    }
    const result = await response.json();
    if (Array.isArray(result?.errors) && result.errors.length) {
      const rejected = result.errors.some((error: { message?: unknown }) =>
        typeof error?.message === "string" && /invalid or expired refresh token|refresh token has been revoked/i.test(error.message));
      return Response.json({ error: rejected ? "Sign-in expired" : "Account service unavailable" }, { status: rejected ? 401 : 502, headers });
    }
    if (!result?.data?.refreshToken) {
      return Response.json({ error: "Account service unavailable" }, { status: 502, headers });
    }
    return Response.json(result.data.refreshToken, { headers });
  } catch {
    return Response.json({ error: "Account service unavailable" }, { status: 502, headers });
  }
}
