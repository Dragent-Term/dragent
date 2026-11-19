import assert from "node:assert/strict";
import { test } from "node:test";
import {
  COMPANION_SESSION_LIMITS,
  SESSION_ENDED_MESSAGE,
  SESSION_STORE_UNAVAILABLE_MESSAGE,
  buildAuthSessionFields,
  isLeaseExpired,
  isValidSessionId,
  parseReadySession,
  redialDelayMs,
  revokeCompanionSession,
} from "../src/transport/companionSession";
import { RelayRpcClient } from "../src/transport/relayRpcClient";

test("legacy ready without session fields stays untracked", () => {
  assert.deepEqual(parseReadySession({ type: "relay-ready", v: 1 }), { kind: "legacy" });
  assert.deepEqual(parseReadySession({ type: "relay-ready", v: 1, session_id: undefined }), { kind: "legacy" });
});

test("complete lease parses with independently valid timestamps", () => {
  const now = 1_000_000;
  const parsed = parseReadySession({
    type: "relay-ready", v: 1,
    session_id: "00000000-0000-4000-8000-000000000001",
    session_expires_at: now + 86_400_000,
    access_expires_at: now + 900_000,
  });
  assert.equal(parsed.kind, "lease");
  if (parsed.kind !== "lease") return;
  assert.equal(parsed.lease.sessionId, "00000000-0000-4000-8000-000000000001");
  assert.equal(parsed.lease.sessionExpiresAt, now + 86_400_000);
  assert.equal(parsed.lease.accessExpiresAt, now + 900_000);
});

test("access may outlive the logical session near the 24h end", () => {
  // Remaining JWT may exceed the logical expiry: accept, schedule min().
  const now = 1_000_000;
  const parsed = parseReadySession({
    type: "relay-ready", v: 1,
    session_id: "abc-123",
    session_expires_at: now + 60_000,
    access_expires_at: now + 900_000,
  });
  assert.equal(parsed.kind, "lease");
});

test("partial or malformed session metadata fails closed, never legacy", () => {
  for (const raw of [
    null, [], "ready",
    { type: "relay-ready", v: 1, session_id: "abc-123" },
    { type: "relay-ready", v: 1, session_expires_at: 1, access_expires_at: 2 },
    { type: "relay-ready", v: 1, session_id: "", session_expires_at: 1, access_expires_at: 2 },
    { type: "relay-ready", v: 1, session_id: "abc 123!", session_expires_at: 1, access_expires_at: 2 },
    { type: "relay-ready", v: 1, session_id: "abc", session_expires_at: -5, access_expires_at: 2 },
    { type: "relay-ready", v: 1, session_id: "abc", session_expires_at: 1.5, access_expires_at: 2 },
    { type: "relay-ready", v: 1, session_id: "abc", session_expires_at: 1, access_expires_at: "soon" },
    { type: "relay-ready", v: 1, session_id: "x".repeat(129), session_expires_at: 1, access_expires_at: 2 },
  ]) {
    assert.deepEqual(parseReadySession(raw), { kind: "invalid" }, `accepted: ${JSON.stringify(raw)}`);
  }
  assert.equal(isValidSessionId("abc-123"), true);
  assert.equal(isValidSessionId(""), false);
  assert.equal(isValidSessionId(null), false);
});

test("lease expiry fails closed on invalid bounds", () => {
  const now = 1_000_000;
  assert.equal(isLeaseExpired(now + 1000, now), false);
  assert.equal(isLeaseExpired(now, now), true);
  assert.equal(isLeaseExpired(now - 1, now), true);
  assert.equal(isLeaseExpired(-5, now), true);
  assert.equal(isLeaseExpired(Number.NaN, now), true);
});

test("proactive redial schedules min(JWT margin, logical expiry)", () => {
  const now = 1_000_000;
  const margin = COMPANION_SESSION_LIMITS.proactiveMarginMs;
  assert.equal(margin, 60_000);
  // JWT-bound deadline first.
  assert.equal(redialDelayMs(now + 86_400_000, now + 900_000, now), 900_000 - margin);
  // Logical expiry first near the 24h end: min wins, never past absolute end.
  assert.equal(redialDelayMs(now + 60_000, now + 900_000, now), 60_000);
  // Already past either deadline: no tight loop; session timer / 4401 own it.
  assert.equal(redialDelayMs(now - 1, now + 900_000, now), null);
  assert.equal(redialDelayMs(now + 86_400_000, now - 1, now), null);
  assert.equal(redialDelayMs(Number.NaN, now + 900_000, now), null);
});

test("auth session fields bind device and resume only when held", () => {
  assert.deepEqual(buildAuthSessionFields("desktop-1", null), { device_id: "desktop-1" });
  assert.deepEqual(buildAuthSessionFields(null, "sess-1"), { resume_session_id: "sess-1" });
  assert.deepEqual(buildAuthSessionFields("desktop-1", "sess-1"), { device_id: "desktop-1", resume_session_id: "sess-1" });
  // Discovery carries neither; blank/invalid values are never invented.
  assert.deepEqual(buildAuthSessionFields(null, null), {});
  assert.deepEqual(buildAuthSessionFields("  ", "sess-1"), { resume_session_id: "sess-1" });
  assert.deepEqual(buildAuthSessionFields("desktop-1", "not a uuid!"), { device_id: "desktop-1" });
  assert.deepEqual(buildAuthSessionFields("desktop-1", ""), { device_id: "desktop-1" });
});

test("session messages are user-safe and specific", () => {
  assert.match(SESSION_ENDED_MESSAGE, /ended/i);
  assert.match(SESSION_STORE_UNAVAILABLE_MESSAGE, /unavailable|try again/i);
  for (const message of [SESSION_ENDED_MESSAGE, SESSION_STORE_UNAVAILABLE_MESSAGE]) {
    assert.doesNotMatch(message, /token|secret|jwt|credential/i);
  }
});

test("revoke sends the bare current-session method and resolves on correlated success", async () => {
  let ids = 0;
  const sent: Record<string, unknown>[] = [];
  const client = new RelayRpcClient({
    send: (frame) => { sent.push(frame); return true; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
  });
  const done = revokeCompanionSession(client);
  const frame = sent.at(-1)!;
  assert.equal(frame.type, "rpc-request");
  assert.equal(frame.method, "companion.session.revoke");
  assert.deepEqual(frame.params, {});
  client.handleMessage({ type: "rpc-reply", v: 1, id: frame.id, ok: true, result: { revoked: true } });
  await done;
  assert.equal(client.pendingCount(), 0);
  client.close();
});

test("revoke rejects unreadable results and passes auth failures through", async () => {
  let ids = 0;
  const sent: Record<string, unknown>[] = [];
  const client = new RelayRpcClient({
    send: (frame) => { sent.push(frame); return true; },
    createRequestId: () => `00000000-0000-4000-8000-${String(++ids).padStart(12, "0")}`,
  });
  const bad = revokeCompanionSession(client);
  const malformed = sent.at(-1)!;
  client.handleMessage({ type: "rpc-reply", v: 1, id: malformed.id, ok: true, result: [1, 2] });
  await assert.rejects(bad, /unreadable/);
  const denied = revokeCompanionSession(client);
  const forbidden = sent.at(-1)!;
  client.handleMessage({
    type: "rpc-reply", v: 1, id: forbidden.id, ok: false,
    error: { message: "Sign in to connect to this machine.", loginRequired: true },
  });
  await assert.rejects(denied, (error: unknown) => {
    assert.match((error as Error).message, /Sign in/);
    return true;
  });
  client.close();
});
