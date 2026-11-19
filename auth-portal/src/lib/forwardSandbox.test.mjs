// Run with: node --experimental-strip-types --test src/lib/forwardSandbox.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  FORWARD_SANDBOX_POLICY,
  dedicatedForwardURL,
  forwardDomain,
  sandboxedForwardResponseHeaders,
  upstreamForwardHeaders,
} from "./forwardSandbox.ts";

test("sandbox policy never keeps the portal origin", () => {
  assert.equal(
    FORWARD_SANDBOX_POLICY,
    "sandbox allow-scripts allow-forms allow-popups allow-modals allow-downloads",
  );
  assert.ok(!FORWARD_SANDBOX_POLICY.includes("allow-same-origin"));
});

test("forwarded responses are sandboxed and cannot plant portal state", () => {
  const upstream = new Headers([
    ["content-type", "text/html"],
    ["content-security-policy", "sandbox allow-scripts allow-same-origin"],
    ["content-security-policy-report-only", "default-src *"],
    ["set-cookie", "token=stolen; Path=/"],
    ["set-cookie", "__Host-dragent_forward=other; Path=/; Secure"],
    ["service-worker-allowed", "/"],
    ["access-control-allow-credentials", "true"],
    ["content-encoding", "gzip"],
    ["content-length", "12"],
    ["x-app", "kept"],
  ]);
  const out = sandboxedForwardResponseHeaders(upstream);
  assert.equal(out.get("content-security-policy"), FORWARD_SANDBOX_POLICY);
  for (const name of [
    "content-security-policy-report-only",
    "set-cookie",
    "service-worker-allowed",
    "access-control-allow-credentials",
    "content-encoding",
    "content-length",
  ]) {
    assert.equal(out.get(name), null, name);
  }
  assert.equal(out.getSetCookie().length, 0);
  assert.equal(out.get("access-control-allow-origin"), "*");
  assert.equal(out.get("content-type"), "text/html");
  assert.equal(out.get("x-app"), "kept");
});

test("portal credentials never reach the forward; the secret travels as the gateway cookie", () => {
  const incoming = new Headers([
    ["cookie", "portal_session=abc"],
    ["authorization", "Bearer portal-token"],
    ["host", "portal.example"],
    ["accept", "text/html"],
  ]);
  const out = upstreamForwardHeaders(incoming, "s3cret");
  assert.equal(out.get("cookie"), "__Host-dragent_forward=s3cret");
  assert.equal(out.get("authorization"), null);
  assert.equal(out.get("host"), null);
  assert.equal(out.get("accept"), "text/html");
  assert.equal(out.get("accept-encoding"), "identity");
  assert.equal(upstreamForwardHeaders(incoming, null).get("cookie"), null);
});

test("forward domain is read from the environment and validated", () => {
  assert.equal(forwardDomain({}), "");
  assert.equal(forwardDomain({ DRAGENT_FORWARD_DOMAIN: " Dragent-Preview.dev. " }), "dragent-preview.dev");
  assert.equal(forwardDomain({ NEXT_PUBLIC_FORWARD_DOMAIN: "preview.example" }), "preview.example");
  assert.equal(forwardDomain({ DRAGENT_FORWARD_DOMAIN: "evil.example/path" }), "");
  assert.equal(forwardDomain({ DRAGENT_FORWARD_DOMAIN: "localhost" }), "");
});

test("portal forward URLs redirect to the dedicated forward host", () => {
  assert.equal(
    dedicatedForwardURL("dragent-preview.dev", "calm-a1b2c3", ["@vite", "client"], "?secret=x"),
    "https://calm-a1b2c3.dragent-preview.dev/@vite/client?secret=x",
  );
  assert.equal(
    dedicatedForwardURL("dragent-preview.dev", "calm-a1b2c3", [], ""),
    "https://calm-a1b2c3.dragent-preview.dev/",
  );
  assert.equal(dedicatedForwardURL("", "calm-a1b2c3", [], ""), null);
  for (const id of ["evil.com#", "calm-A1B2C3", "a-b-a1b2c3", "x@y-a1b2c3"]) {
    assert.equal(dedicatedForwardURL("dragent-preview.dev", id, [], ""), null, id);
  }
});
