// Run with: node --experimental-strip-types --test src/lib/desktopHandoff.test.mjs
import assert from "node:assert/strict";
import { test } from "node:test";
import { appDeepLink, appScheme } from "./desktopHandoff.ts";

test("the installed app and a Dev build are allowed", () => {
  assert.equal(appScheme("dragent"), "dragent");
  assert.equal(appScheme("dragent-dev"), "dragent-dev");
});

test("anything else falls back to dragent", () => {
  for (const raw of [
    null,
    undefined,
    "",
    "evilapp",
    "javascript",
    "Dragent-Dev",
    "DRAGENT",
    "dragent-dev:",
    "dragent-dev://x",
    " dragent-dev",
    "dragent-devx",
    "https",
    ["dragent-dev"],
    42,
  ]) {
    assert.equal(appScheme(raw), "dragent", String(raw));
  }
});

test("the hand-off link goes only to an allowed scheme", () => {
  const h = { state: "s1", code: "c1", user_id: "u1" };
  assert.equal(appDeepLink({ ...h, scheme: "dragent-dev" }), "dragent-dev://auth/callback?code=c1&state=s1&user_id=u1");
  assert.equal(appDeepLink({ ...h }), "dragent://auth/callback?code=c1&state=s1&user_id=u1");
  assert.ok(appDeepLink({ ...h, scheme: "javascript" }).startsWith("dragent://auth/callback?"));
  assert.ok(appDeepLink({ ...h, scheme: "evilapp" }).startsWith("dragent://auth/callback?"));
});
