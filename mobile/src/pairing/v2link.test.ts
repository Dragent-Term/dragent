/**
 * §12.5 phone cases for v2 pairing links (docs/design/e2ee.md §6.2): the full
 * link parses, bare codes are impossible, and no secret ever enters
 * persisted navigation state.
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clearPendingPairLink,
  peekPendingPairLink,
  setPendingPairLink,
  takePendingPairLink,
} from "./pendingLink";
import {
  normalizeDeviceName,
  normalizePairingCode,
  isTrustedGateway,
  normalizePairingGateway,
  parsePairLink,
  wipeBytes,
} from "./pairing";
import { toBase64Url } from "../transport/e2ee/ctrl";
import { redirectSystemPath } from "../../app/+native-intent";

const CODE = "ABCDEFGH234567ABCDEF";
const GATEWAY = "https://api.example.test";
const DESKTOP = "0a0b0c0d-0000-4000-8000-0000000000d1";

function bytes(fill: number): Uint8Array {
  return Uint8Array.from({ length: 32 }, (_, i) => (fill + i) & 255);
}

function v2link(overrides: Record<string, string> = {}): { url: string; k: string; p: string } {
  const k = toBase64Url(bytes(1));
  const p = toBase64Url(bytes(101));
  const params = new URLSearchParams({
    v: "2", c: CODE, g: GATEWAY, d: DESKTOP, k, p, ...overrides,
  });
  return { url: `dragent://pair?${params.toString()}`, k, p };
}

test("v2 links parse every field", () => {
  const { url } = v2link();
  const parsed = parsePairLink(url);
  assert.equal(parsed.code, CODE);
  assert.equal(parsed.gateway, GATEWAY);
  assert.equal(parsed.desktop, DESKTOP);
  assert.equal(parsed.desktopKey.length, 32);
  assert.deepEqual(parsed.desktopKey, bytes(1));
  assert.equal(parsed.secretP.length, 32);
  assert.deepEqual(parsed.secretP, bytes(101));
  wipeBytes(parsed.secretP);
});

test("bare codes are impossible", () => {
  for (const raw of [CODE, CODE.toLowerCase(), `  ${CODE}  `]) {
    assert.throws(() => parsePairLink(raw), /full link/i, raw);
  }
});

test("links without every v2 field are refused", () => {
  const { url } = v2link();
  const drop = (key: string) => {
    const u = new URL(url);
    u.searchParams.delete(key);
    return u.toString();
  };
  for (const key of ["v", "c", "g", "d", "k", "p"]) {
    assert.throws(() => parsePairLink(drop(key)), /QR|link|pair/i, key);
  }
  // Stale (v1) links: update the desktop, don't paste a code.
  const stale = new URL(url);
  stale.searchParams.set("v", "1");
  assert.throws(() => parsePairLink(stale.toString()), /older Dragent|update/i);
  // Off-alphabet keys and non-UUID desktops are refused.
  const badKey = new URL(url);
  badKey.searchParams.set("k", "!!!not-base64!!!");
  assert.throws(() => parsePairLink(badKey.toString()), /security details/i);
  const badDesktop = new URL(url);
  badDesktop.searchParams.set("d", "not-a-uuid");
  assert.throws(() => parsePairLink(badDesktop.toString()), /security details/i);
  // Non-pair schemes and share links are refused.
  assert.throws(() => parsePairLink("dragent://join?session=abc"), /pairing code/i);
  assert.throws(() => parsePairLink("https://example.test/pair"), /pairing code/i);
});

test("pairing gateways require https except loopback dev", () => {
  assert.equal(normalizePairingGateway("https://api.example.test"), "https://api.example.test");
  assert.equal(normalizePairingGateway("wss://api.example.test/socket"), "https://api.example.test");
  assert.throws(() => normalizePairingGateway("http://api.example.test"), /HTTPS/);
  assert.equal(normalizePairingGateway("http://127.0.0.1:8080", true), "http://127.0.0.1:8080");
  assert.throws(() => normalizePairingGateway("http://127.0.0.1:8080"), /HTTPS/);
  assert.throws(() => normalizePairingGateway("https://api.example.test/?code=x"), /gateway/);
});

test("codes are 20 chars of base32", () => {
  assert.equal(normalizePairingCode("abcdefgh234567abcdef"), "ABCDEFGH234567ABCDEF");
  for (const bad of ["short", "abcdefgh234567abcdef0!", "abcdefgh0189ijklmnop", "abcdefgh2345678"]) {
    assert.throws(() => normalizePairingCode(bad), /pairing code/i, bad);
  }
});

test("device names stay bounded (they ride the encrypted payload, never the redeem)", () => {
  assert.equal(normalizeDeviceName("  My  phone "), "My phone");
  assert.throws(() => normalizeDeviceName("   "), /Name this phone/);
  assert.throws(() => normalizeDeviceName("x".repeat(65)), /64/);
});

test("pending links live in memory: take transfers ownership, clear wipes", () => {
  clearPendingPairLink();
  assert.equal(peekPendingPairLink(), null);
  const first = parsePairLink(v2link().url);
  setPendingPairLink(first);
  assert.equal(peekPendingPairLink(), first);
  const taken = takePendingPairLink();
  assert.equal(taken, first);
  assert.equal(peekPendingPairLink(), null);
  // A new link wipes the previous secret.
  const second = parsePairLink(v2link().url);
  setPendingPairLink(second);
  setPendingPairLink(parsePairLink(v2link().url));
  assert.deepEqual(second.secretP, new Uint8Array(32));
  clearPendingPairLink();
  assert.equal(peekPendingPairLink(), null);
  wipeBytes(first.secretP);
});

test("p appears in no persisted navigation state", () => {
  (globalThis as { __DEV__?: boolean }).__DEV__ = true;
  clearPendingPairLink();
  const { url, k, p } = v2link();
  // Deep links arrive absolute (dragent://pair?...); only a bare route
  // enters navigation — never the link's secrets.
  const route = redirectSystemPath({ path: url, initial: true });
  assert.equal(route, "/pair-confirm");
  // What navigation persists (the route string, serialized) carries no secret.
  const persisted = JSON.stringify({ route });
  assert.ok(!persisted.includes(p), "route leaks P");
  assert.ok(!persisted.includes(k), "route leaks the desktop key");
  assert.ok(!persisted.includes(DESKTOP), "route leaks the desktop id");
  assert.ok(!persisted.includes(CODE), "route leaks the code");
  // The secret made it through memory instead.
  const pending = takePendingPairLink();
  assert.ok(pending);
  assert.deepEqual(pending.secretP, Uint8Array.from({ length: 32 }, (_, i) => (101 + i) & 255));
  wipeBytes(pending.secretP);
  // Garbage links land on the scan screen with nothing pending.
  assert.equal(redirectSystemPath({ path: "pair?c=short", initial: true }), "/pair-scan");
  assert.equal(peekPendingPairLink(), null);
  clearPendingPairLink();
});

test("only Dragent's own servers are trusted without asking", () => {
  assert.equal(isTrustedGateway("https://gateway.dragent.dev"), true);
  assert.equal(isTrustedGateway("https://dragent.dev"), true);
  assert.equal(isTrustedGateway("https://evil.example.com"), false);
  assert.equal(isTrustedGateway("https://dragent.dev.evil.example.com"), false);
  assert.equal(isTrustedGateway("https://notdragent.dev"), false);
  assert.equal(isTrustedGateway("http://gateway.dragent.dev"), false);
  assert.equal(isTrustedGateway("http://localhost:8080"), false);
  assert.equal(isTrustedGateway("http://localhost:8080", true), true);
});
