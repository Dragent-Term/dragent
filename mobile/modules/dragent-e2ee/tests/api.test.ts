import assert from "node:assert/strict";
import { test } from "node:test";

import { E2eeError, e2eeErrorCode, resolveE2ee, runBenchmarks } from "../src/api";
import type { NativeE2ee } from "../src/types";

const fake = { platform: "ios" } as unknown as NativeE2ee;

test("E2EE refuses to run on the web platform (§1.3, §12.7)", () => {
  let loaded = false;
  for (const os of ["web", "windows", "macos"]) {
    assert.throws(
      () => resolveE2ee(os, () => ((loaded = true), fake)),
      (e: unknown) => e instanceof E2eeError && e.code === "E2EE_UNSUPPORTED_PLATFORM",
    );
  }
  assert.equal(loaded, false, "the native module must not even be looked up on the web");
});

test("a missing native module is an error, never a fallback", () => {
  assert.throws(() => resolveE2ee("ios", () => null), (e: unknown) => e instanceof E2eeError && e.code === "E2EE_UNSUPPORTED");
  assert.equal(resolveE2ee("ios", () => fake), fake);
  assert.ok(resolveE2ee("android", () => fake), "android gets the module, adapted");
});

// Expo on Android rejects a Uint8Array nested in a record ("DynamicFromMap
// cannot be cast to byte[]"), which broke pairing; those fields go as arrays.
test("on Android, record byte fields reach native as number arrays", () => {
  const seen: unknown[] = [];
  const native = {
    startPairing: (...args: unknown[]) => { seen.push(args); return "pairing"; },
    startKk: (...args: unknown[]) => { seen.push(args); return "kk"; },
    encodeFrame: (frame: unknown) => { seen.push(frame); return new Uint8Array([9]); },
    sha256(this: unknown, b: Uint8Array) { return this === native ? b : null; },
  } as unknown as NativeE2ee;
  const e2ee = resolveE2ee("android", () => native);
  const ids = { account: "a", phone: "p", desktop: "d", chan: new Uint8Array([1, 2, 255]) };
  const key = new Uint8Array([7]);
  assert.equal(e2ee.startPairing(ids, key, key, "Pixel"), "pairing");
  assert.deepEqual(seen[0], [{ account: "a", phone: "p", desktop: "d", chan: [1, 2, 255] }, key, key, "Pixel"]);
  assert.equal(e2ee.startKk(ids, key, ["x"]), "kk");
  assert.deepEqual((seen[1] as unknown[])[0], { account: "a", phone: "p", desktop: "d", chan: [1, 2, 255] });
  e2ee.encodeFrame({ kind: "data", step: 0, purpose: null, chan: new Uint8Array([3]), peer: null, ctr: 1, body: new Uint8Array([4, 5]) });
  assert.deepEqual(seen[2], { kind: "data", step: 0, purpose: null, chan: [3], peer: null, ctr: 1, body: [4, 5] });
  // Everything else is the native function, called on the module itself.
  assert.equal(e2ee.sha256(key), key);
  assert.equal(ids.chan instanceof Uint8Array, true, "the caller's bytes are left alone");
});

test("error codes are read from native exceptions", () => {
  assert.equal(e2eeErrorCode({ code: "E2EE_PROTOCOL_ERROR" }), "E2EE_PROTOCOL_ERROR");
  assert.equal(e2eeErrorCode(new E2eeError("E2EE_STORE_LOST", "x")), "E2EE_STORE_LOST");
  assert.equal(e2eeErrorCode({ code: "ERR_OTHER" }), null);
  assert.equal(e2eeErrorCode("E2EE_REJECTED"), null);
  assert.equal(e2eeErrorCode(null), null);
});

test("the JS benchmark adds JSI-side timings to the native ones", () => {
  let t = 0;
  const calls = { sha: 0, rec: 0 };
  const native = {
    nativeBenchmarks: (n: number) => [{ name: "seal_64b_record", iterations: n, microsPerOp: 2 }],
    sha256: () => ((calls.sha += 1), new Uint8Array(32)),
    encodeRecords: () => ((calls.rec += 1), [new Uint8Array(64)]),
  };
  const r = runBenchmarks(native, 10, () => (t += 1));
  assert.deepEqual(r.map((x) => x.name), ["seal_64b_record", "jsi_call_64b", "jsi_encode_keystroke_record"]);
  assert.deepEqual(calls, { sha: 10, rec: 10 });
  assert.equal(r[1]?.microsPerOp, 100); // 1 ms of fake clock over 10 calls
});
