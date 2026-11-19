/**
 * Pairing HTTP against the gateway the QR names (pure transport: no React
 * Native imports, so `tsx --test` can exercise it with an injected fetch).
 *
 * Contract endpoints (all JSON, docs/design/e2ee.md F11/§6.2):
 * - POST /v1/companion/pairings/redeem {code, platform} (ios|android only;
 *   the device name never goes to the server — it travels inside the
 *   encrypted pairing payload, and a redeem carrying one is refused)
 * - POST /v1/companion/devices/token   {device_id, device_token}
 * - GET  /v1/companion/devices         (Bearer access token)
 * - DELETE /v1/companion/devices/{id}  (Bearer access token)
 */

/** Platform class only (§6.2 step 4). The web never redeems. */
export type PairingPlatform = "ios" | "android";

export class PairingError extends Error {
  constructor(
    message: string,
    readonly kind: "invalid" | "unauthorized" | "gone" | "rate_limited" | "offline" | "passcode",
  ) {
    super(message);
    this.name = "PairingError";
  }
}

export interface PairingFetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  signal?: AbortSignal;
}

export interface PairingFetchResponse {
  readonly ok: boolean;
  readonly status: number;
  text(): Promise<string>;
}

export type PairingFetchFn = (url: string, init?: PairingFetchInit) => Promise<PairingFetchResponse>;

export interface RedeemResult {
  readonly deviceId: string;
  readonly deviceToken: string;
  readonly user: { readonly id: string; readonly displayName: string };
  readonly gatewayUrl: string;
  readonly expiresAt: number;
}

export interface TokenResult {
  readonly accessToken: string;
  readonly accessExpiresAt: number;
  readonly deviceToken: string;
  readonly expiresAt: number;
}

export interface PairedPhone {
  readonly deviceId: string;
  readonly name: string;
  readonly platform: string;
  readonly createdAt: number;
  readonly lastSeenAt: number | null;
}

const PAIRING_TIMEOUT_MS = 15_000;
const PAIRING_MAX_BODY_CHARS = 64 * 1024;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Gateway dates are RFC3339; internal timestamps and mock responses use milliseconds. */
function parseTimestamp(value: unknown): number | null {
  const timestamp = typeof value === "number" ? value
    : typeof value === "string" && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(value)
      ? Date.parse(value) : NaN;
  return Number.isSafeInteger(timestamp) && timestamp > 0 ? timestamp : null;
}

function errorMessage(data: unknown, fallback: string): string {
  if (isRecord(data) && typeof data.error === "string" && data.error.trim()) return data.error;
  return fallback;
}

async function postJSON(fetchFn: PairingFetchFn, url: string, body: Record<string, unknown>, token?: string): Promise<{ status: number; data: unknown }> {
  const encoded = JSON.stringify(body);
  if (encoded.length > PAIRING_MAX_BODY_CHARS) {
    throw new PairingError("The pairing request is too large.", "invalid");
  }
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), PAIRING_TIMEOUT_MS);
  try {
    const headers: Record<string, string> = { Accept: "application/json", "Content-Type": "application/json" };
    if (token) headers.Authorization = `Bearer ${token}`;
    const response = await fetchFn(url, { method: "POST", headers, body: encoded, signal: abort.signal });
    const text = await response.text();
    if (text.length > PAIRING_MAX_BODY_CHARS) {
      throw new PairingError("The pairing response was too large.", "invalid");
    }
    let data: unknown = null;
    try {
      data = text ? (JSON.parse(text) as unknown) : null;
    } catch {
      throw new PairingError(`The pairing server returned an unexpected response (HTTP ${response.status}; not JSON).`, "invalid");
    }
    return { status: response.status, data };
  } catch (error) {
    if (error instanceof PairingError) throw error;
    throw new PairingError("Could not reach your desktop. Check your connection and try again.", "offline");
  } finally {
    clearTimeout(timer);
  }
}

function throwForStatus(status: number, data: unknown, goneMessage: string): never {
  if (status === 410) throw new PairingError(goneMessage, "gone");
  if (status === 429) throw new PairingError("Too many attempts. Wait a little and try again.", "rate_limited");
  if (status === 401) throw new PairingError("This phone's pairing is no longer valid. Pair again to reconnect.", "unauthorized");
  if (status === 400) throw new PairingError(errorMessage(data, "The pairing request was rejected. Try again."), "invalid");
  throw new PairingError(errorMessage(data, "The pairing request failed. Try again."), "offline");
}

function parseDeviceId(value: unknown): string {
  if (typeof value !== "string" || !value || value.length > 128) {
    throw new PairingError("The pairing server returned an invalid device identifier.", "invalid");
  }
  return value;
}

/** Single-use redeem: the first call wins, a second redeem of the same code is 410 gone. */
export async function redeemPairingCode(
  fetchFn: PairingFetchFn,
  gateway: string,
  input: { code: string; platform: PairingPlatform },
): Promise<RedeemResult> {
  const { status, data } = await postJSON(fetchFn, `${gateway}/v1/companion/pairings/redeem`, {
    code: input.code,
    platform: input.platform,
  });
  if (status === 200) {
    const expiresAt = isRecord(data) ? parseTimestamp(data.expires_at) : null;
    if (!isRecord(data) || typeof data.device_id !== "string" || !data.device_id ||
        typeof data.device_token !== "string" || !data.device_token ||
        !isRecord(data.user) || typeof data.user.id !== "string" || !data.user.id.trim() ||
        typeof data.gateway_url !== "string" || !data.gateway_url ||
        expiresAt === null) {
      const fields = !isRecord(data) ? ["response"] : [
        typeof data.device_id !== "string" || !data.device_id ? "device_id" : "",
        typeof data.device_token !== "string" || !data.device_token ? "device_token" : "",
        !isRecord(data.user) || typeof data.user.id !== "string" || !data.user.id.trim() ? "user.id" : "",
        typeof data.gateway_url !== "string" || !data.gateway_url ? "gateway_url" : "",
        expiresAt === null ? "expires_at" : "",
      ].filter(Boolean);
      throw new PairingError(`The pairing server returned invalid fields: ${fields.join(", ")}.`, "invalid");
    }
    const displayName = typeof data.user.display_name === "string" && data.user.display_name
      ? data.user.display_name
      : data.user.id;
    return Object.freeze({
      deviceId: parseDeviceId(data.device_id),
      deviceToken: data.device_token,
      user: Object.freeze({ id: data.user.id, displayName }),
      gatewayUrl: data.gateway_url,
      expiresAt,
    });
  }
  throwForStatus(status, data, "This code has already been used or has expired. On your desktop, open Settings → Mobile and pair again.");
}

/**
 * Mint a short-lived access token, rotating the device token on every call.
 * The caller persists the rotated device token before using the access
 * token; reusing a rotated-out token revokes the device server-side.
 */
export async function refreshDeviceToken(
  fetchFn: PairingFetchFn,
  gateway: string,
  input: { deviceId: string; deviceToken: string },
): Promise<TokenResult> {
  const { status, data } = await postJSON(fetchFn, `${gateway}/v1/companion/devices/token`, {
    device_id: input.deviceId,
    device_token: input.deviceToken,
  });
  if (status === 200) {
    const expiresAt = isRecord(data) ? parseTimestamp(data.expires_at) : null;
    const accessExpiresAt = isRecord(data) ? parseTimestamp(data.access_expires_at) : null;
    if (!isRecord(data) || typeof data.access_token !== "string" || !data.access_token ||
        accessExpiresAt === null ||
        typeof data.device_token !== "string" || !data.device_token ||
        expiresAt === null) {
      throw new PairingError("The pairing server returned invalid credentials during token refresh.", "invalid");
    }
    return Object.freeze({
      accessToken: data.access_token,
      accessExpiresAt,
      deviceToken: data.device_token,
      expiresAt,
    });
  }
  throwForStatus(status, data, "This phone's pairing is no longer valid. Pair again to reconnect.");
}

async function authorized(
  fetchFn: PairingFetchFn,
  method: string,
  url: string,
  accessToken: string,
  body?: Record<string, unknown>,
): Promise<{ status: number; data: unknown }> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), PAIRING_TIMEOUT_MS);
  try {
    const response = await fetchFn(url, {
      method,
      headers: { Accept: "application/json", ...(body ? { "Content-Type": "application/json" } : {}), Authorization: `Bearer ${accessToken}` },
      ...(body ? { body: JSON.stringify(body) } : {}),
      signal: abort.signal,
    });
    const text = await response.text();
    let data: unknown = null;
    try {
      data = text ? (JSON.parse(text) as unknown) : null;
    } catch {
      throw new PairingError("The pairing response was unreadable.", "invalid");
    }
    return { status: response.status, data };
  } catch (error) {
    if (error instanceof PairingError) throw error;
    throw new PairingError("Could not reach your desktop. Check your connection and try again.", "offline");
  } finally {
    clearTimeout(timer);
  }
}

/** Phones this pairing has authorized (desktop Settings → Mobile lists these). */
export async function listPairedPhones(fetchFn: PairingFetchFn, gateway: string, accessToken: string): Promise<readonly PairedPhone[]> {
  const { status, data } = await authorized(fetchFn, "GET", `${gateway}/v1/companion/devices`, accessToken);
  if (status !== 200) throwForStatus(status, data, "This phone's pairing is no longer valid. Pair again to reconnect.");
  if (!isRecord(data) || !Array.isArray(data.devices)) {
    throw new PairingError("The pairing response was unreadable.", "invalid");
  }
  const phones: PairedPhone[] = [];
  for (const entry of data.devices) {
    const createdAt = isRecord(entry) ? parseTimestamp(entry.created_at) : null;
    if (!isRecord(entry) || typeof entry.device_id !== "string" || !entry.device_id ||
        typeof entry.name !== "string" || typeof entry.platform !== "string" ||
        createdAt === null) continue;
    const lastSeen = entry.last_seen_at;
    phones.push(Object.freeze({
      deviceId: entry.device_id,
      name: entry.name,
      platform: entry.platform,
      createdAt,
      lastSeenAt: parseTimestamp(lastSeen),
    }));
  }
  return Object.freeze(phones);
}

/** Revoke one phone. Revoking this phone closes its relay sockets; token calls then 401. */
export async function revokePairedPhone(
  fetchFn: PairingFetchFn,
  gateway: string,
  accessToken: string,
  deviceId: string,
): Promise<void> {
  const { status, data } = await authorized(
    fetchFn, "DELETE", `${gateway}/v1/companion/devices/${encodeURIComponent(deviceId)}`, accessToken);
  if (status === 200 || status === 204) return;
  throwForStatus(status, data, "This phone's pairing is no longer valid. Pair again to reconnect.");
}
