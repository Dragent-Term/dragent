import assert from "node:assert/strict";
import { test } from "node:test";
import { removeDesktop, type RemoveDesktopDeps } from "../src/features/settings/removeDesktop";

function deps(overrides: Partial<RemoveDesktopDeps> = {}): { deps: RemoveDesktopDeps; calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    deps: {
      pairedDesktops: () => ["desk-a", "desk-b"],
      unpairDesktop: async (desktop) => {
        calls.push(`unpair:${desktop}`);
      },
      revokePhoneDevice: async () => {
        calls.push("revoke");
      },
      endCompanionConnections: async () => {
        calls.push("drain");
      },
      clearLabel: async () => {
        calls.push("label");
      },
      forgetPairing: async () => {
        calls.push("forget");
      },
      ...overrides,
    },
  };
}

test("remove unpairs every desktop locally before touching the server", async () => {
  const { deps: d, calls } = deps();
  await removeDesktop(d);
  assert.deepEqual(calls, ["unpair:desk-a", "unpair:desk-b", "revoke", "drain", "label", "forget"]);
});

test("a failing unpair or revoke never blocks local clearing", async () => {
  const { deps: d, calls } = deps({
    unpairDesktop: async (desktop) => {
      calls.push(`unpair:${desktop}`);
      throw new Error("E2EE_STORE_LOST");
    },
    revokePhoneDevice: async () => {
      throw new Error("offline");
    },
  });
  await removeDesktop(d);
  assert.deepEqual(calls, ["unpair:desk-a", "unpair:desk-b", "drain", "label", "forget"]);
});

test("no E2EE store still revokes and forgets", async () => {
  const { deps: d, calls } = deps({
    pairedDesktops: () => {
      throw new Error("E2EE_UNSUPPORTED_PLATFORM");
    },
  });
  await removeDesktop(d);
  assert.deepEqual(calls, ["revoke", "drain", "label", "forget"]);
});
