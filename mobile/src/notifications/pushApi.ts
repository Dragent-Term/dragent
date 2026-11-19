/**
 * Remote push HTTP against the trusted gateway origin (account JWT only).
 *
 * Pure transport: no React Native imports, so `tsx --test` can exercise it
 * with an injected fetch. Bearer header only — tokens never enter URLs.
 * Every call resolves a fresh account JWT; bodies and responses are bounded.
 */

export type PushPlatform = "apns" | "fcm";

export interface PushFetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  signal?: AbortSignal;
}

export interface PushFetchResponse {
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
}

export type PushFetchFn = (url: string, init?: PushFetchInit) => Promise<PushFetchResponse>;

const PUSH_TIMEOUT_MS = 15_000;
const PUSH_MAX_BODY_CHARS = 64 * 1024;

export class PushHttpError extends Error {
  constructor(
    message: string,
    readonly status: number | null = null,
    readonly loginRequired = false,
  ) {
    super(message);
    this.name = "PushHttpError";
  }
}

/**
 * Derive the push HTTPS origin from the trusted relay gateway endpoint.
 * Mirrors `resolveRelayCatalogURL`: wss→https, ws→http (http only for
 * loopback dev), credentials/query/hash rejected. Returns the origin only.
 */
export function resolvePushBaseURL(gateway: string): string | null {
  let url: URL;
  try {
    url = new URL(gateway);
  } catch {
    return null;
  }
  if (url.username || url.password || url.search || url.hash) return null;
  if (url.protocol === "wss:") url.protocol = "https:";
  else if (url.protocol === "ws:") url.protocol = "http:";
  else if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  if (url.protocol === "http:" && !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname)) {
    return null;
  }
  return url.origin;
}

async function postJSON(
  fetchFn: PushFetchFn,
  url: string,
  accessToken: string,
  body: Record<string, unknown>,
): Promise<unknown> {
  const encoded = JSON.stringify(body);
  if (encoded.length > PUSH_MAX_BODY_CHARS) throw new PushHttpError("The push request is too large.");
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), PUSH_TIMEOUT_MS);
  try {
    const response = await fetchFn(url, {
      method: "POST",
      headers: { Accept: "application/json", "Content-Type": "application/json" , Authorization: `Bearer ${accessToken}` },
      body: encoded,
      signal: abort.signal,
    });
    const text = await response.text();
    if (text.length > PUSH_MAX_BODY_CHARS) throw new PushHttpError("The push response was unreadable.");
    let data: unknown = null;
    try {
      data = text ? (JSON.parse(text) as unknown) : null;
    } catch {
      throw new PushHttpError("The push response was unreadable.");
    }
    if (!response.ok) {
      const message = typeof data === "object" && data !== null && "error" in data && typeof (data as { error?: unknown }).error === "string"
        ? (data as { error: string }).error
        : "The push service could not complete this request.";
      if (response.status === 401) throw new PushHttpError("Pair this phone to manage notifications.", 401, true);
      if (response.status === 403) throw new PushHttpError("This device cannot manage these notifications.", 403, false);
      if (response.status === 404) throw new PushHttpError("No notification registration was found.", 404, false);
      throw new PushHttpError(message || "The push service could not complete this request.", response.status, false);
    }
    return data;
  } catch (error) {
    if (error instanceof PushHttpError) throw error;
    throw new PushHttpError("Your gateway is unreachable. Check your connection and try again.");
  } finally {
    clearTimeout(timer);
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export interface PushRegistration {
  readonly deviceId: string;
  readonly platform: PushPlatform;
  readonly expiresAt: string;
}

function parseRegistration(data: unknown): PushRegistration | null {
  if (!isRecord(data) || data.registered !== true) return null;
  if (typeof data.device_id !== "string" || typeof data.platform !== "string" || typeof data.expires_at !== "string") {
    return null;
  }
  if (data.platform !== "apns" && data.platform !== "fcm") return null;
  if (!data.device_id || !data.expires_at) return null;
  return Object.freeze({ deviceId: data.device_id, platform: data.platform, expiresAt: data.expires_at });
}

/**
 * Register (or renew) this device. `deviceId` is omitted on purpose: the
 * server infers it from the verified access-token lease, so mobile never
 * decodes the JWT. An explicit mismatch is still 403 server-side.
 */
export async function registerPush(
  fetchFn: PushFetchFn,
  baseURL: string,
  getAccessToken: () => Promise<string | null>,
  input: { platform: PushPlatform; pushToken: string; foreground: boolean },
): Promise<PushRegistration> {
  const token = await getAccessToken();
  if (!token) throw new PushHttpError("Pair this phone to manage notifications.", 401, true);
  // Push tokens travel only in the JSON body, never in URLs or logs.
  if (!input.pushToken.trim()) throw new PushHttpError("The device token is missing.");
  const data = await postJSON(fetchFn, `${baseURL}/v1/push/register`, token, {
    platform: input.platform,
    push_token: input.pushToken,
    foreground: input.foreground,
  });
  const parsed = parseRegistration(data);
  if (!parsed) throw new PushHttpError("The push response was unreadable.");
  return parsed;
}

/** Remove this device's lease. Best-effort: callers clear local state anyway. */
export async function unregisterPush(
  fetchFn: PushFetchFn,
  baseURL: string,
  getAccessToken: () => Promise<string | null>,
): Promise<void> {
  const token = await getAccessToken();
  if (!token) throw new PushHttpError("Pair this phone to manage notifications.", 401, true);
  await postJSON(fetchFn, `${baseURL}/v1/push/unregister`, token, {});
}
