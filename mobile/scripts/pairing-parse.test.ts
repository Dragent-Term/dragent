import assert from "node:assert/strict";
import { test } from "node:test";
import {
  normalizeDeviceName,
  normalizePairingCode,
  normalizePairingGateway,
  parsePairLink,
  wipeBytes,
} from "../src/pairing/pairing";
import { toBase64Url } from "../src/transport/e2ee/ctrl";

const CODE = "abcdefgh234567abcdef";
const GATEWAY = "https://api.example.test";
const DESKTOP = "0a0b0c0d-0000-4000-8000-0000000000d1";

function bytes(fill: number): Uint8Array {
  return Uint8Array.from({ length: 32 }, (_, i) => (fill + i) & 255);
}

function v2link(): string {
  const params = new URLSearchParams({
    v: "2",
    c: CODE,
    g: GATEWAY,
    d: DESKTOP,
    k: toBase64Url(bytes(1)),
    p: toBase64Url(bytes(101)),
  });
  return `dragent://pair?${params.toString()}`;
}

test("v2 pair links parse code, gateway, desktop, key and secret", () => {
  const parsed = parsePairLink(v2link());
  assert.equal(parsed.code, "ABCDEFGH234567ABCDEF");
  assert.equal(parsed.gateway, "https://api.example.test");
  assert.equal(parsed.desktop, DESKTOP);
  assert.deepEqual(parsed.desktopKey, bytes(1));
  assert.deepEqual(parsed.secretP, bytes(101));
  wipeBytes(parsed.secretP);
});

test("pair links reject share links and non-pair schemes", () => {
  const url = new URL(v2link());
  for (const raw of [
    "dragent://join?session=abc&secret=def",
    "https://example.test/join?session=abc",
    (() => { url.searchParams.set("c", "short"); return url.toString(); })(),
    (() => { const u = new URL(v2link()); u.searchParams.delete("k"); return u.toString(); })(),
    (() => { const u = new URL(v2link()); u.searchParams.set("v", "1"); return u.toString(); })(),
  ]) {
    assert.throws(() => parsePairLink(raw), /pair|QR|link|Dragent/i, raw);
  }
});

test("bare codes are refused: only the full link pairs", () => {
  for (const raw of ["abcdefgh234567abcdef", "  ABCDEFGH234567ABCDEF "]) {
    assert.throws(() => parsePairLink(raw), /full link/i, raw);
  }
});

test("codes are 20 chars of base32", () => {
  assert.equal(normalizePairingCode("abcdefgh234567abcdef"), "ABCDEFGH234567ABCDEF");
  for (const bad of ["short", "abcdefgh234567abcdef0!", "abcdefgh0189ijklmnop", "abcdefgh2345678"]) {
    assert.throws(() => normalizePairingCode(bad), /pairing code/i, bad);
  }
});

test("pairing gateways require https except loopback dev", () => {
  assert.equal(normalizePairingGateway("https://api.example.test"), "https://api.example.test");
  assert.equal(normalizePairingGateway("wss://api.example.test/socket"), "https://api.example.test");
  assert.throws(() => normalizePairingGateway("http://api.example.test"), /HTTPS/);
  assert.equal(normalizePairingGateway("http://127.0.0.1:8080", true), "http://127.0.0.1:8080");
  assert.throws(() => normalizePairingGateway("http://127.0.0.1:8080"), /HTTPS/);
  assert.throws(() => normalizePairingGateway("https://api.example.test/?code=x"), /gateway/);
});

test("device names are bounded and required", () => {
  assert.equal(normalizeDeviceName("  My  phone "), "My phone");
  assert.throws(() => normalizeDeviceName("   "), /Name this phone/);
  assert.throws(() => normalizeDeviceName("x".repeat(65)), /64/);
});
