import assert from "node:assert/strict";
import { test } from "node:test";
import { readAgentAccounts } from "../../accounts/rpc";

test("the desktop's agent accounts read", () => {
  const agents = readAgentAccounts({
    agents: [{
      provider: "claude", name: "Claude Code", supported: true, reason: "",
      accounts: [
        { account: "system", label: "System login", system: true, in_use: false, status: "System login — managed by the CLI", plan: null, windows: [] },
        { account: "0b8e1f9e-7c2f-4d7b-9a51-6a3f0d8c2e11", label: "Work", system: false, in_use: true, status: "Signed in", plan: "max",
          windows: [{ label: "5-hour", used_percent: 130, resets_at: 1 }] },
      ],
    }],
  });
  assert.ok(agents);
  assert.equal(agents[0]!.accounts[1]!.inUse, true);
  assert.equal(agents[0]!.accounts[1]!.windows[0]!.usedPercent, 100, "clamped");
  assert.equal(readAgentAccounts({ agents: [{ provider: "claude", accounts: [{}] }] }), null);
});
