import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clientTokenFor,
  companionOwnerKey,
  isCompanionEligible,
  selectOwnedCompanion,
} from "../src/terminal/companion/owner";

const base = {
  accountId: "a1",
  deviceId: "d1",
  tabId: 7,
  epoch: 1,
  retry: 0,
  clientToken: "client:1",
};

test("owner key changes on any identity field; stable otherwise", () => {
  const key = companionOwnerKey(base);
  assert.equal(companionOwnerKey({ ...base }), key);
  assert.notEqual(companionOwnerKey({ ...base, accountId: "a2" }), key);
  assert.notEqual(companionOwnerKey({ ...base, accountId: null }), key);
  assert.notEqual(companionOwnerKey({ ...base, deviceId: "d2" }), key);
  assert.notEqual(companionOwnerKey({ ...base, tabId: 8 }), key);
  assert.notEqual(companionOwnerKey({ ...base, epoch: 2 }), key);
  assert.notEqual(companionOwnerKey({ ...base, retry: 1 }), key);
  assert.notEqual(companionOwnerKey({ ...base, clientToken: "client:2" }), key);
});

test("client tokens are stable per identity and distinct per client", () => {
  assert.equal(clientTokenFor(null), "none");
  assert.equal(clientTokenFor(undefined), "none");
  const a = {};
  const b = {};
  const first = clientTokenFor(a);
  assert.equal(clientTokenFor(a), first);
  assert.notEqual(clientTokenFor(b), first);
});

test("eligibility requires every gate; no partial credit", () => {
  const eligible = {
    foreground: true,
    socketReady: true,
    leaseActive: true,
    online: true,
    rpcAvailable: true,
    tabValid: true,
  };
  assert.equal(isCompanionEligible(eligible), true);
  for (const field of Object.keys(eligible) as Array<keyof typeof eligible>) {
    assert.equal(isCompanionEligible({ ...eligible, [field]: false }), false, field);
  }
});

test("keyed selector masks stale records synchronously during render", () => {
  const live = { phase: "live-raw" };
  const blank = { phase: "attaching" };
  assert.equal(selectOwnedCompanion("k1", "k1", live, blank), live);
  // Owner changed before passive effects: old record must not render.
  assert.equal(selectOwnedCompanion("k-old", "k-new", live, blank), blank);
});
