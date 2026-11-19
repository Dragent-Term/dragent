import { RelayRpcClient, RelayRpcError } from "./relayRpcClient";

/**
 * Pure companion-session lease helpers (no React, no native imports).
 *
 * Lifecycle contract: fixed 24h logical session from creation. Reconnect and
 * JWT refresh never extend expiry. `relay-ready` carries the lease
 * (`session_id`, `session_expires_at`, `access_expires_at` Unix ms); a ready
 * without them is a legacy server and stays untracked. Code 4408 ends the
 * logical session with no automatic new lease; 4401/4403 stay credential
 * failures with bounded recovery that always resumes (never mints fresh).
 */

/**
 * Canonical logical-session lease. The repository duplicates this shape for
 * now; root removes the duplicate once stable (structural typing keeps the
 * client compatible either way).
 */
export interface CompanionSessionLease {
  readonly sessionId: string;
  readonly expiresAt: number;
}

export interface ReadySessionInfo {
  readonly sessionId: string;
  readonly sessionExpiresAt: number;
  readonly accessExpiresAt: number;
}

export interface CompanionSessionStore {
  loadSession(): Promise<CompanionSessionLease | null>;
  saveSession(lease: CompanionSessionLease): Promise<void>;
  clearSession(): Promise<void>;
}

export const COMPANION_SESSION_LIMITS = Object.freeze({
  /** Proactive redial margin aligns the auth controller's 60s refresh margin. */
  proactiveMarginMs: 60_000,
  /** Bounded 4401 recovery attempts before latching credential failure. */
  authRecoveryMax: 3,
  sessionIdMax: 128,
});

export const SESSION_ENDED_MESSAGE =
  "This session has ended. Start a new session to continue.";

export const SESSION_STORE_UNAVAILABLE_MESSAGE =
  "The saved session is unavailable. Try again.";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function validId(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[A-Za-z0-9-]{1,128}$/.test(value)
  );
}

function validTimestamp(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

export function isValidSessionId(value: unknown): value is string {
  return validId(value);
}

export type ReadySession =
  /** No session fields at all: legacy server, stays untracked. */
  | { readonly kind: "legacy" }
  /** Complete lease: tracked, persisted, resumed. */
  | { readonly kind: "lease"; readonly lease: ReadySessionInfo }
  /**
   * Partial or malformed session metadata: fail closed. The client must not
   * admit this as legacy — that would discard the lease and bypass expiry.
   */
  | { readonly kind: "invalid" };

/**
 * Read the lease from a `relay-ready` frame. Legacy only when all three
 * fields are absent. `access_expires_at` is validated independently: near
 * the 24h end the remaining JWT may outlive the logical session, so
 * access > session must not discard the lease — the client schedules the
 * minimum of the deadlines instead.
 */
export function parseReadySession(raw: unknown): ReadySession {
  if (!isRecord(raw)) return { kind: "invalid" };
  const { session_id, session_expires_at, access_expires_at } = raw;
  if (session_id === undefined && session_expires_at === undefined && access_expires_at === undefined) {
    return { kind: "legacy" };
  }
  if (!validId(session_id)) return { kind: "invalid" };
  if (!validTimestamp(session_expires_at) || !validTimestamp(access_expires_at)) {
    return { kind: "invalid" };
  }
  return {
    kind: "lease",
    lease: Object.freeze({
      sessionId: session_id,
      sessionExpiresAt: session_expires_at as number,
      accessExpiresAt: access_expires_at as number,
    }),
  };
}

/** Invalid timestamps fail closed: treated as expired, never renewed. */
export function isLeaseExpired(expiresAt: number, now: number): boolean {
  if (!validTimestamp(expiresAt) || !Number.isFinite(now)) return true;
  return expiresAt <= now;
}

/**
 * Milliseconds until the client should proactively redial with a fresh
 * account token: the minimum of the JWT proactive deadline and the absolute
 * logical expiry. Null when already past either — the session timer (expiry)
 * or the 4401 bounded recovery (lapsed access) owns those paths instead of a
 * tight redial loop. Firing exactly at logical expiry is safe: admission
 * rechecks the lease and ends instead of renewing.
 */
export function redialDelayMs(
  sessionExpiresAt: number,
  accessExpiresAt: number,
  now: number,
  marginMs: number = COMPANION_SESSION_LIMITS.proactiveMarginMs,
): number | null {
  if (
    !validTimestamp(sessionExpiresAt) ||
    !validTimestamp(accessExpiresAt) ||
    !Number.isFinite(now) ||
    !Number.isFinite(marginMs)
  ) {
    return null;
  }
  const delay = Math.min(accessExpiresAt - marginMs, sessionExpiresAt) - now;
  return delay > 0 ? delay : null;
}

/**
 * First-frame session fields: the selected desktop binds user + target
 * (absent for discovery), and the held resume id resumes the same logical
 * lease. Never invents: no resume without a held unexpired lease.
 */
export function buildAuthSessionFields(
  deviceId: string | null,
  resumeSessionId: string | null,
): { readonly device_id?: string; readonly resume_session_id?: string } {
  const fields: { device_id?: string; resume_session_id?: string } = {};
  if (typeof deviceId === "string" && deviceId.trim() !== "") fields.device_id = deviceId.trim();
  if (typeof resumeSessionId === "string" && validId(resumeSessionId)) {
    fields.resume_session_id = resumeSessionId;
  }
  return Object.freeze(fields);
}

/**
 * `companion.session.revoke` — explicit revocation of the current session.
 * Empty params: the session comes from the socket, no caller id is accepted.
 * Correlated success resolves; the server then closes 4408, which the
 * client surfaces as ended while retaining the lease.
 */
export function revokeCompanionSession(
  client: RelayRpcClient,
  opts?: { timeoutMs?: number },
): Promise<void> {
  return client.request("companion.session.revoke", {}, opts).then((raw: unknown) => {
    if (!isRecord(raw)) throw new RelayRpcError("The relay response was unreadable.");
  });
}
