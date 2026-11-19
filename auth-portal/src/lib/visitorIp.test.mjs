// Run with: node --experimental-strip-types --test src/lib/visitorIp.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { visitorIP } from "./visitorIp.ts";

const ip = (headers) => visitorIP(new Headers(headers));

test("the address CloudFront appended is the visitor", () => {
  assert.equal(ip({ "x-forwarded-for": "203.0.113.9" }), "203.0.113.9");
});

test("entries the caller sent before CloudFront's are ignored", () => {
  assert.equal(ip({ "x-forwarded-for": "1.2.3.4, 5.6.7.8, 203.0.113.9" }), "203.0.113.9");
});

test("hops after the visitor (CloudFront origin-facing, private) are skipped", () => {
  assert.equal(ip({ "x-forwarded-for": "1.2.3.4, 203.0.113.9, 130.176.88.10" }), "203.0.113.9");
  assert.equal(ip({ "x-forwarded-for": "203.0.113.9, 15.158.1.1, 10.0.0.4" }), "203.0.113.9");
  assert.equal(ip({ "x-forwarded-for": "2001:db8:1::5, 172.20.0.1" }), "2001:db8:1::5");
});

test("cf-connecting-ip and other caller-set headers are never trusted", () => {
  assert.equal(
    ip({
      "x-forwarded-for": "203.0.113.9",
      "cf-connecting-ip": "198.51.100.1",
      "x-real-ip": "198.51.100.2",
      "true-client-ip": "198.51.100.3",
      "cloudfront-viewer-address": "198.51.100.4:443",
    }),
    "203.0.113.9",
  );
});

test("rotating spoofed entries can't change the visitor", () => {
  const seen = new Set();
  for (let i = 1; i <= 20; i++) {
    seen.add(ip({ "x-forwarded-for": `198.51.100.${i}, 203.0.113.9`, "cf-connecting-ip": `192.0.2.${i}` }));
  }
  assert.deepEqual([...seen], ["203.0.113.9"]);
});

test("ports, brackets and mapped IPv4 are normalised", () => {
  assert.equal(ip({ "x-forwarded-for": "203.0.113.9:4431" }), "203.0.113.9");
  assert.equal(ip({ "x-forwarded-for": "[2001:db8::7]:443" }), "2001:db8::7");
  assert.equal(ip({ "x-forwarded-for": "::ffff:203.0.113.9" }), "203.0.113.9");
});

test("no public address gives empty (API falls back to the portal's own)", () => {
  assert.equal(ip({}), "");
  assert.equal(ip({ "x-forwarded-for": "10.0.0.1, garbage, 127.0.0.1" }), "");
});
